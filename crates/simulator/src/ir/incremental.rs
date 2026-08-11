//! Change-driven (incremental) comb settle — opt-in via `VERYL_INCR=1`.
//!
//! Motivation (measured on the pe_core DUT): only
//! ~0.4% of comb-buffer words change per cycle, and an ideal change-driven
//! scheduler would re-evaluate ~2% of comb statements per cycle, while the
//! baseline engine evaluates the full list every settle.  This module skips
//! the statements whose inputs did not change.
//!
//! Structure:
//! - At chunk-compile time (`registry::try_compile_chunk`) the full-coverage
//!   read/write sets of the compiled statements are captured as
//!   [`ChunkDeps`] on the artifact (offsets are content-absolute, same as
//!   the compiled code, so a cross-test cache hit stays consistent).
//! - At `ProtoModule::instantiate` a [`IncrPlan`] is built: one entry per
//!   runtime comb statement (batching disabled to keep 1:1 alignment), a
//!   word→consumers reverse index, and the set of comb words writable from
//!   outside the settle (event statements, top-level testbench variables).
//! - At runtime (`Ir::settle_comb_incremental`) dirtiness is seeded from
//!   FF-buffer / external-word diffs against the previous settle, propagated
//!   through the topologically-ordered entry list, and damped by diffing
//!   each entry's output words after it runs.
//!
//! Semantics vs the baseline: entries are pure functions of their input
//! words (side-effecting statements are `always_run`), so skipping an entry
//! whose inputs are unchanged is value-neutral.  Sweeps are capped at
//! `required_comb_passes` like the baseline's fixed pass count; an eager
//! within-sweep backward re-run can only converge earlier, never to a
//! different fixpoint, because the baseline pass count is chosen to reach
//! the fixpoint.

use crate::ir::statement::{
    ProtoStatement, ProtoStatementBlock, ProtoStatements, ProtoSystemFunctionCall, Statement,
};
use crate::ir::variable::{ModuleVariableMeta, VarOffset};
use std::env;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Set by [`force_disable`]; latches on and is never cleared, so the answer
/// [`enabled`] gives cannot change once any caller has acted on it.
static FORCED_OFF: AtomicBool = AtomicBool::new(false);

/// Set by [`default_on`]; consulted only when `VERYL_INCR` is unset.
static DEFAULT_ON: AtomicBool = AtomicBool::new(false);

/// `VERYL_INCR` decides when set (`1` on, `0` off); otherwise the caller's
/// default applies — on for the CLI, off for the library (see [`default_on`]).
pub fn enabled() -> bool {
    if FORCED_OFF.load(Ordering::Relaxed) {
        return false;
    }
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| match env::var("VERYL_INCR").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => DEFAULT_ON.load(Ordering::Relaxed),
    })
}

/// Make the incremental settle the default for this process.
///
/// The CLI calls this; the library default stays off so `cargo test` keeps
/// exercising the whole-module AOT-C path (`aot_size_ok = !incr_on` removes
/// the artifact under the incremental settle, so the coverage assertions in
/// `assert_aot_c_native` would otherwise never run).  Subject to the same
/// call-before-first-`enabled()` rule as [`force_disable`], which wins.
pub fn default_on() {
    DEFAULT_ON.store(true, Ordering::Relaxed);
}

/// Turn the incremental settle off for this process regardless of
/// `VERYL_INCR`.
///
/// Waveform dumping is the caller that needs this.  The plan maintains only
/// the comb words some later statement reads, so words nothing consumes keep
/// whatever they last held — invisible to the simulation (and to
/// `VERYL_INCR_VALIDATE`, which agrees word-for-word) but plainly visible in a
/// dump, where those signals then disagree with a full settle.  Must be called
/// before the first `enabled()` — i.e. before analysis, since the plan shapes
/// chunking — which for the dump case means at CLI startup.
pub fn force_disable() {
    FORCED_OFF.store(true, Ordering::Relaxed);
}

/// Runtime auto-abandon of the incremental plan (`VERYL_INCR_ABANDON`):
/// after the warmup window, if the observed entry-run fraction exceeds the
/// threshold (percent; default 33, `0` disables the check), the plan is
/// permanently abandoned — the settle falls back to the baseline full
/// sweep and the event skip is disabled.  A high-activity DUT (heliodor's
/// OoO SoC runs 44-90% of its entries every settle) pays the full
/// bookkeeping while skipping almost nothing, which measured 6-14x SLOWER
/// than the plain settle; abandoning caps that at the plan-less INCR
/// configuration (~2x).  Low-activity DUTs (pe_core ~4%, caches ~15-20%
/// at the K=8 incremental chunk size) stay well under the threshold.
pub fn abandon_threshold_pct() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| {
        env::var("VERYL_INCR_ABANDON")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(33)
    })
}

/// Auto-abandon warmup: fraction measured over settles
/// [`ABANDON_WARMUP`, `ABANDON_WARMUP + ABANDON_WINDOW`) — the first
/// settles are all-dirty / layout transients and would bias the ratio up.
pub const ABANDON_WARMUP: u64 = 512;
pub const ABANDON_WINDOW: u64 = 4096;

/// `VERYL_INCR_REBIND=<n>` (diagnostic): request a generation swap every
/// `n` settles, re-binding the same plan.  A swap is value-neutral by
/// construction — every id-indexed structure is rebuilt from the plan — so
/// running the existing suites under this knob exercises the swap path the
/// fused chunk space needs (contract v2 §1/§6) end to end.  0 = off.
/// Note it suppresses the auto-abandon when set below `ABANDON_WARMUP`:
/// the abandon window is generation-local and never completes.
pub fn rebind_interval() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| {
        env::var("VERYL_INCR_REBIND")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    })
}

pub fn validate_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| env::var("VERYL_INCR_VALIDATE").as_deref() == Ok("1"))
}

/// `VERYL_INCR_DIAG=1` — the one gate for every incremental diagnostic.
/// The settle is on by default, so anything printed unconditionally here
/// lands in every `veryl test` run's output.
pub fn diag_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| env::var("VERYL_INCR_DIAG").as_deref() == Ok("1"))
}

fn diag(reason: &str) {
    if diag_enabled() {
        eprintln!("[incr] plan disabled: {reason}");
    }
}

/// One read or write of a statement.  `bytes: Some(n)` is the dep's
/// native byte span, recorded at capture time for reads and writes alike —
/// a plan-build meta lookup cannot size meta-less storage (inlined
/// function / version-split temps), and the old 8-byte fallback silently
/// under-covered a wide temp's readers (a 512-bit cache-line temp lost 56
/// of its 64 bytes of wake coverage).  `bytes: None` survives only where
/// no span is known (Readmemh elements, deps-less CompiledBlock
/// fallbacks); the resolver then falls back to the variable meta, then to
/// 8 bytes.  `bits` narrows a dep to a static bit range of the variable
/// (`None` = potentially the whole span) — used to tell truly conflicting
/// (same-bit) writers apart from field-disjoint ones.
#[derive(Debug, Clone)]
pub struct Dep {
    pub off: VarOffset,
    pub bytes: Option<u32>,
    pub bits: Option<(u32, u32)>,
}

/// A written variable of one entry: offset key, covered word range, and the
/// written bit interval (`(0, u32::MAX)` = potentially whole span).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OutVar {
    pub key: u64,
    pub w0: u32,
    pub w1: u32,
    pub lo: u32,
    pub hi: u32,
}

/// Full-coverage dependency sets of a compiled chunk, captured at compile
/// time.  `always_run` marks chunks the plan must re-run every settle
/// (side effects / FF writes the diff cannot model).
#[derive(Debug, Default)]
pub struct ChunkDeps {
    pub ins: Vec<Dep>,
    pub outs: Vec<Dep>,
    pub always_run: bool,
    /// Why `always_run` is set (diagnostics): FF writes and/or side
    /// effects (system functions, testbench calls).
    pub always_ff: bool,
    pub always_side: bool,
    /// A statement whose write set cannot be modelled (e.g. `$readmemh`);
    /// the whole plan is abandoned when set.
    pub unmodelable: bool,
}

/// Walk one statement, appending full-coverage reads/writes.
///
/// Full coverage matters for correctness: a dynamically-indexed array write
/// records the whole array range (not the base+last encoding used by
/// `analyze_dependency`), so a middle-element change still triggers its
/// readers.
pub fn collect_stmt_deps(stmt: &ProtoStatement, deps: &mut ChunkDeps) {
    match stmt {
        ProtoStatement::Assign(x) => {
            let mut ins = Vec::new();
            x.expr.gather_reads_expanded_ranged(&mut ins);
            if let Some(ds) = &x.dynamic_select {
                ds.index_expr.gather_reads_expanded_ranged(&mut ins);
            }
            deps.ins.extend(ins.into_iter().map(|(off, r, nb)| Dep {
                off,
                bytes: Some(nb as u32),
                bits: r.map(|(a, b)| (a.min(b) as u32, a.max(b) as u32)),
            }));
            deps.outs.push(Dep {
                off: x.dst,
                // Precise 2-state payload span from the statement itself
                // (the store touches exactly these bytes); 4-state modules
                // are rejected by `build_plan`, so the mask plane never
                // needs covering here.
                bytes: Some(crate::ir::variable::native_bytes(x.dst_width) as u32),
                // A static bit-select write touches only that range; a
                // dynamic select may touch any bit.
                bits: if x.dynamic_select.is_some() {
                    None
                } else {
                    x.select.map(|(a, b)| (a.min(b) as u32, a.max(b) as u32))
                },
            });
            if x.dst.is_ff() {
                // FF writes go through the write log (NBA); skipping would
                // change log content, so keep the entry always-on.
                deps.always_run = true;
                deps.always_ff = true;
            }
        }
        ProtoStatement::AssignDynamic(x) => {
            let mut ins = Vec::new();
            x.dst_index_expr.gather_reads_expanded_ranged(&mut ins);
            x.expr.gather_reads_expanded_ranged(&mut ins);
            if let Some(ds) = &x.dynamic_select {
                ds.index_expr.gather_reads_expanded_ranged(&mut ins);
            }
            deps.ins.extend(ins.into_iter().map(|(off, r, nb)| Dep {
                off,
                bytes: Some(nb as u32),
                bits: r.map(|(a, b)| (a.min(b) as u32, a.max(b) as u32)),
            }));
            if x.dst_stride < 0 || x.dst_num_elements == 0 {
                deps.unmodelable = true;
                return;
            }
            // Full array range: any element may be written.  The stride is
            // at least the element's native width, so stride×n covers it.
            let span = x.dst_stride as usize * x.dst_num_elements;
            deps.outs.push(Dep {
                off: x.dst_base,
                bytes: Some(span as u32),
                bits: None,
            });
            if x.dst_base.is_ff() {
                deps.always_run = true;
                deps.always_ff = true;
            }
        }
        ProtoStatement::If(x) => {
            if let Some(cond) = &x.cond {
                let mut ins = Vec::new();
                cond.gather_reads_expanded_ranged(&mut ins);
                deps.ins.extend(ins.into_iter().map(|(off, r, nb)| Dep {
                    off,
                    bytes: Some(nb as u32),
                    bits: r.map(|(a, b)| (a.min(b) as u32, a.max(b) as u32)),
                }));
            }
            for s in &x.true_side {
                collect_stmt_deps(s, deps);
            }
            for s in &x.false_side {
                collect_stmt_deps(s, deps);
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &x.arms {
                let mut ins = Vec::new();
                arm.cond.gather_reads_expanded_ranged(&mut ins);
                deps.ins.extend(ins.into_iter().map(|(off, r, nb)| Dep {
                    off,
                    bytes: Some(nb as u32),
                    bits: r.map(|(a, b)| (a.min(b) as u32, a.max(b) as u32)),
                }));
                for s in &arm.body {
                    collect_stmt_deps(s, deps);
                }
            }
            for s in &x.default {
                collect_stmt_deps(s, deps);
            }
        }
        ProtoStatement::For(x) => {
            let mut ins = Vec::new();
            for e in x.range.dynamic_bounds() {
                e.gather_reads_expanded_ranged(&mut ins);
            }
            deps.ins.extend(ins.into_iter().map(|(off, r, nb)| Dep {
                off,
                bytes: Some(nb as u32),
                bits: r.map(|(a, b)| (a.min(b) as u32, a.max(b) as u32)),
            }));
            // The loop variable is written by the For itself.
            deps.outs.push(Dep {
                off: x.var_offset,
                bytes: Some(x.var_native_bytes as u32),
                bits: None,
            });
            for s in &x.body {
                collect_stmt_deps(s, deps);
            }
        }
        ProtoStatement::Break => {}
        ProtoStatement::SystemFunctionCall(x) => {
            deps.always_run = true;
            deps.always_side = true;
            if let ProtoSystemFunctionCall::Readmemh { elements, .. } = x {
                // Writes every element's current slot (testbench-driven
                // `$readmemh` can run mid-simulation).
                for elem in elements {
                    deps.outs.push(Dep {
                        off: elem.current,
                        bytes: None,
                        bits: None,
                    });
                }
            }
        }
        ProtoStatement::CompiledBlock(x) => {
            if !x.original_stmts.is_empty() {
                for s in &x.original_stmts {
                    collect_stmt_deps(s, deps);
                }
            } else {
                // Fall back to the analyzer-grade sets (base+last encoded
                // for arrays — slightly under-covered; validate mode and
                // golden runs guard this path).
                deps.ins.extend(x.input_offsets.iter().map(|&off| Dep {
                    off,
                    bytes: None,
                    bits: None,
                }));
                deps.outs.extend(x.output_offsets.iter().map(|&off| Dep {
                    off,
                    bytes: None,
                    bits: None,
                }));
                if x.output_offsets.iter().any(|o| o.is_ff()) {
                    deps.always_run = true;
                    deps.always_ff = true;
                }
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            // No internal-variable hiding here (unlike
            // `gather_variable_offsets`, which serves dependency ordering):
            // a word that is read before being written inside the block is
            // an external dependency for triggering purposes.  Keeping
            // written words in the read set only adds a benign
            // self-retrigger (idempotent at the fixpoint).
            for s in body {
                collect_stmt_deps(s, deps);
            }
        }
        ProtoStatement::TbMethodCall { .. } => {
            deps.always_run = true;
            deps.always_side = true;
        }
    }
}

/// Capture the dependency sets for one compiled chunk.
pub fn chunk_deps(stmts: &[ProtoStatement]) -> ChunkDeps {
    let mut deps = ChunkDeps::default();
    for s in stmts {
        collect_stmt_deps(s, &mut deps);
    }
    deps
}

/// `out_src` encoding: bits 0..30 = byte offset within the buffer,
/// bit 31 set = ff buffer (comb otherwise), bit 30 set = tail word (the
/// buffer ends mid-word; take the bounds-checked `read_word` path).
pub const OUT_SRC_FF: u32 = 1 << 31;
pub const OUT_SRC_TAIL: u32 = 1 << 30;

/// Byte-granular trigger filtering (`VERYL_INCR_BYTEMASK=0` opts out):
/// every consumer/writer reference carries the mask of the word's bytes it
/// actually reads/writes, and a change only wakes it when the changed
/// bytes intersect.  Variables are byte-aligned, so this separates packed
/// struct fields sharing a 64-bit word.
pub fn bytemask_enabled() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| env::var("VERYL_INCR_BYTEMASK").ok().as_deref() != Some("0"))
}

/// Per-byte non-zero → bit mask (bit `i` set iff byte `i` of `x` is
/// non-zero).  Branchless: high-bit-of-byte trick + multiply gather
/// (constant `Σ 2^(49-7i)` maps byte `i`'s bit `8i+7` to bit `56+i`,
/// collision-free).
#[inline(always)]
pub fn byte_nonzero_mask(x: u64) -> u8 {
    let l = x & 0x7f7f_7f7f_7f7f_7f7f;
    let nz = (l.wrapping_add(0x7f7f_7f7f_7f7f_7f7f) | x) & 0x8080_8080_8080_8080;
    (nz.wrapping_mul(0x0002_0408_1020_4081) >> 56) as u8
}

/// Change-driven settle plan.  Word-id space: comb word `w` → `w`,
/// ff word `w` → `comb_words + w`.
pub struct IncrPlan {
    pub comb_words: usize,
    pub total_words: usize,
    pub n_entries: usize,
    /// Entries that must run every sweep (side effects / FF writes).
    pub always_run: Vec<u32>,
    /// Output word ids of all entries, concatenated (diffed after the
    /// entry runs); entry `e` owns `out_words[entry_out_off[e] ..
    /// entry_out_off[e+1]]`.  Flat so the post-run diff — the hottest
    /// bookkeeping loop — walks one contiguous slice per entry.
    pub out_words: Vec<u32>,
    /// Parallel to `out_words`: pre-resolved read source (see `OUT_SRC_*`),
    /// so the diff loop is a raw unaligned load instead of a word-id
    /// decode + bounds check per word.
    pub out_src: Vec<u32>,
    /// entry → range of `out_words`/`out_src` (`n_entries + 1` fenceposts).
    pub entry_out_off: Vec<u32>,
    /// entry → later same-bit writers of the variables it writes; re-marked
    /// on every run of the entry ("clobber repair" for versioned variables).
    pub repair: Vec<Vec<u32>>,
    /// word id → entries reading it.
    pub consumers: Vec<Vec<u32>>,
    /// Parallel to `consumers`: byte mask of the word the entry reads; a
    /// change wakes the entry only when the changed bytes intersect.
    pub consumer_gmasks: Vec<Vec<u8>>,
    /// word id → entries writing it.  Marked on external seed changes only:
    /// the settled version must be re-established over an external write,
    /// while ordinary diff propagation leaves writers to `repair`.
    pub word_writers: Vec<Vec<u32>>,
    /// Parallel to `word_writers`: byte mask of the word the entry writes
    /// (an external write only clobbers a version it overlaps).
    pub word_writer_gmasks: Vec<Vec<u8>>,
    /// Comb words writable from outside the settle (event statements,
    /// top-level testbench variables incl. ports); diffed as seeds.
    pub ext_comb_words: Vec<u32>,
    /// word id → index+1 into `ext_comb_words`/`prev_ext` (0 = not ext).
    /// Lets the dirty-seed path and the out-diff update `prev_ext`
    /// without scanning the whole ext list.
    pub ext_pos: Vec<u32>,
    pub required_passes: usize,
    /// Per-event change-driven skip plans (`VERYL_INCR_EVENT=0` opts out):
    /// aligned 1:1 with the runtime statement list of each clock/reset
    /// event (comb-style batching must be disabled for these events).
    pub event_plans: crate::HashMap<crate::ir::Event, EventPlan>,
    /// Mark-driven event skip (the default mode; see [`EventSkipMode`]):
    /// number of skippable event chunks and the word → chunk-id CSR the
    /// settle diff / seed scans use to mark chunks whose watched words
    /// changed.  Empty unless mark mode is active.
    pub event_chunk_count: usize,
    pub event_csr_off: Vec<u32>,
    pub event_csr: Vec<u32>,
    /// Parallel to `event_csr`: byte mask of the word the chunk touches; a
    /// change wakes the chunk only when the changed bytes intersect.
    pub event_csr_gmask: Vec<u8>,
    /// Comb words written by `partial_settle` (the derived-clock closure).
    /// Those writes land between the full settle and the event evals, so
    /// the ext seed scan alone would deliver them one event pass late;
    /// `Ir::mark_event_partial` diffs them right after each partial settle.
    pub partial_out_words: Vec<u32>,
    /// Flat pool of event statements' direct comb-write words, sliced per
    /// statement by [`EventPlan::onrun_slice`]: each word is read just
    /// before the statement runs and diffed right after, so the marks
    /// reflect exactly what that run changed (a persistent baseline would
    /// drift behind settle repairs / co-writing statements and miss a
    /// write restoring the stale baseline value).
    pub event_onrun_words: Vec<u32>,
    /// Flat pool of FF co-writer chunk ids, sliced per statement by
    /// [`EventPlan::co_slice`]: when a statement runs, every other writer
    /// of an overlapping FF byte range must run too, or the skipped
    /// writer's committed value is clobbered for one cycle (write-log
    /// order decides the final value, so all overlapping writers must
    /// push together like the baseline does).
    pub event_co_ids: Vec<u32>,
}

/// Provenance of a change notification.  Every mark funnels through
/// [`IncrPlan::mark_word`], and the provenance selects the wake sets: the
/// settle out-diff has already woken its comb consumers off the flat
/// record, the seed scans must wake writers as well as readers (the
/// settled version has to be re-established over an external write), and
/// the deferred sources hand the comb side to the next settle's seed diff
/// instead of waking it inside the current pass.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MarkSource {
    /// Settle out-diff (`Ir::incr_sweep_pass`).
    OutDiff,
    /// External comb word, full or pending seed scan.
    ExtSeed,
    /// FF word, full or pending seed scan.
    FfSeed,
    /// Derived-clock `partial_settle` closure diff.
    Partial,
    /// An event statement's direct comb write, diffed around its run.
    /// `chunk` is that statement's own chunk id (`u32::MAX` when it has
    /// none), excluded from the event wake: it just ran.
    OnRun { chunk: u32 },
}

impl MarkSource {
    /// Slot in `IncrState::stats_event_marks`.
    #[inline(always)]
    fn stats_slot(self) -> usize {
        match self {
            MarkSource::OutDiff => 0,
            MarkSource::ExtSeed => 1,
            MarkSource::FfSeed => 2,
            MarkSource::Partial => 3,
            MarkSource::OnRun { .. } => 4,
        }
    }
}

/// The mutable half of a mark: the two dirty bitmaps, the dirty-seed lists
/// and the mark statistics.  Borrowed as a bundle because the sweep holds
/// `IncrState`'s fields destructured and cannot call an `&mut self` method.
///
/// This is the seam the fused chunk space plugs into: P6 replaces the
/// bodies behind the sink with an OR into the artifact's chunk dirty
/// bitmap, leaving the call sites unchanged.
pub struct MarkSink<'a> {
    pub dirty: &'a mut [u64],
    pub event_dirty: &'a mut [u64],
    pub pending_ff: &'a mut Vec<u32>,
    pub pending_ext: &'a mut Vec<u32>,
    pub seed_full: &'a mut bool,
    pub stats_event_marks: &'a mut [u64; 5],
}

#[inline(always)]
fn push_pending(list: &mut Vec<u32>, seed_full: &mut bool, w: u32) {
    if list.len() >= PENDING_CAP {
        *seed_full = true;
    } else {
        list.push(w);
    }
}

impl IncrPlan {
    /// The single mark entry: the bytes selected by `dgm` of global word
    /// `word` changed, as observed by `src`.  Callers pass a literal
    /// provenance, so the match folds away when inlined.
    #[inline(always)]
    pub fn mark_word(&self, sink: &mut MarkSink<'_>, src: MarkSource, word: usize, dgm: u8) {
        match src {
            MarkSource::ExtSeed | MarkSource::FfSeed => {
                for (&c, &g) in self.consumers[word]
                    .iter()
                    .zip(&self.consumer_gmasks[word])
                    .chain(
                        self.word_writers[word]
                            .iter()
                            .zip(&self.word_writer_gmasks[word]),
                    )
                {
                    if g & dgm == 0 {
                        continue;
                    }
                    let e = c as usize;
                    sink.dirty[e / 64] |= 1u64 << (e % 64);
                }
            }
            MarkSource::Partial | MarkSource::OnRun { .. } => {
                // These writes land between settles; the comb side is
                // delivered by the next settle's dirty-seed diff.
                if word >= self.comb_words {
                    push_pending(
                        sink.pending_ff,
                        sink.seed_full,
                        (word - self.comb_words) as u32,
                    );
                } else if self.ext_pos[word] != 0 {
                    push_pending(sink.pending_ext, sink.seed_full, word as u32);
                } else {
                    *sink.seed_full = true;
                }
            }
            // The comb consumers were woken from the flat record, which
            // carries the pre-filtered and pre-merged mark pool.
            MarkSource::OutDiff => {}
        }
        if self.event_chunk_count == 0 {
            return;
        }
        let a = self.event_csr_off[word] as usize;
        let b = self.event_csr_off[word + 1] as usize;
        for (&id, &gm) in self.event_csr[a..b].iter().zip(&self.event_csr_gmask[a..b]) {
            if gm & dgm == 0 || src == (MarkSource::OnRun { chunk: id }) {
                continue;
            }
            sink.stats_event_marks[src.stats_slot()] += 1;
            let id = id as usize;
            sink.event_dirty[id / 64] |= 1u64 << (id % 64);
        }
    }
}

/// Change-driven skip plan for one event's statement list.
pub struct EventPlan {
    /// Per runtime statement: `Some(words)` = skippable when none of these
    /// words (ins ∪ outs) changed since the statement last ran; `None` =
    /// always run (side effects, interpreted, or no captured deps).
    ///
    /// The union covers both directions: unchanged inputs mean the FF would
    /// write the same values (skipping the write-log entries is
    /// value-neutral), and unchanged outputs mean nobody clobbered the
    /// statement's last-written state (a reset/clock co-writer, a testbench
    /// poke, or a packed-word neighbour store would show here).
    /// Each word carries the byte mask the statement touches within it
    /// (packed-word neighbours are excluded by mask filtering).
    pub stmt_words: Vec<Option<Vec<(u32, u8)>>>,
    /// True when this plan drives the mark-driven skip (`stmt_ids` /
    /// `on_run` are populated); false for the legacy snapshot polling.
    pub mark_mode: bool,
    /// Parallel to `stmt_words` (mark mode): global chunk id of each
    /// skippable statement.  `eval_event_stmts` runs a statement iff its
    /// dirty bit is set, clearing it; the settle diff / seed scans set
    /// bits through [`IncrPlan::event_csr`].
    pub stmt_ids: Vec<Option<u32>>,
    /// Parallel (mark mode): `[start, end)` slice into
    /// [`IncrPlan::event_onrun_words`] — the statement's direct (blocking)
    /// comb writes.  Read just before the statement runs and diffed right
    /// after; readers (via `event_csr`, minus the writer itself) are
    /// marked on change.  Same-pass ordering: the ext seed scan alone
    /// would deliver the change one event pass late.
    pub onrun_slice: Vec<(u32, u32)>,
    /// Parallel (mark mode): `[start, end)` slice into
    /// [`IncrPlan::event_co_ids`] — chunk ids marked unconditionally when
    /// this statement runs (overlapping-FF-byte co-writers).
    pub co_slice: Vec<(u32, u32)>,
}

/// Event skip mode from `VERYL_INCR_EVENT`: unset (default) = mark-driven
/// skip fed by the settle diff / seed scans, `1` = legacy snapshot polling
/// (kept for A/B: it re-reads every skippable chunk's words per fire, which
/// on pe_core costs more than the skipped executions save), `0` = off.
#[derive(PartialEq, Clone, Copy)]
enum EventSkipMode {
    Off,
    Snapshot,
    Mark,
}

fn event_skip_mode() -> EventSkipMode {
    match env::var("VERYL_INCR_EVENT").ok().as_deref() {
        Some("0") => EventSkipMode::Off,
        Some("1") => EventSkipMode::Snapshot,
        _ => EventSkipMode::Mark,
    }
}

/// Build the per-event skip plans.  Only statements with captured
/// full-coverage deps and no side effects are skippable.  Returns
/// `(plans, chunk_count, csr_off, csr, csr_gmask, onrun_words, co_ids)`;
/// the CSR maps word id → skippable chunk ids watching it, with per-entry
/// read byte masks (mark mode only, empty otherwise).
///
/// The caller (`build_plan`) has already bailed on any event statement
/// with unmodelable or missing deps, so every write set seen here is
/// complete; a defensive `poisoned` fallback still strips all skips if
/// that invariant is ever violated.
#[allow(clippy::type_complexity)]
fn build_event_plans(
    events: &crate::HashMap<crate::ir::Event, ProtoStatements>,
    resolver: &WordResolver,
) -> (
    crate::HashMap<crate::ir::Event, EventPlan>,
    usize,
    Vec<u32>,
    Vec<u32>,
    Vec<u8>,
    Vec<u32>,
    Vec<u32>,
) {
    use crate::ir::Event;
    let mode = event_skip_mode();
    if mode == EventSkipMode::Off {
        return (
            crate::HashMap::default(),
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
    }

    // Pass 1: gather per-statement facts, in a stable event order.
    struct StmtInfo {
        /// ins ∪ outs words with per-word byte masks; `None` = always run.
        words: Option<Vec<(u32, u8)>>,
        /// Direct comb writes (blocking, visible to later stmts this pass).
        comb_outs: Vec<u32>,
        /// FF write words with byte masks (write-log outputs); overlapping
        /// writers form clobber-solidarity groups (see `event_co_ids`).
        ff_outs: Vec<(u32, u8)>,
        /// Why `words` is None (diagnostics): 0 skippable, 1 interpreted,
        /// 2 side-effect chunk, 3 missing deps, 4 oversized expansion.
        reason: u8,
    }
    // Shared fact gathering with SoC-scale expansion caps, estimated
    // BEFORE materializing.  A COMB write set past the per-entry cap
    // poisons the whole event skip: the on-run diff pool — the dirty-seed
    // feed every other skip decision depends on — cannot be indexed
    // affordably.  An FF write span past the cap (a whole-memory writer,
    // e.g. a DRAM model) is cheaper to handle: the statement runs
    // unconditionally, its span is recorded in `huge_ff_ranges` instead of
    // being expanded per word, and the post-pass forces any skippable
    // statement writing into that span to always-run — preserving
    // write-log order without a per-word overlap index (its FF commits
    // are tracked exactly by the write log's `note_ff_write`).  A
    // merely-large watch set keeps just that statement always-run.
    // `interp_reason` selects the diagnostics code for the side-effect
    // case (1 = interpreted stmt, 2 = compiled chunk).
    fn stmt_info(
        d: &ChunkDeps,
        interp_reason: u8,
        resolver: &WordResolver,
        poisoned: &mut bool,
        huge_ff_ranges: &mut Vec<(u32, u32)>,
    ) -> StmtInfo {
        let mut comb_est = 0usize;
        let mut ff_est = 0usize;
        for dep in &d.outs {
            let n = resolver.word_count(dep);
            if dep.off.is_ff() {
                ff_est += n;
            } else {
                comb_est += n;
            }
        }
        if comb_est > MAX_ENTRY_IN_WORDS() {
            *poisoned = true;
            return StmtInfo {
                words: None,
                comb_outs: Vec::new(),
                ff_outs: Vec::new(),
                reason: 4,
            };
        }
        let oversized_ff = ff_est > MAX_ENTRY_IN_WORDS();
        // An FF-offset dep can only expand to ids >= comb_words, so the
        // comb-out pool needs the comb deps only — and skipping FF deps
        // here is what keeps an oversized FF span from being expanded.
        let mut comb_outs: Vec<u32> = Vec::new();
        for dep in &d.outs {
            if !dep.off.is_ff() {
                resolver.words(dep, &mut comb_outs);
            }
        }
        comb_outs.retain(|&w| (w as usize) < resolver.comb_words);
        comb_outs.sort_unstable();
        comb_outs.dedup();
        let mut ff_outs: Vec<(u32, u8)> = Vec::new();
        for dep in &d.outs {
            if !dep.off.is_ff() {
                continue;
            }
            if oversized_ff {
                if let Some(r) = resolver.word_range(dep) {
                    huge_ff_ranges.push(r);
                }
            } else {
                resolver.words_masked(dep, &mut ff_outs);
            }
        }
        ff_outs.retain(|&(w, _)| (w as usize) >= resolver.comb_words);
        let (words, reason) = if d.always_side || d.unmodelable {
            (None, interp_reason)
        } else if oversized_ff {
            (None, 4)
        } else {
            let in_est: usize = d.ins.iter().map(|dep| resolver.word_count(dep)).sum();
            if in_est + comb_est + ff_est > MAX_ENTRY_IN_WORDS() {
                (None, 4)
            } else {
                let mut ws: Vec<(u32, u8)> = Vec::new();
                for dep in d.ins.iter().chain(&d.outs) {
                    resolver.words_masked(dep, &mut ws);
                }
                sort_dedup_or(&mut ws);
                (Some(ws), 0)
            }
        };
        StmtInfo {
            words,
            comb_outs,
            ff_outs,
            reason,
        }
    }
    let mut poisoned = false;
    let mut huge_ff_ranges: Vec<(u32, u32)> = Vec::new();
    let mut gathered: Vec<(&Event, Vec<StmtInfo>)> = Vec::new();
    for (event, stmts) in events {
        // One-shot events gain nothing; testbench-driven statements there
        // (readmemh, prints) must always run anyway.
        if !matches!(event, Event::Clock(_) | Event::Reset(_)) {
            continue;
        }
        let mut infos: Vec<StmtInfo> = Vec::new();
        for block in &stmts.0 {
            match block {
                ProtoStatementBlock::Interpreted(list) => {
                    // "Interpreted" is a proto-level classification: most of
                    // these convert to individually JIT-compiled runtime
                    // statements, and their deps make them skippable just
                    // like chunked ones.  Side effects / unmodelable writes
                    // still force always-run.
                    for stmt in list {
                        let mut d = ChunkDeps::default();
                        collect_stmt_deps(stmt, &mut d);
                        if d.unmodelable {
                            poisoned = true;
                        }
                        infos.push(stmt_info(
                            &d,
                            1,
                            resolver,
                            &mut poisoned,
                            &mut huge_ff_ranges,
                        ));
                    }
                }
                ProtoStatementBlock::Compiled(artifact) => {
                    let Some(deps) = artifact.deps.as_deref() else {
                        poisoned = true;
                        infos.push(StmtInfo {
                            words: None,
                            comb_outs: Vec::new(),
                            ff_outs: Vec::new(),
                            reason: 3,
                        });
                        continue;
                    };
                    if deps.unmodelable {
                        poisoned = true;
                    }
                    infos.push(stmt_info(
                        deps,
                        2,
                        resolver,
                        &mut poisoned,
                        &mut huge_ff_ranges,
                    ));
                }
            }
        }
        gathered.push((event, infos));
    }
    // Skippable statements writing into an oversized (unindexed) FF span
    // must always run: the oversized writer pushes its log entries every
    // fire, and write-log order — which decides the committed value on
    // overlap — is only preserved when the overlapping writers push
    // together, like the baseline does.
    if !huge_ff_ranges.is_empty() {
        for (_, infos) in &mut gathered {
            for info in infos {
                if info.words.is_some()
                    && info
                        .ff_outs
                        .iter()
                        .any(|&(w, _)| huge_ff_ranges.iter().any(|&(a, b)| w >= a && w < b))
                {
                    info.words = None;
                    info.reason = 4;
                }
            }
        }
    }

    // Total watch volume across all skippable statements: past the plan
    // budget the CSR index costs more than the skips save.
    let watch_total: usize = gathered
        .iter()
        .flat_map(|(_, infos)| infos.iter())
        .map(|i| i.words.as_ref().map_or(0, |w| w.len()))
        .sum();
    if watch_total > MAX_PLAN_IN_WORDS {
        poisoned = true;
    }
    if env::var("VERYL_INCR_EVENT_DIAG").as_deref() == Ok("1") {
        let mut skippable = 0usize;
        let mut always = 0usize;
        for (event, infos) in &gathered {
            let n_skip = infos.iter().filter(|i| i.words.is_some()).count();
            let n_interp = infos.iter().filter(|i| i.reason == 1).count();
            let n_side = infos.iter().filter(|i| i.reason == 2).count();
            let n_nodeps = infos.iter().filter(|i| i.reason == 3).count();
            let n_big = infos.iter().filter(|i| i.reason == 4).count();
            skippable += n_skip;
            always += infos.len() - n_skip;
            eprintln!(
                "[evplan] {event:?}: {} stmts, {} skippable, {} interp, {} side, {} nodeps, {} oversized",
                infos.len(),
                n_skip,
                n_interp,
                n_side,
                n_nodeps,
                n_big
            );
        }
        eprintln!("[evplan] total: skippable={skippable} always={always} poisoned={poisoned}");
    }

    // Poisoned = a write set the skip machinery cannot track (unmodelable,
    // missing deps, oversized expansion, watch volume past budget).  Emit
    // NO plans at all rather than all-always-run plans: a mark-mode plan
    // with zero skippable chunks never initializes `event_dirty`, so the
    // per-run on-run diff — the dirty-seed feed for the next settle —
    // silently never runs, starving the seed lists (stale-settle bug found
    // on heliodor's OoO core, whose DRAM writer trips the oversized cap).
    // Plan-less events force the full seed scans every settle instead.
    if poisoned {
        return (
            crate::HashMap::default(),
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
    }

    // Pass 2 (mark mode): assign chunk ids, invert to word → chunks, and
    // collect FF writers per word for the clobber-solidarity groups.
    let mut n_chunks = 0usize;
    let mut word_chunks: Vec<Vec<(u32, u8)>> = Vec::new();
    let mut flat_ids: Vec<Option<u32>> = Vec::new();
    let mut ff_writers: crate::HashMap<u32, Vec<(u32, u8)>> = crate::HashMap::default();
    if mode == EventSkipMode::Mark {
        word_chunks.resize(resolver.total_words, Vec::new());
        let mut fidx = 0u32;
        for (_, infos) in &gathered {
            for info in infos {
                let id = info.words.as_ref().map(|ws| {
                    let id = n_chunks as u32;
                    n_chunks += 1;
                    for &(w, gm) in ws {
                        word_chunks[w as usize].push((id, gm));
                    }
                    id
                });
                flat_ids.push(id);
                for &(w, gm) in &info.ff_outs {
                    ff_writers.entry(w).or_default().push((fidx, gm));
                }
                fidx += 1;
            }
        }
    }

    // Pass 3: emit plans (ids assigned in the same iteration order as
    // pass 2), the flat CSR, and the flat on-run word pool.
    let mut plans = crate::HashMap::default();
    let mut onrun_words: Vec<u32> = Vec::new();
    let mut co_ids: Vec<u32> = Vec::new();
    let mut fidx = 0usize;
    for (event, infos) in gathered {
        let n = infos.len();
        let mut stmt_words = Vec::with_capacity(n);
        let mut stmt_ids = Vec::with_capacity(n);
        let mut onrun_slice = Vec::with_capacity(n);
        let mut co_slice = Vec::with_capacity(n);
        for info in infos {
            let id = if mode == EventSkipMode::Mark {
                flat_ids[fidx]
            } else {
                None
            };
            let start = onrun_words.len() as u32;
            let co_start = co_ids.len() as u32;
            if mode == EventSkipMode::Mark {
                // Every direct comb write is diffed post-run: readers of a
                // changed word are marked, and the change feeds the
                // dirty-seed list for the next settle's comb consumers.
                onrun_words.extend_from_slice(&info.comb_outs);
                // Clobber solidarity: other skippable writers of an
                // overlapping FF byte range must run whenever this
                // statement runs (write-log order decides the committed
                // value; the baseline pushes all of them every cycle).
                let mut co: Vec<u32> = Vec::new();
                for &(w, gm) in &info.ff_outs {
                    if let Some(writers) = ff_writers.get(&w) {
                        for &(ofi, ogm) in writers {
                            if ofi as usize != fidx
                                && gm & ogm != 0
                                && let Some(oid) = flat_ids[ofi as usize]
                            {
                                co.push(oid);
                            }
                        }
                    }
                }
                co.sort_unstable();
                co.dedup();
                co_ids.extend_from_slice(&co);
            }
            stmt_words.push(info.words);
            stmt_ids.push(id);
            onrun_slice.push((start, onrun_words.len() as u32));
            co_slice.push((co_start, co_ids.len() as u32));
            fidx += 1;
        }
        plans.insert(
            event.clone(),
            EventPlan {
                stmt_words,
                mark_mode: mode == EventSkipMode::Mark,
                stmt_ids,
                onrun_slice,
                co_slice,
            },
        );
    }
    let mut csr_off = Vec::new();
    let mut csr = Vec::new();
    let mut csr_gmask = Vec::new();
    if mode == EventSkipMode::Mark {
        csr_off.reserve(resolver.total_words + 1);
        csr_off.push(0u32);
        for ws in &word_chunks {
            for &(c, gm) in ws {
                csr.push(c);
                csr_gmask.push(gm);
            }
            csr_off.push(csr.len() as u32);
        }
    }
    (
        plans,
        n_chunks,
        csr_off,
        csr,
        csr_gmask,
        onrun_words,
        co_ids,
    )
}

struct WordResolver {
    comb_words: usize,
    total_words: usize,
    comb_size: crate::HashMap<isize, usize>,
    ff_size: crate::HashMap<isize, usize>,
}

impl WordResolver {
    fn new(meta: &ModuleVariableMeta, comb_bytes: usize, ff_bytes: usize) -> Self {
        fn walk(
            meta: &ModuleVariableMeta,
            comb: &mut crate::HashMap<isize, usize>,
            ff: &mut crate::HashMap<isize, usize>,
        ) {
            for vm in meta.variable_meta.values() {
                for elem in &vm.elements {
                    match elem.current {
                        VarOffset::Comb(o) => {
                            comb.insert(o, elem.native_bytes);
                        }
                        VarOffset::Ff(o) => {
                            ff.insert(o, elem.native_bytes);
                        }
                    }
                }
            }
            for child in &meta.children {
                walk(child, comb, ff);
            }
        }
        let mut comb_size = crate::HashMap::default();
        let mut ff_size = crate::HashMap::default();
        walk(meta, &mut comb_size, &mut ff_size);
        WordResolver {
            comb_words: comb_bytes.div_ceil(8),
            total_words: comb_bytes.div_ceil(8) + ff_bytes.div_ceil(8),
            comb_size,
            ff_size,
        }
    }

    /// Expand a dep to the word ids it covers.  Unknown widths fall back to
    /// the variable's native width from the meta, then to 8 bytes.
    fn words(&self, dep: &Dep, out: &mut Vec<u32>) {
        let (o, base, default) = match dep.off {
            VarOffset::Comb(o) => (o, 0usize, self.comb_size.get(&o).copied()),
            VarOffset::Ff(o) => (o, self.comb_words, self.ff_size.get(&o).copied()),
        };
        if o < 0 {
            return;
        }
        let bytes = dep.bytes.map(|b| b as usize).or(default).unwrap_or(8);
        let w0 = o as usize / 8;
        let w1 = (o as usize + bytes.max(1)).div_ceil(8);
        for w in w0..w1 {
            let id = base + w;
            if id < self.total_words {
                out.push(id as u32);
            }
        }
    }

    /// Word-id interval `[w0, w1)` a dep covers, without materializing.
    fn word_range(&self, dep: &Dep) -> Option<(u32, u32)> {
        let (o, base, default) = match dep.off {
            VarOffset::Comb(o) => (o, 0usize, self.comb_size.get(&o).copied()),
            VarOffset::Ff(o) => (o, self.comb_words, self.ff_size.get(&o).copied()),
        };
        if o < 0 {
            return None;
        }
        let bytes = dep.bytes.map(|b| b as usize).or(default).unwrap_or(8);
        let w0 = (base + o as usize / 8).min(self.total_words);
        let w1 = (base + (o as usize + bytes.max(1)).div_ceil(8)).min(self.total_words);
        (w0 < w1).then_some((w0 as u32, w1 as u32))
    }

    /// Number of words `words`/`words_masked` would expand `dep` to,
    /// without materializing them.
    fn word_count(&self, dep: &Dep) -> usize {
        let (o, default) = match dep.off {
            VarOffset::Comb(o) => (o, self.comb_size.get(&o).copied()),
            VarOffset::Ff(o) => (o, self.ff_size.get(&o).copied()),
        };
        if o < 0 {
            return 0;
        }
        let bytes = dep.bytes.map(|b| b as usize).or(default).unwrap_or(8);
        (o as usize + bytes.max(1)).div_ceil(8) - o as usize / 8
    }

    /// Like `words`, but each word carries the byte mask the dep touches
    /// within it (narrowed by a static bit range when present).
    fn words_masked(&self, dep: &Dep, out: &mut Vec<(u32, u8)>) {
        let (o, base, default) = match dep.off {
            VarOffset::Comb(o) => (o, 0usize, self.comb_size.get(&o).copied()),
            VarOffset::Ff(o) => (o, self.comb_words, self.ff_size.get(&o).copied()),
        };
        if o < 0 {
            return;
        }
        let bytes = dep.bytes.map(|b| b as usize).or(default).unwrap_or(8);
        // Touched byte range [b0, b1) within the buffer.
        let (b0, b1) = match dep.bits {
            Some((lo, hi)) if bytemask_enabled() => (
                o as usize + lo as usize / 8,
                o as usize + hi as usize / 8 + 1,
            ),
            _ => (o as usize, o as usize + bytes.max(1)),
        };
        let w0 = o as usize / 8;
        let w1 = (o as usize + bytes.max(1)).div_ceil(8);
        for w in w0..w1 {
            let id = base + w;
            if id >= self.total_words {
                continue;
            }
            let gm = if bytemask_enabled() {
                let lo = b0.max(w * 8);
                let hi = b1.min(w * 8 + 8);
                if lo >= hi {
                    continue;
                }
                let n = hi - lo;
                (((1u16 << n) - 1) as u8) << (lo - w * 8)
            } else {
                0xff
            };
            out.push((id as u32, gm));
        }
    }
}

fn off_key(off: &VarOffset) -> u64 {
    let (o, ff) = match off {
        VarOffset::Comb(o) => (*o, 0u64),
        VarOffset::Ff(o) => (*o, 1u64),
    };
    (ff << 62) | (o as u64 & ((1u64 << 62) - 1))
}

/// Written words (with per-word byte masks) and per-variable spans of one
/// dep list (append order; the caller sorts/dedups).
fn collect_outs(resolver: &WordResolver, outs: &[Dep]) -> (Vec<(u32, u8)>, Vec<OutVar>) {
    let mut out_words = Vec::new();
    let mut out_vars = Vec::new();
    for d in outs {
        let start = out_words.len();
        resolver.words_masked(d, &mut out_words);
        if out_words.len() > start {
            let w0 = out_words[start..].iter().map(|&(w, _)| w).min().unwrap();
            let w1 = out_words[start..].iter().map(|&(w, _)| w).max().unwrap() + 1;
            let (lo, hi) = d.bits.unwrap_or((0, u32::MAX));
            out_vars.push(OutVar {
                key: off_key(&d.off),
                w0,
                w1,
                lo,
                hi,
            });
        }
    }
    (out_words, out_vars)
}

/// Sort `(word, mask)` pairs and merge duplicates by OR-ing the masks.
fn sort_dedup_or(v: &mut Vec<(u32, u8)>) {
    v.sort_unstable_by_key(|&(w, _)| w);
    let mut out = 0usize;
    let mut i = 0usize;
    while i < v.len() {
        let (w, mut m) = v[i];
        let mut j = i + 1;
        while j < v.len() && v[j].0 == w {
            m |= v[j].1;
            j += 1;
        }
        v[out] = (w, m);
        out += 1;
        i = j;
    }
    v.truncate(out);
}

/// Per-entry accumulators of the plan build.
#[derive(Default)]
struct PlanAcc {
    /// Running total of registered read words ([`MAX_PLAN_IN_WORDS`] cap).
    total_in_words: usize,
    always_run: Vec<u32>,
    entry_out_words: Vec<Vec<u32>>,
    /// Aligned with `entry_out_words`: byte mask written within each word.
    entry_out_word_gmasks: Vec<Vec<u8>>,
    entry_out_vars: Vec<Vec<OutVar>>,
    consumers: Vec<Vec<u32>>,
    /// Aligned with `consumers`: byte mask read within the word.
    consumer_gmasks: Vec<Vec<u8>>,
}

/// Plan-build volume counters (dumped under `VERYL_INCR_DIAG=1`).
static PLAN_STAT_IN: AtomicU64 = AtomicU64::new(0);
static PLAN_STAT_OUT: AtomicU64 = AtomicU64::new(0);
static PLAN_STAT_MAX: AtomicU64 = AtomicU64::new(0);

/// Upper bound on one entry's read-word expansion.  An entry whose inputs
/// cover this many words (8 MB of state at 1M words) is woken by
/// essentially every change, so a change-driven plan buys nothing there —
/// while its per-word consumer index would dominate the plan's build cost
/// and footprint.  Such entries are downgraded to `always_run` with no
/// consumer registration instead of dooming the whole plan: a hierarchical
/// SoC has a handful of whole-memory readers among thousands of ordinary
/// entries.  The same bound applied to an entry's WRITE expansion still
/// abandons the plan (out words feed per-entry snapshots and the post-run
/// diff, both proportional to the span, on every run).  pe_core's largest
/// entry is 35K words, ~30x under the limit.
const MAX_ENTRY_IN_WORDS_DEFAULT: usize = 1 << 20;

/// Diagnostic override (`VERYL_INCR_ENTRY_CAP=<words>`): raise or lower
/// the per-entry expansion cap to bisect huge-reader behaviour.
#[allow(non_snake_case)]
fn MAX_ENTRY_IN_WORDS() -> usize {
    static V: OnceLock<usize> = OnceLock::new();
    *V.get_or_init(|| {
        env::var("VERYL_INCR_ENTRY_CAP")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(MAX_ENTRY_IN_WORDS_DEFAULT)
    })
}

/// Byte span above which `gather_reads_expanded_ranged` collapses a
/// dynamic-select array read to one whole-span dep instead of per-element
/// entries — aligned with [`MAX_ENTRY_IN_WORDS`] so a collapsed dep trips
/// the same always_run downgrade the expanded form would.
pub(crate) fn read_expand_cap_bytes() -> usize {
    MAX_ENTRY_IN_WORDS() * 8
}

/// Total read-word expansion budget across all entries (the consumer
/// index volume, `always_run`-downgraded entries excluded).  A plan past
/// this bound spends more on marking than the skips save; abandon it.
const MAX_PLAN_IN_WORDS: usize = 1 << 24;

/// Conv-time feasibility estimate for the incremental plan, mirroring the
/// instantiate-time guards of [`push_entry`] at the granularity the
/// incremental configuration will actually see: nested CompiledBlocks and
/// SequentialBlocks are fully flattened by `analyze_dependency` under
/// `VERYL_INCR=1`, so the estimate walks per flattened statement.
/// Infeasible = a single statement's write expansion trips
/// [`MAX_ENTRY_IN_WORDS`] (the plan would be abandoned), or the summed
/// read expansion (huge readers excluded — those merely downgrade to
/// `always_run`) trips [`MAX_PLAN_IN_WORDS`].  Runs before the variable
/// meta / word resolver exist, but read and write deps both carry their
/// native byte span since capture time; only the rare deps without one
/// (Readmemh elements, deps-less CompiledBlock fallbacks) count as one
/// word each — close enough against the wide threshold margins.
/// Infeasible modules keep the default (non-incremental) conv
/// configuration: whole-module AOT-C stays engaged and statement batching
/// stays on, since the plan would only be declined at instantiate time
/// anyway and the full-settle fallback then runs the same code the
/// default pipeline produces.
/// Emitted C per `ProtoStatement`, letting a statement count stand in for a
/// code size before anything is emitted.
///
/// Taken from the low end of what measured designs produce.  A smaller figure
/// raises the threshold, erring toward *not* building a plan — the safer
/// direction, since a wrongly built plan forfeits whole-module AOT-C outright
/// while a wrongly skipped one only misses the plan's win.
const CODE_BYTES_PER_STMT: usize = 763;

/// Last-level cache size, if the platform will say.
///
/// A baked constant would be wrong: cache sizes span orders of magnitude
/// across the machines Veryl runs on, and the same design is fetch-bound on
/// one and resident on another.  `None` leaves the caller guessing, which is
/// tolerable — a target without a C compiler has no whole-module AOT-C to
/// forfeit, so the gate has nothing to decide.
#[cfg(target_os = "linux")]
fn llc_bytes() -> Option<usize> {
    // sysfs lists every level; the largest is the last one.
    let mut best = 0usize;
    for entry in fs::read_dir("/sys/devices/system/cpu/cpu0/cache").ok()? {
        // One unreadable entry must not discard the levels already found.
        let Ok(entry) = entry else { continue };
        let path = entry.path().join("size");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let text = text.trim();
        // Values look like "23040K" or "48K"; a bare number means bytes.
        let (digits, mult) = match text.strip_suffix('K') {
            Some(d) => (d, 1024),
            None => match text.strip_suffix('M') {
                Some(d) => (d, 1024 * 1024),
                None => (text, 1),
            },
        };
        if let Ok(n) = digits.parse::<usize>() {
            best = best.max(n * mult);
        }
    }
    (best > 0).then_some(best)
}

#[cfg(target_os = "macos")]
fn llc_bytes() -> Option<usize> {
    // Apple Silicon reports no L3, so its per-cluster L2 is the last level;
    // try the keys in descending order and take the first that answers.
    use std::process::Command;
    for key in [
        "hw.l3cachesize",
        "hw.perflevel0.l2cachesize",
        "hw.l2cachesize",
    ] {
        let Ok(out) = Command::new("sysctl").args(["-n", key]).output() else {
            continue;
        };
        if let Ok(n) = String::from_utf8_lossy(&out.stdout).trim().parse::<usize>()
            && n > 0
        {
            return Some(n);
        }
    }
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn llc_bytes() -> Option<usize> {
    None
}

/// Statement count above which a plan is worth its AOT-C forfeit
/// (`VERYL_INCR_MIN_STMTS` overrides; 0 disables the gate).
///
/// Derived from the running machine rather than fixed, since the question is
/// whether a design's code fits *this* cache.  The measured designs sat far
/// enough either side that the exact value did not matter; designs landing
/// near the boundary are a guess, and a counter-example is worth reporting.
pub fn min_stmts_for_plan() -> usize {
    static V: OnceLock<usize> = OnceLock::new();
    *V.get_or_init(|| {
        if let Some(n) = env::var("VERYL_INCR_MIN_STMTS")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            return n;
        }
        llc_bytes().map_or(65536, |b| b / CODE_BYTES_PER_STMT)
    })
}

/// Whether the comb list is small enough that a plan is the wrong trade.
///
/// A plan forfeits whole-module AOT-C (`aot_size_ok = !incr_on`) to buy
/// skipping, and skipping only pays when the code is too large to issue at
/// speed.  Below the last-level cache the settle is not fetch-bound, so the
/// forfeited AOT-C costs more than skipping saves; above it instruction fetch
/// dominates and skipping is the strongest lever there is.  Both directions
/// are measured.
///
/// Activity cannot substitute for size, which is why the run-fraction check
/// alone could not decide this: a design may skip a larger share of its
/// entries than another and still lose, because its whole program already fits.
///
/// Compared against the whole cache, not a per-worker share of it — that is
/// what the measurements support, even under the parallelism `veryl test` uses.
pub fn plan_too_small(stmts: &[ProtoStatement]) -> bool {
    let min = min_stmts_for_plan();
    min != 0 && stmts.len() < min
}

pub fn stmts_infeasible(stmts: &[ProtoStatement]) -> bool {
    fn dep_words_approx(d: &Dep) -> usize {
        let o = match d.off {
            VarOffset::Comb(o) | VarOffset::Ff(o) => o,
        };
        if o < 0 {
            return 0;
        }
        let bytes = d.bytes.map(|b| b as usize).unwrap_or(8).max(1);
        (o as usize + bytes).div_ceil(8) - o as usize / 8
    }
    fn walk(stmts: &[ProtoStatement], total: &mut usize) -> bool {
        for s in stmts {
            match s {
                ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty() => {
                    if walk(&cb.original_stmts, total) {
                        return true;
                    }
                }
                ProtoStatement::SequentialBlock(body) => {
                    if walk(body, total) {
                        return true;
                    }
                }
                s => {
                    let mut deps = ChunkDeps::default();
                    collect_stmt_deps(s, &mut deps);
                    let est_out: usize = deps.outs.iter().map(dep_words_approx).sum();
                    if est_out > MAX_ENTRY_IN_WORDS() {
                        return true;
                    }
                    let est_in: usize = deps.ins.iter().map(dep_words_approx).sum();
                    if est_in <= MAX_ENTRY_IN_WORDS() {
                        *total += est_in;
                        if *total > MAX_PLAN_IN_WORDS {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
    let mut total = 0usize;
    walk(stmts, &mut total)
}

fn push_entry(resolver: &WordResolver, deps: &ChunkDeps, acc: &mut PlanAcc) -> bool {
    if deps.unmodelable {
        return false;
    }
    let Ok(e) = u32::try_from(acc.entry_out_words.len()) else {
        return false;
    };

    // Estimate both expansions BEFORE materializing them — the expansion
    // itself is what explodes on SoC-sized entries.  A huge WRITE span
    // abandons the plan (snapshots + post-run diff scale with it on every
    // run); a huge READ span merely downgrades the entry to always-run
    // with no consumer registration (it would be woken by essentially
    // every change anyway).
    let est_out: usize = deps.outs.iter().map(|d| resolver.word_count(d)).sum();
    if est_out > MAX_ENTRY_IN_WORDS() {
        diag(&format!(
            "entry write-word expansion {est_out} exceeds {}",
            MAX_ENTRY_IN_WORDS()
        ));
        return false;
    }
    let est: usize = deps.ins.iter().map(|d| resolver.word_count(d)).sum();
    let huge_reader = est > MAX_ENTRY_IN_WORDS();
    if !huge_reader {
        acc.total_in_words += est;
        if acc.total_in_words > MAX_PLAN_IN_WORDS {
            diag(&format!(
                "total read-word expansion {} exceeds {MAX_PLAN_IN_WORDS}",
                acc.total_in_words
            ));
            return false;
        }
    }

    let mut in_words: Vec<(u32, u8)> = Vec::new();
    if !huge_reader {
        for d in &deps.ins {
            resolver.words_masked(d, &mut in_words);
        }
        PLAN_STAT_IN.fetch_add(in_words.len() as u64, Ordering::Relaxed);
        PLAN_STAT_MAX.fetch_max(in_words.len() as u64, Ordering::Relaxed);
        sort_dedup_or(&mut in_words);
    }

    let (mut out_words, mut out_vars) = collect_outs(resolver, &deps.outs);
    PLAN_STAT_OUT.fetch_add(out_words.len() as u64, Ordering::Relaxed);
    sort_dedup_or(&mut out_words);
    out_vars.sort_unstable();
    out_vars.dedup();

    // `in_words` stays empty for a huge reader, so it registers no consumers:
    // it runs every settle anyway, and its index would dominate the plan.
    for &(w, g) in &in_words {
        acc.consumers[w as usize].push(e);
        acc.consumer_gmasks[w as usize].push(g);
    }
    acc.entry_out_words
        .push(out_words.iter().map(|&(w, _)| w).collect());
    acc.entry_out_word_gmasks
        .push(out_words.iter().map(|&(_, g)| g).collect());
    acc.entry_out_vars.push(out_vars);
    if deps.always_run || huge_reader {
        acc.always_run.push(e);
    }
    true
}

/// Build the runtime plan.  `None` disables the incremental path for this
/// module (unmodelable statements, missing chunk deps, or an unsupported
/// configuration); the caller then keeps the baseline full settle.
#[allow(clippy::too_many_arguments)]
pub fn build_plan(
    comb: &ProtoStatements,
    events: &crate::HashMap<crate::ir::Event, ProtoStatements>,
    derived_clock_eval: &ProtoStatements,
    meta: &ModuleVariableMeta,
    comb_bytes: usize,
    ff_bytes: usize,
    required_passes: usize,
    use_4state: bool,
) -> Option<IncrPlan> {
    use crate::ir::statement::ProtoStatementBlock;

    if use_4state {
        // The dependency spans model payload words only; 4-state mask
        // planes would be under-covered, so fall back to the full settle.
        diag("4-state module");
        return None;
    }

    // External components are safe: their outputs are restricted to plain
    // top-level variable references, and every root-scope comb word is in
    // the external seed set below (FF-classified outputs are covered by the
    // ff diff).  Derived clocks are safe for the same reason:
    // `partial_settle` writes only `derived_clock_eval` outputs, which are
    // folded into the seed set below, and the master input clock bit is a
    // root-scope variable.

    let resolver = WordResolver::new(meta, comb_bytes, ff_bytes);

    // Temporary diagnostic: VERYL_INCR_WORD_DBG="w1,w2" prints every
    // variable element covering the given global word ids.
    if let Ok(dbg) = env::var("VERYL_INCR_WORD_DBG") {
        let targets: Vec<usize> = dbg.split(',').filter_map(|s| s.parse().ok()).collect();
        fn walk_dbg(
            meta: &ModuleVariableMeta,
            comb_words: usize,
            targets: &[usize],
            path: &mut Vec<String>,
        ) {
            for vm in meta.variable_meta.values() {
                for (ei, elem) in vm.elements.iter().enumerate() {
                    let (o, base) = match elem.current {
                        VarOffset::Comb(o) => (o, 0usize),
                        VarOffset::Ff(o) => (o, comb_words),
                    };
                    if o < 0 {
                        continue;
                    }
                    let w0 = base + o as usize / 8;
                    let w1 = base + (o as usize + elem.native_bytes.max(1)).div_ceil(8);
                    for &t in targets {
                        if t >= w0 && t < w1 {
                            eprintln!(
                                "[word-dbg] word={t} {}::{}[{ei}] off={o} bytes={} {}",
                                path.join("."),
                                vm.path,
                                elem.native_bytes,
                                if base == 0 { "comb" } else { "ff" },
                            );
                        }
                    }
                }
            }
            for child in &meta.children {
                path.push(format!("{}", child.name));
                walk_dbg(child, comb_words, targets, path);
                path.pop();
            }
        }
        let mut path = vec![format!("{}", meta.name)];
        walk_dbg(meta, resolver.comb_words, &targets, &mut path);
    }
    let mut acc = PlanAcc {
        consumers: vec![Vec::new(); resolver.total_words],
        consumer_gmasks: vec![Vec::new(); resolver.total_words],
        ..Default::default()
    };

    // One entry per runtime comb statement, in `to_statements` expansion
    // order (batching must be disabled by the caller to keep alignment).
    let interp_dump = env::var("VERYL_INTERP_DUMP").as_deref() == Ok("1");
    // Temporary diagnostic: VERYL_INCR_DUMP_ENTRY=<e> prints entry e's
    // captured deps and pre-JIT statements at plan build.
    let dump_entry: Option<usize> = env::var("VERYL_INCR_DUMP_ENTRY")
        .ok()
        .and_then(|s| s.parse().ok());
    let dump = |e: usize, deps: &ChunkDeps, stmts: &[ProtoStatement]| {
        if dump_entry != Some(e) {
            return;
        }
        eprintln!("[entry-dbg] entry {e}: ins={:?}", deps.ins);
        eprintln!("[entry-dbg] entry {e}: outs={:?}", deps.outs);
        for s in stmts {
            let d = format!("{s:?}");
            eprintln!("[entry-dbg] entry {e} stmt: {}", &d[..d.len().min(2000)]);
        }
    };
    for block in &comb.0 {
        match block {
            ProtoStatementBlock::Interpreted(stmts) => {
                for s in stmts {
                    if interp_dump {
                        let d = format!("{s:?}");
                        eprintln!(
                            "[interp-proto] entry {}: {}",
                            acc.entry_out_words.len(),
                            &d[..d.len().min(1200)]
                        );
                    }
                    let mut deps = ChunkDeps::default();
                    collect_stmt_deps(s, &mut deps);
                    dump(acc.entry_out_words.len(), &deps, std::slice::from_ref(s));
                    if !push_entry(&resolver, &deps, &mut acc) {
                        diag("unmodelable comb statement");
                        return None;
                    }
                }
            }
            ProtoStatementBlock::Compiled(artifact) => {
                let Some(deps) = artifact.deps.as_deref() else {
                    diag("comb chunk artifact without captured deps");
                    return None;
                };
                dump(acc.entry_out_words.len(), deps, &[]);
                if !push_entry(&resolver, deps, &mut acc) {
                    diag("unmodelable comb chunk");
                    return None;
                }
            }
        }
    }
    if env::var("VERYL_INCR_DIAG").as_deref() == Ok("1") {
        use Ordering::Relaxed;
        eprintln!(
            "[incr] plan volume: entries={} total_words={} in_pushes={} out_pushes={} max_entry_in={}",
            acc.entry_out_words.len(),
            resolver.total_words,
            PLAN_STAT_IN.swap(0, Relaxed),
            PLAN_STAT_OUT.swap(0, Relaxed),
            PLAN_STAT_MAX.swap(0, Relaxed),
        );
    }
    let PlanAcc {
        total_in_words: _,
        mut always_run,
        entry_out_words,
        entry_out_word_gmasks,
        entry_out_vars,
        consumers,
        consumer_gmasks,
    } = acc;

    // External seed set: comb words writable outside the settle.
    let mut ext = Vec::new();
    // (a) event statement writes (clock events write mostly FF, but
    // comb-scope writes exist on some paths).  Initial statements matter
    // too: they are converted into testbench statements that execute
    // between steps (e.g. `$readmemh` loads mid-run).  The derived-clock
    // closure is written by `partial_settle` outside this path, so its
    // outputs are external writers as well.
    for stmts in events.values().chain(std::iter::once(derived_clock_eval)) {
        for block in &stmts.0 {
            match block {
                ProtoStatementBlock::Interpreted(list) => {
                    for s in list {
                        let mut deps = ChunkDeps::default();
                        collect_stmt_deps(s, &mut deps);
                        if deps.unmodelable {
                            diag("unmodelable event statement");
                            return None;
                        }
                        for d in &deps.outs {
                            if !d.off.is_ff() {
                                resolver.words(d, &mut ext);
                            }
                        }
                    }
                }
                ProtoStatementBlock::Compiled(artifact) => {
                    let Some(deps) = artifact.deps.as_deref() else {
                        diag("event chunk artifact without captured deps");
                        return None;
                    };
                    if deps.unmodelable {
                        diag("unmodelable event chunk");
                        return None;
                    }
                    for d in &deps.outs {
                        if !d.off.is_ff() {
                            resolver.words(d, &mut ext);
                        }
                    }
                }
            }
        }
    }
    // Words `partial_settle` writes (derived-clock closure outputs): the
    // mark-driven event skip must see those changes before the event evals
    // of the same step (see `IncrPlan::partial_out_words`).  FF outputs are
    // included — the closure can carry comb-scope FF writes that land in
    // the FF buffer outside the commit path, invisible to the seed scans
    // until one settle later.  The bail conditions were already checked in
    // the loop above.
    let mut partial_out_words: Vec<u32> = Vec::new();
    for block in &derived_clock_eval.0 {
        match block {
            ProtoStatementBlock::Interpreted(list) => {
                for s in list {
                    let mut deps = ChunkDeps::default();
                    collect_stmt_deps(s, &mut deps);
                    for d in &deps.outs {
                        resolver.words(d, &mut partial_out_words);
                    }
                }
            }
            ProtoStatementBlock::Compiled(artifact) => {
                if let Some(deps) = artifact.deps.as_deref() {
                    for d in &deps.outs {
                        resolver.words(d, &mut partial_out_words);
                    }
                }
            }
        }
    }
    partial_out_words.sort_unstable();
    partial_out_words.dedup();

    // (b) top-level (root scope) variables: the testbench writes DUT ports
    // and harness variables directly.
    for vm in meta.variable_meta.values() {
        for elem in &vm.elements {
            if let VarOffset::Comb(o) = elem.current {
                resolver.words(
                    &Dep {
                        off: VarOffset::Comb(o),
                        bytes: Some(elem.native_bytes as u32),
                        bits: None,
                    },
                    &mut ext,
                );
            }
        }
    }
    ext.sort_unstable();
    ext.dedup();

    // Versioned words (several entries writing the same word) need three
    // mechanisms on top of the plain output diff (see the module doc):
    //  - each entry diffs against ITS OWN previous outputs (handled at
    //    runtime via per-entry snapshots);
    //  - when an entry runs, every LATER writer of the words it writes is
    //    re-marked ("clobber repair", the `repair` lists): the run put an
    //    intermediate version into the buffer on top of the final one;
    //  - a reader scheduled strictly BETWEEN two writers of a word needs
    //    the intermediate version present in the buffer whenever it runs,
    //    which only holds if the earlier writers ran this settle — those
    //    words' writers and intermediate readers go to `always_run`.
    let n_entries = entry_out_words.len();
    let mut word_writers: Vec<Vec<u32>> = vec![Vec::new(); resolver.total_words];
    let mut word_writer_gmasks: Vec<Vec<u8>> = vec![Vec::new(); resolver.total_words];
    for (e, (outs, gmasks)) in entry_out_words
        .iter()
        .zip(&entry_out_word_gmasks)
        .enumerate()
    {
        for (&w, &g) in outs.iter().zip(gmasks) {
            word_writers[w as usize].push(e as u32);
            word_writer_gmasks[w as usize].push(g);
        }
    }
    let mut repair: Vec<Vec<u32>> = vec![Vec::new(); n_entries];
    {
        // Group written (entry, bit interval, word range) per variable and
        // find SAME-BIT overlaps between different entries — field-disjoint
        // writers of a shared variable/word need neither repair nor
        // always_run (each bit has a single writer, and the sweep is
        // single-threaded so read-modify-write stores preserve each other).
        let mut per_var: crate::HashMap<u64, Vec<(u32, OutVar)>> = crate::HashMap::default();
        for (e, out_vars) in entry_out_vars.iter().enumerate() {
            for ov in out_vars {
                per_var.entry(ov.key).or_default().push((e as u32, *ov));
            }
        }
        let mut conflict_pairs = 0usize;
        let mut hole_vars = 0usize;
        let mut extra: Vec<u32> = Vec::new();
        for writes in per_var.values() {
            // Repair links along a SAME-SPAN writer chain are transitively
            // reduced to adjacent pairs: when `a` runs, marking only its
            // immediate same-span successor `b` suffices — `b`'s run marks
            // `c`, and so on, re-establishing the whole chain in schedule
            // order.  Distinct (partially overlapping) spans keep the full
            // pairwise links; the hole detection below stays pairwise
            // regardless (an intermediate reader may only overlap the
            // outer pair of a chain).
            let mut by_span: crate::HashMap<(u32, u32), Vec<u32>> = crate::HashMap::default();
            for (e, v) in writes {
                by_span.entry((v.lo, v.hi)).or_default().push(*e);
            }
            for chain in by_span.values_mut() {
                chain.sort_unstable();
                chain.dedup();
                for pair in chain.windows(2) {
                    repair[pair[0] as usize].push(pair[1]);
                }
            }
            for (i, (e1, v1)) in writes.iter().enumerate() {
                for (e2, v2) in &writes[i..] {
                    if e1 == e2 || v1.lo > v2.hi || v2.lo > v1.hi {
                        continue;
                    }
                    // Same-bit conflict between two entries.
                    conflict_pairs += 1;
                    let (lo_e, hi_e) = (*e1.min(e2), *e1.max(e2));
                    if (v1.lo, v1.hi) != (v2.lo, v2.hi) {
                        repair[lo_e as usize].push(hi_e);
                    }
                    // The earlier writer must run every settle: when a later
                    // guarded writer's condition turns false the base value
                    // has to be re-established, but the base's wake sources
                    // are behind the sweep (a guard input change wakes only
                    // the guarded writer), so change-driven marking can never
                    // reach it forward.  Its repair links then re-mark the
                    // rest of the version chain in schedule order.
                    extra.push(lo_e);
                    // Intermediate readers of the conflicting words need
                    // every earlier version present when they run.
                    let w0 = v1.w0.min(v2.w0);
                    let w1 = v1.w1.max(v2.w1);
                    let mut has_hole = false;
                    for w in w0..w1 {
                        for &rid in &consumers[w as usize] {
                            if rid > lo_e && rid < hi_e {
                                extra.push(rid);
                                has_hole = true;
                            }
                        }
                    }
                    if has_hole {
                        hole_vars += 1;
                        extra.push(lo_e);
                        extra.push(hi_e);
                    }
                }
            }
        }
        for r in &mut repair {
            r.sort_unstable();
            r.dedup();
        }
        always_run.extend(extra);
        always_run.sort_unstable();
        always_run.dedup();
        let repair_links: usize = repair.iter().map(|r| r.len()).sum();
        diag(&format!(
            "plan: {n_entries} entries, {conflict_pairs} same-bit writer pairs, \
             {hole_vars} with holes, {} always-run entries, {repair_links} repair links, \
             {} ext words, {} passes",
            always_run.len(),
            ext.len(),
            required_passes.max(1),
        ));
    }

    // Flatten the per-entry output words and pre-resolve each word's read
    // source, so the post-run diff loop needs no word-id decode.
    let mut out_words_flat = Vec::new();
    let mut out_src = Vec::new();
    let mut entry_out_off = Vec::with_capacity(n_entries + 1);
    entry_out_off.push(0u32);
    for outs in entry_out_words.iter() {
        for &w in outs.iter() {
            let w = w as usize;
            let (buf_len, off, flag) = if w < resolver.comb_words {
                (comb_bytes, w * 8, 0)
            } else {
                (ff_bytes, (w - resolver.comb_words) * 8, OUT_SRC_FF)
            };
            // Encode the byte offset inline only when it fits below the flag
            // bits (OUT_SRC_TAIL=1<<30, OUT_SRC_FF=1<<31); a buffer large
            // enough to push `off` into those bits falls back to the
            // word-indexed tail path, which re-derives the offset safely.
            let src = if off + 8 <= buf_len && off < OUT_SRC_TAIL as usize {
                flag | off as u32
            } else {
                flag | OUT_SRC_TAIL
            };
            out_words_flat.push(w as u32);
            out_src.push(src);
        }
        entry_out_off.push(out_words_flat.len() as u32);
    }

    let mut ext_pos = vec![0u32; resolver.total_words];
    for (i, &w) in ext.iter().enumerate() {
        ext_pos[w as usize] = i as u32 + 1;
    }

    let (
        event_plans,
        event_chunk_count,
        event_csr_off,
        event_csr,
        event_csr_gmask,
        event_onrun_words,
        event_co_ids,
    ) = build_event_plans(events, &resolver);

    // Second half of the VERYL_INCR_WORD_DBG diagnostic: who writes the
    // target words (settle entries / event statements / partial closure).
    if let Ok(dbg) = env::var("VERYL_INCR_WORD_DBG") {
        let targets: Vec<u32> = dbg.split(',').filter_map(|s| s.parse().ok()).collect();
        for (e, outs) in entry_out_words.iter().enumerate() {
            for &t in &targets {
                if outs.contains(&t) {
                    eprintln!(
                        "[word-dbg] word={t} writer=settle entry {e} always_run={}",
                        always_run.contains(&(e as u32)),
                    );
                }
            }
        }
        for (event, stmts) in events {
            let mut si = 0usize;
            for block in &stmts.0 {
                let deps_of = |d: &ChunkDeps| {
                    let mut ws = Vec::new();
                    for dep in &d.outs {
                        resolver.words(dep, &mut ws);
                    }
                    ws
                };
                match block {
                    ProtoStatementBlock::Interpreted(list) => {
                        for s in list {
                            let mut d = ChunkDeps::default();
                            collect_stmt_deps(s, &mut d);
                            let ws = deps_of(&d);
                            for &t in &targets {
                                if ws.contains(&t) {
                                    let mut masked: Vec<(u32, u8)> = Vec::new();
                                    for dep in &d.outs {
                                        resolver.words_masked(dep, &mut masked);
                                    }
                                    let m: Vec<_> =
                                        masked.iter().filter(|&&(w, _)| w == t).collect();
                                    let raw: Vec<_> = d
                                        .outs
                                        .iter()
                                        .filter(|dep| {
                                            let mut v = Vec::new();
                                            resolver.words(dep, &mut v);
                                            v.contains(&t)
                                        })
                                        .collect();
                                    eprintln!(
                                        "[word-dbg] word={t} writer=event {event:?} interp stmt {si} masked={m:?} raw_deps={raw:?}"
                                    );
                                }
                            }
                            si += 1;
                        }
                    }
                    ProtoStatementBlock::Compiled(artifact) => {
                        if let Some(d) = artifact.deps.as_deref() {
                            let ws = deps_of(d);
                            for &t in &targets {
                                if ws.contains(&t) {
                                    eprintln!(
                                        "[word-dbg] word={t} writer=event {event:?} chunk stmt {si}"
                                    );
                                }
                            }
                        }
                        si += 1;
                    }
                }
            }
        }
        for &t in &targets {
            if partial_out_words.contains(&t) {
                eprintln!("[word-dbg] word={t} writer=partial closure");
            }
            if ext.contains(&t) {
                eprintln!("[word-dbg] word={t} in ext_comb_words");
            }
        }
    }

    Some(IncrPlan {
        comb_words: resolver.comb_words,
        total_words: resolver.total_words,
        n_entries,
        always_run,
        out_words: out_words_flat,
        out_src,
        entry_out_off,
        repair,
        consumers,
        consumer_gmasks,
        word_writers,
        word_writer_gmasks,
        ext_comb_words: ext,
        ext_pos,
        required_passes: required_passes.max(1),
        event_plans,
        event_chunk_count,
        event_csr_off,
        event_csr,
        event_csr_gmask,
        partial_out_words,
        event_onrun_words,
        event_co_ids,
    })
}

/// Per-simulator mutable state for the incremental settle.
#[derive(Default)]
pub struct IncrState {
    pub inited: bool,
    /// Bindings performed so far (0 = never bound, 1 = the build-time
    /// plan).  Entry ids, chunk ids and word ids are stable only within one
    /// generation, so every structure indexed by them is rebuilt on a
    /// [`IncrState::rebind`].
    pub generation: u32,
    /// Address of the plan this state is bound to; debug-asserted at the
    /// settle entry so a stale binding cannot go unnoticed.
    pub bound_plan: usize,
    /// Requested next generation, applied at the simulator's single swap
    /// point (`Simulator::apply_incr_swap`).  `Some(None)` retires the
    /// plan.  Verdicts are reached inside `&self` methods, so they queue
    /// the swap rather than performing it.
    pub pending_swap: Option<Option<Arc<IncrPlan>>>,
    /// Settles in the current generation — the auto-abandon window clock.
    /// (The cumulative `stats_settles` cannot serve: it never re-arms the
    /// window after a swap.)
    pub gen_settles: u64,
    /// Mark-driven event skip: dirty bit per skippable event chunk
    /// (`IncrPlan::event_chunk_count` bits, all-ones at init so every
    /// chunk runs its first fire).  Set by the settle diff / seed scans
    /// through `event_csr` and by `EventPlan::on_run`; cleared by
    /// `eval_event_stmts` when the chunk runs.
    pub event_dirty: Vec<u64>,
    /// Dirty-seed lists: words possibly changed since the last settle,
    /// diffed by the next settle in place of the full seed scans.
    /// `pending_ff` holds ff-local word indices (fed by the commit's
    /// compare-on-apply and the partial-settle diff); `pending_ext` holds
    /// global comb word ids (fed by the event on-run diff, the
    /// partial-settle diff, and the input-clock toggles).
    pub pending_ff: Vec<u32>,
    pub pending_ext: Vec<u32>,
    /// Settles that ran the full seed scans because `seed_full` was set
    /// (diagnostics; excludes the first settle).
    pub stats_seed_full: u64,
    /// Fall back to the full seed scans next settle: set by write paths
    /// whose targets aren't tracked word-precisely (testbench statement
    /// execution, `Simulator::set`, components, plan-less event lists) and
    /// by pending-list overflow.
    pub seed_full: bool,
    /// Mark-source diagnostics: event-chunk marks set by
    /// [out-diff, ext seed, ff seed, partial closure, on_run].
    pub stats_event_marks: [u64; 5],
    /// Previous values of `plan.partial_out_words` (aligned), diffed by
    /// `Ir::mark_event_partial` after each partial settle.
    pub prev_partial: Vec<u64>,
    /// Previous-settle ff buffer, as words.
    pub prev_ff: Vec<u64>,
    /// Previous values of `plan.ext_comb_words` (aligned).
    pub prev_ext: Vec<u64>,
    /// Dirty-entry bitmap.
    pub dirty: Vec<u64>,
    /// Flat per-entry run records (see `build_flat`): call target + args,
    /// repair marks, and per-output-word `{src, prev value, dirty marks}`
    /// interleaved into one sequential stream, so a run touches the blob,
    /// the dirty bitmap and the value buffers and nothing else.  The
    /// previous-output snapshots live INSIDE the blob (diff baseline — not
    /// the current buffer, whose value for a versioned word is whichever
    /// version was written last).
    pub blob: Vec<u64>,
    /// entry → start index of its record in `blob`.
    pub blob_off: Vec<u32>,
    /// Deduplicated call-arg tuples `(ff, comb, log, ff_delta)`; records
    /// store an index into this table (uniform args across a module's
    /// chunks make this a 1-2 entry, always-hot table instead of 32
    /// record bytes).
    pub arg_sets: Vec<(u64, u64, u64, i64)>,
    /// Settle index (1-based `stats_settles + 1`) at which each entry last
    /// ran; validate-mode forensics.
    pub last_run: Vec<u64>,
    /// Validate-mode forensics: the true pre-settle comb buffer, set by the
    /// dual-run wrapper before calling `dump_divergence`.
    pub forensics_pre_settle: Option<Vec<u8>>,
    /// Runtime auto-abandon (see [`abandon_threshold_pct`]): once set, the
    /// settle takes the baseline full sweep and the event skip is off.
    pub abandoned: bool,
    /// `stats_runs` snapshot taken at settle [`ABANDON_WARMUP`]; the
    /// abandon check keys off `stats_settles`, so no sentinel is needed.
    pub abandon_runs0: u64,
    pub stats_settles: u64,
    pub stats_runs: u64,
    /// Runs whose output diff found no changed word — wasted work from
    /// coarse (word-granular / chunk-granular) triggering.
    pub stats_runs_nochange: u64,
    pub stats_seed_words: u64,
}

impl IncrState {
    /// Build the flat run records.  Record layout (u64 words):
    ///
    /// ```text
    /// [0]  call target (`FuncPtr` bits; 0 = interpret via dispatch)
    /// [1]  n_repair (low 32) | n_out (bits 32..56) | arg-set idx (top 8)
    /// ⌈n_repair/2⌉ × [entry id pair]            (unconditional re-marks)
    /// n_out × [src (low 32, OUT_SRC_* encoding) | n_marks (high 32)]
    /// n_out × [mark index (low 32, absolute) | word id (high 32)]
    /// n_out × [prev value]
    /// mark pool: packed entry-id pairs          (marked when value changed)
    /// ```
    ///
    /// Call args live in the deduplicated `arg_sets` side table.
    ///
    /// The out-word headers and prev values are FIXED-STRIDE dense arrays:
    /// the diff loop's addresses must not depend on the previous
    /// iteration's data, or the loop serializes on load latency (measured
    /// 30% settle regression from a variable-stride record walk).  Mark
    /// lists are reached through the per-out mark index only when the word
    /// actually changed; ids are packed two per word — same data volume as
    /// the CSR form but with no per-list Vec indirection.
    pub fn build_flat(&mut self, plan: &IncrPlan, stmts: &[Statement]) {
        fn push_ids(blob: &mut Vec<u64>, ids: &[u32]) {
            for pair in ids.chunks(2) {
                let mut w = pair[0] as u64;
                if let Some(&hi) = pair.get(1) {
                    w |= (hi as u64) << 32;
                }
                blob.push(w);
            }
        }
        let mut blob: Vec<u64> = Vec::new();
        let mut event_only_kept = 0u64;
        let mut blob_off = Vec::with_capacity(plan.n_entries);
        let mut arg_sets: Vec<(u64, u64, u64, i64)> = Vec::new();
        for (e, stmt) in stmts.iter().enumerate().take(plan.n_entries) {
            blob_off.push(u32::try_from(blob.len()).expect("incr blob exceeds u32 index space"));
            let (func, asi) = match stmt {
                Statement::Compiled(c) => {
                    let args = (
                        c.ff as usize as u64,
                        c.comb as usize as u64,
                        c.log_buf as usize as u64,
                        c.ff_delta as i64,
                    );
                    let asi = arg_sets.iter().position(|a| *a == args).unwrap_or_else(|| {
                        arg_sets.push(args);
                        arg_sets.len() - 1
                    });
                    if asi > u8::MAX as usize {
                        // Arg-set table overflow: run this entry through the
                        // (slower but arg-exact) dispatch fallback.
                        (0u64, 0usize)
                    } else {
                        (c.artifact.func as usize as u64, asi)
                    }
                }
                _ => (0, 0),
            };
            blob.push(func);
            let s = plan.entry_out_off[e] as usize;
            let t = plan.entry_out_off[e + 1] as usize;
            // Filter each out word's consumer list, and drop undiffable
            // words entirely.  With a single settle pass, waking the writer
            // itself is dead work (the bit would be dropped at settle end),
            // so intra-chunk temporaries — whose only reader is their own
            // entry — carry no consumers at all; unless the word is an FF
            // word (whose diff also maintains the seed snapshot), it can
            // be skipped by the per-run diff loop outright.  Chunk-internal
            // dataflow dominates at larger chunk sizes, so this removes
            // most of the diff work per run.
            let single_pass = plan.required_passes <= 1;
            // (src, word id, mark ids, mark read gmasks)
            type KeptWord = (u32, u32, Vec<u32>, Vec<u8>);
            let mut kept: Vec<KeptWord> = Vec::new();
            for fi in s..t {
                let src = plan.out_src[fi];
                let wid = plan.out_words[fi];
                let cons = &plan.consumers[wid as usize];
                let cgm = &plan.consumer_gmasks[wid as usize];
                let (filtered, fgm): (Vec<u32>, Vec<u8>) = if single_pass {
                    cons.iter()
                        .copied()
                        .zip(cgm.iter().copied())
                        .filter(|&(c, _)| c != e as u32)
                        .unzip()
                } else {
                    (cons.clone(), cgm.clone())
                };
                let is_ff_word = wid as usize >= plan.comb_words;
                // A word with no comb consumers may still feed event chunks
                // (an FF D-input read by nothing else): the mark-driven
                // event skip diffs it through this same record, so it must
                // stay diffable.
                let has_event_consumer = plan.event_chunk_count != 0 && {
                    let a = plan.event_csr_off[wid as usize] as usize;
                    let b = plan.event_csr_off[wid as usize + 1] as usize;
                    a != b
                };
                if filtered.is_empty() && !is_ff_word && !has_event_consumer {
                    continue;
                }
                if filtered.is_empty() && !is_ff_word {
                    event_only_kept += 1;
                }
                kept.push((src, wid, filtered, fgm));
            }
            let n_out = kept.len();
            debug_assert!(n_out < 1 << 24);
            blob.push(plan.repair[e].len() as u64 | (n_out as u64) << 32 | (asi as u64) << 56);
            push_ids(&mut blob, &plan.repair[e]);
            let hdr = blob.len();
            // Reserve the three dense arrays, then append the mark pool and
            // backpatch each out word's absolute mark index.  Each word's
            // pool is `ceil(n/2)` id words followed by `ceil(n/8)` words of
            // packed byte-granularity read masks (mark `m`'s mask lives at
            // byte `m` of that trailer).
            blob.resize(hdr + 3 * n_out, 0);
            for (i, (src, wid, cons, gms)) in kept.iter().enumerate() {
                let mark = blob.len();
                push_ids(&mut blob, cons);
                for chunk in gms.chunks(8) {
                    let mut w = 0u64;
                    for (bi, &g) in chunk.iter().enumerate() {
                        w |= (g as u64) << (bi * 8);
                    }
                    blob.push(w);
                }
                blob[hdr + i] = *src as u64 | (cons.len() as u64) << 32;
                blob[hdr + n_out + i] =
                    u32::try_from(mark).expect("incr blob exceeds u32 index space") as u64
                        | (*wid as u64) << 32;
                // prev value at blob[hdr + 2*n_out + i] stays 0; the first
                // settle runs every entry and fills it.
            }
        }
        if event_only_kept > 0 && env::var("VERYL_INCR_EVENT_DIAG").as_deref() == Ok("1") {
            eprintln!("=== incr build_flat: event-only kept words={event_only_kept} ===");
        }
        self.blob = blob;
        self.blob_off = blob_off;
        self.arg_sets = arg_sets;
    }
}

/// Pending-list cap: past this, precision no longer pays and the next
/// settle falls back to the full seed scans.
pub const PENDING_CAP: usize = 8192;

impl IncrState {
    /// Install `plan` as the live generation, dropping every piece of
    /// generation-local state.  Entry, chunk and word ids are stable only
    /// within a generation (fused contract v2 §1), so nothing indexed by
    /// them may survive: clearing `inited` routes the next settle through
    /// the first-settle path, which rebuilds the flat records, both dirty
    /// bitmaps and every diff snapshot from the new plan.  That makes a
    /// swap **value-neutral by construction** — it costs one full settle
    /// plus one all-dirty event pass, nothing more.
    ///
    /// What survives is what the contract allows: the cumulative statistics
    /// (their generation-local baselines are resynced here) and the plan.
    pub fn rebind(&mut self, plan: Option<&IncrPlan>) {
        self.inited = false;
        self.generation += 1;
        self.bound_plan = plan.map_or(0, |p| std::ptr::from_ref(p) as usize);
        // Indexed by entry id.
        self.dirty.clear();
        self.blob.clear();
        self.blob_off.clear();
        self.arg_sets.clear();
        self.last_run.clear();
        // Indexed by chunk id / word id.
        self.event_dirty.clear();
        self.prev_ff.clear();
        self.prev_ext.clear();
        self.prev_partial.clear();
        // Pending ids belong to the retired generation's word space.
        self.pending_ff.clear();
        self.pending_ext.clear();
        self.seed_full = true;
        // Generation-local baselines of the auto-abandon window.
        self.gen_settles = 0;
        self.abandon_runs0 = self.stats_runs;
    }

    /// Queue a generation swap for the next swap point; `None` retires the
    /// plan (the full settle takes over permanently).
    pub fn request_swap(&mut self, plan: Option<Arc<IncrPlan>>) {
        self.pending_swap = Some(plan);
    }

    /// Borrow the mutable half of a mark; see [`IncrPlan::mark_word`].
    #[inline(always)]
    pub fn sink(&mut self) -> MarkSink<'_> {
        MarkSink {
            dirty: &mut self.dirty,
            event_dirty: &mut self.event_dirty,
            pending_ff: &mut self.pending_ff,
            pending_ext: &mut self.pending_ext,
            seed_full: &mut self.seed_full,
            stats_event_marks: &mut self.stats_event_marks,
        }
    }

    /// Record a possibly-changed FF word (ff-local index) for the next
    /// settle's dirty-seed diff.
    #[inline]
    pub fn note_ff_write(&mut self, w: u32) {
        if self.pending_ff.len() >= PENDING_CAP {
            self.seed_full = true;
        } else {
            self.pending_ff.push(w);
        }
    }

    /// Record a possibly-changed ext comb word (global word id).
    #[inline]
    pub fn note_ext_write(&mut self, w: u32) {
        if self.pending_ext.len() >= PENDING_CAP {
            self.seed_full = true;
        } else {
            self.pending_ext.push(w);
        }
    }

    /// An untracked write path touched the buffers: full seed scans next
    /// settle.
    #[inline]
    pub fn force_full_seed(&mut self) {
        self.seed_full = true;
    }

    /// Validate-mode forensics: for the diverged `word`, report its writers
    /// and, for writers that did not run, which of their input words changed
    /// under the full evaluation (each such word should have triggered the
    /// writer) plus that input's own writers — pinpointing the broken link.
    pub fn dump_divergence(&self, plan: &IncrPlan, word: usize, pre_comb: &[u8], full_comb: &[u8]) {
        let mut visited = crate::HashSet::default();
        self.dump_divergence_rec(plan, word, pre_comb, full_comb, 0, &mut visited);
    }

    /// Recursive divergence walk: from the diverged `word`, follow each
    /// ran-writer's diverging INPUT words up the dataflow until reaching an
    /// entry whose final inputs all agree — the entry that ran against a
    /// transient value (or a writer that never ran at all).
    fn dump_divergence_rec(
        &self,
        plan: &IncrPlan,
        word: usize,
        pre_comb: &[u8],
        full_comb: &[u8],
        depth: usize,
        visited: &mut crate::HashSet<usize>,
    ) {
        if depth > 12 || !visited.insert(word) {
            return;
        }
        let pad = "  ".repeat(depth);
        let settle = self.stats_settles; // just-finished settle stamped this
        let ran = |e: usize| self.last_run.get(e).copied().unwrap_or(0) == settle;
        let writers =
            |w: usize| -> Vec<usize> { plan.word_writers[w].iter().map(|&e| e as usize).collect() };
        let entry_ins = |e: usize| -> Vec<u32> {
            let mut ins = Vec::new();
            for (w, cs) in plan.consumers.iter().enumerate() {
                if cs.contains(&(e as u32)) {
                    ins.push(w as u32);
                }
            }
            ins
        };
        for e in writers(word) {
            eprintln!(
                "[incr forensics] {pad}word {word}: writer entry {e} ran={}",
                ran(e)
            );
            if ran(e) {
                // The writer ran yet the word diverged: either it ran too
                // early (an input settled to a different value afterwards)
                // or an upstream producer is wrong.  Follow inputs whose
                // FINAL values differ between the two evaluations; if none
                // differ, this entry is the culprit (ran on a transient).
                let mut clean = true;
                for w in entry_ins(e) {
                    let w = w as usize;
                    if w >= plan.comb_words {
                        continue;
                    }
                    let a = read_word(pre_comb, w);
                    let b = read_word(full_comb, w);
                    if a != b {
                        clean = false;
                        eprintln!(
                            "[incr forensics] {pad}  ran writer {e} input word {w} differs in \
                             final state ({a:#x} vs {b:#x}); descending",
                        );
                        self.dump_divergence_rec(plan, w, pre_comb, full_comb, depth + 1, visited);
                    }
                }
                if clean {
                    eprintln!(
                        "[incr forensics] {pad}  ran writer {e}: all final inputs agree — \
                         ran against a transient value (ordering hole)",
                    );
                }
                continue;
            }
            for w in entry_ins(e) {
                let w = w as usize;
                if w >= plan.comb_words {
                    continue; // ff seeds handled separately; report comb only
                }
                let pre = read_word(pre_comb, w);
                let full = read_word(full_comb, w);
                if pre != full {
                    let ws: Vec<(usize, bool)> =
                        writers(w).into_iter().map(|e2| (e2, ran(e2))).collect();
                    eprintln!(
                        "[incr forensics] {pad}  entry {e} input word {w} changed under full \
                         ({pre:#x} -> {full:#x}) but did not trigger; its writers: {ws:?}",
                    );
                }
            }
            // The check above compares the two FINAL states, which cannot
            // tell "input never changed" from "input changed identically on
            // both sides but the wake was lost".  When the caller passes the
            // true pre-settle state via `pre_settle`, list the inputs that
            // changed THIS settle under full, with the consumer-link gmask
            // and each writer's ran stamp — the lost wake is in this list.
            if let Some(pre_settle) = self.forensics_pre_settle.as_deref() {
                for w in entry_ins(e) {
                    let w = w as usize;
                    if w >= plan.comb_words || w * 8 + 8 > pre_settle.len() {
                        continue;
                    }
                    let p = read_word(pre_settle, w);
                    let f = read_word(full_comb, w);
                    if p == f {
                        continue;
                    }
                    let gm: Vec<u8> = plan.consumers[w]
                        .iter()
                        .zip(&plan.consumer_gmasks[w])
                        .filter(|&(&c, _)| c == e as u32)
                        .map(|(_, &g)| g)
                        .collect();
                    let dgm = byte_nonzero_mask(p ^ f);
                    let ws: Vec<(usize, bool)> =
                        writers(w).into_iter().map(|e2| (e2, ran(e2))).collect();
                    eprintln!(
                        "[incr forensics] {pad}  entry {e} input word {w} changed this settle \
                         ({p:#x} -> {f:#x}, dgm={dgm:#x}) consumer gmasks={gm:?} ext={} \
                         writers: {ws:?}",
                        plan.ext_pos[w],
                    );
                }
            }
        }
    }
}

impl IncrState {
    pub fn dump_stats(&self, n_entries: usize) {
        if self.stats_settles == 0 {
            return;
        }
        let n = self.stats_settles as f64;
        eprintln!(
            "=== incr settle stats: settles={} avg runs/settle={:.1}/{} ({:.2}%) avg seed words={:.1} ===",
            self.stats_settles,
            self.stats_runs as f64 / n,
            n_entries,
            self.stats_runs as f64 / n * 100.0 / n_entries.max(1) as f64,
            self.stats_seed_words as f64 / n,
        );
        eprintln!(
            "=== incr no-output-change runs: {:.1}% ({}/{}) ===",
            self.stats_runs_nochange as f64 * 100.0 / self.stats_runs.max(1) as f64,
            self.stats_runs_nochange,
            self.stats_runs,
        );
    }
}

/// Tail-safe unaligned 64-bit word read at word index `w` of `buf`.
/// Hot path of the incremental settle (seed scans + per-run output diffs),
/// so the in-bounds case is a single unaligned load.
#[inline(always)]
pub fn read_word(buf: &[u8], w: usize) -> u64 {
    let start = w * 8;
    if start + 8 <= buf.len() {
        // SAFETY: start + 8 <= len just checked.
        u64::from_le(unsafe { (buf.as_ptr().add(start) as *const u64).read_unaligned() })
    } else if start < buf.len() {
        let mut b = [0u8; 8];
        let n = buf.len() - start;
        b[..n].copy_from_slice(&buf[start..]);
        u64::from_le_bytes(b)
    } else {
        0
    }
}
