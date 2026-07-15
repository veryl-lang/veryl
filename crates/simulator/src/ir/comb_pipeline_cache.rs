//! Cross-test cache for the whole comb pipeline.
//!
//! `ProtoModule::conv` runs the top module's merged comb list through
//! `analyze_dependency` → `reorder_by_level` → `dce_aggressive` → dead-var DCE
//! → `try_jit_no_cache`. For a `veryl test` suite that instantiates one shared
//! DUT per test, that whole chain is redundant work: every test tops out with a
//! byte-identical comb list (same statements, same absolute offsets — cross-test
//! relocation delta is zero) and identical event statements. The lower-level
//! chunk cache already elides the JIT *compile*, but the sort, DCE, and chunk
//! grouping walk still rerun per test.
//!
//! This cache memoises the entire chain, keyed by a structural fingerprint of
//! the comb list plus a digest of the event statements and DCE protect set (the
//! two inputs, besides the comb list, that dead-var DCE depends on). A hit
//! returns the pre-JIT stmts, the pass count, the compiled `ProtoStatements`,
//! and the DCE dead-offset set, so the caller reproduces the exact miss-path
//! result — including the in-place dead-var drop on the event statements.
//!
//! Single-flight (like `backend::inst`'s `GLOBAL_STMT_CACHE`): the first test to
//! reach a key computes while parallel peers block on the condvar, so a fleet of
//! tests launched together share one compute instead of all missing at once.
//!
//! Gated to `config.dut_reuse` (CLI only). Content-keyed, so — unlike a pointer
//! key — it is immune to `air::Ir` address reuse across a process.

use super::statement::{ProtoStatement, ProtoStatements};
use super::variable::VarOffset;
use crate::HashMap;
use std::sync::{Arc, Condvar, LazyLock, Mutex};

/// Memoised result of the whole comb pipeline for one comb-list key.
pub struct CombPipeline {
    /// Post-sort, post-DCE comb stmts (the `pre_jit_stmts` snapshot). `Arc` so a
    /// hit shares it (read-only downstream) instead of deep-cloning ~10^5 stmts
    /// per test — the dominant per-test cost the cache would otherwise add.
    pub pre_jit_stmts: Arc<Vec<ProtoStatement>>,
    pub required_comb_passes: usize,
    /// Compiled comb (`try_jit_no_cache` output). Its chunk artifacts bake in
    /// absolute offsets, valid verbatim because a key match implies an
    /// identical comb layout (delta = 0). Mostly `Compiled(Arc)` blocks, so a
    /// hit's clone is a handful of `Arc::clone`s, not a deep copy.
    pub comb_statements: ProtoStatements,
    /// Dead offsets dropped by dead-var DCE. Re-applied to the caller's event
    /// statements on a hit so they match the miss path exactly.
    pub dead_offsets: Vec<VarOffset>,
    /// Non-trivial SCC count (debug/test-only diagnostic; 0 in release).
    pub nontrivial_comb_scc: usize,
}

enum Slot {
    Computing,
    Done(Arc<CombPipeline>),
}

static CACHE: LazyLock<Mutex<HashMap<u128, Slot>>> =
    LazyLock::new(|| Mutex::new(HashMap::default()));
static CV: LazyLock<Condvar> = LazyLock::new(Condvar::new);

/// Outcome of consulting the cache for one comb-list key.
pub enum Outcome {
    /// Hit — the memoised pipeline, shared via `Arc` (no deep clone).
    Hit(Arc<CombPipeline>),
    /// Miss, claimed (single-flight): compute the pipeline then `guard.store`.
    /// Dropping the guard without storing (e.g. a conv error) releases the
    /// claim so blocked peers retry.
    Compute(Claim),
    /// Reuse disabled: compute inline, don't cache.
    Disabled,
}

/// Single-flight claim on a key's slot. `store` publishes the result; `Drop`
/// releases an unfulfilled claim.
pub struct Claim {
    key: u128,
    fulfilled: bool,
}

impl Claim {
    /// Publish `result`, returning it as the shared `Arc` (so the computing
    /// thread uses the same allocation it just cached, no extra clone).
    pub fn store(mut self, result: CombPipeline) -> Arc<CombPipeline> {
        let entry = Arc::new(result);
        let mut cache = CACHE.lock().unwrap();
        cache.insert(self.key, Slot::Done(Arc::clone(&entry)));
        self.fulfilled = true;
        CV.notify_all();
        entry
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if !self.fulfilled {
            let mut cache = CACHE.lock().unwrap();
            cache.remove(&self.key);
            CV.notify_all();
        }
    }
}

/// Consult the cache. On a hit, clone out the memoised pipeline. On a miss,
/// claim the slot single-flight: peers requesting the same key block until this
/// thread publishes via the returned guard.
pub fn try_get_or_claim(key: u128, dut_reuse: bool) -> Outcome {
    if !dut_reuse {
        return Outcome::Disabled;
    }
    let mut cache = CACHE.lock().unwrap();
    loop {
        match cache.get(&key) {
            Some(Slot::Done(entry)) => {
                let entry = Arc::clone(entry);
                drop(cache);
                return Outcome::Hit(entry);
            }
            Some(Slot::Computing) => {
                cache = CV.wait(cache).unwrap();
            }
            None => {
                cache.insert(key, Slot::Computing);
                return Outcome::Compute(Claim {
                    key,
                    fulfilled: false,
                });
            }
        }
    }
}
