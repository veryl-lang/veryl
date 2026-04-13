use crate::conv::Context;
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::ff_table::FfTable;
use crate::ir::{
    AssignDestination, Comptime, Expression, IrResult, Shape, Type, TypeKind, ValueVariant,
    VarPathSelect,
};
use crate::value::Value;
use crate::{AnalyzerError, BigUint, ir_error};
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug)]
pub struct Input(pub Expression);

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub struct Output(pub Vec<AssignDestination>);

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
    pub kind: SystemFunctionKind,
    pub comptime: Comptime,
}

#[derive(Clone, Debug)]
pub enum SystemFunctionKind {
    Bits(Input),
    Size(Input),
    Clog2(Input),
    Onehot(Input),
    Readmemh(Input, Output),
    Display(Vec<Input>),
    Write(Vec<Input>),
    Assert(Input, Option<Input>),
    Finish,
    Signed(Input),
    Unsigned(Input),
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
        if !r#type.compatible(comptime) {
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
        if !r#type.compatible(comptime) {
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
        let mut comptime = Comptime::create_unknown(token);
        comptime.is_const = true;

        match name.to_string().as_str() {
            "$bits" => {
                if args.len() != 1 {
                    context.insert_error(AnalyzerError::mismatch_function_arity(
                        "$bits",
                        1,
                        args.len(),
                        &token,
                    ));
                    return Err(ir_error!(token));
                }
                let arg0 = create_input(context, name, None, args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Bits(arg0),
                    comptime,
                })
            }
            "$size" => {
                if args.len() != 1 {
                    context.insert_error(AnalyzerError::mismatch_function_arity(
                        "$size",
                        1,
                        args.len(),
                        &token,
                    ));
                    return Err(ir_error!(token));
                }
                let arg0 = create_input(context, name, None, args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Size(arg0),
                    comptime,
                })
            }
            "$clog2" => {
                if args.len() != 1 {
                    context.insert_error(AnalyzerError::mismatch_function_arity(
                        "$clog2",
                        1,
                        args.len(),
                        &token,
                    ));
                    return Err(ir_error!(token));
                }
                let arg0_type = {
                    let mut t = Type::new(TypeKind::Logic);
                    t.set_concrete_width(Shape::new(vec![Some(32)]));
                    t
                };
                let arg0 = create_input(context, name, Some(arg0_type), args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Clog2(arg0),
                    comptime,
                })
            }
            "$onehot" => {
                if args.len() != 1 {
                    context.insert_error(AnalyzerError::mismatch_function_arity(
                        "$onehot",
                        1,
                        args.len(),
                        &token,
                    ));
                    return Err(ir_error!(token));
                }
                let arg0_type = {
                    let mut t = Type::new(TypeKind::Logic);
                    t.set_concrete_width(Shape::new(vec![Some(32)]));
                    t
                };
                let arg0 = create_input(context, name, Some(arg0_type), args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Onehot(arg0),
                    comptime,
                })
            }
            "$readmemh" => {
                if args.len() != 2 {
                    context.insert_error(AnalyzerError::mismatch_function_arity(
                        "$readmemh",
                        2,
                        args.len(),
                        &token,
                    ));
                    return Err(ir_error!(token));
                }
                let arg0 = create_input(context, name, None, args.remove(0));
                let arg1 = create_output(context, name, None, args.remove(0));
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Readmemh(arg0, arg1),
                    comptime,
                })
            }
            "$display" => {
                let inputs: Vec<Input> = args
                    .into_iter()
                    .map(|arg| create_input(context, name, None, arg))
                    .collect();
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Display(inputs),
                    comptime,
                })
            }
            "$write" => {
                let inputs: Vec<Input> = args
                    .into_iter()
                    .map(|arg| create_input(context, name, None, arg))
                    .collect();
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Write(inputs),
                    comptime,
                })
            }
            "$assert" => {
                if args.is_empty() || args.len() > 2 {
                    return Err(ir_error!(token));
                }
                let arg0 = create_input(context, name, None, args.remove(0));
                let arg1 = if !args.is_empty() {
                    Some(create_input(context, name, None, args.remove(0)))
                } else {
                    None
                };
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Assert(arg0, arg1),
                    comptime,
                })
            }
            "$finish" => {
                if !args.is_empty() {
                    return Err(ir_error!(token));
                }
                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Finish,
                    comptime,
                })
            }
            "$signed" => {
                if args.len() != 1 {
                    return Err(ir_error!(token));
                }

                let mut arg = args.remove(0);
                arg.0.eval_comptime(context, None);

                let arg0 = create_input(context, name, None, arg);
                comptime.expr_context.signed = true;

                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Signed(arg0),
                    comptime,
                })
            }
            "$unsigned" => {
                if args.len() != 1 {
                    return Err(ir_error!(token));
                }

                let mut arg = args.remove(0);
                arg.0.eval_comptime(context, None);

                let arg0 = create_input(context, name, None, arg);
                comptime.expr_context.signed = false;

                Ok(SystemFunctionCall {
                    kind: SystemFunctionKind::Unsigned(arg0),
                    comptime,
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
                value.map(|x| Value::new(x as u64, 32, false))
            }
            SystemFunctionKind::Size(x) => {
                let mut expr = x.0.clone();
                let comptime = expr.eval_comptime(context, None);
                let value = match &comptime.value {
                    ValueVariant::Numeric(_) => comptime.r#type.total_width(),
                    ValueVariant::Type(x) => x.total_width(),
                    _ => None,
                };
                value.map(|x| Value::new(x as u64, 32, false))
            }
            SystemFunctionKind::Clog2(x) => {
                let value = x.0.eval_value(context)?;
                let value = value.payload();
                let ret = if value.as_ref() == &BigUint::from(0u32) {
                    BigUint::from(0u32)
                } else {
                    value.as_ref() - BigUint::from(1u32)
                };
                Some(Value::new(ret.bits(), 32, false))
            }
            SystemFunctionKind::Onehot(x) => {
                let value = x.0.eval_value(context)?;
                let ret = value.payload().count_ones() == 1;
                Some(Value::new(ret.into(), 1, false))
            }
            SystemFunctionKind::Readmemh(_, _) => None,
            SystemFunctionKind::Display(_) => None,
            SystemFunctionKind::Write(_) => None,
            SystemFunctionKind::Assert(_, _) => None,
            SystemFunctionKind::Finish => None,
            SystemFunctionKind::Signed(x) | SystemFunctionKind::Unsigned(x) => {
                x.0.eval_value(context)
            }
        }
    }

    pub fn eval_comptime(&self, context: &mut Context) -> Comptime {
        let value = self.eval_value(context);
        match &self.kind {
            SystemFunctionKind::Bits(_)
            | SystemFunctionKind::Size(_)
            | SystemFunctionKind::Clog2(_)
            | SystemFunctionKind::Onehot(_) => {
                let mut ret = self.comptime.clone();
                if let Some(x) = value {
                    ret.value = ValueVariant::Numeric(x);
                }
                ret
            }
            SystemFunctionKind::Readmemh(_, _) => self.comptime.clone(),
            SystemFunctionKind::Display(_) => self.comptime.clone(),
            SystemFunctionKind::Write(_) => self.comptime.clone(),
            SystemFunctionKind::Assert(_, _) => self.comptime.clone(),
            SystemFunctionKind::Finish => self.comptime.clone(),
            SystemFunctionKind::Signed(_) | SystemFunctionKind::Unsigned(_) => {
                let mut ret = self.comptime.clone();
                if let Some(x) = value {
                    ret.value = ValueVariant::Numeric(x);
                }
                ret
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
            SystemFunctionKind::Display(args) => {
                let args_str: Vec<_> = args.iter().map(|a| format!("{a}")).collect();
                format!("$display({})", args_str.join(", ")).fmt(f)
            }
            SystemFunctionKind::Write(args) => {
                let args_str: Vec<_> = args.iter().map(|a| format!("{a}")).collect();
                format!("$write({})", args_str.join(", ")).fmt(f)
            }
            SystemFunctionKind::Assert(cond, msg) => {
                if let Some(msg) = msg {
                    format!("$assert({cond}, {msg})").fmt(f)
                } else {
                    format!("$assert({cond})").fmt(f)
                }
            }
            SystemFunctionKind::Finish => "$finish()".fmt(f),
            SystemFunctionKind::Signed(x) => format!("$signed({x})").fmt(f),
            SystemFunctionKind::Unsigned(x) => format!("$unsigned({x})").fmt(f),
        }
    }
}

impl SystemFunctionCall {
    pub fn gather_ff(
        &self,
        context: &mut Context,
        table: &mut FfTable,
        decl: usize,
        from_ff: bool,
    ) {
        let process_input = |input: &Input, context: &mut Context, table: &mut FfTable| {
            input.0.gather_ff(context, table, decl, None, from_ff);
        };
        match &self.kind {
            SystemFunctionKind::Bits(x)
            | SystemFunctionKind::Size(x)
            | SystemFunctionKind::Clog2(x)
            | SystemFunctionKind::Onehot(x)
            | SystemFunctionKind::Signed(x)
            | SystemFunctionKind::Unsigned(x) => {
                process_input(x, context, table);
            }
            SystemFunctionKind::Readmemh(_, _) => {
                // First arg is filename literal, second is output array
                // (written to, so handled via assignment, not reference).
            }
            SystemFunctionKind::Display(args) | SystemFunctionKind::Write(args) => {
                for arg in args {
                    process_input(arg, context, table);
                }
            }
            SystemFunctionKind::Assert(cond, msg) => {
                process_input(cond, context, table);
                if let Some(m) = msg {
                    process_input(m, context, table);
                }
            }
            SystemFunctionKind::Finish => {}
        }
    }
}
