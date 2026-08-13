//! Per-element always_ff write counts.
//!
//! [`FfTable::update_is_ff`](crate::ir::FfTable::update_is_ff) treats a
//! same-block self-reference as safe to read without a register, which only
//! holds while no other statement writes the element. Branch arms are
//! mutually exclusive, so they contribute their maximum rather than their sum;
//! a loop body counts once.

use crate::HashMap;
use crate::conv::Context;
use crate::ir::{AssignDestination, Declaration, Statement, VarId, VarKind};
use crate::symbol::Affiliation;

/// Writes per `(declaration index, VarId, array element)`.
pub type WriteCounts = HashMap<(usize, VarId, usize), u32>;

pub fn count_writes(decls: &[Declaration], context: &mut Context) -> WriteCounts {
    let mut result = WriteCounts::default();
    for (decl, x) in decls.iter().enumerate() {
        if let Declaration::Ff(ff) = x {
            for (key, n) in count_seq(&ff.statements, context) {
                *result.entry((decl, key.0, key.1)).or_insert(0) += n;
            }
        }
    }
    result
}

type ElementCounts = HashMap<(VarId, usize), u32>;

fn count_seq(stmts: &[Statement], context: &mut Context) -> ElementCounts {
    let mut result = ElementCounts::default();
    for s in stmts {
        for (key, n) in count_one(s, context) {
            *result.entry(key).or_insert(0) += n;
        }
    }
    result
}

fn count_one(stmt: &Statement, context: &mut Context) -> ElementCounts {
    let mut result = ElementCounts::default();
    match stmt {
        Statement::Assign(x) => {
            for dst in &x.dst {
                add_dst(dst, context, &mut result);
            }
        }
        Statement::If(x) => {
            branch_max(
                count_seq(&x.true_side, context),
                count_seq(&x.false_side, context),
                &mut result,
            );
        }
        Statement::IfReset(x) => {
            branch_max(
                count_seq(&x.true_side, context),
                count_seq(&x.false_side, context),
                &mut result,
            );
        }
        Statement::Case(x) => {
            return count_seq(&x.lower_to_nested_if(), context);
        }
        Statement::For(x) => {
            return count_seq(&x.body, context);
        }
        Statement::FunctionCall(x) => {
            for outputs in x.outputs.values() {
                for dst in outputs {
                    add_dst(dst, context, &mut result);
                }
            }
        }
        Statement::SystemFunctionCall(_)
        | Statement::TbMethodCall(_)
        | Statement::Break
        | Statement::Unsupported(_)
        | Statement::Null => {}
    }
    result
}

fn branch_max(t: ElementCounts, f: ElementCounts, out: &mut ElementCounts) {
    for (key, n) in t.into_iter().chain(f) {
        let e = out.entry(key).or_insert(0);
        *e = (*e).max(n);
    }
}

/// Mirrors the filters in `AssignDestination::gather_ff` so a counted write
/// always has a matching `FfTable` entry.
fn add_dst(dst: &AssignDestination, context: &mut Context, out: &mut ElementCounts) {
    let Some(variable) = context.get_variable_info(dst.id) else {
        return;
    };
    if variable.kind == VarKind::Let || variable.affiliation == Affiliation::AlwaysFf {
        return;
    }
    if let Some(index) = dst.index.eval_value(context) {
        if let Some(variable) = context.get_variable_info(dst.id)
            && let Some(index) = variable.r#type.array.calc_index(&index)
        {
            *out.entry((dst.id, index)).or_insert(0) += 1;
        }
    } else if let Some(total_array) = context
        .get_variable_info(dst.id)
        .and_then(|v| v.r#type.total_array())
    {
        for i in 0..total_array {
            *out.entry((dst.id, i)).or_insert(0) += 1;
        }
    }
}
