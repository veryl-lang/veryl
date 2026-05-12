// Stage 7 Phase B look-ahead load_cache eviction policy (env-gated).
//
// Pre-computes per-VarOffset future read positions across the JIT
// chunk's top-level ProtoStatement sequence, so that
// `cranelift::build_binary_inner` can evict cache entries with no
// near-future use after each statement.
//
// Mechanism: the load_cache holds Cranelift Values across statements.
// Each cache hit emits a `use_var`-equivalent IR reference, extending
// the SSA Value's live range to the latest hit.  Without intervention,
// values that hit early-and-late but not in between still occupy a
// register across the gap, starving regalloc.  The look-ahead policy
// removes a cached Value from the HashMap when its next read is more
// than `threshold` statements ahead — subsequent IR generation won't
// reference the Value, regalloc sees its live range end at the most
// recent use, and the register is freed.  The `threshold + 1`-th and
// later reads pay one fresh `iadd_imm + load`, but the freed register
// makes room for the values that ARE used densely.
//
// Limitations of this PoC: position is counted in TOP-LEVEL stmt
// indices only (does not descend into If/For bodies).  Reads inside
// conditional bodies are observed as occurring at the position of the
// containing If/For statement.  This is conservative — values used
// only inside one branch are seen as "used at branch position", which
// matches load_cache lifetime (cleared at If boundaries by
// statement.rs).
//
// env vars:
//   VERYL_STAGE7_LOOKAHEAD=1   enable evict policy (default-off)
//   VERYL_STAGE7_LOOKAHEAD_THRESHOLD=N
//                              keep cache entry if next_read - cur <= N
//                              (default 20).  N=0 means evict whenever
//                              next_read is past current stmt; large N
//                              effectively disables eviction.

#![cfg(not(target_family = "wasm"))]

use super::expression::ProtoExpression;
use super::statement::{ProtoForBound, ProtoForRange, ProtoStatement};
use super::variable::VarOffset;
use crate::HashMap;

/// Map from VarOffset to sorted list of top-level stmt indices where
/// the offset is read.  Indices may repeat if the same offset is read
/// multiple times within the expression tree of one statement.
pub type FutureReads = HashMap<VarOffset, Vec<usize>>;

pub fn compute_read_positions(stmts: &[ProtoStatement]) -> FutureReads {
    let mut reads: FutureReads = HashMap::default();
    for (i, s) in stmts.iter().enumerate() {
        walk_stmt(s, i, &mut reads);
    }
    // Each Vec is naturally sorted because we iterate stmts in order.
    reads
}

fn walk_stmt(s: &ProtoStatement, idx: usize, reads: &mut FutureReads) {
    match s {
        ProtoStatement::Assign(a) => {
            walk_expr(&a.expr, idx, reads);
            if let Some(dyn_sel) = &a.dynamic_select {
                walk_expr(&dyn_sel.index_expr, idx, reads);
            }
        }
        ProtoStatement::AssignDynamic(a) => {
            walk_expr(&a.dst_index_expr, idx, reads);
            walk_expr(&a.expr, idx, reads);
            if let Some(dyn_sel) = &a.dynamic_select {
                walk_expr(&dyn_sel.index_expr, idx, reads);
            }
        }
        ProtoStatement::If(if_stmt) => {
            if let Some(cond) = &if_stmt.cond {
                walk_expr(cond, idx, reads);
            }
            for sub in &if_stmt.true_side {
                walk_stmt(sub, idx, reads);
            }
            for sub in &if_stmt.false_side {
                walk_stmt(sub, idx, reads);
            }
        }
        ProtoStatement::For(f) => {
            let walk_bound = |b: &ProtoForBound, r: &mut FutureReads| {
                if let ProtoForBound::Dynamic(expr) = b {
                    walk_expr(expr, idx, r);
                }
            };
            match &f.range {
                ProtoForRange::Forward { start, end, .. }
                | ProtoForRange::Reverse { start, end, .. }
                | ProtoForRange::Stepped { start, end, .. } => {
                    walk_bound(start, reads);
                    walk_bound(end, reads);
                }
            }
            for sub in &f.body {
                walk_stmt(sub, idx, reads);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for sub in body {
                walk_stmt(sub, idx, reads);
            }
        }
        ProtoStatement::SystemFunctionCall(_)
        | ProtoStatement::CompiledBlock(_)
        | ProtoStatement::TbMethodCall { .. }
        | ProtoStatement::Break => {}
    }
}

fn walk_expr(expr: &ProtoExpression, idx: usize, reads: &mut FutureReads) {
    match expr {
        ProtoExpression::Variable {
            var_offset,
            dynamic_select,
            ..
        } => {
            let v = reads.entry(*var_offset).or_default();
            // Avoid pushing duplicate consecutive same-stmt entries —
            // multiple reads of the same offset within one stmt collapse
            // to a single position since the evictor only looks for the
            // soonest future stmt that reads this offset.
            if v.last().copied() != Some(idx) {
                v.push(idx);
            }
            if let Some(dyn_sel) = dynamic_select {
                walk_expr(&dyn_sel.index_expr, idx, reads);
            }
        }
        ProtoExpression::DynamicVariable {
            index_expr,
            dynamic_select,
            ..
        } => {
            walk_expr(index_expr, idx, reads);
            if let Some(dyn_sel) = dynamic_select {
                walk_expr(&dyn_sel.index_expr, idx, reads);
            }
        }
        ProtoExpression::Unary { x, .. } => walk_expr(x, idx, reads),
        ProtoExpression::Binary { x, y, .. } => {
            walk_expr(x, idx, reads);
            walk_expr(y, idx, reads);
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            walk_expr(cond, idx, reads);
            walk_expr(true_expr, idx, reads);
            walk_expr(false_expr, idx, reads);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (e, _, _) in elements {
                walk_expr(e, idx, reads);
            }
        }
        ProtoExpression::Value { .. } => {}
    }
}

