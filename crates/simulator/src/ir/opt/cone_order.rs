//! Cone reorder of the comb schedule.
//!
//! Statements are attributed to the module-instance subtree they compute, and
//! each qualifying subtree's statements are pulled into a few contiguous runs
//! by a topological re-sort of the complete hazard graph, so the reorder never
//! flips a dataflow edge and the positional pass count cannot move.
//!
//! Whether the reorder is adopted is a static heuristic: a run counts as a
//! segment when the bytes that decide whether its inputs moved are few
//! against the statements it holds, and the order is adopted only when some
//! run counts.  Nothing compares those bytes at run time; the count decides
//! only adoption and, through `comb_fusion::adopt_field_defs`, which fused
//! comb list is kept.

use super::owners::{
    OwnerInputs, diag, first_ff_span, first_span, has_side_effects, intersect_ranges, merge_ranges,
    prefix_max_end, subtract_ranges,
};
use crate::ir::big_array::BigArrayFold;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;

thread_local! {
    /// Why statements ended up attributed to the root, and so in no cone:
    /// `0` unbounded storage, `1` no comb output, `2` a root-level write.
    static ROOT_REASONS: std::cell::Cell<[usize; 9]> = const { std::cell::Cell::new([0; 9]) };
}

thread_local! {
    /// Comb offsets that landed in no owner span, for the diag report.
    static MISSES: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn record_miss(off: usize) {
    MISSES.with(|m| m.borrow_mut().push(off));
}

fn root_reason(k: usize) {
    ROOT_REASONS.with(|c| {
        let mut v = c.get();
        v[k] += 1;
        c.set(v);
    });
}

/// Print and clear the [`root_reason`] tally.  Every one of these statements
/// lands outside every cone, so the mix says whether that remainder is an
/// analysis gap or the design's own top level.
fn report_root_reasons(name: &str, owner: &[(usize, usize, u32)]) {
    let v = ROOT_REASONS.with(|c| c.replace([0; 9]));
    if v.iter().any(|&x| x > 0) {
        eprintln!(
            "[cone_order] root-attributed stmts: unbounded={} no_comb_out={} root_write={} {}",
            v[0], v[1], v[2], name,
        );
        eprintln!(
            "[cone_order]   unbounded by: out_comb_miss={} out_ff={} out_neg={} \
             in_comb_miss={} in_ff_miss={} in_neg={}",
            v[3], v[4], v[5], v[6], v[7], v[8],
        );
        MISSES.with(|m| {
            let mut v = m.borrow_mut();
            v.sort_unstable();
            v.dedup();
            let (lo, hi) = (v.first().copied(), v.last().copied());
            eprintln!(
                "[cone_order]   unowned comb offsets: distinct={} min={lo:?} max={hi:?}",
                v.len(),
            );
            let covered: usize = owner.iter().map(|&(a, b, _)| b - a).sum();
            let extent = owner.last().map_or(0, |&(_, e, _)| e);
            eprintln!(
                "[cone_order]   comb_owner: spans={} covered={covered} extent={extent} ({:.1}% of extent)",
                owner.len(),
                100.0 * covered as f64 / extent.max(1) as f64,
            );
            v.clear();
        });
    }
}

/// Input bytes the segments may count in total per comb statement.  A
/// segment needs one statement per 32 of its own input bytes, so this caps
/// the counted segments at a quarter of what that ratio would admit.
const COMPARE_BYTES_PER_STMT: usize = 8;

/// The design's total input-byte budget for segments, proportional to the
/// comb statement count.  An absolute cap would hand the whole budget to
/// whichever cones the schedule reaches first and leave the rest of the
/// design unclustered.
fn compare_budget(comb_stmts: usize) -> usize {
    comb_stmts.saturating_mul(COMPARE_BYTES_PER_STMT)
}
/// A run shorter than this does not count as a segment: a cone that
/// fragments into short bursts is not clustered.
const MIN_SEGMENT_STMTS: usize = 64;
/// A subtree carrying fewer statements than this is not worth a cone of its
/// own; its statements join an ancestor's, or no cone at all.
const MIN_CONE_STMTS: usize = 300;
/// Below this comb statement count the schedule is left alone and only the
/// cone runs it already contains count as segments (see `schedule`).
const CLUSTER_MIN_STMTS: usize = 5_000;

pub(crate) struct StmtInfo {
    pub(crate) node: u32,
    /// Whole-variable comb spans this statement reads / writes.
    pub(crate) in_comb: Vec<(usize, usize)>,
    pub(crate) in_ff: Vec<(usize, usize)>,
    pub(crate) out_comb: Vec<(usize, usize)>,
    /// FF element spans this statement writes.  A combinational write into
    /// an FF word joins no cone (it sets `unbounded`), but the hazard graph
    /// still has to order it against the statements that READ those bytes:
    /// those resolve through `ff_owner` and are tracked, so the
    /// untracked-footprint chain does not cover the pair.
    pub(crate) out_ff: Vec<(usize, usize)>,
    /// A read or write fell outside every known variable.
    pub(crate) unbounded: bool,
    /// A read fell outside every known variable: any writer may feed it.
    pub(crate) unknown_in: bool,
}

/// The reorder, with what the segment selection reads back from it.
pub(crate) struct Schedule {
    /// Permutation: `order[new_index] = old_index`.
    pub(crate) order: Vec<u32>,
    /// `(slot, start, end)` runs of one cone slot in the new order; slot 0 is
    /// outside every cone, slot `i + 1` is `cones[i]`.
    pub(crate) bursts: Vec<(usize, usize, usize)>,
    /// Per statement, in the ORIGINAL order.
    pub(crate) infos: Vec<StmtInfo>,
    pub(crate) cones: Vec<u32>,
    /// Writers of each `comb_owner` span, ascending original positions.
    pub(crate) writers: Vec<Vec<u32>>,
}

/// Reorder `stmts` so each qualifying cone forms few contiguous runs.
/// `None` when no node qualifies.
pub(crate) fn schedule(stmts: &[ProtoStatement], inputs: &OwnerInputs) -> Option<Schedule> {
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
    let parent = |x: u32| -> Option<u32> { inputs.parent(x) };
    let lca = |a: u32, b: u32| -> u32 { inputs.lca(a, b) };

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
            out_ff: Vec::new(),
            unbounded: false,
            unknown_in: false,
        };
        let mut node: Option<u32> = None;
        for o in &outs {
            match o {
                VarOffset::Comb(x) if *x >= 0 => match span_in(&inputs.comb_owner, *x as usize) {
                    Some((s0, e0, id)) => {
                        info.out_comb.push((s0, e0));
                        node = Some(node.map_or(id, |a| lca(a, id)));
                    }
                    None => {
                        info.unbounded = true;
                        if diag() {
                            root_reason(3); // out: comb offset outside every owner span
                            record_miss(*x as usize);
                        }
                    }
                },
                VarOffset::Ff(x) => {
                    if *x >= 0
                        && let Some(span) = inputs.ff_owner.span(*x as usize)
                    {
                        info.out_ff.push(span);
                    }
                    info.unbounded = true; // FF-writing: root only
                    if diag() {
                        root_reason(4);
                    }
                }
                _ => {
                    info.unbounded = true; // negative offset
                    if diag() {
                        root_reason(5);
                    }
                }
            }
        }
        for o in &ins {
            match o {
                VarOffset::Comb(x) if *x >= 0 => match span_in(&inputs.comb_owner, *x as usize) {
                    Some((s0, e0, _)) => info.in_comb.push((s0, e0)),
                    None => {
                        info.unbounded = true;
                        info.unknown_in = true;
                        if diag() {
                            root_reason(6); // in: comb offset outside every owner span
                            record_miss(*x as usize);
                        }
                    }
                },
                VarOffset::Ff(x) if *x >= 0 => match inputs.ff_owner.span(*x as usize) {
                    Some((s0, e0)) => info.in_ff.push((s0, e0)),
                    None => {
                        info.unbounded = true;
                        info.unknown_in = true;
                        if diag() {
                            root_reason(7); // in: ff offset outside every owner run
                        }
                    }
                },
                _ => {
                    info.unbounded = true;
                    info.unknown_in = true;
                    if diag() {
                        root_reason(8); // in: negative offset
                    }
                }
            }
        }
        merge_ranges(&mut info.in_comb);
        merge_ranges(&mut info.in_ff);
        merge_ranges(&mut info.out_comb);
        merge_ranges(&mut info.out_ff);
        info.node = if info.unbounded || outs.is_empty() {
            root
        } else {
            node.unwrap_or(root)
        };
        if diag() && info.node == root {
            // Why this statement could not join a cone.  Every one of these
            // runs on EVERY settle, so the mix decides whether the ceiling is
            // an analysis gap (fixable) or the design's own top level.
            root_reason(if info.unbounded {
                0
            } else if outs.is_empty() {
                1
            } else {
                2
            });
        }
        // A side-effecting or unbounded statement poisons its whole ancestor
        // chain: no cone may contain it.
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
        // Where does the mass outside every cone sit?  For every statement parked at
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
                    "[cone_order] node count={} direct={} rescuable={} poisoned={} {}",
                    count[m], direct[m], rescuable[m], poisoned[m], inputs.node_path[m],
                );
            }
        }
        report_root_reasons(&inputs.node_path[root as usize], &inputs.comb_owner);
    }

    // -- candidates: every qualifying node.  A statement joins its DEEPEST
    //    qualifying ancestor: deep subtrees (an idle arbiter, an FPU) are the
    //    ones whose inputs actually sit still, while a huge ancestor cone
    //    would inherit the always-active core.  A shallower qualifying node
    //    still takes its RESIDUAL statements as its own slot.
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

    // -- per-span writer lists, consumed by the segment pricing
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

    // -- Small designs: keep the original order and take the maximal cone
    //    runs it already contains.  Clustering pulls its extra mass out of
    //    the schedule's ACTIVE remainder, whose fat compare sets price it out
    //    until the statement count is well past `CLUSTER_MIN_STMTS`.
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
        return Some(Schedule {
            order,
            bursts,
            infos,
            cones,
            writers,
        });
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
    // RAW + WAR per (reader, FF element span).  A combinational write into
    // an FF word is unbounded, but its readers resolve through `ff_owner`
    // and are tracked, so the chain below -- which assumes tracked
    // statements never touch untracked bytes -- leaves that pair free to
    // swap.  One such swap flips a settle back-edge and costs the whole
    // comb list an extra pass.  WAW needs no edges here: every FF writer is
    // unbounded, so the chain below already keeps them in order.
    let mut ff_spans: Vec<(usize, usize)> = infos.iter().flat_map(|i| i.out_ff.clone()).collect();
    ff_spans.sort_unstable();
    ff_spans.dedup();
    if !ff_spans.is_empty() {
        let ff_max_end = prefix_max_end(&ff_spans);
        let mut ff_writers: Vec<Vec<u32>> = vec![Vec::new(); ff_spans.len()];
        for (i, info) in infos.iter().enumerate() {
            for &(s, e) in &info.out_ff {
                let mut j = first_ff_span(&ff_max_end, s);
                while j < ff_spans.len() && ff_spans[j].0 < e {
                    if ff_spans[j].1 > s {
                        ff_writers[j].push(i as u32);
                    }
                    j += 1;
                }
            }
        }
        for (i, info) in infos.iter().enumerate() {
            for &(s, e) in &info.in_ff {
                let mut j = first_ff_span(&ff_max_end, s);
                while j < ff_spans.len() && ff_spans[j].0 < e {
                    if ff_spans[j].1 > s {
                        let w = &ff_writers[j];
                        let mut k = w.partition_point(|&x| (x as usize) < i);
                        if let Some(p) = k.checked_sub(1) {
                            add_edge(w[p], i as u32, &mut csucc, &mut cdeg);
                        }
                        // A self-reading writer orders via the untracked chain
                        // below, so skip past itself. It can appear more than
                        // once: unlike the comb side, whose owner spans are
                        // disjoint by construction, these come from each
                        // statement's MERGED out ranges, and two of one
                        // statement's ranges can reach the same span.
                        while w.get(k) == Some(&(i as u32)) {
                            k += 1;
                        }
                        if let Some(&q) = w.get(k) {
                            add_edge(i as u32, q, &mut csucc, &mut cdeg);
                        }
                    }
                    j += 1;
                }
            }
        }
    }
    // Untracked-footprint chain: statements whose reads or writes fall
    // outside the owner tables can only conflict with each other (tracked
    // statements never touch those bytes, except for the FF spans handled
    // just above), so keeping their relative order is a sufficient
    // conservative constraint.
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
    // quiet mass is not fused with its active mass.  Pinning the runs
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
    Some(Schedule {
        order,
        bursts,
        infos,
        cones,
        writers,
    })
}

/// Count the bursts of `sched` that pass the static profitability filter: a
/// burst is worth keeping contiguous only when the bytes that decide whether
/// its inputs moved are few against the statements it holds.  Also returns
/// the bytes spent and the budget they were drawn from.
pub(crate) fn select_segments(sched: &Schedule, inputs: &OwnerInputs) -> (usize, usize, usize) {
    let Schedule {
        order,
        bursts,
        infos,
        cones,
        writers,
    } = sched;
    let n = order.len();
    let budget = compare_budget(n);
    let mut in_burst = vec![false; n];
    let mut segments = 0usize;
    let mut total_bytes = 0usize;
    for &(sl, start, end) in bursts {
        if sl == 0 || end - start < MIN_SEGMENT_STMTS {
            if diag() && sl != 0 && end - start >= 8 {
                eprintln!(
                    "[cone_order] drop too-small [{start}..{end}) stmts={} {}",
                    end - start,
                    inputs.node_path[cones[sl - 1] as usize],
                );
            }
            continue;
        }
        // Outputs another writer shares: a statement outside the burst (an
        // FSM's `nxtstate = 0` default split from its conditional
        // assignments), an event or a component.
        for &oi in &order[start..end] {
            in_burst[oi as usize] = true;
        }
        let mut shared: Vec<(usize, usize)> = Vec::new();
        for &oi in &order[start..end] {
            for &(s, e) in &infos[oi as usize].out_comb {
                let mut j = first_span(&inputs.comb_owner, s);
                while j < inputs.comb_owner.len() && inputs.comb_owner[j].0 < e {
                    let (os, oe, _) = inputs.comb_owner[j];
                    if oe > s && writers[j].iter().any(|&w| !in_burst[w as usize]) {
                        shared.push((os.max(s), oe.min(e)));
                    }
                    j += 1;
                }
            }
        }
        for &oi in &order[start..end] {
            in_burst[oi as usize] = false;
        }
        merge_ranges(&mut shared);
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
        shared.extend(intersect_ranges(&inputs.event_written_comb, &out_comb));
        merge_ranges(&mut shared);
        // Bytes the burst both reads and writes carry its chain state: a read
        // can precede its writer and consume the previous settle's value.
        let backedge = subtract_ranges(&intersect_ranges(&in_comb, &out_comb), &shared);
        let mut comb_cmp = ext;
        comb_cmp.extend(backedge.iter().copied());
        merge_ranges(&mut comb_cmp);
        let comb_cmp = subtract_ranges(&comb_cmp, &shared);
        let bytes: usize = comb_cmp
            .iter()
            .chain(&in_ff)
            .chain(&shared)
            .map(|&(s, e)| e - s)
            .sum();
        // A run counts only with at least one statement per 32 input bytes.
        if (end - start) < bytes / 32 || total_bytes + bytes > budget {
            if diag() {
                eprintln!(
                    "[cone_order] drop unprofitable [{start}..{end}) stmts={} bytes={bytes} {}",
                    end - start,
                    inputs.node_path[cones[sl - 1] as usize],
                );
            }
            continue;
        }
        total_bytes += bytes;
        segments += 1;
    }
    (segments, total_bytes, budget)
}

/// The cone reorder of a comb schedule.
pub struct ConeOrder {
    /// Permutation: `order[new_index] = old_index`.
    pub order: Vec<u32>,
    /// Segments the profitability filter kept, never zero.
    pub segments: usize,
}

/// The reordered schedule and the number of segments that justified it.
/// `None` keeps the incoming order.
pub fn cone_order(stmts: &[ProtoStatement], inputs: &OwnerInputs) -> Option<ConeOrder> {
    let sched = schedule(stmts, inputs)?;
    let (segments, total_bytes, budget) = select_segments(&sched, inputs);
    if diag() {
        eprintln!(
            "[cone_order] order: stmts={} cones={} segments={segments} compare_bytes={total_bytes}/{budget}",
            sched.order.len(),
            sched.cones.len(),
        );
    }
    if segments == 0 {
        return None;
    }
    Some(ConeOrder {
        order: sched.order,
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::opt::owners::FfOwner;

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
        let inputs = OwnerInputs {
            node_path: (0..node_parent.len()).map(|n| format!("n{n}")).collect(),
            node_parent,
            comb_owner,
            ff_owner: FfOwner { runs: vec![] },
            comb_var: Vec::new(),
            ff_node: Vec::new(),
            event_written_comb: vec![],
            ff_next_alias: Vec::new(),
            tour: Default::default(),
        };
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
    fn cone_order_clusters_each_qualifying_subtree() {
        // The re-sort pulls each interleaved chain together.
        let (inputs, stmts) = chain_design(3, 1_700, true);
        let o = cone_order(&stmts, &inputs).unwrap();
        assert_eq!(o.segments, 3);
        assert!(o.order.windows(2).take(1_699).all(|w| w[1] == w[0] + 3));
        // Chains already in place keep their order.
        let (inputs, stmts) = chain_design(3, 400, false);
        let o = cone_order(&stmts, &inputs).unwrap();
        assert_eq!(o.segments, 3);
        assert!(o.order.iter().enumerate().all(|(i, &x)| i == x as usize));
        // Too little to pay for a compare: the incoming order stays.
        let (inputs, stmts) = chain_design(2, 100, false);
        assert!(cone_order(&stmts, &inputs).is_none());
    }

    #[test]
    fn the_compare_budget_tracks_the_design_size() {
        // Under an absolute cap, replicating a design — a second core — leaves
        // the copy unclustered while the original stays clustered.
        assert_eq!(compare_budget(2 * 36_032), 2 * compare_budget(36_032));
        // And it must stay under what the per-segment rule already implies:
        // every kept segment brings a statement per 32 compare bytes, so the
        // sum can never be worth more than `32 * stmts`.
        for stmts in [1usize, 300, 4_324, 36_032, 1 << 20] {
            assert!(compare_budget(stmts) < stmts.saturating_mul(32));
        }
        // A design large enough to overflow the product saturates rather than
        // wrapping into a tiny budget.
        assert_eq!(compare_budget(usize::MAX), usize::MAX);
    }
}
