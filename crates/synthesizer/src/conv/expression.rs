use std::collections::HashMap;

use veryl_analyzer::ir::{
    self as air, Expression, Factor, Op, SystemFunctionInput, SystemFunctionKind,
};

use crate::conv::ConvContext;
use crate::conv::arith;
use crate::conv::statement::{dst_slice_width, process_statements, write_to_dst};
use crate::ir::{CellKind, NET_CONST0, NET_CONST1, NetId};
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

pub(crate) fn try_constant(expr: &Expression) -> Option<u64> {
    if let Expression::Term(factor) = expr
        && let Factor::Value(ct) = factor.as_ref()
    {
        let value = ct.get_value().ok()?;
        return value.to_u64();
    }
    // Fallback for expressions the analyzer wraps (e.g. width casts or a
    // concatenation that is compile-time known). `eval_value` goes through
    // the full constant-folding path but only returns Some when the result
    // actually is constant.
    let mut eval_ctx = veryl_analyzer::Context::default();
    expr.eval_value(&mut eval_ctx).and_then(|v| v.to_u64())
}

/// Like [`try_constant`] but returns the underlying bit-pattern when the
/// value has `x`/`z` don't-care bits (treating them as 0). Useful when
/// lowering a SystemVerilog `case` default that writes `'x` — the bits
/// are free to be anything, and picking 0 keeps the output assignment
/// deterministic without costing cells.
pub(crate) fn try_constant_with_dontcare_zero(expr: &Expression) -> Option<u64> {
    use veryl_analyzer::value::Value;
    if let Some(v) = try_constant(expr) {
        return Some(v);
    }
    let ct = match expr {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Value(ct) => ct,
            _ => return None,
        },
        _ => return None,
    };
    let value = ct.get_value().ok()?;
    match value {
        Value::U64(v) => Some(v.payload & !v.mask_xz),
        Value::BigUint(_) => None,
    }
}

fn build_constant(value: u64, width: usize) -> Vec<NetId> {
    let mut out = Vec::with_capacity(width);
    for i in 0..width {
        let bit_set = i < 64 && (value >> i) & 1 == 1;
        out.push(if bit_set { NET_CONST1 } else { NET_CONST0 });
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
                out.push(if bit { NET_CONST1 } else { NET_CONST0 });
            }
        }
        Value::BigUint(v) => {
            let payload = v.payload();
            for i in 0..width {
                let bit = payload.bit(i as u64);
                out.push(if bit { NET_CONST1 } else { NET_CONST0 });
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
                NET_CONST0
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
            // Detect `(sel == c1) ? v1 : (sel == c2) ? v2 : ... : default`
            // and fold to shared matches + per-bit OR before the generic
            // Mux2 cascade. Hot for case-expressions and LUT decoders.
            if let Some(out) = try_fold_case_ternary(ctx, expr, current, ctx_width)? {
                return Ok(out);
            }
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
        Factor::SystemFunctionCall(call) => synth_system_function_call(ctx, call, current),
        Factor::Anonymous(ct) => {
            // `_` placeholder — drive with all-zero at the declared width.
            let width = ct.r#type.total_width().unwrap_or(0);
            Ok(vec![NET_CONST0; width])
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
            let zero = vec![NET_CONST0; result_width];
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
            arith::ripple_add(ctx, &xs, &ys, NET_CONST0)
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
                        let const_net = if b { NET_CONST1 } else { NET_CONST0 };
                        bits.push(ctx.add_cell(CellKind::Xnor2, vec![xs[i], const_net]));
                    }
                }
                if bits.is_empty() {
                    NET_CONST1
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

/// Synthesizes the synthesizable subset of SystemVerilog system functions.
/// `$signed` / `$unsigned` are bit-pattern pass-throughs (signedness is a
/// type-level attribute the bit vector is already sized for). `$clog2`,
/// `$bits`, `$size`, `$onehot` on constant-foldable args use the analyzer's
/// comptime value. Truly runtime-only calls ($display / $readmemh / etc)
/// stay unsupported.
fn synth_system_function_call(
    ctx: &mut ConvContext,
    call: &air::SystemFunctionCall,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
) -> Result<Vec<NetId>, SynthesizerError> {
    let ret_width = call.comptime.r#type.total_width().unwrap_or(0);
    match &call.kind {
        SystemFunctionKind::Signed(SystemFunctionInput(inner))
        | SystemFunctionKind::Unsigned(SystemFunctionInput(inner)) => {
            synthesize_expr(ctx, inner, current, ret_width)
        }
        SystemFunctionKind::Bits(_)
        | SystemFunctionKind::Size(_)
        | SystemFunctionKind::Clog2(_) => {
            let value = call.comptime.get_value().map_err(|_| {
                SynthesizerError::unsupported(
                    UnsupportedKind::SystemFunctionCall,
                    &call.comptime.token,
                )
            })?;
            Ok(value_to_nets(value, ret_width))
        }
        SystemFunctionKind::Onehot(SystemFunctionInput(inner)) => {
            // `$onehot(x)` is 1 iff exactly one bit of x is set. Walk bits
            // carrying (any, more) state: `any` becomes true once any bit
            // has been seen, `more` flips once a second bit is seen. Final
            // result is `any AND NOT more`.
            let inner_width = inner.comptime().r#type.total_width().unwrap_or(1).max(1);
            let bits = synthesize_expr(ctx, inner, current, inner_width)?;
            let result = onehot_reduce(ctx, &bits);
            Ok(resize(vec![result], ret_width.max(1), false))
        }
        _ => Err(SynthesizerError::unsupported(
            UnsupportedKind::SystemFunctionCall,
            &call.comptime.token,
        )),
    }
}

/// Returns the 1-bit net that is true iff exactly one input bit is set.
/// Scans bits linearly carrying `(any, more)` state: `any` becomes true once
/// any bit has been seen, `more` becomes true as soon as a bit is seen while
/// `any` already holds. Final result is `any AND NOT more`.
fn onehot_reduce(ctx: &mut ConvContext, bits: &[NetId]) -> NetId {
    if bits.is_empty() {
        return NET_CONST0;
    }
    let mut any = bits[0];
    let mut more = NET_CONST0;
    for &b in &bits[1..] {
        let both = ctx.add_cell(CellKind::And2, vec![any, b]);
        more = ctx.add_cell(CellKind::Or2, vec![more, both]);
        any = ctx.add_cell(CellKind::Or2, vec![any, b]);
    }
    let not_more = ctx.add_cell(CellKind::Not, vec![more]);
    ctx.add_cell(CellKind::And2, vec![any, not_more])
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

    process_statements(ctx, &body.statements, &mut inner)?;

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
            let slice_width = dst_slice_width(ctx, dst)?;
            let src = resize(out_nets.clone(), slice_width, false);
            write_to_dst(ctx, dst, &src, current)?;
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
            Ok(vec![NET_CONST0; ret_width])
        }
    } else {
        // Void function in expression context (shouldn't happen per analyzer) — stand-in.
        Ok(vec![NET_CONST0; ret_width])
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

/// Append `match_sig · value` to `terms`, shortcutting the constant cases:
/// `value == CONST0` contributes nothing, `value == CONST1` just passes the
/// match signal through, anything else emits one And2 gate.
pub(crate) fn push_gated_term(
    ctx: &mut ConvContext,
    terms: &mut Vec<NetId>,
    match_sig: NetId,
    value: NetId,
) {
    match value {
        NET_CONST0 => {}
        NET_CONST1 => terms.push(match_sig),
        other => {
            terms.push(ctx.add_cell(CellKind::And2, vec![match_sig, other]));
        }
    }
}

/// One bit of a case-like SOP emission: OR together `matches[i] & arm_bits[i]`
/// across all arms, optionally gated-OR'd with `default_active & default_bit`.
/// `default_active` (= `!any_match`) is lazily materialised on the first bit
/// that actually needs it, so bit columns where every arm + default is CONST0
/// pay no decoder cost.
pub(crate) fn emit_sop_bit(
    ctx: &mut ConvContext,
    matches: &[NetId],
    arm_bits: &[NetId],
    default_bit: NetId,
    default_active: &mut Option<NetId>,
) -> NetId {
    debug_assert_eq!(matches.len(), arm_bits.len());
    let mut terms: Vec<NetId> = Vec::with_capacity(arm_bits.len() + 1);
    for (i, &v) in arm_bits.iter().enumerate() {
        push_gated_term(ctx, &mut terms, matches[i], v);
    }
    if default_bit != NET_CONST0 {
        let da = *default_active.get_or_insert_with(|| {
            let any = reduce_or(ctx, matches);
            ctx.add_cell(CellKind::Not, vec![any])
        });
        push_gated_term(ctx, &mut terms, da, default_bit);
    }
    match terms.len() {
        0 => NET_CONST0,
        1 => terms[0],
        _ => reduce_or(ctx, &terms),
    }
}

pub(crate) fn reduce_and(ctx: &mut ConvContext, bits: &[NetId]) -> NetId {
    reduce(ctx, bits, CellKind::And2, NET_CONST1)
}

pub(crate) fn reduce_or(ctx: &mut ConvContext, bits: &[NetId]) -> NetId {
    reduce(ctx, bits, CellKind::Or2, NET_CONST0)
}

/// Two-level SOP minimization for small truth tables (sel_width ≤ 8).
/// Returns cubes `(value, care_mask)` covering every `on` point and no
/// `off` point; anything listed in neither is don't-care. Each ON seed
/// greedily drops bits while staying off-free (Expresso-style), then a
/// greedy set cover picks the prime implicants to emit.
pub(crate) fn minimize_sop_cubes(on: &[u64], off: &[u64], sel_width: usize) -> Vec<(u64, u64)> {
    if on.is_empty() {
        return Vec::new();
    }
    let sel_mask: u64 = if sel_width >= 64 {
        !0u64
    } else {
        (1u64 << sel_width) - 1
    };
    // Deduplicate ON minterms up-front so the cover step terminates cleanly.
    let mut on_uniq: Vec<u64> = on.iter().map(|m| m & sel_mask).collect();
    on_uniq.sort();
    on_uniq.dedup();

    let hits_off = |value: u64, mask: u64| -> bool {
        let tgt = value & mask;
        off.iter().any(|&o| (o & mask) == tgt)
    };

    // Expand each ON seed into a prime implicant.
    let mut primes: Vec<(u64, u64)> = Vec::new();
    for &seed in &on_uniq {
        let mut value = seed;
        let mut mask = sel_mask;
        for bit in 0..sel_width {
            let candidate_mask = mask & !(1u64 << bit);
            if !hits_off(value, candidate_mask) {
                mask = candidate_mask;
                value &= candidate_mask;
            }
        }
        if !primes.iter().any(|&(v, m)| m == mask && v == value) {
            primes.push((value, mask));
        }
    }

    // Greedy set cover: always pick the prime covering the most uncovered ONs.
    let mut covered = vec![false; on_uniq.len()];
    let mut chosen: Vec<(u64, u64)> = Vec::new();
    while covered.iter().any(|&c| !c) {
        let mut best: Option<(usize, usize)> = None;
        for (pi, &(v, m)) in primes.iter().enumerate() {
            let cnt = on_uniq
                .iter()
                .enumerate()
                .filter(|&(i, &o)| !covered[i] && (o & m) == v)
                .count();
            match best {
                None if cnt > 0 => best = Some((pi, cnt)),
                Some((_, c)) if cnt > c => best = Some((pi, cnt)),
                _ => {}
            }
        }
        match best {
            Some((pi, _)) => {
                let (v, m) = primes[pi];
                for (i, &o) in on_uniq.iter().enumerate() {
                    if (o & m) == v {
                        covered[i] = true;
                    }
                }
                chosen.push((v, m));
            }
            None => break,
        }
    }
    chosen
}

/// Emit a single cube as an AND of literals. `value` / `mask` encode the
/// cube: mask bit i set → bit i is specified, value bit i gives its polarity.
/// Uses `not_cache` to share inverter cells across cubes.
pub(crate) fn emit_cube(
    ctx: &mut ConvContext,
    sel_nets: &[NetId],
    value: u64,
    mask: u64,
    not_cache: &mut HashMap<NetId, NetId>,
) -> NetId {
    let mut lits: Vec<NetId> = Vec::new();
    for (i, &net) in sel_nets.iter().enumerate() {
        if (mask >> i) & 1 == 0 {
            continue;
        }
        let lit = if (value >> i) & 1 == 1 {
            net
        } else {
            *not_cache
                .entry(net)
                .or_insert_with(|| ctx.add_cell(CellKind::Not, vec![net]))
        };
        lits.push(lit);
    }
    if lits.is_empty() {
        NET_CONST1
    } else {
        reduce_and(ctx, &lits)
    }
}

pub(crate) fn reduce_xor(ctx: &mut ConvContext, bits: &[NetId]) -> NetId {
    reduce(ctx, bits, CellKind::Xor2, NET_CONST0)
}

fn reduce(ctx: &mut ConvContext, bits: &[NetId], kind: CellKind, identity: NetId) -> NetId {
    if bits.is_empty() {
        return identity;
    }
    if bits.len() == 1 {
        return bits[0];
    }
    // Balanced pairwise tree: depth O(log N) instead of O(N) chain.
    // Cell count is the same (N-1) but critical path timing improves.
    // Emission stays at 2-input shape so the post-pass can still fuse
    // adjacent And2 pairs into Ao22 / Aoi22 / Oai22 compounds — directly
    // emitting And3 here breaks that fusion window and regresses td4v_Rom.
    let mut level: Vec<NetId> = bits.to_vec();
    while level.len() > 1 {
        let mut next: Vec<NetId> = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            if pair.len() == 2 {
                next.push(ctx.add_cell(kind, vec![pair[0], pair[1]]));
            } else {
                next.push(pair[0]);
            }
        }
        level = next;
    }
    level[0]
}

/// Detect the nested-ternary form of a case expression:
///   `(sel == c1) ? v1 : ((sel == c2) ? v2 : ... : default)`
/// When every selector is the same expression, rewrite to per-bit OR of
/// shared match signals. Arm values may be constants (bits selected by
/// match) or wires (bits AND-gated by match). A constant-bit coverage
/// heuristic gates the fold so wire-heavy chains fall back to the generic
/// Mux2 cascade, which is cheaper per-bit when most arms are wires.
pub(super) fn try_fold_case_ternary(
    ctx: &mut ConvContext,
    expr: &Expression,
    current: &mut HashMap<air::VarId, Vec<NetId>>,
    out_width: usize,
) -> Result<Option<Vec<NetId>>, SynthesizerError> {
    const MIN_ARMS: usize = 3;

    // Peel the ternary chain. Keep arm value expressions for later synthesis.
    let mut arm_exprs: Vec<(u64, &Expression)> = Vec::new();
    let mut sel_expr: Option<&Expression> = None;
    let mut node = expr;
    let default_expr = loop {
        let Expression::Ternary(cond, a, b, _) = node else {
            break node;
        };
        let Some((cond_sel, cond_const)) = extract_ternary_eq(cond) else {
            return Ok(None);
        };
        match sel_expr {
            None => sel_expr = Some(cond_sel),
            Some(prev) => {
                if !expressions_eq(prev, cond_sel) {
                    return Ok(None);
                }
            }
        }
        arm_exprs.push((cond_const, a));
        node = b;
    };
    let Some(sel_expr) = sel_expr else {
        return Ok(None);
    };
    if arm_exprs.len() < MIN_ARMS {
        return Ok(None);
    }
    let sel_width = sel_expr.comptime().r#type.total_width().unwrap_or(0);
    if sel_width == 0 || sel_width > 32 {
        return Ok(None);
    }
    for &(c, _) in &arm_exprs {
        if sel_width < 64 && (c >> sel_width) != 0 {
            return Ok(None);
        }
    }

    // Cost guard: SOP beats a per-bit Mux2 cascade only when arm values are
    // mostly constants (so most bit-slots contribute 0 cells). For wire-heavy
    // chains each arm bit costs an extra And2 gate, which overall exceeds the
    // cascade. Require ≥50% constant coverage over (arms + default) × bits.
    let const_bits_for = |e: &Expression| -> usize {
        if try_constant_with_dontcare_zero(e).is_some() {
            out_width
        } else {
            0
        }
    };
    let const_bits: usize = arm_exprs
        .iter()
        .map(|(_, e)| const_bits_for(e))
        .sum::<usize>()
        + const_bits_for(default_expr);
    let total_bits = (arm_exprs.len() + 1) * out_width;
    if 2 * const_bits < total_bits {
        return Ok(None);
    }

    let sel_nets = synthesize_expr(ctx, sel_expr, current, sel_width)?;

    // One match signal per arm.
    let mut not_cache: HashMap<NetId, NetId> = HashMap::new();
    let mut matches: Vec<NetId> = Vec::with_capacity(arm_exprs.len());
    for &(c, _) in &arm_exprs {
        let mut eq_bits = Vec::with_capacity(sel_width);
        for (i, &net) in sel_nets.iter().enumerate().take(sel_width) {
            let bit_one = (c >> i) & 1 == 1;
            eq_bits.push(if bit_one {
                net
            } else {
                *not_cache
                    .entry(net)
                    .or_insert_with(|| ctx.add_cell(CellKind::Not, vec![net]))
            });
        }
        matches.push(reduce_and(ctx, &eq_bits));
    }

    // Synthesize arm values. Constants become bit arrays of CONST0/CONST1
    // (so the per-bit loop below treats them identically to wire arms).
    let mut arm_nets: Vec<Vec<NetId>> = Vec::with_capacity(arm_exprs.len());
    for &(_, arm_expr) in &arm_exprs {
        let nets = if let Some(c) = try_constant_with_dontcare_zero(arm_expr) {
            build_constant(c & low_mask(out_width), out_width)
        } else {
            synthesize_expr(ctx, arm_expr, current, out_width)?
        };
        arm_nets.push(nets);
    }
    let default_nets = if let Some(c) = try_constant_with_dontcare_zero(default_expr) {
        build_constant(c & low_mask(out_width), out_width)
    } else {
        synthesize_expr(ctx, default_expr, current, out_width)?
    };

    let mut default_active: Option<NetId> = None;
    let mut out_nets = Vec::with_capacity(out_width);
    let mut arm_bits: Vec<NetId> = Vec::with_capacity(arm_nets.len());
    for bit in 0..out_width {
        arm_bits.clear();
        arm_bits.extend(arm_nets.iter().map(|an| an[bit]));
        out_nets.push(emit_sop_bit(
            ctx,
            &matches,
            &arm_bits,
            default_nets[bit],
            &mut default_active,
        ));
    }
    Ok(Some(out_nets))
}

fn low_mask(width: usize) -> u64 {
    if width >= 64 {
        !0u64
    } else {
        (1u64 << width) - 1
    }
}

fn extract_ternary_eq(expr: &Expression) -> Option<(&Expression, u64)> {
    use veryl_analyzer::ir::Op;
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

fn expressions_eq(a: &Expression, b: &Expression) -> bool {
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
