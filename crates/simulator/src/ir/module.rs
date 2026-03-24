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
    /// When true, settle_comb() uses full_comb_statements (the combined
    /// comb + post_comb list) instead of lite comb_statements alone.
    pub use_full_comb_in_step: bool,
    /// Number of eval_comb passes needed for full convergence.
    /// Pre-computed from backward edges in the sorted comb statement list.
    pub required_comb_passes: usize,
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
    pub use_full_comb_in_step: bool,
    /// Number of eval_comb passes needed for full convergence.
    pub required_comb_passes: usize,
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
            use_full_comb_in_step: self.use_full_comb_in_step,
            required_comb_passes: self.required_comb_passes,
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

    // First attempt: standard DAG analysis (CompiledBlocks as single nodes).
    // Track which statement IDs are involved in the cycle.
    {
        let mut dag = Dag::<Node, ()>::new();
        let mut dag_nodes: HashMap<Node, _> = HashMap::default();
        let mut failed_id: Option<usize> = None;

        for (id, x) in &table {
            let mut inputs = vec![];
            let mut outputs = vec![];
            x.gather_variable_offsets(&mut inputs, &mut outputs);
            let stmt_node = Node::Statement(*id);
            let stmt = dag.add_node(stmt_node);
            dag_nodes.insert(stmt_node, stmt);

            let output_set: HashSet<(bool, isize)> = outputs.iter().cloned().collect();
            let mut ok = true;
            for var_key in inputs {
                if output_set.contains(&var_key) {
                    continue;
                }
                let var_node = Node::Var(var_key.0, var_key.1);
                let var = *dag_nodes
                    .entry(var_node)
                    .or_insert_with(|| dag.add_node(var_node));
                if dag.add_edge(var, stmt, ()).is_err() {
                    ok = false;
                    break;
                }
            }
            if !ok {
                failed_id = Some(*id);
                break;
            }
            for var_key in outputs {
                let var_node = Node::Var(var_key.0, var_key.1);
                let var = *dag_nodes
                    .entry(var_node)
                    .or_insert_with(|| dag.add_node(var_node));
                if dag.add_edge(stmt, var, ()).is_err() {
                    ok = false;
                    break;
                }
            }
            if !ok {
                failed_id = Some(*id);
                break;
            }
        }

        if failed_id.is_none() {
            let nodes = algo::toposort(dag.graph(), None).unwrap();
            let mut ret = vec![];
            let mut t = table.clone();
            for i in nodes {
                if let Node::Statement(x) = dag[i]
                    && let Some(s) = t.remove(&x)
                {
                    ret.push(s);
                }
            }
            return Ok(ret);
        }
        // Cycle detected. Check if fine-grained deps are available for retry.
        let has_expandable = table.values().any(
            |x| matches!(x, ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty()),
        );
        if !has_expandable {
            // No expandable CompiledBlocks. If any CBs exist at all (e.g. from
            // shared JIT cache without original_stmts), skip to relaxed ordering
            // instead of reporting a false-positive combinational loop.
            let has_any_cb = table
                .values()
                .any(|x| matches!(x, ProtoStatement::CompiledBlock(_)));
            if !has_any_cb {
                // Genuine loop, no CBs involved — re-do analysis for error message.
                let mut dag2 = Dag::<Node, ()>::new();
                let mut dag_nodes2: HashMap<Node, _> = HashMap::default();
                for (id, x) in &table {
                    let mut inputs = vec![];
                    let mut outputs = vec![];
                    x.gather_variable_offsets(&mut inputs, &mut outputs);
                    let stmt_node = Node::Statement(*id);
                    let stmt = dag2.add_node(stmt_node);
                    dag_nodes2.insert(stmt_node, stmt);

                    let output_set: HashSet<(bool, isize)> = outputs.iter().cloned().collect();
                    for var_key in inputs {
                        if output_set.contains(&var_key) {
                            continue;
                        }
                        let var_node = Node::Var(var_key.0, var_key.1);
                        let var = *dag_nodes2
                            .entry(var_node)
                            .or_insert_with(|| dag2.add_node(var_node));
                        if dag2.add_edge(var, stmt, ()).is_err() {
                            let participant_tokens = collect_cycle_tokens(&dag2, &table, stmt, *id);
                            let trigger_token = table[id].token().unwrap_or_default();
                            return Err(SimulatorError::combinational_loop(
                                &trigger_token,
                                &participant_tokens,
                            ));
                        }
                    }
                    for var_key in outputs {
                        let var_node = Node::Var(var_key.0, var_key.1);
                        let var = *dag_nodes2
                            .entry(var_node)
                            .or_insert_with(|| dag2.add_node(var_node));
                        if dag2.add_edge(stmt, var, ()).is_err() {
                            let participant_tokens = collect_cycle_tokens(&dag2, &table, var, *id);
                            let trigger_token = table[id].token().unwrap_or_default();
                            return Err(SimulatorError::combinational_loop(
                                &trigger_token,
                                &participant_tokens,
                            ));
                        }
                    }
                }
                unreachable!("cycle was detected but re-analysis succeeded");
            }
        } else {
            log::debug!("analyze_dependency: cycle detected, retrying with fine-grained deps ({} CompiledBlocks with original_stmts)",
                table.values().filter(|x| matches!(x, ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty())).count());
        }
    }

    // Expand all CompiledBlocks with original_stmts at once (not one at
    // a time, which depended on HashMap iteration order for CB selection).
    let mut next_id = table.keys().max().copied().unwrap_or(0) + 1;
    let expandable_ids: Vec<usize> = table
        .iter()
        .filter_map(|(id, x)| {
            if matches!(x, ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty()) {
                Some(*id)
            } else {
                None
            }
        })
        .collect();

    log::debug!(
        "analyze_dependency: expanding {} CompiledBlocks with original_stmts",
        expandable_ids.len()
    );

    for eid in expandable_ids {
        if let Some(ProtoStatement::CompiledBlock(cb)) = table.remove(&eid) {
            for stmt in cb.original_stmts {
                table.insert(next_id, stmt);
                next_id += 1;
            }
        }
    }

    // Retry DAG analysis with all CBs expanded.
    // Use sorted keys for deterministic iteration order.
    {
        let mut dag = Dag::<Node, ()>::new();
        let mut dag_nodes_inner: HashMap<Node, _> = HashMap::default();
        let mut failed_id: Option<usize> = None;

        let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
        sorted_keys.sort();

        for id in &sorted_keys {
            let x = &table[id];
            let mut inputs = vec![];
            let mut outputs = vec![];
            x.gather_variable_offsets(&mut inputs, &mut outputs);

            let stmt_node = Node::Statement(*id);
            let stmt = dag.add_node(stmt_node);
            dag_nodes_inner.insert(stmt_node, stmt);

            let output_set: HashSet<(bool, isize)> = outputs.iter().cloned().collect();
            let mut ok = true;
            for var_key in inputs {
                if output_set.contains(&var_key) {
                    continue;
                }
                let var_node = Node::Var(var_key.0, var_key.1);
                let var = *dag_nodes_inner
                    .entry(var_node)
                    .or_insert_with(|| dag.add_node(var_node));
                if dag.add_edge(var, stmt, ()).is_err() {
                    ok = false;
                    break;
                }
            }
            if !ok {
                failed_id = Some(*id);
                break;
            }
            for var_key in outputs {
                let var_node = Node::Var(var_key.0, var_key.1);
                let var = *dag_nodes_inner
                    .entry(var_node)
                    .or_insert_with(|| dag.add_node(var_node));
                if dag.add_edge(stmt, var, ()).is_err() {
                    ok = false;
                    break;
                }
            }
            if !ok {
                failed_id = Some(*id);
                break;
            }
        }

        if failed_id.is_none() {
            let nodes = algo::toposort(dag.graph(), None).unwrap();
            let mut ret = vec![];
            for i in nodes {
                if let Node::Statement(x) = dag[i]
                    && let Some(stmt) = table.remove(&x)
                {
                    ret.push(stmt);
                }
            }
            return Ok(ret);
        }
    }

    // Still have a cycle after expanding all CBs.
    // Check if non-expandable CompiledBlocks remain (false positive from
    // aggregate input/output overlap).
    let has_non_expandable_cb = table
        .values()
        .any(|x| matches!(x, ProtoStatement::CompiledBlock(cb) if cb.original_stmts.is_empty()));

    if !has_non_expandable_cb {
        // Genuine combinational loop (no CompiledBlocks involved).
        // Re-run with sorted keys to get deterministic error.
        let mut dag2 = Dag::<Node, ()>::new();
        let mut dag_nodes2: HashMap<Node, _> = HashMap::default();
        let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
        sorted_keys.sort();
        for id in &sorted_keys {
            let x = &table[id];
            let mut inputs = vec![];
            let mut outputs = vec![];
            x.gather_variable_offsets(&mut inputs, &mut outputs);
            let stmt_node = Node::Statement(*id);
            let stmt = dag2.add_node(stmt_node);
            dag_nodes2.insert(stmt_node, stmt);
            let output_set: HashSet<(bool, isize)> = outputs.iter().cloned().collect();
            for var_key in inputs {
                if output_set.contains(&var_key) {
                    continue;
                }
                let var_node = Node::Var(var_key.0, var_key.1);
                let var = *dag_nodes2
                    .entry(var_node)
                    .or_insert_with(|| dag2.add_node(var_node));
                if dag2.add_edge(var, stmt, ()).is_err() {
                    let participant_tokens = collect_cycle_tokens(&dag2, &table, stmt, *id);
                    let trigger_token = table[id].token().unwrap_or_default();
                    return Err(SimulatorError::combinational_loop(
                        &trigger_token,
                        &participant_tokens,
                    ));
                }
            }
            for var_key in outputs {
                let var_node = Node::Var(var_key.0, var_key.1);
                let var = *dag_nodes2
                    .entry(var_node)
                    .or_insert_with(|| dag2.add_node(var_node));
                if dag2.add_edge(stmt, var, ()).is_err() {
                    let participant_tokens = collect_cycle_tokens(&dag2, &table, var, *id);
                    let trigger_token = table[id].token().unwrap_or_default();
                    return Err(SimulatorError::combinational_loop(
                        &trigger_token,
                        &participant_tokens,
                    ));
                }
            }
        }
        return Err(SimulatorError::combinational_loop(
            &TokenRange::default(),
            &[],
        ));
    }

    // Build a relaxed DAG: skip edges that would create cycles when
    // at least one endpoint is a non-expandable CompiledBlock.
    log::debug!(
        "analyze_dependency: using relaxed ordering for {} stmts with non-expandable CompiledBlocks",
        table.len()
    );
    let mut dag_relaxed = Dag::<Node, ()>::new();
    let mut dag_nodes_relaxed: HashMap<Node, _> = HashMap::default();
    let cb_ids: HashSet<usize> = table
        .iter()
        .filter_map(|(id, x)| {
            if matches!(x, ProtoStatement::CompiledBlock(_)) {
                Some(*id)
            } else {
                None
            }
        })
        .collect();

    let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
    sorted_keys.sort();
    for id in &sorted_keys {
        let x = &table[id];
        let mut inputs = vec![];
        let mut outputs = vec![];
        x.gather_variable_offsets(&mut inputs, &mut outputs);
        let stmt_node = Node::Statement(*id);
        let stmt = dag_relaxed.add_node(stmt_node);
        dag_nodes_relaxed.insert(stmt_node, stmt);

        let output_set: HashSet<(bool, isize)> = outputs.iter().cloned().collect();
        for var_key in &inputs {
            if output_set.contains(var_key) {
                continue;
            }
            let var_node = Node::Var(var_key.0, var_key.1);
            let var = *dag_nodes_relaxed
                .entry(var_node)
                .or_insert_with(|| dag_relaxed.add_node(var_node));
            if dag_relaxed.add_edge(var, stmt, ()).is_err() {
                if cb_ids.contains(id) {
                    continue;
                }
                let written_by_cb = table.iter().any(|(oid, ox)| {
                    cb_ids.contains(oid) && {
                        let mut o_outs = vec![];
                        let mut o_ins = vec![];
                        ox.gather_variable_offsets(&mut o_ins, &mut o_outs);
                        o_outs.contains(var_key)
                    }
                });
                if written_by_cb {
                    continue;
                }
                log::debug!("analyze_dependency: skipping non-CB cyclic input edge");
            }
        }
        for var_key in &outputs {
            let var_node = Node::Var(var_key.0, var_key.1);
            let var = *dag_nodes_relaxed
                .entry(var_node)
                .or_insert_with(|| dag_relaxed.add_node(var_node));
            if dag_relaxed.add_edge(stmt, var, ()).is_err() {
                if cb_ids.contains(id) {
                    continue;
                }
                let read_by_cb = table.iter().any(|(oid, ox)| {
                    cb_ids.contains(oid) && {
                        let mut o_outs = vec![];
                        let mut o_ins = vec![];
                        ox.gather_variable_offsets(&mut o_ins, &mut o_outs);
                        o_ins.contains(var_key)
                    }
                });
                if read_by_cb {
                    continue;
                }
                log::debug!("analyze_dependency: skipping non-CB cyclic output edge");
            }
        }
    }

    let nodes = algo::toposort(dag_relaxed.graph(), None).unwrap();
    let mut ret = vec![];
    for i in nodes {
        if let Node::Statement(x) = dag_relaxed[i]
            && let Some(stmt) = table.remove(&x)
        {
            ret.push(stmt);
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
/// Returns (sorted_statements, needs_swap_offsets, force_all_swap).
/// force_all_swap=true means ff_swap_offsets should be None.
pub(crate) fn sort_ff_event(
    statements: Vec<ProtoStatement>,
) -> (Vec<ProtoStatement>, HashSet<isize>, bool) {
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
        return (vec![], HashSet::default(), false);
    }

    let mut dag = Dag::<Node, ()>::new();
    let mut dag_nodes: HashMap<Node, _> = HashMap::default();
    // Track self-referencing FF offsets (read + write same variable)
    let mut self_ref_all: HashSet<isize> = HashSet::default();

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
        self_ref_all.extend(&self_ref);

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
                    return fallback_preserve_order(table);
                }
            } else {
                let var_node = Node::Var(false, *offset);
                let var = *dag_nodes
                    .entry(var_node)
                    .or_insert_with(|| dag.add_node(var_node));
                if dag.add_edge(var, stmt, ()).is_err() {
                    return fallback_preserve_order(table);
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
                return fallback_preserve_order(table);
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
                    return fallback_preserve_order(table);
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

            // Self-referencing FF variables always need swap.
            let mut needs_swap: HashSet<isize> = self_ref_all;

            // CompiledBlock FF writes are opaque: always need swap.
            // Use ff_canonical_offsets (current offsets) for correct ff_swap matching.
            for stmt in &sorted {
                if let ProtoStatement::CompiledBlock(cb) = stmt {
                    for off in &cb.ff_canonical_offsets {
                        needs_swap.insert(*off);
                    }
                }
            }

            // Walk sorted order to find additional read-after-write violations.
            let mut written_ff = HashSet::default();

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

            (sorted, needs_swap, false)
        }
        Err(_) => fallback_preserve_order(table),
    }
}

/// Fallback: preserve original statement order when DAG has cycles.
/// Compute needs_swap using the same logic as the success path so that
/// rewrite_ff_direct can still convert non-swap FF assignments to direct writes.
fn fallback_preserve_order(
    table: HashMap<usize, ProtoStatement>,
) -> (Vec<ProtoStatement>, HashSet<isize>, bool) {
    // Sort by key (insertion index) to preserve source order
    let mut entries: Vec<_> = table.into_iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    let stmts: Vec<_> = entries.into_iter().map(|(_, v)| v).collect();

    // Self-referencing FF variables need swap
    let mut needs_swap: HashSet<isize> = HashSet::default();
    for stmt in &stmts {
        let mut inputs = vec![];
        let mut outputs = vec![];
        stmt.gather_variable_offsets(&mut inputs, &mut outputs);
        let ff_write_canonical: HashSet<isize> = stmt.gather_ff_canonical_offsets();
        let ff_read_offsets: HashSet<isize> = inputs
            .iter()
            .filter(|(is_ff, _)| *is_ff)
            .map(|(_, off)| *off)
            .collect();
        for off in ff_read_offsets.intersection(&ff_write_canonical) {
            needs_swap.insert(*off);
        }
    }

    // CompiledBlock FF writes are opaque: always need swap
    for stmt in &stmts {
        if let ProtoStatement::CompiledBlock(cb) = stmt {
            for off in &cb.ff_canonical_offsets {
                needs_swap.insert(*off);
            }
        }
    }

    // Walk source order to find read-after-write violations
    let mut written_ff = HashSet::default();
    for stmt in &stmts {
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
    let stmts: Vec<_> = stmts
        .into_iter()
        .map(|stmt| rewrite_ff_direct(stmt, &needs_swap))
        .collect();

    (stmts, needs_swap, false)
}

/// Rewrite a ProtoStatement: for FF assignments whose canonical offset
/// is NOT in needs_swap, change dst_offset from next to current.
pub(crate) fn rewrite_ff_direct(
    stmt: ProtoStatement,
    needs_swap: &HashSet<isize>,
) -> ProtoStatement {
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

/// Compute the number of eval_comb passes required for convergence.
///
/// After topological sorting, some dependency edges may be "backward"
/// (a statement reads a variable written by a later statement in the
/// sorted order). Each backward edge requires an additional eval pass
/// to propagate the correct value. The required number of passes is
/// the longest chain of backward edges + 1.
///
/// Since backward edges always point from higher positions to lower
/// positions, they form a DAG. A single reverse scan computes the
/// longest chain in O(N) time.
fn compute_required_passes(sorted: &[ProtoStatement]) -> usize {
    let n = sorted.len();
    if n == 0 {
        return 1;
    }

    // For each comb variable, record the position of its last writer.
    let mut var_last_writer: HashMap<(bool, isize), usize> = HashMap::default();
    for (pos, stmt) in sorted.iter().enumerate() {
        let mut inputs = vec![];
        let mut outputs = vec![];
        stmt.gather_variable_offsets(&mut inputs, &mut outputs);
        for key in outputs {
            var_last_writer.insert(key, pos);
        }
    }

    // Reverse scan: for each statement, compute the maximum backward
    // chain depth. Because backward edges point from higher to lower
    // positions, delay[writer_pos] is already computed when we visit pos.
    let mut delay = vec![0usize; n];
    for pos in (0..n).rev() {
        let mut inputs = vec![];
        let mut outputs = vec![];
        sorted[pos].gather_variable_offsets(&mut inputs, &mut outputs);
        let output_set: HashSet<(bool, isize)> = outputs.iter().cloned().collect();

        for key in &inputs {
            if output_set.contains(key) {
                continue; // self-reference within same statement
            }
            if let Some(&writer_pos) = var_last_writer.get(key)
                && writer_pos > pos
            {
                delay[pos] = delay[pos].max(delay[writer_pos] + 1);
            }
        }
    }

    let max_delay = delay.iter().copied().max().unwrap_or(0);
    let passes = max_delay + 1;
    if passes > 1 {
        log::info!(
            "compute_required_passes: {} passes needed ({} stmts, {} backward edge chain depth)",
            passes,
            n,
            max_delay
        );
    }
    passes
}

/// Compute dependency levels for sorted ProtoStatements and reorder within
/// each level so that CompiledBlocks with the same func pointer are adjacent.
/// This enables batching of same-function JIT calls.
fn reorder_by_level(sorted: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    // Level = max(var_level[input]) + 1. For CBs, use all offsets
    // (including FF) since gather_variable_offsets filters FF for DAG.
    let mut var_level: HashMap<(bool, isize), usize> = HashMap::default();
    let mut levels: Vec<usize> = Vec::with_capacity(sorted.len());

    for stmt in &sorted {
        let mut inputs = vec![];
        let mut outputs = vec![];
        match stmt {
            ProtoStatement::CompiledBlock(x) => {
                if !x.stmt_deps.is_empty() {
                    for (ins, outs) in &x.stmt_deps {
                        inputs.extend_from_slice(ins);
                        outputs.extend_from_slice(outs);
                    }
                } else {
                    inputs.extend_from_slice(&x.input_offsets);
                    outputs.extend_from_slice(&x.output_offsets);
                }
            }
            _ => {
                stmt.gather_variable_offsets(&mut inputs, &mut outputs);
            }
        }

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

    // Within each level, topological sort by actual variable dependencies.
    for group in groups.iter_mut() {
        if group.len() <= 1 {
            continue;
        }
        *group = topo_sort_within_level(std::mem::take(group));
    }

    groups.into_iter().flatten().collect()
}

/// Local topological sort within a single level group.
///
/// Builds RAW dependency edges among the statements in this group and
/// performs a stable Kahn's-algorithm sort.  Statements with no intra-group
/// dependencies retain their original order (stable).  On cycle detection
/// the original order is preserved as a safe fallback.
fn topo_sort_within_level(stmts: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    let n = stmts.len();

    // Type priority for tie-breaking: CBs before Assigns when unordered.
    let type_priority: Vec<u8> = stmts
        .iter()
        .map(|s| match s {
            ProtoStatement::CompiledBlock(_) => 0,
            ProtoStatement::Assign(_) => 1,
            ProtoStatement::AssignDynamic(_) => 2,
            ProtoStatement::If(_) => 3,
            _ => 4,
        })
        .collect();

    let mut stmt_inputs: Vec<Vec<(bool, isize)>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<(bool, isize)>> = Vec::with_capacity(n);
    for s in &stmts {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    let mut var_writers: HashMap<(bool, isize), Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        for key in outs {
            var_writers.entry(*key).or_default().push(i);
        }
    }

    // RAW edges: writer → reader (skip self-edges).
    let mut adj: Vec<HashSet<usize>> = vec![HashSet::default(); n];
    let mut in_degree = vec![0usize; n];
    for (reader, ins) in stmt_inputs.iter().enumerate() {
        for key in ins {
            if let Some(writers) = var_writers.get(key) {
                for &writer in writers {
                    if writer == reader {
                        continue; // skip self-edge
                    }
                    if adj[writer].insert(reader) {
                        in_degree[reader] += 1;
                    }
                }
            }
        }
    }

    // Kahn's with BTreeSet<(priority, index)> for stable tie-breaking.
    let mut queue: std::collections::BTreeSet<(u8, usize)> = std::collections::BTreeSet::new();
    for i in 0..n {
        if in_degree[i] == 0 {
            queue.insert((type_priority[i], i));
        }
    }

    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(&key) = queue.iter().next() {
        queue.remove(&key);
        let idx = key.1;
        order.push(idx);
        for &next in &adj[idx] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                queue.insert((type_priority[next], next));
            }
        }
    }

    if order.len() != n {
        let mut result = stmts;
        result.sort_by_key(|s| match s {
            ProtoStatement::CompiledBlock(_) => 0,
            ProtoStatement::Assign(_) => 1,
            ProtoStatement::AssignDynamic(_) => 2,
            ProtoStatement::If(_) => 3,
            _ => 4,
        });
        return result;
    }

    let mut indexed: Vec<Option<ProtoStatement>> = stmts.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|i| indexed[i].take().unwrap())
        .collect()
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

        // Transitively add comb statements that depend (directly or
        // indirectly) on post_comb_fns outputs. This handles multi-hop
        // dependency chains: ChildA comb (post_comb) → parent var →
        // parent comb → ChildB input port → ChildB comb (comb).
        // Without transitive closure, the 3-pass settling
        // (post_comb → comb → post_comb) fails when the chain crosses
        // the post_comb/comb boundary more than once.
        if !all_post_comb_fns.is_empty() {
            // Collect all outputs from existing post_comb entries
            let mut post_comb_outputs = HashSet::default();
            for s in &all_post_comb_fns {
                let mut outputs: Vec<(bool, isize)> = vec![];
                let mut inputs: Vec<(bool, isize)> = vec![];
                s.gather_variable_offsets(&mut inputs, &mut outputs);
                post_comb_outputs.extend(outputs);
            }

            // Repeatedly scan comb_statements for dependencies until
            // no new statements are added (transitive closure).
            let mut added_indices = HashSet::default();
            loop {
                let mut changed = false;
                for (idx, stmt) in all_comb_statements.iter().enumerate() {
                    if added_indices.contains(&idx) {
                        continue;
                    }
                    let mut inputs: Vec<(bool, isize)> = vec![];
                    let mut outputs: Vec<(bool, isize)> = vec![];
                    stmt.gather_variable_offsets(&mut inputs, &mut outputs);
                    if inputs.iter().any(|i| post_comb_outputs.contains(i)) {
                        all_post_comb_fns.push(stmt.clone());
                        post_comb_outputs.extend(outputs);
                        added_indices.insert(idx);
                        changed = true;
                    }
                }
                if !changed {
                    break;
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

        let sorted_comb = match analyze_dependency(all_comb_statements.clone()) {
            Ok(sorted) => sorted,
            Err(err) => {
                log::warn!(
                    "analyze_dependency failed for comb ({} stmts) of {:?}: {:?}",
                    all_comb_statements.len(),
                    src.name,
                    err
                );
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
        // Also mark comb offsets read by post_comb_fns as observable.
        // post_comb_fns contain child module comb (CompiledBlocks and
        // port connection assigns). Without this, optimize_comb may
        // inline/DCE comb assigns that post_comb_fns depend on.
        for s in &all_post_comb_fns {
            let mut inputs = vec![];
            let mut outputs = vec![];
            s.gather_variable_offsets(&mut inputs, &mut outputs);
            for (is_ff, off) in inputs {
                if !is_ff {
                    observable_comb.insert(off);
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
        // when merged comb+event is used or when post_comb_fns exist.
        // get()/dump() needs this for correctness after FF swap.
        let mut use_full_comb_in_step = false;
        let mut required_comb_passes = 1usize;
        let full_comb_statements = if has_merged {
            // Decompose CompiledBlocks with original_stmts to avoid false
            // dependency cycles in analyze_dependency.
            let mut full = Vec::new();
            for s in full_comb_extra
                .into_iter()
                .chain(lite_comb_copy.unwrap().into_iter())
            {
                if let ProtoStatement::CompiledBlock(cb) = &s
                    && !cb.original_stmts.is_empty()
                {
                    full.extend(cb.original_stmts.iter().cloned());
                    continue;
                }
                full.push(s);
            }
            let full_sorted = match analyze_dependency(full.clone()) {
                Ok(sorted) => {
                    // Successfully sorted: use in step() for correct
                    // evaluation order across deep comb chains
                    // (e.g., dcache → MMU → memory).
                    use_full_comb_in_step = true;
                    sorted
                }
                Err(_) => {
                    // False cycle detected (often from CompiledBlocks bundling
                    // FF and comb offsets). Use the unsorted list in step()
                    // anyway — iterative evaluation will converge for designs
                    // without genuine comb loops.
                    use_full_comb_in_step = true;
                    full
                }
            };
            // Skip optimize_comb: the full list includes internal comb
            // from merged functions that DCE would incorrectly remove.
            let full_sorted = reorder_by_level(full_sorted);
            required_comb_passes = compute_required_passes(&full_sorted);
            // Keep full_comb interpreted: JIT load_cache forwards stores
            // within a single compiled block, making results depend on the
            // statement ordering chosen by analyze_dependency (FxHashMap).
            // Interpreted evaluation is order-independent for acyclic comb.
            Some(ProtoStatements(vec![ProtoStatementBlock::Interpreted(
                full_sorted,
            )]))
        } else if !all_post_comb_fns.is_empty() {
            // 3+ level hierarchy: middle module has post_comb_fns but no
            // merged functions. Combine comb + post_comb into a single
            // sorted list so eval_comb_full can evaluate everything in
            // one pass with correct dependency ordering.
            //
            // CompiledBlocks that bundle input port writes + output port
            // reads create artificial cycles in analyze_dependency. Decompose
            // them into individual statements for correct sorting.
            let mut full = Vec::new();
            for s in all_comb_statements.iter().chain(all_post_comb_fns.iter()) {
                if let ProtoStatement::CompiledBlock(cb) = s
                    && !cb.original_stmts.is_empty()
                {
                    full.extend(cb.original_stmts.iter().cloned());
                    continue;
                }
                full.push(s.clone());
            }
            match analyze_dependency(full.clone()) {
                Ok(sorted) => {
                    let full_sorted = reorder_by_level(sorted);
                    required_comb_passes = compute_required_passes(&full_sorted);
                    use_full_comb_in_step = true;
                    Some(ProtoStatements(vec![ProtoStatementBlock::Interpreted(
                        full_sorted,
                    )]))
                }
                Err(_) => {
                    // Expand CompiledBlocks to original statements for
                    // correct per-statement dependency analysis.
                    let mut decomposed = Vec::new();
                    for s in &full {
                        if let ProtoStatement::CompiledBlock(cb) = s {
                            if !cb.original_stmts.is_empty() {
                                decomposed.extend(cb.original_stmts.iter().cloned());
                            } else {
                                decomposed.push(s.clone());
                            }
                        } else {
                            decomposed.push(s.clone());
                        }
                    }
                    match analyze_dependency(decomposed) {
                        Ok(sorted) => {
                            use_full_comb_in_step = true;
                            let full_sorted = reorder_by_level(sorted);
                            required_comb_passes = compute_required_passes(&full_sorted);
                            Some(ProtoStatements(vec![ProtoStatementBlock::Interpreted(
                                full_sorted,
                            )]))
                        }
                        Err(_) => {
                            let full_sorted = reorder_by_level(full);
                            required_comb_passes = compute_required_passes(&full_sorted);
                            Some(ProtoStatements(vec![ProtoStatementBlock::Interpreted(
                                full_sorted,
                            )]))
                        }
                    }
                }
            }
        } else {
            None
        };

        // Sort FF event statements and determine selective swap set
        let mut all_swap_offsets = HashSet::default();
        let mut force_all_swap = false;

        let event_statements: HashMap<Event, ProtoStatements> = all_event_statements
            .into_iter()
            .map(|(event, stmts)| {
                if matches!(event, Event::Initial | Event::Final) {
                    (event, try_jit(context, stmts))
                } else {
                    let (sorted, swap_offsets, all_swap) = sort_ff_event(stmts);
                    if all_swap {
                        force_all_swap = true;
                    }
                    if !swap_offsets.is_empty() {
                        all_swap_offsets.extend(&swap_offsets);
                    }
                    (event, try_jit(context, sorted))
                }
            })
            .collect();

        let ff_swap_offsets = if force_all_swap {
            None // swap all FF variables (CompiledBlocks have opaque internal FF)
        } else {
            Some(all_swap_offsets)
        };

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
            post_comb_fns: match analyze_dependency(all_post_comb_fns.clone()) {
                Ok(sorted) => sorted,
                Err(_) => {
                    log::warn!(
                        "analyze_dependency failed for post_comb_fns ({} stmts) of {:?}",
                        all_post_comb_fns.len(),
                        src.name
                    );
                    all_post_comb_fns
                }
            },
            full_comb_statements,
            use_full_comb_in_step,
            required_comb_passes,
            ff_swap_offsets,
        })
    }
}
