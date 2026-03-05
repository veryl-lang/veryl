use crate::ir::{Op, Value, VarId};
use std::fmt;
use veryl_analyzer::ir as air;
use veryl_analyzer::value::MaskCache;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExpressionContext {
    pub width: usize,
    pub signed: bool,
}

impl From<&air::ExpressionContext> for ExpressionContext {
    fn from(value: &air::ExpressionContext) -> Self {
        Self {
            width: value.width,
            signed: value.signed,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Expression {
    Variable(VarId, *const Value, Option<(usize, usize)>),
    Value(Value),
    Unary(Op, Box<Expression>, ExpressionContext),
    Binary(Box<Expression>, Op, Box<Expression>, ExpressionContext),
}

impl Expression {
    pub fn eval(&self, mask_cache: &mut MaskCache) -> Value {
        match self {
            Expression::Variable(_, x, select) => {
                if let Some((beg, end)) = select {
                    unsafe { (**x).select(*beg, *end) }
                } else {
                    unsafe { (**x).clone() }
                }
            }
            Expression::Value(x) => x.clone(),
            Expression::Unary(op, x, expr_context) => {
                let x = x.eval(mask_cache);
                // TODO context_width
                op.eval_value_unary(&x, expr_context.width, expr_context.signed, mask_cache)
            }
            Expression::Binary(x, op, y, expr_context) => {
                let x = x.eval(mask_cache);
                let y = y.eval(mask_cache);
                // TODO context_width
                op.eval_value_binary(&x, &y, expr_context.width, expr_context.signed, mask_cache)
            }
        }
    }

    pub fn expand(&mut self, width: usize) {
        if let Expression::Value(x) = self {
            *x = x.expand(width, x.signed()).into_owned();
        }
    }

    // TODO remove this allow after adding Expression::FunctionCall
    #[allow(clippy::only_used_in_recursion)]
    pub fn gather_variable(&self, inputs: &mut Vec<VarId>, outputs: &mut Vec<VarId>) {
        match self {
            Expression::Variable(id, _, _) => inputs.push(*id),
            Expression::Value(_) => (),
            Expression::Unary(_, x, _) => {
                x.gather_variable(inputs, outputs);
            }
            Expression::Binary(x, _, y, _) => {
                x.gather_variable(inputs, outputs);
                y.gather_variable(inputs, outputs);
            }
        }
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = match self {
            Expression::Variable(_, _, _) => "var".to_string(),
            Expression::Value(_) => "var".to_string(),
            Expression::Unary(x, y, _) => {
                format!("({x} {y})")
            }
            Expression::Binary(x, y, z, _) => {
                format!("({x} {y} {z})")
            }
        };

        ret.fmt(f)
    }
}
