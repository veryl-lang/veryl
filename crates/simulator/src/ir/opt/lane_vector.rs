//! Bit-lane structure recovery: two passes that undo the per-bit shredding a
//! source design performs explicitly, so the backends see word-wide work.
//!
//! RTL that transposes a vector (`rev[j][i] = m[i][j]`) and then reduces each
//! row (`out[j] = |rev[j]`) writes W×N one-bit statements — a sub-word RMW per
//! bit plus a row variable that exists only to be reduced.  Both passes below
//! are shape recoveries, not reassociations: every bit of every result is the
//! same expression it was before.
//!
//! [`lane_merge`] is legal because the operators it merges are bitwise: bit
//! `j` of the merged expression is exactly lane `j`.
//!
//! The two run around the field-store coalescing: the fold turns the
//! reduction rows into one-bit stores of the destination word, the coalescing
//! gathers them into a concatenation, and the merge collapses that
//! concatenation to word width.  Both are stages of
//! [`comb_fusion::inline_single_readers`](super::comb_fusion::inline_single_readers),
//! so a caller that needs every variable to hold its settled value — a
//! waveform dump — turns them off with the fusion.

use crate::HashMap;
use crate::HashSet;
use crate::ir::big_array::BigArrayFold;
use crate::ir::expression::{ExpressionContext, ProtoExpression};
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;
use veryl_analyzer::ir::Op;
use veryl_analyzer::value::Value;

/// `VERYL_LANE_VECTOR=0` opts both passes out.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_LANE_VECTOR").as_deref() != Ok("0"))
}

/// `VERYL_LANE_FOLD=0` / `VERYL_LANE_MERGE=0`: per-pass A/B levers.
pub fn fold_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    enabled() && *ON.get_or_init(|| std::env::var("VERYL_LANE_FOLD").as_deref() != Ok("0"))
}

pub fn merge_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    enabled() && *ON.get_or_init(|| std::env::var("VERYL_LANE_MERGE").as_deref() != Ok("0"))
}

/// Operations a merge must remove to be taken.  A merged leaf reads a window
/// of a variable where the lane read one bit of it, so a merge that removes
/// only a few operations pays the wider read without buying anything —
/// measured a loss on designs whose lanes are two and four bits wide.
/// `VERYL_LANE_MERGE_MIN_OPS` overrides; 0 takes every merge.
fn merge_min_ops() -> usize {
    const DEFAULT: usize = 16;
    #[cfg(test)]
    if let Some(v) = TEST_MIN_OPS.with(|c| c.get()) {
        return v;
    }
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("VERYL_LANE_MERGE_MIN_OPS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT)
    })
}

#[cfg(test)]
thread_local! {
    static TEST_MIN_OPS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_LANE_VECTOR_DIAG").as_deref() == Ok("1"))
}

fn ctx(width: usize) -> ExpressionContext {
    ExpressionContext {
        width,
        signed: false,
    }
}

/// Clip an expression to `w` bits, as the store into a `w`-bit field did.
/// Unconditional: a `w`-wide expression is not thereby `w` bits clean —
/// `~x` reaches a consumer with every bit above the width set — and the
/// backends disagree on which forms leave dirt, so the mask is what makes
/// the replacement carry the guarantee the original had.  A mask that covers
/// every bit its operand can hold is elided when the statement is emitted.
fn clip(expr: ProtoExpression, w: usize) -> ProtoExpression {
    if w >= 64 {
        return expr;
    }
    let mask = (1u64 << w) - 1;
    ProtoExpression::Binary {
        x: Box::new(expr),
        op: Op::BitAnd,
        y: Box::new(ProtoExpression::Value {
            value: Value::new(mask, w, false),
            width: w,
            expr_context: ctx(w),
        }),
        width: w,
        expr_context: ctx(w),
    }
}

// ---------------------------------------------------------------------------
// transpose fold
// ---------------------------------------------------------------------------

/// Statements that write bit `lo` of a comb offset and nothing else.
struct BitWriters {
    /// Statement index per bit.
    by_bit: Vec<Option<usize>>,
    /// Disqualified: a write this pass cannot account for.
    bad: bool,
}

/// Retire comb variables that exist only to be reduced.  `dead` collects the
/// retired offsets (their storage is never written afterwards).  Returns the
/// number of reductions rewritten.
/// `fold` must be the caller's: `externals` is keyed by the offsets it
/// produces, so a different fold would look up an element under a key the
/// caller never wrote.
pub fn transpose_fold(
    stmts: &mut Vec<ProtoStatement>,
    fold: &BigArrayFold,
    externals: &HashSet<isize>,
    dead: &mut Vec<isize>,
) -> usize {
    let n = stmts.len();
    let mut cands: HashMap<isize, BitWriters> = HashMap::default();
    for (i, s) in stmts.iter().enumerate() {
        let ProtoStatement::Assign(a) = s else {
            continue;
        };
        let VarOffset::Comb(o) = a.dst else {
            continue;
        };
        // `read_idxs` / `write_idxs` are array-wide for a folded element —
        // too coarse for the legality window below.
        if fold.covers(a.dst) {
            continue;
        }
        let w = a.dst_width;
        let e = cands.entry(o).or_insert_with(|| BitWriters {
            by_bit: vec![None; w],
            bad: false,
        });
        // A width that disagrees with an earlier write would misplace the
        // bit, so it disqualifies the offset like any other shape.
        let single_bit = a.select.map(|(hi, lo)| hi == lo).unwrap_or(false)
            && a.dynamic_select.is_none()
            && a.rhs_select.is_none()
            && a.expr.store_sign_extend_from(w).is_none()
            && e.by_bit.len() == w;
        if !single_bit {
            e.bad = true;
            continue;
        }
        let (_, lo) = a.select.unwrap();
        match e.by_bit.get_mut(lo) {
            // A bit written twice: last-writer-wins would need ordering the
            // fold, and the reduction reads only the final value.
            Some(slot) if slot.is_none() => *slot = Some(i),
            _ => e.bad = true,
        }
    }
    cands.retain(|o, c| {
        !c.bad
            && c.by_bit.len() >= 2
            && c.by_bit.iter().all(|b| b.is_some())
            && !externals.contains(o)
    });
    if cands.is_empty() {
        return 0;
    }

    let mut read_idxs: HashMap<isize, Vec<usize>> = HashMap::default();
    let mut write_idxs: HashMap<isize, Vec<usize>> = HashMap::default();
    {
        let mut ins: Vec<VarOffset> = vec![];
        let mut outs: Vec<VarOffset> = vec![];
        for (i, s) in stmts.iter().enumerate() {
            ins.clear();
            outs.clear();
            s.gather_variable_offsets_expanded(fold, &mut ins, &mut outs);
            for off in ins.drain(..) {
                if let VarOffset::Comb(o) = off {
                    let v = read_idxs.entry(o).or_default();
                    if v.last() != Some(&i) {
                        v.push(i);
                    }
                }
            }
            for off in outs.drain(..) {
                if let VarOffset::Comb(o) = off {
                    let v = write_idxs.entry(o).or_default();
                    if v.last() != Some(&i) {
                        v.push(i);
                    }
                }
            }
        }
    }

    let mut folded = 0usize;
    let mut deleted = vec![false; n];
    let mut offs: Vec<isize> = cands.keys().copied().collect();
    offs.sort_unstable();
    let mut ins: Vec<VarOffset> = vec![];
    let mut outs: Vec<VarOffset> = vec![];
    for o in offs {
        let c = &cands[&o];
        let Some(readers) = read_idxs.get(&o) else {
            continue;
        };
        if readers.len() != 1 {
            continue;
        }
        let r = readers[0];
        let first = c.by_bit.iter().flatten().copied().min().unwrap();
        let last = c.by_bit.iter().flatten().copied().max().unwrap();
        if r <= last || deleted[r] {
            continue;
        }
        // The stores are the only writes.
        let writers = write_idxs.get(&o).map(Vec::as_slice).unwrap_or(&[]);
        if writers.len() != c.by_bit.len() {
            continue;
        }
        // Every store's RHS is evaluated at the reduction instead, so nothing
        // in the window may rewrite an input of any of them.
        let mut input_rewritten = false;
        'legality: for &i in c.by_bit.iter().flatten() {
            if deleted[i] {
                input_rewritten = true;
                break 'legality;
            }
            ins.clear();
            outs.clear();
            stmts[i].gather_variable_offsets_expanded(fold, &mut ins, &mut outs);
            for off in &ins {
                if let VarOffset::Comb(io) = off
                    && write_idxs
                        .get(io)
                        .map(|v| v.iter().any(|&x| x >= first && x <= r))
                        .unwrap_or(false)
                {
                    input_rewritten = true;
                    break 'legality;
                }
            }
        }
        if input_rewritten {
            continue;
        }

        let w = c.by_bit.len();
        // Try the rewrite on a copy of the reader: it fails if the offset is
        // reachable other than through a full-width reduction.
        let ProtoStatement::Assign(ra) = &stmts[r] else {
            continue;
        };
        let mut expr = ra.expr.clone();
        let mut hits = 0usize;
        let bits: Vec<ProtoExpression> = c
            .by_bit
            .iter()
            .map(|b| {
                let ProtoStatement::Assign(a) = &stmts[b.unwrap()] else {
                    unreachable!()
                };
                clip(a.expr.clone(), 1)
            })
            .collect();
        rewrite_reductions(&mut expr, o, w, &bits, &mut hits);
        if hits == 0 || mentions(&expr, o) {
            continue;
        }
        let ProtoStatement::Assign(ra) = &mut stmts[r] else {
            unreachable!()
        };
        ra.expr = expr;
        for &i in c.by_bit.iter().flatten() {
            deleted[i] = true;
        }
        dead.push(o);
        folded += 1;
        if diag() {
            eprintln!("[lane_vector] transpose fold: off={o:#x} bits={w} reader={r}");
        }
    }

    if folded > 0 {
        let mut i = 0;
        stmts.retain(|_| {
            let keep = !deleted[i];
            i += 1;
            keep
        });
    }
    folded
}

/// Replace every `|offset` (full width) with the stored bits combined, in
/// place.  Other reads of `offset` are left alone for the caller to detect.
fn rewrite_reductions(
    expr: &mut ProtoExpression,
    offset: isize,
    width: usize,
    bits: &[ProtoExpression],
    hits: &mut usize,
) {
    if let ProtoExpression::Unary {
        op,
        x,
        width: w,
        expr_context,
    } = expr
        && matches!(op, Op::BitOr | Op::BitAnd | Op::BitXor)
        && *w == 1
        && is_full_read(x, offset, width)
    {
        let op = *op;
        // The replacement stands where the reduction stood, so it keeps the
        // reduction's own width and context.
        let root_ctx = *expr_context;
        let mut it = bits.iter();
        let mut acc = it.next().unwrap().clone();
        for b in it {
            acc = ProtoExpression::Binary {
                x: Box::new(acc),
                op,
                y: Box::new(b.clone()),
                width: 1,
                expr_context: ctx(1),
            };
        }
        if let ProtoExpression::Binary { expr_context, .. } = &mut acc {
            *expr_context = root_ctx;
        }
        *expr = acc;
        *hits += 1;
        return;
    }
    for child in children_mut(expr) {
        rewrite_reductions(child, offset, width, bits, hits);
    }
}

/// A read of every bit of `offset`.
fn is_full_read(expr: &ProtoExpression, offset: isize, width: usize) -> bool {
    let ProtoExpression::Variable {
        var_offset,
        select,
        dynamic_select,
        var_full_width,
        ..
    } = expr
    else {
        return false;
    };
    *var_offset == VarOffset::Comb(offset)
        && dynamic_select.is_none()
        && *var_full_width == width
        && match select {
            None => true,
            Some((hi, lo)) => *lo == 0 && *hi == width - 1,
        }
}

/// Any reference to `offset` left in the expression — including the index of
/// a dynamic select and every element a dynamic read can reach, which the
/// operand walk does not visit.
fn mentions(expr: &ProtoExpression, offset: isize) -> bool {
    expr.reads_offset(VarOffset::Comb(offset))
}

// ---------------------------------------------------------------------------
// lane merge
// ---------------------------------------------------------------------------

/// Collapse concatenations of one-bit lanes into word-wide expressions.
/// Returns the number of concatenations rewritten.
pub fn lane_merge(stmts: &mut [ProtoStatement]) -> usize {
    let mut merged = 0usize;
    for s in stmts.iter_mut() {
        let ProtoStatement::Assign(a) = s else {
            continue;
        };
        // A static select store carries the same shape in a slice of a wider
        // destination (a word of a bit-assembled vector); the store clips to
        // the field either way, so the merged word is equally bounded.
        if a.dynamic_select.is_some() || a.rhs_select.is_some() {
            continue;
        }
        let ProtoExpression::Concatenation {
            elements, width, ..
        } = &a.expr
        else {
            continue;
        };
        let w = *width;
        if let Some((hi, lo)) = a.select
            && hi.max(lo) - hi.min(lo) + 1 != w
        {
            continue;
        }
        // Wide results would need the multi-word paths for every node built
        // here; the shapes this recovers are word-sized destinations.
        if !(2..=64).contains(&w) || elements.len() != w {
            continue;
        }
        if elements
            .iter()
            .any(|(_, repeat, ew)| *repeat != 1 || *ew != 1)
        {
            continue;
        }
        // The concatenation is written high bit first.
        let lanes: Vec<(&ProtoExpression, usize)> = elements
            .iter()
            .enumerate()
            .map(|(k, (e, _, _))| (e.as_ref(), w - 1 - k))
            .collect();
        // Every lane is the same tree, so one lane sizes what a merge saves:
        // `w` copies of it become one.
        let saved = (w - 1) * node_count(lanes[0].0);
        if saved < merge_min_ops() {
            continue;
        }
        let Some(expr) = merge_lanes(&lanes, w) else {
            continue;
        };
        // The concatenation bounded every lane into its slot; the merged
        // operators do not, so the result carries the same guarantee only
        // once clipped.
        let expr = clip(expr, w);
        if diag() {
            eprintln!(
                "[lane_vector] lane merge: dst={:?} width={w} saved={saved}",
                a.dst
            );
        }
        a.expr = expr;
        merged += 1;
    }
    merged
}

/// The same plain one-bit read in every lane, or `None` if any lane differs
/// or the read is not plain.
fn lane_invariant_read<'a>(lanes: &[(&'a ProtoExpression, usize)]) -> Option<&'a ProtoExpression> {
    let head = lanes[0].0;
    let ProtoExpression::Variable {
        var_offset,
        select,
        dynamic_select: None,
        width,
        var_full_width,
        ..
    } = head
    else {
        return None;
    };
    if *width != 1 {
        return None;
    }
    for (e, _) in &lanes[1..] {
        let ProtoExpression::Variable {
            var_offset: vo,
            select: sel,
            dynamic_select: None,
            width: wd,
            var_full_width: vfw,
            ..
        } = e
        else {
            return None;
        };
        if vo != var_offset || sel != select || wd != width || vfw != var_full_width {
            return None;
        }
    }
    Some(head)
}

/// Merge per-lane one-bit expressions into one `w`-bit expression whose bit
/// `j` is lane `j`, or `None` if the lanes are not one shape over one set of
/// variables.
fn merge_lanes(lanes: &[(&ProtoExpression, usize)], w: usize) -> Option<ProtoExpression> {
    let (head, _) = lanes[0];
    // A leaf that does not follow the lane at all: every lane holds the same
    // bit, so the merged word is that bit replicated.  Only a plain read is
    // matched — a compound invariant subtree arrives here through the
    // operator arms, whose operands are replicated one by one instead.
    if let Some(v) = lane_invariant_read(lanes) {
        return Some(ProtoExpression::Concatenation {
            elements: vec![(Box::new(v.clone()), w, 1)],
            width: w,
            expr_context: ctx(w),
        });
    }
    match head {
        ProtoExpression::Variable {
            var_offset,
            select: Some((hi, lo)),
            dynamic_select: None,
            var_full_width,
            ..
        } if hi == lo => {
            // The leaf selects bit `lane + base` for a base common to every
            // lane, so the merged read is one `w`-bit window.
            let base = hi.checked_sub(lanes[0].1)?;
            if base + w > *var_full_width {
                return None;
            }
            for (e, lane) in lanes {
                let ProtoExpression::Variable {
                    var_offset: vo,
                    select: Some((h, l)),
                    dynamic_select: None,
                    var_full_width: vfw,
                    ..
                } = e
                else {
                    return None;
                };
                if h != l || *h != lane + base || vo != var_offset || vfw != var_full_width {
                    return None;
                }
            }
            Some(ProtoExpression::Variable {
                var_offset: *var_offset,
                select: Some((base + w - 1, base)),
                dynamic_select: None,
                width: w,
                var_full_width: *var_full_width,
                expr_context: ctx(w),
            })
        }
        ProtoExpression::Value { value, .. } => {
            // One constant bit in every lane broadcasts to a constant word.
            let bit = value.to_u64()?;
            if bit > 1 {
                return None;
            }
            for (e, _) in lanes {
                let ProtoExpression::Value { value: v, .. } = e else {
                    return None;
                };
                if v.to_u64() != Some(bit) {
                    return None;
                }
            }
            let payload = if bit == 1 { mask_of(w) } else { 0 };
            Some(ProtoExpression::Value {
                value: Value::new(payload, w, false),
                width: w,
                expr_context: ctx(w),
            })
        }
        ProtoExpression::Unary { op, x, .. } => {
            // `!x` on a one-bit operand is that bit inverted, which is what
            // the merged bitwise complement does per lane.
            let bitwise = match op {
                Op::BitNot => Op::BitNot,
                Op::LogicNot if x.width() == 1 => Op::BitNot,
                _ => return None,
            };
            let mut kids = Vec::with_capacity(lanes.len());
            for (e, lane) in lanes {
                let ProtoExpression::Unary { op: o, x, .. } = e else {
                    return None;
                };
                if o != op || x.width() != 1 {
                    return None;
                }
                kids.push((x.as_ref(), *lane));
            }
            Some(ProtoExpression::Unary {
                op: bitwise,
                x: Box::new(merge_lanes(&kids, w)?),
                width: w,
                expr_context: ctx(w),
            })
        }
        ProtoExpression::Binary { x, op, y, .. } => {
            // The logical forms reduce their operands to one bit first; on
            // one-bit operands that is the bitwise operation.
            let bitwise = match op {
                Op::BitOr | Op::BitAnd | Op::BitXor => *op,
                Op::LogicOr if x.width() == 1 && y.width() == 1 => Op::BitOr,
                Op::LogicAnd if x.width() == 1 && y.width() == 1 => Op::BitAnd,
                _ => return None,
            };
            let mut xs = Vec::with_capacity(lanes.len());
            let mut ys = Vec::with_capacity(lanes.len());
            for (e, lane) in lanes {
                let ProtoExpression::Binary { x, op: o, y, .. } = e else {
                    return None;
                };
                if o != op || x.width() != 1 || y.width() != 1 {
                    return None;
                }
                xs.push((x.as_ref(), *lane));
                ys.push((y.as_ref(), *lane));
            }
            Some(ProtoExpression::Binary {
                x: Box::new(merge_lanes(&xs, w)?),
                op: bitwise,
                y: Box::new(merge_lanes(&ys, w)?),
                width: w,
                expr_context: ctx(w),
            })
        }
        _ => None,
    }
}

fn mask_of(w: usize) -> u64 {
    if w >= 64 { u64::MAX } else { (1u64 << w) - 1 }
}

// ---------------------------------------------------------------------------

fn node_count(expr: &ProtoExpression) -> usize {
    1 + children(expr).map(node_count).sum::<usize>()
}

fn children(expr: &ProtoExpression) -> Box<dyn Iterator<Item = &ProtoExpression> + '_> {
    match expr {
        ProtoExpression::Unary { x, .. } => Box::new(std::iter::once(x.as_ref())),
        ProtoExpression::Binary { x, y, .. } => Box::new([x.as_ref(), y.as_ref()].into_iter()),
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => Box::new([cond.as_ref(), true_expr.as_ref(), false_expr.as_ref()].into_iter()),
        ProtoExpression::Concatenation { elements, .. } => {
            Box::new(elements.iter().map(|(e, _, _)| e.as_ref()))
        }
        _ => Box::new(std::iter::empty()),
    }
}

fn children_mut(expr: &mut ProtoExpression) -> Vec<&mut ProtoExpression> {
    match expr {
        ProtoExpression::Unary { x, .. } => vec![x.as_mut()],
        ProtoExpression::Binary { x, y, .. } => vec![x.as_mut(), y.as_mut()],
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => vec![cond.as_mut(), true_expr.as_mut(), false_expr.as_mut()],
        ProtoExpression::Concatenation { elements, .. } => {
            elements.iter_mut().map(|(e, _, _)| e.as_mut()).collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::statement::ProtoAssignStatement;
    use veryl_analyzer::value::ValueU64;
    use veryl_parser::token_range::TokenRange;

    fn lit(v: u64, w: usize) -> ProtoExpression {
        ProtoExpression::Value {
            value: Value::U64(ValueU64 {
                payload: v,
                mask_xz: 0,
                width: w as u32,
                signed: false,
            }),
            width: w,
            expr_context: ctx(w),
        }
    }

    fn bit(off: isize, bit: usize, full: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: Some((bit, bit)),
            dynamic_select: None,
            width: 1,
            var_full_width: full,
            expr_context: ctx(1),
        }
    }

    fn reduce(op: Op, off: isize, w: usize) -> ProtoExpression {
        ProtoExpression::Unary {
            op,
            x: Box::new(ProtoExpression::Variable {
                var_offset: VarOffset::Comb(off),
                select: Some((w - 1, 0)),
                dynamic_select: None,
                width: w,
                var_full_width: w,
                expr_context: ctx(w),
            }),
            width: 1,
            expr_context: ctx(1),
        }
    }

    fn binop(x: ProtoExpression, op: Op, y: ProtoExpression, w: usize) -> ProtoExpression {
        ProtoExpression::Binary {
            x: Box::new(x),
            op,
            y: Box::new(y),
            width: w,
            expr_context: ctx(w),
        }
    }

    fn store(
        dst: isize,
        w: usize,
        select: Option<(usize, usize)>,
        expr: ProtoExpression,
    ) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(dst),
            dst_width: w,
            select,
            dynamic_select: None,
            rhs_select: None,
            expr,
            dst_ff_current_offset: 0,
            token: TokenRange::default(),
        })
    }

    /// Run `lane_merge` with the benefit threshold out of the way: the shape
    /// tests are about what merges, not about when it is worth it.
    fn merge_any(stmts: &mut [ProtoStatement]) -> usize {
        TEST_MIN_OPS.with(|c| c.set(Some(0)));
        let n = lane_merge(stmts);
        TEST_MIN_OPS.with(|c| c.set(None));
        n
    }

    fn concat(elements: Vec<ProtoExpression>) -> ProtoExpression {
        let w = elements.len();
        ProtoExpression::Concatenation {
            elements: elements.into_iter().map(|e| (Box::new(e), 1, 1)).collect(),
            width: w,
            expr_context: ctx(w),
        }
    }

    /// `clip` puts a width mask on every replacement.
    fn unmask(e: &ProtoExpression) -> &ProtoExpression {
        match e {
            ProtoExpression::Binary {
                x,
                op: Op::BitAnd,
                y,
                ..
            } if matches!(y.as_ref(), ProtoExpression::Value { .. }) => x.as_ref(),
            _ => e,
        }
    }

    fn is_masked_to(e: &ProtoExpression, w: usize) -> bool {
        let ProtoExpression::Binary {
            op: Op::BitAnd, y, ..
        } = e
        else {
            return false;
        };
        let ProtoExpression::Value { value, .. } = y.as_ref() else {
            return false;
        };
        value.to_u64() == Some(mask_of(w))
    }

    fn expr_of(s: &ProtoStatement) -> &ProtoExpression {
        let ProtoStatement::Assign(a) = s else {
            panic!("expected an assign")
        };
        &a.expr
    }

    /// Row 0x40 holds two bits, each written once, and is read only by `|row`.
    fn transposed_row() -> Vec<ProtoStatement> {
        vec![
            store(0x40, 2, Some((0, 0)), bit(0x10, 3, 8)),
            store(0x40, 2, Some((1, 1)), bit(0x20, 3, 8)),
            store(0x80, 1, None, reduce(Op::BitOr, 0x40, 2)),
        ]
    }

    #[test]
    fn fold_retires_a_row_that_only_feeds_a_reduction() {
        let mut stmts = transposed_row();
        let mut dead = vec![];
        assert_eq!(
            transpose_fold(
                &mut stmts,
                &BigArrayFold::default(),
                &HashSet::default(),
                &mut dead
            ),
            1
        );
        assert_eq!(stmts.len(), 1);
        assert_eq!(dead, vec![0x40]);
        // The reduction now combines the stored bits directly, in bit order.
        let ProtoExpression::Binary {
            x, op, y, width, ..
        } = expr_of(&stmts[0])
        else {
            panic!("expected the folded combination")
        };
        assert_eq!((*op, *width), (Op::BitOr, 1));
        assert!(matches!(
            (unmask(x.as_ref()), unmask(y.as_ref())),
            (
                ProtoExpression::Variable {
                    var_offset: VarOffset::Comb(0x10),
                    ..
                },
                ProtoExpression::Variable {
                    var_offset: VarOffset::Comb(0x20),
                    ..
                }
            )
        ));
    }

    #[test]
    fn fold_keeps_a_row_read_other_than_through_the_reduction() {
        let mut stmts = transposed_row();
        // A second read of one bit: retiring the row would starve it.
        stmts.push(store(0x90, 1, None, bit(0x40, 0, 2)));
        let mut dead = vec![];
        assert_eq!(
            transpose_fold(
                &mut stmts,
                &BigArrayFold::default(),
                &HashSet::default(),
                &mut dead
            ),
            0
        );
        assert_eq!(stmts.len(), 4);
        assert!(dead.is_empty());
    }

    #[test]
    fn fold_keeps_a_row_whose_bits_are_not_all_stored() {
        // Bit 1 is never written, so the reduction also observes storage the
        // stores do not account for.
        let mut stmts = vec![
            store(0x40, 2, Some((0, 0)), bit(0x10, 3, 8)),
            store(0x80, 1, None, reduce(Op::BitOr, 0x40, 2)),
        ];
        let mut dead = vec![];
        assert_eq!(
            transpose_fold(
                &mut stmts,
                &BigArrayFold::default(),
                &HashSet::default(),
                &mut dead
            ),
            0
        );
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn fold_keeps_a_row_an_event_reads() {
        let mut stmts = transposed_row();
        let mut dead = vec![];
        let externals: HashSet<isize> = [0x40].into_iter().collect();
        assert_eq!(
            transpose_fold(&mut stmts, &BigArrayFold::default(), &externals, &mut dead),
            0
        );
        assert_eq!(stmts.len(), 3);
    }

    #[test]
    fn fold_keeps_a_row_whose_input_is_rewritten_before_the_reduction() {
        let mut stmts = transposed_row();
        // 0x10 is restored between the store and the reduction, so moving the
        // store's expression to the reduction would read the new value.
        stmts.insert(2, store(0x10, 8, None, lit(0, 8)));
        let mut dead = vec![];
        assert_eq!(
            transpose_fold(
                &mut stmts,
                &BigArrayFold::default(),
                &HashSet::default(),
                &mut dead
            ),
            0
        );
        assert_eq!(stmts.len(), 4);
    }

    #[test]
    fn merge_collapses_a_bit_lane_concatenation() {
        let lanes: Vec<ProtoExpression> = (0..4)
            .rev()
            .map(|j| binop(bit(0x10, j, 4), Op::BitAnd, bit(0x20, j, 4), 1))
            .collect();
        let mut stmts = vec![store(0x80, 4, None, concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 1);
        let ProtoExpression::Binary {
            x, op, y, width, ..
        } = unmask(expr_of(&stmts[0]))
        else {
            panic!("expected the merged word operation")
        };
        assert_eq!((*op, *width), (Op::BitAnd, 4));
        for (side, off) in [(x, 0x10), (y, 0x20)] {
            let ProtoExpression::Variable {
                var_offset,
                select,
                width,
                ..
            } = side.as_ref()
            else {
                panic!("expected a word-wide read")
            };
            assert_eq!(*var_offset, VarOffset::Comb(off));
            assert_eq!((*select, *width), (Some((3, 0)), 4));
        }
    }

    #[test]
    fn merge_reads_a_shifted_window_when_every_lane_shifts_alike() {
        let lanes: Vec<ProtoExpression> = (0..4).rev().map(|j| bit(0x10, j + 2, 8)).collect();
        let mut stmts = vec![store(0x80, 4, None, concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 1);
        let ProtoExpression::Variable { select, .. } = unmask(expr_of(&stmts[0])) else {
            panic!("expected a word-wide read")
        };
        assert_eq!(*select, Some((5, 2)));
    }

    #[test]
    fn merge_rejects_a_lane_that_selects_out_of_step() {
        // Lane 1 reads bit 0: the lanes are a permutation, not a window.
        let mut lanes: Vec<ProtoExpression> = (0..4).rev().map(|j| bit(0x10, j, 4)).collect();
        lanes[2] = bit(0x10, 0, 4);
        let mut stmts = vec![store(0x80, 4, None, concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 0);
        assert!(matches!(
            expr_of(&stmts[0]),
            ProtoExpression::Concatenation { .. }
        ));
    }

    #[test]
    fn merge_rejects_lanes_that_mix_operators() {
        let mut lanes: Vec<ProtoExpression> = (0..4)
            .rev()
            .map(|j| binop(bit(0x10, j, 4), Op::BitAnd, bit(0x20, j, 4), 1))
            .collect();
        lanes[1] = binop(bit(0x10, 2, 4), Op::BitOr, bit(0x20, 2, 4), 1);
        let mut stmts = vec![store(0x80, 4, None, concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 0);
    }

    #[test]
    fn merge_broadcasts_a_constant_lane() {
        // `a[j] & 1` in every lane: the constant becomes an all-ones word.
        let lanes: Vec<ProtoExpression> = (0..4)
            .rev()
            .map(|j| binop(bit(0x10, j, 4), Op::BitAnd, lit(1, 1), 1))
            .collect();
        let mut stmts = vec![store(0x80, 4, None, concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 1);
        let ProtoExpression::Binary { y, .. } = unmask(expr_of(&stmts[0])) else {
            panic!("expected the merged word operation")
        };
        let ProtoExpression::Value { value, width, .. } = y.as_ref() else {
            panic!("expected the broadcast constant")
        };
        assert_eq!((value.to_u64(), *width), (Some(0xf), 4));
    }

    #[test]
    fn merge_broadcasts_a_lane_invariant_read() {
        // `a[j] & b[2]`: one leaf follows the lane, the other holds the same
        // bit everywhere and becomes that bit replicated across the word.
        let lanes: Vec<ProtoExpression> = (0..4)
            .rev()
            .map(|j| binop(bit(0x10, j, 4), Op::BitAnd, bit(0x20, 2, 4), 1))
            .collect();
        let mut stmts = vec![store(0x80, 4, None, concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 1);
        let ProtoExpression::Binary { y, .. } = unmask(expr_of(&stmts[0])) else {
            panic!("expected the merged word operation")
        };
        let ProtoExpression::Concatenation {
            elements, width, ..
        } = y.as_ref()
        else {
            panic!("expected the replicated read")
        };
        assert_eq!((elements.len(), elements[0].1, *width), (1, 4, 4));
    }

    #[test]
    fn merge_takes_a_static_select_store() {
        // One word of a bit-assembled wider vector: the store selects the
        // word, and the lanes inside it merge like any other.
        let lanes: Vec<ProtoExpression> = (0..4)
            .rev()
            .map(|j| binop(bit(0x10, j, 4), Op::BitAnd, lit(1, 1), 1))
            .collect();
        let mut stmts = vec![store(0x80, 12, Some((7, 4)), concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 1);
    }

    #[test]
    fn merge_rejects_a_select_store_whose_width_disagrees() {
        let lanes: Vec<ProtoExpression> = (0..4)
            .rev()
            .map(|j| binop(bit(0x10, j, 4), Op::BitAnd, lit(1, 1), 1))
            .collect();
        let mut stmts = vec![store(0x80, 12, Some((8, 4)), concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 0);
    }

    #[test]
    fn merge_rejects_lanes_that_disagree_on_a_constant() {
        let mut lanes: Vec<ProtoExpression> = (0..4)
            .rev()
            .map(|j| binop(bit(0x10, j, 4), Op::BitAnd, lit(1, 1), 1))
            .collect();
        lanes[1] = binop(bit(0x10, 2, 4), Op::BitAnd, lit(0, 1), 1);
        let mut stmts = vec![store(0x80, 4, None, concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 0);
    }

    #[test]
    fn fold_then_merge_recovers_the_word_form() {
        // The shape the two passes exist for: four rows, each the transpose of
        // one bit position, reduced into the lanes of one word.
        let mut stmts = Vec::new();
        for j in 0..4 {
            let row = 0x100 + j as isize * 0x10;
            for i in 0..3 {
                stmts.push(store(
                    row,
                    3,
                    Some((i, i)),
                    bit(0x10 + i as isize * 8, j, 4),
                ));
            }
        }
        for j in 0..4 {
            let row = 0x100 + j as isize * 0x10;
            stmts.push(store(
                0x80,
                4,
                Some((j as usize, j as usize)),
                reduce(Op::BitOr, row, 3),
            ));
        }
        let mut dead = vec![];
        assert_eq!(
            transpose_fold(
                &mut stmts,
                &BigArrayFold::default(),
                &HashSet::default(),
                &mut dead
            ),
            4
        );
        assert_eq!(dead.len(), 4);
        // Coalescing the four one-bit stores is what the fusion pass does
        // between the two passes here.
        let lanes: Vec<ProtoExpression> =
            (0..4).rev().map(|j| expr_of(&stmts[j]).clone()).collect();
        let mut fused = vec![store(0x80, 4, None, concat(lanes))];
        assert_eq!(merge_any(&mut fused), 1);
        // Three word-wide reads OR'd together — one operation per source
        // instead of one per source and lane.
        let mut leaves = Vec::new();
        fn walk(e: &ProtoExpression, out: &mut Vec<(isize, Option<(usize, usize)>)>) {
            match e {
                ProtoExpression::Binary {
                    x,
                    op: Op::BitAnd,
                    y,
                    ..
                } if matches!(y.as_ref(), ProtoExpression::Value { .. }) => walk(x, out),
                ProtoExpression::Variable {
                    var_offset: VarOffset::Comb(o),
                    select,
                    ..
                } => out.push((*o, *select)),
                ProtoExpression::Binary { x, y, op, .. } => {
                    assert_eq!(*op, Op::BitOr);
                    walk(x, out);
                    walk(y, out);
                }
                _ => panic!("unexpected node in the merged expression"),
            }
        }
        walk(expr_of(&fused[0]), &mut leaves);
        assert_eq!(
            leaves,
            vec![
                (0x10, Some((3, 0))),
                (0x18, Some((3, 0))),
                (0x20, Some((3, 0)))
            ]
        );
    }
    #[test]
    fn merge_declines_when_it_removes_too_little() {
        // Two lanes of one read each: the merge would trade two bit reads for
        // one two-bit window and nothing else.
        let lanes: Vec<ProtoExpression> = (0..2).rev().map(|j| bit(0x10, j, 8)).collect();
        let mut stmts = vec![store(0x80, 2, None, concat(lanes))];
        assert_eq!(lane_merge(&mut stmts), 0);
        // The same shape merges once the threshold is out of the way, so the
        // decline is the benefit test and not the shape.
        assert_eq!(merge_any(&mut stmts), 1);
    }

    #[test]
    fn merge_takes_a_wide_lane_family_by_default() {
        // Sixteen lanes of `a[j] & b[j]`: 15 copies of a three-node tree go
        // away, well past the threshold.
        let lanes: Vec<ProtoExpression> = (0..16)
            .rev()
            .map(|j| binop(bit(0x10, j, 16), Op::BitAnd, bit(0x20, j, 16), 1))
            .collect();
        let mut stmts = vec![store(0x80, 16, None, concat(lanes))];
        assert_eq!(lane_merge(&mut stmts), 1);
    }
    /// `~x` is one bit wide but its payload is not one bit: the backends
    /// emit it with the bits above the width set.
    fn dirty_bit(off: isize, b: usize, full: usize) -> ProtoExpression {
        ProtoExpression::Unary {
            op: Op::BitNot,
            x: Box::new(bit(off, b, full)),
            width: 1,
            expr_context: ctx(1),
        }
    }

    #[test]
    fn fold_clips_a_stored_bit_that_is_not_clean() {
        // The reduction it replaces yielded 0 or 1; a bare `~` in its place
        // would leak set bits into whatever consumes the result.
        let mut stmts = vec![
            store(0x40, 2, Some((0, 0)), dirty_bit(0x10, 3, 8)),
            store(0x40, 2, Some((1, 1)), dirty_bit(0x20, 3, 8)),
            store(0x80, 8, None, reduce(Op::BitOr, 0x40, 2)),
        ];
        let mut dead = vec![];
        assert_eq!(
            transpose_fold(
                &mut stmts,
                &BigArrayFold::default(),
                &HashSet::default(),
                &mut dead
            ),
            1
        );
        let ProtoExpression::Binary { x, y, .. } = expr_of(&stmts[0]) else {
            panic!("expected the folded combination")
        };
        for side in [x.as_ref(), y.as_ref()] {
            assert!(
                is_masked_to(side, 1),
                "a folded bit reaches its consumer unclipped: {side:?}"
            );
        }
    }

    #[test]
    fn merge_clips_a_result_that_is_not_clean() {
        // Sixteen lanes of `~a[j]` into a wider destination: the
        // concatenation bounded each lane, the merged complement does not.
        let lanes: Vec<ProtoExpression> = (0..16).rev().map(|j| dirty_bit(0x10, j, 16)).collect();
        let mut stmts = vec![store(0x80, 32, None, concat(lanes))];
        assert_eq!(merge_any(&mut stmts), 1);
        assert!(
            is_masked_to(expr_of(&stmts[0]), 16),
            "the merged expression reaches its consumer unclipped"
        );
    }

    #[test]
    fn fold_keeps_a_row_a_dynamic_read_can_reach() {
        // The row is also the index of a dynamic read in the same statement,
        // so it still has a reader after the reduction is rewritten.
        let indexed = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x90),
            select: None,
            dynamic_select: Some(crate::ir::expression::ProtoDynamicBitSelect {
                index_expr: Box::new(ProtoExpression::Variable {
                    var_offset: VarOffset::Comb(0x40),
                    select: Some((1, 0)),
                    dynamic_select: None,
                    width: 2,
                    var_full_width: 2,
                    expr_context: ctx(2),
                }),
                elem_width: 1,
                window: 1,
                num_elements: 4,
            }),
            width: 1,
            var_full_width: 4,
            expr_context: ctx(1),
        };
        let mut stmts = transposed_row();
        stmts[2] = store(
            0x80,
            1,
            None,
            binop(reduce(Op::BitOr, 0x40, 2), Op::BitOr, indexed, 1),
        );
        let mut dead = vec![];
        assert_eq!(
            transpose_fold(
                &mut stmts,
                &BigArrayFold::default(),
                &HashSet::default(),
                &mut dead
            ),
            0
        );
        assert_eq!(stmts.len(), 3);
        assert!(dead.is_empty());
    }
}
