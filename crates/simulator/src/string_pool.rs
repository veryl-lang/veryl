//! Storage of runtime `string` variables.
//!
//! A runtime `string` variable stores a 64-bit handle of its text instead
//! of the text's bytes, so its storage has a fixed width whatever text it
//! holds. Each distinct text has exactly one handle, so `==`/`!=` compare
//! handles as integers, and the empty text is handle `0`.
//!
//! The pool keeps the text as raw bytes, as a string literal spells them
//! (an escape can make them other than UTF-8), so its bits read back
//! exactly.
//!
//! Handles are derived from the bytes (FNV-1a), so a handle baked into the
//! IR at conversion time stays valid in any simulation of the same process.

use num_bigint::BigUint;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use veryl_analyzer::value::Value;

/// Storage width of a `string` variable.
pub const HANDLE_WIDTH: usize = 64;

/// Conversion spec standing in for `%s` when its argument is a handle. A
/// private-use character, so no user format string contains it.
pub const HANDLE_SPEC: char = '\u{E000}';

#[derive(Default)]
struct Pool {
    by_bytes: HashMap<Vec<u8>, u64>,
    by_handle: HashMap<u64, Vec<u8>>,
}

static POOL: LazyLock<RwLock<Pool>> = LazyLock::new(Default::default);

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The handle of `bytes`, registering them on first use.
pub fn intern_bytes(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return 0;
    }
    if let Some(handle) = POOL.read().unwrap().by_bytes.get(bytes) {
        return *handle;
    }
    let mut pool = POOL.write().unwrap();
    if let Some(handle) = pool.by_bytes.get(bytes) {
        return *handle;
    }
    // Probe past `0` and any handle already taken by other bytes.
    let mut handle = fnv1a(bytes);
    while handle == 0 || pool.by_handle.contains_key(&handle) {
        handle = handle.wrapping_add(1);
    }
    pool.by_bytes.insert(bytes.to_vec(), handle);
    pool.by_handle.insert(handle, bytes.to_vec());
    handle
}

/// The handle of `text`.
pub fn intern(text: &str) -> u64 {
    intern_bytes(text.as_bytes())
}

/// The bytes of `handle`; empty for `0` or a value that is no handle.
pub fn bytes(handle: u64) -> Vec<u8> {
    if handle == 0 {
        return Vec::new();
    }
    POOL.read()
        .unwrap()
        .by_handle
        .get(&handle)
        .cloned()
        .unwrap_or_default()
}

/// The text of `handle`, as `%s` prints it.
pub fn text(handle: u64) -> String {
    String::from_utf8_lossy(&bytes(handle)).into_owned()
}

/// The bits of the text of `handle` as a `width`-bit value: its bytes most
/// significant first, zero-padded above and cut to the low `width` bits.
pub fn to_value(handle: u64, width: usize) -> Value {
    let bytes = bytes(handle);
    if width <= 64 {
        let mut payload: u64 = 0;
        for b in &bytes {
            payload = (payload << 8) | *b as u64;
        }
        if width < 64 {
            payload &= (1u64 << width) - 1;
        }
        Value::new(payload, width, false)
    } else {
        let mut big = BigUint::from_bytes_be(&bytes);
        if big.bits() as usize > width {
            big &= (BigUint::from(1u8) << width) - 1u8;
        }
        Value::new_biguint(big, width, false)
    }
}

/// The handle of the text a `width`-bit value holds: its bytes, most
/// significant first, without the leading zero bytes that pad a shorter
/// text.
pub fn from_value(value: &Value, width: usize) -> u64 {
    let num_bytes = width.div_ceil(8);
    let mut bytes = vec![0u8; num_bytes];
    match value {
        Value::U64(x) => {
            let mut payload = x.payload;
            for i in (0..num_bytes).rev() {
                bytes[i] = (payload & 0xff) as u8;
                payload >>= 8;
            }
        }
        Value::BigUint(x) => {
            let le = x.payload().to_bytes_le();
            for (i, b) in le.iter().take(num_bytes).enumerate() {
                bytes[num_bytes - 1 - i] = *b;
            }
        }
    }
    let start = bytes.iter().position(|x| *x != 0).unwrap_or(bytes.len());
    intern_bytes(&bytes[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_and_text() {
        assert_eq!(intern(""), 0);
        let a = intern("string_pool_test_a");
        let b = intern("string_pool_test_b");
        assert_ne!(a, 0);
        assert_ne!(a, b);
        assert_eq!(intern("string_pool_test_a"), a);
        assert_eq!(text(a), "string_pool_test_a");
        assert_eq!(text(0), "");
    }

    #[test]
    fn value_round_trip() {
        let h = intern("hi");
        assert_eq!(to_value(h, 32), Value::new(0x6869, 32, false));
        assert_eq!(from_value(&Value::new(0x6869, 32, false), 32), h);
        assert_eq!(to_value(0, 16), Value::new(0, 16, false));
        assert_eq!(from_value(&Value::new(0, 16, false), 16), 0);

        // Bytes that are not UTF-8, and an inner zero byte, survive.
        let raw = from_value(&Value::new(0x00ff_0041, 24, false), 24);
        assert_eq!(bytes(raw), vec![0xff, 0x00, 0x41]);
        assert_eq!(to_value(raw, 24), Value::new(0x00ff_0041, 24, false));

        // Wider than 64 bits.
        let long = intern("0123456789");
        let v = to_value(long, 96);
        assert_eq!(from_value(&v, 96), long);
    }
}
