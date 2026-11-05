//! Storage ownership of the module-instance tree: which instance owns each
//! comb span and FF element, the instance tree itself, and the range algebra
//! the planners that consume them share.

use crate::HashMap;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::{ModuleVariableMeta, VariableElement, VariableMeta, value_size};
use veryl_analyzer::ir::VarId;

/// FF-buffer ownership, held per uniformly strided RUN of elements rather
/// than per element: memory arrays make the element count grow with the
/// modelled depth while the array count does not.
#[derive(Clone, Default)]
pub struct FfOwner {
    /// `(start, stride, count, bytes)`, sorted by `start` and disjoint.  The
    /// run covers `[start, start + count * stride)`, of which each element
    /// occupies the first `bytes`.
    pub(crate) runs: Vec<(usize, usize, usize, usize)>,
}

impl FfOwner {
    /// The element span covering `x`.  `None` also when `x` lands in the
    /// padding a stride wider than the element leaves behind each one.
    pub(crate) fn span(&self, x: usize) -> Option<(usize, usize)> {
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

/// `a` is an ancestor of `m` exactly when `tin[a] <= tin[m] < tout[a]`, so
/// the ancestry test the gate planners run per statement costs two
/// comparisons instead of a walk to the root.
#[derive(Clone, Default)]
pub struct Tour {
    tin: Vec<u32>,
    tout: Vec<u32>,
    depth: Vec<u32>,
}

impl Tour {
    /// `build_inputs` pushes a node only after its parent, so `parent[m] < m`
    /// and plain scans suffice, with no tree walk.
    fn of(parent: &[u32]) -> Tour {
        let n = parent.len();
        let (mut tin, mut tout) = (vec![0u32; n], vec![0u32; n]);
        let mut depth = vec![0u32; n];
        let mut size = vec![1u32; n];
        for m in (1..n).rev() {
            debug_assert!(
                (parent[m] as usize) < m,
                "build_inputs numbers an instance node after its parent"
            );
            size[parent[m] as usize] += size[m];
        }
        // Where the next child of `m` starts.
        let mut next = vec![0u32; n];
        for m in 0..n {
            if m > 0 {
                let p = parent[m] as usize;
                depth[m] = depth[p] + 1;
                tin[m] = next[p];
                next[p] += size[m];
            }
            next[m] = tin[m] + 1;
            tout[m] = tin[m] + size[m];
        }
        Tour { tin, tout, depth }
    }

    pub fn entry(&self, m: u32) -> u32 {
        self.tin[m as usize]
    }

    /// One past the last entry number under `m`.
    pub fn exit(&self, m: u32) -> u32 {
        self.tout[m as usize]
    }

    pub fn is_desc(&self, m: u32, a: u32) -> bool {
        let (i, lo) = (self.tin[m as usize], self.tin[a as usize]);
        lo <= i && i < self.tout[a as usize]
    }

    pub fn depth(&self, m: u32) -> u32 {
        self.depth[m as usize]
    }
}

/// Node tables of the module-instance tree plus storage-ownership intervals,
/// prepared by the caller (`ProtoModule::conv`) from `ModuleVariableMeta`.
#[derive(Clone)]
pub struct OwnerInputs {
    /// Parent per node; the root's parent is `u32::MAX`.
    pub node_parent: Vec<u32>,
    /// Hierarchical path per node (diagnostics).
    pub node_path: Vec<String>,
    /// Sorted disjoint-start `(start, end, node)` comb-buffer intervals.
    pub comb_owner: Vec<(usize, usize, u32)>,
    pub ff_owner: FfOwner,
    /// Sorted disjoint `(start, end, node)` FF-buffer intervals, one per
    /// variable: which instance an FF belongs to (the event gates attribute
    /// an `always_ff` to its instance by the FFs it writes).
    pub ff_node: Vec<(usize, usize, u32)>,
    /// The comb-buffer counterpart, one interval per variable over ALL its
    /// elements: `comb_owner` leaves the padding between a padded array's
    /// elements to nobody, and the event gates need every read covered.
    pub comb_var: Vec<(usize, usize, u32)>,
    /// Merged comb byte ranges any event statement can write.
    pub event_written_comb: Vec<(usize, usize)>,
    /// `(next slot, current slot)` of every dual-slot FF element: an event
    /// names the next slot, the readers the current one.
    pub ff_next_alias: Vec<(usize, usize)>,
    /// Construct it empty: it derives from `node_parent` and nothing else.
    /// Living here means the re-conversions of one DUT share it, since they
    /// take these inputs from the pipeline cache.
    pub(crate) tour: std::sync::OnceLock<Tour>,
}

/// `VERYL_CONE_ORDER_DIAG=1` reports how the owner tables and the cone
/// reorder came out.
pub(crate) fn diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_CONE_ORDER_DIAG").as_deref() == Ok("1"))
}

/// Above this, a variable is owned as ONE span instead of one per element: the
/// `cone_order` segment test prices a run reading this many bytes as
/// unprofitable unless it holds thousands of statements, so per-element spans
/// buy precision that is almost never used.  Absolute on purpose: this caps
/// ELABORATION cost, which grows with the modelled depth, not with the logic.
pub(crate) const MAX_ELEMENT_OWNER_BYTES: usize = 256 << 10;

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
/// The bytes are what the `cone_order` segment test prices.  The span is what
/// [`crate::ir::big_array::BigArrayFold`] weighs, and `cone_order` reads
/// offsets through that fold, so a variable the fold collapses has to collapse
/// here too.
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

/// Give each function-local comb copy and rename temp the owner of the storage
/// it stands in for (`Context::comb_reloc`): the copy is private to the same
/// instance, so the original's owner is exactly right, and without one the
/// statement reads as `unbounded` and pins itself to the root.  `comb_owner`
/// must be sorted on entry; an unowned original adds nothing.  Entries chain
/// (a temp renaming another temp), so passes repeat until none resolves.
fn inherit_reloc_owners(
    comb_owner: &mut Vec<(usize, usize, u32)>,
    comb_reloc: &[(isize, isize, usize)],
) {
    let mut pending: Vec<(usize, usize, usize)> = comb_reloc
        .iter()
        .filter(|&&(old, new, bytes)| old >= 0 && new >= 0 && bytes > 0)
        .map(|&(old, new, bytes)| (old as usize, new as usize, bytes))
        .collect();
    loop {
        let mut extra: Vec<(usize, usize, u32)> = Vec::new();
        pending.retain(|&(old, new, bytes)| {
            let i = comb_owner.partition_point(|&(s, _, _)| s <= old);
            match i.checked_sub(1) {
                Some(i) if comb_owner[i].0 <= old && old < comb_owner[i].1 => {
                    extra.push((new, new + bytes, comb_owner[i].2));
                    false
                }
                _ => true,
            }
        });
        if extra.is_empty() {
            return;
        }
        comb_owner.extend(extra);
        radix_sort_intervals(comb_owner);
    }
}

impl OwnerInputs {
    pub fn tour(&self) -> &Tour {
        self.tour.get_or_init(|| Tour::of(&self.node_parent))
    }

    pub fn parent(&self, m: u32) -> Option<u32> {
        let p = self.node_parent[m as usize];
        (p != u32::MAX).then_some(p)
    }

    pub fn lca(&self, a: u32, b: u32) -> u32 {
        let tour = self.tour();
        let mut a = a;
        while !tour.is_desc(b, a) {
            a = self
                .parent(a)
                .expect("the root is an ancestor of every node");
        }
        a
    }

    /// Follow a comb relayout: the comb intervals move with the units the
    /// schedule placed, one becoming several where its bytes landed apart.
    /// FF storage is never relaid.  A byte the schedule leaves unplaced is
    /// storage no statement references, and keeping it at its old offset
    /// would claim whatever unit now sits there.
    pub fn relayout(&mut self, sched: &crate::ir::comb_layout::CombLayoutSchedule) {
        let valid = |v: Vec<(isize, isize)>| {
            v.into_iter()
                .filter(|&(ns, ne)| ns >= 0 && ne > ns)
                .map(|(ns, ne)| (ns as usize, ne as usize))
        };
        for owned in [&mut self.comb_owner, &mut self.comb_var] {
            let mut out = Vec::with_capacity(owned.len());
            for &(s, e, id) in owned.iter() {
                out.extend(
                    valid(sched.placed_range(s as isize, e as isize)).map(|(ns, ne)| (ns, ne, id)),
                );
            }
            out.sort_unstable();
            *owned = out;
        }
        let mut written = Vec::with_capacity(self.event_written_comb.len());
        for &(s, e) in &self.event_written_comb {
            written.extend(valid(sched.placed_range(s as isize, e as isize)));
        }
        merge_ranges(&mut written);
        self.event_written_comb = written;
    }

    /// Own comb storage allocated after `build_inputs`, given as
    /// `(from, to, bytes)` entries in the `Context::comb_reloc` shape, as the
    /// owner of `from`.
    pub fn adopt_relocations(&mut self, reloc: &[(isize, isize, usize)]) {
        let before = self.comb_owner.len();
        inherit_reloc_owners(&mut self.comb_owner, reloc);
        inherit_reloc_owners(&mut self.comb_var, reloc);
        if diag() {
            eprintln!(
                "[cone_order] adopt_relocations: entries={} owned={}",
                reloc.len(),
                self.comb_owner.len() - before,
            );
        }
    }
}

/// Assemble the node tables from the pre-instantiation variable-meta tree.
/// `event_written` holds every comb offset the event statements can write
/// (element-expanded); it becomes `OwnerInputs::event_written_comb`.
pub fn build_inputs(
    top_name: &str,
    top_vars: &HashMap<VarId, VariableMeta>,
    children: &[ModuleVariableMeta],
    event_written: &crate::HashSet<isize>,
    comb_reloc: &[(isize, isize, usize)],
    use_4state: bool,
) -> OwnerInputs {
    let mut node_parent: Vec<u32> = vec![u32::MAX];
    let mut node_path: Vec<String> = vec![top_name.to_string()];
    let mut comb_owner: Vec<(usize, usize, u32)> = Vec::new();
    let mut ff_runs: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut ff_elems: Vec<(usize, usize)> = Vec::new();
    let mut ff_node: Vec<(usize, usize, u32)> = Vec::new();
    let mut comb_var: Vec<(usize, usize, u32)> = Vec::new();
    let mut ff_next_alias: Vec<(usize, usize)> = Vec::new();
    let add_vars = |vars: &HashMap<VarId, VariableMeta>,
                    id: u32,
                    comb_owner: &mut Vec<(usize, usize, u32)>,
                    ff_runs: &mut Vec<(usize, usize, usize, usize)>,
                    ff_elems: &mut Vec<(usize, usize)>,
                    ff_node: &mut Vec<(usize, usize, u32)>,
                    comb_var: &mut Vec<(usize, usize, u32)>,
                    ff_next_alias: &mut Vec<(usize, usize)>| {
        for vm in vars.values() {
            ff_elems.clear();
            // A variable past `MAX_ELEMENT_OWNER_BYTES` is priced out of almost
            // every segment, so per-element owners cost a span, a sort key and
            // a writer list each for a precision hardly ever used.  One span
            // keeps the lookup exact and makes the table a function of the
            // design's variables rather than its array lengths.
            let (comb_bytes, comb_span_bytes) = comb_extent(&vm.elements, use_4state);
            let fold_var = comb_bytes > MAX_ELEMENT_OWNER_BYTES
                || comb_span_bytes > crate::ir::big_array::FOLD_SPAN_BYTES;
            let mut comb_span: Option<(usize, usize)> = None;
            let mut ff_ext: Option<(usize, usize)> = None;
            let mut comb_ext: Option<(usize, usize)> = None;
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
                    if el.next_offset >= 0 && el.next_offset as usize != off {
                        ff_next_alias.push((el.next_offset as usize, off));
                    }
                    // A dual-slot FF is written at its next slot; the owner
                    // span covers both so a write finds its instance.
                    let end = (el.next_offset.max(off as isize) as usize) + nb;
                    ff_ext = Some(ff_ext.map_or((off, end), |(s, e): (usize, usize)| {
                        (s.min(off), e.max(end))
                    }));
                } else if fold_var {
                    comb_span = Some(match comb_span {
                        None => (off, off + nb),
                        Some((s, e)) => (s.min(off), e.max(off + nb)),
                    });
                } else {
                    comb_owner.push((off, off + nb, id));
                }
                if !el.current.is_ff() {
                    comb_ext = Some(comb_ext.map_or((off, off + nb), |(s, e): (usize, usize)| {
                        (s.min(off), e.max(off + nb))
                    }));
                }
            }
            if let Some((s, e)) = comb_span {
                comb_owner.push((s, e, id));
            }
            if let Some((s, e)) = ff_ext {
                ff_node.push((s, e, id));
            }
            if let Some((s, e)) = comb_ext {
                comb_var.push((s, e, id));
            }
            fold_ff_runs(ff_elems, ff_runs);
        }
    };
    add_vars(
        top_vars,
        0,
        &mut comb_owner,
        &mut ff_runs,
        &mut ff_elems,
        &mut ff_node,
        &mut comb_var,
        &mut ff_next_alias,
    );
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
            &mut ff_node,
            &mut comb_var,
            &mut ff_next_alias,
        );
        for c in &m.children {
            stack.push((id, c));
        }
    }
    radix_sort_intervals(&mut comb_owner);
    let before = comb_owner.len();
    inherit_reloc_owners(&mut comb_owner, comb_reloc);
    comb_var.sort_unstable();
    inherit_reloc_owners(&mut comb_var, comb_reloc);
    if diag() {
        eprintln!(
            "[cone_order] build_inputs: reloc entries={} owned={} comb_extent={}",
            comb_reloc.len(),
            comb_owner.len() - before,
            comb_owner.last().map_or(0, |&(_, e, _)| e),
        );
    }
    ff_runs.sort_unstable();
    ff_runs.dedup();
    ff_node.sort_unstable();
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
    ff_next_alias.sort_unstable();
    OwnerInputs {
        node_parent,
        node_path,
        comb_owner,
        ff_owner: FfOwner { runs: ff_runs },
        ff_node,
        comb_var,
        event_written_comb,
        ff_next_alias,
        tour: std::sync::OnceLock::new(),
    }
}

/// Does the statement (recursively) have effects a skip could lose?
pub(crate) fn has_side_effects(s: &ProtoStatement) -> bool {
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
        ProtoStatement::HierAssign(_) => {
            unreachable!("hierarchical assignment is resolved by resolve_hier_refs")
        }
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
pub(crate) fn first_span(owner: &[(usize, usize, u32)], s: usize) -> usize {
    let mut j = owner.partition_point(|&(cs, _, _)| cs <= s);
    while j > 0 && owner[j - 1].1 > s {
        j -= 1;
    }
    j
}

/// `first_span` over the sorted FF element spans of the hazard graph, given
/// the prefix maxima of their ends.
///
/// The walk `first_span` does cannot be reused: the owner entries it scans
/// are equal or disjoint, whereas these come from each statement's MERGED
/// out ranges and can nest one inside another.  A walk stops at the first
/// span that does not reach `s` and leaves everything under it unvisited.
/// The prefix maxima are non-decreasing, so the first one past `s` is the
/// earliest span that can still reach it.
pub(crate) fn first_ff_span(max_end: &[usize], s: usize) -> usize {
    max_end.partition_point(|&e| e <= s)
}

/// Largest end among each prefix of `spans`, which must be sorted.
pub(crate) fn prefix_max_end(spans: &[(usize, usize)]) -> Vec<usize> {
    let mut run = 0;
    spans
        .iter()
        .map(|&(_, e)| {
            run = run.max(e);
            run
        })
        .collect()
}

pub(crate) fn merge_ranges(v: &mut Vec<(usize, usize)>) {
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

pub(crate) fn subtract_ranges(a: &[(usize, usize)], b: &[(usize, usize)]) -> Vec<(usize, usize)> {
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

pub(crate) fn intersect_ranges(a: &[(usize, usize)], b: &[(usize, usize)]) -> Vec<(usize, usize)> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::variable::VarOffset;

    #[test]
    fn a_relayout_drops_the_owners_of_unplaced_storage() {
        // Node 1's variable at [0, 8) moves to [8, 16); node 2's at [16, 24)
        // moves to [0, 8).  The dead temp at [8, 16), adopted by node 3, is
        // placed nowhere and must not claim node 1's new bytes.
        let mut ci = OwnerInputs {
            node_parent: vec![u32::MAX, 0, 0, 0],
            node_path: vec![String::new(); 4],
            comb_owner: vec![(0, 8, 1), (8, 16, 3), (16, 24, 2)],
            ff_owner: FfOwner { runs: Vec::new() },
            ff_node: Vec::new(),
            comb_var: vec![(0, 8, 1), (8, 16, 3), (16, 24, 2)],
            event_written_comb: vec![(0, 24)],
            ff_next_alias: Vec::new(),
            tour: std::sync::OnceLock::new(),
        };
        let sched = crate::ir::comb_layout::CombLayoutSchedule::from_units(
            vec![(0, 8, 8), (16, 24, 0)],
            16,
        );
        ci.relayout(&sched);
        assert_eq!(ci.comb_owner, vec![(0, 8, 2), (8, 16, 1)]);
        assert_eq!(ci.comb_var, vec![(0, 8, 2), (8, 16, 1)]);
        assert_eq!(ci.event_written_comb, vec![(0, 16)]);
    }

    #[test]
    fn reloc_owners_follow_temp_chains() {
        // Node 1 owns [0, 8); the temp at 8 stands in for it and the temp at
        // 16 for that temp, listed before its parent resolves.  An entry whose
        // original nobody owns adds nothing.
        let mut owner = vec![(0usize, 8usize, 1u32)];
        inherit_reloc_owners(&mut owner, &[(8, 16, 8), (0, 8, 8), (100, 24, 8)]);
        assert_eq!(owner, vec![(0, 8, 1), (8, 16, 1), (16, 24, 1)]);
    }

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
        let just_over = arr(MAX_ELEMENT_OWNER_BYTES / 8 + 1);
        let just_under = arr(MAX_ELEMENT_OWNER_BYTES / 8);
        assert!(comb_extent(&just_over, false).0 > MAX_ELEMENT_OWNER_BYTES);
        assert!(comb_extent(&just_under, false).0 <= MAX_ELEMENT_OWNER_BYTES);
        // A padded array can span past the fold's threshold while its elements
        // stay under the budget; those spans have to collapse anyway, because
        // `cone_order` reads such an array folded.
        let padded: Vec<VariableElement> = (0..64)
            .map(|i| {
                el(
                    8,
                    VarOffset::Comb((i * crate::ir::big_array::FOLD_SPAN_BYTES / 32) as isize),
                )
            })
            .collect();
        let (bytes, span) = comb_extent(&padded, false);
        assert!(bytes <= MAX_ELEMENT_OWNER_BYTES);
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
    fn a_relocated_function_local_inherits_its_original_owner() {
        // A per-call-site copy has no `VariableMeta`; left unowned it reads as
        // `unbounded` and pins its statement to the root.
        let mut owner = vec![(0usize, 8usize, 1u32), (8, 16, 2)];
        inherit_reloc_owners(
            &mut owner,
            &[
                (0, 100, 8),   // copy of node 1's scratch
                (8, 108, 8),   // copy of node 2's scratch
                (900, 116, 8), // original itself unowned: stays out
                (0, 200, 0),   // zero-length: nothing to own
                (-1, 208, 8),  // no original offset at all
            ],
        );
        assert_eq!(
            owner,
            vec![(0, 8, 1), (8, 16, 2), (100, 108, 1), (108, 116, 2)],
            "each copy takes its original's node; an unowned original adds nothing"
        );
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

    #[test]
    fn first_ff_span_starts_before_a_span_that_nests_a_later_one() {
        // A statement's merged out ranges fuse adjacent elements, so one
        // entry can contain another's.  Starting at the first entry whose
        // START is past the byte stops at the nested span and never reaches
        // the one that covers it, leaving a reader bound to no writer.
        let spans = vec![(0usize, 20usize), (1, 2), (5, 8)];
        let max_end = prefix_max_end(&spans);
        assert_eq!(first_ff_span(&max_end, 6), 0);
        assert_eq!(first_ff_span(&max_end, 18), 0);

        // Equal spans, the shape aliased storage gives the comb side.
        let spans = vec![(0usize, 8usize), (8, 16), (16, 24)];
        let max_end = prefix_max_end(&spans);
        assert_eq!(first_ff_span(&max_end, 8), 1);
        // A byte past every entry yields an empty scan rather than a panic.
        assert_eq!(first_ff_span(&max_end, 24), 3);
    }
}
