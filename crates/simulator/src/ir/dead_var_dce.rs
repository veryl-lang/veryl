//! Dead comb-Variable DCE (analyzer-level transform).
//!
//! Walks every pre-JIT ProtoStatement slice for a module, counts each
//! comb VarOffset's reads (local Variable expressions + child
//! CompiledBlock `input_offsets`), and drops every full-width local
//! `Assign` whose dst has zero reads.
//!
//! Complementary to `dup_assign_dce::dce_aggressive`:
//! * `dup_assign_dce` drops a write that is overwritten by a later
//!   same-dst write (last-write-wins).
//! * This pass drops a write whose dst is never read at all (no
//!   consumer anywhere in the module).  Such dst-only Variables tend
//!   to be intermediate temporaries left behind by `comb_to_ff_hoist`
//!   (the hoisted FF now serves the consumer, leaving the comb side
//!   write computed but unused).
//!
//! Safety:
//! * Writes coming from a child `CompiledBlock` cannot be dropped at
//!   this module level — we don't touch the child's compiled body, so
//!   any Variable whose only writes are inherited is kept alive.
//! * Partial-width writes (`Assign` with select / rhs_select,
//!   `AssignDynamic`) target a sub-range of storage that may overlap
//!   later full-width writes — keep them alive even if the dst's
//!   reads count is zero.
//! * Variables read via CompiledBlock `input_offsets` count as reads —
//!   the child consumes them opaquely.
//!
//! Default ON; opt out via `VERYL_DEAD_VAR_DCE=0`.

use crate::HashSet;
use crate::ir::statement::{ProtoForBound, ProtoForRange, ProtoSystemFunctionCall};
use crate::ir::{ProtoExpression, ProtoStatement, VarOffset};
use std::sync::OnceLock;

/// Cached `VERYL_DEAD_VAR_DCE` env probe.  Default ON; opt out by
/// setting `VERYL_DEAD_VAR_DCE=0`.
pub fn enabled() -> bool {
    static EN: OnceLock<bool> = OnceLock::new();
    *EN.get_or_init(|| std::env::var("VERYL_DEAD_VAR_DCE").ok().as_deref() != Some("0"))
}

#[derive(Default, Clone, Copy)]
struct Liveness {
    /// Local full-width Assign writes — the only writes this pass
    /// can drop.
    full_writes: u32,
    /// Local partial-width writes (select / dyn_sel / rhs_select /
    /// AssignDynamic).  Keep the Variable alive even if reads==0
    /// because the partial write may overlap a later full read of
    /// a different sub-range.
    partial_writes: u32,
    /// Writes via child `CompiledBlock.output_offsets` — not
    /// droppable at this level.
    inherited_writes: u32,
    /// Any kind of read.
    reads: u32,
}

/// Descriptor for a runtime-indexed array access (read via
/// `DynamicVariable` or write via `AssignDynamic`).  Every offset
/// inside the range is conservatively "tainted" — DCE must skip
/// candidates falling within any such range.  Recorded by the
/// walker as a single descriptor per access instead of expanding to
/// all N elements, keeping the live map small for designs with large
/// memory arrays.
#[derive(Clone, Copy)]
struct ArrayRange {
    base: VarOffset,
    num: usize,
    stride: isize,
}

impl ArrayRange {
    #[inline]
    fn contains(&self, off: VarOffset) -> bool {
        if off.is_ff() != self.base.is_ff() {
            return false;
        }
        if self.stride == 0 || self.num == 0 {
            return false;
        }
        let delta = off.raw() - self.base.raw();
        if delta < 0 {
            return false;
        }
        if delta % self.stride != 0 {
            return false;
        }
        let idx = delta / self.stride;
        idx >= 0 && idx < self.num as isize
    }
}

#[derive(Default)]
struct Census {
    live: crate::HashMap<VarOffset, Liveness>,
    /// Runtime-indexed array accesses — see `ArrayRange`.
    ranges: Vec<ArrayRange>,
}

fn walk_expr_reads(expr: &ProtoExpression, c: &mut Census) {
    match expr {
        ProtoExpression::Variable {
            var_offset,
            dynamic_select,
            ..
        } => {
            let e = c.live.entry(*var_offset).or_default();
            e.reads = e.reads.saturating_add(1);
            if let Some(dyn_sel) = dynamic_select {
                walk_expr_reads(&dyn_sel.index_expr, c);
            }
        }
        ProtoExpression::Value { .. } => {}
        ProtoExpression::Unary { x, .. } => walk_expr_reads(x, c),
        ProtoExpression::Binary { x, y, .. } => {
            walk_expr_reads(x, c);
            walk_expr_reads(y, c);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (e, _, _) in elements {
                walk_expr_reads(e, c);
            }
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            walk_expr_reads(cond, c);
            walk_expr_reads(true_expr, c);
            walk_expr_reads(false_expr, c);
        }
        ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            index_expr,
            num_elements,
            dynamic_select,
            ..
        } => {
            // Array read at a runtime index — record as a single
            // range descriptor instead of expanding to N entries
            // (designs with large memory arrays would otherwise
            // produce millions of HashMap inserts per pass).  The
            // DCE candidate filter disqualifies any offset that
            // lands in this range.
            c.ranges.push(ArrayRange {
                base: *base_offset,
                num: *num_elements,
                stride: *stride,
            });
            walk_expr_reads(index_expr, c);
            if let Some(dyn_sel) = dynamic_select {
                walk_expr_reads(&dyn_sel.index_expr, c);
            }
        }
    }
}

fn walk_for_range(range: &ProtoForRange, c: &mut Census) {
    let (start, end) = match range {
        ProtoForRange::Forward { start, end, .. }
        | ProtoForRange::Reverse { start, end, .. }
        | ProtoForRange::Stepped { start, end, .. } => (start, end),
    };
    for bound in [start, end] {
        if let ProtoForBound::Dynamic(expr) = bound {
            walk_expr_reads(expr, c);
        }
    }
}

fn walk_stmt_liveness(stmt: &ProtoStatement, c: &mut Census) {
    match stmt {
        ProtoStatement::Assign(x) => {
            walk_expr_reads(&x.expr, c);
            if let Some(dyn_sel) = &x.dynamic_select {
                walk_expr_reads(&dyn_sel.index_expr, c);
            }
            let e = c.live.entry(x.dst).or_default();
            if x.select.is_some() || x.dynamic_select.is_some() || x.rhs_select.is_some() {
                e.partial_writes = e.partial_writes.saturating_add(1);
            } else {
                e.full_writes = e.full_writes.saturating_add(1);
            }
        }
        ProtoStatement::AssignDynamic(x) => {
            walk_expr_reads(&x.dst_index_expr, c);
            walk_expr_reads(&x.expr, c);
            if let Some(dyn_sel) = &x.dynamic_select {
                walk_expr_reads(&dyn_sel.index_expr, c);
            }
            // Array write at a runtime index — record a single
            // range descriptor instead of touching every element.
            c.ranges.push(ArrayRange {
                base: x.dst_base,
                num: x.dst_num_elements,
                stride: x.dst_stride,
            });
        }
        ProtoStatement::If(x) => {
            if let Some(cond) = &x.cond {
                walk_expr_reads(cond, c);
            }
            for s in &x.true_side {
                walk_stmt_liveness(s, c);
            }
            for s in &x.false_side {
                walk_stmt_liveness(s, c);
            }
        }
        ProtoStatement::For(x) => {
            // The For machinery itself writes the induction `var_offset`
            // on every iteration; mark it as a partial write so DCE
            // never drops Assigns aliasing the loop counter.  Range
            // bounds may carry Variable reads in their Dynamic case.
            let e = c.live.entry(x.var_offset).or_default();
            e.partial_writes = e.partial_writes.saturating_add(1);
            walk_for_range(&x.range, c);
            for s in &x.body {
                walk_stmt_liveness(s, c);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                walk_stmt_liveness(s, c);
            }
        }
        ProtoStatement::SystemFunctionCall(x) => match x {
            ProtoSystemFunctionCall::Display { args, .. }
            | ProtoSystemFunctionCall::Write { args, .. } => {
                for a in args {
                    walk_expr_reads(a, c);
                }
            }
            ProtoSystemFunctionCall::Readmemh { .. } => {}
            ProtoSystemFunctionCall::Assert {
                condition, args, ..
            } => {
                walk_expr_reads(condition, c);
                for a in args {
                    walk_expr_reads(a, c);
                }
            }
            ProtoSystemFunctionCall::Finish => {}
        },
        ProtoStatement::CompiledBlock(x) => {
            for s in &x.original_stmts {
                walk_stmt_liveness(s, c);
            }
        }
        ProtoStatement::TbMethodCall { .. } => {}
        ProtoStatement::Break => {}
    }
}

/// Collect the dead-droppable set: comb VarOffsets that have zero
/// reads anywhere in `slices` and whose only writes are local
/// full-width `Assign`s.  No-op (empty set) when the env knob is off.
pub fn collect_dead_offsets(slices: &[&[ProtoStatement]]) -> HashSet<VarOffset> {
    if !enabled() {
        return HashSet::default();
    }
    let mut census = Census::default();
    for slice in slices {
        for s in *slice {
            walk_stmt_liveness(s, &mut census);
        }
    }
    let mut dead = HashSet::default();
    for (off, s) in &census.live {
        if off.is_ff() {
            continue;
        }
        if s.reads != 0 {
            continue;
        }
        if s.partial_writes != 0 || s.inherited_writes != 0 {
            continue;
        }
        if s.full_writes == 0 {
            continue;
        }
        // Disqualify when this offset falls within any array range
        // touched by an AssignDynamic / DynamicVariable elsewhere in
        // the module.  Conservatively keeps the whole array alive.
        if census.ranges.iter().any(|r| r.contains(*off)) {
            continue;
        }
        dead.insert(*off);
    }
    dead
}

/// Filter out every `Assign` whose dst is in `dead`.  Recurses into
/// `If` / `For` / `SequentialBlock`.  CompiledBlock is left intact —
/// its `func` was pre-compiled with the original offsets and can't
/// be patched, so any `Assign` it owns stays put even when its dst
/// looks dead at this layer.  Returns `(new_stmts, dropped_count)`
/// so callers can detect the multi-pass fixpoint.
pub fn apply_counting(
    stmts: Vec<ProtoStatement>,
    dead: &HashSet<VarOffset>,
) -> (Vec<ProtoStatement>, usize) {
    if dead.is_empty() {
        return (stmts, 0);
    }
    let mut out: Vec<ProtoStatement> = Vec::with_capacity(stmts.len());
    let mut dropped = 0usize;
    for s in stmts {
        match s {
            ProtoStatement::Assign(ref a)
                if a.select.is_none()
                    && a.dynamic_select.is_none()
                    && a.rhs_select.is_none()
                    && dead.contains(&a.dst) =>
            {
                dropped += 1;
            }
            ProtoStatement::SequentialBlock(body) => {
                let (body, d) = apply_counting(body, dead);
                dropped += d;
                out.push(ProtoStatement::SequentialBlock(body));
            }
            ProtoStatement::If(mut if_stmt) => {
                let (ts, d1) = apply_counting(if_stmt.true_side, dead);
                let (fs, d2) = apply_counting(if_stmt.false_side, dead);
                dropped += d1 + d2;
                if_stmt.true_side = ts;
                if_stmt.false_side = fs;
                out.push(ProtoStatement::If(if_stmt));
            }
            ProtoStatement::For(mut for_stmt) => {
                let (body, d) = apply_counting(for_stmt.body, dead);
                dropped += d;
                for_stmt.body = body;
                out.push(ProtoStatement::For(for_stmt));
            }
            other => out.push(other),
        }
    }
    (out, dropped)
}
