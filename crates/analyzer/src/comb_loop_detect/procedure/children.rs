//! The direct subexpressions of an expression, so that each visitor spells
//! out only the cases it treats specially.

use crate::ir::{ArrayLiteralItem, Expression, Factor, SystemFunctionKind, VarIndex, VarSelect};
use smallvec::SmallVec;

/// Each direct subexpression of `expression`: the operands, the coordinates
/// of a variable, the inputs of a call, the inputs of a system function,
/// repeat counts, and the values of literals and constructors.
pub(super) fn children(expression: &Expression) -> SmallVec<[&Expression; 4]> {
    let mut children = SmallVec::new();
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Variable(_, index, select, _) => children.extend(coordinates(index, select)),
            Factor::FunctionCall(call) => children.extend(call.inputs.values()),
            Factor::SystemFunctionCall(call) => children.extend(system_inputs(&call.kind)),
            Factor::HierVariable(_)
            | Factor::Value(_)
            | Factor::Anonymous(_)
            | Factor::Unknown(_) => {}
        },
        Expression::Unary(_, operand, _) => children.push(operand.as_ref()),
        Expression::Binary(left, _, right, _) => children.extend([left.as_ref(), right.as_ref()]),
        Expression::Ternary(condition, left, right, _) => {
            children.extend([condition.as_ref(), left.as_ref(), right.as_ref()]);
        }
        Expression::Concatenation(parts, _) => {
            for (part, repeat) in parts {
                children.push(part);
                children.extend(repeat);
            }
        }
        Expression::ArrayLiteral(items, _) => {
            for item in items {
                match item {
                    ArrayLiteralItem::Value(value, repeat) => {
                        children.push(value.as_ref());
                        children.extend(repeat.as_deref());
                    }
                    ArrayLiteralItem::Defaul(value) => children.push(value.as_ref()),
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            children.extend(fields.iter().map(|(_, value)| value));
        }
    }
    children
}

/// The index and select coordinates of a variable access.
pub(super) fn coordinates<'e>(
    index: &'e VarIndex,
    select: &'e VarSelect,
) -> impl Iterator<Item = &'e Expression> {
    index
        .expressions()
        .chain(select.0.iter())
        .chain(select.1.as_ref().map(|(_, end)| end))
}

/// The inputs of a system function.
pub(super) fn system_inputs(kind: &SystemFunctionKind) -> impl Iterator<Item = &Expression> {
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
