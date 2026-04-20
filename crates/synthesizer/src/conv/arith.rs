use crate::conv::ConvContext;
use crate::ir::{CellKind, NetId};
use crate::synthesizer_error::SynthesizerError;
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
) -> Result<Vec<NetId>, SynthesizerError> {
    if a.len() != b.len() {
        return Err(SynthesizerError::internal(
            "ripple_add operand width mismatch",
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
) -> Result<Vec<NetId>, SynthesizerError> {
    if a.len() != b.len() {
        return Err(SynthesizerError::internal(
            "ripple_sub operand width mismatch",
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
) -> Result<NetId, SynthesizerError> {
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
        _ => Err(SynthesizerError::internal(format!(
            "compare() called with non-comparison op {:?}",
            op
        ))),
    }
}

/// Sign- or zero-extends `xs` to `target_width` nets (truncates if wider).
fn extend(xs: &[NetId], target_width: usize, signed: bool) -> Vec<NetId> {
    if xs.len() >= target_width {
        return xs[..target_width].to_vec();
    }
    let pad = if signed && !xs.is_empty() {
        xs[xs.len() - 1]
    } else {
        crate::ir::NET_CONST0
    };
    let mut out = xs.to_vec();
    out.resize(target_width, pad);
    out
}

/// `a * b` synthesized as a shift-add array: for each bit of `b`, AND with
/// `a` shifted left by `i`, then ripple-add all partial products. Works for
/// both unsigned and signed because sign-extending both operands to the
/// target width makes the low `result_width` bits agree between the two
/// interpretations (standard 2's-complement property).
pub(crate) fn multiply(
    ctx: &mut ConvContext,
    a: &[NetId],
    b: &[NetId],
    result_width: usize,
    signed: bool,
) -> Result<Vec<NetId>, SynthesizerError> {
    if result_width == 0 {
        return Ok(Vec::new());
    }
    let a_ext = extend(a, result_width, signed);
    let b_ext = extend(b, result_width, signed);
    let mut acc = vec![crate::ir::NET_CONST0; result_width];
    for i in 0..result_width {
        let mut row = vec![crate::ir::NET_CONST0; result_width];
        for j in i..result_width {
            row[j] = ctx.add_cell(CellKind::And2, vec![b_ext[i], a_ext[j - i]]);
        }
        acc = ripple_add(ctx, &acc, &row, crate::ir::NET_CONST0)?;
    }
    Ok(acc)
}

/// `-x` when `sign == 1`, `x` otherwise, via `(x XOR sign) + sign`.
fn conditional_negate(
    ctx: &mut ConvContext,
    x: &[NetId],
    sign: NetId,
) -> Result<Vec<NetId>, SynthesizerError> {
    let inv: Vec<NetId> = x
        .iter()
        .map(|&b| ctx.add_cell(CellKind::Xor2, vec![b, sign]))
        .collect();
    let zero = vec![crate::ir::NET_CONST0; x.len()];
    ripple_add(ctx, &inv, &zero, sign)
}

/// Signed `a / b` and `a % b` via sign-magnitude: take |a|, |b|, run unsigned
/// restoring division, then fix signs. Follows C99 / SV truncated-division
/// semantics: quotient is signed by XOR of operand signs, remainder takes the
/// sign of the dividend.
pub(crate) fn divide_signed(
    ctx: &mut ConvContext,
    a: &[NetId],
    b: &[NetId],
    n: usize,
) -> Result<(Vec<NetId>, Vec<NetId>), SynthesizerError> {
    if n == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let a = extend(a, n, true);
    let b = extend(b, n, true);
    let sign_a = a[n - 1];
    let sign_b = b[n - 1];

    let abs_a = conditional_negate(ctx, &a, sign_a)?;
    let abs_b = conditional_negate(ctx, &b, sign_b)?;
    let (q_un, r_un) = divide_unsigned(ctx, &abs_a, &abs_b, n)?;

    let q_sign = ctx.add_cell(CellKind::Xor2, vec![sign_a, sign_b]);
    let quo = conditional_negate(ctx, &q_un, q_sign)?;
    let rem = conditional_negate(ctx, &r_un, sign_a)?;
    Ok((quo, rem))
}

/// Unsigned `a / b` and `a % b` computed by the textbook restoring algorithm.
/// Returns `(quotient, remainder)`, each `n` bits. At each step we shift the
/// running remainder left, pull in one bit of `a`, tentatively subtract `b`,
/// and commit the subtraction only when the result is non-negative.
///
/// `b == 0` yields all-ones quotient and a remainder equal to the dividend,
/// matching typical SV simulator behaviour (hardware divide-by-zero is
/// implementation-defined anyway).
pub(crate) fn divide_unsigned(
    ctx: &mut ConvContext,
    a: &[NetId],
    b: &[NetId],
    n: usize,
) -> Result<(Vec<NetId>, Vec<NetId>), SynthesizerError> {
    if n == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let a = extend(a, n, false);
    let mut b_ext = extend(b, n, false);
    b_ext.push(crate::ir::NET_CONST0);

    let mut rem: Vec<NetId> = vec![crate::ir::NET_CONST0; n + 1];
    let mut quo: Vec<NetId> = vec![crate::ir::NET_CONST0; n];

    for i in (0..n).rev() {
        let mut shifted = vec![crate::ir::NET_CONST0; n + 1];
        shifted[0] = a[i];
        for (j, &r) in rem.iter().enumerate().take(n) {
            shifted[j + 1] = r;
        }
        let trial = ripple_sub(ctx, &shifted, &b_ext)?;
        let q_bit = ctx.add_cell(CellKind::Not, vec![trial[n]]);
        quo[i] = q_bit;
        rem = shifted
            .iter()
            .zip(trial.iter())
            .map(|(&s, &t)| ctx.add_cell(CellKind::Mux2, vec![q_bit, s, t]))
            .collect();
    }

    Ok((quo, rem[..n].to_vec()))
}

/// Variable-amount shift built as a log₂(n)-stage barrel shifter: each stage
/// conditionally shifts by 2^i based on one bit of `amount`. Any `amount`
/// bits above stage width saturate the result to the fill value.
pub(crate) fn barrel_shift(
    ctx: &mut ConvContext,
    xs: &[NetId],
    amount: &[NetId],
    op: Op,
    signed_ext: bool,
) -> Vec<NetId> {
    let n = xs.len();
    if n == 0 {
        return Vec::new();
    }
    let fill = if signed_ext && matches!(op, Op::ArithShiftR) {
        xs[n - 1]
    } else {
        crate::ir::NET_CONST0
    };

    let stages = if n <= 1 {
        0
    } else {
        (usize::BITS - (n - 1).leading_zeros()) as usize
    };

    let mut cur = xs.to_vec();
    for i in 0..stages {
        if i >= amount.len() {
            break;
        }
        let shift_amount = 1usize << i;
        let mut shifted = Vec::with_capacity(n);
        for j in 0..n {
            let src = match op {
                Op::LogicShiftL | Op::ArithShiftL => j
                    .checked_sub(shift_amount)
                    .map(|k| cur[k])
                    .unwrap_or(crate::ir::NET_CONST0),
                Op::LogicShiftR | Op::ArithShiftR => {
                    if j + shift_amount < n {
                        cur[j + shift_amount]
                    } else {
                        fill
                    }
                }
                _ => crate::ir::NET_CONST0,
            };
            shifted.push(src);
        }
        let next: Vec<NetId> = cur
            .iter()
            .zip(shifted.iter())
            .map(|(&c, &s)| ctx.add_cell(CellKind::Mux2, vec![amount[i], c, s]))
            .collect();
        cur = next;
    }

    if amount.len() > stages {
        let any_high = crate::conv::expression::reduce_or(ctx, &amount[stages..]);
        cur = cur
            .iter()
            .map(|&c| ctx.add_cell(CellKind::Mux2, vec![any_high, c, fill]))
            .collect();
    }

    cur
}

/// Number of index bits required to address `n` elements.
pub(crate) fn index_bits_for(n: usize) -> usize {
    if n <= 1 {
        1
    } else {
        (usize::BITS - (n - 1).leading_zeros()) as usize
    }
}

/// Combinationally: returns 1 when the little-endian net vector `idx` matches
/// the constant `k`, else 0. Missing high bits of `idx` are treated as 0;
/// high bits of `k` beyond `idx.len()` force the result to 0.
pub(crate) fn eq_const(ctx: &mut ConvContext, idx: &[NetId], k: usize) -> NetId {
    if idx.is_empty() {
        return if k == 0 {
            crate::ir::NET_CONST1
        } else {
            crate::ir::NET_CONST0
        };
    }
    let mut bits = Vec::with_capacity(idx.len());
    for (i, &n) in idx.iter().enumerate() {
        let bit = (k >> i) & 1 == 1;
        let const_net = if bit {
            crate::ir::NET_CONST1
        } else {
            crate::ir::NET_CONST0
        };
        bits.push(ctx.add_cell(CellKind::Xnor2, vec![n, const_net]));
    }
    if (k >> idx.len()) != 0 {
        // k has bits above idx width — can never equal.
        return crate::ir::NET_CONST0;
    }
    crate::conv::expression::reduce_and(ctx, &bits)
}

/// log₂(n) stage 2-to-1 MUX tree that selects among `elements` using
/// `sel_bits` (LSB first). Each element must have the same width. Padding
/// with constant-0 elements keeps the tree balanced when N is not a power of 2.
pub(crate) fn dynamic_mux_tree(
    ctx: &mut ConvContext,
    elements: &[Vec<NetId>],
    sel_bits: &[NetId],
) -> Vec<NetId> {
    assert!(!elements.is_empty(), "empty element list");
    let elem_width = elements[0].len();
    let mut level: Vec<Vec<NetId>> = elements.to_vec();
    let zero_elem: Vec<NetId> = vec![crate::ir::NET_CONST0; elem_width];
    for &sel in sel_bits {
        if level.len() == 1 {
            break;
        }
        let mut next: Vec<Vec<NetId>> = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            let a = &pair[0];
            let b = pair.get(1).unwrap_or(&zero_elem);
            let merged: Vec<NetId> = a
                .iter()
                .zip(b.iter())
                .map(|(&ai, &bi)| ctx.add_cell(CellKind::Mux2, vec![sel, ai, bi]))
                .collect();
            next.push(merged);
        }
        level = next;
    }
    level.remove(0)
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
