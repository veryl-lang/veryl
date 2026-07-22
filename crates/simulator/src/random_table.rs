//! Thread-local RNG table backing the `$tb::random` testbench handles.
//!
//! Like `file_table`, a thread-local keeps the generator state reachable from
//! the testbench driver (which has no `Simulator` handle) and isolates
//! parallel tests. Generators are keyed by the declaring variable's name
//! (`StrId`). Each handle is lazily seeded from the run's base seed (`--seed`
//! / `[test].seed`, i.e. `Ir.seed`) mixed with the handle name, so a run is
//! reproducible for a given seed and distinct handles get distinct streams.
//! `seed()` overrides the seed explicitly; `get_seed()` reads it back.

use crate::ir::Value;
use rand::{RngExt, SeedableRng};
use rand_pcg::Pcg64;
use std::cell::RefCell;
use std::collections::HashMap;
use veryl_parser::resource_table::{self, StrId};

#[derive(Default)]
struct RandomTable {
    /// Run base seed (`Ir.seed`); handles derive their seed from it.
    base_seed: u64,
    /// Per-handle generator paired with the seed currently applied to it.
    /// A non-cryptographic PCG generator is used (fast and reproducible; the
    /// testbench does not need a crypto-strength RNG).
    rngs: HashMap<StrId, (Pcg64, u64)>,
}

thread_local! {
    static TABLE: RefCell<RandomTable> = RefCell::new(RandomTable::default());
}

/// Deterministic FNV-1a over the base seed and the handle name; matches the
/// scheme used for per-instance component seeds.
fn derive_seed(base: u64, key: StrId) -> u64 {
    let name = resource_table::get_str_value(key).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&base.to_le_bytes());
    eat(name.as_bytes());
    h
}

/// Clear all generators and record the run base seed. Call before a test.
pub fn reset(base_seed: u64) {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.base_seed = base_seed;
        t.rngs.clear();
    });
}

fn with_rng<R>(key: StrId, f: impl FnOnce(&mut Pcg64) -> R) -> R {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        let base = t.base_seed;
        let (rng, _) = t.rngs.entry(key).or_insert_with(|| {
            let seed = derive_seed(base, key);
            (Pcg64::seed_from_u64(seed), seed)
        });
        f(rng)
    })
}

/// Set handle `key`'s seed explicitly and reset its stream.
pub fn seed_handle(key: StrId, seed: u64) {
    TABLE.with(|t| {
        t.borrow_mut()
            .rngs
            .insert(key, (Pcg64::seed_from_u64(seed), seed));
    });
}

/// Read back the seed currently applied to handle `key` (lazily seeding it
/// from the base seed if it has not been used yet).
pub fn get_seed_handle(key: StrId) -> u64 {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        let base = t.base_seed;
        t.rngs
            .entry(key)
            .or_insert_with(|| {
                let seed = derive_seed(base, key);
                (Pcg64::seed_from_u64(seed), seed)
            })
            .1
    })
}

fn mask(width: u32) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

/// Generate a uniform value across the full `width`-bit range of the element
/// type (`width <= 64`). The low `width` bits are sampled uniformly; for a
/// signed type this is the full set of two's-complement bit patterns.
pub fn get(key: StrId, width: u32, signed: bool) -> Value {
    let raw = with_rng(key, |rng| rng.random_range(0..=mask(width)));
    Value::new(raw, width as usize, signed)
}

/// Generate a value in the inclusive range `[min, max]`. `min`/`max` are the
/// raw `width`-bit payloads; for a signed type they are interpreted as
/// two's-complement before sampling. Uses `rand`'s uniform range sampler.
pub fn get_range(key: StrId, min: u64, max: u64, width: u32, signed: bool) -> Value {
    let m = mask(width);
    let raw = if signed {
        let a = sign_extend(min & m, width);
        let b = sign_extend(max & m, width);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let sample: i64 = with_rng(key, |rng| rng.random_range(lo..=hi));
        (sample as u64) & m
    } else {
        let a = min & m;
        let b = max & m;
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        with_rng(key, |rng| rng.random_range(lo..=hi))
    };
    Value::new(raw, width as usize, signed)
}

/// Interpret the low `width` bits of `raw` as a two's-complement integer.
fn sign_extend(raw: u64, width: u32) -> i64 {
    if width == 0 || width >= 64 {
        return raw as i64;
    }
    let shift = 64 - width;
    ((raw << shift) as i64) >> shift
}
