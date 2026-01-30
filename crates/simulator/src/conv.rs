use crate::HashMap;
use crate::cranelift;
use crate::ir as sir;
use crate::ir::{Clock, CombValue, FfValue, Module, Reset, VarId, Variable};
use daggy::Dag;
use daggy::petgraph::algo;
use memmap2::Mmap;
use num_traits::ToPrimitive;
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;

pub fn build_ir(ir: air::Ir, top: StrId, config: &Config) -> Option<sir::Ir> {
    for x in &ir.components {
        if let air::Component::Module(x) = x
            && top == x.name
        {
            let mut context = Context {
                config: config.clone(),
                ..Default::default()
            };

            let ret: sir::Module = Conv::conv(&mut context, x)?;

            return Some(sir::Ir {
                name: ret.name,
                ports: ret.ports,
                ff_values: context.all_ff_values,
                comb_values: context.all_comb_values,
                variables: context.all_variables,
                functions: HashMap::default(),
                binary: context.binary,
                event_statements: ret.event_statements,
                comb_statements: ret.comb_statements,
            });
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub use_4state: bool,
    pub use_jit: bool,
    pub dump_cranelift: bool,
    pub dump_asm: bool,
}

impl Config {
    pub fn all() -> Vec<Config> {
        let mut ret = vec![];

        for use_4state in [false, true] {
            for use_jit in [false, true] {
                ret.push(Config {
                    use_4state,
                    use_jit,
                    ..Default::default()
                });
            }
        }

        ret
    }
}

pub struct ScopeContext {
    pub variables: HashMap<VarId, Variable>,
    pub ff_values_ptr: *const FfValue,
    pub comb_values_ptr: *const CombValue,
    pub ff_table: air::FfTable,
    pub analyzer_context: veryl_analyzer::conv::Context,
}

#[derive(Default)]
pub struct Context {
    pub config: Config,
    pub scope_contexts: Vec<ScopeContext>,
    pub all_ff_values: Vec<Box<[FfValue]>>,
    pub all_comb_values: Vec<Box<[CombValue]>>,
    pub all_variables: HashMap<VarId, Variable>,
    pub binary: Vec<Mmap>,
}

impl Context {
    pub fn scope(&mut self) -> &mut ScopeContext {
        self.scope_contexts.last_mut().unwrap()
    }
}

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Option<Self>;
}

impl Conv<&air::Declaration> for sir::ConvDeclaration {
    fn conv(context: &mut Context, src: &air::Declaration) -> Option<Self> {
        match src {
            air::Declaration::Comb(x) => {
                let mut comb_statements = vec![];

                for x in &x.statements {
                    let x: sir::ConvStatement = Conv::conv(context, x)?;
                    comb_statements.push(x);
                }

                Some(sir::ConvDeclaration {
                    event_statements: HashMap::default(),
                    comb_statements,
                })
            }
            air::Declaration::Ff(x) => {
                let mut statements = vec![];
                for x in &x.statements {
                    let x: sir::ConvStatement = Conv::conv(context, x)?;
                    statements.push(x);
                }

                let clock: Clock = Conv::conv(context, &x.clock)?;
                let clock = sir::Event::Clock(clock.id);

                let mut event_statements = HashMap::default();

                if let Some(x) = &x.reset {
                    let reset: Reset = Conv::conv(context, x)?;
                    let reset = sir::Event::Reset(reset.id);

                    let head = statements.remove(0);
                    let (true_side, false_side) = head.split_if_reset().unwrap();

                    event_statements.insert(reset, true_side);
                    event_statements.insert(clock, false_side);
                } else {
                    event_statements.insert(clock, statements);
                }

                Some(sir::ConvDeclaration {
                    event_statements,
                    comb_statements: vec![],
                })
            }
            air::Declaration::Inst(x) => {
                if let air::Component::Module(x) = &x.component {
                    for x in &x.variables {
                        dbg!(x.1.path.to_string());
                    }
                }
                dbg!("a");
                None
            }
            _ => None,
        }
    }
}

impl Conv<&air::FfClock> for sir::Clock {
    fn conv(_context: &mut Context, src: &air::FfClock) -> Option<Self> {
        Some(sir::Clock {
            id: src.id,
            index: None,
            select: None,
        })
    }
}

impl Conv<&air::FfReset> for sir::Reset {
    fn conv(_context: &mut Context, src: &air::FfReset) -> Option<Self> {
        Some(sir::Reset {
            id: src.id,
            index: None,
            select: None,
        })
    }
}

impl Conv<&air::Statement> for sir::ConvStatement {
    fn conv(context: &mut Context, src: &air::Statement) -> Option<Self> {
        match src {
            air::Statement::Assign(x) => {
                let x: sir::ConvAssignStatement = Conv::conv(context, x)?;
                let value = sir::Statement::Assign(x.value);
                let proto = x.proto.map(sir::ProtoStatement::Assign);
                Some(sir::ConvStatement { value, proto })
            }
            air::Statement::If(x) => {
                let x: sir::ConvIfStatement = Conv::conv(context, x)?;
                let value = sir::Statement::If(x.value);
                let proto = x.proto.map(sir::ProtoStatement::If);
                Some(sir::ConvStatement { value, proto })
            }
            air::Statement::IfReset(x) => {
                let x: sir::ConvIfStatement = Conv::conv(context, x)?;
                let value = sir::Statement::If(x.value);
                let proto = x.proto.map(sir::ProtoStatement::If);
                Some(sir::ConvStatement { value, proto })
            }
            _ => None,
        }
    }
}

impl Conv<&air::AssignStatement> for sir::ConvAssignStatement {
    fn conv(context: &mut Context, src: &air::AssignStatement) -> Option<Self> {
        let scope = context.scope();

        // TODO multiple dst
        let dst = &src.dst[0];

        let id = dst.id;
        let dst_var = scope.variables.get(&id).unwrap();

        let index = dst.index.eval_value(&mut scope.analyzer_context)?;
        let index = dst_var.r#type.array.calc_index(&index)?;

        let select = if !dst.select.is_empty() {
            dst.select
                .eval_value(&mut scope.analyzer_context, &dst.comptime.r#type, false)
        } else {
            None
        };

        let dst_is_ff = scope.ff_table.is_ff(id, index);
        let dst_width = dst_var.width;

        let (dst_ptr, dst_offset) = if dst_is_ff {
            let dst_ptr = dst_var.next_values[index];
            let dst_u64_ptr = unsafe { (*dst_ptr).as_u64_ptr() };
            let dst_offset = if let Some(dst_u64_ptr) = dst_u64_ptr {
                let dst_offset =
                    unsafe { dst_u64_ptr.byte_offset_from(scope.ff_values_ptr) as i32 };
                Some(dst_offset)
            } else {
                None
            };
            (dst_ptr, dst_offset)
        } else {
            let dst_ptr = dst_var.current_values[index];
            let dst_u64_ptr = unsafe { (*dst_ptr).as_u64_ptr() };
            let dst_offset = if let Some(dst_u64_ptr) = dst_u64_ptr {
                let dst_offset =
                    unsafe { dst_u64_ptr.byte_offset_from(scope.comb_values_ptr) as i32 };
                Some(dst_offset)
            } else {
                None
            };
            (dst_ptr, dst_offset)
        };

        let expr: sir::ConvExpression = Conv::conv(context, &src.expr)?;

        let value = sir::AssignStatement {
            dst_id: id,
            dst: dst_ptr,
            select,
            expr: expr.value,
        };
        let proto = if let Some(expr) = expr.proto {
            dst_offset.map(|dst_offset| sir::ProtoAssignStatement {
                dst_offset,
                dst_is_ff,
                dst_width,
                select,
                expr,
            })
        } else {
            None
        };

        Some(sir::ConvAssignStatement { value, proto })
    }
}

impl Conv<&air::IfStatement> for sir::ConvIfStatement {
    fn conv(context: &mut Context, src: &air::IfStatement) -> Option<Self> {
        let cond: sir::ConvExpression = Conv::conv(context, &src.cond)?;

        let mut true_side = vec![];
        let mut true_side_proto = vec![];
        for x in &src.true_side {
            let x: sir::ConvStatement = Conv::conv(context, x)?;
            true_side.push(x.value);
            if let Some(x) = x.proto {
                true_side_proto.push(x);
            }
        }

        let mut false_side = vec![];
        let mut false_side_proto = vec![];
        for x in &src.false_side {
            let x: sir::ConvStatement = Conv::conv(context, x)?;
            false_side.push(x.value);
            if let Some(x) = x.proto {
                false_side_proto.push(x);
            }
        }

        let proto = if true_side.len() == true_side_proto.len()
            && false_side.len() == false_side_proto.len()
            && let Some(cond) = cond.proto
        {
            Some(sir::ProtoIfStatement {
                cond: Some(cond),
                true_side: true_side_proto,
                false_side: false_side_proto,
            })
        } else {
            None
        };

        let value = sir::IfStatement {
            cond: Some(cond.value),
            true_side,
            false_side,
        };

        Some(sir::ConvIfStatement { value, proto })
    }
}

impl Conv<&air::IfResetStatement> for sir::ConvIfStatement {
    fn conv(context: &mut Context, src: &air::IfResetStatement) -> Option<Self> {
        let mut true_side = vec![];
        let mut true_side_proto = vec![];
        for x in &src.true_side {
            let x: sir::ConvStatement = Conv::conv(context, x)?;
            true_side.push(x.value);
            if let Some(x) = x.proto {
                true_side_proto.push(x);
            }
        }

        let mut false_side = vec![];
        let mut false_side_proto = vec![];
        for x in &src.false_side {
            let x: sir::ConvStatement = Conv::conv(context, x)?;
            false_side.push(x.value);
            if let Some(x) = x.proto {
                false_side_proto.push(x);
            }
        }

        let proto = if true_side.len() == true_side_proto.len()
            && false_side.len() == false_side_proto.len()
        {
            Some(sir::ProtoIfStatement {
                cond: None,
                true_side: true_side_proto,
                false_side: false_side_proto,
            })
        } else {
            None
        };

        let value = sir::IfStatement {
            cond: None,
            true_side,
            false_side,
        };

        Some(sir::ConvIfStatement { value, proto })
    }
}

impl Conv<&air::Expression> for sir::ConvExpression {
    fn conv(context: &mut Context, src: &air::Expression) -> Option<Self> {
        match src {
            air::Expression::Term(x) => match x.as_ref() {
                air::Factor::Variable(id, index, select, comptime) => {
                    let scope = context.scope();

                    let variable = scope.variables.get_mut(id).unwrap();

                    let index = index.eval_value(&mut scope.analyzer_context)?;
                    let index = variable.r#type.array.calc_index(&index)?;

                    let select = if !select.is_empty() {
                        select.eval_value(&mut scope.analyzer_context, &comptime.r#type, false)
                    } else {
                        None
                    };

                    let ptr = variable.current_values[index];
                    let u64_ptr = unsafe { (*ptr).as_u64_ptr() };

                    let is_ff = scope.ff_table.is_ff(*id, index);
                    let offset = if let Some(u64_ptr) = u64_ptr {
                        if is_ff {
                            unsafe { Some(u64_ptr.byte_offset_from(scope.ff_values_ptr) as i32) }
                        } else {
                            unsafe { Some(u64_ptr.byte_offset_from(scope.comb_values_ptr) as i32) }
                        }
                    } else {
                        None
                    };

                    let value = sir::Expression::Variable(*id, ptr, select);
                    let width = comptime.r#type.total_width()?;
                    let proto = offset.map(|x| sir::ProtoExpression::Variable(x, is_ff, select));
                    let expr_context: sir::ExpressionContext = (&comptime.expr_context).into();
                    Some(sir::ConvExpression {
                        value,
                        width,
                        proto,
                        expr_context,
                    })
                }
                air::Factor::Value(comptime) => {
                    let x = comptime.get_value().ok().cloned()?;

                    let proto = if x.width() <= 64 {
                        let payload = x.payload().to_u64().unwrap();
                        let mask_xz = x.mask_xz().to_u64().unwrap();
                        let mask_xz = if mask_xz != 0 { Some(mask_xz) } else { None };
                        Some(sir::ProtoExpression::Value(payload, mask_xz))
                    } else {
                        None
                    };
                    let value = sir::Expression::Value(x);
                    let width = comptime.r#type.total_width()?;
                    let expr_context: sir::ExpressionContext = (&comptime.expr_context).into();
                    Some(sir::ConvExpression {
                        value,
                        width,
                        proto,
                        expr_context,
                    })
                }
                _ => None,
            },
            air::Expression::Unary(op, x, comptime) => {
                let x: sir::ConvExpression = Conv::conv(context, x.as_ref())?;

                let width = comptime.expr_context.width;
                let expr_context: sir::ExpressionContext = (&comptime.expr_context).into();

                let value = sir::Expression::Unary(*op, Box::new(x.value), x.expr_context);

                let x_width = x.width;

                let proto = x.proto.map(|x| {
                    let x = sir::ProtoOperand {
                        expr: x,
                        width: x_width,
                    };
                    sir::ProtoExpression::Unary(*op, Box::new(x), expr_context)
                });

                Some(sir::ConvExpression {
                    value,
                    width,
                    proto,
                    expr_context,
                })
            }
            air::Expression::Binary(x, op, y, comptime) => {
                let mut x: sir::ConvExpression = Conv::conv(context, x.as_ref())?;
                let mut y: sir::ConvExpression = Conv::conv(context, y.as_ref())?;

                // expand width of constant value
                let width = comptime.expr_context.width;
                let expr_context: sir::ExpressionContext = (&comptime.expr_context).into();
                x.value.expand(width);
                y.value.expand(width);

                let value = sir::Expression::Binary(
                    Box::new(x.value),
                    *op,
                    Box::new(y.value),
                    x.expr_context,
                );

                let x_width = x.width;
                let y_width = y.width;
                let x_expr_context = x.expr_context;

                let proto = if let (Some(x), Some(y)) = (x.proto, y.proto) {
                    let x = sir::ProtoOperand {
                        expr: x,
                        width: x_width,
                    };
                    let y = sir::ProtoOperand {
                        expr: y,
                        width: y_width,
                    };

                    Some(sir::ProtoExpression::Binary(
                        Box::new(x),
                        *op,
                        Box::new(y),
                        x_expr_context,
                    ))
                } else {
                    None
                };

                Some(sir::ConvExpression {
                    value,
                    width,
                    proto,
                    expr_context,
                })
            }
            _ => None,
        }
    }
}

impl Conv<&air::Module> for sir::Module {
    fn conv(context: &mut Context, src: &air::Module) -> Option<sir::Module> {
        let mut analyzer_context = veryl_analyzer::conv::Context::default();
        analyzer_context.variables = src.variables.clone();

        let mut ff_table = air::FfTable::default();
        src.gather_ff(&mut analyzer_context, &mut ff_table);
        ff_table.update_is_ff();

        let (mut ff_values, mut comb_values) =
            create_values(&src.variables, &ff_table, &context.config);

        let variables =
            create_variables(&src.variables, &ff_table, &mut ff_values, &mut comb_values)?;

        context.all_variables.extend(variables.clone().drain());

        let scope_context = ScopeContext {
            variables,
            ff_values_ptr: ff_values.as_ptr(),
            comb_values_ptr: comb_values.as_ptr(),
            ff_table,
            analyzer_context,
        };

        context.scope_contexts.push(scope_context);
        context.all_ff_values.push(ff_values);
        context.all_comb_values.push(comb_values);

        let mut event_conv_statements = HashMap::default();
        let mut comb_conv_statements = vec![];
        for x in &src.declarations {
            let mut x: sir::ConvDeclaration = Conv::conv(context, x)?;

            for (k, mut v) in x.event_statements {
                event_conv_statements
                    .entry(k)
                    .and_modify(|x: &mut Vec<sir::ConvStatement>| x.append(&mut v))
                    .or_insert(v);
            }

            comb_conv_statements.append(&mut x.comb_statements);
        }

        // analyze and sort by dependency between comb_statements
        // TODO report combinational loop
        // TODO bit-wise dependency analysis
        let comb_conv_statements = analyze_dependency(comb_conv_statements)?;

        let comb_statements = apply_jit(context, comb_conv_statements);

        let mut event_statements = HashMap::default();
        for (k, v) in event_conv_statements {
            let statements = apply_jit(context, v);
            event_statements.insert(k, statements);
        }

        let name = src.name;
        let ports = src.ports.clone();

        Some(Module {
            name,
            ports,
            event_statements,
            comb_statements,
        })
    }
}

fn create_values(
    src: &HashMap<VarId, air::Variable>,
    ff_table: &air::FfTable,
    config: &Config,
) -> (Box<[FfValue]>, Box<[CombValue]>) {
    let mut ff_values = vec![];
    let mut comb_values = vec![];
    for v in src.values() {
        let id = v.id;
        for (i, v) in v.value.iter().enumerate() {
            let is_ff = ff_table.is_ff(id, i);

            let mut v = v.clone();
            if !config.use_4state {
                v.clear_xz();
            }

            if is_ff {
                let value = FfValue {
                    current: v.clone(),
                    next: v.clone(),
                };
                ff_values.push(value);
            } else {
                let value = CombValue(v.clone());
                comb_values.push(value);
            }
        }
    }

    // Fix address of [Value] for raw pointer access
    let ff_values = ff_values.into_boxed_slice();
    let comb_values = comb_values.into_boxed_slice();

    (ff_values, comb_values)
}

fn create_variables(
    src: &HashMap<VarId, air::Variable>,
    ff_table: &air::FfTable,
    ff_values: &mut [FfValue],
    comb_values: &mut [CombValue],
) -> Option<HashMap<VarId, Variable>> {
    let mut ff_index = 0;
    let mut comb_index = 0;
    let mut variables = HashMap::default();
    for (k, v) in src {
        let mut current_values = vec![];
        let mut next_values = vec![];
        for (i, _) in v.value.iter().enumerate() {
            if ff_table.is_ff(v.id, i) {
                let ptr = ff_values[ff_index].as_mut_ptr();
                current_values.push(ptr);
                let ptr = ff_values[ff_index].as_next_mut_ptr();
                next_values.push(ptr);
                ff_index += 1;
            } else {
                let ptr = comb_values[comb_index].as_mut_ptr();
                current_values.push(ptr);
                comb_index += 1;
            }
        }

        let variable = Variable {
            path: v.path.clone(),
            r#type: v.r#type.clone(),
            width: v.r#type.total_width()?,
            current_values,
            next_values,
        };
        variables.insert(*k, variable);
    }
    Some(variables)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum Node {
    Var(VarId),
    Statement(usize),
}

fn analyze_dependency(conv_statements: Vec<sir::ConvStatement>) -> Option<Vec<sir::ConvStatement>> {
    let mut table = HashMap::default();
    for (i, x) in conv_statements.into_iter().enumerate() {
        table.insert(i, x);
    }

    let mut dag = Dag::<Node, ()>::new();
    let mut dag_nodes = HashMap::default();

    for (id, x) in &table {
        let mut inputs = vec![];
        let mut outputs = vec![];
        x.value.gather_variable(&mut inputs, &mut outputs);

        let node = Node::Statement(*id);
        let stmt = dag.add_node(node);
        dag_nodes.insert(node, stmt);

        for x in inputs {
            let node = Node::Var(x);

            let var = if let Some(x) = dag_nodes.get(&node) {
                *x
            } else {
                let x = dag.add_node(node);
                dag_nodes.insert(node, x);
                x
            };

            dag.add_edge(var, stmt, ()).ok()?;
        }

        for x in outputs {
            let node = Node::Var(x);

            let var = if let Some(x) = dag_nodes.get(&node) {
                *x
            } else {
                let x = dag.add_node(node);
                dag_nodes.insert(node, x);
                x
            };

            dag.add_edge(stmt, var, ()).ok()?;
        }
    }

    let nodes = algo::toposort(dag.graph(), None).ok()?;

    let mut ret = vec![];

    for i in nodes {
        let node = dag[i];
        if let Node::Statement(x) = node {
            ret.push(table.remove(&x).unwrap());
        }
    }

    Some(ret)
}

fn apply_jit(
    context: &mut Context,
    conv_statements: Vec<sir::ConvStatement>,
) -> Vec<sir::Statement> {
    let proto: Vec<_> = conv_statements
        .iter()
        .filter_map(|x| x.proto.clone())
        .collect();
    let valid_proto = proto.len() == conv_statements.len();

    // TODO mix jit and interpreter
    let mut statements: Vec<_> = conv_statements.into_iter().map(|x| x.value).collect();

    if context.config.use_jit
        && valid_proto
        && let Some(func) = cranelift::build_binary(context, proto)
    {
        let ff_values = context.scope().ff_values_ptr;
        let comb_values = context.scope().comb_values_ptr;
        statements = vec![sir::Statement::Binary((func, ff_values, comb_values))];
    }

    statements
}
