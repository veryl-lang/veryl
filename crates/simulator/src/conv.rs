use crate::HashMap;
use crate::cranelift::{self, FuncPtr};
use crate::ir as sir;
use crate::ir::{Clock, Reset, Value, VarId, Variable};
use memmap2::Mmap;
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;

pub fn build_ir(ir: air::Ir, top: StrId) -> Option<sir::Ir> {
    for x in &ir.components {
        if let air::Component::Module(x) = x
            && top == x.name
        {
            let ret = Conv::conv(x);
            return ret;
        }
    }
    None
}

#[derive(Default)]
pub struct Context {
    pub variables: HashMap<VarId, Variable>,
    pub values: Vec<Value>,
    pub binary: Vec<Mmap>,
    pub declaration_values: HashMap<*mut Value, usize>,
    pub declaration_value_index: usize,
    pub func_ptr_map: HashMap<Vec<sir::StatementProto>, FuncPtr>,
    pub analyzer_context: veryl_analyzer::conv::Context,
}

pub trait Conv<T>: Sized {
    fn conv(src: T) -> Option<Self>;
}

pub trait ConvContext<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Option<Self>;
}

impl Conv<&air::Variable> for sir::Variable {
    fn conv(src: &air::Variable) -> Option<Self> {
        Some(sir::Variable {
            path: src.path.clone(),
            width: src.r#type.total_width()?,
            //value: value.value.clone(),
            value: src.value.iter().map(|x| x.to_usize()).collect(),
        })
    }
}

impl ConvContext<&air::Declaration> for sir::Declaration {
    fn conv(context: &mut Context, src: &air::Declaration) -> Option<Self> {
        context.declaration_values.clear();
        context.declaration_value_index = 0;

        match src {
            air::Declaration::Comb(x) => {
                let clock = None;
                let reset = None;

                let mut statements = vec![];
                let mut statements_proto = vec![];
                for x in &x.statements {
                    let (x, x_proto) = ConvContext::conv(context, x)?;
                    statements.push(x);
                    statements_proto.push(x_proto);
                }

                let binary = cranelift::build_binary(context, statements_proto);

                Some(sir::Declaration {
                    clock,
                    reset,
                    statements,
                    binary,
                })
            }
            air::Declaration::Ff(x) => {
                let clock: Option<Clock> = Some(Conv::conv(&x.clock)?);

                let reset: Option<Reset> = if let Some(x) = &x.reset {
                    Some(Conv::conv(x)?)
                } else {
                    None
                };

                let mut statements = vec![];
                let mut statements_proto = vec![];
                for x in &x.statements {
                    let (x, x_proto) = ConvContext::conv(context, x)?;
                    statements.push(x);
                    statements_proto.push(x_proto);
                }

                let binary = cranelift::build_binary(context, statements_proto);

                Some(sir::Declaration {
                    clock: clock.map(|x| x.id),
                    reset: reset.map(|x| x.id),
                    statements,
                    binary,
                })
            }
            _ => None,
        }
    }
}

impl Conv<&air::FfClock> for sir::Clock {
    fn conv(src: &air::FfClock) -> Option<Self> {
        Some(sir::Clock {
            id: src.id,
            index: None,
            select: None,
        })
    }
}

impl Conv<&air::FfReset> for sir::Reset {
    fn conv(src: &air::FfReset) -> Option<Self> {
        Some(sir::Reset {
            id: src.id,
            index: None,
            select: None,
        })
    }
}

impl ConvContext<&air::Statement> for (sir::Statement, sir::StatementProto) {
    fn conv(context: &mut Context, src: &air::Statement) -> Option<Self> {
        match src {
            air::Statement::Assign(x) => {
                let (x, x_proto) = ConvContext::conv(context, x)?;
                Some((
                    sir::Statement::Assign(x),
                    sir::StatementProto::Assign(x_proto),
                ))
            }
            air::Statement::If(x) => {
                let (x, x_proto) = ConvContext::conv(context, x)?;
                Some((sir::Statement::If(x), sir::StatementProto::If(x_proto)))
            }
            air::Statement::IfReset(x) => {
                let (x, x_proto) = ConvContext::conv(context, x)?;
                Some((sir::Statement::If(x), sir::StatementProto::If(x_proto)))
            }
            _ => None,
        }
    }
}

impl Conv<&air::AssignDestination> for sir::AssignDestination {
    fn conv(src: &air::AssignDestination) -> Option<Self> {
        Some(sir::AssignDestination {
            id: src.id,
            index: None,
            select: None,
        })
    }
}

impl ConvContext<&air::AssignStatement> for (sir::AssignStatement, sir::AssignStatementProto) {
    fn conv(context: &mut Context, src: &air::AssignStatement) -> Option<Self> {
        let dst = &src.dst[0];

        let index = dst.index.eval_value(&mut context.analyzer_context)?;
        let dst = context.analyzer_context.variables.get_mut(&dst.id).unwrap();
        let index = dst.r#type.array.calc_index(&index)?;

        let dst = context.variables.get_mut(&dst.id).unwrap();
        let dst = &raw mut dst.value[index];
        let (expr, expr_proto) = ConvContext::conv(context, &src.expr)?;

        let dst_index = if let Some(x) = context.declaration_values.get(&dst) {
            *x
        } else {
            let index = context.declaration_value_index;
            context.declaration_value_index += 1;
            context.declaration_values.insert(dst, index);
            index
        };

        let body = sir::AssignStatement { dst, expr };
        let proto = sir::AssignStatementProto {
            dst: dst_index,
            expr: expr_proto,
        };

        Some((body, proto))
    }
}

impl ConvContext<&air::IfStatement> for (sir::IfStatement, sir::IfStatementProto) {
    fn conv(context: &mut Context, src: &air::IfStatement) -> Option<Self> {
        let (cond, cond_proto) = ConvContext::conv(context, &src.cond)?;

        let mut true_side = vec![];
        let mut true_side_proto = vec![];
        for x in &src.true_side {
            let (x, x_proto) = ConvContext::conv(context, x)?;
            true_side.push(x);
            true_side_proto.push(x_proto);
        }

        let mut false_side = vec![];
        let mut false_side_proto = vec![];
        for x in &src.false_side {
            let (x, x_proto) = ConvContext::conv(context, x)?;
            false_side.push(x);
            false_side_proto.push(x_proto);
        }

        let body = sir::IfStatement {
            cond: Some(cond),
            true_side,
            false_side,
        };
        let proto = sir::IfStatementProto {
            cond: Some(cond_proto),
            true_side: true_side_proto,
            false_side: false_side_proto,
        };

        Some((body, proto))
    }
}

impl ConvContext<&air::IfResetStatement> for (sir::IfStatement, sir::IfStatementProto) {
    fn conv(context: &mut Context, src: &air::IfResetStatement) -> Option<Self> {
        let mut true_side = vec![];
        let mut true_side_proto = vec![];
        for x in &src.true_side {
            let (x, x_proto) = ConvContext::conv(context, x)?;
            true_side.push(x);
            true_side_proto.push(x_proto);
        }

        let mut false_side = vec![];
        let mut false_side_proto = vec![];
        for x in &src.false_side {
            let (x, x_proto) = ConvContext::conv(context, x)?;
            false_side.push(x);
            false_side_proto.push(x_proto);
        }

        let body = sir::IfStatement {
            cond: None,
            true_side,
            false_side,
        };
        let proto = sir::IfStatementProto {
            cond: None,
            true_side: true_side_proto,
            false_side: false_side_proto,
        };

        Some((body, proto))
    }
}

impl ConvContext<&air::Expression> for (sir::Expression, sir::ExpressionProto) {
    fn conv(context: &mut Context, src: &air::Expression) -> Option<Self> {
        match src {
            air::Expression::Term(x) => match x.as_ref() {
                air::Factor::Variable(id, index, _, _, _) => {
                    let index = index.eval_value(&mut context.analyzer_context)?;
                    let variable = context.analyzer_context.variables.get_mut(id).unwrap();
                    let index = variable.r#type.array.calc_index(&index)?;

                    let variable = context.variables.get_mut(id).unwrap();
                    let ptr = &raw mut variable.value[index];

                    let index = if let Some(x) = context.declaration_values.get(&ptr) {
                        *x
                    } else {
                        let index = context.declaration_value_index;
                        context.declaration_value_index += 1;
                        context.declaration_values.insert(ptr, index);
                        index
                    };

                    let body = sir::Expression::Value(ptr);
                    let proto = sir::ExpressionProto::Value(index);
                    Some((body, proto))
                }
                air::Factor::Value(comptime, _) => {
                    let value = comptime.get_value().ok().cloned().map(|x| x.to_usize())?;
                    context.values.push(value);
                    let ptr = &raw const context.values[context.values.len() - 1];

                    let body = sir::Expression::Value(ptr);
                    let proto = sir::ExpressionProto::Immediate(value as u64);
                    Some((body, proto))
                }
                _ => None,
            },
            //air::Expression::Unary(x, y) => Some(sir::Expression::Unary(
            //    *x,
            //    Box::new(ConvContext::conv(y.as_ref(), variables)?),
            //)),
            air::Expression::Binary(x, y, z) => {
                let (x, x_proto) = ConvContext::conv(context, x.as_ref())?;
                let (z, z_proto) = ConvContext::conv(context, z.as_ref())?;

                let body = sir::Expression::Binary(Box::new(x), *y, Box::new(z));
                let proto = sir::ExpressionProto::Binary(Box::new(x_proto), *y, Box::new(z_proto));

                Some((body, proto))
            }
            _ => None,
        }
    }
}

impl Conv<&air::Module> for sir::Ir {
    fn conv(src: &air::Module) -> Option<sir::Ir> {
        let name = src.name;
        let ports = src.ports.clone();

        let mut variables = HashMap::default();
        for (k, v) in &src.variables {
            let v = Conv::conv(v)?;
            variables.insert(*k, v);
        }

        let functions = HashMap::default();

        let mut analyzer_context = veryl_analyzer::conv::Context::default();
        analyzer_context.variables = src.variables.clone();

        let mut context = Context {
            variables,
            analyzer_context,
            ..Default::default()
        };

        let mut declarations = vec![];
        for x in &src.declarations {
            let x = ConvContext::conv(&mut context, x)?;
            declarations.push(x);
        }

        let ir = sir::Ir {
            name,
            ports,
            values: context.values,
            variables: context.variables,
            functions,
            binary: context.binary,
            declarations,
        };
        Some(ir)
    }
}
