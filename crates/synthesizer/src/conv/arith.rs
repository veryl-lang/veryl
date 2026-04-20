use crate::conv::ConvContext;
use crate::error::SynthError;
use crate::ir::{CellKind, NetId};
use veryl_analyzer::ir::Op;

// sum  = a ^ b ^ cin
// cout = a b + cin (a ^ b)
fn full_adder(ctx: &mut ConvContext, a: NetId, b: NetId, cin: NetId) -> (NetId, NetId) {
    let a_xor_b = ctx.add_cell(CellKind::Xor2, vec![a, b]);
    let sum = ctx.add_cell(CellKind::Xor2, vec![a_xor_b, cin]);
    let a_and_b = ctx.add_cell(CellKind::And2, vec![a, b]);
    let cin_and_ab = ctx.add_cell(CellKind::And2, vec![cin, a_xor_b]);
    let cout = ctx.add_cell(CellKind::Or2, vec![a_and_b, cin_and_ab]);
    (sum, cout)
}

/// Ripple-carry adder returning `a + b + cin`. Operands must be the same width.
pub(crate) fn ripple_add(
    ctx: &mut ConvContext,
    a: &[NetId],
    b: &[NetId],
    cin: NetId,
) -> Result<Vec<NetId>, SynthError> {
    if a.len() != b.len() {
        return Err(SynthError::Internal(
            "ripple_add operand width mismatch".into(),
        ));
    }
    let mut sum = Vec::with_capacity(a.len());
    let mut carry = cin;
    for i in 0..a.len() {
        let (s, c) = full_adder(ctx, a[i], b[i], carry);
        sum.push(s);
        carry = c;
    }
    Ok(sum)
}

/// `a - b` via two's complement: `a + ~b + 1`.
pub(crate) fn ripple_sub(
    ctx: &mut ConvContext,
    a: &[NetId],
    b: &[NetId],
) -> Result<Vec<NetId>, SynthError> {
    if a.len() != b.len() {
        return Err(SynthError::Internal(
            "ripple_sub operand width mismatch".into(),
        ));
    }
    let mut inv_b = Vec::with_capacity(b.len());
    for &bi in b {
        inv_b.push(ctx.add_cell(CellKind::Not, vec![bi]));
    }
    ripple_add(ctx, a, &inv_b, crate::ir::NET_CONST1)
}

/// 1-bit result: AND-reduction of per-bit XNOR. Operands must be the same width.
pub(crate) fn equal(ctx: &mut ConvContext, a: &[NetId], b: &[NetId]) -> NetId {
    if a.is_empty() {
        return crate::ir::NET_CONST1;
    }
    let mut eq_bits = Vec::with_capacity(a.len());
    for i in 0..a.len() {
        eq_bits.push(ctx.add_cell(CellKind::Xnor2, vec![a[i], b[i]]));
    }
    crate::conv::expression::reduce_and(ctx, &eq_bits)
}

/// `a OP b` where OP ∈ {<, <=, >, >=}. Operand widths must match.
pub(crate) fn compare(
    ctx: &mut ConvContext,
    a: &[NetId],
    b: &[NetId],
    op: Op,
    signed: bool,
) -> Result<NetId, SynthError> {
    if a.is_empty() {
        return Ok(crate::ir::NET_CONST0);
    }
    // Extend by one bit so that subtraction doesn't wrap; the MSB of the
    // result then tells us whether `a < b` for both signed and unsigned.
    let width = a.len();
    let (a_ext, b_ext): (Vec<NetId>, Vec<NetId>) = if signed {
        let mut a_ext = a.to_vec();
        a_ext.push(a[width - 1]);
        let mut b_ext = b.to_vec();
        b_ext.push(b[width - 1]);
        (a_ext, b_ext)
    } else {
        let mut a_ext = a.to_vec();
        a_ext.push(crate::ir::NET_CONST0);
        let mut b_ext = b.to_vec();
        b_ext.push(crate::ir::NET_CONST0);
        (a_ext, b_ext)
    };

    let diff = ripple_sub(ctx, &a_ext, &b_ext)?;
    let a_lt_b = diff[diff.len() - 1];

    match op {
        Op::Less => Ok(a_lt_b),
        Op::GreaterEq => Ok(ctx.add_cell(CellKind::Not, vec![a_lt_b])),
        // a > b ≡ b < a
        Op::Greater => {
            let diff = ripple_sub(ctx, &b_ext, &a_ext)?;
            Ok(diff[diff.len() - 1])
        }
        Op::LessEq => {
            let diff = ripple_sub(ctx, &b_ext, &a_ext)?;
            let b_lt_a = diff[diff.len() - 1];
            Ok(ctx.add_cell(CellKind::Not, vec![b_lt_a]))
        }
        _ => Err(SynthError::Internal(format!(
            "compare() called with non-comparison op {:?}",
            op
        ))),
    }
}

/// Constant-amount shift is pure rewiring: no cells are emitted.
pub(crate) fn constant_shift(
    _ctx: &mut ConvContext,
    xs: &[NetId],
    op: Op,
    amount: usize,
    signed_ext: bool,
) -> Vec<NetId> {
    let width = xs.len();
    let mut out = vec![crate::ir::NET_CONST0; width];
    match op {
        Op::LogicShiftL | Op::ArithShiftL if amount < width => {
            out[amount..width].copy_from_slice(&xs[..(width - amount)]);
        }
        Op::LogicShiftR if amount < width => {
            out[..(width - amount)].copy_from_slice(&xs[amount..width]);
        }
        Op::ArithShiftR => {
            let pad = if signed_ext && width > 0 {
                xs[width - 1]
            } else {
                crate::ir::NET_CONST0
            };
            for i in 0..width {
                out[i] = if i + amount < width {
                    xs[i + amount]
                } else {
                    pad
                };
            }
        }
        _ => (),
    }
    out
}
