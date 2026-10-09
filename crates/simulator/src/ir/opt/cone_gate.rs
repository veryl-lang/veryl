//! Module-subtree comb cone scheduling (`VERYL_CONE_GATE`).
//!
//! A flat settle re-derives every comb statement each cycle, including whole
//! module subtrees whose inputs have not moved since the last one.  This pass
//! clusters each qualifying subtree's statements into a few contiguous
//! SEGMENTS of the schedule.  Each segment carries a compare set —
//! the bytes it reads that it does not itself produce, plus any of its
//! outputs an event can overwrite — and a shadow of their last-consumed
//! values.  At the segment's position in the settle, one memcmp decides:
//! unchanged means the fixpoint already in storage is still right and the
//! whole segment is skipped; changed means it runs and the shadow refreshes.
//!
//! Segments are gated INDEPENDENTLY (not per cone): outside statements
//! scheduled between two segments of the same cone can produce inputs of the
//! later segment, so only a compare at the segment's own position sees
//! exactly the values it would consume.  This is what keeps the check sound
//! against every workload — the decision is an exact byte compare, never a
//! prediction; the workload only decides how often it pays off.
//!
//! The unit follows the DESIGN's structure rather than the schedule's because
//! a module subtree's external interface is narrow (its ports plus its own FF
//! state) where a same-size window of the topological order reads much of the
//! design.  That is what buys one vectorised compare per segment: no
//! per-statement dirty bits, no consumer graphs, no propagation.
//!
//! Selection is STATIC — no activity profile exists at build time, and none
//! is needed: gating a segment is worth it iff its worst case (always dirty:
//! compare plus shadow refresh) is small against its own evaluation cost.
//! Both sides are compile-time quantities, so the statements-per-compare-byte
//! floor in `cone_order::select_segments` bounds the downside on ANY workload while leaving
//! the upside intact.  A segment that never skips also auto-offs (see
//! `AUTO_OFF_STREAK`), so the bet is bounded from both ends.
//!
//! The reorder and the segment selection live in `cone_order`, which a
//! dataflow settle uses on its own; this module adds the runtime gate.

use super::cone_order::{self, Schedule, StmtInfo};
use super::owners::{
    OwnerInputs, diag, first_span, intersect_ranges, merge_ranges, subtract_ranges,
};
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;

/// One gated contiguous statement range of the reordered schedule.
#[derive(Clone, Debug)]
pub struct Segment {
    /// `[start, end)` statement indices in the REORDERED comb list.
    pub start: usize,
    pub end: usize,
    /// Compare ranges `(is_ff, start, end)`, sorted, disjoint per buffer.
    pub compare: Vec<(bool, u32, u32)>,
    /// See `RtSegment::backedge` (comb ranges, pre-relayout).
    pub backedge: Vec<(u32, u32)>,
    /// See `RtSegment::replay` (comb ranges, pre-relayout).  Doubles as the
    /// segment's `compare_pre` set — the two are equal by construction, and
    /// the consumer re-derives `compare_pre` from the coalesced ranges so
    /// every replayed byte stays covered by the pre-run compare.
    pub replay: Vec<(u32, u32)>,
    pub bytes: usize,
    /// Skip-streak decrement applied on every successful skip (see
    /// `AUTO_OFF_STREAK`): the break-even skip rate of this segment's
    /// static bet, expressed as a leak.  A segment that skips more rarely
    /// than ~1/(decay+1) drifts to auto-off instead of resetting the
    /// streak on each stray skip.
    pub off_decay: u32,
    /// The events whose writes can reach a byte of `compare` or `replay`,
    /// directly or through the comb statements between (see
    /// `ConeGateState::mask_clean`).  All ones when no mask can describe the
    /// segment, empty when the analysis is off.
    pub trigger: Vec<u64>,
    /// Owning cone's hierarchical path (diagnostics).
    pub cone: String,
}

pub struct ConePlan {
    /// Permutation: `order[new_index] = old_index`.
    pub order: Vec<u32>,
    pub segments: Vec<Segment>,
    /// Shallowest first, so a group's `parent` precedes it.
    pub groups: Vec<Group>,
}

/// A subtree, or one window of it between outside writers of its inputs,
/// whose external inputs stand in for the compare sets of the segments below
/// it: while they hold, every member skips on the one check, because an
/// unchanged boundary keeps every wire inside a settled subtree unchanged too.
#[derive(Clone, Debug)]
pub struct Group {
    pub node: u32,
    /// Indices into `ConePlan::segments`, in schedule order.
    pub segments: Vec<u32>,
    /// Like `Segment::compare`: the window's external inputs (writes by
    /// subtree statements outside the window included), plus the members'
    /// replay spans (outputs an outside writer shares).
    pub compare: Vec<(bool, u32, u32)>,
    pub bytes: usize,
    /// See `Segment::off_decay`: one group compare buys the members' own
    /// compares, so the break-even skip rate follows their byte ratio.
    pub off_decay: u32,
    /// The nearest enclosing group that covers every member.
    pub parent: Option<u32>,
    pub cone: String,
}

/// `Group` in the FINAL storage space, over the `ConeSegment` list.
#[derive(Clone, Debug)]
pub struct ConeGroup {
    /// The subtree's node in `OwnerInputs`.
    pub node: u32,
    pub segments: Vec<u32>,
    pub compare: Vec<(bool, u32, u32)>,
    pub off_decay: u32,
    pub parent: Option<u32>,
    /// Byte offset of the group's gate state inside the comb buffer
    /// (`GROUP_STATE_HEADER_BYTES`, then the compare shadow); zero until the
    /// caller allocates the region.
    pub state_off: u32,
    pub cone: String,
}

/// Bytes a group's runtime state reserves ahead of its shadow: `[0]` primed,
/// `[1]` clean this settle, `[2]` auto-off, `[4..8)` the dirty streak.
pub(crate) const GROUP_STATE_HEADER_BYTES: usize = 8;

/// A gated segment resolved to whole JIT blocks (chunking was split at its
/// edges), with compare ranges in the FINAL (post-relayout) storage space.
#[derive(Clone, Debug)]
pub struct ConeSegment {
    /// `[lo, hi)` block indices into the pipeline's `ProtoStatements`.
    pub block_lo: usize,
    pub block_hi: usize,
    /// `[lo, hi)` statement indices into `pre_jit_stmts` (the whole-comb
    /// emitter's input order).
    pub stmt_lo: usize,
    pub stmt_hi: usize,
    /// Byte offset of this segment's gate state inside the comb buffer
    /// (flags u32 + streak u32, prerun, shadow, replay bytes; see the AOT-C
    /// emitter).  Zero until the caller allocates the region.
    pub state_off: u32,
    /// Compare ranges `(is_ff, start, end)`, final offsets.
    pub compare: Vec<(bool, u32, u32)>,
    /// See `RtSegment::backedge` (final offsets).
    pub backedge: Vec<(u32, u32)>,
    /// See `RtSegment::compare_pre` (final offsets).
    pub compare_pre: Vec<(u32, u32)>,
    /// See `RtSegment::replay` (final offsets).
    pub replay: Vec<(u32, u32)>,
    /// See `Segment::off_decay`.
    pub off_decay: u32,
    /// See `Segment::trigger`.
    pub trigger: Vec<u64>,
    /// See `RtSegment::internal`.
    pub internal: Vec<(u32, u32)>,
    /// Owning cone's hierarchical path (diagnostics).
    pub cone: String,
}

/// A gated segment in the FLAT instantiated `comb_statements` index space.
#[derive(Clone, Debug)]
pub struct RtSegment {
    pub lo: usize,
    pub hi: usize,
    pub compare: Vec<(bool, u32, u32)>,
    /// Comb byte ranges the segment both reads and writes (internal chain
    /// state).  A run that leaves them ALL unchanged proves the segment is at
    /// its fixpoint; only then may a later compare-clean settle skip it —
    /// otherwise the stored downstream values were computed from a MID-run
    /// mixture the post-run shadow cannot represent.
    pub backedge: Vec<(u32, u32)>,
    /// Backedge bytes that outside statements ALSO write (an extracted
    /// `x = 0` default re-zeroing an accumulator every settle): their
    /// check-time value is the segment's INPUT AS CONSUMED, so they compare
    /// against a shadow captured at run START, and they are excluded from
    /// the convergence check (their pre-vs-post churn is the outside
    /// transient the replay already reproduces, not internal iteration).
    pub compare_pre: Vec<(u32, u32)>,
    /// Comb spans written both by this segment and by statements outside it:
    /// a skip writes the stored post-run bytes back so last-writer-wins
    /// ordering inside the settle is preserved.
    pub replay: Vec<(u32, u32)>,
    /// See `Segment::off_decay`.
    pub off_decay: u32,
    /// See `Segment::trigger`.
    pub trigger: Vec<u64>,
    /// Compared bytes the segment rewrites before reading (its internal chain
    /// state): not inputs, so the trigger oracle leaves them out.
    pub internal: Vec<(u32, u32)>,
    pub cone: String,
}

/// After this many consecutive dirty checks a segment stops being checked:
/// on a workload where the subtree is genuinely active, even the bounded
/// compare cost is pure loss, and the streak proves it cannot pay here.
/// Bytes a guard's runtime state reserves ahead of its shadow/replay bytes:
/// `primed`, `converged`, `off`, `streak`.  The emitter offsets from it and
/// the allocator sizes with it, so it lives here rather than in either.
pub(crate) const GUARD_STATE_HEADER_BYTES: usize = 8;

pub(crate) const AUTO_OFF_STREAK: u32 = 1024;

/// Every this many settle passes, auto-offed segments get re-armed for one
/// more try.  Off is a per-phase verdict, not a per-workload one: a segment
/// trained during boot churn (a core spinning through reset toggles every
/// input) can skip 40-90% of evals in steady state, and without a retry the
/// boot verdict is permanent.  The epoch has to fall well inside a run: a
/// longer one may never fire, and the boot verdict stands after all.  A
/// re-armed hopeless segment re-expires after `AUTO_OFF_STREAK` dirty checks,
/// bounding the retry at `AUTO_OFF_STREAK / REARM_EVALS` of its evals.
pub(crate) const REARM_EVALS: u64 = 1 << 14;

/// Per-`Ir` runtime state: one shadow of the compare bytes per segment.
pub struct ConeGateState {
    shadows: Vec<Vec<u8>>,
    /// Shadows of `compare_pre` ranges, captured at run START.
    shadows_pre: Vec<Vec<u8>>,
    /// Stored post-run bytes of each segment's `replay` spans.
    replays: Vec<Vec<u8>>,
    primed: Vec<bool>,
    /// The segment's last run left every backedge byte unchanged — it is at
    /// its internal fixpoint, the precondition for any skip.
    converged: Vec<bool>,
    off: Vec<bool>,
    streak: Vec<u32>,
    /// Pre-run backedge snapshot scratch (reused across segments).
    prerun: Vec<u8>,
    pending: Vec<u64>,
    /// Settle passes seen, driving the `REARM_EVALS` retry of off segments.
    pub evals: u64,
    pub trigger_reports: u64,
    pub skipped: u64,
    pub ran: u64,
    pub next_report: u64,
    /// Per-segment (skipped, ran) tallies, kept for the DIAG report only.
    pub per_seg: Vec<(u64, u64)>,
}

impl ConeGateState {
    pub fn new(nseg: usize) -> Self {
        ConeGateState {
            shadows: vec![Vec::new(); nseg],
            shadows_pre: vec![Vec::new(); nseg],
            replays: vec![Vec::new(); nseg],
            primed: vec![false; nseg],
            converged: vec![false; nseg],
            off: vec![false; nseg],
            streak: vec![0; nseg],
            prerun: Vec::new(),
            pending: Vec::new(),
            evals: 0,
            trigger_reports: 0,
            skipped: 0,
            ran: 0,
            next_report: 1 << 18,
            per_seg: vec![(0, 0); nseg],
        }
    }

    /// Once per settle pass: every `REARM_EVALS` passes, give auto-offed
    /// segments a fresh unprimed start (see `REARM_EVALS`).
    /// `VERYL_CONE_GATE_REARM` overrides the interval (0 disables), matching
    /// the AOT-C entry preamble.
    #[inline]
    pub fn tick_rearm(&mut self) {
        static INTERVAL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let interval = *INTERVAL.get_or_init(|| {
            std::env::var("VERYL_CONE_GATE_REARM")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map_or(REARM_EVALS, |v| {
                    if v == 0 {
                        u64::MAX
                    } else {
                        v.next_power_of_two()
                    }
                })
        });
        self.evals += 1;
        if interval == u64::MAX || !self.evals.is_multiple_of(interval) {
            return;
        }
        for si in 0..self.off.len() {
            if self.off[si] {
                self.off[si] = false;
                self.primed[si] = false;
                self.converged[si] = false;
                self.streak[si] = 0;
            }
        }
    }

    /// True when the segment's inputs are byte-identical to its last run,
    /// AND that run was a no-op on its backedge bytes (fixpoint reached).
    /// Never true before the first run (priming) or after auto-off.
    #[inline]
    pub fn check_clean(&mut self, si: usize, seg: &RtSegment, ff: &[u8], comb: &[u8]) -> bool {
        if self.off[si] || !self.primed[si] || !self.converged[si] {
            return false;
        }
        let mut dirty = false;
        let mut pos = 0usize;
        for &(is_ff, s, e) in &seg.compare {
            let buf: &[u8] = if is_ff { ff } else { comb };
            let (s, e) = (s as usize, e as usize);
            if buf[s..e] != self.shadows[si][pos..pos + (e - s)] {
                dirty = true;
                break;
            }
            pos += e - s;
        }
        if !dirty {
            let mut pos = 0usize;
            for &(s, e) in &seg.compare_pre {
                let (s, e) = (s as usize, e as usize);
                if comb[s..e] != self.shadows_pre[si][pos..pos + (e - s)] {
                    dirty = true;
                    break;
                }
                pos += e - s;
            }
        }
        if dirty {
            self.streak[si] += 1;
            if self.streak[si] >= AUTO_OFF_STREAK {
                self.off[si] = true;
                self.shadows[si] = Vec::new();
                self.shadows_pre[si] = Vec::new();
            }
            return false;
        }
        self.streak[si] = self.streak[si].saturating_sub(seg.off_decay);
        self.skipped += 1;
        self.per_seg[si].0 += 1;
        true
    }

    /// The `compare_pre` spans still hold what the last run left there, which
    /// a replaying mask-only skip needs.  `false` when the shadows cannot
    /// answer (off, unprimed).
    pub fn pre_clean(&self, si: usize, seg: &RtSegment, comb: &[u8]) -> bool {
        if self.off[si] || !self.primed[si] {
            return false;
        }
        let mut pos = 0usize;
        for &(s, e) in &seg.compare_pre {
            let (s, e) = (s as usize, e as usize);
            let n = e - s;
            if self.shadows_pre[si].len() < pos + n
                || comb[s..e] != self.shadows_pre[si][pos..pos + n]
            {
                return false;
            }
            pos += n;
        }
        true
    }

    /// The first compared input byte that differs from the last run's shadow,
    /// for the trigger oracle.  `internal` and `compare_pre` are left out:
    /// the mask does not answer for them.
    pub fn first_moved(
        &self,
        si: usize,
        seg: &RtSegment,
        ff: &[u8],
        comb: &[u8],
    ) -> Option<(bool, usize)> {
        if self.off[si] || !self.primed[si] {
            return None;
        }
        let mut pos = 0usize;
        for &(is_ff, s, e) in &seg.compare {
            let buf: &[u8] = if is_ff { ff } else { comb };
            let (s, e) = (s as usize, e as usize);
            let internal = |x: usize| {
                !is_ff
                    && seg
                        .internal
                        .iter()
                        .any(|&(a, b)| (a as usize) <= x && x < (b as usize))
            };
            if let Some(k) = buf[s..e]
                .iter()
                .zip(&self.shadows[si][pos..pos + (e - s)])
                .enumerate()
                .position(|(k, (a, b))| a != b && !internal(s + k))
            {
                return Some((is_ff, s + k));
            }
            pos += e - s;
        }
        None
    }

    /// About to run the segment: snapshot its backedge bytes so `refresh`
    /// can decide whether the run changed them.
    #[inline]
    pub fn before_run(&mut self, si: usize, seg: &RtSegment, comb: &[u8]) {
        if self.off[si] {
            return;
        }
        self.prerun.clear();
        for &(s, e) in &seg.backedge {
            self.prerun.extend_from_slice(&comb[s as usize..e as usize]);
        }
        let sp = &mut self.shadows_pre[si];
        sp.clear();
        for &(s, e) in &seg.compare_pre {
            sp.extend_from_slice(&comb[s as usize..e as usize]);
        }
    }

    /// No event whose writes can reach the compare set has fired since the
    /// last settle, and the last run converged.  An off segment qualifies
    /// only without a backedge: with one, the convergence verdict stops being
    /// refreshed once it is off.  A mask with no bit set would ignore a
    /// widened pending set, so it never qualifies.
    #[inline]
    pub fn mask_clean(&self, si: usize, seg: &RtSegment) -> bool {
        self.primed[si]
            && self.converged[si]
            && (!self.off[si] || seg.backedge.is_empty())
            && seg.trigger.iter().any(|&m| m != 0)
            && seg
                .trigger
                .iter()
                .zip(&self.pending)
                .all(|(m, p)| m & p == 0)
    }

    #[inline]
    pub fn load_pending(&mut self, bytes: &[u8]) {
        self.pending.clear();
        self.pending.extend(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|&w| u64::from_le_bytes(w)),
        );
    }

    pub fn pending_bit(&self, bit: usize) -> bool {
        self.pending
            .get(bit / 64)
            .is_some_and(|w| w >> (bit % 64) & 1 == 1)
    }

    /// The segment has just run: derive the convergence verdict and snapshot
    /// the compare bytes.
    #[inline]
    pub fn refresh(&mut self, si: usize, seg: &RtSegment, ff: &[u8], comb: &[u8]) {
        self.ran += 1;
        self.per_seg[si].1 += 1;
        // Primed on every run: an off segment without replay spans can still
        // take the trigger skip (see `mask_clean`).
        self.primed[si] = true;
        if self.off[si] {
            return;
        }
        let mut pos = 0usize;
        let mut conv = true;
        for &(s, e) in &seg.backedge {
            if comb[s as usize..e as usize] != self.prerun[pos..pos + (e - s) as usize] {
                conv = false;
                break;
            }
            pos += (e - s) as usize;
        }
        self.converged[si] = conv;
        let shadow = &mut self.shadows[si];
        shadow.clear();
        for &(is_ff, s, e) in &seg.compare {
            let buf: &[u8] = if is_ff { ff } else { comb };
            shadow.extend_from_slice(&buf[s as usize..e as usize]);
        }
        let rp = &mut self.replays[si];
        rp.clear();
        for &(s, e) in &seg.replay {
            rp.extend_from_slice(&comb[s as usize..e as usize]);
        }
    }

    /// A skip is happening: re-establish the segment's stored output bytes
    /// for the spans an outside statement also writes, so last-writer-wins
    /// ordering within the settle is preserved (see `RtSegment::replay`).
    ///
    /// # Safety
    /// `comb` must point at the live comb buffer of at least the spans'
    /// extent; called only from the settle loop that owns it.
    #[inline]
    pub unsafe fn replay(&self, si: usize, seg: &RtSegment, comb: *mut u8) {
        let mut pos = 0usize;
        for &(s, e) in &seg.replay {
            let len = (e - s) as usize;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.replays[si].as_ptr().add(pos),
                    comb.add(s as usize),
                    len,
                );
            }
            pos += len;
        }
    }
}

/// A group's boundary must cost this many times less than the compares it
/// stands in for: the guard is paid on every settle, the members' compares
/// only where it finds them dirty.
const GROUP_MIN_RATIO: usize = 2;

/// Below this many gated segments a subtree is dropped rather than cut at its
/// outside writers, and groups are not ranked in the layout.  It only keeps
/// small designs out: segment count does not tell which designs the split
/// helps.
pub(crate) const SPLIT_MIN_SEGS: usize = 256;

/// Default-on; `VERYL_CONE_TRIGGER=0` leaves the guards to their compares.
pub fn trigger_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_CONE_TRIGGER").as_deref() != Ok("0"))
}

/// Default-on; `VERYL_CONE_GATE=0` opts out.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_CONE_GATE").as_deref() != Ok("0"))
}

/// One statement's eval cost expressed in compare bytes — the exchange rate
/// behind [`Segment::off_decay`]'s break-even skip rate (VERYL_CONE_GATE_COST
/// overrides it for calibration).  At 5, segments with bytes just above
/// 2.5×stmts get decay 0 — a decay-0 streak never drains, so any such segment
/// that runs 1024 times TOTAL auto-offs for good, even one skipping 80-90% of
/// evals once past its boot phase.  15 keeps those alive while segments that
/// genuinely cannot skip still expire.
fn stmt_cost_bytes() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("VERYL_CONE_GATE_COST")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(15)
    })
}

/// Build the plan: reorder `stmts` so each selected cone forms few contiguous
/// segments, and describe each gated segment's compare set.  Returns `None`
/// when nothing qualifies (caller keeps the original order).
pub fn plan(stmts: &[ProtoStatement], inputs: &OwnerInputs) -> Option<ConePlan> {
    let sched = cone_order::schedule(stmts, inputs)?;
    let (selected, total_bytes, budget) = cone_order::select_segments(&sched, inputs);
    let Schedule {
        order,
        infos,
        cones,
        writers,
        ..
    } = sched;
    let n = order.len();
    let mut seg_nodes: Vec<u32> = Vec::with_capacity(selected.len());
    let mut segments: Vec<Segment> = Vec::with_capacity(selected.len());
    for sel in selected {
        // Break-even skip rate of the static bet.  With +1 per dirty check
        // and -decay per skip, the streak drifts into auto-off exactly below
        // it; a free compare (bytes = 0) never expires.
        let off_decay = ((stmt_cost_bytes() * (sel.end - sel.start)).checked_div(sel.bytes))
            .map_or(u32::MAX, |r| r.saturating_sub(1).min(1 << 20) as u32);
        seg_nodes.push(sel.node);
        segments.push(Segment {
            start: sel.start,
            end: sel.end,
            compare: sel.compare,
            backedge: sel.backedge,
            replay: sel.replay,
            bytes: sel.bytes,
            off_decay,
            trigger: Vec::new(),
            cone: inputs.node_path[sel.node as usize].clone(),
        });
    }
    let groups = {
        let masks = trigger_masks(stmts, &order, &infos, inputs, &segments);
        for (seg, mask) in segments.iter_mut().zip(masks) {
            seg.trigger = mask;
        }
        build_groups(
            &segments,
            &seg_nodes,
            &infos,
            inputs,
            &writers,
            &order,
            segments.len() >= SPLIT_MIN_SEGS,
        )
    };
    if diag() {
        for g in &groups {
            eprintln!(
                "[cone_gate] group segs={} child_bytes={} bytes={} parent={:?} {}",
                g.segments.len(),
                g.segments
                    .iter()
                    .map(|&i| segments[i as usize].bytes)
                    .sum::<usize>(),
                g.bytes,
                g.parent,
                g.cone,
            );
        }
        eprintln!(
            "[cone_gate] stmts={} cones={} segments={} gated_stmts={} compare_bytes={}/{}",
            n,
            cones.len(),
            segments.len(),
            segments.iter().map(|s| s.end - s.start).sum::<usize>(),
            total_bytes,
            budget,
        );
        for s in &segments {
            eprintln!(
                "[cone_gate]   [{}..{}) stmts={} bytes={} {}",
                s.start,
                s.end,
                s.end - s.start,
                s.bytes,
                s.cone,
            );
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some(ConePlan {
        order,
        segments,
        groups,
    })
}

/// Schedule sweeps before the trigger analysis gives up on a fixpoint; each
/// sweep past the first carries a domain across one more back-edge.
const TRIGGER_MAX_SWEEPS: usize = 16;

fn spans_in(owner: &[(usize, usize, u32)], s: usize, e: usize) -> impl Iterator<Item = usize> + '_ {
    let first = first_span(owner, s);
    owner[first..]
        .iter()
        .enumerate()
        .take_while(move |(_, span)| span.0 < e)
        .filter(move |(_, span)| span.1 > s)
        .map(move |(k, _)| first + k)
}

/// `dst |= src`; true when `dst` grew.
fn or_into(dst: &mut [u64], src: &[u64]) -> bool {
    let mut grew = false;
    for (d, &x) in dst.iter_mut().zip(src) {
        let n = *d | x;
        grew |= n != *d;
        *d = n;
    }
    grew
}

/// Writes the trigger analysis does not model: a compiled block without its
/// originals coarsens a dynamic write to its ends, a method return lands by
/// variable id, a `$readmemh` names elements the offset gather misses, and an
/// FF write that is not `comb_direct` lands at the next commit, not in the
/// settle that runs it.
fn writes_untracked(s: &ProtoStatement) -> bool {
    use crate::ir::ProtoSystemFunctionCall as F;
    use crate::ir::statement::ProtoTbMethodKind as K;
    match s {
        ProtoStatement::Assign(a) => a.dst.is_ff() && !a.comb_direct,
        ProtoStatement::AssignDynamic(a) => a.dst_base.is_ff() && !a.comb_direct,
        ProtoStatement::Break => false,
        ProtoStatement::If(x) => x
            .true_side
            .iter()
            .chain(x.false_side.iter())
            .any(writes_untracked),
        ProtoStatement::Case(x) => x
            .arms
            .iter()
            .flat_map(|a| a.body.iter())
            .chain(x.default.iter())
            .any(writes_untracked),
        ProtoStatement::For(x) => x.body.iter().any(writes_untracked),
        ProtoStatement::SequentialBlock(b) => b.iter().any(writes_untracked),
        ProtoStatement::CompiledBlock(cb) => {
            cb.original_stmts.is_empty() || cb.original_stmts.iter().any(writes_untracked)
        }
        ProtoStatement::SystemFunctionCall(c) => matches!(c, F::Readmemh { .. }),
        ProtoStatement::TbMethodCall { method, .. } => matches!(
            method,
            K::Component { ret: Some(_), .. }
                | K::RandomGet { ret: Some(_), .. }
                | K::RandomGetRange { ret: Some(_), .. }
                | K::RandomGetSeed { ret: Some(_) }
        ),
        ProtoStatement::HierAssign(_) => {
            unreachable!("hierarchical assignment is resolved by resolve_hier_refs")
        }
    }
}

/// Per-segment trigger masks (see `Segment::trigger`).
///
/// Every owner span carries one domain: the events that can change its bytes,
/// seeded from the events' writes and the master toggles and carried through
/// the comb statements until nothing grows.  Whole-variable granularity is
/// coarse but safe.  A segment's mask is the union over its compare set,
/// `replay` included.
///
/// Empty masks (the analysis off) when there are no trigger bits, when a
/// statement writes storage the analysis does not model, when an event writes
/// an FF outside every run, or when the sweep does not settle.
fn trigger_masks(
    stmts: &[ProtoStatement],
    order: &[u32],
    infos: &[StmtInfo],
    inputs: &OwnerInputs,
    segments: &[Segment],
) -> Vec<Vec<u64>> {
    let nev = inputs.trigger_events.len();
    let off = || vec![Vec::new(); segments.len()];
    if nev == 0 || segments.is_empty() {
        return off();
    }
    if order
        .iter()
        .any(|&oi| writes_untracked(&stmts[oi as usize]))
    {
        if diag() {
            eprintln!(
                "[cone_gate] trigger: off, a comb statement writes storage the analysis does not model"
            );
        }
        return off();
    }
    // One bit past the events is the WILD bit: every mask includes it and
    // only a widened pending set carries it.
    let words = (nev + 1).div_ceil(64);
    let mut zero = vec![0u64; words];
    zero[nev / 64] |= 1u64 << (nev % 64);
    // An event whose writes could not be enumerated belongs in every mask.
    for (bit, ev) in inputs.event_writes.iter().enumerate() {
        if ev.wild {
            zero[bit / 64] |= 1u64 << (bit % 64);
        }
    }
    let all = vec![u64::MAX; words];
    let comb_span = |x: usize| -> Option<usize> {
        let i = inputs
            .comb_owner
            .partition_point(|&(s, _, _)| s <= x)
            .checked_sub(1)?;
        (x < inputs.comb_owner[i].1).then_some(i)
    };
    let ff_run = |x: usize| -> Option<usize> {
        inputs.ff_owner.run_index(x).or_else(|| {
            let i = inputs.ff_next_alias.partition_point(|&(n, _)| n < x);
            match inputs.ff_next_alias.get(i) {
                Some(&(n, cur)) if n == x => inputs.ff_owner.run_index(cur),
                _ => None,
            }
        })
    };
    let mut comb_dom: Vec<Vec<u64>> = vec![vec![0u64; words]; inputs.comb_owner.len()];
    let mut ff_dom: Vec<Vec<u64>> = vec![vec![0u64; words]; inputs.ff_owner.runs.len()];
    let seed = |o: VarOffset,
                bit: usize,
                comb_dom: &mut Vec<Vec<u64>>,
                ff_dom: &mut Vec<Vec<u64>>|
     -> bool {
        let raw = o.raw();
        if raw < 0 {
            return false;
        }
        if o.is_ff() {
            // FF storage is all variables; an offset outside every run is
            // one this table does not describe.
            match ff_run(raw as usize) {
                Some(r) => ff_dom[r][bit / 64] |= 1u64 << (bit % 64),
                None => return false,
            }
        } else if let Some(j) = comb_span(raw as usize) {
            comb_dom[j][bit / 64] |= 1u64 << (bit % 64);
        }
        // A comb offset no variable owns is read only by statements the
        // owner tables could not place either, and those run every settle.
        true
    };
    for (bit, ev) in inputs.event_writes.iter().enumerate() {
        if ev.wild {
            // Its bit is in every mask already.
            continue;
        }
        for &o in &ev.comb {
            if !seed(VarOffset::Comb(o), bit, &mut comb_dom, &mut ff_dom) {
                return off();
            }
        }
        for &o in &ev.ff {
            if !seed(VarOffset::Ff(o), bit, &mut comb_dom, &mut ff_dom) {
                return off();
            }
        }
    }
    for &(o, bit) in &inputs.master_clocks {
        if !seed(o, bit, &mut comb_dom, &mut ff_dom) {
            return off();
        }
    }
    let mut dom = vec![0u64; words];
    let mut settled = false;
    for _ in 0..TRIGGER_MAX_SWEEPS {
        let mut changed = false;
        for &oi in order {
            let info = &infos[oi as usize];
            if info.unknown_in {
                dom.copy_from_slice(&all);
            } else {
                dom.fill(0);
                for &(s, e) in &info.in_comb {
                    for j in spans_in(&inputs.comb_owner, s, e) {
                        or_into(&mut dom, &comb_dom[j]);
                    }
                }
                for &(s, e) in &info.in_ff {
                    for r in inputs.ff_owner.runs_in(s, e) {
                        or_into(&mut dom, &ff_dom[r]);
                    }
                }
            }
            for &(s, e) in &info.out_comb {
                for j in spans_in(&inputs.comb_owner, s, e) {
                    changed |= or_into(&mut comb_dom[j], &dom);
                }
            }
            for &(s, e) in &info.out_ff {
                for r in inputs.ff_owner.runs_in(s, e) {
                    changed |= or_into(&mut ff_dom[r], &dom);
                }
            }
        }
        if !changed {
            settled = true;
            break;
        }
    }
    if !settled {
        if diag() {
            eprintln!("[cone_gate] trigger: off, no fixpoint in {TRIGGER_MAX_SWEEPS} sweeps");
        }
        return off();
    }
    // Last schedule position writing each span.  A settle is one pass, so a
    // compared byte whose writer runs after the segment changes only at the
    // next settle, when the pending set that named the event is consumed: such
    // a segment gets all ones and keeps its compare.
    let mut comb_last: Vec<usize> = vec![0; inputs.comb_owner.len()];
    let mut ff_last: Vec<usize> = vec![0; inputs.ff_owner.runs.len()];
    for (pos, &oi) in order.iter().enumerate() {
        let info = &infos[oi as usize];
        for &(s, e) in &info.out_comb {
            for j in spans_in(&inputs.comb_owner, s, e) {
                comb_last[j] = comb_last[j].max(pos + 1);
            }
        }
        for &(s, e) in &info.out_ff {
            for r in inputs.ff_owner.runs_in(s, e) {
                ff_last[r] = ff_last[r].max(pos + 1);
            }
        }
    }
    let masks: Vec<Vec<u64>> = segments
        .iter()
        .map(|seg| {
            let mut m = zero.clone();
            let mut written_late = false;
            for &(is_ff, s, e) in &seg.compare {
                let (s, e) = (s as usize, e as usize);
                if is_ff {
                    for r in inputs.ff_owner.runs_in(s, e) {
                        or_into(&mut m, &ff_dom[r]);
                        written_late |= ff_last[r] > seg.end;
                    }
                } else {
                    for j in spans_in(&inputs.comb_owner, s, e) {
                        or_into(&mut m, &comb_dom[j]);
                        written_late |= comb_last[j] > seg.end;
                    }
                }
            }
            for &(s, e) in &seg.replay {
                for j in spans_in(&inputs.comb_owner, s as usize, e as usize) {
                    or_into(&mut m, &comb_dom[j]);
                }
            }
            if written_late {
                return all.clone();
            }
            m
        })
        .collect();
    masks
}

/// See `Group`.  `seg_nodes[i]` is the cone node of `segments[i]`, `writers`
/// the statements writing each owner span, `order` the schedule.  Every
/// ancestor of a segment's node is a candidate.  An outside writer of what the
/// subtree reads or replays, scheduled among its members, cuts it into windows
/// when `split` is set and drops it otherwise.
fn build_groups(
    segments: &[Segment],
    seg_nodes: &[u32],
    infos: &[StmtInfo],
    inputs: &OwnerInputs,
    writers: &[Vec<u32>],
    order: &[u32],
    split: bool,
) -> Vec<Group> {
    let mut pos = vec![0usize; infos.len()];
    for (k, &oi) in order.iter().enumerate() {
        pos[oi as usize] = k;
    }
    let parent = |m: u32| -> Option<u32> { inputs.parent(m) };
    let is_desc = |m: u32, a: u32| -> bool { inputs.tour().is_desc(m, a) };
    let depth = |m: u32| -> usize { inputs.tour().depth(m) as usize };
    let mut cands: Vec<u32> = Vec::new();
    for &sn in seg_nodes {
        let mut m = sn;
        loop {
            cands.push(m);
            match parent(m) {
                Some(p) => m = p,
                None => break,
            }
        }
    }
    cands.sort_unstable();
    cands.dedup();
    // Shallowest first: a group finds its enclosing group already built.
    cands.sort_by_key(|&m| depth(m));
    let mut groups: Vec<Group> = Vec::new();
    for a in cands {
        let all_members: Vec<u32> = (0..segments.len() as u32)
            .filter(|&i| is_desc(seg_nodes[i as usize], a))
            .collect();
        if all_members.len() < 2 {
            continue;
        }
        // In schedule order, so each window takes its own slice.
        let mut sub: Vec<(usize, u32)> = (0..infos.len() as u32)
            .filter(|&i| is_desc(infos[i as usize].node, a))
            .map(|i| (pos[i as usize], i))
            .collect();
        sub.sort_unstable();
        let span_lo = all_members
            .iter()
            .map(|&i| segments[i as usize].start)
            .min()
            .unwrap_or(0);
        let span_hi = all_members
            .iter()
            .map(|&i| segments[i as usize].end)
            .max()
            .unwrap_or(0);
        // A guard is evaluated once, ahead of its members, so it cannot answer
        // for an outside write that lands after it: not to an input, and not
        // to a replayed output either, which a clean verdict would restore
        // over the fresh value.
        let windows: Vec<(usize, usize)> = if split {
            let mut watched: Vec<(usize, usize)> = Vec::new();
            for &(_, i) in &sub {
                watched.extend_from_slice(&infos[i as usize].in_comb);
            }
            for &m in &all_members {
                watched.extend(
                    segments[m as usize]
                        .replay
                        .iter()
                        .map(|&(s, e)| (s as usize, e as usize)),
                );
            }
            merge_ranges(&mut watched);
            let mut cuts: Vec<usize> = Vec::new();
            let owner = &inputs.comb_owner;
            for &(s, e) in &watched {
                let mut j = first_span(owner, s);
                while j < owner.len() && owner[j].0 < e {
                    if owner[j].1 > s {
                        for &w in &writers[j] {
                            let p = pos[w as usize];
                            if (span_lo..span_hi).contains(&p)
                                && !is_desc(infos[w as usize].node, a)
                            {
                                cuts.push(p);
                            }
                        }
                    }
                    j += 1;
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            let mut w = Vec::with_capacity(cuts.len() + 1);
            let mut lo = span_lo;
            for &c in &cuts {
                if c > lo {
                    w.push((lo, c));
                }
                lo = c + 1;
            }
            if span_hi > lo {
                w.push((lo, span_hi));
            }
            w
        } else {
            vec![(0, usize::MAX)]
        };
        for (lo, hi) in windows {
            let members: Vec<u32> = all_members
                .iter()
                .copied()
                .filter(|&i| segments[i as usize].start >= lo && segments[i as usize].end <= hi)
                .collect();
            if members.len() < 2 {
                continue;
            }
            let child_bytes: usize = members.iter().map(|&i| segments[i as usize].bytes).sum();
            let first = members
                .iter()
                .map(|&i| segments[i as usize].start)
                .min()
                .unwrap_or(0);
            let last = members
                .iter()
                .map(|&i| segments[i as usize].end)
                .max()
                .unwrap_or(0);
            // A subtree statement outside this window is not covered by this
            // guard, so its writes do not answer for the window's reads.
            let inside = |w: u32| -> bool {
                let p = pos[w as usize];
                (lo..hi).contains(&p) && is_desc(infos[w as usize].node, a)
            };
            let s0 = sub.partition_point(|&(p, _)| p < lo);
            let s1 = sub.partition_point(|&(p, _)| p < hi);
            let mut in_comb: Vec<(usize, usize)> = Vec::new();
            let mut in_ff: Vec<(usize, usize)> = Vec::new();
            let mut out_comb: Vec<(usize, usize)> = Vec::new();
            for &(_, i) in &sub[s0..s1] {
                let info = &infos[i as usize];
                in_comb.extend_from_slice(&info.in_comb);
                in_ff.extend_from_slice(&info.in_ff);
                out_comb.extend_from_slice(&info.out_comb);
            }
            merge_ranges(&mut in_comb);
            merge_ranges(&mut in_ff);
            merge_ranges(&mut out_comb);
            let mut comb = subtract_ranges(&in_comb, &out_comb);
            // The skip argument covers what the subtree computes itself.  A
            // byte it reads that a statement outside it (or an event) also
            // writes is an input even when a subtree statement writes it too,
            // and `in − out` hides it.
            let owner = &inputs.comb_owner;
            let mut late = false;
            for &(s, e) in &in_comb {
                let mut j = first_span(owner, s);
                while j < owner.len() && owner[j].0 < e {
                    let (os, oe, _) = owner[j];
                    if oe > s {
                        let mut any = false;
                        for &w in writers[j].iter().filter(|&&w| !inside(w)) {
                            any = true;
                            late |= (first..last).contains(&pos[w as usize]);
                        }
                        if any {
                            comb.push((os.max(s), oe.min(e)));
                        }
                    }
                    j += 1;
                }
            }
            for &m in &members {
                for &(s, e) in &segments[m as usize].replay {
                    let (s, e) = (s as usize, e as usize);
                    let mut j = first_span(owner, s);
                    while j < owner.len() && owner[j].0 < e {
                        if owner[j].1 > s {
                            late |= writers[j]
                                .iter()
                                .any(|&w| !inside(w) && (first..last).contains(&pos[w as usize]));
                        }
                        j += 1;
                    }
                }
            }
            // An outside write among the members lands after the check (see
            // the windows above); only an unsplit subtree still meets one.
            if late {
                continue;
            }
            comb.extend(intersect_ranges(&inputs.event_written_comb, &in_comb));
            // A member output an outside writer also writes is an input as
            // consumed (see `RtSegment::compare_pre`), which `in − out` hides.
            for &i in &members {
                comb.extend(
                    segments[i as usize]
                        .replay
                        .iter()
                        .map(|&(s, e)| (s as usize, e as usize)),
                );
            }
            merge_ranges(&mut comb);
            let compare: Vec<(bool, u32, u32)> = comb
                .iter()
                .map(|&(s, e)| (false, s as u32, e as u32))
                .chain(in_ff.iter().map(|&(s, e)| (true, s as u32, e as u32)))
                .collect();
            let bytes: usize = compare.iter().map(|&(_, s, e)| (e - s) as usize).sum();
            if bytes * GROUP_MIN_RATIO > child_bytes {
                if diag() && members.len() >= 4 {
                    eprintln!(
                        "[cone_gate] group-reject cost segs={} child_bytes={child_bytes} bytes={bytes} {}",
                        members.len(),
                        inputs.node_path[a as usize]
                    );
                }
                continue;
            }
            let off_decay = (child_bytes / bytes.max(1)).saturating_sub(1).min(1 << 20) as u32;
            // After a split, ancestry alone no longer means a group covers
            // every one of these members.
            let parent_group = groups
                .iter()
                .rposition(|g| {
                    is_desc(a, g.node)
                        && members.iter().all(|m| g.segments.binary_search(m).is_ok())
                })
                .map(|p| p as u32);
            // The same members under a tighter boundary: one group, the deeper.
            if let Some(pi) = parent_group
                && groups[pi as usize].segments == members
            {
                let pg = &mut groups[pi as usize];
                if bytes <= pg.bytes {
                    pg.node = a;
                    pg.compare = compare;
                    pg.bytes = bytes;
                    pg.off_decay = off_decay;
                    pg.cone = inputs.node_path[a as usize].clone();
                }
                continue;
            }
            groups.push(Group {
                node: a,
                segments: members,
                compare,
                bytes,
                off_decay,
                parent: parent_group,
                cone: inputs.node_path[a as usize].clone(),
            });
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Event;
    use crate::ir::opt::owners::{EventWrites, FfOwner};
    use veryl_analyzer::ir::VarId;

    #[test]
    fn groups_cover_ancestors_of_several_segments_at_their_boundary() {
        // root 0 { 1 { 2, 3 }, 4 }: x → [2] → y → [3] → z → [4] → w.
        let inputs = OwnerInputs {
            node_parent: vec![u32::MAX, 0, 1, 1, 0],
            node_path: ["top", "top.a", "top.a.p", "top.a.q", "top.b"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            comb_owner: vec![(0, 8, 0), (8, 16, 2), (16, 24, 3), (24, 32, 4)],
            ff_owner: FfOwner { runs: vec![] },
            comb_var: Vec::new(),
            ff_node: Vec::new(),
            event_written_comb: vec![],
            trigger_events: Vec::new(),
            event_writes: Vec::new(),
            master_clocks: Vec::new(),
            ff_next_alias: Vec::new(),
            tour: Default::default(),
        };
        let info = |node: u32, ins: &[(usize, usize)], outs: &[(usize, usize)]| StmtInfo {
            node,
            in_comb: ins.to_vec(),
            in_ff: vec![],
            out_comb: outs.to_vec(),
            out_ff: vec![],
            unbounded: false,
            unknown_in: false,
        };
        let infos = [
            info(2, &[(0, 8)], &[(8, 16)]),
            info(3, &[(8, 16)], &[(16, 24)]),
            info(4, &[(16, 24)], &[(24, 32)]),
        ];
        let seg = |i: usize, compare: (u32, u32), replay: Vec<(u32, u32)>| Segment {
            start: i,
            end: i + 1,
            compare: vec![(false, compare.0, compare.1)],
            backedge: vec![],
            replay,
            bytes: 30,
            off_decay: 0,
            trigger: Vec::new(),
            cone: String::new(),
        };
        let segments = [
            seg(0, (0, 8), vec![(40, 48)]),
            seg(1, (8, 16), vec![]),
            seg(2, (16, 24), vec![]),
        ];
        // Owner spans x, y, z, w: written by nobody, [2], [3], [4].
        let writers: Vec<Vec<u32>> = vec![vec![], vec![0], vec![1], vec![2]];
        let order: Vec<u32> = (0..3).collect();
        let groups = build_groups(
            &segments,
            &[2, 3, 4],
            &infos,
            &inputs,
            &writers,
            &order,
            false,
        );
        assert_eq!(groups.len(), 2, "{groups:?}");
        // The root covers everything at its single external input.
        assert_eq!(groups[0].segments, vec![0, 1, 2]);
        assert_eq!(groups[0].compare, vec![(false, 0, 8), (false, 40, 48)]);
        assert_eq!(groups[0].parent, None);
        // `a` covers [2] and [3]; y is internal, the replay span joins.
        assert_eq!(groups[1].node, 1);
        assert_eq!(groups[1].segments, vec![0, 1]);
        assert_eq!(groups[1].compare, vec![(false, 0, 8), (false, 40, 48)]);
        assert_eq!(groups[1].parent, Some(0));
        // Members identical to the enclosing group's collapse into one group
        // at the deeper node.
        let writers2: Vec<Vec<u32>> = vec![vec![], vec![0], vec![1], vec![]];
        let same = build_groups(
            &segments[..2],
            &[2, 3],
            &infos[..2],
            &inputs,
            &writers2,
            &order[..2],
            false,
        );
        assert_eq!(same.len(), 1, "{same:?}");
        assert_eq!((same[0].node, same[0].parent), (1, None));
        // y is written inside `a` but also by [4] outside it: an input of
        // `a` as consumed, so it stays in `a`'s boundary (not the root's,
        // where both writers are inside).
        let shared_writers: Vec<Vec<u32>> = vec![vec![], vec![0, 2], vec![1], vec![2]];
        let shared = build_groups(
            &segments,
            &[2, 3, 4],
            &infos,
            &inputs,
            &shared_writers,
            &order,
            false,
        );
        assert_eq!(shared.len(), 2, "{shared:?}");
        assert_eq!(shared[0].compare, vec![(false, 0, 8), (false, 40, 48)]);
        assert_eq!(shared[1].compare, vec![(false, 0, 16), (false, 40, 48)]);
        // The same, with [4] scheduled between `a`'s members: y is then
        // written after `a`'s check, so `a` is not a group.
        let mid: Vec<u32> = vec![0, 2, 1];
        let mid_segments = [
            seg(0, (0, 8), vec![(40, 48)]),
            seg(2, (8, 16), vec![]),
            seg(1, (16, 24), vec![]),
        ];
        let late = build_groups(
            &mid_segments,
            &[2, 3, 4],
            &infos,
            &inputs,
            &shared_writers,
            &mid,
            false,
        );
        assert_eq!(late.len(), 1, "{late:?}");
        assert_eq!(late[0].node, 0);
        // A boundary as wide as the members' own compares is not worth it.
        let wide = [
            seg(
                0,
                (0, 8),
                (0..6).map(|k| (100 + 8 * k, 108 + 8 * k)).collect(),
            ),
            seg(1, (8, 16), vec![]),
        ];
        assert!(
            build_groups(
                &wide,
                &[2, 3],
                &infos[..2],
                &inputs,
                &writers2,
                &order[..2],
                false
            )
            .is_empty()
        );
    }

    fn group_inputs(node_parent: Vec<u32>, comb_owner: Vec<(usize, usize, u32)>) -> OwnerInputs {
        OwnerInputs {
            node_path: (0..node_parent.len()).map(|n| format!("n{n}")).collect(),
            node_parent,
            comb_owner,
            ff_owner: FfOwner { runs: vec![] },
            comb_var: Vec::new(),
            ff_node: Vec::new(),
            event_written_comb: vec![],
            trigger_events: Vec::new(),
            event_writes: Vec::new(),
            master_clocks: Vec::new(),
            ff_next_alias: Vec::new(),
            tour: Default::default(),
        }
    }

    fn group_info(node: u32, ins: &[(usize, usize)], outs: &[(usize, usize)]) -> StmtInfo {
        StmtInfo {
            node,
            in_comb: ins.to_vec(),
            in_ff: vec![],
            out_comb: outs.to_vec(),
            out_ff: vec![],
            unbounded: false,
            unknown_in: false,
        }
    }

    fn group_seg(pos: usize, replay: Vec<(u32, u32)>) -> Segment {
        Segment {
            start: pos,
            end: pos + 1,
            compare: vec![],
            backedge: vec![],
            replay,
            bytes: 30,
            off_decay: 0,
            trigger: Vec::new(),
            cone: String::new(),
        }
    }

    #[test]
    fn a_split_subtree_gets_a_group_on_each_side_of_an_outside_writer() {
        // root 0 { 1 { 2, 3, 4, 5 }, 6 }, scheduled p q C r s: C (node 6)
        // writes v3, which r reads.
        let inputs = group_inputs(
            vec![u32::MAX, 0, 1, 1, 1, 1, 0],
            vec![
                (0, 8, 0),
                (8, 16, 2),
                (16, 24, 3),
                (24, 32, 6),
                (32, 40, 4),
                (40, 48, 5),
            ],
        );
        let infos = [
            group_info(2, &[(0, 8)], &[(8, 16)]),
            group_info(3, &[(8, 16)], &[(16, 24)]),
            group_info(6, &[(0, 8)], &[(24, 32)]),
            group_info(4, &[(16, 32)], &[(32, 40)]),
            group_info(5, &[(32, 40)], &[(40, 48)]),
        ];
        let segments = [
            group_seg(0, vec![]),
            group_seg(1, vec![]),
            group_seg(3, vec![]),
            group_seg(4, vec![]),
        ];
        let writers: Vec<Vec<u32>> = vec![vec![], vec![0], vec![1], vec![2], vec![3], vec![4]];
        let order: Vec<u32> = (0..5).collect();
        let build = |split| {
            build_groups(
                &segments,
                &[2, 3, 4, 5],
                &infos,
                &inputs,
                &writers,
                &order,
                split,
            )
        };
        let whole = build(false);
        assert_eq!(whole.len(), 1, "{whole:?}");
        assert_eq!(whole[0].node, 0);
        let split = build(true);
        assert_eq!(split.len(), 3, "{split:?}");
        assert_eq!(
            (split[1].node, &split[1].segments, split[1].parent),
            (1, &vec![0, 1], Some(0))
        );
        assert_eq!(
            (split[2].node, &split[2].segments, split[2].parent),
            (1, &vec![2, 3], Some(0))
        );
        // q's output is an input of the second window: q runs outside it.
        assert_eq!(split[2].compare, vec![(false, 16, 32)]);
    }

    #[test]
    fn an_outside_writer_of_a_replayed_output_among_the_members_blocks_the_group() {
        // root 0 { 1 { 2, 3 }, 6 }, scheduled p C q: C writes v9, which q also
        // writes and replays.  A clean verdict on `a` would restore v9 over
        // C's value.
        let inputs = group_inputs(
            vec![u32::MAX, 0, 1, 1, 0, 0, 0],
            vec![(0, 8, 0), (8, 16, 2), (16, 24, 3), (48, 56, 3)],
        );
        let infos = [
            group_info(2, &[(0, 8)], &[(8, 16)]),
            group_info(6, &[(0, 8)], &[(48, 56)]),
            group_info(3, &[(8, 16)], &[(16, 24), (48, 56)]),
        ];
        let segments = [group_seg(0, vec![]), group_seg(2, vec![(48, 56)])];
        let writers: Vec<Vec<u32>> = vec![vec![], vec![0], vec![2], vec![1, 2]];
        let order: Vec<u32> = (0..3).collect();
        for split in [false, true] {
            let groups = build_groups(&segments, &[2, 3], &infos, &inputs, &writers, &order, split);
            assert!(
                groups.iter().all(|g| g.node != 1),
                "split={split}: {groups:?}"
            );
        }
    }

    #[test]
    fn an_unsplit_group_counts_every_subtree_statement_as_inside() {
        // root 0 { 1 { 2, 3 } }: an ungated node-2 statement x -> y runs
        // before the segments y -> z and z -> w.
        let inputs = group_inputs(
            vec![u32::MAX, 0, 1, 1],
            vec![(0, 8, 0), (8, 16, 2), (16, 24, 3), (24, 32, 3)],
        );
        let infos = [
            group_info(2, &[(0, 8)], &[(8, 16)]),
            group_info(2, &[(8, 16)], &[(16, 24)]),
            group_info(3, &[(16, 24)], &[(24, 32)]),
        ];
        let segments = [group_seg(1, vec![]), group_seg(2, vec![])];
        let writers: Vec<Vec<u32>> = vec![vec![], vec![0], vec![1], vec![2]];
        let order: Vec<u32> = (0..3).collect();
        let groups = build_groups(&segments, &[2, 3], &infos, &inputs, &writers, &order, false);
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].compare, vec![(false, 0, 8)]);
    }

    #[test]
    fn rearm_retries_offed_segments_each_epoch() {
        let mut st = ConeGateState::new(2);
        st.off[0] = true;
        st.primed[0] = true;
        st.converged[0] = true;
        st.streak[0] = AUTO_OFF_STREAK;
        st.streak[1] = 7;
        for _ in 0..REARM_EVALS - 1 {
            st.tick_rearm();
        }
        assert!(st.off[0], "re-arm fired before the epoch boundary");
        st.tick_rearm();
        assert!(!st.off[0] && !st.primed[0] && !st.converged[0]);
        assert_eq!(st.streak[0], 0);
        // A live segment's streak is untouched by the epoch tick.
        assert_eq!(st.streak[1], 7);
    }

    /// Owner spans a, b, c, d (comb) and one FF run e.  Events: E0 writes e,
    /// E1 writes a, E2 is wild.  Schedule: [0] e → b, [1] a → c, [2] b, c → d.
    fn trigger_fixture() -> (OwnerInputs, Vec<StmtInfo>, Vec<u32>) {
        let inputs = OwnerInputs {
            node_parent: vec![u32::MAX],
            node_path: vec!["top".to_string()],
            comb_owner: vec![(0, 8, 0), (8, 16, 0), (16, 24, 0), (24, 32, 0)],
            ff_owner: FfOwner {
                runs: vec![(0, 4, 1, 4)],
            },
            comb_var: Vec::new(),
            ff_node: Vec::new(),
            event_written_comb: vec![(0, 8)],
            trigger_events: vec![
                Event::clock(VarId::from_raw(1)),
                Event::clock(VarId::from_raw(2)),
                Event::InitialBlock(1),
            ],
            event_writes: vec![
                EventWrites {
                    comb: vec![],
                    ff: vec![0],
                    wild: false,
                },
                EventWrites {
                    comb: vec![0],
                    ff: vec![],
                    wild: false,
                },
                EventWrites {
                    wild: true,
                    ..Default::default()
                },
            ],
            master_clocks: Vec::new(),
            ff_next_alias: Vec::new(),
            tour: Default::default(),
        };
        let info =
            |in_comb: &[(usize, usize)], in_ff: &[(usize, usize)], out: (usize, usize)| StmtInfo {
                node: 0,
                in_comb: in_comb.to_vec(),
                in_ff: in_ff.to_vec(),
                out_comb: vec![out],
                out_ff: vec![],
                unbounded: false,
                unknown_in: false,
            };
        let infos = vec![
            info(&[], &[(0, 4)], (8, 16)),
            info(&[(0, 8)], &[], (16, 24)),
            info(&[(8, 16), (16, 24)], &[], (24, 32)),
        ];
        (inputs, infos, vec![0, 1, 2])
    }

    /// A segment scheduled after every writer in the fixture.
    fn trigger_seg(compare: &[(bool, u32, u32)], replay: &[(u32, u32)]) -> Segment {
        Segment {
            start: 3,
            end: 3,
            compare: compare.to_vec(),
            backedge: vec![],
            replay: replay.to_vec(),
            bytes: 8,
            off_decay: 0,
            trigger: Vec::new(),
            cone: String::new(),
        }
    }

    #[test]
    fn trigger_masks_carry_event_writes_along_the_schedule() {
        let (inputs, infos, order) = trigger_fixture();
        let stmts = vec![ProtoStatement::Break; 3];
        let segments = [
            trigger_seg(&[(true, 0, 4)], &[]),                    // reads e
            trigger_seg(&[(false, 0, 8)], &[]),                   // reads a
            trigger_seg(&[(false, 8, 16), (false, 16, 24)], &[]), // reads b, c
            trigger_seg(&[], &[(24, 32)]),                        // replays d
            trigger_seg(&[(false, 40, 48)], &[]),                 // reads storage nobody writes
        ];
        let masks = trigger_masks(&stmts, &order, &infos, &inputs, &segments);
        const WILD: u64 = 0b1000;
        // E2's writes could not be enumerated, so every mask carries its bit:
        // the mask is the only account of it both gate paths share.
        const E2: u64 = 0b100;
        assert_eq!(masks[0], vec![WILD | E2 | 0b001]);
        assert_eq!(masks[1], vec![WILD | E2 | 0b010]);
        assert_eq!(masks[2], vec![WILD | E2 | 0b011]);
        assert_eq!(
            masks[3],
            vec![WILD | E2 | 0b011],
            "replay spans join the mask"
        );
        assert_eq!(
            masks[4],
            vec![WILD | E2],
            "nothing bounded reaches it: a widened mask or E2 wakes it"
        );
    }

    #[test]
    fn a_segment_before_a_writer_of_what_it_compares_keeps_its_compare() {
        let (inputs, infos, order) = trigger_fixture();
        let stmts = vec![ProtoStatement::Break; 3];
        // A settle is one pass: the segment placed before the writer of c
        // sees that write only at the next settle, when the pending set no
        // longer names the event that caused it.  Its mask cannot gate.
        let mut early = trigger_seg(&[(false, 16, 24)], &[]);
        early.start = 0;
        early.end = 1;
        let mut late_enough = trigger_seg(&[(false, 16, 24)], &[]);
        late_enough.start = 2;
        late_enough.end = 2;
        let masks = trigger_masks(&stmts, &order, &infos, &inputs, &[early, late_enough]);
        assert_eq!(masks[0], vec![u64::MAX], "placed before the writer of c");
        assert_eq!(
            masks[1],
            vec![0b1000 | 0b100 | 0b010],
            "placed after it: the precise mask stands"
        );
    }

    #[test]
    fn trigger_masks_treat_an_unplaced_read_as_reached_by_everything() {
        let (inputs, mut infos, order) = trigger_fixture();
        infos[0].unknown_in = true;
        let stmts = vec![ProtoStatement::Break; 3];
        let segments = [trigger_seg(&[(false, 8, 16)], &[])];
        let masks = trigger_masks(&stmts, &order, &infos, &inputs, &segments);
        assert_eq!(masks[0], vec![u64::MAX]);
    }

    #[test]
    fn trigger_masks_seed_a_master_clock_toggle() {
        let (mut inputs, infos, order) = trigger_fixture();
        // The master's level lives in a and reports E0, which no event
        // write reaches c through: only the seed puts it in c's mask.
        inputs.master_clocks = vec![(VarOffset::Comb(0), 0)];
        let stmts = vec![ProtoStatement::Break; 3];
        let segments = [trigger_seg(&[(false, 16, 24)], &[])];
        let masks = trigger_masks(&stmts, &order, &infos, &inputs, &segments);
        assert_eq!(masks[0], vec![0b1000 | 0b100 | 0b010 | 0b001]);
    }

    /// A constant store to the 4-byte variable at `dst`.
    fn const_assign(dst: VarOffset, comb_direct: bool) -> ProtoStatement {
        use crate::ir::{ExpressionContext, ProtoAssignStatement, ProtoExpression};
        use veryl_analyzer::value::{Value, ValueU64};
        ProtoStatement::Assign(ProtoAssignStatement {
            dst,
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Value {
                value: Value::U64(ValueU64 {
                    payload: 1,
                    mask_xz: 0,
                    width: 32,
                    signed: false,
                }),
                width: 32,
                expr_context: ExpressionContext {
                    width: 32,
                    signed: false,
                },
            },
            dst_ff_current_offset: if dst.is_ff() { dst.raw() } else { -1 },
            comb_direct,
            token: Default::default(),
        })
    }

    #[test]
    fn trigger_masks_go_off_when_a_comb_statement_logs_an_ff_write() {
        // The logged write lands at the next commit, after whatever events
        // that step fires, which no mask can name.
        let (inputs, infos, order) = trigger_fixture();
        let segments = [trigger_seg(&[(true, 0, 4)], &[])];
        let mut stmts = vec![ProtoStatement::Break; 3];
        stmts[0] = const_assign(VarOffset::Ff(0), false);
        assert!(trigger_masks(&stmts, &order, &infos, &inputs, &segments)[0].is_empty());
        stmts[0] = ProtoStatement::SequentialBlock(vec![const_assign(VarOffset::Ff(0), false)]);
        assert!(trigger_masks(&stmts, &order, &infos, &inputs, &segments)[0].is_empty());
        // A comb write that only lands in FF storage stores in place.
        stmts[0] = const_assign(VarOffset::Ff(0), true);
        assert!(!trigger_masks(&stmts, &order, &infos, &inputs, &segments)[0].is_empty());
    }

    fn rt_seg(trigger: Vec<u64>, backedge: Vec<(u32, u32)>) -> RtSegment {
        RtSegment {
            lo: 0,
            hi: 1,
            compare: vec![(false, 0, 8)],
            backedge,
            compare_pre: vec![],
            replay: vec![],
            off_decay: 0,
            trigger,
            internal: vec![],
            cone: String::new(),
        }
    }

    #[test]
    fn a_mask_skip_needs_a_converged_last_run() {
        let mut st = ConeGateState::new(1);
        st.load_pending(&0u64.to_le_bytes());
        let seg = rt_seg(vec![0b1000], vec![]);
        st.primed[0] = true;
        assert!(!st.mask_clean(0, &seg), "the last run moved its backedge");
        st.converged[0] = true;
        assert!(st.mask_clean(0, &seg));
        st.load_pending(&0b1000u64.to_le_bytes());
        assert!(!st.mask_clean(0, &seg), "a pending bit in the mask");
    }

    #[test]
    fn an_off_segment_mask_skips_only_without_a_backedge() {
        // Off, the convergence verdict is no longer refreshed; without a
        // backedge it is true by construction.
        let mut st = ConeGateState::new(1);
        st.load_pending(&0u64.to_le_bytes());
        st.primed[0] = true;
        st.converged[0] = true;
        st.off[0] = true;
        assert!(st.mask_clean(0, &rt_seg(vec![0b1000], vec![])));
        assert!(!st.mask_clean(0, &rt_seg(vec![0b1000], vec![(8, 16)])));
    }

    #[test]
    fn an_all_zero_mask_never_skips() {
        // It would ignore even a widened pending set.
        let mut st = ConeGateState::new(1);
        st.primed[0] = true;
        st.converged[0] = true;
        let seg = rt_seg(vec![0], vec![]);
        st.load_pending(&0u64.to_le_bytes());
        assert!(!st.mask_clean(0, &seg));
        st.load_pending(&u64::MAX.to_le_bytes());
        assert!(!st.mask_clean(0, &seg));
    }

    #[test]
    fn trigger_masks_go_off_when_an_event_write_has_no_home() {
        let (mut inputs, infos, order) = trigger_fixture();
        inputs.event_writes[0].ff.push(100);
        let stmts = vec![ProtoStatement::Break; 3];
        let segments = [trigger_seg(&[(true, 0, 4)], &[])];
        let masks = trigger_masks(&stmts, &order, &infos, &inputs, &segments);
        assert!(masks[0].is_empty());
        let (inputs, infos, order) = trigger_fixture();
        let mut off = inputs;
        off.trigger_events.clear();
        off.event_writes.clear();
        assert!(trigger_masks(&stmts, &order, &infos, &off, &segments)[0].is_empty());
    }

    /// `dst = src`, one-byte comb variables.
    fn copy(dst: usize, src: usize) -> ProtoStatement {
        use crate::ir::{ExpressionContext, ProtoAssignStatement, ProtoExpression};
        let ctx = ExpressionContext {
            width: 8,
            signed: false,
        };
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(dst as isize),
            dst_width: 8,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Variable {
                var_offset: VarOffset::Comb(src as isize),
                select: None,
                dynamic_select: None,
                width: 8,
                var_full_width: 8,
                expr_context: ctx,
            },
            dst_ff_current_offset: -1,
            comb_direct: false,
            token: Default::default(),
        })
    }

    /// One copy chain of `len` statements per leaf under the root, the
    /// chains' statements `interleaved` or one chain after another.
    fn chain_design(
        leaves: usize,
        len: usize,
        interleaved: bool,
    ) -> (OwnerInputs, Vec<ProtoStatement>) {
        let var = |leaf: usize, i: usize| (leaf * (len + 1) + i) * 8;
        let mut node_parent = vec![u32::MAX];
        node_parent.extend(std::iter::repeat_n(0, leaves));
        let mut comb_owner = Vec::new();
        for leaf in 0..leaves {
            for i in 0..=len {
                comb_owner.push((var(leaf, i), var(leaf, i) + 1, leaf as u32 + 1));
            }
        }
        let inputs = group_inputs(node_parent, comb_owner);
        let stmt = |leaf: usize, i: usize| copy(var(leaf, i + 1), var(leaf, i));
        let stmts = if interleaved {
            (0..len)
                .flat_map(|i| (0..leaves).map(move |leaf| (leaf, i)))
                .map(|(leaf, i)| stmt(leaf, i))
                .collect()
        } else {
            (0..leaves)
                .flat_map(|leaf| (0..len).map(move |i| (leaf, i)))
                .map(|(leaf, i)| stmt(leaf, i))
                .collect()
        };
        (inputs, stmts)
    }

    #[test]
    fn cone_order_matches_the_gate_plan() {
        // The clustering re-sort, the in-place runs, and too little to gate.
        for (leaves, len, interleaved, gated) in [
            (3, 1_700, true, true),
            (3, 400, false, true),
            (2, 100, false, false),
        ] {
            let (inputs, stmts) = chain_design(leaves, len, interleaved);
            let plan = plan(&stmts, &inputs);
            let order = cone_order::cone_order(&stmts, &inputs);
            assert_eq!(plan.is_some(), gated);
            assert_eq!(
                plan.map(|p| (p.order, p.segments.len())),
                order.map(|o| (o.order, o.segments)),
                "leaves={leaves} len={len}"
            );
        }
        // The re-sort pulls each interleaved chain together.
        let (inputs, stmts) = chain_design(3, 1_700, true);
        let order = cone_order::cone_order(&stmts, &inputs).unwrap().order;
        assert!(order.windows(2).take(1_699).all(|w| w[1] == w[0] + 3));
    }
}
