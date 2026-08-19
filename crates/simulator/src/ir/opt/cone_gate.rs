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
//! floor in `finish_plan` bounds the downside on ANY workload while leaving
//! the upside intact.  A segment that never skips also auto-offs (see
//! `AUTO_OFF_STREAK`), so the bet is bounded from both ends.

use crate::HashMap;
use crate::ir::big_array::BigArrayFold;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::{
    ModuleVariableMeta, VarOffset, VariableElement, VariableMeta, value_size,
};
use veryl_analyzer::ir::VarId;

/// FF-buffer ownership, held per uniformly strided RUN of elements rather
/// than per element: memory arrays make the element count grow with the
/// modelled depth while the array count does not.
pub struct FfOwner {
    /// `(start, stride, count, bytes)`, sorted by `start` and disjoint.  The
    /// run covers `[start, start + count * stride)`, of which each element
    /// occupies the first `bytes`.
    runs: Vec<(usize, usize, usize, usize)>,
}

impl FfOwner {
    /// The element span covering `x`.  `None` also when `x` lands in the
    /// padding a stride wider than the element leaves behind each one.
    fn span(&self, x: usize) -> Option<(usize, usize)> {
        let i = self
            .runs
            .partition_point(|&(s, ..)| s <= x)
            .checked_sub(1)?;
        let (s, stride, count, bytes) = self.runs[i];
        let k = (x - s) / stride;
        if k >= count {
            return None;
        }
        let start = s + k * stride;
        (x < start + bytes).then_some((start, start + bytes))
    }
}

/// Node tables of the module-instance tree plus storage-ownership intervals,
/// prepared by the caller (`ProtoModule::conv`) from `ModuleVariableMeta`.
pub struct ConeGateInputs {
    /// Parent per node; the root's parent is `u32::MAX`.
    pub node_parent: Vec<u32>,
    /// Hierarchical path per node (diagnostics).
    pub node_path: Vec<String>,
    /// Sorted disjoint-start `(start, end, node)` comb-buffer intervals.
    pub comb_owner: Vec<(usize, usize, u32)>,
    pub ff_owner: FfOwner,
    /// Merged comb byte ranges any event statement can write.
    pub event_written_comb: Vec<(usize, usize)>,
}

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
    /// Owning cone's hierarchical path (diagnostics).
    pub cone: String,
}

pub struct ConePlan {
    /// Permutation: `order[new_index] = old_index`.
    pub order: Vec<u32>,
    pub segments: Vec<Segment>,
}

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
    pub cone: String,
}

/// After this many consecutive dirty checks a segment stops being checked:
/// on a workload where the subtree is genuinely active, even the bounded
/// compare cost is pure loss, and the streak proves it cannot pay here.
const AUTO_OFF_STREAK: u32 = 1024;

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
            skipped: 0,
            ran: 0,
            next_report: 1 << 18,
            per_seg: vec![(0, 0); nseg],
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

    /// The segment has just run: derive the convergence verdict and snapshot
    /// the compare bytes.
    #[inline]
    pub fn refresh(&mut self, si: usize, seg: &RtSegment, ff: &[u8], comb: &[u8]) {
        self.ran += 1;
        self.per_seg[si].1 += 1;
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
        self.primed[si] = true;
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

/// Default-on; `VERYL_CONE_GATE=0` opts out.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_CONE_GATE").as_deref() != Ok("0"))
}

pub(crate) fn diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_CONE_GATE_DIAG").as_deref() == Ok("1"))
}

/// Total gated compare bytes cap: bounds the worst-case (always-dirty)
/// per-settle overhead regardless of how many cones qualify.
pub(crate) const MAX_TOTAL_COMPARE: usize = 256 << 10;
/// A segment smaller than this is not worth its dispatch branch.
const MIN_SEGMENT_STMTS: usize = 64;
/// A subtree carrying fewer statements than this is not worth a cone of its
/// own; its statements gate as part of an ancestor's, or not at all.
const MIN_CONE_STMTS: usize = 300;
/// Below this comb statement count the schedule is left alone and only the
/// cone runs it already contains are gated (see `plan`).
const CLUSTER_MIN_STMTS: usize = 5_000;

/// Sort ownership intervals into the exact `sort_unstable()` (lexicographic
/// tuple) order.  One entry per comb ELEMENT is enough for a comparison
/// sort's N·log N to show up in elaboration.  LSD radix over the `start` field (stable
/// byte passes, high passes skipped when every key shares that byte) is O(N);
/// only runs of EQUAL starts (storage aliases, a handful) then need their
/// `(end, id)` tie broken by a comparison sort to make the order exact and
/// deterministic regardless of the source hash-map iteration order.
fn radix_sort_intervals(v: &mut [(usize, usize, u32)]) {
    if v.len() < 2 {
        return;
    }
    // The byte passes cover 32 bits; a start past that (no real buffer is
    // 4 GiB, but stay safe) falls back to the comparison sort.
    if v.iter().any(|t| t.0 > u32::MAX as usize) {
        v.sort_unstable();
        return;
    }
    let mut scratch = vec![(0usize, 0usize, 0u32); v.len()];
    let (mut src, mut dst): (&mut [_], &mut [_]) = (v, &mut scratch);
    let mut flipped = false;
    for pass in 0..4 {
        let shift = pass * 8;
        let mut hist = [0usize; 256];
        for t in src.iter() {
            hist[(t.0 >> shift) & 0xff] += 1;
        }
        if hist.contains(&src.len()) {
            continue;
        }
        let mut pos = [0usize; 256];
        let mut acc = 0;
        for b in 0..256 {
            pos[b] = acc;
            acc += hist[b];
        }
        for &t in src.iter() {
            let b = (t.0 >> shift) & 0xff;
            dst[pos[b]] = t;
            pos[b] += 1;
        }
        std::mem::swap(&mut src, &mut dst);
        flipped = !flipped;
    }
    if flipped {
        dst.copy_from_slice(src);
        std::mem::swap(&mut src, &mut dst);
    }
    // `src` is now `v`. Break ties among equal starts.
    let n = src.len();
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && src[j].0 == src[i].0 {
            j += 1;
        }
        if j - i > 1 {
            src[i..j].sort_unstable();
        }
        i = j;
    }
}

/// Fold `(offset, bytes)` elements into maximal ascending, uniformly strided,
/// equal-width runs, so an array laid out that way collapses to one entry.
/// Anything that breaks the pattern just starts a new run, so the result
/// covers every element exactly once whatever the layout.
fn fold_ff_runs(elems: &[(usize, usize)], out: &mut Vec<(usize, usize, usize, usize)>) {
    let mut i = 0;
    while i < elems.len() {
        let (start, bytes) = elems[i];
        // Two elements decide the stride; a lone one strides by its own width
        // so the run covers exactly it.
        let stride = match elems.get(i + 1) {
            Some(&(next, nb)) if nb == bytes && next > start => next - start,
            _ => bytes,
        };
        let mut count = 1;
        while let Some(&(off, nb)) = elems.get(i + count) {
            if nb != bytes || off != start + count * stride {
                break;
            }
            count += 1;
        }
        out.push((start, stride, count, bytes));
        i += count;
    }
}

/// The comb bytes one variable contributes, and the range they span.
///
/// The bytes are what a compare set would have to cover.  The span is what
/// [`BigArrayFold`] weighs, and `plan` reads offsets through that fold, so a
/// variable the fold collapses has to collapse here too.
fn comb_extent(elements: &[VariableElement], use_4state: bool) -> (usize, usize) {
    let (mut bytes, mut lo, mut hi) = (0usize, usize::MAX, 0usize);
    for el in elements {
        let off = el.current.raw();
        if el.current.is_ff() || off < 0 {
            continue;
        }
        let (off, nb) = (off as usize, value_size(el.native_bytes, use_4state));
        bytes += nb;
        lo = lo.min(off);
        hi = hi.max(off + nb);
    }
    (bytes, hi.saturating_sub(lo))
}

/// Assemble the node tables from the pre-instantiation variable-meta tree.
/// `event_written` holds every comb offset the event statements can write
/// (element-expanded); the covering variable spans join each segment's
/// compare set so an event overwrite wakes the segment.
pub fn build_inputs(
    top_name: &str,
    top_vars: &HashMap<VarId, VariableMeta>,
    children: &[ModuleVariableMeta],
    event_written: &crate::HashSet<isize>,
    use_4state: bool,
) -> ConeGateInputs {
    let mut node_parent: Vec<u32> = vec![u32::MAX];
    let mut node_path: Vec<String> = vec![top_name.to_string()];
    let mut comb_owner: Vec<(usize, usize, u32)> = Vec::new();
    let mut ff_runs: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut ff_elems: Vec<(usize, usize)> = Vec::new();
    let add_vars = |vars: &HashMap<VarId, VariableMeta>,
                    id: u32,
                    comb_owner: &mut Vec<(usize, usize, u32)>,
                    ff_runs: &mut Vec<(usize, usize, usize, usize)>,
                    ff_elems: &mut Vec<(usize, usize)>| {
        for vm in vars.values() {
            ff_elems.clear();
            // A variable past `MAX_TOTAL_COMPARE` can never join a compare set
            // — that constant caps the sum over every gated segment — so
            // per-element owners cost a span, a sort key and a writer list each
            // for a precision no segment can spend.  One span keeps the lookup
            // exact and makes the table a function of the design's variables
            // rather than its array lengths.
            let (comb_bytes, comb_span_bytes) = comb_extent(&vm.elements, use_4state);
            let fold_var = comb_bytes > MAX_TOTAL_COMPARE
                || comb_span_bytes > crate::ir::big_array::FOLD_SPAN_BYTES;
            let mut comb_span: Option<(usize, usize)> = None;
            for el in &vm.elements {
                let off = el.current.raw();
                if off < 0 {
                    continue;
                }
                // The element's WHOLE footprint: under 4-state an equally
                // long X/Z half follows the value bytes, and a compare or a
                // replay covering only the value half would miss changes in
                // it.
                let (off, nb) = (off as usize, value_size(el.native_bytes, use_4state));
                if el.current.is_ff() {
                    ff_elems.push((off, nb));
                } else if fold_var {
                    comb_span = Some(match comb_span {
                        None => (off, off + nb),
                        Some((s, e)) => (s.min(off), e.max(off + nb)),
                    });
                } else {
                    comb_owner.push((off, off + nb, id));
                }
            }
            if let Some((s, e)) = comb_span {
                comb_owner.push((s, e, id));
            }
            fold_ff_runs(ff_elems, ff_runs);
        }
    };
    add_vars(top_vars, 0, &mut comb_owner, &mut ff_runs, &mut ff_elems);
    let mut stack: Vec<(u32, &ModuleVariableMeta)> = Vec::new();
    for c in children {
        stack.push((0, c));
    }
    while let Some((pid, m)) = stack.pop() {
        let id = node_parent.len() as u32;
        node_parent.push(pid);
        node_path.push(format!("{}.{}", node_path[pid as usize], m.name));
        add_vars(
            &m.variable_meta,
            id,
            &mut comb_owner,
            &mut ff_runs,
            &mut ff_elems,
        );
        for c in &m.children {
            stack.push((id, c));
        }
    }
    radix_sort_intervals(&mut comb_owner);
    ff_runs.sort_unstable();
    ff_runs.dedup();
    debug_assert!(
        ff_runs
            .windows(2)
            .all(|w| w[0].0 + w[0].1 * w[0].2 <= w[1].0),
        "FF runs overlap"
    );
    let mut event_written_comb: Vec<(usize, usize)> = Vec::new();
    for &off in event_written {
        if off < 0 {
            continue;
        }
        let x = off as usize;
        let i = comb_owner.partition_point(|&(s, _, _)| s <= x);
        if let Some(i) = i.checked_sub(1)
            && comb_owner[i].0 <= x
            && x < comb_owner[i].1
        {
            event_written_comb.push((comb_owner[i].0, comb_owner[i].1));
        }
    }
    merge_ranges(&mut event_written_comb);
    ConeGateInputs {
        node_parent,
        node_path,
        comb_owner,
        ff_owner: FfOwner { runs: ff_runs },
        event_written_comb,
    }
}

/// Does the statement (recursively) have effects a skip could lose?
fn has_side_effects(s: &ProtoStatement) -> bool {
    match s {
        ProtoStatement::Assign(_) | ProtoStatement::AssignDynamic(_) | ProtoStatement::Break => {
            false
        }
        ProtoStatement::If(x) => x
            .true_side
            .iter()
            .chain(x.false_side.iter())
            .any(has_side_effects),
        ProtoStatement::Case(x) => x
            .arms
            .iter()
            .flat_map(|a| a.body.iter())
            .chain(x.default.iter())
            .any(has_side_effects),
        ProtoStatement::For(x) => x.body.iter().any(has_side_effects),
        ProtoStatement::SequentialBlock(b) => b.iter().any(has_side_effects),
        ProtoStatement::SystemFunctionCall(_)
        | ProtoStatement::CompiledBlock(_)
        | ProtoStatement::TbMethodCall { .. } => true,
    }
}

/// Index of the first `owner` entry that can overlap byte `s`: the
/// partition point walked back over any preceding entries (aliased
/// connect spans share storage) that still end past `s`.  Iterate from
/// here while `owner[j].0 < e`, filtering on `owner[j].1 > s`, to visit
/// every variable span a merged `[s, e)` range covers — a statement's
/// merged output range can span SEVERAL adjacent variables (a case FSM
/// writing neighbouring nets), and touching only the first span would
/// hide the rest from the shared-writer analysis.
fn first_span(owner: &[(usize, usize, u32)], s: usize) -> usize {
    let mut j = owner.partition_point(|&(cs, _, _)| cs <= s);
    while j > 0 && owner[j - 1].1 > s {
        j -= 1;
    }
    j
}

fn merge_ranges(v: &mut Vec<(usize, usize)>) {
    v.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(v.len());
    for &(s, e) in v.iter() {
        match out.last_mut() {
            Some(p) if s <= p.1 => p.1 = p.1.max(e),
            _ => out.push((s, e)),
        }
    }
    *v = out;
}

fn subtract_ranges(a: &[(usize, usize)], b: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut bi = 0;
    for &(mut s, e) in a {
        while s < e {
            while bi < b.len() && b[bi].1 <= s {
                bi += 1;
            }
            match b.get(bi) {
                Some(&(bs, be)) if bs < e => {
                    if s < bs {
                        out.push((s, bs));
                    }
                    s = be.max(s);
                }
                _ => {
                    out.push((s, e));
                    break;
                }
            }
        }
    }
    out
}

fn intersect_ranges(a: &[(usize, usize)], b: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut ai, mut bi) = (0, 0);
    while ai < a.len() && bi < b.len() {
        let (s, e) = (a[ai].0.max(b[bi].0), a[ai].1.min(b[bi].1));
        if s < e {
            out.push((s, e));
        }
        if a[ai].1 < b[bi].1 {
            ai += 1;
        } else {
            bi += 1;
        }
    }
    out
}

struct StmtInfo {
    node: u32,
    /// Whole-variable comb spans this statement reads / writes.
    in_comb: Vec<(usize, usize)>,
    in_ff: Vec<(usize, usize)>,
    out_comb: Vec<(usize, usize)>,
    /// A read or write fell outside every known variable.
    unbounded: bool,
}

/// Build the plan: reorder `stmts` so each selected cone forms few contiguous
/// segments, and describe each gated segment's compare set.  Returns `None`
/// when nothing qualifies (caller keeps the original order).
pub fn plan(stmts: &[ProtoStatement], inputs: &ConeGateInputs) -> Option<ConePlan> {
    let n = stmts.len();
    if n == 0 {
        return None;
    }
    let span_in = |table: &[(usize, usize, u32)], x: usize| -> Option<(usize, usize, u32)> {
        let i = table.partition_point(|&(s, _, _)| s <= x);
        i.checked_sub(1)
            .map(|i| table[i])
            .filter(|&(s, e, _)| s <= x && x < e)
    };
    let parent = |x: u32| -> Option<u32> {
        let p = inputs.node_parent[x as usize];
        (p != u32::MAX).then_some(p)
    };
    let depth = |mut x: u32| -> usize {
        let mut d = 0;
        while let Some(p) = parent(x) {
            x = p;
            d += 1;
        }
        d
    };
    let lca = |mut a: u32, mut b: u32| -> u32 {
        let (mut da, mut db) = (depth(a), depth(b));
        while da > db {
            a = parent(a).unwrap();
            da -= 1;
        }
        while db > da {
            b = parent(b).unwrap();
            db -= 1;
        }
        while a != b {
            a = parent(a).unwrap();
            b = parent(b).unwrap();
        }
        a
    };

    // -- per-statement info + node attribution.
    let root = (0..inputs.node_parent.len() as u32)
        .find(|&i| inputs.node_parent[i as usize] == u32::MAX)?;
    let mut infos: Vec<StmtInfo> = Vec::with_capacity(n);
    let mut poisoned: Vec<bool> = vec![false; inputs.node_parent.len()];
    let mut ins: Vec<VarOffset> = Vec::new();
    let mut outs: Vec<VarOffset> = Vec::new();
    // Membership and the compare sets are keyed on owner spans, and a folded
    // array owns exactly one — `build_inputs` collapses whatever this fold
    // does.  Expanding its elements would resolve that same span once per
    // element, before `merge_ranges` collapsed them back into it.
    let fold = BigArrayFold::from_statements(stmts.iter());
    for s in stmts {
        ins.clear();
        outs.clear();
        s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
        let mut info = StmtInfo {
            node: root,
            in_comb: Vec::new(),
            in_ff: Vec::new(),
            out_comb: Vec::new(),
            unbounded: false,
        };
        let mut node: Option<u32> = None;
        for o in &outs {
            match o {
                VarOffset::Comb(x) if *x >= 0 => match span_in(&inputs.comb_owner, *x as usize) {
                    Some((s0, e0, id)) => {
                        info.out_comb.push((s0, e0));
                        node = Some(node.map_or(id, |a| lca(a, id)));
                    }
                    None => info.unbounded = true,
                },
                _ => info.unbounded = true, // FF-writing / negative: root only
            }
        }
        for o in &ins {
            match o {
                VarOffset::Comb(x) if *x >= 0 => match span_in(&inputs.comb_owner, *x as usize) {
                    Some((s0, e0, _)) => info.in_comb.push((s0, e0)),
                    None => info.unbounded = true,
                },
                VarOffset::Ff(x) if *x >= 0 => match inputs.ff_owner.span(*x as usize) {
                    Some((s0, e0)) => info.in_ff.push((s0, e0)),
                    None => info.unbounded = true,
                },
                _ => info.unbounded = true,
            }
        }
        merge_ranges(&mut info.in_comb);
        merge_ranges(&mut info.in_ff);
        merge_ranges(&mut info.out_comb);
        info.node = if info.unbounded || outs.is_empty() {
            root
        } else {
            node.unwrap_or(root)
        };
        // A side-effecting or unbounded statement poisons its whole ancestor
        // chain: no cone containing it may skip.
        if has_side_effects(s) || info.unbounded {
            let mut m = info.node;
            loop {
                poisoned[m as usize] = true;
                match parent(m) {
                    Some(p) => m = p,
                    None => break,
                }
            }
        }
        infos.push(info);
    }
    // -- statement counts per node, aggregated up.
    let mut count: Vec<usize> = vec![0; inputs.node_parent.len()];
    for info in &infos {
        let mut m = info.node;
        loop {
            count[m as usize] += 1;
            match parent(m) {
                Some(p) => m = p,
                None => break,
            }
        }
    }
    if diag() {
        // Where does the ungated mass sit?  For every statement parked at
        // the root by an untracked offset (a rename/selector temp), estimate
        // the node its TRACKED reads/writes point at — the recoverable mass
        // per node if temps were carried in the owner tables.
        let mut direct: Vec<usize> = vec![0; inputs.node_parent.len()];
        let mut rescuable: Vec<usize> = vec![0; inputs.node_parent.len()];
        for (i, info) in infos.iter().enumerate() {
            direct[info.node as usize] += 1;
            if info.unbounded && !has_side_effects(&stmts[i]) {
                let mut node: Option<u32> = None;
                for &(s0, _) in &info.out_comb {
                    if let Some((_, _, id)) = span_in(&inputs.comb_owner, s0) {
                        node = Some(node.map_or(id, |a| lca(a, id)));
                    }
                }
                if node.is_none() {
                    for &(s0, _) in &info.in_comb {
                        if let Some((_, _, id)) = span_in(&inputs.comb_owner, s0) {
                            node = Some(node.map_or(id, |a| lca(a, id)));
                        }
                    }
                }
                if let Some(m) = node
                    && m != root
                {
                    rescuable[m as usize] += 1;
                }
            }
        }
        for m in 0..inputs.node_parent.len() {
            if direct[m] >= 64 || rescuable[m] >= 64 || count[m] >= MIN_CONE_STMTS {
                eprintln!(
                    "[cone_gate] node count={} direct={} rescuable={} poisoned={} {}",
                    count[m], direct[m], rescuable[m], poisoned[m], inputs.node_path[m],
                );
            }
        }
    }

    // -- candidates: every qualifying node.  A statement joins its DEEPEST
    //    qualifying ancestor: deep subtrees (an idle arbiter, an FPU) are the
    //    ones whose inputs actually sit still, while a huge ancestor cone
    //    would inherit the always-active core and never skip.  A shallower
    //    qualifying node still gates its RESIDUAL statements as its own slot.
    let qualifies = |m: u32| -> bool {
        m != root && !poisoned[m as usize] && count[m as usize] >= MIN_CONE_STMTS
    };
    let cones: Vec<u32> = (0..inputs.node_parent.len() as u32)
        .filter(|&m| qualifies(m))
        .collect();
    if cones.is_empty() {
        return None;
    }

    // -- statement -> cone slot (0 = outside, i+1 = cones[i]).
    let cone_of = |mut m: u32| -> usize {
        loop {
            if qualifies(m)
                && let Some(i) = cones.iter().position(|&c| c == m)
            {
                return i + 1;
            }
            match parent(m) {
                Some(p) => m = p,
                None => return 0,
            }
        }
    };
    let slot: Vec<usize> = infos.iter().map(|i| cone_of(i.node)).collect();

    // -- per-span writer lists, consumed by the compare/replay construction
    //    and the clustering re-sort below.  They come out position-ascending
    //    and duplicate-free because the statement loop runs in order and two
    //    owner spans are always either identical or disjoint, so no span is
    //    reachable from two of one statement's merged out ranges.
    let mut writers: Vec<Vec<u32>> = vec![Vec::new(); inputs.comb_owner.len()];
    for (i, info) in infos.iter().enumerate() {
        for &(s, e) in &info.out_comb {
            let mut j = first_span(&inputs.comb_owner, s);
            while j < inputs.comb_owner.len() && inputs.comb_owner[j].0 < e {
                if inputs.comb_owner[j].1 > s {
                    writers[j].push(i as u32);
                }
                j += 1;
            }
        }
    }

    // -- Small designs: keep the original order and gate the maximal cone
    //    runs it already contains.  Clustering buys its extra gated mass out
    //    of the schedule's ACTIVE remainder, whose fat compare sets and guard
    //    glue cost more than the few extra skips return until the statement
    //    count is well past `CLUSTER_MIN_STMTS`.
    if n < CLUSTER_MIN_STMTS {
        let order: Vec<u32> = (0..n as u32).collect();
        let mut bursts: Vec<(usize, usize, usize)> = Vec::new();
        let mut start = 0usize;
        for i in 1..=n {
            if i == n || slot[i] != slot[start] {
                bursts.push((slot[start], start, i));
                start = i;
            }
        }
        return finish_plan(order, bursts, &infos, inputs, &cones, &writers);
    }

    // -- Pass-preserving clustering re-sort: an anchored Kahn over the
    //    COMPLETE hazard graph.  Constraint edges cover RAW (a reader stays
    //    after its latest earlier writer), WAR (a reader stays before its
    //    next later writer — this also pins the orientation of every settle
    //    back-edge, whose reader positionally precedes its writer), WAW
    //    (writers of a span keep their order), and a chain over statements
    //    with untracked footprints (side effects, offsets outside the owner
    //    tables — rename/selector temps).  Every edge points forward in the
    //    incoming order, so the graph is acyclic with the incoming order as
    //    witness, and ANY topological order of it preserves each dataflow
    //    edge's positional direction — the positional pass metric cannot
    //    move.  WAR is what makes that hold: without it a settle back-edge
    //    can flip, and the extra passes cost more than tighter segments save.
    let mut edge_set: crate::HashSet<(u32, u32)> = crate::HashSet::default();
    let mut csucc: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut cdeg: Vec<u32> = vec![0; n];
    let mut add_edge = |a: u32, b: u32, csucc: &mut Vec<Vec<u32>>, cdeg: &mut Vec<u32>| {
        debug_assert!(a < b, "constraint edges must point forward");
        if edge_set.insert((a, b)) {
            csucc[a as usize].push(b);
            cdeg[b as usize] += 1;
        }
    };
    // WAW: chain each span's writers.
    for w in &writers {
        for p in w.windows(2) {
            add_edge(p[0], p[1], &mut csucc, &mut cdeg);
        }
    }
    // RAW + WAR per (reader, span).
    for (i, info) in infos.iter().enumerate() {
        for &(s, e) in &info.in_comb {
            let mut j = first_span(&inputs.comb_owner, s);
            while j < inputs.comb_owner.len() && inputs.comb_owner[j].0 < e {
                if inputs.comb_owner[j].1 > s {
                    let w = &writers[j];
                    let mut k = w.partition_point(|&x| (x as usize) < i);
                    if let Some(p) = k.checked_sub(1) {
                        add_edge(w[p], i as u32, &mut csucc, &mut cdeg);
                    }
                    if w.get(k) == Some(&(i as u32)) {
                        k += 1; // a self-reading writer orders via WAW
                    }
                    if let Some(&q) = w.get(k) {
                        add_edge(i as u32, q, &mut csucc, &mut cdeg);
                    }
                }
                j += 1;
            }
        }
    }
    // Untracked-footprint chain: statements whose reads or writes fall
    // outside the owner tables can only conflict with each other (tracked
    // statements never touch those bytes), so keeping their relative order
    // is a sufficient conservative constraint.
    let mut prev_special: Option<u32> = None;
    for (i, info) in infos.iter().enumerate() {
        if info.unbounded || has_side_effects(&stmts[i]) {
            if let Some(p) = prev_special {
                add_edge(p, i as u32, &mut csucc, &mut cdeg);
            }
            prev_special = Some(i as u32);
        }
    }

    // Slot-sticky Kahn: outside statements are emitted by lowest original
    // position (their relative order survives), cone statements are pulled
    // into contiguous runs.  Eager emission stratifies each cone by
    // readiness: its FF-fed (quiet) statements are all ready before any
    // outside statement runs and convene into the first bursts, while
    // parts chained behind live core logic surface later — so a cone's
    // skippable mass is not fused with its active mass.  Pinning the runs
    // the original order already contains would defeat that: their members
    // block the rest of their cone from convening, shattering one cone into
    // several fat compare sets.  The runs are instead recovered by
    // SEGMENTATION below — same-wave members pop in ascending original
    // position, so a run re-emerges contiguously inside its cone's burst
    // and the seam split hands it back its own narrow compare set.
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let nslots = cones.len() + 1;
    let mut pinned = vec![false; n];
    {
        let mut start = 0usize;
        for i in 1..=n {
            if i == n || slot[i] != slot[start] {
                if slot[start] != 0 && i - start >= MIN_SEGMENT_STMTS {
                    pinned[start..i].iter_mut().for_each(|p| *p = true);
                }
                start = i;
            }
        }
    }
    let mut ready: Vec<BinaryHeap<Reverse<u32>>> = (0..nslots).map(|_| BinaryHeap::new()).collect();
    for i in 0..n {
        if cdeg[i] == 0 {
            ready[slot[i]].push(Reverse(i as u32));
        }
    }
    let mut order: Vec<u32> = Vec::with_capacity(n);
    let mut cur_slot = 0usize;
    while order.len() < n {
        let sl = if !ready[cur_slot].is_empty() {
            cur_slot
        } else {
            (1..nslots)
                .filter(|&sl| !ready[sl].is_empty())
                .min_by_key(|&sl| ready[sl].peek().unwrap().0)
                .unwrap_or(0)
        };
        let Some(Reverse(i)) = ready[sl].pop() else {
            // Unreachable by construction (all edges forward => acyclic);
            // bail to the unclustered schedule rather than trusting a
            // partial order.
            debug_assert!(false, "hazard graph stalled");
            order = (0..n as u32).collect();
            break;
        };
        cur_slot = sl;
        order.push(i);
        for &c in &csucc[i as usize] {
            let c = c as usize;
            cdeg[c] -= 1;
            if cdeg[c] == 0 {
                ready[slot[c]].push(Reverse(c as u32));
            }
        }
    }
    // (slot, start, end) bursts in the new order, split at pinned↔cluster
    // seams so a pinned run keeps its own narrow compare set even when a
    // same-slot cluster burst lands adjacent.
    let mut bursts: Vec<(usize, usize, usize)> = Vec::new();
    let mut start = 0usize;
    for i in 1..=n {
        let brk = i == n
            || slot[order[i] as usize] != slot[order[start] as usize]
            || pinned[order[i] as usize] != pinned[order[start] as usize];
        if brk {
            bursts.push((slot[order[start] as usize], start, i));
            start = i;
        }
    }
    finish_plan(order, bursts, &infos, inputs, &cones, &writers)
}

/// Turn the (order, bursts) of either scheduler into gated segments with
/// their compare sets, applying the static profitability filter.
fn finish_plan(
    order: Vec<u32>,
    bursts: Vec<(usize, usize, usize)>,
    infos: &[StmtInfo],
    inputs: &ConeGateInputs,
    cones: &[u32],
    writers: &[Vec<u32>],
) -> Option<ConePlan> {
    let n = order.len();
    let mut in_burst = vec![false; n];
    let mut segments: Vec<Segment> = Vec::new();
    let mut total_bytes = 0usize;
    for &(sl, start, end) in &bursts {
        if sl == 0 || end - start < MIN_SEGMENT_STMTS {
            if diag() && sl != 0 && end - start >= 8 {
                eprintln!(
                    "[cone_gate] drop too-small [{start}..{end}) stmts={} {}",
                    end - start,
                    inputs.node_path[cones[sl - 1] as usize],
                );
            }
            continue;
        }
        // A variable written both by this burst and by a statement OUTSIDE it
        // (an FSM's `nxtstate = 0` default split from its conditional
        // assignments) breaks under a plain skip: last-writer-wins depends on
        // the write ORDER within the settle, and skipping only the inside
        // writers leaves the outside writer's value standing.  A skip must
        // therefore REPLAY the segment's stored outputs for those spans —
        // that reproduces a real run exactly whether the outside writer sits
        // before the segment (the segment's write wins either way) or after
        // it (the outside statement still runs later and overwrites).
        for &oi in &order[start..end] {
            in_burst[oi as usize] = true;
        }
        let mut replay: Vec<(usize, usize)> = Vec::new();
        for &oi in &order[start..end] {
            for &(s, e) in &infos[oi as usize].out_comb {
                // Walk EVERY variable span the merged range covers: an
                // outside writer of any one of them makes the whole range
                // order-dependent.
                let mut j = first_span(&inputs.comb_owner, s);
                let shared = loop {
                    if j >= inputs.comb_owner.len() || inputs.comb_owner[j].0 >= e {
                        break false;
                    }
                    if inputs.comb_owner[j].1 > s
                        && writers[j].iter().any(|&w| !in_burst[w as usize])
                    {
                        break true;
                    }
                    j += 1;
                };
                if shared {
                    replay.push((s, e));
                }
            }
        }
        for &oi in &order[start..end] {
            in_burst[oi as usize] = false;
        }
        merge_ranges(&mut replay);
        let mut in_comb: Vec<(usize, usize)> = Vec::new();
        let mut in_ff: Vec<(usize, usize)> = Vec::new();
        let mut out_comb: Vec<(usize, usize)> = Vec::new();
        for &oi in &order[start..end] {
            let info = &infos[oi as usize];
            in_comb.extend_from_slice(&info.in_comb);
            in_ff.extend_from_slice(&info.in_ff);
            out_comb.extend_from_slice(&info.out_comb);
        }
        merge_ranges(&mut in_comb);
        merge_ranges(&mut in_ff);
        merge_ranges(&mut out_comb);
        let ext = subtract_ranges(&in_comb, &out_comb);
        // Event/component writes to member outputs are outside writers too:
        // route them through the replay, whose pre-run compare judges each
        // byte as the segment CONSUMED it.  The post-run compare cannot —
        // such a write lands after the run its shadow describes, so the
        // shadow no longer says anything about the byte.
        let evt_out = intersect_ranges(&inputs.event_written_comb, &out_comb);
        replay.extend(evt_out);
        merge_ranges(&mut replay);
        // Bytes the segment both reads and writes carry its internal chain
        // state: a read can precede its writer (a settle back-edge, a
        // self-reading RMW, a bit-sliced accumulator) and then consumes the
        // PREVIOUS pass's value, which `in − out` hides.  Spans are whole
        // variables, so partial writes make a positional read-before-write
        // analysis unsound here — keep the whole intersection in the compare
        // set.  Under unchanged external inputs these bytes are at their
        // fixpoint and compare equal, so the skip rate is unharmed.
        let backedge_all = intersect_ranges(&in_comb, &out_comb);
        // EVERY replay span must pass the pre-run compare, not just the
        // read-back ones.  Replay restores whole-variable snapshots, but a
        // burst statement may write only PART of the variable (a field
        // assign): the untouched bytes in the snapshot belong to the
        // OUTSIDE writers as of the snapshot settle, and blindly restoring
        // them would roll those writers back.  Requiring the span's bytes
        // at the segment position to equal their value at the snapshot
        // run's start makes the replay an exact reproduction: touched
        // bytes recompute equal (inputs unchanged), untouched bytes are
        // rewritten with the value they already hold.  A fresh-but-equal
        // event write still compares equal here, which is the point of
        // the pre-run compare over the post-run one.
        let compare_pre = replay.clone();
        let backedge = subtract_ranges(&backedge_all, &replay);
        let mut compare: Vec<(bool, u32, u32)> = Vec::new();
        let mut comb_cmp = ext;
        comb_cmp.extend(backedge.iter().copied());
        merge_ranges(&mut comb_cmp);
        let comb_cmp = subtract_ranges(&comb_cmp, &compare_pre);
        compare.extend(comb_cmp.iter().map(|&(s, e)| (false, s as u32, e as u32)));
        compare.extend(in_ff.iter().map(|&(s, e)| (true, s as u32, e as u32)));
        let bytes: usize = compare
            .iter()
            .map(|&(_, s, e)| (e - s) as usize)
            .sum::<usize>()
            + compare_pre.iter().map(|&(s, e)| e - s).sum::<usize>();
        // Static bet: a vectorised compare costs far less per byte than
        // evaluating a statement, so one statement per 32 compare bytes still
        // pays off several-fold at a high skip rate — and a segment that
        // cannot skip auto-offs into a cost of zero, bounding the downside.
        if (end - start) < bytes / 32 || total_bytes + bytes > MAX_TOTAL_COMPARE {
            if diag() {
                eprintln!(
                    "[cone_gate] drop unprofitable [{start}..{end}) stmts={} bytes={bytes} {}",
                    end - start,
                    inputs.node_path[cones[sl - 1] as usize],
                );
            }
            continue;
        }
        total_bytes += bytes;
        // Break-even skip rate of that bet.  With +1 per dirty check and
        // -decay per skip, the streak drifts into auto-off exactly below it;
        // a free compare (bytes = 0) never expires.
        let off_decay = ((5 * (end - start)).checked_div(bytes))
            .map_or(u32::MAX, |r| r.saturating_sub(1).min(1 << 20) as u32);
        segments.push(Segment {
            start,
            end,
            compare,
            backedge: backedge
                .iter()
                .map(|&(s, e)| (s as u32, e as u32))
                .collect(),
            replay: replay.iter().map(|&(s, e)| (s as u32, e as u32)).collect(),
            bytes,
            off_decay,
            cone: inputs.node_path[cones[sl - 1] as usize].clone(),
        });
    }
    if diag() {
        eprintln!(
            "[cone_gate] stmts={} cones={} segments={} gated_stmts={} compare_bytes={}",
            n,
            cones.len(),
            segments.len(),
            segments.iter().map(|s| s.end - s.start).sum::<usize>(),
            total_bytes,
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
    Some(ConePlan { order, segments })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random intervals, including the shapes the sort
    /// has to special-case: repeated starts (storage aliases), starts sharing
    /// a byte in every position, and a run long enough to need all passes.
    fn sample_intervals(n: usize) -> Vec<(usize, usize, u32)> {
        let mut v = Vec::with_capacity(n);
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        for i in 0..n {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let start = (x % 4096) as usize * 8;
            v.push((start, start + 8, i as u32));
        }
        v
    }

    #[test]
    fn radix_sort_matches_the_comparison_sort() {
        for n in [0, 1, 2, 3, 17, 256, 1000] {
            let mut a = sample_intervals(n);
            let mut b = a.clone();
            radix_sort_intervals(&mut a);
            b.sort_unstable();
            assert_eq!(a, b, "n={n}");
        }
        // Every start in one byte bucket: the high passes are skipped, so the
        // final copy-back has to land on the right buffer.
        let mut a: Vec<(usize, usize, u32)> = (0..64).rev().map(|i| (i, i + 1, i as u32)).collect();
        let mut b = a.clone();
        radix_sort_intervals(&mut a);
        b.sort_unstable();
        assert_eq!(a, b, "single byte bucket");
        // Equal starts must break their tie on (end, id), not on input order.
        let mut a = vec![(8, 40, 7u32), (8, 16, 3), (8, 40, 2), (0, 8, 9)];
        let mut b = a.clone();
        radix_sort_intervals(&mut a);
        b.sort_unstable();
        assert_eq!(a, b, "aliased starts");
        // Past the 32-bit passes the comparison sort takes over.
        let big = (u32::MAX as usize) + 16;
        let mut a = vec![(big, big + 8, 1u32), (0, 8, 0)];
        radix_sort_intervals(&mut a);
        assert_eq!(a, vec![(0, 8, 0), (big, big + 8, 1)]);
    }

    #[test]
    fn range_algebra_is_exact() {
        let mut v = vec![(10, 20), (0, 5), (18, 25), (5, 8)];
        merge_ranges(&mut v);
        assert_eq!(v, vec![(0, 8), (10, 25)]);

        // Subtraction has to split a range a hole falls inside, drop one it
        // covers entirely, and leave one it misses.
        assert_eq!(
            subtract_ranges(&[(0, 100)], &[(10, 20), (30, 40)]),
            vec![(0, 10), (20, 30), (40, 100)]
        );
        assert_eq!(subtract_ranges(&[(10, 20)], &[(0, 100)]), vec![]);
        assert_eq!(subtract_ranges(&[(10, 20)], &[(20, 30)]), vec![(10, 20)]);
        assert_eq!(subtract_ranges(&[], &[(0, 10)]), vec![]);

        assert_eq!(
            intersect_ranges(&[(0, 10), (20, 30)], &[(5, 25)]),
            vec![(5, 10), (20, 25)]
        );
        assert_eq!(intersect_ranges(&[(0, 10)], &[(10, 20)]), vec![]);
    }

    #[test]
    fn ff_runs_fold_only_what_keeps_the_pattern() {
        let fold = |e: &[(usize, usize)]| {
            let mut v = Vec::new();
            fold_ff_runs(e, &mut v);
            v
        };
        // A plain array: one run.
        assert_eq!(
            fold(&[(0, 8), (8, 8), (16, 8), (24, 8)]),
            vec![(0, 8, 4, 8)]
        );
        // A gap breaks the stride, so the tail starts its own run.
        assert_eq!(
            fold(&[(0, 8), (8, 8), (64, 8), (72, 8)]),
            vec![(0, 8, 2, 8), (64, 8, 2, 8)]
        );
        // So does a width change, and a lone element covers just itself.
        assert_eq!(fold(&[(0, 8), (8, 4)]), vec![(0, 8, 1, 8), (8, 4, 1, 4)]);
        // Unpacked FFs stride by twice the element, leaving padding behind
        // each; the run must record the stride, not the width.
        assert_eq!(fold(&[(0, 8), (16, 8), (32, 8)]), vec![(0, 16, 3, 8)]);
        // A descending pair cannot be a run.
        assert_eq!(fold(&[(16, 8), (0, 8)]), vec![(16, 8, 1, 8), (0, 8, 1, 8)]);
        assert!(fold(&[]).is_empty());
        // Every element is covered exactly once, whatever the shape.
        let elems = [(0, 8), (8, 8), (24, 8), (32, 8), (33, 1)];
        let covered: usize = fold(&elems).iter().map(|&(_, _, c, _)| c).sum();
        assert_eq!(covered, elems.len());
    }

    #[test]
    fn comb_extent_measures_the_compare_set_and_the_fold() {
        let el = |nb: usize, off: VarOffset| VariableElement {
            native_bytes: nb,
            current: off,
            next_offset: 0,
        };
        // FF elements gate through `FfOwner`, and a negative offset is not
        // storage at all; neither joins a comb compare set.
        let mixed = [
            el(8, VarOffset::Comb(0)),
            el(8, VarOffset::Ff(0)),
            el(8, VarOffset::Comb(-1)),
            el(4, VarOffset::Comb(64)),
        ];
        assert_eq!(comb_extent(&mixed, false), (12, 68));
        // 4-state doubles every element: the X/Z half has to be compared too.
        assert_eq!(comb_extent(&mixed, true), (24, 72));
        // The budget decides the fold, so an array just over it folds and one
        // just under it does not.
        let arr = |n: usize| -> Vec<VariableElement> {
            (0..n)
                .map(|i| el(8, VarOffset::Comb((i * 8) as isize)))
                .collect()
        };
        let just_over = arr(MAX_TOTAL_COMPARE / 8 + 1);
        let just_under = arr(MAX_TOTAL_COMPARE / 8);
        assert!(comb_extent(&just_over, false).0 > MAX_TOTAL_COMPARE);
        assert!(comb_extent(&just_under, false).0 <= MAX_TOTAL_COMPARE);
        // A padded array can span past the fold's threshold while its elements
        // stay under the budget; those spans have to collapse anyway, because
        // `plan` reads such an array folded.
        let padded: Vec<VariableElement> = (0..64)
            .map(|i| {
                el(
                    8,
                    VarOffset::Comb((i * crate::ir::big_array::FOLD_SPAN_BYTES / 32) as isize),
                )
            })
            .collect();
        let (bytes, span) = comb_extent(&padded, false);
        assert!(bytes <= MAX_TOTAL_COMPARE);
        assert!(span > crate::ir::big_array::FOLD_SPAN_BYTES);
    }

    #[test]
    fn ff_owner_resolves_a_run_back_to_its_element() {
        // One 4-element array of 8-byte elements, then a lone 16-byte one.
        let o = FfOwner {
            runs: vec![(64, 8, 4, 8), (128, 16, 1, 16)],
        };
        assert_eq!(o.span(64), Some((64, 72)));
        assert_eq!(o.span(71), Some((64, 72)));
        assert_eq!(o.span(72), Some((72, 80)));
        assert_eq!(o.span(88), Some((88, 96)));
        assert_eq!(o.span(128), Some((128, 144)));
        // Past the last element of a run, and before the first run.
        assert_eq!(o.span(96), None);
        assert_eq!(o.span(63), None);
        assert_eq!(o.span(144), None);
        // Padding between elements belongs to none of them.
        let padded = FfOwner {
            runs: vec![(0, 16, 3, 8)],
        };
        assert_eq!(padded.span(0), Some((0, 8)));
        assert_eq!(padded.span(7), Some((0, 8)));
        assert_eq!(padded.span(8), None);
        assert_eq!(padded.span(15), None);
        assert_eq!(padded.span(16), Some((16, 24)));
        assert_eq!(padded.span(47), None);
    }

    #[test]
    fn first_span_reaches_every_alias_of_a_byte() {
        // Aliased connect spans share storage, so one byte can be owned by
        // several entries; the scan must start at the first of them or the
        // shared-writer analysis silently misses the rest.
        let owner = vec![
            (0, 8, 0u32),
            (8, 16, 1),
            (8, 16, 2),
            (8, 16, 3),
            (16, 24, 4),
        ];
        assert_eq!(first_span(&owner, 0), 0);
        assert_eq!(first_span(&owner, 8), 1);
        assert_eq!(first_span(&owner, 12), 1);
        assert_eq!(first_span(&owner, 16), 4);
        // A byte past every entry yields an empty scan rather than a panic.
        assert_eq!(first_span(&owner, 24), 5);
    }
}
