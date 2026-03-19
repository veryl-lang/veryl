use crate::HashMap;
use crate::HashSet;
use crate::cranelift;
use crate::ir::context::{Context, Conv, ScopeContext};
use crate::ir::variable::{
    ModuleVariableMeta, ModuleVariables, Variable, create_variable_meta, value_size,
    write_native_value,
};
use crate::ir::{
    Event, ProtoDeclaration, ProtoStatement, ProtoStatementBlock, ProtoStatements, Statement,
    VarId, VarPath,
};
use crate::simulator_error::SimulatorError;
use daggy::Dag;
use daggy::petgraph::algo;
use daggy::petgraph::visit::{Bfs, Walker};
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

pub struct Module {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_values: Box<[u8]>,
    pub comb_values: Box<[u8]>,
    pub module_variables: ModuleVariables,

    pub event_statements: HashMap<Event, Vec<Statement>>,
    pub comb_statements: Vec<Statement>,
    /// Post-comb functions: child module comb-only JIT functions that run
    /// after lite comb (port connections) to compute child comb values
    /// before events fire. Bypasses analyze_dependency.
    pub post_comb_fns: Vec<Statement>,
    /// Full comb statements (includes per-core internal comb).
    /// Used by get()/dump() for correctness after FF swap.
    /// When merged events exist, comb_statements is the "lite" version
    /// (port connections + top-level comb only), while full_comb_statements
    /// includes everything.
    pub full_comb_statements: Option<Vec<Statement>>,
    /// FF swap entries: (current_offset, value_size) pairs.
    pub ff_swap_entries: Vec<(usize, usize)>,
}

pub struct ProtoModule {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_bytes: usize,
    pub comb_bytes: usize,
    pub use_4state: bool,
    pub module_variable_meta: ModuleVariableMeta,

    pub event_statements: HashMap<Event, ProtoStatements>,
    pub comb_statements: ProtoStatements,
    /// Post-comb: child comb-only JIT functions for pre-event evaluation.
    pub post_comb_fns: Vec<ProtoStatement>,
    /// Full comb statements when merged events exist.
    pub full_comb_statements: Option<ProtoStatements>,
    /// FF canonical offsets (current_offset) that need swapping.
    /// If None, all ff variables are swapped.
    pub ff_swap_offsets: Option<HashSet<isize>>,
}

fn create_buffers(
    module_variable_meta: &ModuleVariableMeta,
    ff_bytes: usize,
    comb_bytes: usize,
    use_4state: bool,
) -> (Box<[u8]>, Box<[u8]>) {
    let mut ff_values = vec![0u8; ff_bytes];
    let mut comb_values = vec![0u8; comb_bytes];

    fill_buffers_recursive(
        module_variable_meta,
        &mut ff_values,
        &mut comb_values,
        use_4state,
    );

    (ff_values.into_boxed_slice(), comb_values.into_boxed_slice())
}

/// Fill byte buffers with initial values, writing at the offsets stored in VariableElement.
fn fill_buffers_recursive(
    module_meta: &ModuleVariableMeta,
    ff_values: &mut [u8],
    comb_values: &mut [u8],
    use_4state: bool,
) {
    let mut sorted: Vec<_> = module_meta.variable_meta.iter().collect();
    sorted.sort_by_key(|(k, _)| **k);

    for (_, meta) in &sorted {
        for (element, initial) in meta.elements.iter().zip(meta.initial_values.iter()) {
            let nb = element.native_bytes;
            if element.is_ff {
                let cur = &mut ff_values[element.current_offset as usize..] as *mut [u8] as *mut u8;
                let nxt = &mut ff_values[element.next_offset as usize..] as *mut [u8] as *mut u8;
                unsafe {
                    write_native_value(cur, nb, use_4state, initial);
                    write_native_value(nxt, nb, use_4state, initial);
                }
            } else {
                let cur =
                    &mut comb_values[element.current_offset as usize..] as *mut [u8] as *mut u8;
                unsafe {
                    write_native_value(cur, nb, use_4state, initial);
                }
            }
        }
    }

    for child in &module_meta.children {
        fill_buffers_recursive(child, ff_values, comb_values, use_4state);
    }
}

fn create_variables_recursive(
    module_meta: &ModuleVariableMeta,
    ff_base: *mut u8,
    comb_base: *mut u8,
) -> ModuleVariables {
    let mut variables = HashMap::default();

    for (id, meta) in &module_meta.variable_meta {
        let mut current_values: Vec<*mut u8> = vec![];
        let mut next_values: Vec<*mut u8> = vec![];

        for element in &meta.elements {
            let current = unsafe {
                let base = if element.is_ff { ff_base } else { comb_base };
                base.add(element.current_offset as usize)
            };
            current_values.push(current);

            if element.is_ff {
                let next = unsafe { ff_base.add(element.next_offset as usize) };
                next_values.push(next);
            }
        }

        variables.insert(
            *id,
            Variable {
                path: meta.path.clone(),
                r#type: meta.r#type.clone(),
                width: meta.width,
                native_bytes: meta.native_bytes,
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

/// Collect all FF swap entries (current_offset, value_size) from module metadata.
fn collect_ff_swap_entries(
    module_meta: &ModuleVariableMeta,
    use_4state: bool,
    filter: Option<&HashSet<isize>>,
) -> Vec<(usize, usize)> {
    let mut entries = vec![];
    for meta in module_meta.variable_meta.values() {
        for element in &meta.elements {
            if element.is_ff {
                let include = match filter {
                    Some(offsets) => offsets.contains(&element.current_offset),
                    None => true,
                };
                if include {
                    let vs = value_size(element.native_bytes, use_4state);
                    entries.push((element.current_offset as usize, vs));
                }
            }
        }
    }
    for child in &module_meta.children {
        entries.extend(collect_ff_swap_entries(child, use_4state, filter));
    }
    entries.sort_unstable();
    entries
}

impl ProtoModule {
    pub fn instantiate(&self) -> Module {
        let (mut ff_values, mut comb_values) = create_buffers(
            &self.module_variable_meta,
            self.ff_bytes,
            self.comb_bytes,
            self.use_4state,
        );

        let ff_base = ff_values.as_mut_ptr();
        let comb_base = comb_values.as_mut_ptr();

        let module_variables =
            create_variables_recursive(&self.module_variable_meta, ff_base, comb_base);

        let ff_ptr = ff_values.as_mut_ptr();
        let comb_ptr = comb_values.as_mut_ptr();

        let event_statements = self
            .event_statements
            .iter()
            .map(|(event, stmts)| {
                let s = stmts.to_statements(ff_ptr, comb_ptr, self.use_4state);
                (event.clone(), batch_binary_statements(s))
            })
            .collect();

        let comb_statements = batch_binary_statements(self.comb_statements.to_statements(
            ff_ptr,
            comb_ptr,
            self.use_4state,
        ));

        let post_comb_fns = ProtoStatements(vec![ProtoStatementBlock::Interpreted(
            self.post_comb_fns.clone(),
        )])
        .to_statements(ff_ptr, comb_ptr, self.use_4state);

        let full_comb_statements = self.full_comb_statements.as_ref().map(|stmts| {
            batch_binary_statements(stmts.to_statements(ff_ptr, comb_ptr, self.use_4state))
        });

        let ff_swap_entries = collect_ff_swap_entries(
            &self.module_variable_meta,
            self.use_4state,
            self.ff_swap_offsets.as_ref(),
        );

        Module {
            name: self.name,
            ports: self.ports.clone(),
            ff_values,
            comb_values,
            module_variables,

            event_statements,
            comb_statements,
            post_comb_fns,
            full_comb_statements,
            ff_swap_entries,
        }
    }
}

/// Maximum number of statements per JIT function.
/// Keeps regalloc2 cost manageable (O(N^2) in SSA variable count).
const JIT_CHUNK_SIZE: usize = 256;

fn try_jit_group(
    context: &mut Context,
    blocks: &mut Vec<ProtoStatementBlock>,
    group: Vec<ProtoStatement>,
) {
    // Split large groups into chunks to avoid regalloc2 O(N^2) scaling
    if group.len() <= JIT_CHUNK_SIZE {
        match cranelift::build_binary(context, group.clone()) {
            Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
            None => blocks.push(ProtoStatementBlock::Interpreted(group)),
        }
    } else {
        for chunk in group.chunks(JIT_CHUNK_SIZE) {
            let chunk = chunk.to_vec();
            match cranelift::build_binary(context, chunk.clone()) {
                Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                None => blocks.push(ProtoStatementBlock::Interpreted(chunk)),
            }
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
                    try_jit_group(context, &mut blocks, group);
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
            try_jit_group(context, &mut blocks, current_group);
        } else {
            blocks.push(ProtoStatementBlock::Interpreted(current_group));
        }
    }

    ProtoStatements(blocks)
}

pub(crate) fn analyze_dependency(
    statements: Vec<ProtoStatement>,
) -> Result<Vec<ProtoStatement>, SimulatorError> {
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

    // Closure to collect cycle participant tokens via BFS from a node
    let collect_cycle_tokens = |dag: &Dag<Node, ()>,
                                table: &HashMap<usize, ProtoStatement>,
                                from: daggy::NodeIndex,
                                trigger_id: usize| {
        let mut tokens = vec![];
        let bfs = Bfs::new(dag.graph(), from);
        for node_idx in bfs.iter(dag.graph()) {
            if let Node::Statement(id) = dag.graph()[node_idx]
                && id != trigger_id
                && let Some(stmt) = table.get(&id)
                && let Some(token) = stmt.token()
                && token != TokenRange::default()
            {
                tokens.push(token);
            }
        }
        tokens
    };

    for (id, x) in &table {
        let mut inputs = vec![];
        let mut outputs = vec![];
        x.gather_variable_offsets(&mut inputs, &mut outputs);

        let stmt_node = Node::Statement(*id);
        let stmt = dag.add_node(stmt_node);
        dag_nodes.insert(stmt_node, stmt);

        // Skip self-referencing inputs (same offset in both inputs and
        // outputs). This naturally occurs in conditional comb blocks like
        // always_comb { a = 0; if cond { a = f(a); } } and is NOT a
        // combinational loop.
        let output_set: HashSet<(bool, isize)> = outputs.iter().cloned().collect();
        for var_key in inputs {
            if output_set.contains(&var_key) {
                continue; // Skip self-reference
            }
            let var_node = Node::Var(var_key.0, var_key.1);
            let var = *dag_nodes
                .entry(var_node)
                .or_insert_with(|| dag.add_node(var_node));
            if dag.add_edge(var, stmt, ()).is_err() {
                let participant_tokens = collect_cycle_tokens(&dag, &table, stmt, *id);
                let trigger_token = table[id].token().unwrap_or_default();
                return Err(SimulatorError::combinational_loop(
                    &trigger_token,
                    &participant_tokens,
                ));
            }
        }

        for var_key in outputs {
            let var_node = Node::Var(var_key.0, var_key.1);
            let var = *dag_nodes
                .entry(var_node)
                .or_insert_with(|| dag.add_node(var_node));
            if dag.add_edge(stmt, var, ()).is_err() {
                let participant_tokens = collect_cycle_tokens(&dag, &table, var, *id);
                let trigger_token = table[id].token().unwrap_or_default();
                return Err(SimulatorError::combinational_loop(
                    &trigger_token,
                    &participant_tokens,
                ));
            }
        }
    }

    let nodes = algo::toposort(dag.graph(), None).unwrap();

    let mut ret = vec![];
    for i in nodes {
        if let Node::Statement(x) = dag[i] {
            ret.push(table.remove(&x).unwrap());
        }
    }


    Ok(ret)
}

/// Sort event (FF) statements and determine which FF variables need
/// double-buffered swap.
///
/// Non-blocking assignment semantics require all reads to see "old" values.
/// With native-width storage, each variable may have a different value_size,
/// so we use dst_ff_current_offset as canonical key instead of computing
/// from a uniform ff_next_delta.
///
/// Returns (sorted_statements, needs_swap_offsets).
/// `needs_swap_offsets`: FF current_offsets that need swapping.
/// For non-swap variables, dst_offset is rewritten from next to current.
fn sort_ff_event(statements: Vec<ProtoStatement>) -> (Vec<ProtoStatement>, HashSet<isize>) {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    enum Node {
        /// Variable node. For FF, offset is the canonical current_offset.
        Var(bool, isize),
        Statement(usize),
    }

    let mut table: HashMap<usize, ProtoStatement> = HashMap::default();
    for (i, x) in statements.into_iter().enumerate() {
        table.insert(i, x);
    }

    if table.is_empty() {
        return (vec![], HashSet::default());
    }

    let mut dag = Dag::<Node, ()>::new();
    let mut dag_nodes: HashMap<Node, _> = HashMap::default();

    // Collect the canonical offsets for FF outputs from each statement
    for (id, x) in &table {
        let mut inputs = vec![];
        let mut outputs = vec![];
        x.gather_variable_offsets(&mut inputs, &mut outputs);

        // For FF outputs, gather_variable_offsets returns the actual dst_offset (next).
        // We need canonical (current) offsets. Get them from the statement.
        let ff_write_canonical: HashSet<isize> = x.gather_ff_canonical_offsets();
        let ff_read_offsets: HashSet<isize> = inputs
            .iter()
            .filter(|(is_ff, _)| *is_ff)
            .map(|(_, off)| *off)
            .collect();
        let self_ref: HashSet<isize> = ff_read_offsets
            .intersection(&ff_write_canonical)
            .cloned()
            .collect();

        let stmt_node = Node::Statement(*id);
        let stmt = dag.add_node(stmt_node);
        dag_nodes.insert(stmt_node, stmt);

        for (is_ff, offset) in &inputs {
            if *is_ff {
                if self_ref.contains(offset) {
                    continue;
                }
                // FF read: REVERSED edge (stmt → var)
                let var_node = Node::Var(true, *offset);
                let var = *dag_nodes
                    .entry(var_node)
                    .or_insert_with(|| dag.add_node(var_node));
                if dag.add_edge(stmt, var, ()).is_err() {
                    return fallback_all_swap(table);
                }
            } else {
                let var_node = Node::Var(false, *offset);
                let var = *dag_nodes
                    .entry(var_node)
                    .or_insert_with(|| dag.add_node(var_node));
                if dag.add_edge(var, stmt, ()).is_err() {
                    return fallback_all_swap(table);
                }
            }
        }

        for canonical in &ff_write_canonical {
            if self_ref.contains(canonical) {
                continue;
            }
            // FF write: REVERSED edge (var → stmt)
            let var_node = Node::Var(true, *canonical);
            let var = *dag_nodes
                .entry(var_node)
                .or_insert_with(|| dag.add_node(var_node));
            if dag.add_edge(var, stmt, ()).is_err() {
                return fallback_all_swap(table);
            }
        }

        // Comb writes: normal edge (stmt → var)
        for (is_ff, offset) in &outputs {
            if !*is_ff {
                let var_node = Node::Var(false, *offset);
                let var = *dag_nodes
                    .entry(var_node)
                    .or_insert_with(|| dag.add_node(var_node));
                if dag.add_edge(stmt, var, ()).is_err() {
                    return fallback_all_swap(table);
                }
            }
        }
    }

    match algo::toposort(dag.graph(), None) {
        Ok(nodes) => {
            let mut sorted = vec![];
            for i in nodes {
                if let Node::Statement(x) = dag[i] {
                    sorted.push(table.remove(&x).unwrap());
                }
            }

            // Walk sorted order to find read-after-write violations.
            let mut written_ff = HashSet::default();
            let mut needs_swap = HashSet::default();

            for stmt in &sorted {
                let mut inputs = vec![];
                let mut outputs = vec![];
                stmt.gather_variable_offsets(&mut inputs, &mut outputs);

                for (is_ff, offset) in &inputs {
                    if *is_ff && written_ff.contains(offset) {
                        needs_swap.insert(*offset);
                    }
                }

                let canonical = stmt.gather_ff_canonical_offsets();
                for off in canonical {
                    written_ff.insert(off);
                }
            }

            // Rewrite non-swap FF assignments: dst_offset next → current
            let sorted: Vec<_> = sorted
                .into_iter()
                .map(|stmt| rewrite_ff_direct(stmt, &needs_swap))
                .collect();

            (sorted, needs_swap)
        }
        Err(_) => fallback_all_swap(table),
    }
}

/// Fallback: all FF variables need swap (no optimization).
fn fallback_all_swap(
    table: HashMap<usize, ProtoStatement>,
) -> (Vec<ProtoStatement>, HashSet<isize>) {
    let mut all_ff_offsets = HashSet::default();
    for x in table.values() {
        for off in x.gather_ff_canonical_offsets() {
            all_ff_offsets.insert(off);
        }
    }
    let stmts: Vec<_> = table.into_values().collect();
    (stmts, all_ff_offsets)
}

/// Rewrite a ProtoStatement: for FF assignments whose canonical offset
/// is NOT in needs_swap, change dst_offset from next to current.
fn rewrite_ff_direct(stmt: ProtoStatement, needs_swap: &HashSet<isize>) -> ProtoStatement {
    match stmt {
        ProtoStatement::Assign(mut x) => {
            if x.dst_is_ff {
                let canonical = x.dst_ff_current_offset;
                if !needs_swap.contains(&canonical) {
                    x.dst_offset = canonical;
                }
            }
            ProtoStatement::Assign(x)
        }
        ProtoStatement::AssignDynamic(mut x) => {
            if x.dst_is_ff {
                let canonical = x.dst_ff_current_base_offset;
                if !needs_swap.contains(&canonical) {
                    x.dst_base_offset = canonical;
                }
            }
            ProtoStatement::AssignDynamic(x)
        }
        ProtoStatement::If(mut x) => {
            x.true_side = x
                .true_side
                .into_iter()
                .map(|s| rewrite_ff_direct(s, needs_swap))
                .collect();
            x.false_side = x
                .false_side
                .into_iter()
                .map(|s| rewrite_ff_direct(s, needs_swap))
                .collect();
            ProtoStatement::If(x)
        }
        other => other,
    }
}

/// Compute dependency levels for sorted ProtoStatements and reorder within
/// each level so that CompiledBlocks with the same func pointer are adjacent.
/// This enables batching of same-function JIT calls.
fn reorder_by_level(sorted: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    // Compute level for each statement:
    // level = max(var_level[input] for each known input) + 1, or 0 if no known inputs
    let mut var_level: HashMap<(bool, isize), usize> = HashMap::default();
    let mut levels: Vec<usize> = Vec::with_capacity(sorted.len());

    for stmt in &sorted {
        let mut inputs = vec![];
        let mut outputs = vec![];
        stmt.gather_variable_offsets(&mut inputs, &mut outputs);

        let level = inputs
            .iter()
            .filter_map(|key| var_level.get(key))
            .copied()
            .max()
            .map(|l| l + 1)
            .unwrap_or(0);

        for key in &outputs {
            let e = var_level.entry(*key).or_insert(0);
            if level > *e {
                *e = level;
            }
        }

        levels.push(level);
    }

    // Group statements by level
    let max_level = levels.iter().copied().max().unwrap_or(0);
    let mut groups: Vec<Vec<ProtoStatement>> = vec![vec![]; max_level + 1];
    for (stmt, level) in sorted.into_iter().zip(levels) {
        groups[level].push(stmt);
    }

    // Within each level, sort so CompiledBlocks with same func are adjacent
    for group in &mut groups {
        group.sort_by_key(|stmt| match stmt {
            ProtoStatement::CompiledBlock(x) => (0, x.func as usize),
            ProtoStatement::Assign(_) => (1, 0),
            ProtoStatement::AssignDynamic(_) => (2, 0),
            ProtoStatement::If(_) => (3, 0),
            ProtoStatement::SystemFunctionCall(_) => (4, 0),
            ProtoStatement::TbMethodCall { .. } => (4, 0),
        });
    }

    groups.into_iter().flatten().collect()
}

/// Merge consecutive Binary statements with the same function pointer into BinaryBatch.
fn batch_binary_statements(stmts: Vec<Statement>) -> Vec<Statement> {
    let mut result: Vec<Statement> = Vec::with_capacity(stmts.len());

    for stmt in stmts {
        match stmt {
            Statement::Binary(func, ff, comb) => {
                let func_addr = func as usize;
                match result.last_mut() {
                    Some(Statement::BinaryBatch(batch_func, args))
                        if *batch_func as usize == func_addr =>
                    {
                        args.push((ff, comb));
                    }
                    Some(Statement::Binary(prev_func, prev_ff, prev_comb))
                        if *prev_func as usize == func_addr =>
                    {
                        let prev_ff = *prev_ff;
                        let prev_comb = *prev_comb;
                        let prev_func = *prev_func;
                        *result.last_mut().unwrap() = Statement::BinaryBatch(
                            prev_func,
                            vec![(prev_ff, prev_comb), (ff, comb)],
                        );
                    }
                    _ => {
                        result.push(Statement::Binary(func, ff, comb));
                    }
                }
            }
            other => result.push(other),
        }
    }

    result
}

impl Conv<&air::Module> for ProtoModule {
    fn conv(context: &mut Context, src: &air::Module) -> Result<Self, SimulatorError> {
        let mut analyzer_context = veryl_analyzer::conv::Context::default();
        analyzer_context.variables = src.variables.clone();
        analyzer_context.functions = src.functions.clone();

        let mut ff_table = air::FfTable::default();
        src.gather_ff(&mut analyzer_context, &mut ff_table);
        ff_table.update_is_ff();

        let ff_start = context.ff_total_bytes as isize;
        let comb_start = context.comb_total_bytes as isize;
        let (variable_meta, ff_bytes, comb_bytes) = create_variable_meta(
            &src.variables,
            &ff_table,
            context.config.use_4state,
            ff_start,
            comb_start,
        )
        .unwrap();

        context.ff_total_bytes += ff_bytes;
        context.comb_total_bytes += comb_bytes;

        let scope = ScopeContext {
            variable_meta: variable_meta.clone(),
            analyzer_context,
        };
        context.scope_contexts.push(scope);

        let mut all_event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        let mut all_comb_statements: Vec<ProtoStatement> = vec![];
        let mut all_post_comb_fns: Vec<ProtoStatement> = vec![];
        let mut all_child_modules: Vec<ModuleVariableMeta> = vec![];
        let mut has_merged = false;
        // Collect full internal comb for sub-modules that use merged comb+event
        let mut full_comb_extra: Vec<ProtoStatement> = vec![];

        for decl in &src.declarations {
            let proto_decl: ProtoDeclaration = Conv::conv(context, decl)?;

            for (event, mut stmts) in proto_decl.event_statements {
                all_event_statements
                    .entry(event)
                    .and_modify(|v| v.append(&mut stmts))
                    .or_insert(stmts);
            }
            if let Some(full_comb) = proto_decl.full_internal_comb {
                has_merged = true;
                full_comb_extra.extend(full_comb);
            }
            all_comb_statements.append(&mut proto_decl.comb_statements.clone());
            all_post_comb_fns.extend(proto_decl.post_comb_fns);
            all_child_modules.extend(proto_decl.child_modules);
        }

        // Add comb statements that depend on post_comb output port
        // destinations. This handles cross-child dependencies:
        // ChildA comb → output port → parent var → ChildB input port.
        // Only Assign outputs are tracked (not CompiledBlock internals).
        if !all_post_comb_fns.is_empty() {
            let mut port_outputs = HashSet::default();
            for s in &all_post_comb_fns {
                if !matches!(s, ProtoStatement::CompiledBlock(_)) {
                    let mut outputs: Vec<(bool, isize)> = vec![];
                    let mut inputs: Vec<(bool, isize)> = vec![];
                    s.gather_variable_offsets(&mut inputs, &mut outputs);
                    port_outputs.extend(outputs);
                }
            }
            if !port_outputs.is_empty() {
                for stmt in &all_comb_statements {
                    let mut inputs: Vec<(bool, isize)> = vec![];
                    let mut outputs: Vec<(bool, isize)> = vec![];
                    stmt.gather_variable_offsets(&mut inputs, &mut outputs);
                    if inputs.iter().any(|i| port_outputs.contains(i)) {
                        all_post_comb_fns.push(stmt.clone());
                    }
                }
            }
        }

        context.scope_contexts.pop();

        // Save lite comb for full_comb construction (before analyze_dependency consumes it)
        let lite_comb_copy = if has_merged {
            Some(all_comb_statements.clone())
        } else {
            None
        };

        let sorted_comb = match analyze_dependency(all_comb_statements) {
            Ok(sorted) => sorted,
            Err(err) => {
                return Err(err);
            }
        };

        // Collect comb offsets from named variables (ports, user-visible state).
        // These must NOT be eliminated by DFG optimization since they are
        // observable via sim.get() / dump_variables().
        let mut observable_comb = HashSet::default();
        for meta in variable_meta.values() {
            for elem in &meta.elements {
                if !elem.is_ff {
                    observable_comb.insert(elem.current_offset);
                }
            }
        }

        let sorted_comb = super::optimize::optimize_comb(
            sorted_comb,
            &all_event_statements,
            &observable_comb,
            context.config.use_jit,
        );
        let sorted_comb = reorder_by_level(sorted_comb);

        let comb_statements = try_jit(context, sorted_comb);

        // Build full comb statements (including per-core internal comb)
        // when merged comb+event is used. get()/dump() needs this for correctness.
        let full_comb_statements = if has_merged {
            let mut full = full_comb_extra;
            full.extend(lite_comb_copy.unwrap());
            let full_sorted = match analyze_dependency(full) {
                Ok(sorted) => sorted,
                Err(err) => {
                    return Err(err);
                }
            };
            let full_sorted = super::optimize::optimize_comb(
                full_sorted,
                &all_event_statements,
                &observable_comb,
                context.config.use_jit,
            );
            let full_sorted = reorder_by_level(full_sorted);
            Some(try_jit(context, full_sorted))
        } else {
            None
        };

        // Sort FF event statements and determine selective swap set
        let mut all_swap_offsets = HashSet::default();

        let event_statements: HashMap<Event, ProtoStatements> = all_event_statements
            .into_iter()
            .map(|(event, stmts)| {
                // Initial/Final events should preserve source order (no FF dependency sorting)
                if matches!(event, Event::Initial | Event::Final) {
                    (event, try_jit(context, stmts))
                } else {
                    let (sorted, swap_offsets) = sort_ff_event(stmts);
                    if !swap_offsets.is_empty() {
                        all_swap_offsets.extend(&swap_offsets);
                    }
                    (event, try_jit(context, sorted))
                }
            })
            .collect();

        let ff_swap_offsets = Some(all_swap_offsets);

        let module_variable_meta = ModuleVariableMeta {
            name: src.name,
            variable_meta,
            children: all_child_modules,
        };

        Ok(ProtoModule {
            name: src.name,
            ports: src.ports.clone(),
            ff_bytes: context.ff_total_bytes,
            comb_bytes: context.comb_total_bytes,
            use_4state: context.config.use_4state,
            module_variable_meta,
            event_statements,
            comb_statements,
            post_comb_fns: all_post_comb_fns,
            full_comb_statements,
            ff_swap_offsets,
        })
    }
}
