use std::collections::{HashMap, HashSet};

use veryl_analyzer::ir::{self as air, Statement};

use crate::conv::ConvContext;
use crate::conv::expression::synthesize_expr;
use crate::error::SynthError;
use crate::ir::{CellKind, NetId};

pub(crate) fn process_statements(
    ctx: &mut ConvContext,
    stmts: &[Statement],
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<(), SynthError> {
    for stmt in stmts {
        process_statement(ctx, stmt, current)?;
    }
    Ok(())
}

fn process_statement(
    ctx: &mut ConvContext,
    stmt: &Statement,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<(), SynthError> {
    match stmt {
        Statement::Assign(a) => {
            if a.dst.len() != 1 {
                return Err(SynthError::unsupported(
                    "multiple assignment destinations (Phase 2+)",
                ));
            }
            let dst = &a.dst[0];
            if !dst.index.0.is_empty() {
                return Err(SynthError::unsupported(
                    "indexed assignment destination (Phase 2+)",
                ));
            }

            let slot = ctx
                .variables
                .get(&dst.id)
                .ok_or_else(|| SynthError::Internal(format!("unknown assign target {}", dst.id)))?;
            let full_width = slot.width;
            if full_width == 0 {
                return Ok(());
            }

            let (hi, lo) = if dst.select.is_empty() {
                (full_width - 1, 0)
            } else if !dst.select.is_const() {
                return Err(SynthError::dynamic_select(format!("dst {}", dst.id)));
            } else {
                let mut eval_ctx = veryl_analyzer::Context::default();
                dst.select
                    .eval_value(&mut eval_ctx, &dst.comptime.r#type, false)
                    .ok_or_else(|| SynthError::dynamic_select(format!("dst {}", dst.id)))?
            };
            let slice_width = hi + 1 - lo;

            let src = synthesize_expr(ctx, &a.expr, current, slice_width)?;

            let var_current = current.entry(dst.id).or_insert_with(|| {
                ctx.variables
                    .get(&dst.id)
                    .map(|s| s.nets.clone())
                    .unwrap_or_default()
            });

            for (i, n) in src.iter().enumerate().take(slice_width) {
                if lo + i < var_current.len() {
                    var_current[lo + i] = *n;
                }
            }
            Ok(())
        }
        Statement::If(ifst) => {
            let cond = synthesize_expr(ctx, &ifst.cond, current, 1)?[0];
            let mut true_branch = current.clone();
            process_statements(ctx, &ifst.true_side, &mut true_branch)?;
            let mut false_branch = current.clone();
            process_statements(ctx, &ifst.false_side, &mut false_branch)?;
            merge_branches(ctx, cond, current, &true_branch, &false_branch);
            Ok(())
        }
        Statement::IfReset(_) => {
            // Top-level if_reset is split out by split_if_reset; encountering
            // one here means it's nested, which we don't handle.
            Err(SynthError::unsupported("nested if_reset"))
        }
        Statement::For(_) => Err(SynthError::unsupported("for loop (Phase 2+)")),
        // $display / $finish etc have no gate-level effect.
        Statement::SystemFunctionCall(_) => Ok(()),
        Statement::FunctionCall(_) => Err(SynthError::unsupported(
            "function call statement (Phase 2+)",
        )),
        Statement::TbMethodCall(_) => Err(SynthError::unsupported("testbench-method statement")),
        Statement::Break => Err(SynthError::unsupported("break statement")),
        Statement::Unsupported(_) | Statement::Null => Ok(()),
    }
}

fn merge_branches(
    ctx: &mut ConvContext,
    cond: NetId,
    base: &mut HashMap<air::VarId, Vec<NetId>>,
    tbranch: &HashMap<air::VarId, Vec<NetId>>,
    fbranch: &HashMap<air::VarId, Vec<NetId>>,
) {
    let mut keys: HashSet<air::VarId> = HashSet::new();
    for k in tbranch.keys() {
        keys.insert(*k);
    }
    for k in fbranch.keys() {
        keys.insert(*k);
    }

    for key in keys {
        let base_nets = base.entry(key).or_insert_with(|| {
            ctx.variables
                .get(&key)
                .map(|s| s.nets.clone())
                .unwrap_or_default()
        });
        let t_nets = tbranch
            .get(&key)
            .cloned()
            .unwrap_or_else(|| base_nets.clone());
        let f_nets = fbranch
            .get(&key)
            .cloned()
            .unwrap_or_else(|| base_nets.clone());

        let width = base_nets.len().max(t_nets.len()).max(f_nets.len());
        let mut new_nets = Vec::with_capacity(width);
        for i in 0..width {
            let tn = *t_nets.get(i).unwrap_or(&crate::ir::NET_CONST0);
            let fn_ = *f_nets.get(i).unwrap_or(&crate::ir::NET_CONST0);
            if tn == fn_ {
                new_nets.push(tn);
            } else {
                new_nets.push(ctx.add_cell(CellKind::Mux2, vec![cond, fn_, tn]));
            }
        }
        *base_nets = new_nets;
    }
}
