use std::collections::HashMap;

use veryl_analyzer::ir::{self as air, Expression, Factor, Op};

use crate::conv::ConvContext;
use crate::conv::arith;
use crate::ir::{CellKind, NetId};
use crate::synthesizer_error::{SynthesizerError, UnsupportedKind};

/// Returns `target_width` nets in LSB-first order. Narrower results are
/// zero- or sign-extended; wider results are truncated.
///
/// `target_width` also acts as the SV context width: context-determined
/// sub-expressions (add/sub/bitwise/ternary-arms) are evaluated at this
/// width rather than the analyzer's `expr_context.width`, which avoids
/// synthesizing a 32-bit adder just because a literal `1` defaulted to 32-bit.
pub(crate) fn synthesize_expr(
    ctx: &mut ConvContext,
    expr: &Expression,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
    target_width: usize,
) -> Result<Vec<NetId>, SynthesizerError> {
    if let Some(constant) = try_constant(expr) {
        return Ok(build_constant(constant, target_width));
    }

    let raw = synth_raw(ctx, expr, current, target_width)?;
    Ok(resize(raw, target_width, expr_signed(expr)))
}

fn expr_signed(expr: &Expression) -> bool {
    expr.comptime().r#type.signed
}

fn try_constant(expr: &Expression) -> Option<u64> {
    if let Expression::Term(factor) = expr
        && let Factor::Value(ct) = factor.as_ref()
    {
        let value = ct.get_value().ok()?;
        return value.to_u64();
    }
    None
}

fn build_constant(value: u64, width: usize) -> Vec<NetId> {
    let mut out = Vec::with_capacity(width);
    for i in 0..width {
        let bit_set = i < 64 && (value >> i) & 1 == 1;
        out.push(if bit_set {
            crate::ir::NET_CONST1
        } else {
            crate::ir::NET_CONST0
        });
    }
    out
}

/// Bit-level expansion of an analyzer `Value` into net IDs, handling both the
/// U64 and BigUint variants. Missing high bits are zero.
fn value_to_nets(value: &veryl_analyzer::value::Value, width: usize) -> Vec<NetId> {
    use veryl_analyzer::value::Value;
    let mut out = Vec::with_capacity(width);
    match value {
        Value::U64(v) => {
            for i in 0..width {
                let bit = i < 64 && (v.payload >> i) & 1 == 1;
                out.push(if bit {
                    crate::ir::NET_CONST1
                } else {
                    crate::ir::NET_CONST0
                });
            }
        }
        Value::BigUint(v) => {
            let payload = v.payload();
            for i in 0..width {
                let bit = payload.bit(i as u64);
                out.push(if bit {
                    crate::ir::NET_CONST1
                } else {
                    crate::ir::NET_CONST0
                });
            }
        }
    }
    out
}

fn resize(mut nets: Vec<NetId>, target: usize, signed: bool) -> Vec<NetId> {
    use std::cmp::Ordering;
    match nets.len().cmp(&target) {
        Ordering::Equal => nets,
        Ordering::Greater => {
            nets.truncate(target);
            nets
        }
        Ordering::Less => {
            let pad = if signed && !nets.is_empty() {
                *nets.last().unwrap()
            } else {
                crate::ir::NET_CONST0
            };
            nets.resize(target, pad);
            nets
        }
    }
}

fn synth_raw(
    ctx: &mut ConvContext,
    expr: &Expression,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
    context_width: usize,
) -> Result<Vec<NetId>, SynthesizerError> {
    let ctx_width = context_width.max(1);
    match expr {
        Expression::Term(factor) => synth_factor(ctx, factor, current),
        Expression::Unary(op, inner, _comptime) => synth_unary(ctx, *op, inner, current, ctx_width),
        Expression::Binary(x, op, y, comptime) => {
            let signed = comptime.expr_context.signed;
            synth_binary(ctx, x, *op, y, current, ctx_width, signed)
        }
        Expression::Ternary(cond, a, b, _comptime) => {
            let sel = synthesize_expr(ctx, cond, current, 1)?[0];
            let true_nets = synthesize_expr(ctx, a, current, ctx_width)?;
            let false_nets = synthesize_expr(ctx, b, current, ctx_width)?;
            let mut out = Vec::with_capacity(ctx_width);
            for i in 0..ctx_width {
                out.push(ctx.add_cell(CellKind::Mux2, vec![sel, false_nets[i], true_nets[i]]));
            }
            Ok(out)
        }
        Expression::Concatenation(items, _) => {
            // SV `{a, b, c}` puts the leftmost operand in the high bits. Our
            // nets are LSB-first, so the last item becomes the low bits and
            // we glue parts in reverse.
            let mut parts: Vec<Vec<NetId>> = Vec::new();
            for (e, repeat) in items {
                let w = e.comptime().r#type.total_width().unwrap_or(0);
                let mut bits = synthesize_expr(ctx, e, current, w)?;
                if let Some(rep) = repeat {
                    let rep_v = rep
                        .eval_value(&mut veryl_analyzer::Context::default())
                        .and_then(|v| v.to_usize())
                        .ok_or_else(|| {
                            SynthesizerError::internal(
                                "non-constant concatenation repeat reached synthesizer",
                            )
                        })?;
                    let single = bits.clone();
                    for _ in 1..rep_v {
                        bits.extend(single.iter().copied());
                    }
                }
                parts.push(bits);
            }
            let mut out = Vec::new();
            for part in parts.into_iter().rev() {
                out.extend(part);
            }
            Ok(out)
        }
        Expression::ArrayLiteral(_, _) | Expression::StructConstructor(_, _, _) => {
            // Analyzer folds these into element-wise Assign statements.
            Err(SynthesizerError::internal(
                "array or struct literal reached synthesizer",
            ))
        }
    }
}

fn synth_factor(
    ctx: &mut ConvContext,
    factor: &Factor,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<Vec<NetId>, SynthesizerError> {
    match factor {
        Factor::Value(ct) => {
            let value = ct.get_value().map_err(|_| {
                SynthesizerError::unsupported(UnsupportedKind::NonNumericValueFactor, &ct.token)
            })?;
            let width = ct.r#type.total_width().unwrap_or(value.width());
            Ok(value_to_nets(value, width))
        }
        Factor::Variable(id, index, select, ct) => {
            // The `current` map shadows the persistent nets within a block
            // so that `a = a + 1`-style reads pick up the previous value.
            let src_nets = current
                .get(id)
                .cloned()
                .or_else(|| ctx.variables.get(id).map(|s| s.nets.clone()))
                .ok_or_else(|| {
                    SynthesizerError::internal(format!("reference to unknown variable {}", id))
                })?;

            let (scalar_width, shape) = {
                let slot = ctx.variables.get(id).ok_or_else(|| {
                    SynthesizerError::internal(format!("reference to unknown variable {}", id))
                })?;
                (slot.scalar_width, slot.shape.clone())
            };

            let element_nets: Vec<NetId> = if index.0.is_empty() {
                src_nets
            } else if index.is_const() {
                let mut eval_ctx = veryl_analyzer::Context::default();
                let indices = index.eval_value(&mut eval_ctx).ok_or_else(|| {
                    SynthesizerError::dynamic_select(format!("array index on {}", id), &ct.token)
                })?;
                let flat = shape.calc_index(&indices).ok_or_else(|| {
                    SynthesizerError::internal(format!(
                        "array index out of range for {} (dims {})",
                        id,
                        shape.dims()
                    ))
                })?;
                let start = flat * scalar_width;
                src_nets[start..start + scalar_width].to_vec()
            } else {
                if shape.dims() != 1 {
                    return Err(SynthesizerError::unsupported(
                        UnsupportedKind::DynamicMultiDimIndex {
                            what: format!("variable {}", id),
                        },
                        &ct.token,
                    ));
                }
                let num_elements = shape.total().unwrap_or(0);
                if num_elements == 0 {
                    return Err(SynthesizerError::internal(format!(
                        "{} has zero elements",
                        id
                    )));
                }
                let idx_bits = arith::index_bits_for(num_elements);
                let idx_nets = synthesize_expr(ctx, &index.0[0], current, idx_bits)?;
                let elements: Vec<Vec<NetId>> = (0..num_elements)
                    .map(|k| src_nets[k * scalar_width..(k + 1) * scalar_width].to_vec())
                    .collect();
                arith::dynamic_mux_tree(ctx, &elements, &idx_nets)
            };

            // For struct/union members, the analyzer encodes the member range
            // either in `select` (base-coordinate range, non-empty) or in
            // `part_select` (path metadata, with select empty). Do NOT apply
            // both — they represent the same slice.
            if select.is_empty() {
                return apply_part_select(&element_nets, ct, id);
            }
            if select.is_const() && !select.is_range() {
                let mut eval_ctx = veryl_analyzer::Context::default();
                let (high, low) = select
                    .eval_value(&mut eval_ctx, &ct.r#type, false)
                    .ok_or_else(|| {
                        SynthesizerError::dynamic_select(format!("variable {}", id), &ct.token)
                    })?;
                if high >= element_nets.len() {
                    return Err(SynthesizerError::internal(format!(
                        "bit select out of range: {}..={} of {}-bit signal",
                        low,
                        high,
                        element_nets.len()
                    )));
                }
                return Ok(element_nets[low..=high].to_vec());
            }
            if select.is_range() {
                if !select.is_const() {
                    return Err(SynthesizerError::unsupported(
                        UnsupportedKind::DynamicRangeSelect {
                            what: format!("variable {}", id),
                        },
                        &ct.token,
                    ));
                }
                if let Some((_, end)) = &select.1
                    && !end.comptime().is_const
                {
                    return Err(SynthesizerError::unsupported(
                        UnsupportedKind::DynamicRangeEnd {
                            what: format!("variable {}", id),
                        },
                        &ct.token,
                    ));
                }
                let mut eval_ctx = veryl_analyzer::Context::default();
                let (high, low) = select
                    .eval_value(&mut eval_ctx, &ct.r#type, false)
                    .ok_or_else(|| {
                        SynthesizerError::dynamic_select(format!("variable {}", id), &ct.token)
                    })?;
                if high >= element_nets.len() {
                    return Err(SynthesizerError::internal(format!(
                        "bit select out of range: {}..={} of {}-bit signal",
                        low,
                        high,
                        element_nets.len()
                    )));
                }
                return Ok(element_nets[low..=high].to_vec());
            }
            if select.0.len() != 1 {
                return Err(SynthesizerError::unsupported(
                    UnsupportedKind::MultiDimDynamicSelect {
                        what: format!("variable {}", id),
                    },
                    &ct.token,
                ));
            }
            let idx_bits = arith::index_bits_for(element_nets.len());
            let idx_nets = synthesize_expr(ctx, &select.0[0], current, idx_bits)?;
            let elements: Vec<Vec<NetId>> = element_nets.iter().map(|&n| vec![n]).collect();
            Ok(arith::dynamic_mux_tree(ctx, &elements, &idx_nets))
        }
        Factor::FunctionCall(call) => synth_function_call(ctx, call, current),
        Factor::SystemFunctionCall(call) => Err(SynthesizerError::unsupported(
            UnsupportedKind::SystemFunctionCall,
            &call.comptime.token,
        )),
        Factor::Anonymous(ct) => {
            // `_` placeholder — drive with all-zero at the declared width.
            let width = ct.r#type.total_width().unwrap_or(0);
            Ok(vec![crate::ir::NET_CONST0; width])
        }
        Factor::Unknown(ct) => Err(SynthesizerError::unsupported(
            UnsupportedKind::UnknownFactor,
            &ct.token,
        )),
    }
}

fn synth_unary(
    ctx: &mut ConvContext,
    op: Op,
    inner: &Expression,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
    result_width: usize,
) -> Result<Vec<NetId>, SynthesizerError> {
    match op {
        Op::BitNot => {
            // Context-determined: inner evaluates at our result width.
            let xs = synthesize_expr(ctx, inner, current, result_width)?;
            let mut out = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(ctx.add_cell(CellKind::Not, vec![x]));
            }
            Ok(out)
        }
        Op::LogicNot => {
            let w = inner.comptime().expr_context.width.max(1);
            let xs = synthesize_expr(ctx, inner, current, w)?;
            let or = reduce_or(ctx, &xs);
            let inv = ctx.add_cell(CellKind::Not, vec![or]);
            Ok(resize(vec![inv], result_width, false))
        }
        Op::BitAnd => {
            let w = inner.comptime().expr_context.width.max(1);
            let xs = synthesize_expr(ctx, inner, current, w)?;
            Ok(resize(vec![reduce_and(ctx, &xs)], result_width, false))
        }
        Op::BitOr => {
            let w = inner.comptime().expr_context.width.max(1);
            let xs = synthesize_expr(ctx, inner, current, w)?;
            Ok(resize(vec![reduce_or(ctx, &xs)], result_width, false))
        }
        Op::BitXor => {
            let w = inner.comptime().expr_context.width.max(1);
            let xs = synthesize_expr(ctx, inner, current, w)?;
            Ok(resize(vec![reduce_xor(ctx, &xs)], result_width, false))
        }
        Op::BitNand => {
            let w = inner.comptime().expr_context.width.max(1);
            let xs = synthesize_expr(ctx, inner, current, w)?;
            let a = reduce_and(ctx, &xs);
            let n = ctx.add_cell(CellKind::Not, vec![a]);
            Ok(resize(vec![n], result_width, false))
        }
        Op::BitNor => {
            let w = inner.comptime().expr_context.width.max(1);
            let xs = synthesize_expr(ctx, inner, current, w)?;
            let o = reduce_or(ctx, &xs);
            let n = ctx.add_cell(CellKind::Not, vec![o]);
            Ok(resize(vec![n], result_width, false))
        }
        Op::BitXnor => {
            let w = inner.comptime().expr_context.width.max(1);
            let xs = synthesize_expr(ctx, inner, current, w)?;
            let x = reduce_xor(ctx, &xs);
            let n = ctx.add_cell(CellKind::Not, vec![x]);
            Ok(resize(vec![n], result_width, false))
        }
        Op::Add => synthesize_expr(ctx, inner, current, result_width),
        Op::Sub => {
            // Two's complement: -x = 0 - x.
            let xs = synthesize_expr(ctx, inner, current, result_width)?;
            let zero = vec![crate::ir::NET_CONST0; result_width];
            arith::ripple_sub(ctx, &zero, &xs)
        }
        _ => Err(SynthesizerError::internal(format!(
            "unary operator {:?} reached synthesizer",
            op
        ))),
    }
}

fn synth_binary(
    ctx: &mut ConvContext,
    x: &Expression,
    op: Op,
    y: &Expression,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
    result_width: usize,
    signed: bool,
) -> Result<Vec<NetId>, SynthesizerError> {
    match op {
        Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor => {
            let xs = synthesize_expr(ctx, x, current, result_width)?;
            let ys = synthesize_expr(ctx, y, current, result_width)?;
            let mut out = Vec::with_capacity(result_width);
            for i in 0..result_width {
                let kind = match op {
                    Op::BitAnd => CellKind::And2,
                    Op::BitOr => CellKind::Or2,
                    Op::BitXor => CellKind::Xor2,
                    Op::BitXnor => CellKind::Xnor2,
                    _ => unreachable!(),
                };
                out.push(ctx.add_cell(kind, vec![xs[i], ys[i]]));
            }
            Ok(out)
        }
        Op::LogicAnd => {
            let xs = synthesize_expr(ctx, x, current, 1)?;
            let ys = synthesize_expr(ctx, y, current, 1)?;
            let a = ctx.add_cell(CellKind::And2, vec![xs[0], ys[0]]);
            Ok(resize(vec![a], result_width, false))
        }
        Op::LogicOr => {
            let xs = synthesize_expr(ctx, x, current, 1)?;
            let ys = synthesize_expr(ctx, y, current, 1)?;
            let o = ctx.add_cell(CellKind::Or2, vec![xs[0], ys[0]]);
            Ok(resize(vec![o], result_width, false))
        }
        Op::Add => {
            let xs = synthesize_expr(ctx, x, current, result_width)?;
            let ys = synthesize_expr(ctx, y, current, result_width)?;
            arith::ripple_add(ctx, &xs, &ys, crate::ir::NET_CONST0)
        }
        Op::Sub => {
            let xs = synthesize_expr(ctx, x, current, result_width)?;
            let ys = synthesize_expr(ctx, y, current, result_width)?;
            arith::ripple_sub(ctx, &xs, &ys)
        }
        Op::Eq => {
            let w = x
                .comptime()
                .r#type
                .total_width()
                .unwrap_or(result_width)
                .max(y.comptime().r#type.total_width().unwrap_or(result_width))
                .max(1);
            let xs = synthesize_expr(ctx, x, current, w)?;
            let ys = synthesize_expr(ctx, y, current, w)?;
            let eq = arith::equal(ctx, &xs, &ys);
            Ok(resize(vec![eq], result_width, false))
        }
        Op::Ne => {
            let w = x
                .comptime()
                .r#type
                .total_width()
                .unwrap_or(result_width)
                .max(y.comptime().r#type.total_width().unwrap_or(result_width))
                .max(1);
            let xs = synthesize_expr(ctx, x, current, w)?;
            let ys = synthesize_expr(ctx, y, current, w)?;
            let eq = arith::equal(ctx, &xs, &ys);
            let ne = ctx.add_cell(CellKind::Not, vec![eq]);
            Ok(resize(vec![ne], result_width, false))
        }
        Op::Less | Op::LessEq | Op::Greater | Op::GreaterEq => {
            let w = x
                .comptime()
                .r#type
                .total_width()
                .unwrap_or(result_width)
                .max(y.comptime().r#type.total_width().unwrap_or(result_width))
                .max(1);
            let xs = synthesize_expr(ctx, x, current, w)?;
            let ys = synthesize_expr(ctx, y, current, w)?;
            let result = arith::compare(ctx, &xs, &ys, op, signed)?;
            Ok(resize(vec![result], result_width, false))
        }
        Op::LogicShiftL | Op::LogicShiftR | Op::ArithShiftL | Op::ArithShiftR => {
            let xs = synthesize_expr(ctx, x, current, result_width)?;
            let signed_ext = matches!(op, Op::ArithShiftR);
            if let Some(amount) = try_constant(y).map(|n| n as usize) {
                Ok(arith::constant_shift(ctx, &xs, op, amount, signed_ext))
            } else {
                let shift_width = y.comptime().r#type.total_width().unwrap_or(0).max(1);
                let amount_nets = synthesize_expr(ctx, y, current, shift_width)?;
                Ok(arith::barrel_shift(ctx, &xs, &amount_nets, op, signed_ext))
            }
        }
        Op::Mul => {
            let xs = synthesize_expr(ctx, x, current, result_width)?;
            let ys = synthesize_expr(ctx, y, current, result_width)?;
            arith::multiply(ctx, &xs, &ys, result_width, signed)
        }
        Op::Div | Op::Rem => {
            let xs = synthesize_expr(ctx, x, current, result_width)?;
            let ys = synthesize_expr(ctx, y, current, result_width)?;
            let (quo, rem) = if signed {
                arith::divide_signed(ctx, &xs, &ys, result_width)?
            } else {
                arith::divide_unsigned(ctx, &xs, &ys, result_width)?
            };
            Ok(if matches!(op, Op::Div) { quo } else { rem })
        }
        Op::As => synthesize_expr(ctx, x, current, result_width),
        Op::EqWildcard | Op::NeWildcard => {
            let w = x
                .comptime()
                .r#type
                .total_width()
                .unwrap_or(result_width)
                .max(y.comptime().r#type.total_width().unwrap_or(result_width))
                .max(1);

            // analyzer always puts the case-label pattern on the RHS of the
            // `?=` produced from `range_item`, but accept either side so that
            // user-written `pattern ?= sig` also works.
            let y_pat = try_wildcard_pattern(y, w);
            let x_pat = if y_pat.is_none() {
                try_wildcard_pattern(x, w)
            } else {
                None
            };

            let eq = if let Some(pattern) = y_pat.as_ref().or(x_pat.as_ref()) {
                let sig_expr = if y_pat.is_some() { x } else { y };
                let xs = synthesize_expr(ctx, sig_expr, current, w)?;
                let mut bits = Vec::new();
                for (i, pat_bit) in pattern.iter().enumerate().take(w) {
                    if let Some(b) = *pat_bit {
                        let const_net = if b {
                            crate::ir::NET_CONST1
                        } else {
                            crate::ir::NET_CONST0
                        };
                        bits.push(ctx.add_cell(CellKind::Xnor2, vec![xs[i], const_net]));
                    }
                }
                if bits.is_empty() {
                    crate::ir::NET_CONST1
                } else {
                    reduce_and(ctx, &bits)
                }
            } else {
                let xs = synthesize_expr(ctx, x, current, w)?;
                let ys = synthesize_expr(ctx, y, current, w)?;
                arith::equal(ctx, &xs, &ys)
            };

            let result = if matches!(op, Op::NeWildcard) {
                ctx.add_cell(CellKind::Not, vec![eq])
            } else {
                eq
            };
            Ok(resize(vec![result], result_width, false))
        }
        Op::Pow => Err(SynthesizerError::unsupported(
            UnsupportedKind::PowOperator,
            &x.comptime().token,
        )),
        _ => Err(SynthesizerError::internal(format!(
            "binary operator {:?} reached synthesizer",
            op
        ))),
    }
}

/// Inlines a user-defined function call. The body runs against a clone of
/// `current` so the call sees the caller's updates to module-level signals
/// but doesn't leak its own locals back — only declared outputs and the
/// return value are propagated.
fn synth_function_call(
    ctx: &mut ConvContext,
    call: &air::FunctionCall,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<Vec<NetId>, SynthesizerError> {
    let func = ctx
        .functions
        .get(&call.id)
        .cloned()
        .ok_or_else(|| SynthesizerError::internal(format!("function {} not found", call.id)))?;
    let body = match &call.index {
        Some(idx) => func.get_function(idx),
        None => func.get_function(&[]),
    }
    .ok_or_else(|| {
        SynthesizerError::internal(format!(
            "function {} has no body for the requested variant",
            call.id
        ))
    })?;

    let mut inner = current.clone();

    for (path, expr) in &call.inputs {
        let arg_vid = body.arg_map.get(path).ok_or_else(|| {
            SynthesizerError::internal(format!(
                "input arg path {:?} not found in function {} arg_map",
                path, call.id
            ))
        })?;
        let arg_width = ctx.variables.get(arg_vid).map(|s| s.width).ok_or_else(|| {
            SynthesizerError::internal(format!("function arg {} not in module.variables", arg_vid))
        })?;
        let expr_nets = synthesize_expr(ctx, expr, current, arg_width)?;
        inner.insert(*arg_vid, expr_nets);
    }

    crate::conv::statement::process_statements(ctx, &body.statements, &mut inner)?;

    for (path, dsts) in &call.outputs {
        let arg_vid = body.arg_map.get(path).ok_or_else(|| {
            SynthesizerError::internal(format!(
                "output arg path {:?} not found in function {} arg_map",
                path, call.id
            ))
        })?;
        let out_nets = inner.get(arg_vid).cloned().unwrap_or_else(|| {
            ctx.variables
                .get(arg_vid)
                .map(|s| s.nets.clone())
                .unwrap_or_default()
        });
        for dst in dsts {
            let slice_width = crate::conv::statement::dst_slice_width(ctx, dst)?;
            let src = resize(out_nets.clone(), slice_width, false);
            crate::conv::statement::write_to_dst(ctx, dst, &src, current)?;
        }
    }

    let ret_width = call.comptime.r#type.total_width().unwrap_or(0);
    if let Some(ret_vid) = body.ret {
        if let Some(ret_nets) = inner.get(&ret_vid).cloned() {
            Ok(resize(ret_nets, ret_width, call.comptime.r#type.signed))
        } else if let Some(slot) = ctx.variables.get(&ret_vid) {
            Ok(resize(
                slot.nets.clone(),
                ret_width,
                call.comptime.r#type.signed,
            ))
        } else {
            Ok(vec![crate::ir::NET_CONST0; ret_width])
        }
    } else {
        // Void function in expression context (shouldn't happen per analyzer) — stand-in.
        Ok(vec![crate::ir::NET_CONST0; ret_width])
    }
}

/// Statement-context wrapper around `synth_function_call` for void / side-effect
/// only calls. Discards the return value.
pub(crate) fn synth_function_call_stmt(
    ctx: &mut ConvContext,
    call: &air::FunctionCall,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<(), SynthesizerError> {
    synth_function_call(ctx, call, current)?;
    Ok(())
}

/// Narrows `nets` to the bits owned by a struct/union member via the
/// `comptime.part_select` path metadata. No-op when part_select is absent.
pub(crate) fn apply_part_select(
    nets: &[NetId],
    ct: &air::Comptime,
    id: &air::VarId,
) -> Result<Vec<NetId>, SynthesizerError> {
    let Some(ps) = &ct.part_select else {
        return Ok(nets.to_vec());
    };
    let offset: usize = ps.part_select.iter().map(|p| p.pos).sum();
    let width = ps
        .part_select
        .last()
        .and_then(|p| p.r#type.total_width())
        .ok_or_else(|| SynthesizerError::unknown_width(format!("{}.part_select", id), &ct.token))?;
    if offset + width > nets.len() {
        return Err(SynthesizerError::internal(format!(
            "part_select out of range on {}: offset {} + width {} > {}",
            id,
            offset,
            width,
            nets.len()
        )));
    }
    Ok(nets[offset..offset + width].to_vec())
}

/// Extracts a per-bit wildcard pattern from a literal `Factor::Value` whose
/// analyzer `Value` carries don't-care bits (`mask_xz`). Returns `None` when
/// the operand is not a literal, carries no don't-cares, or is a BigUint
/// pattern (>64 bit wildcards are not supported yet).
fn try_wildcard_pattern(expr: &Expression, width: usize) -> Option<Vec<Option<bool>>> {
    use veryl_analyzer::value::Value;
    let Expression::Term(factor) = expr else {
        return None;
    };
    let Factor::Value(ct) = factor.as_ref() else {
        return None;
    };
    let value = ct.get_value().ok()?;
    if !value.is_xz() {
        return None;
    }
    match value {
        Value::U64(v) => {
            let mut bits = Vec::with_capacity(width);
            for i in 0..width {
                let is_xz = (v.mask_xz >> i) & 1 == 1;
                if is_xz {
                    bits.push(None);
                } else {
                    bits.push(Some((v.payload >> i) & 1 == 1));
                }
            }
            Some(bits)
        }
        Value::BigUint(_) => None,
    }
}

pub(crate) fn reduce_and(ctx: &mut ConvContext, bits: &[NetId]) -> NetId {
    reduce(ctx, bits, CellKind::And2, crate::ir::NET_CONST1)
}

pub(crate) fn reduce_or(ctx: &mut ConvContext, bits: &[NetId]) -> NetId {
    reduce(ctx, bits, CellKind::Or2, crate::ir::NET_CONST0)
}

pub(crate) fn reduce_xor(ctx: &mut ConvContext, bits: &[NetId]) -> NetId {
    reduce(ctx, bits, CellKind::Xor2, crate::ir::NET_CONST0)
}

fn reduce(ctx: &mut ConvContext, bits: &[NetId], kind: CellKind, identity: NetId) -> NetId {
    if bits.is_empty() {
        return identity;
    }
    if bits.len() == 1 {
        return bits[0];
    }
    let mut acc = bits[0];
    for &b in &bits[1..] {
        acc = ctx.add_cell(kind, vec![acc, b]);
    }
    acc
}
