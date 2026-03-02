use crate::conv::Context;
use crate::conv::checker::clock_domain::check_clock_domain;
use crate::conv::utils::eval_repeat;
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::{
    Comptime, FfTable, FunctionCall, Op, Shape, SystemFunctionCall, Type, TypeKind, ValueVariant,
    VarId, VarIndex, VarSelect,
};
use crate::symbol::ClockDomain;
use crate::value::{Value, ValueBigUint};
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::ExpressionIdentifier;

#[derive(Clone, Debug)]
pub enum Expression {
    Term(Box<Factor>),
    Unary(Op, Box<Expression>),
    Binary(Box<Expression>, Op, Box<Expression>),
    Ternary(Box<Expression>, Box<Expression>, Box<Expression>),
    Concatenation(Vec<(Expression, Option<Expression>)>),
    ArrayLiteral(Vec<ArrayLiteralItem>),
    StructConstructor(Type, Vec<(StrId, Expression)>),
}

impl Expression {
    pub fn create_value(value: Value, token: TokenRange) -> Self {
        Self::Term(Box::new(Factor::create_value(value, token)))
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = match self {
            Expression::Term(x) => x.to_string(),
            Expression::Unary(x, y) => {
                format!("({x} {y})")
            }
            Expression::Binary(x, y, z) => {
                format!("({x} {y} {z})")
            }
            Expression::Ternary(x, y, z) => {
                format!("({x} ? {y} : {z})")
            }
            Expression::Concatenation(x) => {
                let mut ret = String::new();
                for (x, y) in x {
                    if let Some(y) = y {
                        ret = format!("{ret}, {x} repeat {y}")
                    } else {
                        ret = format!("{ret}, {x}")
                    }
                }
                format!("{{{}}}", &ret[2..])
            }
            Expression::ArrayLiteral(x) => {
                let mut ret = String::new();
                for x in x {
                    ret = format!("{ret}, {x}")
                }
                format!("'{{{}}}", &ret[2..])
            }
            Expression::StructConstructor(_, x) => {
                let mut ret = String::new();
                for x in x {
                    ret = format!("{ret}, {}: {}", x.0, x.1)
                }
                format!("'{{{}}}", &ret[2..])
            }
        };

        ret.fmt(f)
    }
}

impl Expression {
    pub fn is_assignable(&self) -> bool {
        match self {
            Expression::Term(x) => x.is_assignable(),
            Expression::Concatenation(x) => x.iter().all(|x| x.0.is_assignable() && x.1.is_none()),
            _ => false,
        }
    }

    pub fn eval_signed(&self) -> bool {
        match self {
            Expression::Term(x) => x.eval_signed(),
            Expression::Unary(op, x) => op.unary_signed(x.eval_signed()),
            Expression::Binary(x, op, y) => op.binary_signed(x.eval_signed(), y.eval_signed()),
            Expression::Ternary(_, y, z) => y.eval_signed() && z.eval_signed(),
            Expression::Concatenation(_) => false,
            Expression::StructConstructor(_, _) => false,
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_) => false,
        }
    }

    pub fn eval_value(
        &self,
        context: &mut Context,
        context_width: Option<usize>,
        signed: bool,
    ) -> Option<Value> {
        match self {
            Expression::Term(x) => x.eval_value(context, context_width),
            Expression::Unary(op, x) => {
                let x_context_width = op.unary_context_width(context_width);

                let ret = x.eval_value(context, x_context_width, signed)?;
                let ret = op.eval_unary(&ret, context_width, signed, &mut context.mask_cache);
                Some(ret)
            }
            Expression::Binary(x, op, y) => {
                let x_context_width = op.binary_x_context_width(context_width);
                let y_context_width = op.binary_y_context_width(context_width);

                let x = x.eval_value(context, x_context_width, signed)?;

                if op == &Op::As {
                    return Some(x);
                }

                let y = y.eval_value(context, y_context_width, signed)?;

                let ret = op.eval_binary(&x, &y, context_width, signed, &mut context.mask_cache);
                Some(ret)
            }
            Expression::Ternary(x, y, z) => {
                let x = x.eval_value(context, None, signed)?;
                let y = y.eval_value(context, context_width, signed)?;
                let z = z.eval_value(context, context_width, signed)?;

                let width = y.width().max(z.width());

                let ret = if x.to_usize().unwrap_or(0) == 0 { z } else { y };
                let ret = ret.expand(width, false).into_owned();
                Some(ret)
            }
            Expression::Concatenation(x) => {
                let mut ret = Value::new(0, 0, false);
                for (exp, rep) in x.iter() {
                    let exp = exp.eval_value(context, None, signed)?;

                    let rep = if let Some(rep) = rep {
                        let token = rep.token_range();
                        let rep = rep.eval_value(context, None, signed)?;
                        let rep = rep.to_usize()?;
                        context.check_size(rep, token)?
                    } else {
                        1
                    };

                    for _ in 0..rep {
                        ret = ret.concat(&exp);
                    }
                }
                Some(ret)
            }
            Expression::StructConstructor(r#type, exprs) => {
                let mut ret = Value::new(0u32.into(), 0, false);
                for (name, expr) in exprs {
                    let sub_type = r#type.get_member_type(*name)?;
                    let width = sub_type.total_width()?;
                    let mut value = expr.eval_value(context, Some(width), signed)?;
                    value.trunc(width);
                    ret = ret.concat(&value);
                }
                Some(ret)
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_) => None,
        }
    }

    pub fn eval_comptime(
        &mut self,
        context: &mut Context,
        context_width: Option<usize>,
        signed: bool,
    ) -> Comptime {
        let token = self.token_range();
        let value = self.eval_value(context, context_width, signed);
        let value = if let Some(x) = value {
            ValueVariant::Numeric(x)
        } else {
            ValueVariant::Unknown
        };

        let ret = match self {
            Expression::Term(x) => x.eval_comptime(context, context_width),
            Expression::Unary(op, x) => {
                let range = x.token_range();
                let x_context_width = op.unary_context_width(context_width);
                let x = x.eval_comptime(context, x_context_width, signed);
                let mut ret = x.clone();
                ret.value = value;

                // array / type can't be operated
                if ret.r#type.is_array() | ret.r#type.is_type() {
                    ret.invalid_operand(context, *op, &x.r#type, &range);
                    return ret;
                }

                match op {
                    // reduction
                    Op::BitAnd
                    | Op::BitOr
                    | Op::BitXor
                    | Op::BitXnor
                    | Op::BitNand
                    | Op::BitNor => {
                        if ret.r#type.is_clock() | ret.r#type.is_reset() {
                            ret.invalid_operand(context, *op, &x.r#type, &range);
                            return ret;
                        }
                        ret.r#type.signed = false;
                        ret.r#type.width = Shape::new(vec![Some(1)]);
                    }
                    Op::BitNot => {
                        ret.r#type.signed = false;
                    }
                    Op::LogicNot => {
                        // operand of ! should be 1-bit
                        if !ret.r#type.is_binary() {
                            ret.invalid_logical_operand(context, &range);
                            return ret;
                        }
                        ret.r#type.signed = false;
                    }
                    Op::Add | Op::Sub => {
                        if ret.r#type.is_clock() | ret.r#type.is_reset() {
                            ret.invalid_operand(context, *op, &x.r#type, &range);
                            return ret;
                        }
                    }
                    _ => unreachable!(),
                }

                ret.r#type.kind = if x.r#type.is_2state() {
                    TypeKind::Bit
                } else {
                    TypeKind::Logic
                };
                ret
            }
            Expression::Binary(x, op, y) => {
                let range_x = x.token_range();
                let range_y = y.token_range();

                let x_context_width = op.binary_x_context_width(context_width);
                let y_context_width = op.binary_y_context_width(context_width);

                let x = x.eval_comptime(context, x_context_width, signed);
                let y = y.eval_comptime(context, y_context_width, signed);

                let mut ret = x.clone();
                ret.value = value;

                // array / type can't be operated
                if x.r#type.is_array() | x.r#type.is_type() {
                    ret.invalid_operand(context, *op, &x.r#type, &range_x);
                    return ret;
                }
                if y.r#type.is_array() | (y.r#type.is_type() && *op != Op::As) {
                    ret.invalid_operand(context, *op, &y.r#type, &range_y);
                    return ret;
                }

                // string and non-string can't be operated
                if x.r#type.is_string() ^ y.r#type.is_string() {
                    if !x.r#type.is_string() {
                        ret.invalid_operand(context, *op, &x.r#type, &range_x);
                    } else {
                        ret.invalid_operand(context, *op, &y.r#type, &range_y);
                    }
                    return ret;
                }
                if x.r#type.is_string() && !matches!(op, Op::Eq | Op::Ne) {
                    ret.invalid_operand(context, *op, &x.r#type, &range_x);
                    return ret;
                }

                check_clock_domain(context, &x, &y, &token.beg);

                let x_width = x.r#type.total_width();
                let y_width = y.r#type.total_width();

                if matches!(op, Op::LogicAnd | Op::LogicOr) {
                    if !x.r#type.is_binary() {
                        ret.invalid_logical_operand(context, &range_x);
                        return ret;
                    }
                    if !y.r#type.is_binary() {
                        ret.invalid_logical_operand(context, &range_y);
                        return ret;
                    }
                }

                let width = match op {
                    Op::Pow
                    | Op::ArithShiftL
                    | Op::ArithShiftR
                    | Op::LogicShiftL
                    | Op::LogicShiftR => x_width.map(|x| x.max(context_width.unwrap_or(0))),
                    Op::Div
                    | Op::Rem
                    | Op::Mul
                    | Op::Add
                    | Op::Sub
                    | Op::BitAnd
                    | Op::BitOr
                    | Op::BitXor
                    | Op::BitXnor => {
                        if let Some(x_width) = x_width
                            && let Some(y_width) = y_width
                        {
                            Some(x_width.max(y_width).max(context_width.unwrap_or(0)))
                        } else {
                            None
                        }
                    }
                    Op::LessEq
                    | Op::GreaterEq
                    | Op::Less
                    | Op::Greater
                    | Op::Eq
                    | Op::EqWildcard
                    | Op::Ne
                    | Op::NeWildcard
                    | Op::LogicAnd
                    | Op::LogicOr => Some(1),
                    Op::As => {
                        if let ValueVariant::Numeric(y) = &y.value
                            && let Some(y) = y.to_usize()
                        {
                            Some(y)
                        } else {
                            Some(0)
                        }
                    }
                    _ => unreachable!(),
                };
                ret.r#type.width = Shape::new(vec![width]);

                match op {
                    Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor => {
                        #[allow(clippy::if_same_then_else)]
                        if x.r#type.is_unknown() | x.r#type.is_systemverilog() {
                            ret.r#type.kind = y.r#type.kind;
                        } else if y.r#type.is_unknown() | y.r#type.is_systemverilog() {
                            ret.r#type.kind = x.r#type.kind;
                        } else if x.r#type.is_clock() || x.r#type.is_reset() {
                            ret.r#type.kind = x.r#type.kind;
                        } else if y.r#type.is_clock() || y.r#type.is_reset() {
                            ret.r#type.kind = y.r#type.kind;
                        } else if x.r#type.is_2state() && y.r#type.is_2state() {
                            ret.r#type.kind = TypeKind::Bit;
                        } else {
                            ret.r#type.kind = TypeKind::Logic;
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
                            ret.r#type.kind = y.r#type.kind;
                        } else if y.r#type.is_unknown() | y.r#type.is_systemverilog() {
                            ret.r#type.kind = x.r#type.kind;
                        } else if x.r#type.is_2state() && y.r#type.is_2state() {
                            ret.r#type.kind = TypeKind::Bit;
                        } else {
                            ret.r#type.kind = TypeKind::Logic;
                        }
                    }
                    Op::ArithShiftL | Op::ArithShiftR | Op::LogicShiftL | Op::LogicShiftR => {
                        if x.r#type.is_2state() {
                            ret.r#type.kind = TypeKind::Bit;
                        } else {
                            ret.r#type.kind = TypeKind::Logic;
                        }
                    }
                    Op::As => {
                        if let ValueVariant::Numeric(y) = &y.value
                            && let Some(_width) = y.to_usize()
                        {
                            // TODO
                            // Check width range
                        } else if let ValueVariant::Type(y) = y.value {
                            let invalid_clock_cast = y.is_clock() && !x.r#type.is_clock();
                            let invalid_reset_cast = y.is_reset() && x.r#type.is_clock();

                            if invalid_clock_cast || invalid_reset_cast {
                                ret.invalid_cast(context, &y, &x.r#type, &range_y);
                            }

                            ret.r#type = y;
                        } else {
                            ret.invalid_operand(context, *op, &y.r#type, &range_y);
                            return ret;
                        }
                    }
                    _ => unreachable!(),
                }

                ret.is_const = x.is_const & y.is_const;
                ret.is_global = x.is_global & y.is_global;
                ret
            }
            Expression::Ternary(x, y, z) => {
                let range_x = x.token_range();
                let range_y = y.token_range();
                let range_z = z.token_range();

                let x = x.eval_comptime(context, None, signed);
                let y = y.eval_comptime(context, context_width, signed);
                let z = z.eval_comptime(context, context_width, signed);

                let mut ret = x.clone();
                ret.value = value;

                // array / type can't be operated
                if x.r#type.is_array() | x.r#type.is_type() {
                    ret.invalid_operand(context, Op::Ternary, &x.r#type, &range_x);
                    return ret;
                }
                if y.r#type.is_array() | y.r#type.is_type() {
                    ret.invalid_operand(context, Op::Ternary, &y.r#type, &range_y);
                    return ret;
                }
                if z.r#type.is_array() | z.r#type.is_type() {
                    ret.invalid_operand(context, Op::Ternary, &z.r#type, &range_z);
                    return ret;
                }

                // condition should be 1-bit
                if !x.r#type.is_binary() {
                    ret.invalid_logical_operand(context, &range_x);
                    return ret;
                }

                check_clock_domain(context, &x, &y, &self.token_range().beg);
                check_clock_domain(context, &x, &z, &self.token_range().beg);

                let y_width = y.r#type.total_width();
                let z_width = z.r#type.total_width();
                let width = y_width.max(z_width);

                ret.r#type.width = Shape::new(vec![width]);

                if y.r#type.is_unknown() | y.r#type.is_systemverilog() {
                    ret.r#type.kind = z.r#type.kind;
                } else if z.r#type.is_unknown() | z.r#type.is_systemverilog() {
                    ret.r#type.kind = y.r#type.kind;
                } else if y.r#type.is_2state() && z.r#type.is_2state() {
                    ret.r#type.kind = TypeKind::Bit;
                } else {
                    ret.r#type.kind = TypeKind::Logic;
                }

                ret.is_const = x.is_const & y.is_const & z.is_const;
                ret.is_global = x.is_global & y.is_global & z.is_global;

                ret
            }
            Expression::Concatenation(x) => {
                let mut ret = Comptime::create_unknown(ClockDomain::None, token);
                ret.value = value;

                let mut width = Some(0);
                let mut is_const = true;
                let mut is_global = true;
                let mut kind = TypeKind::Bit;
                for (expr, repeat) in x {
                    let range = expr.token_range();
                    let expr = expr.eval_comptime(context, None, signed);

                    // array / type can't be operated
                    if expr.r#type.is_array() | expr.r#type.is_type() {
                        ret.invalid_operand(context, Op::Concatenation, &expr.r#type, &range);
                        return ret;
                    }

                    check_clock_domain(context, &ret, &expr, &token.beg);
                    ret.clock_domain = expr.clock_domain;

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
                            let mut ret = Comptime::create_unknown(ClockDomain::None, token);
                            ret.is_const = is_const;
                            ret.is_global = is_global;
                            return ret;
                        }
                    } else if let Some(total_width) = expr.r#type.total_width()
                        && let Some(width) = &mut width
                    {
                        *width += total_width;
                    } else {
                        width = None;
                    }
                }

                ret.r#type.kind = kind;
                ret.r#type.width = Shape::new(vec![width]);
                ret.is_const = is_const;
                ret.is_global = is_global;

                ret
            }
            Expression::StructConstructor(r#type, exprs) => {
                let mut is_const = true;
                let mut is_global = true;
                for (_, expr) in exprs {
                    let comptime = expr.eval_comptime(context, None, signed);
                    is_const &= comptime.is_const;
                    is_global &= comptime.is_global;
                }

                let mut ret = Comptime::from_type(r#type.clone(), ClockDomain::None, token);
                ret.value = value;
                ret.is_const = is_const;
                ret.is_global = is_global;
                ret
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(items) => {
                let mut is_const = true;
                let mut is_global = true;
                for item in items {
                    is_const &= item.is_const(context);
                    is_global &= item.is_global(context);
                }

                let mut ret = Comptime::create_unknown(ClockDomain::None, token);
                ret.is_const = is_const;
                ret.is_global = is_global;
                ret
            }
        };

        // const optimization
        if ret.is_const
            && let Ok(value) = ret.get_value()
            && !value.is_xz()
        {
            *self = Expression::create_value(value.clone(), self.token_range());
        }

        ret
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        match self {
            Expression::Term(x) => x.eval_assign(context, assign_table, assign_context),
            Expression::Unary(_, x) => x.eval_assign(context, assign_table, assign_context),
            Expression::Binary(x, _, y) => {
                x.eval_assign(context, assign_table, assign_context);
                y.eval_assign(context, assign_table, assign_context);
            }
            Expression::Ternary(x, y, z) => {
                x.eval_assign(context, assign_table, assign_context);
                y.eval_assign(context, assign_table, assign_context);
                z.eval_assign(context, assign_table, assign_context);
            }
            Expression::Concatenation(x) => {
                for (x, y) in x {
                    x.eval_assign(context, assign_table, assign_context);
                    if let Some(y) = y {
                        y.eval_assign(context, assign_table, assign_context);
                    }
                }
            }
            Expression::StructConstructor(_, exprs) => {
                for (_, expr) in exprs {
                    expr.eval_assign(context, assign_table, assign_context);
                }
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_) => (),
        }
    }

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        match self {
            Expression::Term(x) => x.gather_ff(context, table, decl),
            Expression::Unary(_, x) => x.gather_ff(context, table, decl),
            Expression::Binary(x, _, y) => {
                x.gather_ff(context, table, decl);
                y.gather_ff(context, table, decl);
            }
            Expression::Ternary(x, y, z) => {
                x.gather_ff(context, table, decl);
                y.gather_ff(context, table, decl);
                z.gather_ff(context, table, decl);
            }
            Expression::Concatenation(x) => {
                for (x, y) in x {
                    x.gather_ff(context, table, decl);
                    if let Some(y) = y {
                        y.gather_ff(context, table, decl);
                    }
                }
            }
            Expression::StructConstructor(_, exprs) => {
                for (_, expr) in exprs {
                    expr.gather_ff(context, table, decl);
                }
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_) => (),
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        match self {
            Expression::Term(x) => x.set_index(index),
            Expression::Unary(_, x) => x.set_index(index),
            Expression::Binary(x, _, y) => {
                x.set_index(index);
                y.set_index(index);
            }
            Expression::Ternary(x, y, z) => {
                x.set_index(index);
                y.set_index(index);
                z.set_index(index);
            }
            Expression::Concatenation(x) => {
                for (x, y) in x {
                    x.set_index(index);
                    if let Some(y) = y {
                        y.set_index(index);
                    }
                }
            }
            Expression::StructConstructor(_, exprs) => {
                for (_, expr) in exprs {
                    expr.set_index(index);
                }
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_) => (),
        }
    }

    pub fn token_range(&self) -> TokenRange {
        match self {
            Expression::Term(x) => x.token_range(),
            Expression::Unary(_, x) => x.token_range(),
            Expression::Binary(x, _, y) => {
                let beg = x.token_range();
                let mut end = y.token_range();
                end.set_beg(beg);
                end
            }
            Expression::Ternary(x, _, y) => {
                let beg = x.token_range();
                let mut end = y.token_range();
                end.set_beg(beg);
                end
            }
            Expression::Concatenation(x) => {
                let beg = x.first().unwrap().0.token_range();
                let last = x.last().unwrap();
                let mut end = if let Some(x) = &last.1 {
                    x.token_range()
                } else {
                    last.0.token_range()
                };
                end.set_beg(beg);
                end
            }
            Expression::ArrayLiteral(x) => {
                let beg = x.first().unwrap().token_range();
                let mut end = x.last().unwrap().token_range();
                end.set_beg(beg);
                end
            }
            Expression::StructConstructor(_, x) => {
                let beg = x.first().unwrap().1.token_range();
                let mut end = x.last().unwrap().1.token_range();
                end.set_beg(beg);
                end
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Factor {
    Variable(VarId, VarIndex, VarSelect, Comptime, TokenRange),
    Value(Comptime, TokenRange),
    SystemFunctionCall(SystemFunctionCall, TokenRange),
    FunctionCall(FunctionCall, TokenRange),
    Anonymous(TokenRange),
    Unresolved(ExpressionIdentifier, TokenRange),
    Unknown(TokenRange),
}

impl Factor {
    pub fn create_value(value: Value, token: TokenRange) -> Self {
        let comptime = Comptime::create_value(value, token);
        Factor::Value(comptime, token)
    }

    pub fn is_assignable(&self) -> bool {
        match self {
            // SystemVerilog member is interpreted as Factor::Value, but it may be assignable.
            Factor::Value(x, _) => x.r#type.is_systemverilog(),
            Factor::FunctionCall(_, _) | Factor::SystemFunctionCall(_, _) => false,
            _ => true,
        }
    }

    pub fn eval_signed(&self) -> bool {
        match self {
            Factor::Variable(_, _, select, comptime, _) => {
                if select.is_empty() {
                    comptime.r#type.signed
                } else {
                    false
                }
            }
            Factor::Value(x, _) => x.r#type.signed,
            Factor::SystemFunctionCall(_, _) => false,
            Factor::FunctionCall(x, _) => {
                if let Some(x) = &x.ret {
                    x.r#type.signed
                } else {
                    false
                }
            }
            Factor::Anonymous(_) => false,
            Factor::Unresolved(_, _) => false,
            Factor::Unknown(_) => false,
        }
    }

    pub fn eval_value(
        &self,
        context: &mut Context,
        _context_width: Option<usize>,
    ) -> Option<Value> {
        match self {
            Factor::Variable(id, index, select, comptime, _) => {
                let index = index.eval_value(context)?;
                let value = context.variables.get(id)?.get_value(&index)?.clone();

                if !select.is_empty() {
                    let (beg, end) = select.eval_value(context, &comptime.r#type, false)?;
                    Some(value.select(beg, end))
                } else {
                    Some(value)
                }
            }
            Factor::Value(x, _) => x.get_value().ok().cloned(),
            Factor::SystemFunctionCall(x, _) => x.eval_value(context),
            Factor::FunctionCall(x, _) => x.eval_value(context),
            Factor::Anonymous(_) => None,
            Factor::Unresolved(_, _) => None,
            Factor::Unknown(_) => None,
        }
    }

    pub fn eval_comptime(
        &mut self,
        context: &mut Context,
        context_width: Option<usize>,
    ) -> Comptime {
        let value = self.eval_value(context, context_width);
        match self {
            Factor::Variable(_, index, select, comptime, _) => {
                let mut ret = comptime.clone();

                let value = if let Some(x) = value {
                    ValueVariant::Numeric(x)
                } else {
                    ValueVariant::Unknown
                };

                ret.r#type.array.drain(0..index.dimension());

                // Struct/Union/Enum should be treated as flatten bit/logic when it is bit-selected
                if !select.is_empty() {
                    ret.r#type.flatten_struct_union_enum()
                }

                if let Some(width) = select.eval_comptime(context, &ret.r#type, false) {
                    ret.r#type.width = width;
                }

                ret.value = value;
                ret
            }
            Factor::Value(x, _) => x.clone(),
            Factor::SystemFunctionCall(x, _) => x.eval_comptime(context),
            Factor::FunctionCall(x, _) => x.eval_comptime(context),
            Factor::Anonymous(token) => {
                let value = ValueVariant::Unknown;
                let r#type = Type {
                    kind: TypeKind::Unknown,
                    ..Default::default()
                };
                Comptime {
                    value,
                    r#type,
                    is_const: true,
                    is_global: true,
                    token: *token,
                    ..Default::default()
                }
            }
            Factor::Unresolved(_, token) => {
                let value = ValueVariant::Unknown;
                let r#type = Type {
                    kind: TypeKind::Unknown,
                    ..Default::default()
                };
                Comptime {
                    value,
                    r#type,
                    is_const: true,
                    is_global: false,
                    token: *token,
                    ..Default::default()
                }
            }
            Factor::Unknown(token) => {
                let value = ValueVariant::Unknown;
                let r#type = Type {
                    kind: TypeKind::Unknown,
                    ..Default::default()
                };
                Comptime {
                    value,
                    r#type,
                    is_const: false,
                    is_global: false,
                    token: *token,
                    ..Default::default()
                }
            }
        }
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        match self {
            Factor::Variable(id, index, select, _, _) => {
                if let Some(index) = index.eval_value(context)
                    && let Some(variable) = context.variables.get(id).cloned()
                    && let Some((beg, end)) = select.eval_value(context, &variable.r#type, false)
                {
                    let mask = ValueBigUint::gen_mask_range(beg, end);
                    assign_table.insert_reference(&variable, index, mask);
                }
            }
            Factor::FunctionCall(x, _) => {
                x.eval_assign(context, assign_table, assign_context);
            }
            Factor::SystemFunctionCall(x, _) => {
                x.eval_assign(context, assign_table, assign_context);
            }
            _ => (),
        }
    }

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        match self {
            Factor::Variable(id, index, _, _, _) => {
                if let Some(variable) = context.get_variable_info(*id) {
                    if let Some(index) = index.eval_value(context) {
                        if let Some(index) = variable.r#type.array.calc_index(&index) {
                            table.insert_refered(*id, index, decl);
                        }
                    } else if let Some(total_array) = variable.r#type.total_array() {
                        for i in 0..total_array {
                            table.insert_refered(*id, i, decl);
                        }
                    }
                }
            }
            Factor::FunctionCall(x, _) => {
                x.gather_ff(context, table, decl);
            }
            _ => (),
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        match self {
            Factor::Variable(_, i, _, _, _) => {
                *i = index.clone();
            }
            Factor::FunctionCall(x, _) => {
                x.set_index(index);
            }
            _ => (),
        }
    }

    pub fn token_range(&self) -> TokenRange {
        match self {
            Factor::Variable(_, _, _, _, x) => *x,
            Factor::Value(_, x) => *x,
            Factor::SystemFunctionCall(_, x) => *x,
            Factor::FunctionCall(_, x) => *x,
            Factor::Anonymous(x) => *x,
            Factor::Unresolved(_, x) => *x,
            Factor::Unknown(x) => *x,
        }
    }
}

impl fmt::Display for Factor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = match self {
            Factor::Variable(id, index, select, _, _) => {
                format!("{id}{index}{select}")
            }
            Factor::Value(x, _) => {
                if let Ok(x) = x.get_value() {
                    format!("{:x}", x)
                } else {
                    String::from("unknown")
                }
            }
            Factor::SystemFunctionCall(x, _) => x.to_string(),
            Factor::FunctionCall(x, _) => x.to_string(),
            Factor::Anonymous(_) => String::from("_"),
            Factor::Unresolved(_, _) => String::from("unresolved"),
            Factor::Unknown(_) => String::from("unknown"),
        };

        ret.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub enum ArrayLiteralItem {
    Value(Expression, Option<Expression>),
    Defaul(Expression),
}

impl ArrayLiteralItem {
    pub fn token_range(&self) -> TokenRange {
        match self {
            ArrayLiteralItem::Value(x, y) => {
                let beg = x.token_range();
                let mut end = if let Some(y) = y {
                    y.token_range()
                } else {
                    beg
                };
                end.set_beg(beg);
                end
            }
            ArrayLiteralItem::Defaul(x) => x.token_range(),
        }
    }

    pub fn is_const(&mut self, context: &mut Context) -> bool {
        match self {
            ArrayLiteralItem::Value(x, y) => {
                let mut ret = x.eval_comptime(context, None, false).is_const;
                if let Some(y) = y {
                    ret &= y.eval_comptime(context, None, false).is_const;
                }
                ret
            }
            ArrayLiteralItem::Defaul(x) => x.eval_comptime(context, None, false).is_const,
        }
    }

    pub fn is_global(&mut self, context: &mut Context) -> bool {
        match self {
            ArrayLiteralItem::Value(x, y) => {
                let mut ret = x.eval_comptime(context, None, false).is_global;
                if let Some(y) = y {
                    ret &= y.eval_comptime(context, None, false).is_global;
                }
                ret
            }
            ArrayLiteralItem::Defaul(x) => x.eval_comptime(context, None, false).is_global,
        }
    }
}

impl fmt::Display for ArrayLiteralItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = match self {
            ArrayLiteralItem::Value(x, y) => {
                if let Some(y) = y {
                    format!("{} repeat {}", x, y)
                } else {
                    format!("{}", x)
                }
            }
            ArrayLiteralItem::Defaul(x) => {
                format!("default: {}", x)
            }
        };

        ret.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conv::utils::parse_expression;
    use crate::conv::{Context, Conv};
    use veryl_parser::veryl_token::{Token, TokenSource};

    fn calc_expression(s: &str, context_width: Option<usize>) -> Value {
        let mut context = Context::default();
        let x = parse_expression(s);
        let x: Expression = Conv::conv(&mut context, &x).unwrap();
        x.eval_value(&mut context, context_width, x.eval_signed())
            .unwrap()
    }

    #[test]
    fn arithmetic() {
        let x00 = calc_expression("1 + 2", None);
        let x01 = calc_expression("5 - 1", None);
        let x02 = calc_expression("2 * 7", None);
        let x03 = calc_expression("8 / 3", None);
        let x04 = calc_expression("9 % 4", None);
        let x05 = calc_expression("2 ** 3", None);
        let x06 = calc_expression("+5", None);
        let x07 = calc_expression("-1", None);
        let x08 = calc_expression("1 << 2", None);
        let x09 = calc_expression("1 <<< 2", None);
        let x10 = calc_expression("-8 >> 2", None);
        let x11 = calc_expression("-8 >>> 2", None);

        assert_eq!(format!("{:x}", x00), "32'sh00000003");
        assert_eq!(format!("{:x}", x01), "32'sh00000004");
        assert_eq!(format!("{:x}", x02), "32'sh0000000e");
        assert_eq!(format!("{:x}", x03), "32'sh00000002");
        assert_eq!(format!("{:x}", x04), "32'sh00000001");
        assert_eq!(format!("{:x}", x05), "32'sh00000008");
        assert_eq!(format!("{:x}", x06), "32'sh00000005");
        assert_eq!(format!("{:x}", x07), "32'shffffffff");
        assert_eq!(format!("{:x}", x08), "32'h00000004");
        assert_eq!(format!("{:x}", x09), "32'sh00000004");
        assert_eq!(format!("{:x}", x10), "32'h3ffffffe");
        assert_eq!(format!("{:x}", x11), "32'shfffffffe");
    }

    #[test]
    fn relational() {
        let x0 = calc_expression("1 <: 2", None);
        let x1 = calc_expression("1 >: 2", None);
        let x2 = calc_expression("1 <= 2", None);
        let x3 = calc_expression("1 >= 2", None);
        let x4 = calc_expression("2 <: 2", None);
        let x5 = calc_expression("2 >: 2", None);
        let x6 = calc_expression("2 <= 2", None);
        let x7 = calc_expression("2 >= 2", None);

        assert_eq!(format!("{:x}", x0), "1'h1");
        assert_eq!(format!("{:x}", x1), "1'h0");
        assert_eq!(format!("{:x}", x2), "1'h1");
        assert_eq!(format!("{:x}", x3), "1'h0");
        assert_eq!(format!("{:x}", x4), "1'h0");
        assert_eq!(format!("{:x}", x5), "1'h0");
        assert_eq!(format!("{:x}", x6), "1'h1");
        assert_eq!(format!("{:x}", x7), "1'h1");
    }

    #[test]
    fn equality() {
        let x0 = calc_expression("1 == 2", None);
        let x1 = calc_expression("1 != 2", None);
        let x2 = calc_expression("2 == 2", None);
        let x3 = calc_expression("2 != 2", None);

        assert_eq!(format!("{:x}", x0), "1'h0");
        assert_eq!(format!("{:x}", x1), "1'h1");
        assert_eq!(format!("{:x}", x2), "1'h1");
        assert_eq!(format!("{:x}", x3), "1'h0");
    }

    #[test]
    fn wildcard_equality() {
        let x0 = calc_expression("4'b0000 ==? 4'b00xx", None);
        let x1 = calc_expression("4'b0011 ==? 4'b00xx", None);
        let x2 = calc_expression("4'b0100 ==? 4'b00xx", None);
        let x3 = calc_expression("4'b0111 ==? 4'b00xx", None);
        let x4 = calc_expression("4'b0000 !=? 4'b00xx", None);
        let x5 = calc_expression("4'b0011 !=? 4'b00xx", None);
        let x6 = calc_expression("4'b0100 !=? 4'b00xx", None);
        let x7 = calc_expression("4'b0111 !=? 4'b00xx", None);

        assert_eq!(format!("{:x}", x0), "1'h1");
        assert_eq!(format!("{:x}", x1), "1'h1");
        assert_eq!(format!("{:x}", x2), "1'h0");
        assert_eq!(format!("{:x}", x3), "1'h0");
        assert_eq!(format!("{:x}", x4), "1'h0");
        assert_eq!(format!("{:x}", x5), "1'h0");
        assert_eq!(format!("{:x}", x6), "1'h1");
        assert_eq!(format!("{:x}", x7), "1'h1");
    }

    #[test]
    fn logical() {
        let x0 = calc_expression("10 && 0", None);
        let x1 = calc_expression("10 || 0", None);
        let x2 = calc_expression("!0", None);
        let x3 = calc_expression("!10", None);

        assert_eq!(format!("{:x}", x0), "1'h0");
        assert_eq!(format!("{:x}", x1), "1'h1");
        assert_eq!(format!("{:x}", x2), "1'h1");
        assert_eq!(format!("{:x}", x3), "1'h0");
    }

    #[test]
    fn bitwise() {
        let x0 = calc_expression("4'b0001  & 4'b0101", None);
        let x1 = calc_expression("4'b0001  | 4'b0101", None);
        let x2 = calc_expression("4'b0001  ^ 4'b0101", None);
        let x3 = calc_expression("4'b0001 ~^ 4'b0101", None);
        let x4 = calc_expression("~4'b0101", None);

        assert_eq!(format!("{:x}", x0), "4'h1");
        assert_eq!(format!("{:x}", x1), "4'h5");
        assert_eq!(format!("{:x}", x2), "4'h4");
        assert_eq!(format!("{:x}", x3), "4'hb");
        assert_eq!(format!("{:x}", x4), "4'ha");
    }

    #[test]
    fn reduction() {
        let x00 = calc_expression(" &4'b0000", None);
        let x01 = calc_expression(" |4'b0000", None);
        let x02 = calc_expression(" ^4'b0000", None);
        let x03 = calc_expression("~&4'b0000", None);
        let x04 = calc_expression("~|4'b0000", None);
        let x05 = calc_expression("~^4'b0000", None);

        let x06 = calc_expression(" &4'b1111", None);
        let x07 = calc_expression(" |4'b1111", None);
        let x08 = calc_expression(" ^4'b1111", None);
        let x09 = calc_expression("~&4'b1111", None);
        let x10 = calc_expression("~|4'b1111", None);
        let x11 = calc_expression("~^4'b1111", None);

        let x12 = calc_expression(" &4'b0110", None);
        let x13 = calc_expression(" |4'b0110", None);
        let x14 = calc_expression(" ^4'b0110", None);
        let x15 = calc_expression("~&4'b0110", None);
        let x16 = calc_expression("~|4'b0110", None);
        let x17 = calc_expression("~^4'b0110", None);

        let x18 = calc_expression(" &4'b1000", None);
        let x19 = calc_expression(" |4'b1000", None);
        let x20 = calc_expression(" ^4'b1000", None);
        let x21 = calc_expression("~&4'b1000", None);
        let x22 = calc_expression("~|4'b1000", None);
        let x23 = calc_expression("~^4'b1000", None);

        assert_eq!(format!("{:x}", x00), "1'h0");
        assert_eq!(format!("{:x}", x01), "1'h0");
        assert_eq!(format!("{:x}", x02), "1'h0");
        assert_eq!(format!("{:x}", x03), "1'h1");
        assert_eq!(format!("{:x}", x04), "1'h1");
        assert_eq!(format!("{:x}", x05), "1'h1");

        assert_eq!(format!("{:x}", x06), "1'h1");
        assert_eq!(format!("{:x}", x07), "1'h1");
        assert_eq!(format!("{:x}", x08), "1'h0");
        assert_eq!(format!("{:x}", x09), "1'h0");
        assert_eq!(format!("{:x}", x10), "1'h0");
        assert_eq!(format!("{:x}", x11), "1'h1");

        assert_eq!(format!("{:x}", x12), "1'h0");
        assert_eq!(format!("{:x}", x13), "1'h1");
        assert_eq!(format!("{:x}", x14), "1'h0");
        assert_eq!(format!("{:x}", x15), "1'h1");
        assert_eq!(format!("{:x}", x16), "1'h0");
        assert_eq!(format!("{:x}", x17), "1'h1");

        assert_eq!(format!("{:x}", x18), "1'h0");
        assert_eq!(format!("{:x}", x19), "1'h1");
        assert_eq!(format!("{:x}", x20), "1'h1");
        assert_eq!(format!("{:x}", x21), "1'h1");
        assert_eq!(format!("{:x}", x22), "1'h0");
        assert_eq!(format!("{:x}", x23), "1'h0");
    }

    #[test]
    fn conditional() {
        let x0 = calc_expression("if 0 ? 1 : 2", None);
        let x1 = calc_expression("if 1 ? 1 : 2", None);

        assert_eq!(format!("{:x}", x0), "32'sh00000002");
        assert_eq!(format!("{:x}", x1), "32'sh00000001");
    }

    #[test]
    fn concatenation() {
        let x0 = calc_expression("{4'h1, 4'h3}", None);
        let x1 = calc_expression("{4'h1 repeat 3, 4'h3}", None);

        assert_eq!(format!("{:x}", x0), "8'h13");
        assert_eq!(format!("{:x}", x1), "16'h1113");
    }

    #[test]
    fn inside_outside() {
        let x0 = calc_expression("inside 1 {1, 2}", None);
        let x1 = calc_expression("inside 0 {1, 2}", None);
        let x2 = calc_expression("inside 5 {3, 2..=10}", None);
        let x3 = calc_expression("inside 1 {3, 2..=10}", None);
        let x4 = calc_expression("outside 1 {1, 2}", None);
        let x5 = calc_expression("outside 0 {1, 2}", None);
        let x6 = calc_expression("outside 5 {3, 2..=10}", None);
        let x7 = calc_expression("outside 1 {3, 2..=10}", None);

        assert_eq!(format!("{:x}", x0), "1'h1");
        assert_eq!(format!("{:x}", x1), "1'h0");
        assert_eq!(format!("{:x}", x2), "1'h1");
        assert_eq!(format!("{:x}", x3), "1'h0");
        assert_eq!(format!("{:x}", x4), "1'h0");
        assert_eq!(format!("{:x}", x5), "1'h1");
        assert_eq!(format!("{:x}", x6), "1'h0");
        assert_eq!(format!("{:x}", x7), "1'h1");
    }

    fn token_range() -> TokenRange {
        let beg = Token::new("", 0, 0, 0, 0, TokenSource::External);
        let end = beg;
        TokenRange { beg, end }
    }

    fn bit(width: usize) -> Box<Expression> {
        let ret = Comptime {
            value: ValueVariant::Unknown,
            r#type: Type {
                kind: TypeKind::Bit,
                width: Shape::new(vec![Some(width)]),
                ..Default::default()
            },
            ..Default::default()
        };
        Box::new(Expression::Term(Box::new(Factor::Value(
            ret,
            token_range(),
        ))))
    }

    fn logic(width: usize) -> Box<Expression> {
        let ret = Comptime {
            value: ValueVariant::Unknown,
            r#type: Type {
                kind: TypeKind::Logic,
                width: Shape::new(vec![Some(width)]),
                ..Default::default()
            },
            ..Default::default()
        };
        Box::new(Expression::Term(Box::new(Factor::Value(
            ret,
            token_range(),
        ))))
    }

    fn value(value: usize) -> Box<Expression> {
        let ret = Comptime {
            value: ValueVariant::Numeric(Value::new(value as u64, 32, false)),
            r#type: Type {
                kind: TypeKind::Logic,
                width: Shape::new(vec![Some(32)]),
                ..Default::default()
            },
            is_const: true,
            is_global: true,
            ..Default::default()
        };
        Box::new(Expression::Term(Box::new(Factor::Value(
            ret,
            token_range(),
        ))))
    }

    fn eval_comptime_unary(context: &mut Context, op: Op, x: Box<Expression>) -> Comptime {
        let mut ret = Expression::Unary(op, x);
        ret.eval_comptime(context, None, ret.eval_signed())
    }

    fn eval_comptime_binary(
        context: &mut Context,
        x: Box<Expression>,
        op: Op,
        y: Box<Expression>,
    ) -> Comptime {
        let mut ret = Expression::Binary(x, op, y);
        ret.eval_comptime(context, None, ret.eval_signed())
    }

    fn eval_comptime_ternary(
        context: &mut Context,
        x: Box<Expression>,
        y: Box<Expression>,
        z: Box<Expression>,
    ) -> Comptime {
        let mut ret = Expression::Ternary(x, y, z);
        ret.eval_comptime(context, None, ret.eval_signed())
    }

    fn eval_comptime_concat(
        context: &mut Context,
        x: Vec<(Expression, Option<Expression>)>,
    ) -> Comptime {
        let mut ret = Expression::Concatenation(x);
        ret.eval_comptime(context, None, ret.eval_signed())
    }

    #[test]
    fn unary_type() {
        let mut context = Context::default();

        let x0 = eval_comptime_unary(&mut context, Op::BitAnd, bit(3));
        let x1 = eval_comptime_unary(&mut context, Op::BitAnd, logic(4));
        let x2 = eval_comptime_unary(&mut context, Op::BitNot, bit(3));
        let x3 = eval_comptime_unary(&mut context, Op::BitNot, logic(4));
        let x4 = eval_comptime_unary(&mut context, Op::LogicNot, bit(1));
        let x5 = eval_comptime_unary(&mut context, Op::LogicNot, logic(1));
        let x6 = eval_comptime_unary(&mut context, Op::Add, bit(3));
        let x7 = eval_comptime_unary(&mut context, Op::Add, logic(4));

        let errors = context.drain_errors();
        assert!(errors.is_empty());

        assert_eq!(format!("{}", x0.r#type), "bit<1>");
        assert_eq!(format!("{}", x1.r#type), "logic<1>");
        assert_eq!(format!("{}", x2.r#type), "bit<3>");
        assert_eq!(format!("{}", x3.r#type), "logic<4>");
        assert_eq!(format!("{}", x4.r#type), "bit<1>");
        assert_eq!(format!("{}", x5.r#type), "logic<1>");
        assert_eq!(format!("{}", x6.r#type), "bit<3>");
        assert_eq!(format!("{}", x7.r#type), "logic<4>");
    }

    #[test]
    fn binary_type() {
        let mut context = Context::default();

        let x00 = eval_comptime_binary(&mut context, bit(4), Op::Add, bit(3));
        let x01 = eval_comptime_binary(&mut context, bit(4), Op::Add, logic(3));
        let x02 = eval_comptime_binary(&mut context, logic(4), Op::Add, logic(3));
        let x03 = eval_comptime_binary(&mut context, bit(4), Op::Eq, bit(3));
        let x04 = eval_comptime_binary(&mut context, bit(4), Op::Eq, logic(3));
        let x05 = eval_comptime_binary(&mut context, logic(4), Op::Eq, logic(3));
        let x06 = eval_comptime_binary(&mut context, bit(1), Op::LogicAnd, bit(1));
        let x07 = eval_comptime_binary(&mut context, bit(1), Op::LogicAnd, logic(1));
        let x08 = eval_comptime_binary(&mut context, logic(1), Op::LogicAnd, logic(1));
        let x09 = eval_comptime_binary(&mut context, bit(4), Op::ArithShiftL, bit(3));
        let x10 = eval_comptime_binary(&mut context, bit(4), Op::ArithShiftL, logic(3));
        let x11 = eval_comptime_binary(&mut context, logic(4), Op::ArithShiftL, logic(3));

        let errors = context.drain_errors();
        assert!(errors.is_empty());

        assert_eq!(format!("{}", x00.r#type), "bit<4>");
        assert_eq!(format!("{}", x01.r#type), "logic<4>");
        assert_eq!(format!("{}", x02.r#type), "logic<4>");
        assert_eq!(format!("{}", x03.r#type), "bit<1>");
        assert_eq!(format!("{}", x04.r#type), "logic<1>");
        assert_eq!(format!("{}", x05.r#type), "logic<1>");
        assert_eq!(format!("{}", x06.r#type), "bit<1>");
        assert_eq!(format!("{}", x07.r#type), "logic<1>");
        assert_eq!(format!("{}", x08.r#type), "logic<1>");
        assert_eq!(format!("{}", x09.r#type), "bit<4>");
        assert_eq!(format!("{}", x10.r#type), "bit<4>");
        assert_eq!(format!("{}", x11.r#type), "logic<4>");
    }

    #[test]
    fn ternary_type() {
        let mut context = Context::default();

        let x0 = eval_comptime_ternary(&mut context, bit(1), bit(4), bit(3));
        let x1 = eval_comptime_ternary(&mut context, bit(1), bit(4), logic(3));
        let x2 = eval_comptime_ternary(&mut context, bit(1), logic(4), bit(3));
        let x3 = eval_comptime_ternary(&mut context, bit(1), logic(4), logic(3));
        let x4 = eval_comptime_ternary(&mut context, logic(1), bit(4), bit(3));
        let x5 = eval_comptime_ternary(&mut context, logic(1), bit(4), logic(3));
        let x6 = eval_comptime_ternary(&mut context, logic(1), logic(4), bit(3));
        let x7 = eval_comptime_ternary(&mut context, logic(1), logic(4), logic(3));

        let errors = context.drain_errors();
        assert!(errors.is_empty());

        assert_eq!(format!("{}", x0.r#type), "bit<4>");
        assert_eq!(format!("{}", x1.r#type), "logic<4>");
        assert_eq!(format!("{}", x2.r#type), "logic<4>");
        assert_eq!(format!("{}", x3.r#type), "logic<4>");
        assert_eq!(format!("{}", x4.r#type), "bit<4>");
        assert_eq!(format!("{}", x5.r#type), "logic<4>");
        assert_eq!(format!("{}", x6.r#type), "logic<4>");
        assert_eq!(format!("{}", x7.r#type), "logic<4>");
    }

    #[test]
    fn concat_type() {
        let mut context = Context::default();

        let x0 = eval_comptime_concat(
            &mut context,
            vec![(*bit(1), None), (*bit(4), None), (*bit(3), None)],
        );
        let x1 = eval_comptime_concat(
            &mut context,
            vec![(*bit(1), None), (*logic(4), None), (*bit(3), None)],
        );
        let x2 = eval_comptime_concat(
            &mut context,
            vec![(*logic(1), None), (*logic(4), None), (*logic(3), None)],
        );
        let x3 = eval_comptime_concat(
            &mut context,
            vec![(*bit(1), Some(*value(2))), (*bit(4), None), (*bit(3), None)],
        );
        let x4 = eval_comptime_concat(
            &mut context,
            vec![
                (*bit(1), None),
                (*logic(4), Some(*value(2))),
                (*bit(3), None),
            ],
        );
        let x5 = eval_comptime_concat(
            &mut context,
            vec![
                (*logic(1), None),
                (*logic(4), None),
                (*logic(3), Some(*value(3))),
            ],
        );

        let errors = context.drain_errors();
        assert!(errors.is_empty());

        assert_eq!(format!("{}", x0.r#type), "bit<8>");
        assert_eq!(format!("{}", x1.r#type), "logic<8>");
        assert_eq!(format!("{}", x2.r#type), "logic<8>");
        assert_eq!(format!("{}", x3.r#type), "bit<9>");
        assert_eq!(format!("{}", x4.r#type), "logic<12>");
        assert_eq!(format!("{}", x5.r#type), "logic<14>");
    }
}
