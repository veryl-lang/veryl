use std::collections::HashMap;

use veryl_analyzer::ir::{self as air, Expression, Factor, Op};

use crate::conv::ConvContext;
use crate::conv::arith;
use crate::error::SynthError;
use crate::ir::{CellKind, NetId};

/// Returns `target_width` nets in LSB-first order. Narrower results are
/// zero- or sign-extended; wider results are truncated.
pub(crate) fn synthesize_expr(
    ctx: &mut ConvContext,
    expr: &Expression,
    current: &HashMap<air::VarId, Vec<NetId>>,
    target_width: usize,
) -> Result<Vec<NetId>, SynthError> {
    if let Some(constant) = try_constant(expr) {
        return Ok(build_constant(constant, target_width));
    }

    let raw = synth_raw(ctx, expr, current)?;
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
    current: &HashMap<air::VarId, Vec<NetId>>,
) -> Result<Vec<NetId>, SynthError> {
    match expr {
        Expression::Term(factor) => synth_factor(ctx, factor, current),
        Expression::Unary(op, inner, comptime) => {
            let ctx_width = comptime.expr_context.width.max(1);
            synth_unary(ctx, *op, inner, current, ctx_width)
        }
        Expression::Binary(x, op, y, comptime) => {
            let ctx_width = comptime.expr_context.width.max(1);
            let signed = comptime.expr_context.signed;
            synth_binary(ctx, x, *op, y, current, ctx_width, signed)
        }
        Expression::Ternary(cond, a, b, comptime) => {
            let ctx_width = comptime.expr_context.width.max(1);
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
                            SynthError::unsupported("non-constant concatenation repeat")
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
            Err(SynthError::unsupported("array / struct literal (Phase 2+)"))
        }
    }
}

fn synth_factor(
    ctx: &mut ConvContext,
    factor: &Factor,
    current: &HashMap<air::VarId, Vec<NetId>>,
) -> Result<Vec<NetId>, SynthError> {
    match factor {
        Factor::Value(ct) => {
            let value = ct
                .get_value()
                .map_err(|_| SynthError::unsupported("non-numeric value factor"))?;
            let n = value
                .to_u64()
                .ok_or_else(|| SynthError::unsupported("value too wide (>64 bits) in Phase 1"))?;
            let width = ct.r#type.total_width().unwrap_or(value.width());
            Ok(build_constant(n, width))
        }
        Factor::Variable(id, index, select, ct) => {
            if !index.0.is_empty() {
                return Err(SynthError::unsupported(
                    "array indexing in expressions (Phase 2+)",
                ));
            }
            // The `current` map shadows the persistent nets within a block
            // so that `a = a + 1`-style reads pick up the previous value.
            let src_nets = current
                .get(id)
                .cloned()
                .or_else(|| ctx.variables.get(id).map(|s| s.nets.clone()))
                .ok_or_else(|| {
                    SynthError::Internal(format!("reference to unknown variable {}", id))
                })?;
            if select.is_empty() {
                return Ok(src_nets);
            }
            if !select.is_const() {
                return Err(SynthError::dynamic_select(format!("variable {}", id)));
            }
            let mut eval_ctx = veryl_analyzer::Context::default();
            let (high, low) = select
                .eval_value(&mut eval_ctx, &ct.r#type, false)
                .ok_or_else(|| SynthError::dynamic_select(format!("variable {}", id)))?;
            if high >= src_nets.len() {
                return Err(SynthError::Internal(format!(
                    "bit select out of range: {}..={} of {}-bit signal",
                    low,
                    high,
                    src_nets.len()
                )));
            }
            Ok(src_nets[low..=high].to_vec())
        }
        Factor::FunctionCall(_) | Factor::SystemFunctionCall(_) => Err(SynthError::unsupported(
            "function / system-function call in Phase 1",
        )),
        Factor::Anonymous(_) | Factor::Unknown(_) => {
            Err(SynthError::unsupported("anonymous / unknown factor"))
        }
    }
}

fn synth_unary(
    ctx: &mut ConvContext,
    op: Op,
    inner: &Expression,
    current: &HashMap<air::VarId, Vec<NetId>>,
    result_width: usize,
) -> Result<Vec<NetId>, SynthError> {
    match op {
        Op::BitNot => {
            let w = inner.comptime().expr_context.width.max(result_width);
            let xs = synthesize_expr(ctx, inner, current, w)?;
            let mut out = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(ctx.add_cell(CellKind::Not, vec![x]));
            }
            Ok(resize(out, result_width, false))
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
        _ => Err(SynthError::unsupported(format!("unary operator {:?}", op))),
    }
}

fn synth_binary(
    ctx: &mut ConvContext,
    x: &Expression,
    op: Op,
    y: &Expression,
    current: &HashMap<air::VarId, Vec<NetId>>,
    result_width: usize,
    signed: bool,
) -> Result<Vec<NetId>, SynthError> {
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
            let amount = try_constant(y)
                .map(|n| n as usize)
                .ok_or_else(|| SynthError::unsupported("variable shift amount (Phase 2+)"))?;
            let xs = synthesize_expr(ctx, x, current, result_width)?;
            let signed_ext = matches!(op, Op::ArithShiftR);
            Ok(arith::constant_shift(ctx, &xs, op, amount, signed_ext))
        }
        Op::Mul | Op::Div | Op::Rem | Op::Pow => Err(SynthError::unsupported(format!(
            "arithmetic operator '{}' (Phase 2+)",
            op
        ))),
        Op::As => synthesize_expr(ctx, x, current, result_width),
        Op::EqWildcard | Op::NeWildcard => {
            Err(SynthError::unsupported("wildcard equality (Phase 2+)"))
        }
        _ => Err(SynthError::unsupported(format!("binary operator {:?}", op))),
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
