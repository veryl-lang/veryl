use crate::conv::Context;
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::{
    AssignDestination, Comptime, Expression, IrResult, Shape, Type, TypeKind, ValueVariant,
    VarPathSelect,
};
use crate::symbol::ClockDomain;
use crate::value::Value;
use crate::{AnalyzerError, BigUint, ir_error};
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug)]
pub struct Input(Expression);

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub struct Output(Vec<AssignDestination>);

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        if self.0.len() == 1 {
            ret.push_str(&format!("{}", self.0[0]));
        } else if !self.0.is_empty() {
            ret.push_str(&format!("{{{}", self.0[0]));
            for d in &self.0[1..] {
                ret.push_str(&format!(", {}", d));
            }
            ret.push_str("}}");
        }

        ret.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub struct SystemFunctionCall {
    kind: SystemFunctionKind,
    token: TokenRange,
}

#[derive(Clone, Debug)]
pub enum SystemFunctionKind {
    Bits(Input),
    Size(Input),
    Clog2(Input),
    Onehot(Input),
    Readmemh(Input, Output),
}

fn create_input(
    context: &mut Context,
    name: StrId,
    r#type: Option<Type>,
    arg: (Expression, Vec<VarPathSelect>, TokenRange),
) -> Input {
    let (mut expr, _, token) = arg;

    if let Some(r#type) = r#type {
        let comptime = expr.eval_comptime(context, None);
        if !r#type.compatible(&comptime) {
            context.insert_error(AnalyzerError::mismatch_function_arg(
                &name.to_string(),
                &comptime.r#type.to_string(),
                &token,
            ));
        }
    }

    Input(expr)
}

fn create_output(
    context: &mut Context,
    name: StrId,
    r#type: Option<Type>,
    arg: (Expression, Vec<VarPathSelect>, TokenRange),
) -> Output {
    let (mut expr, dst, token) = arg;

    let dst = dst
        .into_iter()
        .filter_map(|x| x.to_assign_destination(context, false))
        .collect();

    if let Some(r#type) = r#type {
        let comptime = expr.eval_comptime(context, None);
        if !r#type.compatible(&comptime) {
            context.insert_error(AnalyzerError::mismatch_function_arg(
                &name.to_string(),
                &comptime.r#type.to_string(),
                &token,
            ));
        }
    }

    Output(dst)
}

impl SystemFunctionCall {
    pub fn new(
        context: &mut Context,
        name: StrId,
        mut args: Vec<(Expression, Vec<VarPathSelect>, TokenRange)>,
        token: TokenRange,
    ) -> IrResult<Self> {
        match name.to_string().as_str() {
            "$bits" => {
                if args.len() != 1 {
                    // TODO mismatch arity
                    return Err(ir_error!(token));
                }
                let arg0 = create_input(context, name, None, args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Bits(arg0),
                    token,
                })
            }
            "$size" => {
                if args.len() != 1 {
                    // TODO mismatch arity
                    return Err(ir_error!(token));
                }
                let arg0 = create_input(context, name, None, args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Size(arg0),
                    token,
                })
            }
            "$clog2" => {
                if args.len() != 1 {
                    // TODO mismatch arity
                    return Err(ir_error!(token));
                }
                let arg0_type = Type {
                    kind: TypeKind::Logic,
                    width: Shape::new(vec![Some(32)]),
                    ..Default::default()
                };
                let arg0 = create_input(context, name, Some(arg0_type), args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Clog2(arg0),
                    token,
                })
            }
            "$onehot" => {
                if args.len() != 1 {
                    // TODO mismatch arity
                    return Err(ir_error!(token));
                }
                let arg0_type = Type {
                    kind: TypeKind::Logic,
                    width: Shape::new(vec![Some(32)]),
                    ..Default::default()
                };
                let arg0 = create_input(context, name, Some(arg0_type), args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Onehot(arg0),
                    token,
                })
            }
            "$readmemh" => {
                if args.len() != 2 {
                    // TODO mismatch arity
                    return Err(ir_error!(token));
                }
                let arg0 = create_input(context, name, None, args.remove(0));
                let arg1 = create_output(context, name, None, args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Readmemh(arg0, arg1),
                    token,
                })
            }
            _ => Err(ir_error!(token)),
        }
    }

    pub fn eval_value(&self, context: &mut Context) -> Option<Value> {
        match &self.kind {
            SystemFunctionKind::Bits(x) => {
                let mut expr = x.0.clone();
                let comptime = expr.eval_comptime(context, None);
                let value = match &comptime.value {
                    ValueVariant::Numeric(_) => comptime.r#type.total_width(),
                    ValueVariant::Type(x) => x.total_width(),
                    _ => None,
                };
                value.map(|x| Value::new(x.into(), 32, false))
            }
            SystemFunctionKind::Size(x) => {
                let mut expr = x.0.clone();
                let comptime = expr.eval_comptime(context, None);
                let value = match &comptime.value {
                    ValueVariant::Numeric(_) => comptime.r#type.total_width(),
                    ValueVariant::Type(x) => x.total_width(),
                    _ => None,
                };
                value.map(|x| Value::new(x.into(), 32, false))
            }
            SystemFunctionKind::Clog2(x) => {
                let value = x.0.eval_value(context, None)?;
                let ret = if value.payload == 0u32.into() {
                    BigUint::from(0u32)
                } else {
                    value.payload - BigUint::from(1u32)
                };
                Some(Value::new(ret.bits().into(), 32, false))
            }
            SystemFunctionKind::Onehot(x) => {
                let value = x.0.eval_value(context, None)?;
                let ret = value.payload.count_ones() == 1;
                Some(Value::new(ret.into(), 1, false))
            }
            SystemFunctionKind::Readmemh(_, _) => None,
        }
    }

    pub fn eval_comptime(&self, context: &mut Context) -> Comptime {
        let value = self.eval_value(context);
        match &self.kind {
            SystemFunctionKind::Bits(_) => {
                if let Some(x) = value {
                    Comptime::create_value(x.payload, 32, self.token)
                } else {
                    let mut ret = Comptime::create_unknown(ClockDomain::None, self.token);
                    ret.is_const = true;
                    ret
                }
            }
            SystemFunctionKind::Size(_) => {
                if let Some(x) = value {
                    Comptime::create_value(x.payload, 32, self.token)
                } else {
                    let mut ret = Comptime::create_unknown(ClockDomain::None, self.token);
                    ret.is_const = true;
                    ret
                }
            }
            SystemFunctionKind::Clog2(_) => {
                if let Some(x) = value {
                    Comptime::create_value(x.payload, 32, self.token)
                } else {
                    let mut ret = Comptime::create_unknown(ClockDomain::None, self.token);
                    ret.is_const = true;
                    ret
                }
            }
            SystemFunctionKind::Onehot(_) => {
                if let Some(x) = value {
                    Comptime::create_value(x.payload, 1, self.token)
                } else {
                    let mut ret = Comptime::create_unknown(ClockDomain::None, self.token);
                    ret.is_const = true;
                    ret
                }
            }
            SystemFunctionKind::Readmemh(_, _) => {
                Comptime::create_unknown(ClockDomain::None, self.token)
            }
        }
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        if let SystemFunctionKind::Readmemh(_, x) = &self.kind {
            for x in &x.0 {
                x.eval_assign(context, assign_table, assign_context);
            }
        }
    }
}

impl fmt::Display for SystemFunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            SystemFunctionKind::Bits(x) => format!("$bits({x})").fmt(f),
            SystemFunctionKind::Size(x) => format!("$size({x})").fmt(f),
            SystemFunctionKind::Clog2(x) => format!("$clog2({x})").fmt(f),
            SystemFunctionKind::Onehot(x) => format!("$onehot({x})").fmt(f),
            SystemFunctionKind::Readmemh(x, y) => format!("$readmemh({x}, {y})").fmt(f),
        }
    }
}
