use std::collections::{BTreeSet, HashMap, HashSet};

use veryl_analyzer::ir::{self as air, Statement};

use crate::conv::ConvContext;
use crate::conv::arith;
use crate::conv::expression::{
    emit_cube, emit_sop_bit, minimize_sop_cubes, reduce_and, reduce_or, synth_function_call_stmt,
    synthesize_expr, try_constant,
};
use crate::ir::{CellKind, NET_CONST0, NET_CONST1, NetId};
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
            if try_fold_case_like(ctx, ifst, current)? {
                return Ok(());
            }
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
        Statement::FunctionCall(call) => synth_function_call_stmt(ctx, call, current),
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

/// Rewrite an if-else-if chain of `sel == K_i` (or half-open range) arms
/// into shared match signals fed through sum-of-products, replacing the
/// generic per-bit Mux2 cascade. Arm bodies are synthesised into a local
/// scope so nested folds still work; vars an arm doesn't touch fall back
/// to their pre-If value. Returns `true` when the fold fires.
fn try_fold_case_like(
    ctx: &mut ConvContext,
    ifst: &air::IfStatement,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<bool, SynthesizerError> {
    // Need ≥ 3 arms to beat the generic path; below that the shared-match
    // overhead costs more than it saves.
    const MIN_ARMS: usize = 3;

    let Some(chain) = collect_case_chain(ifst) else {
        return Ok(false);
    };
    if chain.arms.len() < MIN_ARMS {
        return Ok(false);
    }

    let sel_width = chain.sel_expr.comptime().r#type.total_width().unwrap_or(0);
    if sel_width == 0 || sel_width > 32 {
        return Ok(false);
    }
    let sel_mask_u: u64 = if sel_width >= 64 {
        !0u64
    } else {
        (1u64 << sel_width) - 1
    };
    for arm in &chain.arms {
        let ok = match arm.cond {
            ArmCondition::Eq(k) => sel_width >= 64 || (k >> sel_width) == 0,
            ArmCondition::Range(lo, hi) => {
                lo.is_none_or(|v| sel_width >= 64 || v <= sel_mask_u + 1)
                    && hi.is_none_or(|v| sel_width >= 64 || v <= sel_mask_u + 1)
            }
        };
        if !ok {
            return Ok(false);
        }
    }

    // Match signal per arm: Eq → AND-reduce bit literals, Range → cached
    // `sel >= K` so worklist CSE can share thresholds across arms.
    let sel_nets = synthesize_expr(ctx, chain.sel_expr, current, sel_width)?;
    let mut matches: Vec<NetId> = Vec::with_capacity(chain.arms.len());
    let mut not_cache: HashMap<NetId, NetId> = HashMap::new();
    let mut ge_cache: HashMap<u64, NetId> = HashMap::new();
    for arm in &chain.arms {
        let m = match arm.cond {
            ArmCondition::Eq(match_const) => {
                let mut eq_bits = Vec::with_capacity(sel_width);
                for (i, &net) in sel_nets.iter().enumerate().take(sel_width) {
                    let bit_one = (match_const >> i) & 1 == 1;
                    if bit_one {
                        eq_bits.push(net);
                    } else {
                        let inv = *not_cache
                            .entry(net)
                            .or_insert_with(|| ctx.add_cell(CellKind::Not, vec![net]));
                        eq_bits.push(inv);
                    }
                }
                reduce_and(ctx, &eq_bits)
            }
            ArmCondition::Range(lo, hi) => synth_range_match(
                ctx,
                &sel_nets,
                lo,
                hi,
                sel_mask_u,
                &mut ge_cache,
                &mut not_cache,
            )?,
        };
        matches.push(m);
    }

    // Snapshot `current` so each arm synthesises in its own scope and
    // provides the fallback value for vars the arm doesn't write.
    let base = current.clone();

    let mut arm_currents: Vec<HashMap<air::VarId, Vec<NetId>>> =
        Vec::with_capacity(chain.arms.len());
    for arm in &chain.arms {
        let mut local = base.clone();
        process_statements(ctx, arm.body, &mut local)?;
        arm_currents.push(local);
    }
    let mut default_current = base.clone();
    process_statements(ctx, chain.default_body, &mut default_current)?;

    // Variables touched by any arm or the default. Arm-local `let`
    // bindings are filtered out (not present in `base`) so they don't
    // leak into the parent scope.
    let mut changed_vars: BTreeSet<air::VarId> = BTreeSet::new();
    let probe = |vars: &mut BTreeSet<air::VarId>, m: &HashMap<air::VarId, Vec<NetId>>| {
        for (var_id, nets) in m {
            if !base.contains_key(var_id) {
                continue;
            }
            if base.get(var_id) != Some(nets) {
                vars.insert(*var_id);
            }
        }
    };
    for arm_cur in &arm_currents {
        probe(&mut changed_vars, arm_cur);
    }
    probe(&mut changed_vars, &default_current);

    if changed_vars.is_empty() {
        // Returning true would swallow the statement; fall through so the
        // caller runs the usual (no-op) merge_branches.
        return Ok(false);
    }

    let mut default_active: Option<NetId> = None;

    // Narrow dense sels (≤ 4) always qualify for 2-level minimisation;
    // wider sels need `'x` don't-cares to beat match-based SOP, since
    // unused slots otherwise pin to default and inflate every cube.
    const MIN_WIDTH: usize = 1;
    const NARROW_WIDTH: usize = 4;
    const MAX_SOP_WIDTH: usize = 8;
    let width_ok = (MIN_WIDTH..=MAX_SOP_WIDTH).contains(&sel_width);

    // Pre-scan arm and default bodies for `'x` bits. Net-level state
    // collapses `'x` to CONST0 (indistinguishable from an explicit 0), so
    // the analyzer expression is the only place we can spot them.
    let default_dc = if width_ok {
        collect_body_dc(chain.default_body)
    } else {
        HashMap::new()
    };
    let arm_dc: Vec<HashMap<air::VarId, u64>> = if width_ok {
        chain
            .arms
            .iter()
            .map(|arm| collect_body_dc(arm.body))
            .collect()
    } else {
        Vec::new()
    };
    let has_dc =
        default_dc.values().any(|&m| m != 0) || arm_dc.iter().any(|m| m.values().any(|&v| v != 0));
    let sop_eligible = width_ok && (sel_width <= NARROW_WIDTH || has_dc);

    // F5 minimisation needs concrete per-sel-value constraints; it only
    // supports arms with equality conditions for now. Skip when any arm is
    // a Range (the fold still fires via match-based SOP above).
    let all_eq = chain
        .arms
        .iter()
        .all(|a| matches!(a.cond, ArmCondition::Eq(_)));
    let f5_eligible = sop_eligible && all_eq;
    let matched_sels: HashSet<u64> = if f5_eligible {
        chain
            .arms
            .iter()
            .filter_map(|a| match a.cond {
                ArmCondition::Eq(k) => Some(k),
                ArmCondition::Range(_, _) => None,
            })
            .collect()
    } else {
        HashSet::new()
    };
    let sel_universe_mask: u64 = if sel_width >= 64 {
        !0u64
    } else {
        (1u64 << sel_width) - 1
    };

    for var_id in changed_vars {
        let base_nets = base.get(&var_id).cloned().unwrap_or_default();
        let var_width = ctx
            .variables
            .get(&var_id)
            .map(|s| s.nets.len())
            .unwrap_or(0);
        let mut out_nets = Vec::with_capacity(var_width);
        for bit in 0..var_width {
            let base_bit = base_nets.get(bit).copied().unwrap_or(NET_CONST0);

            if sop_eligible
                && let Some(net) = try_minimize_bit(
                    ctx,
                    &sel_nets,
                    sel_width,
                    sel_universe_mask,
                    &matched_sels,
                    &chain.arms,
                    &arm_currents,
                    &default_current,
                    &arm_dc,
                    &default_dc,
                    var_id,
                    bit,
                    base_bit,
                    &mut not_cache,
                )
            {
                out_nets.push(net);
                continue;
            }

            let arm_bits: Vec<NetId> = arm_currents
                .iter()
                .map(|arm_cur| {
                    arm_cur
                        .get(&var_id)
                        .and_then(|v| v.get(bit))
                        .copied()
                        .unwrap_or(base_bit)
                })
                .collect();
            let default_bit = default_current
                .get(&var_id)
                .and_then(|v| v.get(bit))
                .copied()
                .unwrap_or(base_bit);
            out_nets.push(emit_sop_bit(
                ctx,
                &matches,
                &arm_bits,
                default_bit,
                &mut default_active,
            ));
        }
        current.insert(var_id, out_nets);
    }
    Ok(true)
}

/// Per-bit 2-level minimization entry point used by `try_fold_case_like`.
/// Returns `Some(net)` when the bit's value is a pure function of `sel`
/// (every arm + the default contribute a compile-time 0 / 1 / don't-care);
/// otherwise returns `None` so the caller falls back to match-gated SOP.
#[allow(clippy::too_many_arguments)]
fn try_minimize_bit(
    ctx: &mut ConvContext,
    sel_nets: &[NetId],
    sel_width: usize,
    sel_universe_mask: u64,
    matched_sels: &HashSet<u64>,
    arms: &[CaseArm<'_>],
    arm_currents: &[HashMap<air::VarId, Vec<NetId>>],
    default_current: &HashMap<air::VarId, Vec<NetId>>,
    arm_dc: &[HashMap<air::VarId, u64>],
    default_dc: &HashMap<air::VarId, u64>,
    var_id: air::VarId,
    bit: usize,
    base_bit: NetId,
    not_cache: &mut HashMap<NetId, NetId>,
) -> Option<NetId> {
    let bit_is_dc = |dc_map: &HashMap<air::VarId, u64>| {
        dc_map.get(&var_id).is_some_and(|m| (m >> bit) & 1 == 1)
    };

    let mut on: Vec<u64> = Vec::new();
    let mut off: Vec<u64> = Vec::new();
    for (i, arm_cur) in arm_currents.iter().enumerate() {
        let arm_bit = arm_cur
            .get(&var_id)
            .and_then(|v| v.get(bit))
            .copied()
            .unwrap_or(base_bit);
        // F5 is only invoked when every arm is an Eq — caller guarantees
        // this via `f5_eligible`, so this is an internal invariant.
        let ArmCondition::Eq(k) = arms[i].cond else {
            return None;
        };
        // `'x` bit in an arm: this arm's sel value is DC — don't constrain it.
        if bit_is_dc(&arm_dc[i]) {
            continue;
        }
        match arm_bit {
            NET_CONST0 => off.push(k),
            NET_CONST1 => on.push(k),
            _ => return None,
        }
    }
    let default_bit = default_current
        .get(&var_id)
        .and_then(|v| v.get(bit))
        .copied()
        .unwrap_or(base_bit);
    let default_bit_is_dc = bit_is_dc(default_dc);
    let default_class: Option<bool> = if default_bit_is_dc {
        None
    } else {
        match default_bit {
            NET_CONST0 => Some(false),
            NET_CONST1 => Some(true),
            _ => return None,
        }
    };

    // Distribute sel values not covered by any arm. Default `'x` bits let
    // every unused sel slot be don't-care; an explicit default value pins
    // them to ON or OFF.
    if let Some(default_is_one) = default_class {
        let mut s: u64 = 0;
        loop {
            if !matched_sels.contains(&s) {
                if default_is_one {
                    on.push(s);
                } else {
                    off.push(s);
                }
            }
            if s == sel_universe_mask {
                break;
            }
            s += 1;
        }
    }

    if on.is_empty() {
        return Some(NET_CONST0);
    }
    if off.is_empty() {
        return Some(NET_CONST1);
    }

    let cubes = minimize_sop_cubes(&on, &off, sel_width);
    if cubes.is_empty() {
        return Some(NET_CONST0);
    }
    let mut cube_nets = Vec::with_capacity(cubes.len());
    for (v, m) in cubes {
        cube_nets.push(emit_cube(ctx, sel_nets, v, m, not_cache));
    }
    Some(if cube_nets.len() == 1 {
        cube_nets[0]
    } else {
        reduce_or(ctx, &cube_nets)
    })
}

/// Build a match signal for an `ArmCondition::Range(lo, hi)` arm:
/// `match = (sel >= lo) & !(sel >= hi)`. Each `sel >= K` is synthesised
/// once per unique threshold via `ge_cache` so adjacent arms in the chain
/// share their compare chain — this is the key win on BCD decoders like
/// cnt60 where ge_K's arise at both `K <= sel` in one arm and as a
/// bound-check in the next.
fn synth_range_match(
    ctx: &mut ConvContext,
    sel_nets: &[NetId],
    lo: Option<u64>,
    hi: Option<u64>,
    sel_mask: u64,
    ge_cache: &mut HashMap<u64, NetId>,
    not_cache: &mut HashMap<NetId, NetId>,
) -> Result<NetId, SynthesizerError> {
    // `sel >= K` when K = 0 is tautology; when K > universe it is false.
    let synth_ge = |ctx: &mut ConvContext,
                    ge_cache: &mut HashMap<u64, NetId>,
                    k: u64|
     -> Result<NetId, SynthesizerError> {
        if k == 0 {
            return Ok(NET_CONST1);
        }
        if k > sel_mask {
            return Ok(NET_CONST0);
        }
        if let Some(&n) = ge_cache.get(&k) {
            return Ok(n);
        }
        let k_nets = (0..sel_nets.len())
            .map(|i| {
                if (k >> i) & 1 == 1 {
                    NET_CONST1
                } else {
                    NET_CONST0
                }
            })
            .collect::<Vec<_>>();
        let ge = arith::compare(
            ctx,
            sel_nets,
            &k_nets,
            veryl_analyzer::ir::Op::GreaterEq,
            false,
        )?;
        ge_cache.insert(k, ge);
        Ok(ge)
    };

    let mut terms: Vec<NetId> = Vec::new();
    if let Some(lo_k) = lo {
        let ge_lo = synth_ge(ctx, ge_cache, lo_k)?;
        if ge_lo != NET_CONST1 {
            terms.push(ge_lo);
        }
    }
    if let Some(hi_k) = hi {
        let ge_hi = synth_ge(ctx, ge_cache, hi_k)?;
        let lt_hi = match ge_hi {
            NET_CONST1 => NET_CONST0,
            NET_CONST0 => NET_CONST1,
            n => *not_cache
                .entry(n)
                .or_insert_with(|| ctx.add_cell(CellKind::Not, vec![n])),
        };
        if lt_hi != NET_CONST1 {
            terms.push(lt_hi);
        }
    }
    Ok(match terms.len() {
        0 => NET_CONST1,
        1 => terms[0],
        _ => reduce_and(ctx, &terms),
    })
}

/// Flattened view of a case-like If chain: one record per arm plus the
/// default-arm body. Arm bodies are borrowed from the analyzer IR and
/// synthesised on demand, so any statement shape (const assign, nested
/// case, ternary-of-constants, wire reference) is acceptable.
/// Walk a body of (typically case-arm) statements and return the
/// accumulated `'x`/`'z` don't-care mask per variable whose assignment
/// rhs is a literal. Only flat `var = <literal>` (no bit-select, no
/// array index) contributes — slice/index assignments are conservatively
/// ignored (kept as explicit zeros). The map is consumed by
/// `try_minimize_bit` to relax ON/OFF constraints on DC bits.
fn collect_body_dc(stmts: &[Statement]) -> HashMap<air::VarId, u64> {
    use veryl_analyzer::value::Value;
    let mut dc: HashMap<air::VarId, u64> = HashMap::new();
    for st in stmts {
        if let Statement::Assign(a) = st {
            let Some(factor) = (match &a.expr {
                air::Expression::Term(f) => Some(f),
                _ => None,
            }) else {
                continue;
            };
            let ct = match factor.as_ref() {
                air::Factor::Value(ct) => ct,
                _ => continue,
            };
            let Ok(value) = ct.get_value() else { continue };
            let mask_xz = match value {
                Value::U64(v) => v.mask_xz,
                Value::BigUint(_) => continue,
            };
            if mask_xz == 0 {
                continue;
            }
            for d in &a.dst {
                // Skip anything with bit-select or array-index — we'd have to
                // shift mask_xz to match the slice range, which isn't worth
                // the complexity for the handful of case defaults that use
                // partial-width `'x` writes.
                if !d.select.is_empty() || d.index.dimension() != 0 {
                    continue;
                }
                *dc.entry(d.id).or_insert(0) |= mask_xz;
            }
        }
    }
    dc
}

struct CaseChain<'a> {
    /// The selector expression every `cond` compares against.
    sel_expr: &'a air::Expression,
    /// Ordered by source position.
    arms: Vec<CaseArm<'a>>,
    /// Body of the final `else {...}` (empty slice when the source had no
    /// explicit default — unassigned bits then keep their base value).
    default_body: &'a [Statement],
}

struct CaseArm<'a> {
    /// Condition under which this arm fires (in its own right, pre-cascade).
    cond: ArmCondition,
    /// Statements executed when the arm fires.
    body: &'a [Statement],
}

/// How an arm's own condition relates to `sel`. `Eq` covers the classic
/// `if sel == K` case-statement lowering; `Range(lo, hi)` covers range
/// chains like `if sel < 10 { } else if 10 <= sel && sel < 20 { } ...`
/// where each arm's cond is a half-open interval over sel.
#[derive(Clone, Copy, Debug)]
enum ArmCondition {
    Eq(u64),
    /// `lo` is the inclusive lower bound (`None` = open / `-inf`), `hi` the
    /// exclusive upper bound (`None` = open / `+inf`). At least one bound
    /// must be `Some`.
    Range(Option<u64>, Option<u64>),
}

/// Walk a nested-If chain and return a `CaseChain` when every `cond`
/// tests the same selector (equality or half-open range). Arm bodies are
/// borrowed verbatim — the SOP combiner re-processes them via
/// `process_statements`, so no statement-shape check is needed here.
fn collect_case_chain(ifst: &air::IfStatement) -> Option<CaseChain<'_>> {
    let (sel_expr, first_cond) = extract_chain_cond(&ifst.cond, None)?;

    let mut arms = vec![CaseArm {
        cond: first_cond,
        body: &ifst.true_side,
    }];

    let mut tail = &ifst.false_side;
    loop {
        if tail.is_empty() {
            return Some(CaseChain {
                sel_expr,
                arms,
                default_body: &[],
            });
        }
        if tail.len() == 1 && matches!(&tail[0], Statement::If(_)) {
            let Statement::If(next) = &tail[0] else {
                unreachable!()
            };
            let (next_sel, next_cond) = extract_chain_cond(&next.cond, Some(sel_expr))?;
            if !expression_eq(sel_expr, next_sel) {
                return None;
            }
            arms.push(CaseArm {
                cond: next_cond,
                body: &next.true_side,
            });
            tail = &next.false_side;
        } else {
            return Some(CaseChain {
                sel_expr,
                arms,
                default_body: tail,
            });
        }
    }
}

/// Accept either `sel == K` (Eq arm) or a range condition like `sel < K`,
/// `K <= sel`, or `K1 <= sel && sel < K2`. When `anchor` is provided the
/// detected `sel` must match it structurally (later arms in a chain).
fn extract_chain_cond<'a>(
    expr: &'a air::Expression,
    anchor: Option<&air::Expression>,
) -> Option<(&'a air::Expression, ArmCondition)> {
    if let Some((sel, k)) = extract_eq_with_const(expr)
        && anchor.is_none_or(|a| expression_eq(a, sel))
    {
        return Some((sel, ArmCondition::Eq(k)));
    }
    if let Some((sel, lo, hi)) = extract_range_cond(expr, anchor) {
        return Some((sel, ArmCondition::Range(lo, hi)));
    }
    None
}

/// Match a half-open range cond over `sel`. Returns `(sel, lo, hi)` with
/// `lo` inclusive (None = open), `hi` exclusive (None = open). Handles:
///   `sel < K`             → (None, Some(K))
///   `sel >= K` / `K <= sel` → (Some(K), None)
///   `K1 <= sel && sel < K2` (either conjunct order) → (Some(K1), Some(K2))
fn extract_range_cond<'a>(
    expr: &'a air::Expression,
    anchor: Option<&air::Expression>,
) -> Option<(&'a air::Expression, Option<u64>, Option<u64>)> {
    use veryl_analyzer::ir::{Expression, Op};
    // Combined `lo && hi`.
    if let Expression::Binary(l, Op::LogicAnd, r, _) = expr {
        let (sl, lo_l, hi_l) = extract_side_cond(l, anchor)?;
        let (sr, lo_r, hi_r) = extract_side_cond(r, Some(sl))?;
        if !expression_eq(sl, sr) {
            return None;
        }
        // Merge — at most one of each should be specified across the two sides.
        let lo = lo_l.or(lo_r);
        let hi = hi_l.or(hi_r);
        if lo_l.is_some() && lo_r.is_some() || hi_l.is_some() && hi_r.is_some() {
            return None;
        }
        if lo.is_none() && hi.is_none() {
            return None;
        }
        return Some((sl, lo, hi));
    }
    let (sel, lo, hi) = extract_side_cond(expr, anchor)?;
    Some((sel, lo, hi))
}

fn extract_side_cond<'a>(
    expr: &'a air::Expression,
    anchor: Option<&air::Expression>,
) -> Option<(&'a air::Expression, Option<u64>, Option<u64>)> {
    use veryl_analyzer::ir::{Expression, Op};
    let Expression::Binary(x, op, y, _) = expr else {
        return None;
    };
    // Figure out which side is the selector vs the constant.
    let (sel, k, swapped) = if let Some(k) = try_constant(y) {
        (x.as_ref(), k, false)
    } else if let Some(k) = try_constant(x) {
        (y.as_ref(), k, true)
    } else {
        return None;
    };
    if anchor.is_some_and(|a| !expression_eq(a, sel)) {
        return None;
    }
    // Normalise "sel OP K" form (swap op when the analyzer wrote `K OP sel`).
    let op = if swapped {
        match op {
            Op::Less => Op::Greater,
            Op::LessEq => Op::GreaterEq,
            Op::Greater => Op::Less,
            Op::GreaterEq => Op::LessEq,
            other => *other,
        }
    } else {
        *op
    };
    match op {
        Op::Less => Some((sel, None, Some(k))),
        Op::LessEq => Some((sel, None, Some(k.saturating_add(1)))),
        Op::Greater => Some((sel, Some(k.saturating_add(1)), None)),
        Op::GreaterEq => Some((sel, Some(k), None)),
        _ => None,
    }
}

/// Shallow structural equality on analyzer expressions — good enough to
/// confirm that every `if sel == c` level uses the same selector. Full
/// equality would compare token ranges which vary across clones, so we
/// recurse only into the parts that would differ for distinct variables.
fn expression_eq(a: &air::Expression, b: &air::Expression) -> bool {
    use veryl_analyzer::ir::{Expression, Factor};
    match (a, b) {
        (Expression::Term(fa), Expression::Term(fb)) => match (fa.as_ref(), fb.as_ref()) {
            (Factor::Variable(ia, idxa, sela, _), Factor::Variable(ib, idxb, selb, _)) => {
                ia == ib
                    && format!("{:?}", idxa) == format!("{:?}", idxb)
                    && format!("{:?}", sela) == format!("{:?}", selb)
            }
            _ => false,
        },
        _ => false,
    }
}

/// If `expr` is `sel == const` (or `const == sel`), return the selector
/// side and the constant value. Accepts both `==` and `==?` because the
/// analyzer lowers SystemVerilog `case` to the wildcard form; for the
/// fully-specified constants we care about the two operators produce the
/// same gate-level result.
fn extract_eq_with_const(expr: &air::Expression) -> Option<(&air::Expression, u64)> {
    use veryl_analyzer::ir::{Expression, Op};
    let Expression::Binary(x, op, y, _) = expr else {
        return None;
    };
    if !matches!(op, Op::Eq | Op::EqWildcard) {
        return None;
    }
    if let Some(c) = try_constant(y) {
        Some((x, c))
    } else if let Some(c) = try_constant(x) {
        Some((y, c))
    } else {
        None
    }
}
