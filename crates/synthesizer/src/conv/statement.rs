use std::collections::{HashMap, HashSet};

use veryl_analyzer::ir::{self as air, Statement};

use crate::conv::ConvContext;
use crate::conv::arith;
use crate::conv::expression::synthesize_expr;
use crate::ir::{CellKind, NET_CONST0, NetId};
use crate::synthesizer_error::{SynthesizerError, UnsupportedKind};

pub(crate) fn process_statements(
    ctx: &mut ConvContext,
    stmts: &[Statement],
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<(), SynthesizerError> {
    for stmt in stmts {
        process_statement(ctx, stmt, current)?;
    }
    Ok(())
}

fn process_statement(
    ctx: &mut ConvContext,
    stmt: &Statement,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<(), SynthesizerError> {
    match stmt {
        Statement::Assign(a) => {
            if a.dst.len() > 1 {
                // Concat-LHS `{d, e, ...} = a` — slice MSB-first so dst[0]
                // gets the high bits.
                let mut widths: Vec<usize> = Vec::with_capacity(a.dst.len());
                for dst in &a.dst {
                    widths.push(dst_slice_width(ctx, dst)?);
                }
                let total: usize = widths.iter().sum();
                if total == 0 {
                    return Ok(());
                }
                let src = synthesize_expr(ctx, &a.expr, current, total)?;
                let mut lo = 0;
                for (dst, w) in a.dst.iter().zip(widths.iter()).rev() {
                    let slice = src[lo..lo + w].to_vec();
                    write_to_dst(ctx, dst, &slice, current)?;
                    lo += w;
                }
                return Ok(());
            }
            if a.dst.is_empty() {
                return Ok(());
            }
            let dst = &a.dst[0];
            let slice_width = dst_slice_width(ctx, dst)?;
            if slice_width == 0 {
                return Ok(());
            }
            let src = synthesize_expr(ctx, &a.expr, current, slice_width)?;
            write_to_dst(ctx, dst, &src, current)?;
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
        Statement::IfReset(_) => Err(SynthesizerError::internal(
            "nested if_reset reached synthesizer",
        )),
        Statement::For(fs) => Err(SynthesizerError::unsupported(
            UnsupportedKind::ForStatement,
            &fs.token,
        )),
        // $display / $finish etc have no gate-level effect.
        Statement::SystemFunctionCall(_) => Ok(()),
        Statement::FunctionCall(call) => {
            crate::conv::expression::synth_function_call_stmt(ctx, call, current)
        }
        Statement::TbMethodCall(_) => Err(SynthesizerError::internal(
            "testbench method call reached synthesizer",
        )),
        Statement::Break => Err(SynthesizerError::internal(
            "break statement reached synthesizer",
        )),
        Statement::Unsupported(_) | Statement::Null => Ok(()),
    }
}

/// Computes how many source bits an `AssignDestination` consumes. Purely
/// structural — doesn't allocate any cells. `write_to_dst` produces the
/// dynamic-index/select nets later.
pub(crate) fn dst_slice_width(
    ctx: &ConvContext,
    dst: &air::AssignDestination,
) -> Result<usize, SynthesizerError> {
    let scalar_width = ctx
        .variables
        .get(&dst.id)
        .map(|s| s.scalar_width)
        .ok_or_else(|| SynthesizerError::internal(format!("unknown assign target {}", dst.id)))?;
    let member_width = match &dst.comptime.part_select {
        Some(ps) => ps
            .part_select
            .last()
            .and_then(|p| p.r#type.total_width())
            .unwrap_or(scalar_width),
        None => scalar_width,
    };

    if dst.select.is_empty() {
        Ok(member_width)
    } else if dst.select.is_const() {
        let mut eval_ctx = veryl_analyzer::Context::default();
        let (hi, lo) = dst
            .select
            .eval_value(&mut eval_ctx, &dst.comptime.r#type, false)
            .ok_or_else(|| {
                SynthesizerError::dynamic_select(format!("dst {}", dst.id), &dst.token)
            })?;
        Ok(hi + 1 - lo)
    } else if dst.select.is_range() {
        // Non-const range — endpoints must be constant to derive a fixed width.
        Err(SynthesizerError::unsupported(
            UnsupportedKind::DynamicRangeSelect {
                what: format!("dst {}", dst.id),
            },
            &dst.token,
        ))
    } else {
        // Dynamic single-bit select `a[idx] = ...`.
        Ok(1)
    }
}

/// Writes `src` (already sized to `dst_slice_width(dst)`) into `current`'s
/// entry for `dst.id`. Handles static / dynamic array index, static range /
/// dynamic single-bit select, and struct-member part_select by flattening
/// both dimensions into per-bit target positions with optional match nets.
/// Shared by single-dst Assign, concat-LHS, and inst-output wiring.
pub(crate) fn write_to_dst(
    ctx: &mut ConvContext,
    dst: &air::AssignDestination,
    src: &[NetId],
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<(), SynthesizerError> {
    let (total_width, scalar_width, shape) = {
        let slot = ctx.variables.get(&dst.id).ok_or_else(|| {
            SynthesizerError::internal(format!("unknown assign target {}", dst.id))
        })?;
        (slot.width, slot.scalar_width, slot.shape.clone())
    };
    if total_width == 0 {
        return Ok(());
    }

    let (member_offset, member_width) = match &dst.comptime.part_select {
        Some(ps) => {
            let offset: usize = ps.part_select.iter().map(|p| p.pos).sum();
            let width = ps
                .part_select
                .last()
                .and_then(|p| p.r#type.total_width())
                .unwrap_or(scalar_width);
            (offset, width)
        }
        None => (0, scalar_width),
    };

    enum SelectKind {
        Static { lo: usize, hi: usize },
        DynamicSingle { idx_nets: Vec<NetId> },
    }
    let select_kind = if dst.select.is_empty() {
        SelectKind::Static {
            lo: 0,
            hi: member_width - 1,
        }
    } else if dst.select.is_const() && !dst.select.is_range() {
        let mut eval_ctx = veryl_analyzer::Context::default();
        let (hi, lo) = dst
            .select
            .eval_value(&mut eval_ctx, &dst.comptime.r#type, false)
            .ok_or_else(|| {
                SynthesizerError::dynamic_select(format!("dst {}", dst.id), &dst.token)
            })?;
        SelectKind::Static { lo, hi }
    } else if dst.select.is_range() {
        if !dst.select.is_const() {
            return Err(SynthesizerError::unsupported(
                UnsupportedKind::DynamicRangeSelect {
                    what: format!("dst {}", dst.id),
                },
                &dst.token,
            ));
        }
        if let Some((_, end)) = &dst.select.1
            && !end.comptime().is_const
        {
            return Err(SynthesizerError::unsupported(
                UnsupportedKind::DynamicRangeEnd {
                    what: format!("dst {}", dst.id),
                },
                &dst.token,
            ));
        }
        let mut eval_ctx = veryl_analyzer::Context::default();
        let (hi, lo) = dst
            .select
            .eval_value(&mut eval_ctx, &dst.comptime.r#type, false)
            .ok_or_else(|| {
                SynthesizerError::dynamic_select(format!("dst {}", dst.id), &dst.token)
            })?;
        SelectKind::Static { lo, hi }
    } else {
        if dst.select.0.len() != 1 {
            return Err(SynthesizerError::unsupported(
                UnsupportedKind::MultiDimDynamicSelect {
                    what: format!("dst {}", dst.id),
                },
                &dst.token,
            ));
        }
        let idx_bits = arith::index_bits_for(member_width);
        let idx_nets = synthesize_expr(ctx, &dst.select.0[0], current, idx_bits)?;
        SelectKind::DynamicSingle { idx_nets }
    };

    let slice_width = match &select_kind {
        SelectKind::Static { lo, hi } => hi + 1 - lo,
        SelectKind::DynamicSingle { .. } => 1,
    };

    enum IndexKind {
        Static(usize),
        Dynamic {
            idx_nets: Vec<NetId>,
            num_elements: usize,
        },
    }

    let index_kind = if dst.index.0.is_empty() {
        IndexKind::Static(0)
    } else if dst.index.is_const() {
        let mut eval_ctx = veryl_analyzer::Context::default();
        let indices = dst.index.eval_value(&mut eval_ctx).ok_or_else(|| {
            SynthesizerError::dynamic_select(format!("dst index {}", dst.id), &dst.token)
        })?;
        let flat = shape.calc_index(&indices).ok_or_else(|| {
            SynthesizerError::internal(format!(
                "array index out of range for {} (shape dims {})",
                dst.id,
                shape.dims()
            ))
        })?;
        IndexKind::Static(flat * scalar_width)
    } else {
        if shape.dims() != 1 {
            return Err(SynthesizerError::unsupported(
                UnsupportedKind::DynamicMultiDimIndex {
                    what: format!("assign destination {}", dst.id),
                },
                &dst.token,
            ));
        }
        let num_elements = shape.total().unwrap_or(0);
        if num_elements == 0 {
            return Err(SynthesizerError::internal(format!(
                "zero-element array in assign dst {}",
                dst.id
            )));
        }
        let idx_bits = arith::index_bits_for(num_elements);
        let idx_nets = synthesize_expr(ctx, &dst.index.0[0], current, idx_bits)?;
        IndexKind::Dynamic {
            idx_nets,
            num_elements,
        }
    };

    let var_current = current.entry(dst.id).or_insert_with(|| {
        ctx.variables
            .get(&dst.id)
            .map(|s| s.nets.clone())
            .unwrap_or_default()
    });

    // Flatten index/select into per-element and per-bit coordinates plus
    // optional dynamic match nets, then write `Mux(match, old, src_bit)` at
    // each target bit (or a direct overwrite for fully-static dsts).
    let (element_offsets, element_match): (Vec<usize>, Option<Vec<NetId>>) = match index_kind {
        IndexKind::Static(off) => (vec![off], None),
        IndexKind::Dynamic {
            idx_nets,
            num_elements,
        } => {
            let offsets: Vec<usize> = (0..num_elements).map(|k| k * scalar_width).collect();
            let matches: Vec<NetId> = (0..num_elements)
                .map(|k| arith::eq_const(ctx, &idx_nets, k))
                .collect();
            (offsets, Some(matches))
        }
    };
    let (bit_positions, bit_match): (Vec<usize>, Option<Vec<NetId>>) = match select_kind {
        SelectKind::Static { lo, hi } => ((lo..=hi).map(|p| member_offset + p).collect(), None),
        SelectKind::DynamicSingle { idx_nets } => {
            let positions: Vec<usize> = (0..member_width).map(|b| member_offset + b).collect();
            let matches: Vec<NetId> = (0..member_width)
                .map(|b| arith::eq_const(ctx, &idx_nets, b))
                .collect();
            (positions, Some(matches))
        }
    };

    for (i, &b) in bit_positions.iter().enumerate() {
        // DynamicSingle gives one bit_position per member bit (all potential
        // targets), so we iterate every entry and gate with bit_match[i].
        // Static maps src[i] → bit_positions[i] one-for-one, so we stop at
        // slice_width / src.len().
        let new_val = if bit_match.is_some() {
            *src.first().unwrap_or(&NET_CONST0)
        } else {
            if i >= slice_width || i >= src.len() {
                break;
            }
            src[i]
        };
        for (e, &offset) in element_offsets.iter().enumerate() {
            let target_bit = offset + b;
            if target_bit >= var_current.len() {
                continue;
            }
            let combined = match (element_match.as_ref(), bit_match.as_ref()) {
                (None, None) => None,
                (Some(em), None) => Some(em[e]),
                (None, Some(bm)) => Some(bm[i]),
                (Some(em), Some(bm)) => Some(ctx.add_cell(CellKind::And2, vec![em[e], bm[i]])),
            };
            var_current[target_bit] = match combined {
                None => new_val,
                Some(m) => {
                    let old = var_current[target_bit];
                    ctx.add_cell(CellKind::Mux2, vec![m, old, new_val])
                }
            };
        }
    }
    Ok(())
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
            let tn = *t_nets.get(i).unwrap_or(&NET_CONST0);
            let fn_ = *f_nets.get(i).unwrap_or(&NET_CONST0);
            if tn == fn_ {
                new_nets.push(tn);
            } else {
                new_nets.push(ctx.add_cell(CellKind::Mux2, vec![cond, fn_, tn]));
            }
        }
        *base_nets = new_nets;
    }
}
