use crate::ir::{Op, Value, VarId};
use std::fmt;
use veryl_analyzer::value::MaskCache;

#[derive(Clone, Debug)]
pub enum Expression {
    Variable(VarId, *const Value),
    Value(Value),
    Unary(Op, Box<Expression>),
    Binary(Box<Expression>, Op, Box<Expression>),
}

impl Expression {
    pub fn eval(&self, context: Option<usize>, signed: bool, mask_cache: &mut MaskCache) -> Value {
        match self {
            Expression::Variable(_, x) => unsafe { (**x).clone() },
            Expression::Value(x) => x.clone(),
            Expression::Unary(op, x) => {
                let x = x.eval(context, signed, mask_cache);
                // TODO context_width
                op.eval_unary(&x, context, signed, mask_cache)
            }
            Expression::Binary(x, op, y) => {
                let x = x.eval(context, signed, mask_cache);
                let y = y.eval(context, signed, mask_cache);
                // TODO context_width
                op.eval_binary(&x, &y, context, signed, mask_cache)
            }
        }
    }

    pub fn eval_width(&self) -> usize {
        match self {
            Expression::Variable(_, x) => unsafe { (**x).width() },
            Expression::Value(x) => x.width(),
            Expression::Unary(op, x) => {
                let x = x.eval_width();
                op.eval_unary_width_usize(x, None)
            }
            Expression::Binary(x, op, y) => {
                let x = x.eval_width();
                let y = y.eval_width();
                op.eval_binary_width_usize(x, y, None)
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
            Expression::Variable(id, _) => inputs.push(*id),
            Expression::Value(_) => (),
            Expression::Unary(_, x) => {
                x.gather_variable(inputs, outputs);
            }
            Expression::Binary(x, _, y) => {
                x.gather_variable(inputs, outputs);
                y.gather_variable(inputs, outputs);
            }
        }
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = match self {
            Expression::Variable(_, _) => "var".to_string(),
            Expression::Value(_) => "var".to_string(),
            Expression::Unary(x, y) => {
                format!("({x} {y})")
            }
            Expression::Binary(x, y, z) => {
                format!("({x} {y} {z})")
            }
        };

        ret.fmt(f)
    }
}
