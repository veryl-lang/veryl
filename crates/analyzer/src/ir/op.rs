use crate::conv::Context;
use crate::conv::checker::clock_domain::check_clock_domain;
use crate::conv::utils::eval_repeat;
use crate::ir::{Comptime, Expression, ExpressionContext, Shape, Type, TypeKind, ValueVariant};
use crate::value::{MaskCache, Value, ValueBigUint, ValueU64};
use crate::{AnalyzerError, BigUint};
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

    pub fn eval_context_unary(&self, x: ExpressionContext) -> ExpressionContext {
        match self {
            Op::Add | Op::Sub | Op::BitNot => x,
            Op::BitAnd
            | Op::BitNand
            | Op::BitOr
            | Op::BitNor
            | Op::BitXor
            | Op::BitXnor
            | Op::LogicNot => ExpressionContext {
                width: 1,
                signed: false,
                is_const: x.is_const,
                is_global: x.is_global,
            },
            _ => unreachable!(),
        }
    }

    pub fn eval_context_binary(
        &self,
        x: ExpressionContext,
        y: ExpressionContext,
    ) -> ExpressionContext {
        match self {
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Rem
            | Op::BitAnd
            | Op::BitOr
            | Op::BitXor
            | Op::BitXnor => ExpressionContext {
                width: x.width.max(y.width),
                signed: x.signed & y.signed,
                is_const: x.is_const & y.is_const,
                is_global: x.is_global & y.is_global,
            },
            Op::Eq
            | Op::EqWildcard
            | Op::Ne
            | Op::NeWildcard
            | Op::Greater
            | Op::GreaterEq
            | Op::Less
            | Op::LessEq
            | Op::LogicAnd
            | Op::LogicOr => ExpressionContext {
                width: 1,
                signed: false,
                is_const: x.is_const & y.is_const,
                is_global: x.is_global & y.is_global,
            },
            Op::LogicShiftL | Op::LogicShiftR | Op::ArithShiftL | Op::ArithShiftR | Op::Pow => {
                ExpressionContext {
                    width: x.width,
                    signed: x.signed,
                    is_const: x.is_const & y.is_const,
                    is_global: x.is_global & y.is_global,
                }
            }
            _ => unreachable!(),
        }
    }

    fn invalid_operand(&self, context: &mut Context, x: &Comptime) -> Type {
        context.insert_error(AnalyzerError::invalid_operand(
            &x.r#type.to_string(),
            &self.to_string(),
            &x.token,
        ));
        return Type {
            kind: TypeKind::Unknown,
            ..Default::default()
        };
    }

    fn invalid_logical_operand(&self, context: &mut Context, x: &Comptime) -> Type {
        context.insert_error(AnalyzerError::invalid_logical_operand(true, &x.token));
        return Type {
            kind: TypeKind::Unknown,
            ..Default::default()
        };
    }

    pub fn eval_type_unary(&self, context: &mut Context, x: &Comptime, dst: &mut Comptime) {
        // array / type can't be operated
        if x.r#type.is_array() || x.r#type.is_type() {
            dst.r#type = self.invalid_operand(context, x);
            return;
        }

        // clock / reset can't be operated except ~ and !
        if (x.r#type.is_clock() || x.r#type.is_reset())
            && !matches!(self, Op::BitNot | Op::LogicNot)
        {
            dst.r#type = self.invalid_operand(context, x);
            return;
        }

        // logical operand should be 1-bit
        if (self == &Op::LogicNot) && !x.r#type.is_binary() {
            dst.r#type = self.invalid_logical_operand(context, x);
            return;
        }

        let kind = if x.r#type.is_2state() {
            TypeKind::Bit
        } else {
            TypeKind::Logic
        };

        let r#type = match self {
            Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor | Op::BitNand | Op::BitNor => Type {
                kind,
                signed: false,
                width: Shape::new(vec![Some(1)]),
                ..Default::default()
            },
            Op::BitNot | Op::LogicNot => Type {
                kind,
                signed: false,
                width: x.r#type.width.clone(),
                ..Default::default()
            },
            Op::Add | Op::Sub => x.r#type.clone(),
            _ => unreachable!(),
        };

        dst.r#type = r#type;
        dst.clock_domain = x.clock_domain;
    }

    pub fn eval_type_binary(
        &self,
        context: &mut Context,
        x: &Comptime,
        y: &Comptime,
        dst: &mut Comptime,
    ) {
        // array / type can't be operated
        if x.r#type.is_array() | x.r#type.is_type() {
            dst.r#type = self.invalid_operand(context, x);
            return;
        }
        if y.r#type.is_array() | (y.r#type.is_type() && self != &Op::As) {
            dst.r#type = self.invalid_operand(context, y);
            return;
        }

        // string and non-string can't be operated
        if x.r#type.is_string() ^ y.r#type.is_string() {
            if !x.r#type.is_string() {
                dst.r#type = self.invalid_operand(context, x);
                return;
            } else {
                dst.r#type = self.invalid_operand(context, y);
                return;
            }
        }
        if x.r#type.is_string() && !matches!(self, Op::Eq | Op::Ne) {
            dst.r#type = self.invalid_operand(context, x);
            return;
        }

        // logical operand should be 1-bit
        if matches!(self, Op::LogicAnd | Op::LogicOr) {
            if !x.r#type.is_binary() {
                dst.r#type = self.invalid_logical_operand(context, x);
                return;
            }
            if !y.r#type.is_binary() {
                dst.r#type = self.invalid_logical_operand(context, y);
                return;
            }
        }

        check_clock_domain(context, x, y, &dst.token.beg);

        let x_width = x.r#type.total_width();
        let y_width = y.r#type.total_width();

        let width = if let Some(x_width) = x_width
            && let Some(y_width) = y_width
        {
            Some(self.eval_binary_width_usize(x_width, y_width, None))
        } else {
            None
        };

        let kind = match self {
            Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor => {
                #[allow(clippy::if_same_then_else)]
                if x.r#type.is_unknown() | x.r#type.is_systemverilog() {
                    y.r#type.kind.clone()
                } else if y.r#type.is_unknown() | y.r#type.is_systemverilog() {
                    x.r#type.kind.clone()
                } else if x.r#type.is_clock() || x.r#type.is_reset() {
                    x.r#type.kind.clone()
                } else if y.r#type.is_clock() || y.r#type.is_reset() {
                    y.r#type.kind.clone()
                } else if x.r#type.is_2state() && y.r#type.is_2state() {
                    TypeKind::Bit
                } else {
                    TypeKind::Logic
                }
            }
            Op::Pow
            | Op::Div
            | Op::Rem
            | Op::Mul
            | Op::Add
            | Op::Sub
            | Op::LessEq
            | Op::GreaterEq
            | Op::Less
            | Op::Greater
            | Op::Eq
            | Op::EqWildcard
            | Op::Ne
            | Op::NeWildcard
            | Op::LogicAnd
            | Op::LogicOr => {
                if x.r#type.is_unknown() | x.r#type.is_systemverilog() {
                    y.r#type.kind.clone()
                } else if y.r#type.is_unknown() | y.r#type.is_systemverilog() {
                    x.r#type.kind.clone()
                } else if x.r#type.is_2state() && y.r#type.is_2state() {
                    TypeKind::Bit
                } else {
                    TypeKind::Logic
                }
            }
            Op::ArithShiftL | Op::ArithShiftR | Op::LogicShiftL | Op::LogicShiftR => {
                if x.r#type.is_2state() {
                    TypeKind::Bit
                } else {
                    TypeKind::Logic
                }
            }
            Op::As => {
                if let ValueVariant::Type(y_value) = &y.value {
                    let invalid_clock_cast = y_value.is_clock() && !x.r#type.is_clock();
                    let invalid_reset_cast = y_value.is_reset() && x.r#type.is_clock();

                    if invalid_clock_cast || invalid_reset_cast {
                        context.insert_error(AnalyzerError::invalid_cast(
                            &x.r#type.to_string(),
                            &y_value.to_string(),
                            &y.token,
                        ));
                    }

                    dst.r#type = y_value.clone();
                    return;
                } else {
                    dst.r#type = self.invalid_operand(context, y);
                    return;
                }
            }
            _ => unreachable!(),
        };

        dst.r#type.width = Shape::new(vec![width]);
        dst.r#type.kind = kind;
        dst.clock_domain = x.clock_domain;
    }

    pub fn eval_type_ternary(
        &self,
        context: &mut Context,
        x: &Comptime,
        y: &Comptime,
        z: &Comptime,
        dst: &mut Comptime,
    ) {
        // array / type can't be operated
        if x.r#type.is_array() | x.r#type.is_type() {
            dst.r#type = self.invalid_operand(context, x);
            return;
        }
        if y.r#type.is_array() | y.r#type.is_type() {
            dst.r#type = self.invalid_operand(context, y);
            return;
        }
        if z.r#type.is_array() | z.r#type.is_type() {
            dst.r#type = self.invalid_operand(context, z);
            return;
        }

        // condition should be 1-bit
        if !x.r#type.is_binary() {
            dst.r#type = self.invalid_logical_operand(context, x);
            return;
        }

        check_clock_domain(context, x, y, &dst.token.beg);
        check_clock_domain(context, x, z, &dst.token.beg);

        let y_width = y.r#type.total_width();
        let z_width = z.r#type.total_width();
        let width = y_width.max(z_width);

        let kind = if y.r#type.is_unknown() | y.r#type.is_systemverilog() {
            z.r#type.kind.clone()
        } else if z.r#type.is_unknown() | z.r#type.is_systemverilog() {
            y.r#type.kind.clone()
        } else if y.r#type.is_2state() && z.r#type.is_2state() {
            TypeKind::Bit
        } else {
            TypeKind::Logic
        };

        dst.r#type.kind = kind;
        dst.r#type.width = Shape::new(vec![width]);
        dst.clock_domain = x.clock_domain;
    }

    pub fn eval_type_concatenation(
        &self,
        context: &mut Context,
        x: &mut [(Expression, Option<Expression>)],
        dst: &mut Comptime,
    ) {
        let mut width = Some(0);
        let mut is_const = true;
        let mut is_global = true;
        let mut kind = TypeKind::Bit;
        for (expr, repeat) in x {
            let expr_context = expr.gather_context(context);
            expr.apply_context(context, expr_context);
            let expr = expr.comptime();

            // array / type can't be operated
            if expr.r#type.is_array() | expr.r#type.is_type() {
                dst.r#type = self.invalid_operand(context, &expr);
                return;
            }

            check_clock_domain(context, &dst, &expr, &dst.token.beg);
            dst.clock_domain = expr.clock_domain;

            if expr.r#type.is_4state() {
                kind = TypeKind::Logic;
            }

            is_const &= expr.is_const;
            is_global &= expr.is_global;

            if let Some(repeat) = repeat {
                if let Some(repeat) = eval_repeat(context, repeat) {
                    if let Some(total_width) = expr.r#type.total_width()
                        && let Some(width) = &mut width
                    {
                        *width += total_width * repeat.to_usize().unwrap_or(0);
                    } else {
                        width = None;
                    }
                } else {
                    width = None;
                }
            } else if let Some(total_width) = expr.r#type.total_width()
                && let Some(width) = &mut width
            {
                *width += total_width;
            } else {
                width = None;
            }
        }

        dst.r#type.kind = kind;
        dst.r#type.width = Shape::new(vec![width]);
        dst.is_const = is_const;
        dst.is_global = is_global;
    }

    pub fn unary_self_determined(&self) -> bool {
        matches!(
            self,
            Op::BitAnd
                | Op::BitNand
                | Op::BitOr
                | Op::BitNor
                | Op::BitXor
                | Op::BitXnor
                | Op::LogicNot
        )
    }

    pub fn binary_x_self_determined(&self) -> bool {
        matches!(self, Op::LogicAnd | Op::LogicOr)
    }

    pub fn binary_y_self_determined(&self) -> bool {
        matches!(
            self,
            Op::LogicAnd
                | Op::LogicOr
                | Op::LogicShiftL
                | Op::LogicShiftR
                | Op::ArithShiftL
                | Op::ArithShiftR
                | Op::Pow
        )
    }

    pub fn binary_op_self_determined(&self) -> bool {
        matches!(
            self,
            Op::Eq
                | Op::EqWildcard
                | Op::Ne
                | Op::NeWildcard
                | Op::Greater
                | Op::GreaterEq
                | Op::Less
                | Op::LessEq
        )
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

    pub fn eval_value_unary(
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

    pub fn eval_value_binary(
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
