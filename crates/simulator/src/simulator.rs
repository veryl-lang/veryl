use crate::backend::{CompiledWhole, DispatchOutcome};
use crate::component::loader::ComponentError;
use crate::component::runtime::{RuntimeComponent, build_components};
use crate::ir::write_log::{
    WriteLogBuffer, clear_event_write_log, ff_commit_from_log, ff_commit_from_log_watched,
    set_event_write_log,
};
use crate::ir::{
    Event, Ir, ModuleVariables, Statement, Value, VarId, VarPath, dispatch_stmt_fast,
    read_native_value, write_native_value,
};
use crate::residency;
use crate::wave_dumper::{DumpVar, WaveDumper};
use smallvec::SmallVec;
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
#[cfg(feature = "profile")]
use std::time::Instant;
use veryl_analyzer::value::MaskCache;

#[cfg(feature = "profile")]
#[derive(Default, Debug)]
pub struct SimProfile {
    pub step_count: u64,
    pub settle_comb_count: u64,
    pub comb_eval_count: u64,
    pub extra_pass_count: u64,
    pub converged_first_try: u64,
    pub settle_comb_ns: u64,
    pub event_eval_ns: u64,
    pub ff_swap_ns: u64,
    pub eval_comb_full_ns: u64,
}

#[cfg(not(feature = "profile"))]
#[derive(Default, Debug)]
pub struct SimProfile;

pub struct Simulator {
    pub ir: Ir,
    pub time: u64,
    pub dump: Option<WaveDumper>,
    dump_vars: Vec<DumpVar>,
    pub mask_cache: MaskCache,
    comb_dirty: bool,
    pub profile: SimProfile,
    /// Which testbench statements are known not to invalidate the comb.
    /// Empty (the default) settles after every testbench statement.
    pub(crate) tb_dirty: crate::tb_dirty::TbDirtyFilter,
    last_event: Option<Event>,
    last_event_stmts: *const Vec<Statement>,
    /// Whole-event AOT-C handle for `last_event`, cached alongside
    /// `last_event_stmts` (same predicate, same post-construction-immutable
    /// `whole_events` invariant) so the hot path skips a per-cycle
    /// `whole_events` HashMap probe + `Arc` clone.  `None` = no whole-event
    /// backend for `last_event`.  Points into `self.ir.whole_events`'s `Arc`,
    /// which is never mutated after `Ir` construction.
    last_whole_event: Option<*const dyn CompiledWhole>,
    /// Previous-step derived-clock values (sampled at master=0).  Empty
    /// when no derived clocks; otherwise used for 0→1 edge detection.
    prev_derived_clock_values: Vec<u8>,
    /// Scratch for the master-high sample below.  A field, not a local: the
    /// step is the innermost loop and its length is fixed for the run.
    derived_clock_high: Vec<u8>,
    /// Same rationale: per-step scratch for the fired flags and the
    /// refreshed clock values of `step_with_derived_clocks`.
    fired_mask_scratch: Vec<bool>,
    new_values_scratch: Vec<u8>,
    /// Whether each derived reset READ AS ASSERTED at the last check, so a
    /// 0→1 here is the assertion whichever polarity the net has.  Checked
    /// after every commit within a step, not once per step: the assertion
    /// is a consequence of a commit, and the block it reaches must run
    /// before anything samples the state that commit produced.
    prev_derived_reset_asserted: Vec<u8>,
    /// Env-gated `VERYL_WRITE_LOG_DIAG=1` diagnostics for the write-log
    /// commit path.  Accumulated across the run; `dump` is invoked
    /// automatically when the cycle counter crosses a logarithmic
    /// checkpoint (doubling cadence, capped at 1 M cycles).
    pub write_log_diag: WriteLogDiag,
    /// Env-gated `VERYL_EVENT_DIAG=1` per-statement time attribution for
    /// event evaluation: (event, stmt index) → (cumulative ns, fires).
    pub event_diag: Option<HashMap<(Event, usize), (u64, u64)>>,
    /// Stop the testbench after this many clock cycles; `None` runs to completion.
    pub cycle_limit: Option<u64>,
    pub cycle_count: u64,
    /// Env-gated `VERYL_STEP_WATCH=path1,path2` debug watch: resolved
    /// variable pointers printed at each phase of `step_with_derived_clocks`.
    watch_vars: Vec<WatchVar>,
    /// User-defined component instances, created by `init_components`.
    pub components: Vec<RuntimeComponent>,
    /// True while the IR declares components but `init_components` has not
    /// run; stepping in that state would silently skip every hook.
    components_pending: bool,
    /// Waveform handles for component trace variables:
    /// (handle, component index, trace variable index).
    trace_dump_vars: Vec<(crate::wave_dumper::VarHandle, usize, usize)>,
    /// `(clock event, reset event)` installed by `step_in_reset`: components
    /// keep their own reset hook while the RTL takes an ordinary clock edge.
    component_event_override: Option<(Event, Event)>,
    /// The async-reset assertion edge to evaluate alongside this step's clock
    /// event, taken once.  See `step_in_reset`.
    pending_assertion_edge: Option<Event>,
    /// Settle filter (`VERYL_SETTLE_FILTER=0` opts out): variable spans of
    /// FF storage annotated with whether the comb can read them.  With it,
    /// `comb_dirty` is maintained precisely, so a step that changes no comb
    /// input (a divided clock's empty base tick) skips its settle.  `None`
    /// keeps the legacy conservative dirtying.
    settle_filter: Option<crate::tb_dirty::SpanTable>,
    /// Whether `set_input_clock_bit` must dirty the comb: true when the
    /// filter is off, or when a comb statement can read an input clock's
    /// level (the master toggle would then be visible to the settle).
    clock_toggle_dirties: bool,
    /// Events whose writes outside the FF log can reach a comb read AND
    /// cannot be value-compared (unboundable, unresolvable, or over the
    /// watch cap) — a fire of one dirties the comb.  Classified once in
    /// `new` from `Ir::event_comb_writes` against the span table; empty
    /// when the filter is off.
    dirty_events: crate::HashSet<Event>,
    /// Per event, the comb byte ranges `(offset, len)` its statements can
    /// write AND the comb can read — mostly FF-like registers held in comb
    /// storage (divided clock waves, misclassified FFs).  Snapshotted
    /// before the event fires and compared after: a changed byte dirties
    /// the comb, an unchanged fire stays invisible.  Disjoint from
    /// `dirty_events`.
    event_comb_watch: crate::HashMap<Event, (u32, u32)>,
    /// Flat storage behind `event_comb_watch`: each entry is a
    /// `(start, len)` slice of this pool.  Immutable after `new`, and the
    /// cache below holds indices — not pointers — so a future map insert
    /// cannot invalidate anything.
    watch_pool: Vec<(u32, u32)>,
    /// Scratch for the pre-fire snapshot (sized to the largest watch).
    comb_watch_scratch: Vec<u8>,
    /// Whether `last_event` is in `dirty_events` (cached alongside
    /// `last_event_stmts`).
    last_event_writes_comb: bool,
    /// `last_event`'s watch slice of `watch_pool`, cached alongside
    /// `last_event_stmts`.
    last_event_watch: Option<(u32, u32)>,
    /// `VERYL_SETTLE_FILTER_DIAG=1`: print settle counts on drop.
    settle_diag: bool,
    settles_run: u64,
    settles_skipped: u64,
    /// Diag attribution: how often each source dirtied a clean comb.
    dirty_from_event: u64,
    dirty_from_commit: u64,
    /// First few watched FF offsets the commit compare flagged (diag).
    dirty_commit_offsets: Vec<(usize, usize)>,
    /// Consecutive armed settles no skip interrupted; drives the auto-off
    /// in `filter_note_settle`.
    filter_miss_streak: u32,
    /// Settles remaining until a disarmed filter re-arms.
    filter_rearm_countdown: u32,
    /// Adaptive gate over `settle_filter`: false suspends the commit
    /// compares and event watches, dirtying every commit unconditionally.
    filter_armed: bool,
    /// `SpanTable::ff_never_reaches_comb`: the commit then keeps the comb
    /// clean without any per-entry compare.
    ff_unreachable: bool,
}

/// A design whose settles are never skipped pays the filter's commit
/// compares and event-watch snapshots for nothing — after this many
/// consecutive unskipped settles the filter disarms.
const SETTLE_FILTER_AUTO_OFF_STREAK: u32 = 1024;
/// A disarmed filter re-arms after this many settles, so a workload that
/// enters a skippable phase (an idle loop) gets its skips back.
const SETTLE_FILTER_REARM: u32 = 1 << 20;

/// Disjoint cover of comb-buffer element byte ranges, as runs of equally
/// sized elements `(start, end, elem_len)`, for resolving an event's
/// static comb write offset to the bytes a change can occupy.  An unpacked
/// array lays its elements out consecutively and coalesces into one run —
/// an entry per element would scale with total memory depth.  Overlapping
/// runs (aliased storage) collapse into a single-element run; mere
/// adjacency is kept separate so a watch range stays one element wide.
fn comb_element_cover(ir: &Ir) -> Vec<(usize, usize, usize)> {
    let comb_base = ir.comb_values.as_ptr() as usize;
    let comb_end = comb_base + ir.comb_values.len();
    let mut v: Vec<(usize, usize, usize)> = Vec::new();
    let mut stack = vec![&ir.module_variables];
    while let Some(vars) = stack.pop() {
        for var in vars.variables.values() {
            // A 4-state element stores its mask bytes after the payload.
            let len = crate::ir::value_size(var.native_bytes, ir.use_4state);
            let from = v.len();
            for &ptr in var.current_values.iter().chain(var.next_values.iter()) {
                let p = ptr as usize;
                if !(comb_base..comb_end).contains(&p) {
                    continue;
                }
                let s = p - comb_base;
                let e = (s + len).min(comb_end - comb_base);
                match v[from..].last_mut() {
                    Some(run) if run.1 == s && run.2 == len => run.1 = e,
                    _ => v.push((s, e, len)),
                }
            }
        }
        for child in &vars.children {
            stack.push(child);
        }
    }
    v.sort_unstable();
    let mut out: Vec<(usize, usize, usize)> = Vec::with_capacity(v.len());
    for (s, e, l) in v {
        match out.last_mut() {
            Some(p) if s < p.1 => {
                p.1 = p.1.max(e);
                p.2 = p.1 - p.0;
            }
            _ => out.push((s, e, l)),
        }
    }
    out
}

/// The settle filter's instantiation-invariant products: span table,
/// clock-toggle verdict, and the per-event comb-write classification
/// (scratch no comb statement reads is invisible; a bounded comb-reaching
/// write is value-compared per fire; everything else dirties every fire).
/// Cached through `Ir::settle_info` — one build serves every
/// instantiation of the module.
fn build_settle_info(ir: &Ir, diag: bool) -> crate::tb_dirty::SettleInfo {
    let table = crate::tb_dirty::SpanTable::build(ir);
    // The master toggle always returns to its baseline within the step,
    // but the mid-step settles run while it is high — so it is invisible
    // to the settle only when no comb statement can read an input clock's
    // level.
    let use_4state = ir.use_4state;
    let clock_toggle_dirties = ir
        .derived_clock_schedule
        .master_input_clocks
        .iter()
        .any(|id| match ir.module_variables.variables.get(id) {
            Some(v) => {
                let len = crate::ir::value_size(v.native_bytes, use_4state);
                table.write_may_reach_comb(v.current_values[0], len)
            }
            None => true,
        });
    let cover = comb_element_cover(ir);
    let elem_of = |off: usize| -> Option<(usize, usize)> {
        let i = cover.partition_point(|&(s, _, _)| s <= off);
        let &(s, e, l) = cover.get(i.checked_sub(1)?)?;
        if off >= e {
            return None;
        }
        let k = (off - s) / l;
        Some((s + k * l, (s + (k + 1) * l).min(e)))
    };
    // Beyond this an event is cheaper to settle than to compare.
    const WATCH_CAP_BYTES: usize = 1024;
    let mut max_watch = 0usize;
    let mut dirty_events: crate::HashSet<Event> = Default::default();
    let mut event_comb_watch: crate::HashMap<Event, (u32, u32)> = Default::default();
    let mut watch_pool: Vec<(u32, u32)> = Vec::new();
    for (event, writes) in &ir.event_comb_writes {
        let Some(offs) = writes else {
            dirty_events.insert(event.clone());
            continue;
        };
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut unresolved = false;
        for &(lo, hi) in offs {
            if lo < 0 || hi < lo {
                unresolved = true;
                break;
            }
            let (lo, hi) = (lo as usize, hi as usize);
            if !table.comb_change_may_reach_comb(lo, hi - lo + 1) {
                continue;
            }
            match (elem_of(lo), elem_of(hi)) {
                (Some(a), Some(b)) => ranges.push((a.0, b.1.max(a.1))),
                _ => {
                    unresolved = true;
                    break;
                }
            }
        }
        if unresolved {
            if diag {
                eprintln!("[settle_filter] event {event:?}: DIRTY (unresolved offset)");
            }
            dirty_events.insert(event.clone());
            continue;
        }
        if ranges.is_empty() {
            if diag {
                eprintln!("[settle_filter] event {event:?}: CLEAN");
            }
            continue;
        }
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for (s, e) in ranges {
            match merged.last_mut() {
                Some(p) if s < p.1 => p.1 = p.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        let total: usize = merged.iter().map(|&(s, e)| e - s).sum();
        if total > WATCH_CAP_BYTES {
            if diag {
                eprintln!("[settle_filter] event {event:?}: DIRTY (watch {total} bytes over cap)");
            }
            dirty_events.insert(event.clone());
            continue;
        }
        max_watch = max_watch.max(total);
        if diag {
            eprintln!(
                "[settle_filter] event {event:?}: WATCH {total} bytes in {} ranges",
                merged.len()
            );
        }
        let start = watch_pool.len() as u32;
        watch_pool.extend(merged.iter().map(|&(s, e)| (s as u32, (e - s) as u32)));
        event_comb_watch.insert(event.clone(), (start, merged.len() as u32));
    }
    crate::tb_dirty::SettleInfo {
        table,
        clock_toggle_dirties,
        dirty_events,
        event_comb_watch,
        watch_pool,
        max_watch,
    }
}

struct WatchVar {
    label: String,
    ptr: *const u8,
    native_bytes: usize,
    width: u32,
    /// Raw byte snapshot (payload + optional 4-state mask) for the
    /// change-only reporting used by `step_legacy`.
    last: Vec<u8>,
}

#[derive(Default)]
pub struct WriteLogDiag {
    pub enabled: bool,
    pub total_cycles: u64,
    pub total_entries: u64,
    pub max_entries_per_cycle: u32,
    pub cycles_with_entries: u64,
    next_print_cycle: u64,
}

impl WriteLogDiag {
    fn maybe_print(&mut self) {
        if !self.enabled {
            return;
        }
        if self.total_cycles >= self.next_print_cycle {
            self.next_print_cycle = self.next_print_cycle.saturating_mul(2).max(1_000_000);
            self.dump();
        }
    }

    pub fn dump(&self) {
        let avg = if self.cycles_with_entries > 0 {
            self.total_entries as f64 / self.cycles_with_entries as f64
        } else {
            0.0
        };
        eprintln!(
            "[write_log_diag] cycles={} cycles_with_entries={} total_entries={} max_per_cycle={} avg_per_active_cycle={:.2}",
            self.total_cycles,
            self.cycles_with_entries,
            self.total_entries,
            self.max_entries_per_cycle,
            avg,
        );
    }
}

impl Simulator {
    pub fn new(ir: Ir, dump: Option<WaveDumper>) -> Self {
        let n_derived = ir.derived_clock_schedule.clocks.len();
        let n_derived_resets = ir.derived_clock_schedule.resets.len();
        let components_pending = !ir.external_components.is_empty();
        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_vars: Vec::new(),
            mask_cache: MaskCache::default(),
            comb_dirty: true,
            profile: Default::default(),
            tb_dirty: Default::default(),
            last_event: None,
            last_event_stmts: std::ptr::null(),
            last_whole_event: None,
            prev_derived_clock_values: vec![0u8; n_derived],
            derived_clock_high: vec![0u8; n_derived],
            fired_mask_scratch: vec![false; n_derived],
            new_values_scratch: vec![0u8; n_derived],
            prev_derived_reset_asserted: vec![0u8; n_derived_resets],
            write_log_diag: WriteLogDiag {
                enabled: env::var("VERYL_WRITE_LOG_DIAG").as_deref() == Ok("1"),
                next_print_cycle: 1_000_000,
                ..Default::default()
            },
            event_diag: (env::var("VERYL_EVENT_DIAG").as_deref() == Ok("1")).then(HashMap::default),
            cycle_limit: None,
            cycle_count: 0,
            watch_vars: Vec::new(),
            components: Vec::new(),
            components_pending,
            trace_dump_vars: Vec::new(),
            component_event_override: None,
            pending_assertion_edge: None,
            settle_filter: None,
            clock_toggle_dirties: true,
            dirty_events: Default::default(),
            event_comb_watch: Default::default(),
            watch_pool: Vec::new(),
            comb_watch_scratch: Vec::new(),
            last_event_writes_comb: false,
            last_event_watch: None,
            settle_diag: env::var("VERYL_SETTLE_FILTER_DIAG").as_deref() == Ok("1"),
            settles_run: 0,
            settles_skipped: 0,
            dirty_from_event: 0,
            dirty_from_commit: 0,
            dirty_commit_offsets: Vec::new(),
            filter_miss_streak: 0,
            filter_rearm_countdown: 0,
            filter_armed: true,
            ff_unreachable: false,
        };

        // A comb statement writing FF storage (a dual-driven variable under
        // `#[allow(multiple_assign)]`) needs no standdown: comb outs are in
        // `comb_touched_offsets`, so those FF bytes lie in touched spans and
        // any event-side change to them dirties the comb through the commit
        // compare.  `--disable-ff-opt` does stand down — its comb-scope FF
        // writes bypass the compare.
        if env::var("VERYL_SETTLE_FILTER").as_deref() != Ok("0") && !ret.ir.disable_ff_opt {
            let diag = ret.settle_diag;
            let info = Arc::clone(
                ret.ir
                    .settle_info
                    .get_or_init(|| Arc::new(build_settle_info(&ret.ir, diag))),
            );
            ret.clock_toggle_dirties = info.clock_toggle_dirties;
            ret.dirty_events = info.dirty_events.clone();
            ret.event_comb_watch = info.event_comb_watch.clone();
            ret.watch_pool = info.watch_pool.clone();
            ret.comb_watch_scratch = vec![0u8; info.max_watch];
            let spans = info.table.rebased(&ret.ir);
            if ret.settle_diag {
                fn find_var_by_ptr(module: &ModuleVariables, ptr: *const u8) -> Option<String> {
                    for v in module.variables.values() {
                        if v.current_values.first().copied() == Some(ptr as *mut u8) {
                            return Some(v.path.to_string());
                        }
                    }
                    for child in &module.children {
                        if let Some(n) = find_var_by_ptr(child, ptr) {
                            return Some(format!("{}.{n}", child.name));
                        }
                    }
                    None
                }
                for (e, writes) in &ret.ir.event_comb_writes {
                    let Some(offs) = writes else {
                        eprintln!("[settle_filter] event {e:?}: UNBOUNDED writes");
                        continue;
                    };
                    let hits: Vec<String> = offs
                        .iter()
                        .filter(|&&(lo, hi)| {
                            lo < 0
                                || hi < lo
                                || spans
                                    .comb_change_may_reach_comb(lo as usize, (hi - lo) as usize + 1)
                        })
                        .take(6)
                        .map(|&(lo, _)| {
                            let name = if lo >= 0 {
                                let ptr = unsafe { ret.ir.comb_values.as_ptr().add(lo as usize) };
                                find_var_by_ptr(&ret.ir.module_variables, ptr)
                            } else {
                                None
                            };
                            format!("{lo:#x}={}", name.unwrap_or_else(|| "?".into()))
                        })
                        .collect();
                    if !hits.is_empty() {
                        eprintln!("[settle_filter] event {e:?}: comb-reaching writes {hits:?}");
                    }
                }
            }
            ret.ff_unreachable = spans.ff_never_reaches_comb();
            ret.settle_filter = Some(spans);
        }

        // VERYL_CONE_FF_DIAG=1: static breakdown of the cone-gate compare
        // sets by buffer, to size compare-set optimizations.
        if env::var("VERYL_CONE_FF_DIAG").as_deref() == Ok("1") && !ret.ir.cone_segments.is_empty()
        {
            let (mut ff_total, mut comb_total, mut pre_total) = (0usize, 0usize, 0usize);
            let mut rows: Vec<(usize, usize, usize, &str)> = Vec::new();
            for s in &ret.ir.cone_segments {
                let ff: usize = s
                    .compare
                    .iter()
                    .filter(|r| r.0)
                    .map(|r| (r.2 - r.1) as usize)
                    .sum();
                let comb: usize = s
                    .compare
                    .iter()
                    .filter(|r| !r.0)
                    .map(|r| (r.2 - r.1) as usize)
                    .sum();
                let pre: usize = s.compare_pre.iter().map(|r| (r.1 - r.0) as usize).sum();
                ff_total += ff;
                comb_total += comb;
                pre_total += pre;
                rows.push((ff, comb, pre, s.cone.as_str()));
            }
            let range_count: usize = ret
                .ir
                .cone_segments
                .iter()
                .map(|s| s.compare.len() + s.compare_pre.len())
                .sum();
            let replay_bytes: usize = ret
                .ir
                .cone_segments
                .iter()
                .flat_map(|s| s.replay.iter())
                .map(|r| (r.1 - r.0) as usize)
                .sum();
            eprintln!(
                "[cone_ff_diag] module={} segments={} compare bytes: ff={} comb={} pre(comb)={} in {} ranges (avg {:.1} B/range), replay bytes={} — ff share {:.1}%",
                ret.ir.name,
                ret.ir.cone_segments.len(),
                ff_total,
                comb_total,
                pre_total,
                range_count,
                (ff_total + comb_total + pre_total) as f64 / range_count.max(1) as f64,
                replay_bytes,
                100.0 * ff_total as f64 / (ff_total + comb_total + pre_total).max(1) as f64,
            );
            rows.sort_by_key(|&(ff, comb, pre, _)| std::cmp::Reverse(ff + comb + pre));
            for (ff, comb, pre, cone) in rows.iter().take(12) {
                eprintln!("[cone_ff_diag]   ff={ff:6} comb={comb:6} pre={pre:5}  {cone}");
            }
            let mut hist = [0usize; 8];
            let mut hist_bytes = [0usize; 8];
            for s in &ret.ir.cone_segments {
                for len in s
                    .compare
                    .iter()
                    .map(|r| (r.2 - r.1) as usize)
                    .chain(s.compare_pre.iter().map(|r| (r.1 - r.0) as usize))
                {
                    let b = match len {
                        0..=8 => 0,
                        9..=32 => 1,
                        33..=64 => 2,
                        65..=128 => 3,
                        129..=256 => 4,
                        257..=512 => 5,
                        513..=2048 => 6,
                        _ => 7,
                    };
                    hist[b] += 1;
                    hist_bytes[b] += len;
                }
            }
            eprintln!(
                "[cone_ff_diag] range histogram (count/bytes): ≤8:{}/{} ≤32:{}/{} ≤64:{}/{} ≤128:{}/{} ≤256:{}/{} ≤512:{}/{} ≤2K:{}/{} >2K:{}/{}",
                hist[0],
                hist_bytes[0],
                hist[1],
                hist_bytes[1],
                hist[2],
                hist_bytes[2],
                hist[3],
                hist_bytes[3],
                hist[4],
                hist_bytes[4],
                hist[5],
                hist_bytes[5],
                hist[6],
                hist_bytes[6],
                hist[7],
                hist_bytes[7],
            );
            // Overlap: bytes compared by SEVERAL segments — the dedupe
            // headroom a shared-summary scheme could reclaim.
            let mut events: Vec<(u32, i32)> = Vec::new();
            for s in &ret.ir.cone_segments {
                for r in s.compare.iter().filter(|r| !r.0) {
                    events.push((r.1, 1));
                    events.push((r.2, -1));
                }
                for r in &s.compare_pre {
                    events.push((r.0, 1));
                    events.push((r.1, -1));
                }
            }
            events.sort_unstable();
            let (mut depth, mut prev, mut uniq, mut multi) = (0i32, 0u32, 0usize, 0usize);
            for &(x, d) in &events {
                let span = (x - prev) as usize;
                if depth >= 1 {
                    uniq += span;
                }
                if depth >= 2 {
                    multi += span;
                }
                depth += d;
                prev = x;
            }
            eprintln!(
                "[cone_ff_diag] comb compare coverage: unique={uniq} bytes, of which shared(≥2 segs)={multi}; sum-with-multiplicity={}",
                comb_total + pre_total,
            );
        }

        // Reset nets start DEASSERTED: zeroed storage reads as ASSERTED on an
        // active-low reset and would hold every `if_reset` block from time 0.
        // A driven net is overwritten by the first comb settle, so this decides
        // only the nets an external driver owns.
        let reset_ids: Vec<VarId> = ret
            .ir
            .module_variables
            .variables
            .iter()
            .filter(|(_, x)| x.r#type.is_reset())
            .map(|(id, _)| *id)
            .collect();
        for id in reset_ids {
            ret.set_reset_level(&id, false);
        }

        if env::var("VERYL_DERIVED_CLOCK_DUMP").as_deref() == Ok("1") {
            fn find_var_by_ptr(
                module: &ModuleVariables,
                ptr: *const u8,
                prefix: &str,
            ) -> Option<String> {
                for v in module.variables.values() {
                    if v.current_values.first().copied() == Some(ptr as *mut u8) {
                        return Some(format!("{prefix}{}", v.path));
                    }
                }
                for child in &module.children {
                    if let Some(n) =
                        find_var_by_ptr(child, ptr, &format!("{prefix}{}.", child.name))
                    {
                        return Some(n);
                    }
                }
                None
            }
            for (i, clk) in ret.ir.derived_clock_schedule.clocks.iter().enumerate() {
                let raw = clk.current_offset.raw();
                let ptr = if raw >= 0 {
                    if clk.current_offset.is_ff() {
                        unsafe { ret.ir.ff_values.as_ptr().add(raw as usize) }
                    } else {
                        unsafe { ret.ir.comb_values.as_ptr().add(raw as usize) }
                    }
                } else {
                    std::ptr::null()
                };
                let name = find_var_by_ptr(&ret.ir.module_variables, ptr, "")
                    .unwrap_or_else(|| format!("{:?}@{raw}", clk.var_id));
                eprintln!(
                    "[derived_clock] [{i}] {name} is_ff={} master_gated={} has_events={}",
                    clk.current_offset.is_ff(),
                    clk.master_gated,
                    ret.ir
                        .event_statements
                        .contains_key(&Event::Clock(clk.var_id)),
                );
            }
            eprintln!(
                "[derived_clock] eval chunk: {} entries x {} passes",
                ret.ir.derived_clock_eval_stmts.len(),
                ret.ir.derived_clock_eval_passes,
            );
        }

        if env::var("VERYL_STEP_WATCH_LIST").as_deref() == Ok("1") {
            fn dump_tree(module: &ModuleVariables, prefix: &str) {
                for var in module.variables.values() {
                    eprintln!("[step_watch_list] {prefix}{}", var.path);
                }
                for child in &module.children {
                    dump_tree(child, &format!("{prefix}{}.", child.name));
                }
            }
            dump_tree(&ret.ir.module_variables, "");
        }

        if let Ok(watch) = env::var("VERYL_STEP_WATCH") {
            for path in watch.split(',').filter(|s| !s.is_empty()) {
                // Optional unpacked-array element selector: `a.b.gpr[4]`.
                let (base, elem) = match path.rfind('[') {
                    Some(pos) if path.ends_with(']') => {
                        match path[pos + 1..path.len() - 1].parse::<usize>() {
                            Ok(i) => (&path[..pos], Some(i)),
                            Err(_) => (path, None),
                        }
                    }
                    _ => (path, None),
                };
                // Collect ALL matches: one path can resolve to several
                // storages (see collect_var_meta_in_module).
                let mut found = Vec::new();
                Self::collect_var_meta_in_module(&ret.ir.module_variables, base, elem, &mut found);
                if found.is_empty() {
                    eprintln!("[step_watch] UNRESOLVED: {path}");
                }
                let multi = found.len() > 1;
                for (k, (ptr, native_bytes, width)) in found.into_iter().enumerate() {
                    let label = if multi {
                        format!("{path}#{k}")
                    } else {
                        path.to_string()
                    };
                    ret.watch_vars.push(WatchVar {
                        label,
                        ptr,
                        native_bytes,
                        width,
                        last: Vec::new(),
                    });
                }
            }
        }

        if let Some(dumper) = dump {
            ret.setup_dump(dumper);
        }

        // Seed prev values from the initial post-settle state.
        if n_derived > 0 || n_derived_resets > 0 {
            ret.do_settle_comb();
            ret.comb_dirty = false;
            for i in 0..n_derived {
                let clk = &ret.ir.derived_clock_schedule.clocks[i];
                ret.prev_derived_clock_values[i] = ret.read_derived_clock_bit(clk);
            }
            for i in 0..n_derived_resets {
                let rst = &ret.ir.derived_clock_schedule.resets[i];
                ret.prev_derived_reset_asserted[i] = ret.read_derived_reset_asserted(rst);
            }
        }

        ret
    }

    /// 1 while a derived clock reads its ACTIVE level, so every edge test is
    /// a 0→1 transition whichever edge the declared type selects.  X/Z → 0
    /// (matches the posedge SV rule).
    fn read_derived_clock_bit(&self, clk: &crate::ir::DerivedClock) -> u8 {
        let bit = self.read_edge_bit(clk.current_offset, clk.native_bytes);
        if clk.negedge { 1 - bit } else { bit }
    }

    /// 1 while a derived reset reads its ASSERTED level, so the assertion
    /// is a 0→1 transition whichever polarity the net has.
    fn read_derived_reset_asserted(&self, rst: &crate::ir::DerivedReset) -> u8 {
        let bit = self.read_edge_bit(rst.current_offset, rst.native_bytes);
        if rst.active_low { 1 - bit } else { bit }
    }

    /// Re-arm every derived reset against the state as it now reads.
    fn snapshot_derived_reset_levels(&mut self) {
        for i in 0..self.ir.derived_clock_schedule.resets.len() {
            let rst = &self.ir.derived_clock_schedule.resets[i];
            self.prev_derived_reset_asserted[i] = self.read_derived_reset_asserted(rst);
        }
    }

    /// Fire `Event::Clock` for a batch of derived clocks that reached their
    /// active level together, as one event region.  The caller must have
    /// settled first.
    fn fire_derived_clock_batch(&mut self, batch: &[usize]) {
        self.fire_derived_clock_batch_once(batch);
        self.settle_comb_if_stale();
        // A clock that rose BECAUSE this batch committed reaches its own
        // blocks here, in the same half period -- the rule the reset chain
        // below already follows.  It cannot be left to the next step: the
        // end-of-step snapshot records the new level, so the post-commit
        // chain loop would see no edge and the domain would never run.
        let n = self.ir.derived_clock_schedule.clocks.len();
        for _ in 0..n {
            let mut chained: SmallVec<[usize; 4]> = SmallVec::new();
            for i in 0..n {
                let clk = &self.ir.derived_clock_schedule.clocks[i];
                // Master-gated combinational clocks are the caller's to fire.
                if !clk.current_offset.is_ff() && clk.master_gated {
                    continue;
                }
                if self.prev_derived_clock_values[i] == 0 && self.read_derived_clock_bit(clk) == 1 {
                    chained.push(i);
                }
            }
            if chained.is_empty() {
                break;
            }
            for &i in &chained {
                self.prev_derived_clock_values[i] = 1;
            }
            self.fire_derived_clock_batch_once(&chained);
            self.settle_comb_if_stale();
        }
        // The batch can have asserted an async reset; that reaches its
        // blocks here, in the same half period.
        self.fire_asserted_derived_resets();
    }

    fn fire_derived_clock_batch_once(&mut self, batch: &[usize]) {
        let watch_enabled = !self.watch_vars.is_empty();
        let has_components = !self.components.is_empty();
        for &i in batch {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            if has_components {
                self.stage_components(&Event::Clock(vid));
            }
        }
        if watch_enabled {
            self.dump_watch("before_negedge_batch");
        }
        for &i in batch {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            self.eval_event_stmts(&Event::Clock(vid));
        }
        self.commit_event_log();
        if watch_enabled {
            self.dump_watch("after_negedge_batch");
        }
        for &i in batch {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            if has_components {
                self.fire_components(&Event::Clock(vid));
            }
        }
    }

    /// Fire `Event::Reset` for every derived reset that has just reached its
    /// asserted level, then settle and look again (an assertion can produce
    /// another).  The caller must have settled first.
    fn fire_asserted_derived_resets(&mut self) {
        let n = self.ir.derived_clock_schedule.resets.len();
        for _ in 0..=n {
            let mut fired: SmallVec<[usize; 4]> = SmallVec::new();
            for i in 0..n {
                let rst = &self.ir.derived_clock_schedule.resets[i];
                let asserted = self.read_derived_reset_asserted(rst);
                if self.prev_derived_reset_asserted[i] == 0 && asserted == 1 {
                    fired.push(i);
                }
                self.prev_derived_reset_asserted[i] = asserted;
            }
            if fired.is_empty() {
                return;
            }
            let has_components = !self.components.is_empty();
            if has_components {
                for &i in &fired {
                    let vid = self.ir.derived_clock_schedule.resets[i].var_id;
                    self.stage_components(&Event::Reset(vid));
                }
            }
            for &i in &fired {
                let vid = self.ir.derived_clock_schedule.resets[i].var_id;
                self.eval_event_stmts(&Event::Reset(vid));
            }
            self.commit_event_log();
            for &i in &fired {
                let vid = self.ir.derived_clock_schedule.resets[i].var_id;
                if has_components {
                    self.fire_components(&Event::Reset(vid));
                }
            }
            self.settle_comb_if_stale();
        }
    }

    fn read_edge_bit(&self, offset: crate::ir::variable::VarOffset, native_bytes: usize) -> u8 {
        let raw = offset.raw();
        if raw < 0 {
            return 0;
        }
        let off = raw as usize;
        let buf: &[u8] = if offset.is_ff() {
            &self.ir.ff_values
        } else {
            &self.ir.comb_values
        };
        if off >= buf.len() {
            return 0;
        }
        let payload_bit = buf[off] & 1;
        if self.ir.use_4state {
            let mask_off = off + native_bytes;
            if mask_off < buf.len() && (buf[mask_off] & 1) != 0 {
                return 0;
            }
        }
        payload_bit
    }

    fn set_input_clock_bit(&mut self, var_id: VarId, value: u8) {
        let Some(var) = self.ir.module_variables.variables.get(&var_id) else {
            return;
        };
        let ptr = var.current_values[0];
        let native_bytes = var.native_bytes;
        if ptr.is_null() {
            return;
        }
        // SAFETY: ptr is heap-stable for `self.ir`'s lifetime.
        // Writes LSB only (clocks are 1-bit).
        unsafe {
            let v = if value != 0 { 1u8 } else { 0u8 };
            *ptr = v;
            if self.ir.use_4state {
                *ptr.add(native_bytes) = 0;
            }
        }
        // Clocks toggle every step, so a full-scan fallback here would
        // defeat the dirty-seed path: record the covered word precisely
        // through the shared range entry.
        if self.clock_toggle_dirties {
            self.comb_dirty = true;
        }
    }

    /// Settle the comb list with whichever engine is configured.
    #[inline]
    fn do_settle_comb(&mut self) {
        self.settles_run += 1;
        self.ir.settle_comb(&mut self.mask_cache, &mut self.profile);
    }

    /// Full settle unless the filter's precise tracking proves the comb
    /// already matches the current state.  Without the filter this is
    /// unconditional — the legacy flag is not maintained mid-step, so
    /// `comb_dirty == false` proves nothing there.
    fn settle_comb_if_stale(&mut self) {
        if self.settle_filter.is_none() || self.comb_dirty {
            self.do_settle_comb();
            self.comb_dirty = false;
            self.filter_note_settle();
        } else {
            self.settles_skipped += 1;
            self.filter_miss_streak = 0;
            self.check_skipped_settle();
        }
    }

    /// Adaptive auto-off over the settle filter.  Called at each settle
    /// the filter failed to skip: a long enough miss streak disarms it
    /// (commits then dirty unconditionally, paying no compares), and a
    /// disarmed filter re-arms after `SETTLE_FILTER_REARM` settles.
    #[inline]
    fn filter_note_settle(&mut self) {
        if self.settle_filter.is_none() {
            return;
        }
        if self.filter_armed {
            self.filter_miss_streak += 1;
            if self.filter_miss_streak >= SETTLE_FILTER_AUTO_OFF_STREAK {
                self.filter_armed = false;
                self.filter_rearm_countdown = SETTLE_FILTER_REARM;
            }
        } else {
            self.filter_rearm_countdown -= 1;
            if self.filter_rearm_countdown == 0 {
                self.filter_armed = true;
                self.filter_miss_streak = 0;
            }
        }
    }

    /// `VERYL_SETTLE_FILTER_CHECK=1`: a skipped settle must be a no-op on
    /// the comb — run it anyway and panic on the first byte it would have
    /// changed.  Slow (clones comb per skip); diagnostics only, the settle
    /// filter's analog of `VERYL_CONE_GATE_CHECK`.
    fn check_skipped_settle(&mut self) {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let on = *ON.get_or_init(|| env::var("VERYL_SETTLE_FILTER_CHECK").as_deref() == Ok("1"));
        if !on || self.settle_filter.is_none() {
            return;
        }
        // The cone-gate state region (streaks, shadows) lives at the tail
        // of the comb buffer and mutates on every settle whether or not any
        // logic value moves — compare only the storage below it.
        let limit = (self.ir.cone_state_base as usize).min(self.ir.comb_values.len());
        let before = self.ir.comb_values[..limit].to_vec();
        self.do_settle_comb();
        self.comb_dirty = false;
        if let Some(i) = before
            .iter()
            .zip(self.ir.comb_values[..limit].iter())
            .position(|(a, b)| a != b)
        {
            panic!(
                "[settle_filter] WRONG SKIP: comb byte {i:#x} changed under a skipped settle (last event {:?})",
                self.last_event,
            );
        }
    }

    /// Dump the `VERYL_EVENT_DIAG=1` per-statement event-eval time
    /// attribution: top entries by cumulative time plus per-kind totals.
    /// No-op when the diagnostic is disabled.
    pub fn dump_event_diag(&self) {
        let Some(diag) = &self.event_diag else {
            return;
        };
        let classify = |event: &Event, idx: usize| -> String {
            let Some(stmts) = self.ir.event_statements.get(event) else {
                return "?".into();
            };
            match stmts.get(idx) {
                Some(Statement::Compiled(_)) => "cb".into(),
                Some(Statement::CompiledBatch(b)) => format!("batch(x{})", b.args.len()),
                Some(Statement::Assign(_)) => "assign".into(),
                Some(Statement::AssignDynamic(_)) => "assign_dyn".into(),
                Some(Statement::If(_)) => "if".into(),
                Some(Statement::Case(_)) => "case".into(),
                Some(Statement::For(_)) => "for".into(),
                Some(Statement::SequentialBlock(_)) => "seq".into(),
                Some(Statement::SystemFunctionCall(_)) => "sysfn".into(),
                Some(Statement::TbMethodCall { .. }) => "tbcall".into(),
                Some(Statement::Break) => "break".into(),
                None => "?".into(),
            }
        };
        let mut rows: Vec<_> = diag.iter().collect();
        rows.sort_by_key(|(_, (ns, _))| std::cmp::Reverse(*ns));
        let total_ns: u64 = rows.iter().map(|(_, (ns, _))| ns).sum();
        let total_fires: u64 = rows.iter().map(|(_, (_, f))| f).sum();
        eprintln!(
            "=== event diag: {} entries, total {:.1}ms, {} fires ===",
            rows.len(),
            total_ns as f64 / 1e6,
            total_fires
        );
        let mut by_kind: HashMap<String, (u64, u64)> = HashMap::default();
        for (k, v) in diag {
            let kind = classify(&k.0, k.1);
            let coarse = kind.split('(').next().unwrap_or("?").to_string();
            let e = by_kind.entry(coarse).or_insert((0, 0));
            e.0 += v.0;
            e.1 += v.1;
        }
        let mut kinds: Vec<_> = by_kind.into_iter().collect();
        kinds.sort_by_key(|(_, (ns, _))| std::cmp::Reverse(*ns));
        for (kind, (ns, fires)) in &kinds {
            eprintln!(
                "  kind {:10} {:8.1}ms {:>9} fires ({:.0}ns/fire)",
                kind,
                *ns as f64 / 1e6,
                fires,
                *ns as f64 / (*fires).max(1) as f64
            );
        }
        for ((event, idx), (ns, fires)) in rows.iter().take(40) {
            eprintln!(
                "  {:8.2}ms {:>9} fires ({:>5.0}ns/f) {:24} {:?}[{}]",
                *ns as f64 / 1e6,
                fires,
                *ns as f64 / (*fires).max(1) as f64,
                classify(event, *idx),
                event,
                idx
            );
        }
    }

    pub fn set(&mut self, port: &str, value: Value) {
        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.ir.ports.get(&port)
            && let Some(x) = self.ir.module_variables.variables.get_mut(id)
        {
            let mut value = value;
            value.trunc(x.width);
            unsafe {
                write_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    &value,
                );
            }
            self.comb_dirty = true;
        }
    }

    pub fn get(&mut self, port: &str) -> Option<Value> {
        self.ensure_comb_updated();

        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.ir.ports.get(&port)
            && let Some(x) = self.ir.module_variables.variables.get(id)
        {
            let value = unsafe {
                read_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    x.width as u32,
                    false,
                )
            };
            Some(value)
        } else {
            None
        }
    }

    /// Get a variable value by hierarchical path (e.g., "dut.cnt").
    /// Searches all module variables including children.
    pub fn get_var(&mut self, path: &str) -> Option<Value> {
        self.ensure_comb_updated();

        let target = VarPath::from_str(path).unwrap();
        Self::find_var_in_module(&self.ir.module_variables, &target, self.ir.use_4state)
    }

    fn find_var_in_module(
        module: &ModuleVariables,
        target: &VarPath,
        use_4state: bool,
    ) -> Option<Value> {
        // If target has multiple segments, try matching child module by name first
        if target.0.len() > 1 {
            for child in &module.children {
                if child.name == target.0[0] {
                    let sub = VarPath::from_slice(&target.0[1..]);
                    if let Some(v) = Self::find_var_in_module(child, &sub, use_4state) {
                        return Some(v);
                    }
                }
            }
        }

        // Look for a variable whose path matches exactly
        for var in module.variables.values() {
            if var.path == *target {
                let value = unsafe {
                    read_native_value(
                        var.current_values[0],
                        var.native_bytes,
                        use_4state,
                        var.width as u32,
                        false,
                    )
                };
                return Some(value);
            }
        }
        None
    }

    /// Watch-path resolver that collects EVERY match (generate
    /// instances share hierarchical names) and supports an unpacked-array
    /// element index (`elem` selects `current_values[elem]`).
    fn collect_var_meta_in_module(
        module: &ModuleVariables,
        target: &str,
        elem: Option<usize>,
        out: &mut Vec<(*const u8, usize, u32)>,
    ) {
        if let Some((head, rest)) = target.split_once('.') {
            for child in &module.children {
                if child.name.to_string() == head {
                    Self::collect_var_meta_in_module(child, rest, elem, out);
                }
            }
        }
        for var in module.variables.values() {
            if var.path.to_string() == target {
                let idx = elem.unwrap_or(0);
                if let Some(&ptr) = var.current_values.get(idx) {
                    out.push((ptr, var.native_bytes, var.width as u32));
                }
            }
        }
    }

    fn dump_watch(&self, tag: &str) {
        if self.watch_vars.is_empty() {
            return;
        }
        let mut line = format!("[step_watch] t={} {tag}:", self.time);
        for w in &self.watch_vars {
            let value = unsafe {
                read_native_value(w.ptr, w.native_bytes, self.ir.use_4state, w.width, false)
            };
            line.push_str(&format!(" {}={:x?}", w.label, value));
        }
        eprintln!("{line}");
    }

    /// Change-only watch used by `step_legacy`: prints a line per watched
    /// var whose raw storage bytes changed since the previous call.
    fn dump_watch_changes(&mut self, tag: &str) {
        let time = self.time;
        let use_4state = self.ir.use_4state;
        for w in &mut self.watch_vars {
            let span = w.native_bytes * (1 + use_4state as usize);
            let cur = unsafe { std::slice::from_raw_parts(w.ptr, span) };
            if w.last.as_slice() != cur {
                let value =
                    unsafe { read_native_value(w.ptr, w.native_bytes, use_4state, w.width, false) };
                eprintln!("[step_watch] t={time} {tag}: {}={:x?}", w.label, value);
                w.last = cur.to_vec();
            }
        }
    }

    pub fn ensure_comb_updated(&mut self) {
        if self.comb_dirty {
            #[cfg(feature = "profile")]
            let start = Instant::now();

            self.do_settle_comb();
            self.comb_dirty = false;

            #[cfg(feature = "profile")]
            {
                self.profile.settle_comb_ns += start.elapsed().as_nanos() as u64;
            }
        } else {
            self.check_skipped_settle();
        }
    }

    pub fn mark_comb_dirty(&mut self) {
        self.comb_dirty = true;
    }

    pub fn get_clock(&self, port: &str) -> Option<Event> {
        let port = VarPath::from_str(port).unwrap();
        self.ir.ports.get(&port).map(|id| Event::Clock(*id))
    }

    pub fn get_reset(&self, port: &str) -> Option<Event> {
        let port = VarPath::from_str(port).unwrap();
        let id = self.ir.ports.get(&port)?;
        let var = self.ir.module_variables.variables.get(id)?;
        var.r#type.is_reset().then_some(Event::Reset(*id))
    }

    /// Drives a reset net to its asserted or deasserted level — all it takes
    /// to put the design in or out of reset, since `if_reset` samples the
    /// level at the next clock edge.
    pub fn set_reset_level(&mut self, id: &VarId, asserted: bool) {
        let high = asserted != self.ir.reset_active_low(id);
        self.set_var_by_id(id, Value::new(high as u64, 1, false));
    }

    /// One clock edge with `reset` asserted around it, for driving a design
    /// directly.  A testbench instead holds the level across the whole
    /// assertion window and calls `step_in_reset` per edge.
    pub fn step_reset(&mut self, clock: &Event, reset: &Event) {
        let id = reset.var_id();
        if let Some(ref id) = id {
            self.set_reset_level(id, true);
        }
        self.step_in_reset(clock, reset, true);
        if let Some(ref id) = id {
            self.set_reset_level(id, false);
        }
    }

    /// One clock edge taken while `reset` is asserted by the caller.
    ///
    /// `assertion_edge` also fires the reset's own event in the same step,
    /// which is what reaches a block whose clock is not running (an async
    /// reset asserting into a gated-off domain).  Components have a reset
    /// hook of their own, so they are staged and fired with the reset event.
    pub fn step_in_reset(&mut self, clock: &Event, reset: &Event, assertion_edge: bool) {
        if assertion_edge {
            self.pending_assertion_edge = Some(reset.clone());
        }
        if self.components.is_empty() {
            self.step(clock);
        } else {
            self.component_event_override = Some((clock.clone(), reset.clone()));
            self.step(clock);
            self.component_event_override = None;
        }
        self.pending_assertion_edge = None;
    }

    /// Event whose component hooks `event` fires; see `step_in_reset`.
    fn component_event<'a>(&'a self, event: &'a Event) -> &'a Event {
        match &self.component_event_override {
            Some((from, to)) if from == event => to,
            _ => event,
        }
    }

    pub fn step(&mut self, event: &Event) {
        // A missing init_components call would let the run pass vacuously
        // (no hook ever fires); catch that bug in debug builds without
        // paying an assert on every step.
        debug_assert!(
            !self.components_pending,
            "simulator has user-defined components but init_components was not called"
        );

        #[cfg(feature = "profile")]
        {
            self.profile.step_count += 1;
        }

        // Common case (no derived clocks) skips the edge-detect loop.
        if self.ir.derived_clock_schedule.is_empty() {
            self.step_legacy(event);
        } else {
            self.step_with_derived_clocks(event);
        }
    }

    fn step_legacy(&mut self, event: &Event) {
        // Install before settle_comb so comb-scope FF writes
        // (`--disable-ff-opt` path) hit a live log.
        // SAFETY: buffer outlives every dispatch_stmt_fast call below
        // and is cleared before this frame returns.
        unsafe {
            set_event_write_log(&mut self.ir.write_log_buffer);
        }

        if self.comb_dirty {
            #[cfg(feature = "profile")]
            let start = Instant::now();

            self.do_settle_comb();
            self.comb_dirty = false;
            self.filter_note_settle();

            #[cfg(feature = "profile")]
            {
                self.profile.settle_comb_ns += start.elapsed().as_nanos() as u64;
            }
        } else if self.settle_filter.is_some() {
            self.settles_skipped += 1;
            self.filter_miss_streak = 0;
            self.check_skipped_settle();
        }

        self.step_event_inner(event);

        clear_event_write_log();
        // With the filter, the commit compare / event comb-write flag have
        // already dirtied the comb when needed.
        if self.settle_filter.is_none() {
            self.comb_dirty = true;
        }

        if !self.watch_vars.is_empty() {
            let tag = match event {
                Event::Clock(_) => "clk",
                Event::Reset(_) => "rst",
                _ => "evt",
            };
            self.dump_watch_changes(tag);
        }

        self.dump_variables();
    }

    /// Fire `event_statements[event]` then `ff_commit_from_log`.  The
    /// caller is responsible for `set_event_write_log`, `settle_comb`,
    /// and `dump_variables`.
    fn step_event_inner(&mut self, event: &Event) {
        let has_components = !self.components.is_empty();
        if has_components {
            self.stage_components(event);
        }
        self.eval_event_stmts(event);
        // The async-reset assertion edge, if this step carries one.
        if let Some(reset) = self.pending_assertion_edge.take() {
            self.eval_event_stmts(&reset);
        }
        self.commit_event_log();
        if has_components {
            self.fire_components(event);
        }
    }

    /// Loads and creates every user-defined component in the IR, then runs
    /// `on_init` so initial output values are visible from the first
    /// settle. A returned error is a test failure.
    pub fn init_components(
        &mut self,
        seed_base: u64,
        test_name: &str,
    ) -> Result<(), ComponentError> {
        self.components_pending = false;
        if self.ir.external_components.is_empty() {
            return Ok(());
        }
        let mut components = build_components(&self.ir, seed_base, test_name)?;
        for c in &mut components {
            c.on_init();
            c.drain_logs();
            c.apply_outputs(&mut self.ir.module_variables, self.ir.use_4state);
        }
        self.comb_dirty = true;
        for c in &components {
            if c.host.failed() {
                let mut msgs = vec![];
                for c in &mut components {
                    msgs.extend(c.host.take_failures());
                }
                return Err(ComponentError::InitFailed {
                    messages: msgs.join("\n"),
                });
            }
        }
        self.components = components;
        Ok(())
    }

    /// Stages pre-edge input values for every component listening to
    /// `event`. Must run before `commit_event_log`.
    fn stage_components(&mut self, event: &Event) {
        if self.components.is_empty() {
            return;
        }
        let event = self.component_event(event).clone();
        let mut components = std::mem::take(&mut self.components);
        for c in &mut components {
            if c.listens_to(&event) {
                c.stage_inputs(&mut self.mask_cache);
            }
        }
        self.components = components;
    }

    /// Fires component hooks for `event` and writes dirty outputs back.
    /// Must run after `commit_event_log` (the same edge's RTL then never
    /// observes component outputs — NBA semantics).
    fn fire_components(&mut self, event: &Event) {
        if self.components.is_empty() {
            return;
        }
        let event = self.component_event(event).clone();
        let event = &event;
        let mut components = std::mem::take(&mut self.components);
        let mut wrote = false;
        for c in &mut components {
            if c.listens_to(event) {
                c.fire(event, self.time);
                c.drain_logs();
                wrote |= c.apply_outputs(&mut self.ir.module_variables, self.ir.use_4state);
            }
        }
        self.components = components;
        if wrote {
            self.comb_dirty = true;
        }
    }

    pub fn component_finish_requested(&self) -> bool {
        self.components.iter().any(|c| c.host.finish_requested())
    }

    pub fn components_failed(&self) -> bool {
        self.components.iter().any(|c| c.host.failed())
    }

    pub fn take_component_failures(&mut self) -> Vec<String> {
        let mut msgs = vec![];
        for c in &mut self.components {
            msgs.extend(c.host.take_failures());
        }
        msgs
    }

    /// Zero-time method call on a component instance. An `Err` carries the
    /// failure messages the component reported (the test fails
    /// immediately).
    pub fn call_component_method(
        &mut self,
        inst: veryl_parser::resource_table::StrId,
        method: veryl_parser::resource_table::StrId,
        args: &[crate::component::host::HostValue],
    ) -> Result<crate::component::host::HostValue, String> {
        let Some(idx) = self.components.iter().position(|c| c.name_id == inst) else {
            let name = veryl_parser::resource_table::get_str_value(inst).unwrap_or_default();
            return Err(format!("unknown component instance `{name}`"));
        };
        let method_name = veryl_parser::resource_table::get_str_value(method).unwrap_or_default();
        let mut components = std::mem::take(&mut self.components);
        let c = &mut components[idx];
        c.host.time = self.time;
        let result = c.instance.call_method(&mut c.host, &method_name, args);
        c.drain_logs();
        let failures = c.host.take_failures();
        self.components = components;
        match result {
            Some(value) if failures.is_empty() => Ok(value),
            // Both `ctx.fail` during the call and a component error stop
            // the test immediately.
            None if failures.is_empty() => Err(format!("component method `{method_name}` failed")),
            _ => Err(failures.join("\n")),
        }
    }

    /// Fires `on_finish` on every component (end-of-test checks may still
    /// fail the test).
    pub fn finish_components(&mut self) {
        let mut components = std::mem::take(&mut self.components);
        for c in &mut components {
            c.on_finish();
            c.drain_logs();
        }
        self.components = components;
    }

    /// Evaluate `event_statements[event]` into the write log without
    /// committing, so simultaneous events (master + gated clocks) share
    /// one pre-commit state and one commit.
    fn eval_event_stmts(&mut self, event: &Event) {
        #[cfg(feature = "profile")]
        let event_start = Instant::now();

        // Cache both the per-stmt list AND the whole-event AOT-C handle for
        // the current event, keyed on `last_event`.  `event_statements` and
        // `whole_events` are both immutable after `Ir` construction, so the
        // raw pointers stay valid; this turns the per-cycle `whole_events`
        // HashMap probe + `Arc` clone into a single predicate check that the
        // per-stmt cache already pays for.
        let (stmts_ptr, whole_event_ptr) = if self.last_event.as_ref() == Some(event) {
            (self.last_event_stmts, self.last_whole_event)
        } else {
            let ptr: *const Vec<Statement> = match self.ir.event_statements.get(event) {
                Some(v) => v as *const _,
                None => std::ptr::null(),
            };
            let wptr: Option<*const dyn CompiledWhole> =
                self.ir.whole_events.get(event).map(Arc::as_ptr);
            self.last_event = Some(event.clone());
            self.last_event_stmts = ptr;
            self.last_whole_event = wptr;
            // An event absent from the classification must fail CLOSED: a
            // future path firing an unclassified event gets a settle, not a
            // silent skip.
            self.last_event_writes_comb =
                !self.ir.event_comb_writes.contains_key(event) || self.dirty_events.contains(event);
            self.last_event_watch = self.event_comb_watch.get(event).copied();
            (ptr, wptr)
        };

        // Writes outside the FF write log (comb bytes, readmemh, tb-method
        // returns) bypass the commit compare, so the settle filter must
        // treat this fire as dirtying the comb.  Disarmed, every fire
        // dirties — that also keeps the watch snapshot below dark.
        if self.settle_filter.is_some() && (self.last_event_writes_comb || !self.filter_armed) {
            if self.settle_diag && !self.comb_dirty && self.last_event_writes_comb {
                self.dirty_from_event += 1;
            }
            self.comb_dirty = true;
        }

        // Snapshot the event's comb-reaching writes (`event_comb_watch`) so
        // an unchanged fire — a divided clock wave on its flat tick — stays
        // invisible to the settle.  Pointless once the comb is dirty.
        let watch: Option<(u32, u32)> = if !self.comb_dirty && self.settle_filter.is_some() {
            self.last_event_watch
        } else {
            None
        };
        if let Some((ws, wl)) = watch {
            let mut pos = 0usize;
            for &(off, len) in &self.watch_pool[ws as usize..(ws + wl) as usize] {
                let (off, len) = (off as usize, len as usize);
                self.comb_watch_scratch[pos..pos + len]
                    .copy_from_slice(&self.ir.comb_values[off..off + len]);
                pos += len;
            }
        }

        // Whole-event backend (today: AOT-C): if a backend committed to
        // a one-function compile for this event, invoke it in place of
        // the per-stmt Cranelift dispatch.  The function reads ff/comb
        // current values and pushes WriteLogEntries into the buffer
        // (3rd arg), exactly as the Cranelift event JIT does;
        // `ff_commit_from_log` below applies them.
        let dispatched = if let Some(wptr) = whole_event_ptr {
            // SAFETY: `wptr` = `Arc::as_ptr` of an `Arc` owned by
            // `self.ir.whole_events`, which is never mutated after `Ir`
            // construction, so the pointee outlives this call.  Same
            // invariant the `last_event_stmts` raw pointer relies on.
            let whole: &dyn CompiledWhole = unsafe { &*wptr };
            let ff_ptr = self.ir.ff_values.as_ptr();
            let comb_ptr = self.ir.comb_values.as_ptr() as *mut u8;
            let log_ptr = (&*self.ir.write_log_buffer) as *const _ as *mut u8;

            // VERYL_AOT_C_VALIDATE=1: dual-run paths and diff.  Default-off.
            let validate = self.ir.aot_c_validate;

            if !validate {
                match whole.try_dispatch(ff_ptr, comb_ptr, log_ptr) {
                    DispatchOutcome::Done => true,
                    DispatchOutcome::NotReady => {
                        // `false` degrades to the per-stmt path below
                        // (see `residency`).
                        if !self
                            .ir
                            .whole_event_fallback_recorded
                            .swap(true, Ordering::Relaxed)
                        {
                            residency::record_fallback("whole_event", &self.ir.name.to_string());
                        }
                        false
                    }
                }
            } else {
                // For validate, the wrapper compares the whole-event
                // dispatch against the per-stmt Cranelift path and panics
                // on divergence.  The whole-event backend only exists on
                // native (BackendRegistry stays empty on wasm), so this
                // branch is effectively native-only at runtime.  NotReady
                // returns false → normal per-stmt fallback below.
                self.validate_event_aot(whole, stmts_ptr)
            }
        } else {
            false
        };

        if !dispatched && !stmts_ptr.is_null() {
            // SAFETY: event_statements is never mutated after Ir construction.
            let statements: &Vec<Statement> = unsafe { &*stmts_ptr };
            for x in statements {
                dispatch_stmt_fast(x, &mut self.mask_cache);
            }
        }

        if let Some((ws, wl)) = watch {
            let mut pos = 0usize;
            for &(off, len) in &self.watch_pool[ws as usize..(ws + wl) as usize] {
                let (off, len) = (off as usize, len as usize);
                if self.comb_watch_scratch[pos..pos + len] != self.ir.comb_values[off..off + len] {
                    if self.settle_diag {
                        self.dirty_from_event += 1;
                    }
                    self.comb_dirty = true;
                    break;
                }
                pos += len;
            }
        }

        #[cfg(feature = "profile")]
        {
            self.profile.event_eval_ns += event_start.elapsed().as_nanos() as u64;
        }
    }

    /// Apply the accumulated write log to FF storage and reset the buffer.
    fn commit_event_log(&mut self) {
        #[cfg(feature = "profile")]
        let ff_start = Instant::now();

        match &self.settle_filter {
            // Value-compare the commit against the comb's reach: when no
            // byte the comb can read changes, the standing settled state
            // stays valid and `comb_dirty` stays false.  An already-dirty
            // comb skips the compare — the verdict cannot improve.
            Some(_) if self.ff_unreachable && !self.comb_dirty => {
                ff_commit_from_log(&mut self.ir.ff_values, &self.ir.write_log_buffer);
            }
            Some(spans) if self.filter_armed && !self.comb_dirty => {
                let diag_offsets = &mut self.dirty_commit_offsets;
                let record = self.settle_diag;
                if ff_commit_from_log_watched(
                    &mut self.ir.ff_values,
                    &self.ir.write_log_buffer,
                    &mut |off, len| {
                        let hit = spans.ff_change_may_reach_comb(off, len);
                        if hit && record && diag_offsets.len() < 16 {
                            diag_offsets.push((off, len));
                        }
                        hit
                    },
                ) {
                    if self.settle_diag {
                        self.dirty_from_commit += 1;
                    }
                    self.comb_dirty = true;
                }
            }
            _ => {
                ff_commit_from_log(&mut self.ir.ff_values, &self.ir.write_log_buffer);
                // Disarmed, the flag must still reach the next settle
                // decision (the event fire above covers eval'd paths, but
                // not commits without one, e.g. the reset batch).
                if self.settle_filter.is_some() && !self.filter_armed {
                    self.comb_dirty = true;
                }
            }
        }

        if self.write_log_diag.enabled {
            let n = self.ir.write_log_buffer.count();
            self.write_log_diag.total_cycles += 1;
            if n > 0 {
                self.write_log_diag.total_entries += n as u64;
                self.write_log_diag.cycles_with_entries += 1;
                if n > self.write_log_diag.max_entries_per_cycle {
                    self.write_log_diag.max_entries_per_cycle = n;
                }
            }
            self.write_log_diag.maybe_print();
        }
        self.ir.write_log_buffer.reset();

        #[cfg(feature = "profile")]
        {
            self.profile.ff_swap_ns += ff_start.elapsed().as_nanos() as u64;
        }
    }

    /// Toggles master 0→1, fires the event + chained derived-clock
    /// events, then restores master=0 so `prev_derived_clock_values`
    /// samples on a consistent baseline.
    fn step_with_derived_clocks(&mut self, event: &Event) {
        // SAFETY: same as `step_legacy`; one install covers settle_comb
        // plus every step_event_inner fire in this step.
        unsafe {
            set_event_write_log(&mut self.ir.write_log_buffer);
        }

        // Hoisted like `has_eval_chunk` so loops test a local bool.
        let watch_enabled = !self.watch_vars.is_empty();

        // Subsequent partial_settle only refreshes the dep subset, so
        // the rest of the design must already be settled.
        if self.comb_dirty {
            self.do_settle_comb();
            self.comb_dirty = false;
            self.filter_note_settle();
        } else if self.settle_filter.is_some() {
            self.settles_skipped += 1;
            self.filter_miss_streak = 0;
            self.check_skipped_settle();
        }
        if watch_enabled {
            self.dump_watch("after_settle");
        }

        let master_id_opt = match event {
            Event::Clock(id) | Event::Reset(id) => {
                let id = *id;
                let is_master = self
                    .ir
                    .derived_clock_schedule
                    .master_input_clocks
                    .contains(&id);
                if is_master { Some(id) } else { None }
            }
            _ => None,
        };

        let has_eval_chunk = !self.ir.derived_clock_eval_stmts.is_empty();

        // Master high → gated-clock exprs see the rising edge.
        if let Some(id) = master_id_opt {
            self.set_input_clock_bit(id, 1);
            if has_eval_chunk {
                self.ir.partial_settle(&mut self.mask_cache);
            }
        }

        // Two-phase firing.  A master-gated clock's edge IS the master
        // edge qualified by the pre-commit enable (ICG semantics), so
        // those clocks fire here with the master event, sharing its
        // pre-commit state and write-log commit; a same-cycle enable
        // change waits for the next edge (the post-commit loop skips
        // them).  FF-driven clocks fire post-commit instead, matching
        // SV's NBA-driven edge propagation.
        let n = self.ir.derived_clock_schedule.clocks.len();
        let mut fired_mask = std::mem::take(&mut self.fired_mask_scratch);
        fired_mask.fill(false);
        let mut pre_fire: SmallVec<[usize; 8]> = SmallVec::new();
        // Master-gated values WHILE THE MASTER IS HIGH.  A clock the master
        // inverts is low here and rises when the master falls, so that edge
        // needs this baseline -- `prev_derived_clock_values` is the previous
        // step's low phase, where an inversion already reads 1.
        let mut high_values = std::mem::take(&mut self.derived_clock_high);
        high_values.fill(0);
        if master_id_opt.is_some() {
            for (i, high) in high_values.iter_mut().enumerate() {
                let clk = &self.ir.derived_clock_schedule.clocks[i];
                if clk.current_offset.is_ff() || !clk.master_gated {
                    continue;
                }
                *high = self.read_derived_clock_bit(clk);
                if self.prev_derived_clock_values[i] == 0 && *high == 1 {
                    pre_fire.push(i);
                }
            }
        }

        self.stage_components(event);
        for &i in &pre_fire {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            self.stage_components(&Event::Clock(vid));
        }
        self.eval_event_stmts(event);
        for &i in &pre_fire {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            if watch_enabled {
                self.dump_watch(&format!("pre_fire[{i}]"));
            }
            self.eval_event_stmts(&Event::Clock(vid));
            fired_mask[i] = true;
        }
        // Rides the master event's commit so a domain whose clock is gated off
        // still takes its reset values.
        if let Some(reset) = self.pending_assertion_edge.take() {
            self.eval_event_stmts(&reset);
        }
        self.commit_event_log();
        self.fire_components(event);
        for &i in &pre_fire {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            self.fire_components(&Event::Clock(vid));
        }
        if watch_enabled {
            self.dump_watch("after_master_event");
        }

        // Detect remaining 0→1 edges (caused by this step's FF commits)
        // and chain-fire one at a time, re-evaluating after each fire so
        // NBA glitch suppression works (a transient edge cancelled by a
        // same-cycle FF write must not trigger).
        // Convergence: each clock fires at most once (`fired_mask`) and
        // `analyze_dependency` rejects comb cycles, so n+1 iterations
        // suffice; the debug_assert catches bookkeeping regressions.
        let mut new_values = std::mem::take(&mut self.new_values_scratch);
        new_values.fill(0);
        let n_rst = self.ir.derived_clock_schedule.resets.len();
        let max_iters = n + n_rst + 1;
        let mut iters = 0;
        loop {
            if has_eval_chunk {
                self.ir.partial_settle(&mut self.mask_cache);
            }
            for (i, v) in new_values.iter_mut().enumerate().take(n) {
                let clk = &self.ir.derived_clock_schedule.clocks[i];
                *v = self.read_derived_clock_bit(clk);
            }

            // Earliest unfired clock with a real 0→1 edge.  An edge on a
            // master-gated comb clock here is a committed enable change,
            // which must not pulse (see the pre-commit phase above).
            //
            // ALL of them, not the first: clocks that rose from the same
            // settled state are edges at one instant, so every `always_ff` on
            // them samples pre-edge values.  Firing one at a time commits the
            // first domain before the second reads it.  A clock that rises
            // only BECAUSE an earlier fire committed is a genuine chain and
            // waits for the next iteration.
            let mut batch: SmallVec<[usize; 8]> = SmallVec::new();
            for i in 0..n {
                if fired_mask[i] {
                    continue;
                }
                let clk = &self.ir.derived_clock_schedule.clocks[i];
                if !clk.current_offset.is_ff() && clk.master_gated {
                    continue;
                }
                if self.prev_derived_clock_values[i] == 0 && new_values[i] == 1 {
                    batch.push(i);
                }
            }

            // Resets that have just reached their asserted level, from the
            // same refreshed closure.  A reset asserts BECAUSE a commit put
            // it there, so it belongs in this loop and not once per step.
            let mut rst_batch: SmallVec<[usize; 4]> = SmallVec::new();
            for i in 0..n_rst {
                let rst = &self.ir.derived_clock_schedule.resets[i];
                if self.prev_derived_reset_asserted[i] == 0
                    && self.read_derived_reset_asserted(rst) == 1
                {
                    rst_batch.push(i);
                }
            }

            if batch.is_empty() && rst_batch.is_empty() {
                break;
            }

            // The partial settle only refreshed the clock closure and the
            // fired domains read arbitrary comb, so settle fully before
            // firing (paid only when an edge fires).
            self.settle_comb_if_stale();
            // Re-verify on the fully settled state: the partial closure can
            // show a transient that full settling cancels.  Those are not
            // marked fired — the next iteration re-reads a consistent 0, so
            // the loop still ends.
            batch.retain(|i| {
                let clk = &self.ir.derived_clock_schedule.clocks[*i];
                self.read_derived_clock_bit(clk) == 1
            });
            rst_batch.retain(|i| {
                let rst = &self.ir.derived_clock_schedule.resets[*i];
                self.read_derived_reset_asserted(rst) == 1
            });
            // Record what the SETTLED state says about every reset, before
            // firing: an assertion is then seen once, and a level that fell
            // back re-arms.  This is also what makes the loop terminate.
            self.snapshot_derived_reset_levels();
            if batch.is_empty() && rst_batch.is_empty() {
                continue;
            }

            iters += 1;
            debug_assert!(
                iters <= max_iters,
                "derived clock fixpoint exceeded n+n_rst+1 iterations (n={n}, n_rst={n_rst})",
            );

            // One event region for the whole batch: stage, evaluate every
            // domain against the pre-edge state, then commit once.
            let has_components = !self.components.is_empty();
            for &i in &batch {
                let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
                if has_components {
                    self.stage_components(&Event::Clock(vid));
                }
            }
            if has_components {
                for &i in &rst_batch {
                    let vid = self.ir.derived_clock_schedule.resets[i].var_id;
                    self.stage_components(&Event::Reset(vid));
                }
            }
            for &i in &batch {
                let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
                if watch_enabled {
                    self.dump_watch(&format!("before_derived[{i}]"));
                }
                self.eval_event_stmts(&Event::Clock(vid));
            }
            // Resets last, so a net that both clocks and resets this
            // instant takes the reset value SV gives it.
            for &i in &rst_batch {
                let vid = self.ir.derived_clock_schedule.resets[i].var_id;
                self.eval_event_stmts(&Event::Reset(vid));
            }
            // The async-reset assertion edge, if this step carries one.
            if let Some(reset) = self.pending_assertion_edge.take() {
                self.eval_event_stmts(&reset);
            }
            self.commit_event_log();
            for &i in &rst_batch {
                let vid = self.ir.derived_clock_schedule.resets[i].var_id;
                if has_components {
                    self.fire_components(&Event::Reset(vid));
                }
            }
            for &i in &batch {
                let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
                if has_components {
                    self.fire_components(&Event::Clock(vid));
                }
                if watch_enabled {
                    self.dump_watch(&format!("after_derived[{i}]"));
                }
                fired_mask[i] = true;
            }
        }

        // master=0 + resettle so the prev snapshot matches the next
        // step's starting baseline.
        if let Some(id) = master_id_opt {
            self.set_input_clock_bit(id, 0);
            if has_eval_chunk {
                self.ir.partial_settle(&mut self.mask_cache);
            }
            // A clock the master inverts -- `~clk`, or a `clock_negedge`
            // whose active level `read_derived_clock_bit` inverts -- reaches
            // its active level HERE.  The pre-commit phase fires on the
            // master's rising edge, where an inversion falls, and the
            // post-commit loop skips master-gated clocks, so this is the only
            // place such a flop can fire.  It fires in place, not next step:
            // the fall is at `time + high_time`, inside this step, so a
            // testbench reading between two `clk.next(1)` calls is reading
            // after it.
            let mut fall: SmallVec<[usize; 8]> = SmallVec::new();
            for (i, high) in high_values.iter().enumerate() {
                let clk = &self.ir.derived_clock_schedule.clocks[i];
                if clk.current_offset.is_ff() || !clk.master_gated {
                    continue;
                }
                if *high == 0 && self.read_derived_clock_bit(clk) == 1 {
                    fall.push(i);
                }
            }
            if !fall.is_empty() {
                // The partial settle only refreshed the clock closure.
                self.settle_comb_if_stale();
                fall.retain(|i| {
                    let clk = &self.ir.derived_clock_schedule.clocks[*i];
                    self.read_derived_clock_bit(clk) == 1
                });
                self.fire_derived_clock_batch(&fall);
            }
        }

        for i in 0..n {
            let clk = &self.ir.derived_clock_schedule.clocks[i];
            self.prev_derived_clock_values[i] = self.read_derived_clock_bit(clk);
        }
        self.derived_clock_high = high_values;
        self.fired_mask_scratch = fired_mask;
        self.new_values_scratch = new_values;
        self.snapshot_derived_reset_levels();

        clear_event_write_log();
        // With the filter, the commit compare / event comb-write flag have
        // already dirtied the comb when needed.
        if self.settle_filter.is_none() {
            self.comb_dirty = true;
        }
        self.dump_variables();
    }

    /// VERYL_AOT_C_VALIDATE event-path check: run the AOT-C event function and
    /// the Cranelift per-stmt dispatch on identical inputs, compare the
    /// WriteLogEntries they push plus any direct ff/comb writes, and panic on
    /// first divergence.  Leaves the Cranelift result live (ground truth).
    /// Slow (clones ff/comb each event) — diagnostics only.  Unreachable on
    /// wasm since no whole-event backend ever registers there.
    ///
    /// Returns true when the AOT function ran (and was compared).  NotReady —
    /// the compile hasn't landed (or failed) — writes nothing, so comparing
    /// would diff an empty log against the real Cranelift effect and report a
    /// phantom divergence; mirror the non-validate path instead: return false
    /// and let the caller run the per-stmt Cranelift dispatch.
    ///
    /// ONLY reachable for an event that has a whole-event handle.  An event the
    /// emitter declined has none, so validate never sees it and never says so:
    /// a green VALIDATE run covers the events AOT-C already compiled, not the
    /// ones that most need checking.  Read `VERYL_BACKEND_DIAG` alongside it to
    /// know what was actually compared.
    fn validate_event_aot(
        &mut self,
        whole: &dyn CompiledWhole,
        stmts_ptr: *const Vec<Statement>,
    ) -> bool {
        // On off-stride cycles return false (skipping the AOT-C compare) so the
        // caller runs the per-stmt Cranelift dispatch — the ground truth.
        let stride = self.ir.aot_c_validate_stride;
        if stride > 1 {
            thread_local! {
                static EV_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
            }
            let sample = EV_COUNT.with(|c| {
                let v = c.get();
                c.set(v.wrapping_add(1));
                v % stride == 0
            });
            if !sample {
                return false;
            }
        }
        let ff_ptr = self.ir.ff_values.as_ptr();
        let comb_ptr = self.ir.comb_values.as_ptr() as *mut u8;
        let log_ptr = (&*self.ir.write_log_buffer) as *const _ as *mut u8;

        let ff_snap = self.ir.ff_values.to_vec();
        let comb_snap = self.ir.comb_values.to_vec();
        let count_before = self.ir.write_log_buffer.narrow_count as usize;
        let wide_count_before = self.ir.write_log_buffer.wide_count as usize;

        // Whole-event backend, then capture its pushed entries + ff/comb.
        if matches!(
            whole.try_dispatch(ff_ptr, comb_ptr, log_ptr),
            DispatchOutcome::NotReady,
        ) {
            return false;
        }
        // The committed FF effect is what `ff_commit_from_log` writes: all
        // narrow entries first (typed stores of `width_class` bytes), then all
        // wide entries (memcpy of `native_bytes`), last-write-wins per byte.
        // The SAME committed value can be routed through DIFFERENT pools by
        // different backends — a 65-128 bit FF is ONE wide entry for AOT-C /
        // interpret but TWO narrow u64 entries for the Cranelift JIT — so we
        // must compare the RESOLVED per-byte effect, not pool-specific entry
        // maps, or a byte-identical commit would false-positive.  (The dual-slot
        // "next slot" direct writes are vestigial; only the log drives commit.)
        let committed_bytes =
            |buf: &WriteLogBuffer, nlo: usize, nhi: usize, wlo: usize, whi: usize| {
                let mut m: HashMap<u32, u8> = Default::default();
                for e in &buf.narrow_entries_slice()[nlo..nhi] {
                    let nb = (e.width_class as usize).min(8);
                    let bytes = e.payload.to_le_bytes();
                    for (i, &b) in bytes.iter().take(nb).enumerate() {
                        m.insert(e.offset + i as u32, b);
                    }
                }
                for e in &buf.wide_entries_slice()[wlo..whi] {
                    let nb = (e.native_bytes as usize).min(e.payload.len());
                    for (i, &b) in e.payload.iter().take(nb).enumerate() {
                        m.insert(e.offset + i as u32, b);
                    }
                }
                m
            };
        let aot_count = self.ir.write_log_buffer.narrow_count as usize;
        let aot_wide_count = self.ir.write_log_buffer.wide_count as usize;
        let aot_bytes = committed_bytes(
            &self.ir.write_log_buffer,
            count_before,
            aot_count,
            wide_count_before,
            aot_wide_count,
        );
        // The is_ff refinement can demote an `always_ff` variable to comb, and
        // the event path then writes it directly rather than through the log —
        // invisible to the committed-FF compare below.
        let aot_comb = self.ir.comb_values.to_vec();

        // Restore inputs + log count, then run the Cranelift event.
        unsafe {
            std::ptr::copy_nonoverlapping(
                ff_snap.as_ptr(),
                self.ir.ff_values.as_ptr() as *mut u8,
                ff_snap.len(),
            );
            std::ptr::copy_nonoverlapping(
                comb_snap.as_ptr(),
                self.ir.comb_values.as_ptr() as *mut u8,
                comb_snap.len(),
            );
        }
        self.ir.write_log_buffer.narrow_count = count_before as u32;
        self.ir.write_log_buffer.wide_count = wide_count_before as u32;
        if !stmts_ptr.is_null() {
            let statements: &Vec<Statement> = unsafe { &*stmts_ptr };
            for x in statements {
                dispatch_stmt_fast(x, &mut self.mask_cache);
            }
        }
        let cr_count = self.ir.write_log_buffer.narrow_count as usize;
        let cr_wide_count = self.ir.write_log_buffer.wide_count as usize;
        let cr_bytes = committed_bytes(
            &self.ir.write_log_buffer,
            count_before,
            cr_count,
            wide_count_before,
            cr_wide_count,
        );

        let comb_diff = aot_comb
            .iter()
            .zip(self.ir.comb_values.iter())
            .filter(|(a, c)| a != c)
            .count();
        if comb_diff > 0 {
            eprintln!(
                "[aot_event_validate] DIVERGENCE module={} event={:?}: comb storage differs ({comb_diff} bytes)",
                self.ir.name, self.last_event,
            );
            let mut shown = 0;
            for (off, (a, c)) in aot_comb.iter().zip(self.ir.comb_values.iter()).enumerate() {
                if a != c {
                    eprintln!("  comb off={off:#x}: aot={a:#04x} cranelift={c:#04x}");
                    shown += 1;
                    if shown == 32 {
                        eprintln!("  ... (further differing bytes suppressed)");
                        break;
                    }
                }
            }
            panic!("AOT-C event validate divergence in comb storage (see above)");
        }

        // Backends may log different byte SETS for one committed effect: a
        // full-width select RMW (Cranelift) re-logs untouched bytes with
        // their old values, while a range-exact slice writer (AOT-C) logs
        // only the slice.  Commit overlays entries onto existing storage, so
        // compare the POST-COMMIT state rather than the logged sets.
        let eff = |m: &HashMap<u32, u8>, off: u32| {
            m.get(&off)
                .copied()
                .or_else(|| ff_snap.get(off as usize).copied())
        };
        let mut offs: BTreeSet<u32> = Default::default();
        offs.extend(aot_bytes.keys());
        offs.extend(cr_bytes.keys());
        if offs
            .iter()
            .any(|&off| eff(&aot_bytes, off) != eff(&cr_bytes, off))
        {
            eprintln!(
                "[aot_event_validate] DIVERGENCE module={} event={:?}: committed-FF bytes differ (aot {} bytes, cranelift {} bytes)",
                self.ir.name,
                self.last_event,
                aot_bytes.len(),
                cr_bytes.len(),
            );
            for off in offs {
                let a = eff(&aot_bytes, off);
                let c = eff(&cr_bytes, off);
                if a != c {
                    eprintln!("  byte off={off:#x}: aot={a:?} cranelift={c:?}");
                }
            }
            panic!("AOT-C event validate divergence (see above)");
        }
        true
    }

    /// Set a variable value by VarId. Used to write clock/reset signal values
    /// into the variable storage so they appear in wave dumps.
    pub fn set_var_by_id(&mut self, var_id: &VarId, val: Value) {
        if let Some(x) = self.ir.module_variables.variables.get_mut(var_id) {
            let mut val = val;
            val.trunc(x.width);
            unsafe {
                write_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    &val,
                );
            }
            self.comb_dirty = true;
        }
    }

    pub fn dump_start(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.begin_dumpvars();
            dump.dump_all_vars(&self.dump_vars, self.ir.use_4state);
            Self::dump_trace_vars(dump, &self.trace_dump_vars, &self.components);
            dump.end_dumpvars();
        }
    }

    pub fn dump_variables(&mut self) {
        if self.dump.is_some() {
            if self.comb_dirty {
                self.do_settle_comb();
                self.comb_dirty = false;
            }
            let dump = self.dump.as_mut().unwrap();
            dump.timestamp(self.time);
            dump.dump_all_vars(&self.dump_vars, self.ir.use_4state);
            Self::dump_trace_vars(dump, &self.trace_dump_vars, &self.components);
        }
    }

    fn dump_trace_vars(
        dump: &mut WaveDumper,
        trace_dump_vars: &[(crate::wave_dumper::VarHandle, usize, usize)],
        components: &[RuntimeComponent],
    ) {
        for &(handle, comp_idx, trace_idx) in trace_dump_vars {
            let var = &components[comp_idx].host.trace_vars[trace_idx];
            let mut value = crate::component::runtime::words_to_value(&var.words, var.width);
            // Excess high bits written by the component must not leak
            // into the waveform.
            value.trunc(var.width as usize);
            dump.change_vector(handle, &value);
        }
    }

    /// Sets up waveform dumping. Called via `Simulator::new` when no
    /// components are involved; the native-test flow calls `attach_dump`
    /// after `init_components` instead, so component trace variables
    /// (registered during `create`) make it into the header.
    fn setup_dump(&mut self, mut dumper: WaveDumper) {
        dumper.timescale();
        dumper.setup_module(&self.ir.module_variables, &mut self.dump_vars);
        for (comp_idx, comp) in self.components.iter().enumerate() {
            if comp.host.trace_vars.is_empty() {
                continue;
            }
            dumper.add_module(&comp.name);
            for (trace_idx, var) in comp.host.trace_vars.iter().enumerate() {
                let handle = dumper.add_wire(var.width, &var.name);
                self.trace_dump_vars.push((handle, comp_idx, trace_idx));
            }
            dumper.upscope();
        }
        dumper.finish_header();
        self.dump = Some(dumper);
    }

    /// See `setup_dump`; the public entry used after `init_components`.
    pub fn attach_dump(&mut self, dumper: WaveDumper) {
        self.setup_dump(dumper);
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        if self.settle_diag {
            eprintln!(
                "[settle_filter] module={} settles_run={} settles_skipped={} filter_on={} armed={} clock_toggle_dirties={} dirty_from_event={} dirty_from_commit={} first_commit_hits={:?}",
                self.ir.name,
                self.settles_run,
                self.settles_skipped,
                self.settle_filter.is_some(),
                self.filter_armed,
                self.clock_toggle_dirties,
                self.dirty_from_event,
                self.dirty_from_commit,
                self.dirty_commit_offsets,
            );
            let mut evs: Vec<String> = self.dirty_events.iter().map(|e| format!("{e:?}")).collect();
            evs.sort();
            eprintln!(
                "[settle_filter] module={} dirty_events={:?}",
                self.ir.name, evs
            );
        }
    }
}
