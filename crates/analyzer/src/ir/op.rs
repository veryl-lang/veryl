use crate::BigUint;
use crate::value::{MaskCache, Value, ValueBigUint, ValueU64};
use num_traits::{One, Zero};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// ==?
    EqWildcard,
    /// !=
    Ne,
    /// !=?
    NeWildcard,
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
    /// ~^
    BitXnor,
    /// ~&
    BitNand,
    /// ~|
    BitNor,
    /// ~
    BitNot,
    /// as
    As,
    /// ternary
    Ternary,
    /// concatenation
    Concatenation,
    /// array literal
    ArrayLiteral,
    /// condition
    Condition,
    /// repeat
    Repeat,
}

fn b0() -> BigUint {
    BigUint::zero()
}

fn b1() -> BigUint {
    BigUint::one()
}

impl Op {
    pub fn eval(&self, x: usize, y: usize) -> usize {
        match self {
            Op::Add => x + y,
            Op::Sub => x - y,
            Op::Mul => x * y,
            Op::Div => x / y,
            Op::Rem => x % y,
            Op::BitAnd => x & y,
            Op::BitOr => x | y,
            Op::BitXor => x ^ y,
            Op::ArithShiftL => x << y,
            Op::ArithShiftR => x >> y,
            Op::LogicShiftL => x << y,
            Op::LogicShiftR => x >> y,
            _ => unimplemented!(),
        }
    }

    pub fn unary_signed(&self, x: bool) -> bool {
        matches!(self, Op::Add | Op::Sub | Op::BitNot) & x
    }

    pub fn binary_signed(&self, x: bool, y: bool) -> bool {
        match self {
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Rem
            | Op::Greater
            | Op::GreaterEq
            | Op::Less
            | Op::LessEq => x && y,
            Op::LogicShiftL | Op::LogicShiftR | Op::ArithShiftL | Op::ArithShiftR | Op::Pow => x,
            _ => false,
        }
    }

    pub fn unary_context_width(&self, context: Option<usize>) -> Option<usize> {
        match self {
            Op::BitAnd
            | Op::BitNand
            | Op::BitOr
            | Op::BitNor
            | Op::BitXor
            | Op::BitXnor
            | Op::LogicNot => None,
            _ => context,
        }
    }

    pub fn binary_x_context_width(&self, context: Option<usize>) -> Option<usize> {
        match self {
            Op::LogicAnd | Op::LogicOr => None,
            _ => context,
        }
    }

    pub fn binary_y_context_width(&self, context: Option<usize>) -> Option<usize> {
        match self {
            Op::LogicAnd
            | Op::LogicOr
            | Op::LogicShiftL
            | Op::LogicShiftR
            | Op::ArithShiftL
            | Op::ArithShiftR
            | Op::Pow => None,
            _ => context,
        }
    }

    pub fn eval_binary_width_usize(&self, x: usize, y: usize, context: Option<usize>) -> usize {
        match self {
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Rem
            | Op::BitAnd
            | Op::BitOr
            | Op::BitXor
            | Op::BitXnor => x.max(y).max(context.unwrap_or(0)),
            Op::Eq
            | Op::EqWildcard
            | Op::Ne
            | Op::NeWildcard
            | Op::Less
            | Op::LessEq
            | Op::Greater
            | Op::GreaterEq
            | Op::LogicAnd
            | Op::LogicOr => 1.max(context.unwrap_or(0)),
            Op::LogicShiftL | Op::LogicShiftR | Op::ArithShiftL | Op::ArithShiftR | Op::Pow => {
                x.max(context.unwrap_or(0))
            }
            _ => unimplemented!(),
        }
    }

    pub fn eval_binary_width(&self, x: &Value, y: &Value, context: Option<usize>) -> usize {
        self.eval_binary_width_usize(x.width(), y.width(), context)
    }

    pub fn eval_unary_width_usize(&self, x: usize, context: Option<usize>) -> usize {
        match self {
            Op::Add | Op::Sub | Op::BitNot => x.max(context.unwrap_or(0)),
            Op::BitAnd
            | Op::BitNand
            | Op::BitOr
            | Op::BitNor
            | Op::BitXor
            | Op::BitXnor
            | Op::LogicNot => 1.max(context.unwrap_or(0)),
            _ => unimplemented!(),
        }
    }

    pub fn eval_unary_width(&self, x: &Value, context: Option<usize>) -> usize {
        self.eval_unary_width_usize(x.width(), context)
    }

    pub fn eval_unary(
        &self,
        x: &Value,
        context: Option<usize>,
        signed: bool,
        mask_cache: &mut MaskCache,
    ) -> Value {
        let width = self.eval_unary_width(x, context);

        match self {
            Op::Add => {
                let x = x.expand(width, signed);
                x.into_owned()
            }
            Op::Sub => {
                let x = x.expand(width, signed);
                match x.as_ref() {
                    Value::U64(x) => {
                        let mut ret = x.clone();
                        if ret.is_xz() {
                            Value::U64(ValueU64::new_x(width, x.signed))
                        } else {
                            let mask = ValueU64::gen_mask(width);
                            ret.payload ^= mask;
                            ret.payload += 1;
                            ret.payload &= mask;
                            Value::U64(ret)
                        }
                    }
                    Value::BigUint(x) => {
                        let mut ret = x.clone();
                        if ret.is_xz() {
                            Value::BigUint(ValueBigUint::new_x(width, x.signed))
                        } else {
                            let mask = mask_cache.get(width);
                            *ret.payload ^= mask;
                            *ret.payload += b1();
                            *ret.payload &= mask;
                            Value::BigUint(ret)
                        }
                    }
                }
            }
            Op::BitNot => {
                let x = x.expand(width, signed);
                match x.as_ref() {
                    Value::U64(x) => {
                        let mut ret = x.clone();
                        let mask = ValueU64::gen_mask(width);
                        ret.payload ^= mask;
                        ret.payload &= ret.mask_xz ^ mask;
                        Value::U64(ret)
                    }
                    Value::BigUint(x) => {
                        let mut ret = x.clone();
                        let mask = mask_cache.get(width);
                        *ret.payload ^= mask;
                        *ret.payload &= ret.mask_xz() ^ mask;
                        Value::BigUint(ret)
                    }
                }
            }
            Op::BitAnd => {
                let (is_zero, is_x) = match x {
                    Value::U64(x) => {
                        let mask = ValueU64::gen_mask(x.width as usize);
                        let is_zero = x.payload | x.mask_xz != mask;
                        let is_x = x.mask_xz != 0;
                        (is_zero, is_x)
                    }
                    Value::BigUint(x) => {
                        let mask = mask_cache.get(x.width as usize);
                        let is_zero = x.payload() | x.mask_xz() != *mask;
                        let is_x = x.mask_xz() != &b0();
                        (is_zero, is_x)
                    }
                };

                let ret = ValueU64::new_bit_0x(is_zero, is_x);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::BitNand => {
                let (is_one, is_x) = match x {
                    Value::U64(x) => {
                        let mask = ValueU64::gen_mask(x.width as usize);
                        let is_one = x.payload | x.mask_xz != mask;
                        let is_x = x.mask_xz != 0;
                        (is_one, is_x)
                    }
                    Value::BigUint(x) => {
                        let mask = mask_cache.get(x.width as usize);
                        let is_one = x.payload() | x.mask_xz() != *mask;
                        let is_x = x.mask_xz() != &b0();
                        (is_one, is_x)
                    }
                };

                let ret = ValueU64::new_bit_1x(is_one, is_x);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::BitOr => {
                let (is_one, is_x) = match x {
                    Value::U64(x) => {
                        let mask = ValueU64::gen_mask(x.width as usize);
                        let is_one = x.payload & (x.mask_xz ^ mask) != 0;
                        let is_x = x.mask_xz != 0;
                        (is_one, is_x)
                    }
                    Value::BigUint(x) => {
                        let mask = mask_cache.get(x.width as usize);
                        let is_one = x.payload() & (x.mask_xz() ^ mask) != b0();
                        let is_x = x.mask_xz() != &b0();
                        (is_one, is_x)
                    }
                };

                let ret = ValueU64::new_bit_1x(is_one, is_x);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::BitNor | Op::LogicNot => {
                let (is_zero, is_x) = match x {
                    Value::U64(x) => {
                        let mask = ValueU64::gen_mask(x.width as usize);
                        let is_zero = x.payload & (x.mask_xz ^ mask) != 0;
                        let is_x = x.mask_xz != 0;
                        (is_zero, is_x)
                    }
                    Value::BigUint(x) => {
                        let mask = mask_cache.get(x.width as usize);
                        let is_zero = x.payload() & (x.mask_xz() ^ mask) != b0();
                        let is_x = x.mask_xz() != &b0();
                        (is_zero, is_x)
                    }
                };

                let ret = ValueU64::new_bit_0x(is_zero, is_x);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::BitXor => {
                let ret = if x.is_xz() {
                    ValueU64::new_x(1, false)
                } else {
                    let ret = match x {
                        Value::U64(x) => x.payload.count_ones() % 2 == 1,
                        Value::BigUint(x) => x.payload.count_ones() % 2 == 1,
                    };
                    ValueU64::new(ret.into(), 1, false)
                };
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::BitXnor => {
                let ret = if x.is_xz() {
                    ValueU64::new_x(1, false)
                } else {
                    let ret = match x {
                        Value::U64(x) => x.payload.count_ones() % 2 == 0,
                        Value::BigUint(x) => x.payload.count_ones() % 2 == 0,
                    };
                    ValueU64::new(ret.into(), 1, false)
                };
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            _ => unimplemented!(),
        }
    }

    pub fn eval_binary(
        &self,
        x: &Value,
        y: &Value,
        context: Option<usize>,
        signed: bool,
        mask_cache: &mut MaskCache,
    ) -> Value {
        let width = self.eval_binary_width(x, y, context);
        match self {
            Op::Add => {
                let x = x.expand(width, signed);
                let y = y.expand(width, signed);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        if x.is_xz() | y.is_xz() {
                            Value::U64(ValueU64::new_x(width, signed))
                        } else {
                            let mut payload = x.payload + y.payload;
                            payload &= ValueU64::gen_mask(width);
                            Value::U64(ValueU64::new(payload, width, signed))
                        }
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        if x.is_xz() | y.is_xz() {
                            Value::BigUint(ValueBigUint::new_x(width, signed))
                        } else {
                            let mut payload = x.payload() + y.payload();
                            payload &= mask_cache.get(width);
                            Value::BigUint(ValueBigUint::new_biguint(payload, width, signed))
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Op::Sub => {
                let x = x.expand(width, signed);
                let y = y.expand(width, signed);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        if x.is_xz() | y.is_xz() {
                            Value::U64(ValueU64::new_x(width, signed))
                        } else {
                            let mut payload = x.payload.wrapping_sub(y.payload);
                            payload &= ValueU64::gen_mask(width);
                            Value::U64(ValueU64::new(payload, width, signed))
                        }
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        if x.is_xz() | y.is_xz() {
                            Value::BigUint(ValueBigUint::new_x(width, signed))
                        } else {
                            let mask = mask_cache.get(width);
                            // create -y
                            let y = ((y.payload() ^ mask) + b1()) & mask;
                            let mut payload = x.payload() + y;
                            payload &= mask;
                            Value::BigUint(ValueBigUint::new_biguint(payload, width, signed))
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Op::Mul => {
                let x = x.expand(width, signed);
                let y = y.expand(width, signed);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        if x.is_xz() | y.is_xz() {
                            Value::U64(ValueU64::new_x(width, signed))
                        } else {
                            let mut payload = x.payload.wrapping_mul(y.payload);
                            payload &= ValueU64::gen_mask(width);
                            Value::U64(ValueU64::new(payload, width, signed))
                        }
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        if x.is_xz() | y.is_xz() {
                            Value::BigUint(ValueBigUint::new_x(width, signed))
                        } else {
                            let mut payload = x.payload() * y.payload();
                            payload &= mask_cache.get(width);
                            Value::BigUint(ValueBigUint::new_biguint(payload, width, signed))
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Op::Div => {
                let x = x.expand(width, signed);
                let y = y.expand(width, signed);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        if x.is_xz() || y.is_xz() || y.payload == 0 {
                            Value::U64(ValueU64::new_x(width, signed))
                        } else {
                            let mut payload = if signed {
                                let x = x.to_i64().unwrap();
                                let y = y.to_i64().unwrap();
                                (x / y) as u64
                            } else {
                                x.payload / y.payload
                            };

                            payload &= ValueU64::gen_mask(width);
                            Value::U64(ValueU64::new(payload, width, signed))
                        }
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        if x.is_xz() || y.is_xz() || y.payload() == &b0() {
                            Value::BigUint(ValueBigUint::new_x(width, signed))
                        } else if signed {
                            let x = x.to_bigint().unwrap();
                            let y = y.to_bigint().unwrap();
                            let payload = x / y;
                            Value::BigUint(ValueBigUint::new_bigint(payload, width, signed))
                        } else {
                            let mut payload = x.payload() / y.payload();
                            payload &= mask_cache.get(width);
                            Value::BigUint(ValueBigUint::new_biguint(payload, width, signed))
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Op::Rem => {
                let x = x.expand(width, signed);
                let y = y.expand(width, signed);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        if x.is_xz() || y.is_xz() || y.payload == 0 {
                            Value::U64(ValueU64::new_x(width, signed))
                        } else {
                            let mut payload = if signed {
                                let x = x.to_i64().unwrap();
                                let y = y.to_i64().unwrap();
                                (x % y) as u64
                            } else {
                                x.payload % y.payload
                            };

                            payload &= ValueU64::gen_mask(width);
                            Value::U64(ValueU64::new(payload, width, signed))
                        }
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        if x.is_xz() || y.is_xz() || y.payload() == &b0() {
                            Value::BigUint(ValueBigUint::new_x(width, signed))
                        } else if signed {
                            let x = x.to_bigint().unwrap();
                            let y = y.to_bigint().unwrap();
                            let payload = x % y;
                            Value::BigUint(ValueBigUint::new_bigint(payload, width, signed))
                        } else {
                            let mut payload = x.payload() % y.payload();
                            payload &= mask_cache.get(width);
                            Value::BigUint(ValueBigUint::new_biguint(payload, width, signed))
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Op::BitAnd => {
                let x = x.expand(width, false);
                let y = y.expand(width, false);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let payload = x.payload & y.payload;
                        let mut ret = ValueU64::new(payload, width, false);
                        ret.mask_xz = (x.mask_xz & y.mask_xz)
                            | (x.mask_xz & !y.mask_xz & y.payload)
                            | (y.mask_xz & !x.mask_xz & x.payload);
                        ret.payload &= !ret.mask_xz;
                        Value::U64(ret)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let payload = x.payload() & y.payload();
                        let mut ret = ValueBigUint::new_biguint(payload, width, false);
                        let mask = mask_cache.get(width);
                        *ret.mask_xz = (x.mask_xz() & y.mask_xz())
                            | (x.mask_xz() & (y.mask_xz() ^ mask) & y.payload())
                            | (y.mask_xz() & (x.mask_xz() ^ mask) & x.payload());
                        *ret.payload &= ret.mask_xz() ^ mask;
                        Value::BigUint(ret)
                    }
                    _ => unreachable!(),
                }
            }
            Op::BitOr => {
                let x = x.expand(width, false);
                let y = y.expand(width, false);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let payload = x.payload | y.payload;
                        let mut ret = ValueU64::new(payload, width, false);
                        ret.mask_xz = (x.mask_xz & y.mask_xz)
                            | (x.mask_xz & !y.mask_xz & !y.payload)
                            | (y.mask_xz & !x.mask_xz & !x.payload);
                        ret.payload &= !ret.mask_xz;
                        Value::U64(ret)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let payload = x.payload() | y.payload();
                        let mut ret = ValueBigUint::new_biguint(payload, width, false);
                        let mask = mask_cache.get(width);
                        *ret.mask_xz = (x.mask_xz() & y.mask_xz())
                            | (x.mask_xz() & (y.mask_xz() ^ mask) & (y.payload() ^ mask))
                            | (y.mask_xz() & (x.mask_xz() ^ mask) & (x.payload() ^ mask));
                        *ret.payload &= ret.mask_xz() ^ mask;
                        Value::BigUint(ret)
                    }
                    _ => unreachable!(),
                }
            }
            Op::BitXor => {
                let x = x.expand(width, false);
                let y = y.expand(width, false);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let payload = x.payload ^ y.payload;
                        let mut ret = ValueU64::new(payload, width, false);
                        ret.mask_xz = x.mask_xz | y.mask_xz;
                        ret.payload &= !ret.mask_xz;
                        Value::U64(ret)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let payload = x.payload() ^ y.payload();
                        let mut ret = ValueBigUint::new_biguint(payload, width, false);
                        let mask = mask_cache.get(width);
                        *ret.mask_xz = x.mask_xz() | y.mask_xz();
                        *ret.payload &= ret.mask_xz() ^ mask;
                        Value::BigUint(ret)
                    }
                    _ => unreachable!(),
                }
            }
            Op::BitXnor => {
                let x = x.expand(width, false);
                let y = y.expand(width, false);

                match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let mask = ValueU64::gen_mask(width);
                        let payload = x.payload ^ y.payload ^ mask;
                        let mut ret = ValueU64::new(payload, width, false);
                        ret.mask_xz = x.mask_xz | y.mask_xz;
                        ret.payload &= !ret.mask_xz;
                        Value::U64(ret)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let mask = mask_cache.get(width);
                        let payload = x.payload() ^ y.payload() ^ mask;
                        let mut ret = ValueBigUint::new_biguint(payload, width, false);
                        *ret.mask_xz = x.mask_xz() | y.mask_xz();
                        *ret.payload &= ret.mask_xz() ^ mask;
                        Value::BigUint(ret)
                    }
                    _ => unreachable!(),
                }
            }
            Op::Eq => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, false);
                let y = y.expand(xy_width, false);

                let (is_zero, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let is_zero = (x.payload & !x.mask_xz) != (y.payload & !y.mask_xz);
                        let is_x = x.mask_xz != 0 || y.mask_xz != 0;

                        (is_zero, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let x_mask = mask_cache.get(x.width as usize).clone();
                        let y_mask = mask_cache.get(y.width as usize);

                        let is_zero = (x.payload() & (x.mask_xz() ^ x_mask))
                            != (y.payload() & (y.mask_xz() ^ y_mask));
                        let is_x = x.mask_xz() != &b0() || y.mask_xz() != &b0();

                        (is_zero, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_0x(is_zero, is_x);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::Ne => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, false);
                let y = y.expand(xy_width, false);

                let (is_one, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let is_one = (x.payload & !x.mask_xz) != (y.payload & !y.mask_xz);
                        let is_x = x.mask_xz != 0 || y.mask_xz != 0;

                        (is_one, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let x_mask = mask_cache.get(x.width as usize).clone();
                        let y_mask = mask_cache.get(y.width as usize);

                        let is_one = (x.payload() & (x.mask_xz() ^ x_mask))
                            != (y.payload() & (y.mask_xz() ^ y_mask));
                        let is_x = x.mask_xz() != &b0() || y.mask_xz() != &b0();

                        (is_one, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_1x(is_one, is_x);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::EqWildcard => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, false);
                let y = y.expand(xy_width, false);

                let (is_zero, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let wildcard = !y.mask_xz;
                        let is_zero = x.payload & wildcard != y.payload & wildcard;
                        let is_x = x.mask_xz & wildcard != 0;
                        (is_zero, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let wildcard = mask_cache.get(y.width as usize);
                        let wildcard = y.mask_xz() ^ wildcard;

                        let is_zero = x.payload() & &wildcard != y.payload() & &wildcard;
                        let is_x = x.mask_xz() & &wildcard != b0();
                        (is_zero, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_x0(is_x, is_zero);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::NeWildcard => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, false);
                let y = y.expand(xy_width, false);

                let (is_one, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let wildcard = !y.mask_xz;
                        let is_one = x.payload & wildcard != y.payload & wildcard;
                        let is_x = x.mask_xz & wildcard != 0;
                        (is_one, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let wildcard = mask_cache.get(y.width as usize);
                        let wildcard = y.mask_xz() ^ wildcard;

                        let is_one = x.payload() & &wildcard != y.payload() & &wildcard;
                        let is_x = x.mask_xz() & &wildcard != b0();
                        (is_one, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_x1(is_x, is_one);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::Greater => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, signed);
                let y = y.expand(xy_width, signed);

                let (is_one, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let is_one = if signed {
                            x.to_i64() > y.to_i64()
                        } else {
                            x.payload > y.payload
                        };
                        let is_x = x.mask_xz != 0 || y.mask_xz != 0;

                        (is_one, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let is_one = if signed {
                            x.to_bigint() > y.to_bigint()
                        } else {
                            x.payload() > y.payload()
                        };
                        let is_x = x.mask_xz() != &b0() || y.mask_xz() != &b0();

                        (is_one, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_x1(is_x, is_one);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::GreaterEq => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, signed);
                let y = y.expand(xy_width, signed);

                let (is_one, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let is_one = if signed {
                            x.to_i64() >= y.to_i64()
                        } else {
                            x.payload >= y.payload
                        };
                        let is_x = x.mask_xz != 0 || y.mask_xz != 0;

                        (is_one, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let is_one = if signed {
                            x.to_bigint() >= y.to_bigint()
                        } else {
                            x.payload() >= y.payload()
                        };
                        let is_x = x.mask_xz() != &b0() || y.mask_xz() != &b0();

                        (is_one, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_x1(is_x, is_one);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::Less => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, signed);
                let y = y.expand(xy_width, signed);

                let (is_one, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let is_one = if signed {
                            x.to_i64() < y.to_i64()
                        } else {
                            x.payload < y.payload
                        };
                        let is_x = x.mask_xz != 0 || y.mask_xz != 0;

                        (is_one, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let is_one = if signed {
                            x.to_bigint() < y.to_bigint()
                        } else {
                            x.payload() < y.payload()
                        };
                        let is_x = x.mask_xz() != &b0() || y.mask_xz() != &b0();

                        (is_one, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_x1(is_x, is_one);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::LessEq => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, signed);
                let y = y.expand(xy_width, signed);

                let (is_one, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let is_one = if signed {
                            x.to_i64() <= y.to_i64()
                        } else {
                            x.payload <= y.payload
                        };
                        let is_x = x.mask_xz != 0 || y.mask_xz != 0;

                        (is_one, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let is_one = if signed {
                            x.to_bigint() <= y.to_bigint()
                        } else {
                            x.payload() <= y.payload()
                        };
                        let is_x = x.mask_xz() != &b0() || y.mask_xz() != &b0();

                        (is_one, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_x1(is_x, is_one);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::LogicAnd => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, false);
                let y = y.expand(xy_width, false);

                let (is_one, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let is_one = (x.payload & !x.mask_xz != 0) && (y.payload & !y.mask_xz != 0);
                        let is_x = x.mask_xz != 0 || y.mask_xz != 0;

                        (is_one, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let x_mask = mask_cache.get(x.width as usize).clone();
                        let y_mask = mask_cache.get(y.width as usize);
                        let is_one = (x.payload() & (x.mask_xz() ^ x_mask) != b0())
                            && (y.payload() & (y.mask_xz() ^ y_mask) != b0());
                        let is_x = x.mask_xz() != &b0() || y.mask_xz() != &b0();

                        (is_one, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_1x(is_one, is_x);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::LogicOr => {
                let xy_width = x.width().max(y.width());
                let x = x.expand(xy_width, false);
                let y = y.expand(xy_width, false);

                let (is_one, is_x) = match (x.as_ref(), y.as_ref()) {
                    (Value::U64(x), Value::U64(y)) => {
                        let is_one = (x.payload & !x.mask_xz != 0) || (y.payload & !y.mask_xz != 0);
                        let is_x = x.mask_xz != 0 || y.mask_xz != 0;

                        (is_one, is_x)
                    }
                    (Value::BigUint(x), Value::BigUint(y)) => {
                        let x_mask = mask_cache.get(x.width as usize).clone();
                        let y_mask = mask_cache.get(y.width as usize);
                        let is_one = (x.payload() & (x.mask_xz() ^ x_mask) != b0())
                            || (y.payload() & (y.mask_xz() ^ y_mask) != b0());
                        let is_x = x.mask_xz() != &b0() || y.mask_xz() != &b0();

                        (is_one, is_x)
                    }
                    _ => unreachable!(),
                };

                let ret = ValueU64::new_bit_1x(is_one, is_x);
                let ret = Value::U64(ret);
                ret.expand(width, false).into_owned()
            }
            Op::LogicShiftR => {
                let x = x.expand(width, signed);
                let y = y.to_usize();

                match x.as_ref() {
                    Value::U64(x) => {
                        if let Some(y) = y {
                            let mut ret = x.clone();
                            ret.signed = false;
                            ret.payload >>= y;
                            ret.mask_xz >>= y;
                            let ret = Value::U64(ret);
                            ret.expand(width, false).into_owned()
                        } else {
                            Value::U64(ValueU64::new_x(width, false))
                        }
                    }
                    Value::BigUint(x) => {
                        if let Some(y) = y {
                            let mut ret = x.clone();
                            ret.signed = false;
                            *ret.payload >>= y;
                            *ret.mask_xz >>= y;
                            let ret = Value::BigUint(ret);
                            ret.expand(width, false).into_owned()
                        } else {
                            Value::BigUint(ValueBigUint::new_x(width, false))
                        }
                    }
                }
            }
            Op::LogicShiftL => {
                let x = x.expand(width, signed);
                let y = y.to_usize();

                match x.as_ref() {
                    Value::U64(x) => {
                        if let Some(y) = y {
                            let mask = ValueU64::gen_mask(width);
                            let mut ret = x.clone();
                            ret.signed = false;
                            ret.payload <<= y;
                            ret.mask_xz <<= y;
                            ret.payload &= mask;
                            ret.mask_xz &= mask;
                            let ret = Value::U64(ret);
                            ret.expand(width, false).into_owned()
                        } else {
                            Value::U64(ValueU64::new_x(width, false))
                        }
                    }
                    Value::BigUint(x) => {
                        if let Some(y) = y {
                            let mask = mask_cache.get(width);
                            let mut ret = x.clone();
                            ret.signed = false;
                            *ret.payload <<= y;
                            *ret.mask_xz <<= y;
                            *ret.payload &= mask;
                            *ret.mask_xz &= mask;
                            let ret = Value::BigUint(ret);
                            ret.expand(width, false).into_owned()
                        } else {
                            Value::BigUint(ValueBigUint::new_x(width, false))
                        }
                    }
                }
            }
            Op::ArithShiftR => {
                let x = x.expand(width, signed);
                let y = y.to_usize();

                match x.as_ref() {
                    Value::U64(x) => {
                        if let Some(y) = y {
                            let mut ret = x.clone();

                            let (ext_payload, ext_mask_xz) = if x.signed {
                                let mut ext_mask = ValueU64::gen_mask(width - y);
                                ext_mask ^= ValueU64::gen_mask(width);

                                let payload_msb = ((x.payload >> (x.width - 1)) & 1) == 1;
                                let mask_xz_msb = ((x.mask_xz >> (x.width - 1)) & 1) == 1;
                                let ext_payload = if payload_msb { ext_mask } else { 0 };
                                let ext_mask_xz = if mask_xz_msb { ext_mask } else { 0 };
                                (ext_payload, ext_mask_xz)
                            } else {
                                (0, 0)
                            };

                            ret.payload >>= y;
                            ret.mask_xz >>= y;
                            ret.payload |= ext_payload;
                            ret.mask_xz |= ext_mask_xz;
                            let ret = Value::U64(ret);
                            ret.expand(width, false).into_owned()
                        } else {
                            Value::U64(ValueU64::new_x(width, false))
                        }
                    }
                    Value::BigUint(x) => {
                        if let Some(y) = y {
                            let mut ret = x.clone();

                            let (ext_payload, ext_mask_xz) = if x.signed {
                                let mut ext_mask = mask_cache.get(width - y).clone();
                                ext_mask ^= mask_cache.get(width);

                                let payload_msb = ((x.payload() >> (x.width - 1)) & b1()) == b1();
                                let mask_xz_msb = ((x.mask_xz() >> (x.width - 1)) & b1()) == b1();
                                let ext_payload = if payload_msb { ext_mask.clone() } else { b0() };
                                let ext_mask_xz = if mask_xz_msb { ext_mask.clone() } else { b0() };
                                (ext_payload, ext_mask_xz)
                            } else {
                                (b0(), b0())
                            };

                            *ret.payload >>= y;
                            *ret.mask_xz >>= y;
                            *ret.payload |= ext_payload;
                            *ret.mask_xz |= ext_mask_xz;
                            let ret = Value::BigUint(ret);
                            ret.expand(width, false).into_owned()
                        } else {
                            Value::BigUint(ValueBigUint::new_x(width, false))
                        }
                    }
                }
            }
            Op::ArithShiftL => {
                let x = x.expand(width, signed);
                let y = y.to_usize();

                match x.as_ref() {
                    Value::U64(x) => {
                        if let Some(y) = y {
                            let mask = ValueU64::gen_mask(width);
                            let mut ret = x.clone();
                            ret.payload <<= y;
                            ret.mask_xz <<= y;
                            ret.payload &= mask;
                            ret.mask_xz &= mask;
                            let ret = Value::U64(ret);
                            ret.expand(width, false).into_owned()
                        } else {
                            Value::U64(ValueU64::new_x(width, false))
                        }
                    }
                    Value::BigUint(x) => {
                        if let Some(y) = y {
                            let mask = mask_cache.get(width);
                            let mut ret = x.clone();
                            *ret.payload <<= y;
                            *ret.mask_xz <<= y;
                            *ret.payload &= mask;
                            *ret.mask_xz &= mask;
                            let ret = Value::BigUint(ret);
                            ret.expand(width, false).into_owned()
                        } else {
                            Value::BigUint(ValueBigUint::new_x(width, false))
                        }
                    }
                }
            }
            Op::Pow => {
                let x = x.expand(width, signed);
                let y = y.to_usize();

                match x.as_ref() {
                    Value::U64(x) => {
                        if let Some(y) = y {
                            if x.is_xz() {
                                Value::U64(ValueU64::new_x(width, x.signed))
                            } else if x.signed {
                                let ret = x.to_i64().unwrap();
                                let ret = ret.pow(y as u32);
                                Value::U64(ValueU64::new(ret as u64, width, true))
                            } else {
                                let ret = x.payload.pow(y as u32);
                                Value::U64(ValueU64::new(ret, width, false))
                            }
                        } else {
                            Value::U64(ValueU64::new_x(width, false))
                        }
                    }
                    Value::BigUint(x) => {
                        if let Some(y) = y {
                            if x.is_xz() {
                                Value::BigUint(ValueBigUint::new_x(width, x.signed))
                            } else if x.signed {
                                let ret = x.to_bigint().unwrap();
                                let ret = ret.pow(y as u32);
                                Value::BigUint(ValueBigUint::new_bigint(ret, width, true))
                            } else {
                                let ret = x.payload.pow(y as u32);
                                Value::BigUint(ValueBigUint::new_biguint(ret, width, false))
                            }
                        } else {
                            Value::BigUint(ValueBigUint::new_x(width, false))
                        }
                    }
                }
            }
            _ => unimplemented!(),
        }
    }
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
            Op::EqWildcard => "==?",
            Op::Ne => "!=",
            Op::NeWildcard => "!=?",
            Op::LogicAnd => "&&",
            Op::LogicOr => "||",
            Op::LogicNot => "!",
            Op::BitAnd => "&",
            Op::BitOr => "|",
            Op::BitXor => "^",
            Op::BitXnor => "~^",
            Op::BitNand => "~&",
            Op::BitNor => "~|",
            Op::BitNot => "~",
            Op::As => "as",
            Op::Ternary => "ternary",
            Op::Concatenation => "concatenation",
            Op::ArrayLiteral => "array litaral",
            Op::Condition => "condition",
            Op::Repeat => "repeat",
        };

        str.fmt(f)
    }
}
