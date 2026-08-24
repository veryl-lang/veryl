//! Recognizing a chain of equality tests against one selector: `case` and the
//! `if / else if` cascade it lowers to describe the same shape, and it is that
//! shape which lets a backend swap the comparisons for an index — so the
//! recognizer lives here rather than in either backend.

use crate::ir::{ProtoCaseStatement, ProtoExpression, ProtoIfStatement, ProtoStatement};

pub struct EqChainArm<'a> {
    pub value: u64,
    pub body: &'a [ProtoStatement],
}

pub struct EqChain<'a> {
    pub selector: &'a ProtoExpression,
    pub arms: Vec<EqChainArm<'a>>,
    pub default: &'a [ProtoStatement],
}

/// The `sel == const` halves of an equality test, either way round.
pub fn extract_eq_const(cond: &ProtoExpression) -> Option<(&ProtoExpression, u64)> {
    let (x, op, y) = match cond {
        ProtoExpression::Binary { x, op, y, .. } => (x.as_ref(), *op, y.as_ref()),
        _ => return None,
    };
    if !matches!(
        op,
        veryl_analyzer::ir::Op::Eq | veryl_analyzer::ir::Op::EqWildcard
    ) {
        return None;
    }
    fn try_extract<'b>(
        val_side: &'b ProtoExpression,
        var_side: &'b ProtoExpression,
    ) -> Option<(&'b ProtoExpression, u64)> {
        match val_side {
            ProtoExpression::Value { value, .. } => {
                // xz constants would match any value under wildcard
                // semantics and cannot be encoded as a table index.
                if value.is_xz() {
                    None
                } else {
                    value.to_u64().map(|v| (var_side, v))
                }
            }
            _ => None,
        }
    }
    if let Some(r) = try_extract(y, x) {
        return Some(r);
    }
    try_extract(x, y)
}

/// Whether two reads name the same bits of the same variable, so one hoisted
/// value can drive every comparison in a chain.
pub fn same_var_read(a: &ProtoExpression, b: &ProtoExpression) -> bool {
    match (a, b) {
        (
            ProtoExpression::Variable {
                var_offset: oa,
                select: sa,
                dynamic_select: dsa,
                ..
            },
            ProtoExpression::Variable {
                var_offset: ob,
                select: sb,
                dynamic_select: dsb,
                ..
            },
        ) => oa == ob && sa == sb && dsa.is_none() && dsb.is_none(),
        _ => false,
    }
}

/// Walk an `if / else if` cascade for as long as it keeps testing one selector
/// against constants.  `min_arms` is the shortest chain worth reporting.
pub fn collect_eq_chain(start: &ProtoIfStatement, min_arms: usize) -> Option<EqChain<'_>> {
    let mut arms: Vec<EqChainArm<'_>> = Vec::new();
    let mut selector: Option<&ProtoExpression> = None;
    let mut current = start;
    // Else-chain once the eq-const prefix stops: the *last consumed* arm's
    // false_side. A dirty break descends into a non-eq-const / different-selector
    // node that must stay whole as the default, not be reduced to its false_side.
    let mut default: &[ProtoStatement] = &[];
    loop {
        let cond = current.cond.as_ref()?;
        let (var_expr, const_val) = match extract_eq_const(cond) {
            Some(p) => p,
            None => break,
        };
        // Dynamic-indexed reads cannot share a single hoisted dispatch
        // value across arms, so they break the chain.
        if let ProtoExpression::Variable {
            dynamic_select: Some(_),
            ..
        } = var_expr
        {
            break;
        }
        if let Some(sel) = selector {
            if !same_var_read(sel, var_expr) {
                break;
            }
        } else {
            selector = Some(var_expr);
        }
        arms.push(EqChainArm {
            value: const_val,
            body: &current.true_side,
        });
        default = &current.false_side[..];
        if current.false_side.len() == 1
            && let ProtoStatement::If(next) = &current.false_side[0]
        {
            current = next;
            continue;
        }
        break;
    }
    if arms.len() < min_arms {
        return None;
    }
    Some(EqChain {
        selector: selector?,
        arms,
        default,
    })
}

/// Extract the constant selector values from a `case` arm condition — succeeds
/// only for `sel == const` leaves OR-combined against ONE non-dynamic selector,
/// so a range/casez/mixed-selector leaf rejects it (→ comparison cascade).
fn extract_case_eq_values<'a>(
    cond: &'a ProtoExpression,
    selector: &mut Option<&'a ProtoExpression>,
    out: &mut Vec<u64>,
) -> bool {
    if let ProtoExpression::Binary {
        x,
        op: veryl_analyzer::ir::Op::LogicOr,
        y,
        ..
    } = cond
    {
        return extract_case_eq_values(x, selector, out)
            && extract_case_eq_values(y, selector, out);
    }
    let Some((var_expr, const_val)) = extract_eq_const(cond) else {
        return false;
    };
    if let ProtoExpression::Variable {
        dynamic_select: Some(_),
        ..
    } = var_expr
    {
        return false;
    }
    match selector {
        Some(sel) if !same_var_read(sel, var_expr) => return false,
        Some(_) => {}
        None => *selector = Some(var_expr),
    }
    out.push(const_val);
    true
}

/// Reshape the flat arms of a `case` into an `EqChain` (one entry per matched
/// value) so the chain consumers can be shared; `None` if not all eq-const.
pub fn case_as_eq_chain(case: &ProtoCaseStatement) -> Option<EqChain<'_>> {
    let mut selector: Option<&ProtoExpression> = None;
    let mut arms: Vec<EqChainArm<'_>> = Vec::new();
    for arm in &case.arms {
        let mut values = Vec::new();
        if !extract_case_eq_values(&arm.cond, &mut selector, &mut values) {
            return None;
        }
        for value in values {
            arms.push(EqChainArm {
                value,
                body: &arm.body,
            });
        }
    }
    Some(EqChain {
        selector: selector?,
        arms,
        default: &case.default,
    })
}

#[cfg(test)]
mod eq_chain_tests {
    use super::*;
    use crate::ir::{ExpressionContext, ProtoExpression, Value, VarOffset};
    use veryl_analyzer::ir::Op;
    use veryl_analyzer::value::ValueU64;

    fn ctx(width: usize) -> ExpressionContext {
        ExpressionContext {
            width,
            signed: false,
        }
    }
    fn var_expr(off: isize, width: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: None,
            dynamic_select: None,
            width,
            var_full_width: width,
            expr_context: ctx(width),
        }
    }
    fn const_expr(payload: u64) -> ProtoExpression {
        ProtoExpression::Value {
            value: Value::U64(ValueU64 {
                payload,
                mask_xz: 0,
                width: 8,
                signed: false,
            }),
            width: 8,
            expr_context: ctx(8),
        }
    }
    fn cmp(off: isize, op: Op, val: u64) -> ProtoExpression {
        ProtoExpression::Binary {
            x: Box::new(var_expr(off, 8)),
            op,
            y: Box::new(const_expr(val)),
            width: 1,
            expr_context: ctx(1),
        }
    }
    fn if_node(
        cond: ProtoExpression,
        t: Vec<ProtoStatement>,
        f: Vec<ProtoStatement>,
    ) -> ProtoIfStatement {
        ProtoIfStatement {
            cond: Some(cond),
            true_side: t,
            false_side: f,
        }
    }
    fn eq_prefix(sel: isize, tail: Vec<ProtoStatement>) -> ProtoIfStatement {
        let mut chain = tail;
        for v in (0..4u64).rev() {
            chain = vec![ProtoStatement::If(if_node(
                cmp(sel, Op::Eq, v),
                vec![ProtoStatement::Break],
                chain,
            ))];
        }
        match chain.into_iter().next().unwrap() {
            ProtoStatement::If(x) => x,
            _ => unreachable!(),
        }
    }

    // A node that ends the eq-const prefix because it tests a *different*
    // selector must survive whole — its cond and true_side — inside the
    // chain default.
    #[test]
    fn dirty_break_keeps_node_in_default_selector_mismatch() {
        let dirty = ProtoStatement::If(if_node(
            cmp(0x20, Op::Eq, 5),
            vec![ProtoStatement::Break],
            vec![ProtoStatement::Break],
        ));
        let head = eq_prefix(0x10, vec![dirty]);
        let chain = collect_eq_chain(&head, 2).expect("4-arm eq chain");
        assert_eq!(
            chain.arms.iter().map(|a| a.value).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(chain.default.len(), 1);
        assert!(
            matches!(chain.default[0], ProtoStatement::If(_)),
            "default must retain the whole different-selector node"
        );
    }

    // Same, but the prefix ends on a non-Eq operator (extract_eq_const fails).
    #[test]
    fn dirty_break_keeps_node_in_default_non_eq_op() {
        let dirty = ProtoStatement::If(if_node(
            cmp(0x10, Op::Less, 4),
            vec![ProtoStatement::Break],
            vec![ProtoStatement::Break],
        ));
        let head = eq_prefix(0x10, vec![dirty]);
        let chain = collect_eq_chain(&head, 2).expect("4-arm eq chain");
        assert_eq!(chain.arms.len(), 4);
        assert_eq!(chain.default.len(), 1);
        assert!(matches!(chain.default[0], ProtoStatement::If(_)));
    }

    // A clean chain whose final else is plain statements keeps that else as
    // the default (regression guard for the common case).
    #[test]
    fn clean_chain_default_is_final_else() {
        let head = eq_prefix(0x10, vec![ProtoStatement::Break]);
        let chain = collect_eq_chain(&head, 2).expect("4-arm eq chain");
        assert_eq!(chain.arms.len(), 4);
        assert_eq!(chain.default.len(), 1);
        assert!(matches!(chain.default[0], ProtoStatement::Break));
    }
}
