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

use crate::{HashMap, HashSet};
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

fn counts() -> &'static Mutex<HashMap<String, (u64, u64)>> {
    static COUNTS: OnceLock<Mutex<HashMap<String, (u64, u64)>>> = OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(HashMap::default()))
}

/// Publish an Ir's dispatch tallies.  Called once, when the Ir is dropped —
/// the dispatch path itself only touches a local relaxed atomic.
pub fn record_dispatch(kind: &str, module: &str, ran: u64, fell_back: u64) {
    if ran == 0 && fell_back == 0 {
        return; // no whole-* handle for this module: nothing to report
    }
    let Ok(mut counts) = counts().lock() else {
        return;
    };
    let e = counts.entry(format!("{kind}:{module}")).or_insert((0, 0));
    e.0 += ran;
    e.1 += fell_back;
}

/// `(kind:module, ran, fell_back)` per module that held a whole-* handle,
/// sorted.  This is the measure; [`degraded_modules`] is only its indicator.
pub fn dispatch_counts() -> Vec<(String, u64, u64)> {
    let Ok(counts) = counts().lock() else {
        return Vec::new();
    };
    let mut out: Vec<(String, u64, u64)> =
        counts.iter().map(|(k, v)| (k.clone(), v.0, v.1)).collect();
    out.sort();
    out
}

/// Every `kind:module` that fell back AT LEAST ONCE, sorted.  Empty is exact;
/// non-empty is not a measure -- one startup `NotReady` marks the whole run,
/// so use [`dispatch_counts`] for how much of it ran which engine.
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

    /// Must accumulate rather than latch, and must keep a `fell_back == 0`
    /// module -- neither of which `degraded_modules` can express.
    #[test]
    fn dispatch_counts_accumulate_and_keep_zero_fallbacks() {
        record_dispatch("whole_event", "disp_test_a", 100, 1);
        record_dispatch("whole_event", "disp_test_a", 50, 1);
        record_dispatch("whole_comb", "disp_test_b", 7, 0);
        // A module with no whole-* handle records nothing at all.
        record_dispatch("whole_comb", "disp_test_none", 0, 0);

        let got: Vec<(String, u64, u64)> = dispatch_counts()
            .into_iter()
            .filter(|(k, _, _)| k.contains("disp_test_"))
            .collect();
        assert_eq!(
            got,
            [
                ("whole_comb:disp_test_b".to_string(), 7, 0),
                ("whole_event:disp_test_a".to_string(), 150, 2),
            ],
            "counts must sum across publishes and drop the no-handle module"
        );
    }

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
