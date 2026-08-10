//! Which engine a run actually used, as opposed to the one it asked for.
//!
//! A whole-comb / whole-event handle exists as soon as the emit is accepted,
//! but its artifact lands asynchronously (`cc` runs off the critical path).
//! Until then every dispatch returns `NotReady` and the module falls back to
//! the per-chunk path — correct, but a different engine, and `--format json`
//! names the *requested* backend, so a run can report `cc` while timing the
//! fallback.  What is recorded here surfaces as its `degraded_modules`.
//!
//! A module that never had an artifact to wait for (emit declined, below the
//! AOT size threshold) holds no whole-* handle, so nothing is recorded for it.

use crate::HashSet;
use std::sync::{Mutex, OnceLock};

fn seen() -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::default()))
}

/// `kind` is the dispatch site (`"whole_comb"` / `"whole_event"`).  Repeats
/// collapse to one entry per (kind, module).
pub fn record_fallback(kind: &str, module: &str) {
    let Ok(mut seen) = seen().lock() else { return };
    seen.insert(format!("{kind}:{module}"));
}

/// Every `kind:module` that fell back, sorted.  Empty means the run used the
/// engine it asked for throughout.
pub fn degraded_modules() -> Vec<String> {
    let Ok(seen) = seen().lock() else {
        return Vec::new();
    };
    let mut out: Vec<String> = seen.iter().cloned().collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallbacks_are_recorded_once_and_sorted() {
        record_fallback("whole_comb", "resid_test_b");
        record_fallback("whole_event", "resid_test_a");
        record_fallback("whole_comb", "resid_test_b");
        let listed: Vec<String> = degraded_modules()
            .into_iter()
            .filter(|s| s.contains("resid_test_"))
            .collect();
        assert_eq!(
            listed,
            ["whole_comb:resid_test_b", "whole_event:resid_test_a"],
            "one entry per (kind, module), sorted"
        );
    }
}
