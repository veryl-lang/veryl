use crate::conv::ConvContext;
use crate::conv::expression::{reduce_and, reduce_or};
use crate::ir::{CellKind, NET_CONST0, NET_CONST1, NetId};
use crate::synthesizer_error::SynthesizerError;
use std::mem;
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

/// `a + b + cin`. Dispatches to ripple-carry for narrow adds where the
/// ripple's compact area wins, and Kogge-Stone prefix adder for wider adds
/// where the O(log N) depth pays off. The threshold is chosen so that KS's
/// extra prefix logic isn't amortized poorly on small widths.
pub(crate) fn ripple_add(
    ctx: &mut ConvContext,
    a: &[NetId],
    b: &[NetId],
    cin: NetId,
) -> Result<Vec<NetId>, SynthesizerError> {
    if a.len() != b.len() {
        return Err(SynthesizerError::internal("add operand width mismatch"));
    }
    if a.len() < 4 {
        Ok(ripple_add_core(ctx, a, b, cin))
    } else {
        Ok(kogge_stone_add(ctx, a, b, cin))
    }
}

fn ripple_add_core(ctx: &mut ConvContext, a: &[NetId], b: &[NetId], cin: NetId) -> Vec<NetId> {
    let mut sum = Vec::with_capacity(a.len());
    let mut carry = cin;
    for i in 0..a.len() {
        let (s, c) = full_adder(ctx, a[i], b[i], carry);
        sum.push(s);
        carry = c;
    }
    sum
}

/// Kogge-Stone parallel-prefix adder. Depth is O(log N) vs ripple's O(N).
///   seed: P[i] = A[i] XOR B[i], G[i] = A[i] AND B[i]
///   prefix (d = 1, 2, 4, ...):
///     (P[i], G[i]) ← (P[i] AND P[i-d], G[i] OR (P[i] AND G[i-d]))
///   carry[i] = G[i-1] OR (P[i-1] AND cin); carry[0] = cin
///   sum[i]   = P_seed[i] XOR carry[i]
fn kogge_stone_add(ctx: &mut ConvContext, a: &[NetId], b: &[NetId], cin: NetId) -> Vec<NetId> {
    let n = a.len();
    if n == 0 {
        return Vec::new();
    }
    let p_seed: Vec<NetId> = (0..n)
        .map(|i| ctx.add_cell(CellKind::Xor2, vec![a[i], b[i]]))
        .collect();
    let g_seed: Vec<NetId> = (0..n)
        .map(|i| ctx.add_cell(CellKind::And2, vec![a[i], b[i]]))
        .collect();

    let mut p = p_seed.clone();
    let mut g = g_seed.clone();
    let mut d = 1;
    while d < n {
        let mut p_new = p.clone();
        let mut g_new = g.clone();
        for i in d..n {
            let p_and = ctx.add_cell(CellKind::And2, vec![p[i], p[i - d]]);
            let p_and_g = ctx.add_cell(CellKind::And2, vec![p[i], g[i - d]]);
            let g_or = ctx.add_cell(CellKind::Or2, vec![g[i], p_and_g]);
            p_new[i] = p_and;
            g_new[i] = g_or;
        }
        p = p_new;
        g = g_new;
        d *= 2;
    }

    let mut sum = Vec::with_capacity(n);
    let sum_0 = ctx.add_cell(CellKind::Xor2, vec![p_seed[0], cin]);
    sum.push(sum_0);
    for i in 1..n {
        let p_and_cin = ctx.add_cell(CellKind::And2, vec![p[i - 1], cin]);
        let carry_i = ctx.add_cell(CellKind::Or2, vec![g[i - 1], p_and_cin]);
        let sum_i = ctx.add_cell(CellKind::Xor2, vec![p_seed[i], carry_i]);
        sum.push(sum_i);
    }
    sum
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
    ripple_add(ctx, a, &inv_b, NET_CONST1)
}

/// 1-bit result: AND-reduction of per-bit XNOR. Operands must be the same width.
pub(crate) fn equal(ctx: &mut ConvContext, a: &[NetId], b: &[NetId]) -> NetId {
    if a.is_empty() {
        return NET_CONST1;
    }
    let mut eq_bits = Vec::with_capacity(a.len());
    for i in 0..a.len() {
        eq_bits.push(ctx.add_cell(CellKind::Xnor2, vec![a[i], b[i]]));
    }
    reduce_and(ctx, &eq_bits)
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
        return Ok(NET_CONST0);
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
        a_ext.push(NET_CONST0);
        let mut b_ext = b.to_vec();
        b_ext.push(NET_CONST0);
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
        NET_CONST0
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
    if result_width >= 4 {
        wallace_multiply(ctx, &a_ext, &b_ext, result_width)
    } else {
        shift_add_multiply(ctx, &a_ext, &b_ext, result_width)
    }
}

fn shift_add_multiply(
    ctx: &mut ConvContext,
    a_ext: &[NetId],
    b_ext: &[NetId],
    result_width: usize,
) -> Result<Vec<NetId>, SynthesizerError> {
    let mut acc = vec![NET_CONST0; result_width];
    for i in 0..result_width {
        let mut row = vec![NET_CONST0; result_width];
        for j in i..result_width {
            row[j] = ctx.add_cell(CellKind::And2, vec![b_ext[i], a_ext[j - i]]);
        }
        acc = ripple_add(ctx, &acc, &row, NET_CONST0)?;
    }
    Ok(acc)
}

/// Wallace-tree multiplier. Produces all AND-array partial products, groups
/// them per output-column bucket, then iteratively compresses each bucket to
/// ≤ 2 entries using full/half adders (3:2 and 2:2 compressors). The leftover
/// two rows are summed by a final carry-propagate adder (Kogge-Stone via
/// `ripple_add`). Depth is O(log N) in the compression vs shift-add's O(N).
fn wallace_multiply(
    ctx: &mut ConvContext,
    a_ext: &[NetId],
    b_ext: &[NetId],
    result_width: usize,
) -> Result<Vec<NetId>, SynthesizerError> {
    let mut buckets: Vec<Vec<NetId>> = vec![Vec::new(); result_width];
    for i in 0..result_width {
        for j in 0..(result_width - i) {
            let pp = ctx.add_cell(CellKind::And2, vec![b_ext[i], a_ext[j]]);
            buckets[i + j].push(pp);
        }
    }

    loop {
        let max_h = buckets.iter().map(|b| b.len()).max().unwrap_or(0);
        if max_h <= 2 {
            break;
        }
        let mut next: Vec<Vec<NetId>> = vec![Vec::new(); result_width];
        for c in 0..result_width {
            let col = mem::take(&mut buckets[c]);
            let mut k = 0;
            while k + 2 < col.len() {
                let (s, cy) = full_adder(ctx, col[k], col[k + 1], col[k + 2]);
                next[c].push(s);
                if c + 1 < result_width {
                    next[c + 1].push(cy);
                }
                k += 3;
            }
            if col.len() - k == 2 {
                // Half adder on the tail pair: reduces the current column by 1
                // and pushes a carry into c+1, helping the sequence converge.
                let s = ctx.add_cell(CellKind::Xor2, vec![col[k], col[k + 1]]);
                let cy = ctx.add_cell(CellKind::And2, vec![col[k], col[k + 1]]);
                next[c].push(s);
                if c + 1 < result_width {
                    next[c + 1].push(cy);
                }
                k += 2;
            }
            while k < col.len() {
                next[c].push(col[k]);
                k += 1;
            }
        }
        buckets = next;
    }

    let mut row0 = vec![NET_CONST0; result_width];
    let mut row1 = vec![NET_CONST0; result_width];
    for c in 0..result_width {
        if !buckets[c].is_empty() {
            row0[c] = buckets[c][0];
        }
        if buckets[c].len() >= 2 {
            row1[c] = buckets[c][1];
        }
    }
    ripple_add(ctx, &row0, &row1, NET_CONST0)
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
    let zero = vec![NET_CONST0; x.len()];
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
    b_ext.push(NET_CONST0);

    let mut rem: Vec<NetId> = vec![NET_CONST0; n + 1];
    let mut quo: Vec<NetId> = vec![NET_CONST0; n];

    for i in (0..n).rev() {
        let mut shifted = vec![NET_CONST0; n + 1];
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
        NET_CONST0
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
                    .unwrap_or(NET_CONST0),
                Op::LogicShiftR | Op::ArithShiftR => {
                    if j + shift_amount < n {
                        cur[j + shift_amount]
                    } else {
                        fill
                    }
                }
                _ => NET_CONST0,
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
        let any_high = reduce_or(ctx, &amount[stages..]);
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
        return if k == 0 { NET_CONST1 } else { NET_CONST0 };
    }
    let mut bits = Vec::with_capacity(idx.len());
    for (i, &n) in idx.iter().enumerate() {
        let bit = (k >> i) & 1 == 1;
        let const_net = if bit { NET_CONST1 } else { NET_CONST0 };
        bits.push(ctx.add_cell(CellKind::Xnor2, vec![n, const_net]));
    }
    if (k >> idx.len()) != 0 {
        // k has bits above idx width — can never equal.
        return NET_CONST0;
    }
    reduce_and(ctx, &bits)
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
    let zero_elem: Vec<NetId> = vec![NET_CONST0; elem_width];
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
    let mut out = vec![NET_CONST0; width];
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
                NET_CONST0
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
