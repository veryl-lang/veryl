use crate::variable::{Value, VarId, Variable};
use num_bigint::BigUint;
use std::collections::HashMap;
use std::fmt;

pub enum Expression {
    Term(Factor),
    Unary(Op, Box<Expression>),
    Binary(Box<Expression>, Op, Box<Expression>),
    Concatenation(Vec<Expression>),
}

impl Expression {
    pub fn eval(&self, context_width: Option<usize>, map: &HashMap<VarId, Variable>) -> Value {
        match self {
            Expression::Term(x) => match x {
                Factor::Variable(x) => map.get(x).unwrap().value.clone(),
                Factor::Value(x) => x.clone(),
            },
            Expression::Unary(op, x) => {
                let mut ret = x.eval(context_width, map);
                match op {
                    Op::BitAnd => {
                        ret.payload = reduction(ret.payload, ret.width, |x, y| x & y);
                    }
                    Op::BitOr => {
                        ret.payload = reduction(ret.payload, ret.width, |x, y| x | y);
                    }
                    Op::BitXor => {
                        ret.payload = reduction(ret.payload, ret.width, |x, y| x ^ y);
                    }
                    Op::BitXnor => {
                        ret.payload = reduction(ret.payload, ret.width, |x, y| {
                            ((x ^ y) == 0u32.into()).into()
                        });
                    }
                    Op::BitNand => {
                        ret.payload = reduction(ret.payload, ret.width, |x, y| {
                            ((x & y) == 0u32.into()).into()
                        });
                    }
                    Op::BitNor => {
                        ret.payload = reduction(ret.payload, ret.width, |x, y| {
                            ((x | y) == 0u32.into()).into()
                        });
                    }
                    Op::BitNot => {
                        ret.payload = BigUint::from_slice(
                            &ret.payload
                                .to_u32_digits()
                                .iter()
                                .map(|x| !x)
                                .collect::<Vec<_>>(),
                        );
                    }
                    Op::LogicNot => {
                        ret.payload = (ret.payload == 0u32.into()).into();
                        ret.width = 1;
                        ret.signed = false;
                    }
                    _ => unreachable!(),
                }
                ret
            }
            Expression::Binary(x, op, y) => {
                let x = x.eval(context_width, map);
                let y = y.eval(context_width, map);
                match op {
                    Op::Pow => {
                        let (width, payload) = binary_op(
                            (x.width, x.payload),
                            (y.width, y.payload),
                            context_width,
                            |x, _, z| x.max(z.unwrap_or(0)),
                            |x, y| y.try_into().map(|y| x.pow(y)).unwrap(),
                        );
                        Value {
                            payload,
                            width,
                            signed: false,
                        }
                    }
                    Op::Div => {
                        let (width, payload) = binary_op(
                            (x.width, x.payload),
                            (y.width, y.payload),
                            context_width,
                            |x, y, z| x.max(y).max(z.unwrap_or(0)),
                            |x, y| x / y,
                        );
                        Value {
                            payload,
                            width,
                            signed: false,
                        }
                    }
                    Op::Rem => {
                        let (width, payload) = binary_op(
                            (x.width, x.payload),
                            (y.width, y.payload),
                            context_width,
                            |x, y, z| x.max(y).max(z.unwrap_or(0)),
                            |x, y| x % y,
                        );
                        Value {
                            payload,
                            width,
                            signed: false,
                        }
                    }
                    Op::Mul => todo!(),
                    Op::Add => todo!(),
                    Op::Sub => todo!(),
                    Op::ArithShiftL => todo!(),
                    Op::ArithShiftR => todo!(),
                    Op::LogicShiftL => todo!(),
                    Op::LogicShiftR => todo!(),
                    Op::LessEq => todo!(),
                    Op::GreaterEq => todo!(),
                    Op::Less => todo!(),
                    Op::Greater => todo!(),
                    Op::Eq => todo!(),
                    Op::Ne => todo!(),
                    Op::LogicAnd => todo!(),
                    Op::LogicOr => todo!(),
                    Op::LogicNot => todo!(),
                    Op::BitAnd => todo!(),
                    Op::BitOr => todo!(),
                    Op::BitXor => todo!(),
                    Op::BitXnor => todo!(),
                    Op::BitNand => todo!(),
                    Op::BitNor => todo!(),
                    Op::BitNot => todo!(),
                }
            }
            _ => todo!(),
        }
    }
}

fn reduction<T: Fn(BigUint, BigUint) -> BigUint>(value: BigUint, width: usize, func: T) -> BigUint {
    let mut tmp = value;
    let mut ret = tmp.clone() & BigUint::from(1u32);
    for _ in 0..width {
        tmp >>= 1;
        ret = func(ret, tmp.clone() & BigUint::from(1u32));
    }
    ret
}

fn binary_op<T: Fn(usize, usize, Option<usize>) -> usize, U: Fn(BigUint, BigUint) -> BigUint>(
    x: (usize, BigUint),
    y: (usize, BigUint),
    context_width: Option<usize>,
    calc_width: T,
    calc_value: U,
) -> (usize, BigUint) {
    let width = calc_width(x.0, y.0, context_width);
    let value = calc_value(x.1, y.1);
    (width, value)
}

pub enum Factor {
    Variable(VarId),
    Value(Value),
}

pub enum Op {
    /// **
    Pow,
    /// /
    Div,
    /// %
    Rem,
    /// *
    Mul,
    /// +
    Add,
    /// -
    Sub,
    /// <<<
    ArithShiftL,
    /// >>>
    ArithShiftR,
    /// <<
    LogicShiftL,
    /// >>
    LogicShiftR,
    /// <=
    LessEq,
    /// >=
    GreaterEq,
    /// <:
    Less,
    /// >:
    Greater,
    /// ==
    Eq,
    /// !=
    Ne,
    /// &&
    LogicAnd,
    /// ||
    LogicOr,
    /// !
    LogicNot,
    /// &
    BitAnd,
    /// |
    BitOr,
    /// ^
    BitXor,
    /// ~^ ^~
    BitXnor,
    /// ~&
    BitNand,
    /// ~|
    BitNor,
    /// ~
    BitNot,
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let str = match self {
            Op::Pow => "**",
            Op::Div => "/",
            Op::Rem => "%",
            Op::Mul => "*",
            Op::Add => "+",
            Op::Sub => "-",
            Op::ArithShiftL => "<<<",
            Op::ArithShiftR => ">>>",
            Op::LogicShiftL => "<<",
            Op::LogicShiftR => ">>",
            Op::LessEq => "<=",
            Op::GreaterEq => ">=",
            Op::Less => "<:",
            Op::Greater => ">:",
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::LogicAnd => "&&",
            Op::LogicOr => "||",
            Op::LogicNot => "!",
            Op::BitAnd => "&",
            Op::BitOr => "|",
            Op::BitXor => "^",
            Op::BitXnor => "^~",
            Op::BitNand => "~&",
            Op::BitNor => "~|",
            Op::BitNot => "~",
        };

        str.fmt(f)
    }
}
