use crate::HashMap;
use crate::cranelift::{self, FuncPtr};
use crate::ir as sir;
use crate::ir::{Clock, CombValue, FfValue, Reset, VarId, Variable};
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
            let ret = Conv::conv(config, x);
            return ret;
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

#[derive(Default)]
pub struct Context {
    pub config: Config,
    pub variables: HashMap<VarId, Variable>,
    pub ff_values_ptr: *const FfValue,
    pub comb_values_ptr: *const CombValue,
    pub ff_table: veryl_analyzer::ir::FfTable,
    pub binary: Vec<Mmap>,
    pub func_ptr_map: HashMap<Vec<sir::ProtoStatement>, FuncPtr>,
    pub analyzer_context: veryl_analyzer::conv::Context,
}

pub trait Conv<T>: Sized {
    fn conv(config: &Config, src: T) -> Option<Self>;
}

pub trait ConvContext<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Option<Self>;
}

impl ConvContext<&air::Declaration> for sir::ConvDeclaration {
    fn conv(context: &mut Context, src: &air::Declaration) -> Option<Self> {
        match src {
            air::Declaration::Comb(x) => {
                let mut comb_statements = vec![];

                for x in &x.statements {
                    let x: sir::ConvStatement = ConvContext::conv(context, x)?;
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
                    let x: sir::ConvStatement = ConvContext::conv(context, x)?;
                    statements.push(x);
                }

                let clock: Clock = Conv::conv(&context.config, &x.clock)?;
                let clock = sir::Event::Clock(clock.id);

                let mut event_statements = HashMap::default();

                if let Some(x) = &x.reset {
                    let reset: Reset = Conv::conv(&context.config, x)?;
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
            _ => None,
        }
    }
}

impl Conv<&air::FfClock> for sir::Clock {
    fn conv(_config: &Config, src: &air::FfClock) -> Option<Self> {
        Some(sir::Clock {
            id: src.id,
            index: None,
            select: None,
        })
    }
}

impl Conv<&air::FfReset> for sir::Reset {
    fn conv(_config: &Config, src: &air::FfReset) -> Option<Self> {
        Some(sir::Reset {
            id: src.id,
            index: None,
            select: None,
        })
    }
}

impl ConvContext<&air::Statement> for sir::ConvStatement {
    fn conv(context: &mut Context, src: &air::Statement) -> Option<Self> {
        match src {
            air::Statement::Assign(x) => {
                let x: sir::ConvAssignStatement = ConvContext::conv(context, x)?;
                let value = sir::Statement::Assign(x.value);
                let proto = x.proto.map(sir::ProtoStatement::Assign);
                Some(sir::ConvStatement { value, proto })
            }
            air::Statement::If(x) => {
                let x: sir::ConvIfStatement = ConvContext::conv(context, x)?;
                let value = sir::Statement::If(x.value);
                let proto = x.proto.map(sir::ProtoStatement::If);
                Some(sir::ConvStatement { value, proto })
            }
            air::Statement::IfReset(x) => {
                let x: sir::ConvIfStatement = ConvContext::conv(context, x)?;
                let value = sir::Statement::If(x.value);
                let proto = x.proto.map(sir::ProtoStatement::If);
                Some(sir::ConvStatement { value, proto })
            }
            _ => None,
        }
    }
}

impl Conv<&air::AssignDestination> for sir::AssignDestination {
    fn conv(_config: &Config, src: &air::AssignDestination) -> Option<Self> {
        Some(sir::AssignDestination {
            id: src.id,
            index: None,
            select: None,
        })
    }
}

impl ConvContext<&air::AssignStatement> for sir::ConvAssignStatement {
    fn conv(context: &mut Context, src: &air::AssignStatement) -> Option<Self> {
        let dst = &src.dst[0];

        let id = dst.id;
        let index = dst.index.eval_value(&mut context.analyzer_context)?;
        let dst = context.analyzer_context.variables.get_mut(&id).unwrap();
        let index = dst.r#type.array.calc_index(&index)?;

        let dst = context.variables.get(&dst.id).unwrap();
        let dst_is_ff = context.ff_table.is_ff(id, index);
        let dst_width = dst.width;
        let (dst_ptr, dst_offset) = if dst_is_ff {
            let dst_ptr = dst.next_values[index];
            let dst_u64_ptr = unsafe { (*dst_ptr).as_u64_ptr() };
            let dst_offset = if let Some(dst_u64_ptr) = dst_u64_ptr {
                let dst_offset =
                    unsafe { dst_u64_ptr.byte_offset_from(context.ff_values_ptr) as i32 };
                Some(dst_offset)
            } else {
                None
            };
            (dst_ptr, dst_offset)
        } else {
            let dst_ptr = dst.current_values[index];
            let dst_u64_ptr = unsafe { (*dst_ptr).as_u64_ptr() };
            let dst_offset = if let Some(dst_u64_ptr) = dst_u64_ptr {
                let dst_offset =
                    unsafe { dst_u64_ptr.byte_offset_from(context.comb_values_ptr) as i32 };
                Some(dst_offset)
            } else {
                None
            };
            (dst_ptr, dst_offset)
        };

        let expr_signed = src.expr.eval_signed();
        let expr: sir::ConvExpression = ConvContext::conv(context, &src.expr)?;

        let value = sir::AssignStatement {
            dst_id: id,
            dst: dst_ptr,
            expr: expr.value,
            expr_signed,
        };
        let proto = if let Some(expr) = expr.proto {
            dst_offset.map(|dst_offset| sir::ProtoAssignStatement {
                dst_offset,
                dst_is_ff,
                dst_width,
                expr,
                expr_signed,
            })
        } else {
            None
        };

        Some(sir::ConvAssignStatement { value, proto })
    }
}

impl ConvContext<&air::IfStatement> for sir::ConvIfStatement {
    fn conv(context: &mut Context, src: &air::IfStatement) -> Option<Self> {
        let cond: sir::ConvExpression = ConvContext::conv(context, &src.cond)?;

        let mut true_side = vec![];
        let mut true_side_proto = vec![];
        for x in &src.true_side {
            let x: sir::ConvStatement = ConvContext::conv(context, x)?;
            true_side.push(x.value);
            if let Some(x) = x.proto {
                true_side_proto.push(x);
            }
        }

        let mut false_side = vec![];
        let mut false_side_proto = vec![];
        for x in &src.false_side {
            let x: sir::ConvStatement = ConvContext::conv(context, x)?;
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

impl ConvContext<&air::IfResetStatement> for sir::ConvIfStatement {
    fn conv(context: &mut Context, src: &air::IfResetStatement) -> Option<Self> {
        let mut true_side = vec![];
        let mut true_side_proto = vec![];
        for x in &src.true_side {
            let x: sir::ConvStatement = ConvContext::conv(context, x)?;
            true_side.push(x.value);
            if let Some(x) = x.proto {
                true_side_proto.push(x);
            }
        }

        let mut false_side = vec![];
        let mut false_side_proto = vec![];
        for x in &src.false_side {
            let x: sir::ConvStatement = ConvContext::conv(context, x)?;
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

impl ConvContext<&air::Expression> for sir::ConvExpression {
    fn conv(context: &mut Context, src: &air::Expression) -> Option<Self> {
        match src {
            air::Expression::Term(x) => match x.as_ref() {
                air::Factor::Variable(id, index, _, _, _) => {
                    let index = index.eval_value(&mut context.analyzer_context)?;
                    let variable = context.analyzer_context.variables.get_mut(id).unwrap();
                    let index = variable.r#type.array.calc_index(&index)?;

                    let variable = context.variables.get_mut(id).unwrap();
                    let ptr = variable.current_values[index];
                    let u64_ptr = unsafe { (*ptr).as_u64_ptr() };

                    let is_ff = context.ff_table.is_ff(*id, index);
                    let offset = if let Some(u64_ptr) = u64_ptr {
                        if is_ff {
                            unsafe { Some(u64_ptr.byte_offset_from(context.ff_values_ptr) as i32) }
                        } else {
                            unsafe {
                                Some(u64_ptr.byte_offset_from(context.comb_values_ptr) as i32)
                            }
                        }
                    } else {
                        None
                    };

                    let value = sir::Expression::Variable(*id, ptr);
                    let proto = offset.map(|x| sir::ProtoExpression::Variable(x, is_ff));
                    Some(sir::ConvExpression { value, proto })
                }
                air::Factor::Value(comptime, _) => {
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
                    Some(sir::ConvExpression { value, proto })
                }
                _ => None,
            },
            air::Expression::Unary(op, x) => {
                let x: sir::ConvExpression = ConvContext::conv(context, x.as_ref())?;

                let x_width = x.value.eval_width();
                let value = sir::Expression::Unary(*op, Box::new(x.value));

                let proto = x
                    .proto
                    .map(|x| sir::ProtoExpression::Unary(*op, Box::new(x), x_width));

                Some(sir::ConvExpression { value, proto })
            }
            air::Expression::Binary(x, op, y) => {
                let mut x: sir::ConvExpression = ConvContext::conv(context, x.as_ref())?;
                let mut y: sir::ConvExpression = ConvContext::conv(context, y.as_ref())?;

                // expand width of constant value
                let x_width = x.value.eval_width();
                let y_width = y.value.eval_width();
                let width = op.eval_binary_width_usize(x_width, y_width, None);
                x.value.expand(width);
                y.value.expand(width);

                let value = sir::Expression::Binary(Box::new(x.value), *op, Box::new(y.value));

                let proto = if let (Some(x), Some(y)) = (x.proto, y.proto) {
                    Some(sir::ProtoExpression::Binary(
                        Box::new(x),
                        x_width,
                        *op,
                        Box::new(y),
                        y_width,
                    ))
                } else {
                    None
                };

                Some(sir::ConvExpression { value, proto })
            }
            _ => None,
        }
    }
}

impl Conv<&air::Module> for sir::Ir {
    fn conv(config: &Config, src: &air::Module) -> Option<sir::Ir> {
        let mut analyzer_context = veryl_analyzer::conv::Context::default();
        analyzer_context.variables = src.variables.clone();

        let name = src.name;
        let ports = src.ports.clone();

        let mut ff_table = air::FfTable::default();
        src.gather_ff(&mut analyzer_context, &mut ff_table);
        ff_table.update_is_ff();

        let (mut ff_values, mut comb_values) = create_values(&src.variables, &ff_table, config);

        let variables =
            create_variables(&src.variables, &ff_table, &mut ff_values, &mut comb_values)?;
        // TODO
        let functions = HashMap::default();

        let mut context = Context {
            config: config.clone(),
            variables,
            ff_values_ptr: ff_values.as_ptr(),
            comb_values_ptr: comb_values.as_ptr(),
            ff_table,
            analyzer_context,
            ..Default::default()
        };

        let mut event_conv_statements = HashMap::default();
        let mut comb_conv_statements = vec![];
        for x in &src.declarations {
            let mut x: sir::ConvDeclaration = ConvContext::conv(&mut context, x)?;

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
        let comb_conv_statements = analyze_dependency(comb_conv_statements)?;

        let comb_statements = apply_jit(&mut context, comb_conv_statements);

        let mut event_statements = HashMap::default();
        for (k, v) in event_conv_statements {
            let statements = apply_jit(&mut context, v);
            event_statements.insert(k, statements);
        }

        let ir = sir::Ir {
            name,
            ports,
            ff_values,
            comb_values,
            variables: context.variables,
            functions,
            binary: context.binary,
            event_statements,
            comb_statements,
        };
        Some(ir)
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

    let mut statements: Vec<_> = conv_statements.into_iter().map(|x| x.value).collect();

    if context.config.use_jit
        && valid_proto
        && let Some(func) = cranelift::build_binary(context, proto)
    {
        let ff_values = context.ff_values_ptr;
        let comb_values = context.comb_values_ptr;
        statements = vec![sir::Statement::Binary((func, ff_values, comb_values))];
    }

    statements
}
