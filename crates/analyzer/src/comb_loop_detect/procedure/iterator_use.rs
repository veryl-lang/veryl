//! Whether a counted loop body uses its iterator only in forms that one
//! symbolic iteration follows exactly.
//!
//! A symbolic iteration relates positions through affine indices and selects,
//! and confines branches that compare the iterator with a constant or test its
//! remainder. Any other use, such as `a[i >> 1]`, `case i` or a call argument,
//! selects positions that one iteration cannot tell apart, so the loop takes
//! each iteration's value instead while that stays within the size limit.

use crate::ir::{
    ArrayLiteralItem, AssignDestination, CasePattern, Expression, Factor, ForBound, ForRange,
    ForStatement, Op, Statement, SystemFunctionKind, VarId, VarIndex, VarSelect,
};

/// Whether a statically known loop takes each iteration's constant value
/// instead of one symbolic iteration. A loop that breaks, or whose step is not
/// additive, reaches values a symbolic iterator over `min..=max` cannot
/// represent; a body may also use its iterator other than affinely.
pub(in crate::comb_loop_detect) fn loop_needs_values(statement: &ForStatement) -> bool {
    crate::ir::peel::has_own_break(&statement.body)
        || matches!(statement.range, ForRange::Stepped { .. })
        || iterator_needs_values(&statement.body, statement.var_id)
}

/// Whether `statements` use `iterator` in a form a symbolic iteration loses.
pub(in crate::comb_loop_detect) fn iterator_needs_values(
    statements: &[Statement],
    iterator: VarId,
) -> bool {
    statements
        .iter()
        .any(|statement| statement_needs_values(statement, iterator))
}

fn statement_needs_values(statement: &Statement, iterator: VarId) -> bool {
    match statement {
        Statement::Assign(assign) => {
            assign
                .dst
                .iter()
                .any(|destination| destination_needs_values(destination, iterator))
                || value_needs_values(&assign.expr, iterator)
        }
        Statement::If(statement) => {
            condition_needs_values(&statement.cond, iterator)
                || iterator_needs_values(&statement.true_side, iterator)
                || iterator_needs_values(&statement.false_side, iterator)
        }
        Statement::IfReset(statement) => {
            iterator_needs_values(&statement.true_side, iterator)
                || iterator_needs_values(&statement.false_side, iterator)
        }
        Statement::Case(statement) => {
            let patterns = statement.arms.iter().flat_map(|arm| &arm.patterns);
            mentions(&statement.case_target, iterator)
                || value_needs_values(&statement.case_target, iterator)
                || patterns.into_iter().any(|pattern| match pattern {
                    CasePattern::Eq(value) => mentions(value, iterator),
                    CasePattern::Range { lo, hi, .. } => {
                        mentions(lo, iterator) || mentions(hi, iterator)
                    }
                })
                || statement
                    .arms
                    .iter()
                    .any(|arm| iterator_needs_values(&arm.body, iterator))
                || iterator_needs_values(&statement.default, iterator)
        }
        Statement::For(statement) => {
            range_mentions(&statement.range, iterator)
                || iterator_needs_values(&statement.body, iterator)
        }
        Statement::FunctionCall(call) => {
            call.inputs
                .values()
                .any(|input| argument_needs_values(input, iterator))
                || call
                    .outputs
                    .values()
                    .flatten()
                    .any(|destination| destination_needs_values(destination, iterator))
        }
        Statement::SystemFunctionCall(call) => {
            system_inputs(&call.kind).any(|input| value_needs_values(input, iterator))
        }
        Statement::TbMethodCall(_)
        | Statement::Break
        | Statement::Unsupported(_)
        | Statement::Null => false,
    }
}

fn destination_needs_values(destination: &AssignDestination, iterator: VarId) -> bool {
    coordinates_need_values(&destination.index, &destination.select, iterator)
}

fn coordinates<'e>(
    index: &'e VarIndex,
    select: &'e VarSelect,
) -> impl Iterator<Item = &'e Expression> {
    index
        .expressions()
        .chain(select.0.iter())
        .chain(select.1.as_ref().map(|(_, end)| end))
}

/// An index or select coordinate keeps its positions when it is affine in
/// the iterator.
fn coordinates_need_values(index: &VarIndex, select: &VarSelect, iterator: VarId) -> bool {
    coordinates(index, select).any(|coordinate| {
        if mentions(coordinate, iterator) && !is_affine(coordinate) {
            true
        } else {
            value_needs_values(coordinate, iterator)
        }
    })
}

/// A value carries no position of its own, so the iterator may appear in it
/// freely; only the coordinates and conditions inside it matter.
fn value_needs_values(expression: &Expression, iterator: VarId) -> bool {
    let recurse = |expression: &Expression| value_needs_values(expression, iterator);
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Variable(_, index, select, _) => {
                coordinates_need_values(index, select, iterator)
            }
            Factor::FunctionCall(call) => call
                .inputs
                .values()
                .any(|input| argument_needs_values(input, iterator)),
            Factor::SystemFunctionCall(call) => system_inputs(&call.kind).any(recurse),
            Factor::HierVariable(_)
            | Factor::Value(_)
            | Factor::Anonymous(_)
            | Factor::Unknown(_) => false,
        },
        Expression::Unary(_, operand, _) => recurse(operand),
        Expression::Binary(left, _, right, _) => recurse(left) || recurse(right),
        Expression::Ternary(condition, left, right, _) => {
            condition_needs_values(condition, iterator) || recurse(left) || recurse(right)
        }
        // A repeat count sets how many positions the value has, so the
        // iterator in it is a coordinate, not a value.
        Expression::Concatenation(parts, _) => parts.iter().any(|(part, repeat)| {
            recurse(part)
                || repeat
                    .as_ref()
                    .is_some_and(|repeat| mentions(repeat, iterator))
        }),
        Expression::ArrayLiteral(items, _) => items.iter().any(|item| match item {
            ArrayLiteralItem::Value(value, repeat) => {
                recurse(value)
                    || repeat
                        .as_deref()
                        .is_some_and(|repeat| mentions(repeat, iterator))
            }
            ArrayLiteralItem::Defaul(value) => recurse(value),
        }),
        Expression::StructConstructor(_, fields, _) => {
            fields.iter().any(|(_, value)| recurse(value))
        }
    }
}

/// A branch condition confines each side when it compares the bare iterator
/// with a constant, tests its remainder by a constant, or negates such a
/// condition. Otherwise it follows the iterator only through affine
/// coordinates.
fn condition_needs_values(condition: &Expression, iterator: VarId) -> bool {
    if confines_iterator(condition, iterator) {
        return false;
    }
    uses_as_value(condition, iterator) || value_needs_values(condition, iterator)
}

/// A function sees its actuals as values, so it follows the iterator only
/// through affine coordinates of what it is passed.
fn argument_needs_values(argument: &Expression, iterator: VarId) -> bool {
    uses_as_value(argument, iterator) || value_needs_values(argument, iterator)
}

/// Whether `iterator` appears in `expression` outside every coordinate.
fn uses_as_value(expression: &Expression, iterator: VarId) -> bool {
    let recurse = |expression: &Expression| uses_as_value(expression, iterator);
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Variable(id, ..) => *id == iterator,
            Factor::FunctionCall(call) => call.inputs.values().any(recurse),
            Factor::SystemFunctionCall(call) => system_inputs(&call.kind).any(recurse),
            Factor::HierVariable(_)
            | Factor::Value(_)
            | Factor::Anonymous(_)
            | Factor::Unknown(_) => false,
        },
        Expression::Unary(_, operand, _) => recurse(operand),
        Expression::Binary(left, _, right, _) => recurse(left) || recurse(right),
        Expression::Ternary(condition, left, right, _) => {
            recurse(condition) || recurse(left) || recurse(right)
        }
        Expression::Concatenation(parts, _) => parts
            .iter()
            .any(|(part, repeat)| recurse(part) || repeat.as_ref().is_some_and(recurse)),
        Expression::ArrayLiteral(items, _) => items.iter().any(|item| match item {
            ArrayLiteralItem::Value(value, repeat) => {
                recurse(value) || repeat.as_deref().is_some_and(recurse)
            }
            ArrayLiteralItem::Defaul(value) => recurse(value),
        }),
        Expression::StructConstructor(_, fields, _) => {
            fields.iter().any(|(_, value)| recurse(value))
        }
    }
}

fn confines_iterator(condition: &Expression, iterator: VarId) -> bool {
    match condition {
        Expression::Unary(Op::LogicNot, operand, _) => confines_iterator(operand, iterator),
        Expression::Binary(left, op, right, _) => {
            let compared = |side: &Expression| {
                is_variable(side, iterator)
                    || matches!(op, Op::Eq | Op::Ne) && is_remainder(side, iterator)
            };
            matches!(
                op,
                Op::Eq | Op::Ne | Op::Less | Op::LessEq | Op::Greater | Op::GreaterEq
            ) && (compared(left) && is_constant(right) || compared(right) && is_constant(left))
        }
        _ => false,
    }
}

fn is_remainder(expression: &Expression, iterator: VarId) -> bool {
    matches!(
        expression,
        Expression::Binary(dividend, Op::Rem, divisor, _)
            if is_variable(dividend, iterator) && is_constant(divisor)
    )
}

fn is_variable(expression: &Expression, id: VarId) -> bool {
    matches!(
        expression,
        Expression::Term(factor)
            if matches!(
                factor.as_ref(),
                Factor::Variable(variable, index, select, _)
                    if *variable == id && index.indices.is_empty() && select.is_empty()
            )
    )
}

fn is_constant(expression: &Expression) -> bool {
    expression.comptime().is_const
}

/// The shapes an affine coordinate takes: sums and differences of bare
/// variables and constants, scaled by constants and cast.
fn is_affine(expression: &Expression) -> bool {
    if is_constant(expression) {
        return true;
    }
    match expression {
        Expression::Term(factor) => matches!(
            factor.as_ref(),
            Factor::Variable(_, index, select, _) if index.indices.is_empty() && select.is_empty()
        ),
        Expression::Unary(Op::Add | Op::Sub, operand, _) => is_affine(operand),
        Expression::Binary(left, Op::Add | Op::Sub, right, _) => {
            is_affine(left) && is_affine(right)
        }
        Expression::Binary(left, Op::Mul, right, _) => {
            is_constant(left) && is_affine(right) || is_constant(right) && is_affine(left)
        }
        Expression::Binary(left, Op::As, _, _) => is_affine(left),
        _ => false,
    }
}

fn range_mentions(range: &ForRange, iterator: VarId) -> bool {
    let (start, end) = range.bounds();
    [start, end].into_iter().any(|bound| match bound {
        ForBound::Expression(expression) => mentions(expression, iterator),
        ForBound::Const(..) => false,
    })
}

fn system_inputs(kind: &SystemFunctionKind) -> impl Iterator<Item = &Expression> {
    let inputs: Vec<&Expression> = match kind {
        SystemFunctionKind::Bits(input)
        | SystemFunctionKind::Clog2(input)
        | SystemFunctionKind::Onehot(input)
        | SystemFunctionKind::Signed(input)
        | SystemFunctionKind::Unsigned(input)
        | SystemFunctionKind::Readmemh(input, _) => vec![&input.0],
        SystemFunctionKind::Size(input, dimension) => std::iter::once(&input.0)
            .chain(dimension.as_ref().map(|x| &x.0))
            .collect(),
        SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => {
            inputs.iter().map(|x| &x.0).collect()
        }
        SystemFunctionKind::Assert { cond, args, .. } => std::iter::once(&cond.0)
            .chain(args.iter().map(|x| &x.0))
            .collect(),
        SystemFunctionKind::Finish => Vec::new(),
    };
    inputs.into_iter()
}

/// Whether `iterator` appears anywhere in `expression`.
fn mentions(expression: &Expression, iterator: VarId) -> bool {
    let recurse = |expression: &Expression| mentions(expression, iterator);
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Variable(id, index, select, _) => {
                *id == iterator || coordinates(index, select).any(recurse)
            }
            Factor::FunctionCall(call) => call.inputs.values().any(recurse),
            Factor::SystemFunctionCall(call) => system_inputs(&call.kind).any(recurse),
            Factor::HierVariable(_)
            | Factor::Value(_)
            | Factor::Anonymous(_)
            | Factor::Unknown(_) => false,
        },
        Expression::Unary(_, operand, _) => recurse(operand),
        Expression::Binary(left, _, right, _) => recurse(left) || recurse(right),
        Expression::Ternary(condition, left, right, _) => {
            recurse(condition) || recurse(left) || recurse(right)
        }
        Expression::Concatenation(parts, _) => parts
            .iter()
            .any(|(part, repeat)| recurse(part) || repeat.as_ref().is_some_and(recurse)),
        Expression::ArrayLiteral(items, _) => items.iter().any(|item| match item {
            ArrayLiteralItem::Value(value, repeat) => {
                recurse(value) || repeat.as_deref().is_some_and(recurse)
            }
            ArrayLiteralItem::Defaul(value) => recurse(value),
        }),
        Expression::StructConstructor(_, fields, _) => {
            fields.iter().any(|(_, value)| recurse(value))
        }
    }
}
