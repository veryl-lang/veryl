//! Settle-order in-place relayout of the comb value space (`VERYL_COMB_LAYOUT`).
//!
//! The default allocator hands out comb offsets in declaration order, which
//! interleaves the slots the settle sweep actually touches with cold storage
//! (testbench scratch, debug-only nets).  On designs whose live set exceeds
//! L1d the sweep then walks the whole extent in a data-dependent order the
//! prefetcher cannot follow.  This pass permutes the storage units WITHIN the
//! existing bump space: units are ordered by their first use in the settled
//! statement order (hot first, cold last), and the statements plus every
//! offset-bearing side structure are rewritten through the resulting
//! translation.  No probe build, no address-space growth — the pass runs once
//! inside `run_comb_pipeline` (after DCE, before the JIT consumes the
//! statements) and its result is memoised with the pipeline.
//!
//! A unit is normally one variable's contiguous comb span (stride preserved,
//! so dynamic indexing keeps working).  Two shapes force coarser units:
//!
//! - `CompiledBlock`s bake absolute offsets into machine code and can only be
//!   shifted by their uniform `comb_delta_bytes`, so every comb byte a block
//!   touches merges into one rigid unit (`cb_comb_span`).  Blocks cloned
//!   across instances by a single delta (`try_compile_inst_chunks`,
//!   `GLOBAL_STMT_CACHE`) stay sound for the same reason.
//! - Offsets that appear in statements but in no variable's meta (function
//!   per-call-site body copies allocate straight from `comb_total_bytes`) are
//!   promoted to units extending to the next known boundary, so no live byte
//!   is ever treated as dead space.
//!
//! Bytes belonging to no unit and referenced nowhere (alignment padding) are
//! genuinely dead: they are NOT placed, so the permuted layout packs tighter
//! than the original.  Space past `buffer_end` up to the old total stays
//! usable by later allocations (`cond_hoist_transform` bumps from
//! `comb_total_bytes`, which the caller keeps at least as large as
//! `buffer_end`).

use crate::HashMap;
use crate::ir::event::Event;
use crate::ir::expression::ProtoExpression;
use crate::ir::statement::{CompiledBlockStatement, ProtoStatement};
use crate::ir::variable::{ModuleVariableMeta, VarOffset, VariableMeta, value_size};
use veryl_analyzer::ir::VarId;

/// Old-offset → new-offset translation for the comb space, as a sorted set of
/// relocated intervals.  Anything outside every interval stays in place.
#[derive(Debug, Default)]
pub struct CombLayoutSchedule {
    /// `(old_start, old_end, new_start)`, sorted by `old_start`, pairwise
    /// disjoint.  A unit keeps its internal layout: `x` in `[old_start,
    /// old_end)` lands at `new_start + (x - old_start)`.
    units: Vec<(isize, isize, isize)>,
    /// One past the highest byte any placed unit occupies.  The comb buffer
    /// must be at least this large (it never exceeds the old total by more
    /// than alignment padding, and usually undercuts it).
    pub buffer_end: usize,
    /// Diagnostics: units given a first-use rank / total placed units.
    pub hot_units: usize,
    pub total_units: usize,
}

impl CombLayoutSchedule {
    /// Translate a comb byte offset through the schedule.
    pub fn translate(&self, x: isize) -> isize {
        match self.units.binary_search_by(|&(s, _, _)| s.cmp(&x)) {
            Ok(i) => self.units[i].2,
            Err(0) => x,
            Err(i) => {
                let (s, e, n) = self.units[i - 1];
                if x < e { n + (x - s) } else { x }
            }
        }
    }

    /// Translate a typed offset; FF offsets pass through untouched.
    pub fn translate_off(&self, off: VarOffset) -> VarOffset {
        match off {
            VarOffset::Ff(_) => off,
            VarOffset::Comb(o) => VarOffset::Comb(self.translate(o)),
        }
    }
}

/// Everything the caller must gather BEFORE the pipeline for
/// `build_schedule`: the per-variable spans (the pipeline never sees the meta
/// tree), the offsets referenced outside any statement list, and the bump
/// total the units must fit in.
pub struct LayoutInputs {
    pub meta_units: Vec<(isize, isize)>,
    pub extra_offsets: Vec<VarOffset>,
    pub comb_total: usize,
}

/// `VERYL_COMB_LAYOUT=1` opt-in.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_COMB_LAYOUT").as_deref() == Ok("1"))
}

fn diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_COMB_LAYOUT_DIAG").as_deref() == Ok("1"))
}

/// Collect per-variable comb spans from one meta map: each variable's comb
/// elements are allocated contiguously, so its span moves as one unit and the
/// dynamic-index stride survives.  Port-aliased children share parent offsets;
/// the overlap merge in `build_schedule` collapses those into one unit.
pub fn collect_meta_units_map(
    metas: &HashMap<VarId, VariableMeta>,
    use_4state: bool,
    out: &mut Vec<(isize, isize)>,
) {
    for meta in metas.values() {
        let mut start: Option<isize> = None;
        let mut end: isize = 0;
        for el in &meta.elements {
            if let VarOffset::Comb(o) = el.current {
                let e = o + value_size(el.native_bytes, use_4state) as isize;
                start = Some(start.map_or(o, |s: isize| s.min(o)));
                end = end.max(e);
            }
        }
        if let Some(s) = start {
            out.push((s, end));
        }
    }
}

/// `collect_meta_units_map` over a whole child-module tree.
pub fn collect_meta_units_tree(
    node: &ModuleVariableMeta,
    use_4state: bool,
    out: &mut Vec<(isize, isize)>,
) {
    collect_meta_units_map(&node.variable_meta, use_4state, out);
    for child in &node.children {
        collect_meta_units_tree(child, use_4state, out);
    }
}

/// Expand every top-level `CompiledBlock` back to its pre-JIT originals
/// (inst-chunk CBs replace whole statement lists, so top level is exhaustive).
///
/// A CB's baked artifact freezes its entire touched span into one rigid
/// unit.  The expanded originals remap freely; the normal chunk compilation
/// downstream (the pipeline's `try_jit_no_cache` for comb, the caller's
/// `try_jit` for events) rebuilds artifacts from the remapped statements,
/// trading one redundant inst-chunk compile per process for a
/// variable-granular schedule.
pub fn expand_compiled_blocks(stmts: &mut Vec<ProtoStatement>) {
    if !stmts
        .iter()
        .any(|s| matches!(s, ProtoStatement::CompiledBlock(_)))
    {
        return;
    }
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts.drain(..) {
        match s {
            ProtoStatement::CompiledBlock(cb) => out.extend(cb.original_stmts),
            other => out.push(other),
        }
    }
    *stmts = out;
}

/// The comb byte span a `CompiledBlock` touches (side tables + pre-JIT
/// originals).  The end is exclusive-by-one-byte: it only needs to overlap the
/// last touched variable's unit for the merge to make the whole extent rigid.
fn cb_comb_span(cb: &CompiledBlockStatement) -> Option<(isize, isize)> {
    let mut lo = isize::MAX;
    let mut hi = isize::MIN;
    let mut visit = |off: &VarOffset| {
        if let VarOffset::Comb(o) = off {
            lo = lo.min(*o);
            hi = hi.max(*o);
        }
    };
    for off in cb.input_offsets.iter().chain(cb.output_offsets.iter()) {
        visit(off);
    }
    let mut ins = vec![];
    let mut outs = vec![];
    for s in &cb.original_stmts {
        ins.clear();
        outs.clear();
        s.gather_variable_offsets(&mut ins, &mut outs);
        for off in ins.iter().chain(outs.iter()) {
            visit(off);
        }
    }
    if lo == isize::MAX {
        None
    } else {
        Some((lo, hi + 1))
    }
}

fn collect_cb_units(stmts: &[ProtoStatement], out: &mut Vec<(isize, isize)>) {
    for s in stmts {
        match s {
            ProtoStatement::CompiledBlock(cb) => {
                if let Some(span) = cb_comb_span(cb) {
                    out.push(span);
                }
            }
            ProtoStatement::SequentialBlock(body) => collect_cb_units(body, out),
            _ => {}
        }
    }
}

/// Whole extents the compressed offset gather does not spell out: a
/// runtime-indexed access reads `translate(base) + stride*i` and so assumes
/// the allocation stays contiguous — its full extent must be ONE unit.  For
/// loop counters are collected too (no gather form mentions them, but the
/// loop driver writes them every iteration).
fn collect_dynamic_spans(stmts: &[ProtoStatement], out: &mut Vec<(isize, isize)>) {
    fn expr(e: &ProtoExpression, out: &mut Vec<(isize, isize)>) {
        match e {
            ProtoExpression::DynamicVariable {
                base_offset,
                stride,
                num_elements,
                element_native_bytes,
                index_expr,
                ..
            } => {
                if let VarOffset::Comb(o) = base_offset {
                    let span = stride.unsigned_abs() * num_elements.saturating_sub(1)
                        + element_native_bytes;
                    let base = if *stride < 0 {
                        *o + *stride * (num_elements.saturating_sub(1) as isize)
                    } else {
                        *o
                    };
                    out.push((base, base + span as isize));
                }
                expr(index_expr, out);
            }
            ProtoExpression::Variable {
                var_offset,
                dynamic_select,
                var_full_width,
                width,
                ..
            } => {
                if let Some(ds) = dynamic_select {
                    if let VarOffset::Comb(o) = var_offset {
                        let full = (*var_full_width).max(*width);
                        out.push((*o, *o + crate::ir::native_bytes(full).max(1) as isize));
                    }
                    expr(&ds.index_expr, out);
                }
            }
            ProtoExpression::HierVariable(_) | ProtoExpression::Value { .. } => {}
            ProtoExpression::Unary { x, .. } => expr(x, out),
            ProtoExpression::Binary { x, y, .. } => {
                expr(x, out);
                expr(y, out);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (e, _, _) in elements {
                    expr(e, out);
                }
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                expr(cond, out);
                expr(true_expr, out);
                expr(false_expr, out);
            }
        }
    }
    for s in stmts {
        match s {
            ProtoStatement::Assign(a) => {
                if let (VarOffset::Comb(o), Some(_)) = (a.dst, &a.dynamic_select) {
                    out.push((o, o + crate::ir::native_bytes(a.dst_width).max(1) as isize));
                }
                if let Some(ds) = &a.dynamic_select {
                    expr(&ds.index_expr, out);
                }
                expr(&a.expr, out);
            }
            ProtoStatement::AssignDynamic(a) => {
                if let VarOffset::Comb(o) = a.dst_base {
                    let span = a.dst_stride.unsigned_abs() * a.dst_num_elements.saturating_sub(1)
                        + crate::ir::native_bytes(a.dst_width).max(1);
                    let base = if a.dst_stride < 0 {
                        o + a.dst_stride * (a.dst_num_elements.saturating_sub(1) as isize)
                    } else {
                        o
                    };
                    out.push((base, base + span as isize));
                }
                expr(&a.dst_index_expr, out);
                expr(&a.expr, out);
                if let Some(ds) = &a.dynamic_select {
                    expr(&ds.index_expr, out);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(c) = &x.cond {
                    expr(c, out);
                }
                collect_dynamic_spans(&x.true_side, out);
                collect_dynamic_spans(&x.false_side, out);
            }
            ProtoStatement::Case(x) => {
                for arm in &x.arms {
                    expr(&arm.cond, out);
                    collect_dynamic_spans(&arm.body, out);
                }
                collect_dynamic_spans(&x.default, out);
            }
            ProtoStatement::For(x) => {
                if let VarOffset::Comb(o) = x.var_offset {
                    out.push((o, o + 8));
                }
                collect_dynamic_spans(&x.body, out);
            }
            ProtoStatement::SequentialBlock(b) => collect_dynamic_spans(b, out),
            ProtoStatement::CompiledBlock(cb) => collect_dynamic_spans(&cb.original_stmts, out),
            _ => {}
        }
    }
}

/// One relocatable region of the old comb space.
struct Unit {
    start: isize,
    end: isize,
    /// First-use index in the settle order; cold units sort last.
    rank: usize,
}

/// Events in a deterministic order (the schedule must not depend on hash-map
/// iteration).
fn sorted_events(
    events: &HashMap<Event, Vec<ProtoStatement>>,
) -> Vec<(&Event, &Vec<ProtoStatement>)> {
    let mut evs: Vec<_> = events.iter().collect();
    evs.sort_by_key(|(ev, _)| format!("{ev:?}"));
    evs
}

/// Derive the relayout from the settled statement order.
///
/// - `meta_units`: per-variable comb spans (top map + child tree), collected
///   by the caller before the pipeline consumed anything.
/// - `unified`: post-sort post-DCE comb statements in execution order.
/// - `events`: post-DCE event statements (order made deterministic here).
/// - `extra_offsets`: comb offsets referenced outside any statement list
///   (external-component connect exprs, nested derived-clock candidates) —
///   they contribute no rank but must not be treated as dead space.
/// - `comb_total`: the bump total; every unit must lie inside it.
///
/// Returns `None` when there is nothing to move.
pub fn build_schedule(
    meta_units: &[(isize, isize)],
    unified: &[ProtoStatement],
    events: &HashMap<Event, Vec<ProtoStatement>>,
    extra_offsets: &[VarOffset],
    comb_total: usize,
) -> Option<CombLayoutSchedule> {
    if comb_total == 0 {
        return None;
    }
    let evs = sorted_events(events);

    // -- 1. seed units: variable spans + rigid CompiledBlock spans.
    let mut spans: Vec<(isize, isize)> =
        meta_units.iter().copied().filter(|&(s, e)| e > s).collect();
    let meta_span_count = spans.len();
    collect_cb_units(unified, &mut spans);
    let comb_cb_count = spans.len() - meta_span_count;
    for (_, stmts) in &evs {
        collect_cb_units(stmts, &mut spans);
    }
    collect_dynamic_spans(unified, &mut spans);
    for (_, stmts) in &evs {
        collect_dynamic_spans(stmts, &mut spans);
    }
    spans.retain(|&(s, e)| e > s);
    if diag() {
        let cb_spans = &spans[meta_span_count..];
        let widest = cb_spans.iter().map(|&(s, e)| e - s).max().unwrap_or(0);
        eprintln!(
            "[comb_layout] meta_spans={} cb_spans={} (comb {}) widest_cb_span={}",
            meta_span_count,
            cb_spans.len(),
            comb_cb_count,
            widest,
        );
    }

    // -- 2. sort + overlap merge (overlapping storage must move together:
    //    port aliasing makes meta many-to-one, CB spans swallow whole
    //    subtrees).
    spans.sort_by_key(|&(s, e)| (s, std::cmp::Reverse(e)));
    let mut units: Vec<Unit> = Vec::with_capacity(spans.len());
    for (s, e) in spans {
        match units.last_mut() {
            Some(last) if s < last.end => {
                last.end = last.end.max(e);
            }
            _ => units.push(Unit {
                start: s,
                end: e,
                rank: usize::MAX,
            }),
        }
    }

    // -- 3. first sweep: find offsets covered by no unit (function body
    //    per-call-site copies allocate outside any variable meta).  They are
    //    promoted to units below so nothing live is treated as dead space.
    let locate = |units: &[Unit], o: isize| -> Option<usize> {
        match units.binary_search_by(|u| u.start.cmp(&o)) {
            Ok(i) => Some(i),
            Err(0) => None,
            Err(i) => (o < units[i - 1].end).then_some(i - 1),
        }
    };
    let mut orphans: std::collections::BTreeSet<isize> = std::collections::BTreeSet::new();
    {
        let mut ins: Vec<VarOffset> = Vec::new();
        let mut outs: Vec<VarOffset> = Vec::new();
        let mut sweep = |stmts: &[ProtoStatement],
                         orphans: &mut std::collections::BTreeSet<isize>| {
            for s in stmts {
                ins.clear();
                outs.clear();
                s.gather_variable_offsets(&mut ins, &mut outs);
                for off in ins.iter().chain(outs.iter()) {
                    if let VarOffset::Comb(o) = off
                        && locate(&units, *o).is_none()
                    {
                        orphans.insert(*o);
                    }
                }
            }
        };
        sweep(unified, &mut orphans);
        for (_, stmts) in &evs {
            sweep(stmts, &mut orphans);
        }
        for off in extra_offsets {
            if let VarOffset::Comb(o) = off
                && locate(&units, *o).is_none()
            {
                orphans.insert(*o);
            }
        }
    }
    let orphan_count = orphans.len();
    if !orphans.is_empty() {
        // Promote each orphan to a unit reaching the next known boundary
        // (its width is unknown; anything up to the next unit start / next
        // orphan is either the orphan's storage or padding nobody reads).
        let starts: Vec<isize> = units.iter().map(|u| u.start).collect();
        let orphan_list: Vec<isize> = orphans.iter().copied().collect();
        for (i, &o) in orphan_list.iter().enumerate() {
            let next_unit = match starts.binary_search(&o) {
                Ok(_) => unreachable!("orphan inside a unit"),
                Err(j) => starts.get(j).copied().unwrap_or(comb_total as isize),
            };
            let next_orphan = orphan_list
                .get(i + 1)
                .copied()
                .unwrap_or(comb_total as isize);
            let end = next_unit.min(next_orphan).min(comb_total as isize);
            assert!(end > o, "orphan promotion produced an empty unit");
            units.push(Unit {
                start: o,
                end,
                rank: usize::MAX,
            });
        }
        units.sort_by_key(|u| u.start);
        assert!(
            units.windows(2).all(|w| w[0].end <= w[1].start),
            "promoted orphan units overlap existing units",
        );
    }

    // -- 4. second sweep: first-use rank over the settle order, then the
    //    events in their deterministic order.
    {
        let mut idx = 0usize;
        let mut ins: Vec<VarOffset> = Vec::new();
        let mut outs: Vec<VarOffset> = Vec::new();
        let mut sweep = |stmts: &[ProtoStatement], units: &mut [Unit], idx: &mut usize| {
            for s in stmts {
                ins.clear();
                outs.clear();
                s.gather_variable_offsets(&mut ins, &mut outs);
                for off in ins.iter().chain(outs.iter()) {
                    if let VarOffset::Comb(o) = off
                        && let Some(i) = locate(units, *o)
                        && units[i].rank == usize::MAX
                    {
                        units[i].rank = *idx;
                        *idx += 1;
                    }
                }
            }
        };
        sweep(unified, &mut units, &mut idx);
        for (_, stmts) in &evs {
            sweep(stmts, &mut units, &mut idx);
        }
    }

    // -- 5. in-place packing: hot units first in rank order, cold units after
    //    in address order.  Ghost bytes (no unit, referenced nowhere) are not
    //    placed — the layout packs tighter than the original.  Alignment
    //    preserves each unit's old alignment (up to 8), so no access widens
    //    past what the old layout already allowed.
    let hot_units = units.iter().filter(|u| u.rank != usize::MAX).count();
    let total_units = units.len();
    let mut order: Vec<usize> = (0..units.len()).collect();
    order.sort_by_key(|&i| (units[i].rank, units[i].start));

    let align_of = |start: isize, size: isize| -> isize {
        let from_start = if start == 0 {
            8
        } else {
            1 << (start.trailing_zeros().min(3))
        };
        // A unit never needs alignment past its own size: a native-8 access
        // implies at least 8 bytes of storage inside the unit.
        let from_size = match size {
            _ if size >= 8 => 8,
            _ if size >= 4 => 4,
            _ if size >= 2 => 2,
            _ => 1,
        };
        from_start.min(from_size)
    };
    let mut pos: isize = 0;
    let mut placed: Vec<(isize, isize, isize)> = Vec::with_capacity(order.len());
    for i in order {
        let u = &units[i];
        let a = align_of(u.start, u.end - u.start);
        pos = (pos + a - 1) & !(a - 1);
        if pos != u.start {
            placed.push((u.start, u.end, pos));
        }
        pos += u.end - u.start;
    }
    let buffer_end = pos as usize;
    placed.sort_by_key(|&(s, _, _)| s);

    if placed.is_empty() {
        return None;
    }
    let sched = CombLayoutSchedule {
        units: placed,
        buffer_end,
        hot_units,
        total_units,
    };
    if diag() {
        eprintln!(
            "[comb_layout] units={} hot={} moved={} orphans={} comb_total={} buffer_end={}",
            sched.total_units,
            sched.hot_units,
            sched.units.len(),
            orphan_count,
            comb_total,
            sched.buffer_end,
        );
    }
    Some(sched)
}

/// Rewrite every comb offset in `stmts` through the schedule.
/// `CompiledBlock`s shift rigidly (their whole span is one unit by
/// construction, so a single delta is exact).
pub fn apply_to_stmts(stmts: &mut [ProtoStatement], sched: &CombLayoutSchedule) {
    let f = move |off: VarOffset| sched.translate_off(off);
    for s in stmts {
        apply_one(s, sched, &f);
    }
}

fn apply_one(
    s: &mut ProtoStatement,
    sched: &CombLayoutSchedule,
    f: &dyn Fn(VarOffset) -> VarOffset,
) {
    match s {
        ProtoStatement::CompiledBlock(cb) => rigid_shift(cb, sched),
        ProtoStatement::SequentialBlock(body) => {
            for t in body {
                apply_one(t, sched, f);
            }
        }
        _ => {
            assert!(
                !contains_cb(s),
                "CompiledBlock nested under control flow in comb relayout",
            );
            s.remap_offsets_with(f);
        }
    }
}

fn contains_cb(s: &ProtoStatement) -> bool {
    match s {
        ProtoStatement::CompiledBlock(_) => true,
        ProtoStatement::SequentialBlock(body) => body.iter().any(contains_cb),
        ProtoStatement::If(x) => {
            x.true_side.iter().any(contains_cb) || x.false_side.iter().any(contains_cb)
        }
        ProtoStatement::Case(x) => {
            x.arms.iter().any(|a| a.body.iter().any(contains_cb))
                || x.default.iter().any(contains_cb)
        }
        ProtoStatement::For(x) => x.body.iter().any(contains_cb),
        _ => false,
    }
}

fn rigid_shift(cb: &mut CompiledBlockStatement, sched: &CombLayoutSchedule) {
    let Some((lo, hi)) = cb_comb_span(cb) else {
        return;
    };
    let delta = sched.translate(lo) - lo;
    // The whole span is one unit by construction; verify the translation is
    // uniform across the touched bytes (a violation means the unit merge and
    // this shift disagree — a miscompile, so fail loudly even in release).
    assert_eq!(
        sched.translate(hi - 1) - (hi - 1),
        delta,
        "comb relayout: CompiledBlock span [{lo},{hi}) not rigid",
    );
    if delta == 0 {
        return;
    }
    cb.comb_delta_bytes += delta;
    for off in cb
        .input_offsets
        .iter_mut()
        .chain(cb.output_offsets.iter_mut())
    {
        if let VarOffset::Comb(o) = off {
            *off = VarOffset::Comb(*o + delta);
        }
    }
    for s in &mut cb.original_stmts {
        s.adjust_offsets(0, delta);
    }
    for (ins, outs) in &mut cb.stmt_deps {
        for off in ins.iter_mut().chain(outs.iter_mut()) {
            if let VarOffset::Comb(o) = off {
                *off = VarOffset::Comb(*o + delta);
            }
        }
    }
}

/// Rewrite every comb element offset in one meta map.
pub fn translate_meta_map(metas: &mut HashMap<VarId, VariableMeta>, sched: &CombLayoutSchedule) {
    for meta in metas.values_mut() {
        for el in &mut meta.elements {
            if let VarOffset::Comb(o) = el.current {
                el.current = VarOffset::Comb(sched.translate(o));
            }
        }
    }
}

/// `translate_meta_map` over a whole child-module tree.
pub fn translate_meta_tree(node: &mut ModuleVariableMeta, sched: &CombLayoutSchedule) {
    translate_meta_map(&mut node.variable_meta, sched);
    for child in &mut node.children {
        translate_meta_tree(child, sched);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(units: Vec<(isize, isize, isize)>, end: usize) -> CombLayoutSchedule {
        CombLayoutSchedule {
            units,
            buffer_end: end,
            hot_units: 0,
            total_units: 0,
        }
    }

    #[test]
    fn translate_inside_and_outside() {
        let s = sched(vec![(16, 32, 128), (64, 72, 144)], 160);
        assert_eq!(s.translate(0), 0); // before every unit
        assert_eq!(s.translate(16), 128); // unit start
        assert_eq!(s.translate(31), 143); // inside
        assert_eq!(s.translate(32), 32); // one past the end -> identity
        assert_eq!(s.translate(64), 144);
        assert_eq!(s.translate(71), 151);
        assert_eq!(s.translate(100), 100); // past every unit
    }

    #[test]
    fn empty_schedule_is_identity() {
        let s = CombLayoutSchedule::default();
        assert_eq!(s.translate(1234), 1234);
    }

    #[test]
    fn ff_offsets_pass_through() {
        let s = sched(vec![(0, 8, 16)], 24);
        assert_eq!(s.translate_off(VarOffset::Ff(0)), VarOffset::Ff(0));
        assert_eq!(s.translate_off(VarOffset::Comb(4)), VarOffset::Comb(20));
    }

    use crate::ir::expression::{ExpressionContext, ProtoExpression};
    use crate::ir::statement::ProtoAssignStatement;

    fn assign(dst: isize, src: isize) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(dst),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Variable {
                var_offset: VarOffset::Comb(src),
                select: None,
                dynamic_select: None,
                width: 32,
                var_full_width: 32,
                expr_context: ExpressionContext {
                    width: 32,
                    signed: false,
                },
            },
            dst_ff_current_offset: 0,
            token: Default::default(),
        })
    }

    /// Two hot 4-byte vars used in reverse address order + one cold var:
    /// the schedule must pack first-use first and push the cold var last.
    #[test]
    fn packs_by_first_use() {
        let meta_units = vec![(0, 4), (4, 8), (8, 12)];
        // settle order touches offset 8 first, then 0; 4 is cold.
        let unified = vec![assign(8, 8), assign(0, 8)];
        let events = HashMap::default();
        let sched = build_schedule(&meta_units, &unified, &events, &[], 12).unwrap();
        assert_eq!(sched.translate(8), 0); // first used -> packed first
        assert_eq!(sched.translate(0), 4); // second
        assert_eq!(sched.translate(4), 8); // cold -> last
        assert_eq!(sched.buffer_end, 12);
    }

    /// A per-call-site array read through a runtime index assumes its old
    /// contiguity (`translate(base) + stride*i`), so its whole extent must
    /// relocate as one unit even when other orphans interleave its ranks.
    #[test]
    fn dynamic_extent_stays_one_unit() {
        let dyn_read = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(0x20),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::DynamicVariable {
                base_offset: VarOffset::Comb(0x40),
                stride: 4,
                element_native_bytes: 4,
                index_expr: Box::new(ProtoExpression::Variable {
                    var_offset: VarOffset::Comb(0x94),
                    select: None,
                    dynamic_select: None,
                    width: 32,
                    var_full_width: 32,
                    expr_context: ExpressionContext {
                        width: 32,
                        signed: false,
                    },
                }),
                num_elements: 3,
                select: None,
                dynamic_select: None,
                width: 32,
                expr_context: ExpressionContext {
                    width: 32,
                    signed: false,
                },
            },
            dst_ff_current_offset: 0,
            token: Default::default(),
        });
        let unified = vec![
            assign(0x40, 0x90), // arr[0]
            assign(0x80, 0x91), // an unrelated orphan ranks in between
            assign(0x44, 0x92), // arr[1]
            dyn_read,
        ];
        let events = HashMap::default();
        let sched = build_schedule(&[(0x20, 0x24)], &unified, &events, &[], 0xa0).unwrap();
        let base = sched.translate(0x40);
        assert_eq!(sched.translate(0x44), base + 4);
        assert_eq!(sched.translate(0x48), base + 8);
    }

    /// A For counter appears in no gather sweep, but the loop driver writes
    /// it every iteration — it must get a unit, not stay a ghost of the old
    /// space.
    #[test]
    fn for_counter_gets_a_unit() {
        let for_stmt = ProtoStatement::For(crate::ir::statement::ProtoForStatement {
            var_offset: VarOffset::Comb(0x40),
            var_width: 32,
            var_native_bytes: 8,
            var_signed: false,
            range: crate::ir::statement::ProtoForRange::Forward {
                start: crate::ir::statement::ProtoForBound::Const(0),
                end: crate::ir::statement::ProtoForBound::Const(4),
                inclusive: false,
                step: 1,
            },
            body: vec![],
        });
        let unified = vec![assign(0, 0), for_stmt];
        let events = HashMap::default();
        let sched = build_schedule(&[(0, 4)], &unified, &events, &[], 0x48).unwrap();
        // The counter's unit is cold (no gather form ranks it) and packs
        // after the hot var, 8-aligned — inside the packed region, not at
        // its old identity address.
        assert_eq!(sched.translate(0x40), 8);
    }

    /// An offset appearing in statements but absent from every meta span
    /// (function per-call-site copies) must be promoted, not overwritten.
    /// Reads rank before writes, so src offset 8 (an orphan) is hottest.
    #[test]
    fn orphan_offsets_are_promoted() {
        let meta_units = vec![(0, 4)];
        // offset 8 belongs to no meta unit; 4..8 is ghost padding.
        let unified = vec![assign(0, 8)];
        let events = HashMap::default();
        let sched = build_schedule(&meta_units, &unified, &events, &[], 16).unwrap();
        // The promoted unit [8,16) packs first (first use), [0,4) follows.
        assert_eq!(sched.translate(8), 0);
        assert_eq!(sched.translate(0), 8);
        assert_eq!(sched.buffer_end, 12);
    }

    /// Ghost bytes (no unit, no reference) are dropped, so the packed layout
    /// is tighter than the original total.  Reads rank before writes: src 0
    /// keeps its slot, dst 64 packs right behind it.
    #[test]
    fn ghost_bytes_vanish() {
        let meta_units = vec![(0, 4), (64, 68)];
        let unified = vec![assign(64, 0)];
        let events = HashMap::default();
        let sched = build_schedule(&meta_units, &unified, &events, &[], 128).unwrap();
        assert_eq!(sched.buffer_end, 8);
        assert_eq!(sched.translate(0), 0);
        assert_eq!(sched.translate(64), 4);
    }
}
