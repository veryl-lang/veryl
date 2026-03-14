use crate::HashMap;
use crate::cranelift;
use crate::ir::context::{Context, Conv, ScopeContext};
use crate::ir::variable::{ModuleVariableMeta, ModuleVariables, create_variable_meta};
use crate::ir::{
    CombValue, Event, FfValue, ProtoDeclaration, ProtoStatement, ProtoStatementBlock,
    ProtoStatements, Statement, Value, VarId, VarPath, Variable,
};
use daggy::Dag;
use daggy::petgraph::algo;
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;

pub struct Module {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_values: Box<[FfValue]>,
    pub comb_values: Box<[CombValue]>,
    pub module_variables: ModuleVariables,

    pub event_statements: HashMap<Event, Vec<Statement>>,
    pub comb_statements: Vec<Statement>,
}

pub struct ProtoModule {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_count: usize,
    pub comb_count: usize,
    pub module_variable_meta: ModuleVariableMeta,

    pub event_statements: HashMap<Event, ProtoStatements>,
    pub comb_statements: ProtoStatements,
}

fn create_buffers(
    module_variable_meta: &ModuleVariableMeta,
    ff_count: usize,
    comb_count: usize,
) -> (Box<[FfValue]>, Box<[CombValue]>) {
    let mut ff_values: Vec<FfValue> = Vec::with_capacity(ff_count);
    let mut comb_values: Vec<CombValue> = Vec::with_capacity(comb_count);

    fill_buffers_recursive(module_variable_meta, &mut ff_values, &mut comb_values);

    (ff_values.into_boxed_slice(), comb_values.into_boxed_slice())
}

/// Fill buffers recursively per-module in VarId order.
/// This must match the allocation order used in create_variable_meta,
/// where each module's variables are sorted by VarId and allocated
/// contiguously within their ff_start/comb_start range.
fn fill_buffers_recursive(
    module_meta: &ModuleVariableMeta,
    ff_values: &mut Vec<FfValue>,
    comb_values: &mut Vec<CombValue>,
) {
    let mut sorted: Vec<_> = module_meta.variable_meta.iter().collect();
    sorted.sort_by_key(|(k, _)| **k);

    for (_, meta) in &sorted {
        for (element, initial) in meta.elements.iter().zip(meta.initial_values.iter()) {
            if element.is_ff {
                ff_values.push(FfValue {
                    current: initial.clone(),
                    next: initial.clone(),
                });
            } else {
                comb_values.push(CombValue(initial.clone()));
            }
        }
    }

    for child in &module_meta.children {
        fill_buffers_recursive(child, ff_values, comb_values);
    }
}

fn create_variables_recursive(
    module_meta: &ModuleVariableMeta,
    ff_base: *mut u8,
    comb_base: *mut u8,
) -> ModuleVariables {
    let mut variables = HashMap::default();

    for (id, meta) in &module_meta.variable_meta {
        let mut current_values: Vec<*mut Value> = vec![];
        let mut next_values: Vec<*mut Value> = vec![];

        for element in &meta.elements {
            let current = unsafe {
                let base = if element.is_ff { ff_base } else { comb_base };
                base.add(element.current_offset as usize) as *mut Value
            };
            current_values.push(current);

            if element.is_ff {
                let next = unsafe { ff_base.add(element.next_offset as usize) as *mut Value };
                next_values.push(next);
            }
        }

        variables.insert(
            *id,
            Variable {
                path: meta.path.clone(),
                r#type: meta.r#type.clone(),
                width: meta.width,
                current_values,
                next_values,
            },
        );
    }

    let children = module_meta
        .children
        .iter()
        .map(|child| create_variables_recursive(child, ff_base, comb_base))
        .collect();

    ModuleVariables {
        name: module_meta.name,
        variables,
        children,
    }
}

impl ProtoModule {
    pub fn instantiate(&self) -> Module {
        let (mut ff_values, mut comb_values) =
            create_buffers(&self.module_variable_meta, self.ff_count, self.comb_count);

        let ff_base = ff_values.as_mut_ptr() as *mut u8;
        let comb_base = comb_values.as_mut_ptr() as *mut u8;

        let module_variables =
            create_variables_recursive(&self.module_variable_meta, ff_base, comb_base);

        let ff_ptr = ff_values.as_mut_ptr();
        let comb_ptr = comb_values.as_mut_ptr();

        let event_statements = self
            .event_statements
            .iter()
            .map(|(event, stmts)| (event.clone(), stmts.to_statements(ff_ptr, comb_ptr)))
            .collect();

        let comb_statements = self.comb_statements.to_statements(ff_ptr, comb_ptr);

        Module {
            name: self.name,
            ports: self.ports.clone(),
            ff_values,
            comb_values,
            module_variables,

            event_statements,
            comb_statements,
        }
    }
}

fn try_jit(context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
    if !context.config.use_jit {
        return ProtoStatements(vec![ProtoStatementBlock::Interpreted(proto)]);
    }

    // Group consecutive statements by can_build_binary() result
    let mut blocks: Vec<ProtoStatementBlock> = Vec::new();
    let mut current_jittable: Option<bool> = None;
    let mut current_group: Vec<ProtoStatement> = Vec::new();

    for stmt in proto {
        let jittable = stmt.can_build_binary();

        if current_jittable == Some(jittable) {
            current_group.push(stmt);
        } else {
            if let Some(was_jittable) = current_jittable {
                let group = std::mem::take(&mut current_group);
                if was_jittable {
                    // Try to JIT compile
                    match cranelift::build_binary(context, group.clone()) {
                        Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                        None => blocks.push(ProtoStatementBlock::Interpreted(group)),
                    }
                } else {
                    blocks.push(ProtoStatementBlock::Interpreted(group));
                }
            }
            current_jittable = Some(jittable);
            current_group.push(stmt);
        }
    }

    // Flush the last group
    if let Some(was_jittable) = current_jittable {
        if was_jittable {
            match cranelift::build_binary(context, current_group.clone()) {
                Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                None => blocks.push(ProtoStatementBlock::Interpreted(current_group)),
            }
        } else {
            blocks.push(ProtoStatementBlock::Interpreted(current_group));
        }
    }

    ProtoStatements(blocks)
}

fn analyze_dependency(statements: Vec<ProtoStatement>) -> Option<Vec<ProtoStatement>> {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    enum Node {
        Var(bool, isize), // (is_ff, offset)
        Statement(usize),
    }

    let mut table = HashMap::default();
    for (i, x) in statements.into_iter().enumerate() {
        table.insert(i, x);
    }

    let mut dag = Dag::<Node, ()>::new();
    let mut dag_nodes: HashMap<Node, _> = HashMap::default();

    for (id, x) in &table {
        let mut inputs = vec![];
        let mut outputs = vec![];
        x.gather_variable_offsets(&mut inputs, &mut outputs);

        let stmt_node = Node::Statement(*id);
        let stmt = dag.add_node(stmt_node);
        dag_nodes.insert(stmt_node, stmt);

        for var_key in inputs {
            let var_node = Node::Var(var_key.0, var_key.1);
            let var = *dag_nodes
                .entry(var_node)
                .or_insert_with(|| dag.add_node(var_node));
            dag.add_edge(var, stmt, ()).ok()?;
        }

        for var_key in outputs {
            let var_node = Node::Var(var_key.0, var_key.1);
            let var = *dag_nodes
                .entry(var_node)
                .or_insert_with(|| dag.add_node(var_node));
            dag.add_edge(stmt, var, ()).ok()?;
        }
    }

    let nodes = algo::toposort(dag.graph(), None).ok()?;

    let mut ret = vec![];
    for i in nodes {
        if let Node::Statement(x) = dag[i] {
            ret.push(table.remove(&x).unwrap());
        }
    }

    Some(ret)
}

impl Conv<&air::Module> for ProtoModule {
    fn conv(context: &mut Context, src: &air::Module) -> Option<Self> {
        let mut analyzer_context = veryl_analyzer::conv::Context::default();
        analyzer_context.variables = src.variables.clone();
        analyzer_context.functions = src.functions.clone();

        let mut ff_table = air::FfTable::default();
        src.gather_ff(&mut analyzer_context, &mut ff_table);
        ff_table.update_is_ff();

        let ff_start = context.ff_total_count as isize;
        let comb_start = context.comb_total_count as isize;
        let (variable_meta, ff_count, comb_count) = create_variable_meta(
            &src.variables,
            &ff_table,
            context.config.use_4state,
            ff_start,
            comb_start,
        )?;

        context.ff_total_count += ff_count;
        context.comb_total_count += comb_count;

        let scope = ScopeContext {
            variable_meta: variable_meta.clone(),
            analyzer_context,
        };
        context.scope_contexts.push(scope);

        let mut all_event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        let mut all_comb_statements: Vec<ProtoStatement> = vec![];
        let mut all_child_modules: Vec<ModuleVariableMeta> = vec![];

        for decl in &src.declarations {
            let proto_decl: ProtoDeclaration = match Conv::conv(context, decl) {
                Some(d) => d,
                None => continue,
            };

            for (event, mut stmts) in proto_decl.event_statements {
                all_event_statements
                    .entry(event)
                    .and_modify(|v| v.append(&mut stmts))
                    .or_insert(stmts);
            }
            all_comb_statements.append(&mut proto_decl.comb_statements.clone());
            all_child_modules.extend(proto_decl.child_modules);
        }

        context.scope_contexts.pop();

        // TODO report combinational loop
        let sorted_comb = analyze_dependency(all_comb_statements)?;
        let comb_statements = try_jit(context, sorted_comb);

        let event_statements = all_event_statements
            .into_iter()
            .map(|(event, stmts)| (event, try_jit(context, stmts)))
            .collect();

        let module_variable_meta = ModuleVariableMeta {
            name: src.name,
            variable_meta,
            children: all_child_modules,
        };

        Some(ProtoModule {
            name: src.name,
            ports: src.ports.clone(),
            ff_count: context.ff_total_count,
            comb_count: context.comb_total_count,
            module_variable_meta,
            event_statements,
            comb_statements,
        })
    }
}
