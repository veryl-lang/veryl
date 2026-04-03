use crate::conv::Context;
use crate::conv::checker::clock_domain::check_clock_domain;
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::utils::convert_cast;
use crate::ir::{
    Comptime, ExpressionContext, FfTable, FunctionCall, Op, SystemFunctionCall, Type, ValueVariant,
    VarId, VarIndex, VarSelect,
};
use crate::value::{Value, ValueBigUint};
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug)]
pub enum Expression {
    Term(Box<Factor>),
    Unary(Op, Box<Expression>, Box<Comptime>),
    Binary(Box<Expression>, Op, Box<Expression>, Box<Comptime>),
    Ternary(
        Box<Expression>,
        Box<Expression>,
        Box<Expression>,
        Box<Comptime>,
    ),
    Concatenation(Vec<(Expression, Option<Expression>)>, Box<Comptime>),
    ArrayLiteral(Vec<ArrayLiteralItem>, Box<Comptime>),
    StructConstructor(Type, Vec<(StrId, Expression)>, Box<Comptime>),
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
            Expression::Unary(x, y, _) => {
                format!("({x} {y})")
            }
            Expression::Binary(x, y, z, _) => {
                format!("({x} {y} {z})")
            }
            Expression::Ternary(x, y, z, _) => {
                format!("({x} ? {y} : {z})")
            }
            Expression::Concatenation(x, _) => {
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
            Expression::ArrayLiteral(x, _) => {
                let mut ret = String::new();
                for x in x {
                    ret = format!("{ret}, {x}")
                }
                format!("'{{{}}}", &ret[2..])
            }
            Expression::StructConstructor(_, x, _) => {
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
            Expression::Concatenation(x, _) => {
                x.iter().all(|x| x.0.is_assignable() && x.1.is_none())
            }
            _ => false,
        }
    }

    pub fn gather_context(&mut self, context: &mut Context) -> ExpressionContext {
        match self {
            Expression::Term(x) => x.gather_context(context),
            Expression::Unary(op, x, comptime) => {
                let x = x.gather_context(context);

                comptime.is_const = x.is_const;
                comptime.is_global = x.is_global;

                op.eval_context_unary(x)
            }
            Expression::Binary(x, op, y, comptime) => {
                let (x, y) = if op.binary_op_self_determined() {
                    let expr_context = x.gather_context(context).merge(y.gather_context(context));
                    x.apply_context(context, expr_context);
                    y.apply_context(context, expr_context);
                    (expr_context, expr_context)
                } else {
                    let x = if op.binary_x_self_determined() {
                        let expr_context = x.gather_context(context);
                        x.apply_context(context, expr_context);
                        x.comptime().expr_context
                    } else {
                        x.gather_context(context)
                    };

                    let y = if op.binary_y_self_determined() {
                        let expr_context = y.gather_context(context);
                        y.apply_context(context, expr_context);
                        y.comptime().expr_context
                    } else {
                        y.gather_context(context)
                    };

                    (x, y)
                };

                comptime.is_const = x.is_const & y.is_const;
                comptime.is_global = x.is_global & y.is_global;

                op.eval_context_binary(x, y)
            }
            Expression::Ternary(x, y, z, comptime) => {
                // x is self-determined
                let expr_context = x.gather_context(context);
                x.apply_context(context, expr_context);
                let x = x.comptime();

                let y = y.gather_context(context);
                let z = z.gather_context(context);

                let is_const = x.is_const & y.is_const & z.is_const;
                let is_global = x.is_global & y.is_global & z.is_global;

                comptime.is_const = is_const;
                comptime.is_global = is_global;

                ExpressionContext {
                    width: y.width.max(z.width),
                    signed: y.signed & z.signed,
                    is_const,
                    is_global,
                }
            }
            Expression::Concatenation(x, comptime) => {
                let op = Op::Concatenation;
                op.eval_type_concatenation(context, x, comptime);
                comptime.evaluated = true;

                ExpressionContext {
                    width: comptime.r#type.total_width().unwrap_or(0),
                    signed: comptime.r#type.signed,
                    is_const: comptime.is_const,
                    is_global: comptime.is_global,
                }
            }
            Expression::StructConstructor(r#type, exprs, comptime) => {
                let mut is_const = true;
                let mut is_global = true;
                for (_, expr) in exprs {
                    let expr_context = expr.gather_context(context);
                    is_const &= expr_context.is_const;
                    is_global &= expr_context.is_global;

                    let expr = expr.comptime();
                    check_clock_domain(context, comptime, expr, &comptime.token.beg);
                    comptime.clock_domain = expr.clock_domain;
                }

                comptime.r#type = r#type.clone();
                comptime.is_const = is_const;
                comptime.is_global = is_global;
                comptime.evaluated = true;

                ExpressionContext {
                    width: r#type.total_width().unwrap_or(0),
                    signed: false,
                    is_const,
                    is_global,
                }
            }
            Expression::ArrayLiteral(items, comptime) => {
                let mut is_const = true;
                let mut is_global = true;
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(x, y) => {
                            let x_context = x.gather_context(context);
                            is_const &= x_context.is_const;
                            is_global &= x_context.is_global;
                            if let Some(y) = y {
                                let y_context = y.gather_context(context);
                                is_const &= y_context.is_const;
                                is_global &= y_context.is_global;
                            }
                        }
                        ArrayLiteralItem::Defaul(x) => {
                            let x_context = x.gather_context(context);
                            is_const &= x_context.is_const;
                            is_global &= x_context.is_global;
                        }
                    }
                }

                comptime.is_const = is_const;
                comptime.is_global = is_global;
                comptime.evaluated = true;

                // ArrayLiteral doesn't affect context width
                ExpressionContext {
                    width: 0,
                    signed: false,
                    is_const,
                    is_global,
                }
            }
        }
    }

    pub fn apply_context(&mut self, context: &mut Context, expr_context: ExpressionContext) {
        match self {
            Expression::Term(x) => x.apply_context(expr_context),
            Expression::Unary(op, x, comptime) => {
                x.apply_context(context, expr_context);
                comptime.expr_context = expr_context;
                op.eval_type_unary(context, x.comptime(), comptime);
                comptime.evaluated = true;
            }
            Expression::Binary(x, op, y, comptime) => {
                if !op.binary_op_self_determined() {
                    if !op.binary_x_self_determined() {
                        x.apply_context(context, expr_context);
                    }
                    if !op.binary_y_self_determined() {
                        y.apply_context(context, expr_context);
                    }
                }

                comptime.expr_context = expr_context;
                op.eval_type_binary(context, x.comptime(), y.comptime(), comptime);
                comptime.evaluated = true;
            }
            Expression::Ternary(x, y, z, comptime) => {
                y.apply_context(context, expr_context);
                z.apply_context(context, expr_context);

                comptime.expr_context = expr_context;
                let op = Op::Ternary;
                op.eval_type_ternary(context, x.comptime(), y.comptime(), z.comptime(), comptime);
                comptime.evaluated = true;
            }
            Expression::Concatenation(_, _) => (),
            Expression::StructConstructor(_, _, _) => (),
            Expression::ArrayLiteral(_, _) => (),
        }
    }

    pub fn eval_value(&self, context: &mut Context) -> Option<Value> {
        let context_width = self.comptime().expr_context.width;
        let signed = self.comptime().expr_context.signed;

        match self {
            Expression::Term(x) => x.eval_value(context),
            Expression::Unary(op, x, _) => {
                let ret = x.eval_value(context)?;
                let x_kind = &x.comptime().r#type.kind;
                if x_kind.is_float() {
                    op.eval_float_unary(&ret, x_kind)
                } else {
                    let ret =
                        op.eval_value_unary(&ret, context_width, signed, &mut context.mask_cache);
                    Some(ret)
                }
            }
            Expression::Binary(x, op, y, comptime) => {
                if op == &Op::As {
                    let src_kind = &x.comptime().r#type.kind;
                    let dst_kind = &comptime.r#type.kind;
                    let val = x.eval_value(context)?;
                    return Some(convert_cast(val, src_kind, dst_kind, context_width));
                }

                let x_kind = x.comptime().r#type.kind.clone();
                let x = x.eval_value(context)?;
                let y = y.eval_value(context)?;
                if x_kind.is_float() {
                    op.eval_float_binary(&x, &y, &x_kind)
                } else {
                    let ret = op.eval_value_binary(
                        &x,
                        &y,
                        context_width,
                        signed,
                        &mut context.mask_cache,
                    );
                    Some(ret)
                }
            }
            Expression::Ternary(x, y, z, _) => {
                let x = x.eval_value(context)?;
                let y = y.eval_value(context)?;
                let z = z.eval_value(context)?;

                let width = y.width().max(z.width());

                let ret = if x.to_usize().unwrap_or(0) == 0 { z } else { y };
                let ret = ret.expand(width, false).into_owned();
                Some(ret)
            }
            Expression::Concatenation(x, _) => {
                let mut ret = Value::new(0, 0, false);
                for (exp, rep) in x.iter() {
                    let exp = exp.eval_value(context)?;

                    let rep = if let Some(rep) = rep {
                        let token = rep.token_range();
                        let rep = rep.eval_value(context)?;
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
            Expression::StructConstructor(r#type, exprs, _) => {
                let mut ret = Value::new(0u32.into(), 0, false);
                for (name, expr) in exprs {
                    let sub_type = r#type.get_member_type(*name)?;
                    let width = sub_type.total_width()?;
                    let mut value = expr.eval_value(context)?;
                    value.trunc(width);
                    ret = ret.concat(&value);
                }
                Some(ret)
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_, _) => None,
        }
    }

    pub fn eval_comptime(
        &mut self,
        context: &mut Context,
        context_width: Option<usize>,
    ) -> &Comptime {
        if !self.comptime().evaluated {
            let mut expr_context = self.gather_context(context);
            expr_context.width = expr_context.width.max(context_width.unwrap_or(0));
            self.apply_context(context, expr_context);

            let value = self.eval_value(context);
            let comptime = self.comptime_mut();
            if comptime.value.is_unknown()
                && let Some(x) = &value
            {
                comptime.value = ValueVariant::Numeric(x.clone());
            }

            // const optimization
            if comptime.is_const
                && let Some(value) = value
                && !value.is_xz()
            {
                let is_global = comptime.is_global;
                let r#type = comptime.r#type.clone();
                let mut expr = Expression::create_value(value.clone(), self.token_range());

                let expr_comptime = expr.comptime_mut();
                expr_comptime.is_global = is_global;
                expr_comptime.r#type = r#type;
                expr_comptime.evaluated = true;

                *self = expr;
            }
        }

        self.comptime()
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        match self {
            Expression::Term(x) => x.eval_assign(context, assign_table, assign_context),
            Expression::Unary(_, x, _) => x.eval_assign(context, assign_table, assign_context),
            Expression::Binary(x, _, y, _) => {
                x.eval_assign(context, assign_table, assign_context);
                y.eval_assign(context, assign_table, assign_context);
            }
            Expression::Ternary(x, y, z, _) => {
                x.eval_assign(context, assign_table, assign_context);
                y.eval_assign(context, assign_table, assign_context);
                z.eval_assign(context, assign_table, assign_context);
            }
            Expression::Concatenation(x, _) => {
                for (x, y) in x {
                    x.eval_assign(context, assign_table, assign_context);
                    if let Some(y) = y {
                        y.eval_assign(context, assign_table, assign_context);
                    }
                }
            }
            Expression::StructConstructor(_, exprs, _) => {
                for (_, expr) in exprs {
                    expr.eval_assign(context, assign_table, assign_context);
                }
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_, _) => (),
        }
    }

    pub fn gather_ff(
        &self,
        context: &mut Context,
        table: &mut FfTable,
        decl: usize,
        assign_target: Option<(VarId, Option<usize>)>,
    ) {
        match self {
            Expression::Term(x) => x.gather_ff(context, table, decl, assign_target),
            Expression::Unary(_, x, _) => x.gather_ff(context, table, decl, assign_target),
            Expression::Binary(x, _, y, _) => {
                x.gather_ff(context, table, decl, assign_target);
                y.gather_ff(context, table, decl, assign_target);
            }
            Expression::Ternary(x, y, z, _) => {
                x.gather_ff(context, table, decl, assign_target);
                y.gather_ff(context, table, decl, assign_target);
                z.gather_ff(context, table, decl, assign_target);
            }
            Expression::Concatenation(x, _) => {
                for (x, y) in x {
                    x.gather_ff(context, table, decl, assign_target);
                    if let Some(y) = y {
                        y.gather_ff(context, table, decl, assign_target);
                    }
                }
            }
            Expression::StructConstructor(_, exprs, _) => {
                for (_, expr) in exprs {
                    expr.gather_ff(context, table, decl, assign_target);
                }
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_, _) => (),
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        match self {
            Expression::Term(x) => x.set_index(index),
            Expression::Unary(_, x, _) => x.set_index(index),
            Expression::Binary(x, _, y, _) => {
                x.set_index(index);
                y.set_index(index);
            }
            Expression::Ternary(x, y, z, _) => {
                x.set_index(index);
                y.set_index(index);
                z.set_index(index);
            }
            Expression::Concatenation(x, _) => {
                for (x, y) in x {
                    x.set_index(index);
                    if let Some(y) = y {
                        y.set_index(index);
                    }
                }
            }
            Expression::StructConstructor(_, exprs, _) => {
                for (_, expr) in exprs {
                    expr.set_index(index);
                }
            }
            // ArrayLiteral doesn't require evaluation because it is expanded in conv phase
            Expression::ArrayLiteral(_, _) => (),
        }
    }

    pub fn comptime(&self) -> &Comptime {
        match self {
            Expression::Term(x) => x.comptime(),
            Expression::Unary(_, _, x) => x,
            Expression::Binary(_, _, _, x) => x,
            Expression::Ternary(_, _, _, x) => x,
            Expression::Concatenation(_, x) => x,
            Expression::ArrayLiteral(_, x) => x,
            Expression::StructConstructor(_, _, x) => x,
        }
    }

    pub fn comptime_mut(&mut self) -> &mut Comptime {
        match self {
            Expression::Term(x) => x.comptime_mut(),
            Expression::Unary(_, _, x) => x,
            Expression::Binary(_, _, _, x) => x,
            Expression::Ternary(_, _, _, x) => x,
            Expression::Concatenation(_, x) => x,
            Expression::ArrayLiteral(_, x) => x,
            Expression::StructConstructor(_, _, x) => x,
        }
    }

    pub fn token_range(&self) -> TokenRange {
        match self {
            Expression::Term(x) => x.token_range(),
            Expression::Unary(_, _, x) => x.token,
            Expression::Binary(_, _, _, x) => x.token,
            Expression::Ternary(_, _, _, x) => x.token,
            Expression::Concatenation(_, x) => x.token,
            Expression::ArrayLiteral(_, x) => x.token,
            Expression::StructConstructor(_, _, x) => x.token,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Factor {
    Variable(VarId, VarIndex, VarSelect, Comptime),
    Value(Comptime),
    SystemFunctionCall(SystemFunctionCall),
    FunctionCall(FunctionCall),
    Anonymous(Comptime),
    Unknown(Comptime),
}

impl Factor {
    pub fn create_value(value: Value, token: TokenRange) -> Self {
        let comptime = Comptime::create_value(value, token);
        Factor::Value(comptime)
    }

    pub fn is_assignable(&self) -> bool {
        match self {
            // SystemVerilog member is interpreted as Factor::Value, but it may be assignable.
            Factor::Value(x) => x.r#type.is_systemverilog(),
            Factor::FunctionCall(_) | Factor::SystemFunctionCall(_) => false,
            _ => true,
        }
    }

    pub fn gather_context(&mut self, context: &mut Context) -> ExpressionContext {
        match self {
            Factor::Variable(_, _index, select, comptime) => {
                // Array dimensions are already drained at Factor construction time
                // (in VarPathSelect::to_expression and eval_factor).

                // Struct/Union/Enum should be treated as flatten bit/logic when it is bit-selected
                if !select.is_empty() {
                    comptime.r#type.flatten_struct_union_enum()
                }

                if let Some(width) = select.eval_comptime(context, &comptime.r#type, false) {
                    comptime.r#type.width = width;
                }

                ExpressionContext {
                    width: comptime.r#type.total_width().unwrap_or(0),
                    signed: comptime.r#type.signed,
                    is_const: comptime.is_const,
                    is_global: comptime.is_global,
                }
            }
            Factor::Value(x) => ExpressionContext {
                width: x.r#type.total_width().unwrap_or(0),
                signed: x.r#type.signed,
                is_const: x.is_const,
                is_global: x.is_global,
            },
            Factor::FunctionCall(x) => {
                x.eval_type(context);
                ExpressionContext {
                    width: x.comptime.r#type.total_width().unwrap_or(0),
                    signed: x.comptime.r#type.signed,
                    is_const: x.comptime.is_const,
                    is_global: x.comptime.is_global,
                }
            }
            Factor::SystemFunctionCall(x) => ExpressionContext {
                width: x.comptime.r#type.total_width().unwrap_or(0),
                signed: x.comptime.expr_context.signed,
                is_const: x.comptime.is_const,
                is_global: x.comptime.is_global,
            },
            Factor::Anonymous(_) | Factor::Unknown(_) => ExpressionContext::default(),
        }
    }

    pub fn apply_context(&mut self, expr_context: ExpressionContext) {
        match self {
            Factor::Variable(_, _, _, x) => {
                x.expr_context = expr_context;
                x.evaluated = true;
            }
            Factor::Value(x) => {
                x.expr_context = expr_context;
                x.evaluated = true;
            }
            Factor::SystemFunctionCall(x) => {
                x.comptime.expr_context = expr_context;
                x.comptime.evaluated = true;
            }
            Factor::FunctionCall(x) => {
                x.comptime.expr_context = expr_context;
                x.comptime.evaluated = true;
            }
            Factor::Anonymous(x) => {
                x.expr_context = expr_context;
                x.evaluated = true
            }
            Factor::Unknown(x) => {
                x.expr_context = expr_context;
                x.evaluated = true;
            }
        }
    }

    pub fn eval_value(&self, context: &mut Context) -> Option<Value> {
        match self {
            Factor::Variable(id, index, select, comptime) => {
                let index = index.eval_value(context)?;
                let value = context.variables.get(id)?.get_value(&index)?.clone();

                if !select.is_empty() {
                    let (beg, end) = select.eval_value(context, &comptime.r#type, false)?;
                    Some(value.select(beg, end))
                } else {
                    Some(value)
                }
            }
            Factor::Value(x) => x.get_value().ok().cloned(),
            Factor::SystemFunctionCall(x) => x.eval_value(context),
            Factor::FunctionCall(x) => x.eval_value(context),
            Factor::Anonymous(_) => None,
            Factor::Unknown(_) => None,
        }
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        match self {
            Factor::Variable(id, index, select, _) => {
                if let Some(index) = index.eval_value(context)
                    && let Some(variable) = context.variables.get(id).cloned()
                    && let Some((beg, end)) = select.eval_value(context, &variable.r#type, false)
                {
                    let mask = ValueBigUint::gen_mask_range(beg, end);
                    assign_table.insert_reference(&variable, index, mask);
                }
            }
            Factor::FunctionCall(x) => {
                x.eval_assign(context, assign_table, assign_context);
            }
            Factor::SystemFunctionCall(x) => {
                x.eval_assign(context, assign_table, assign_context);
            }
            _ => (),
        }
    }

    pub fn gather_ff(
        &self,
        context: &mut Context,
        table: &mut FfTable,
        decl: usize,
        assign_target: Option<(VarId, Option<usize>)>,
    ) {
        match self {
            Factor::Variable(id, index, _, _) => {
                if let Some(variable) = context.get_variable_info(*id) {
                    if let Some(index) = index.eval_value(context) {
                        if let Some(index) = variable.r#type.array.calc_index(&index) {
                            table.insert_refered(*id, index, decl, assign_target);
                        }
                    } else if let Some(total_array) = variable.r#type.total_array() {
                        for i in 0..total_array {
                            table.insert_refered(*id, i, decl, assign_target);
                        }
                    }
                }
            }
            Factor::FunctionCall(x) => {
                x.gather_ff(context, table, decl, assign_target);
            }
            _ => (),
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        match self {
            Factor::Variable(_, i, _, _) => {
                *i = index.clone();
            }
            Factor::FunctionCall(x) => {
                x.set_index(index);
            }
            _ => (),
        }
    }

    pub fn comptime(&self) -> &Comptime {
        match self {
            Factor::Variable(_, _, _, x) => x,
            Factor::Value(x) => x,
            Factor::SystemFunctionCall(x) => &x.comptime,
            Factor::FunctionCall(x) => &x.comptime,
            Factor::Anonymous(x) => x,
            Factor::Unknown(x) => x,
        }
    }

    pub fn comptime_mut(&mut self) -> &mut Comptime {
        match self {
            Factor::Variable(_, _, _, x) => x,
            Factor::Value(x) => x,
            Factor::SystemFunctionCall(x) => &mut x.comptime,
            Factor::FunctionCall(x) => &mut x.comptime,
            Factor::Anonymous(x) => x,
            Factor::Unknown(x) => x,
        }
    }

    pub fn token_range(&self) -> TokenRange {
        match self {
            Factor::Variable(_, _, _, x) => x.token,
            Factor::Value(x) => x.token,
            Factor::SystemFunctionCall(x) => x.comptime.token,
            Factor::FunctionCall(x) => x.comptime.token,
            Factor::Anonymous(x) => x.token,
            Factor::Unknown(x) => x.token,
        }
    }
}

impl fmt::Display for Factor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = match self {
            Factor::Variable(id, index, select, _) => {
                format!("{id}{index}{select}")
            }
            Factor::Value(x) => {
                if let Ok(x) = x.get_value() {
                    format!("{:x}", x)
                } else {
                    String::from("unknown")
                }
            }
            Factor::SystemFunctionCall(x) => x.to_string(),
            Factor::FunctionCall(x) => x.to_string(),
            Factor::Anonymous(_) => String::from("_"),
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

    pub fn is_const(&self) -> bool {
        match self {
            ArrayLiteralItem::Value(x, y) => {
                let mut ret = x.comptime().is_const;
                if let Some(y) = y {
                    ret &= y.comptime().is_const;
                }
                ret
            }
            ArrayLiteralItem::Defaul(x) => x.comptime().is_const,
        }
    }

    pub fn is_global(&self) -> bool {
        match self {
            ArrayLiteralItem::Value(x, y) => {
                let mut ret = x.comptime().is_global;
                if let Some(y) = y {
                    ret &= y.comptime().is_global;
                }
                ret
            }
            ArrayLiteralItem::Defaul(x) => x.comptime().is_global,
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
    use crate::ir::{Shape, TypeKind};

    fn calc_expression(s: &str, context_width: Option<usize>) -> Value {
        let mut context = Context::default();
        let x = parse_expression(s);
        let mut x: Expression = Conv::conv(&mut context, &x).unwrap();
        x.eval_comptime(&mut context, context_width)
            .get_value()
            .unwrap()
            .clone()
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

    #[test]
    fn context_width() {
        let x0 = calc_expression("(2'd0 - 2'd1) == 3'd7", None);

        assert_eq!(format!("{:x}", x0), "1'h1");
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
        Box::new(Expression::Term(Box::new(Factor::Value(ret))))
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
        Box::new(Expression::Term(Box::new(Factor::Value(ret))))
    }

    fn signed_bit(width: usize) -> Box<Expression> {
        let ret = Comptime {
            value: ValueVariant::Unknown,
            r#type: Type {
                kind: TypeKind::Bit,
                width: Shape::new(vec![Some(width)]),
                signed: true,
                ..Default::default()
            },
            ..Default::default()
        };
        Box::new(Expression::Term(Box::new(Factor::Value(ret))))
    }

    fn signed_logic(width: usize) -> Box<Expression> {
        let ret = Comptime {
            value: ValueVariant::Unknown,
            r#type: Type {
                kind: TypeKind::Logic,
                width: Shape::new(vec![Some(width)]),
                signed: true,
                ..Default::default()
            },
            ..Default::default()
        };
        Box::new(Expression::Term(Box::new(Factor::Value(ret))))
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
        Box::new(Expression::Term(Box::new(Factor::Value(ret))))
    }

    fn eval_comptime_unary(context: &mut Context, op: Op, x: Box<Expression>) -> Comptime {
        let comptime = Box::new(Comptime::create_unknown(TokenRange::default()));
        let mut ret = Expression::Unary(op, x, comptime);
        ret.eval_comptime(context, None).clone()
    }

    fn eval_comptime_binary(
        context: &mut Context,
        x: Box<Expression>,
        op: Op,
        y: Box<Expression>,
    ) -> Comptime {
        let comptime = Box::new(Comptime::create_unknown(TokenRange::default()));
        let mut ret = Expression::Binary(x, op, y, comptime);
        ret.eval_comptime(context, None).clone()
    }

    fn eval_comptime_ternary(
        context: &mut Context,
        x: Box<Expression>,
        y: Box<Expression>,
        z: Box<Expression>,
    ) -> Comptime {
        let comptime = Box::new(Comptime::create_unknown(TokenRange::default()));
        let mut ret = Expression::Ternary(x, y, z, comptime);
        ret.eval_comptime(context, None).clone()
    }

    fn eval_comptime_concat(
        context: &mut Context,
        x: Vec<(Expression, Option<Expression>)>,
    ) -> Comptime {
        let comptime = Box::new(Comptime::create_unknown(TokenRange::default()));
        let mut ret = Expression::Concatenation(x, comptime);
        ret.eval_comptime(context, None).clone()
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
        let x09 = eval_comptime_binary(&mut context, signed_bit(4), Op::ArithShiftL, bit(3));
        let x10 = eval_comptime_binary(&mut context, signed_bit(4), Op::ArithShiftL, logic(3));
        let x11 = eval_comptime_binary(&mut context, signed_logic(4), Op::ArithShiftL, logic(3));

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
        assert_eq!(format!("{}", x09.r#type), "signed bit<4>");
        assert_eq!(format!("{}", x10.r#type), "signed bit<4>");
        assert_eq!(format!("{}", x11.r#type), "signed logic<4>");
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
