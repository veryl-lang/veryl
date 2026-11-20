//! Text of handle `string` variables.
//!
//! A `string` variable whose text is only known while the testbench runs
//! (it is not driven by `always_comb`/`always_ff`/`assign`) stores a 64-bit
//! handle instead of the text's bytes, so its storage has a fixed width.
//! Each distinct text has exactly one handle, so `==`/`!=` compare handles
//! as integers, and the empty text is handle `0` (the initial value).
//!
//! Handles are derived from the text (FNV-1a), so a handle baked into the IR
//! at conversion time stays valid in any simulation of the same process.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// Storage width of a handle `string` variable.
pub const HANDLE_WIDTH: usize = 64;

/// Conversion spec standing in for `%s` when its argument is a handle. A
/// private-use character, so no user format string contains it.
pub const HANDLE_SPEC: char = '\u{E000}';

#[derive(Default)]
struct Pool {
    by_text: HashMap<String, u64>,
    by_handle: HashMap<u64, String>,
}

static POOL: LazyLock<RwLock<Pool>> = LazyLock::new(Default::default);

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The handle of `text`, registering it on first use.
pub fn intern(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    if let Some(handle) = POOL.read().unwrap().by_text.get(text) {
        return *handle;
    }
    let mut pool = POOL.write().unwrap();
    if let Some(handle) = pool.by_text.get(text) {
        return *handle;
    }
    // Probe past `0` and any handle already taken by another text.
    let mut handle = fnv1a(text);
    while handle == 0 || pool.by_handle.contains_key(&handle) {
        handle = handle.wrapping_add(1);
    }
    pool.by_text.insert(text.to_string(), handle);
    pool.by_handle.insert(handle, text.to_string());
    handle
}

/// The text of `handle`; empty for `0` or a value that is no handle.
pub fn text(handle: u64) -> String {
    if handle == 0 {
        return String::new();
    }
    POOL.read()
        .unwrap()
        .by_handle
        .get(&handle)
        .cloned()
        .unwrap_or_default()
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
}
