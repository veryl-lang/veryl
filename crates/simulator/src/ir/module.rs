use crate::HashMap;
use crate::HashSet;
use crate::cranelift;
use crate::ir::context::{Context, Conv, ScopeContext};
use crate::ir::variable::{
    ModuleVariableMeta, ModuleVariables, VarOffset, Variable, create_variable_meta, value_size,
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
    /// Unified comb statements: all port connections, child comb, and internal
    /// comb combined into a single dependency-sorted list.
    pub comb_statements: Vec<Statement>,
    /// Number of eval_comb passes needed for full convergence.
    /// Pre-computed from backward edges in the sorted comb statement list.
    pub required_comb_passes: usize,
    /// FF commit entries: (current_offset, value_size) pairs.
    pub ff_commit_entries: Vec<(usize, usize)>,
}

pub struct ProtoModule {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_bytes: usize,
    pub comb_bytes: usize,
    pub use_4state: bool,
    pub module_variable_meta: ModuleVariableMeta,

    pub event_statements: HashMap<Event, ProtoStatements>,
    /// Unified comb statements: all port connections, child comb, and internal
    /// comb combined into a single dependency-sorted list.
    pub comb_statements: ProtoStatements,
    /// Number of eval_comb passes needed for full convergence.
    pub required_comb_passes: usize,
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
            let vs = value_size(nb, use_4state);
            if element.is_ff() {
                #[cfg(debug_assertions)]
                {
                    let off = element.current_offset() as usize;
                    debug_assert!(
                        off + vs <= ff_values.len(),
                        "FF current_offset out of bounds"
                    );
                    debug_assert!(
                        element.next_offset as usize + vs <= ff_values.len(),
                        "FF next_offset out of bounds"
                    );
                }
                let cur =
                    &mut ff_values[element.current_offset() as usize..] as *mut [u8] as *mut u8;
                let nxt = &mut ff_values[element.next_offset as usize..] as *mut [u8] as *mut u8;
                unsafe {
                    write_native_value(cur, nb, use_4state, initial);
                    write_native_value(nxt, nb, use_4state, initial);
                }
            } else {
                #[cfg(debug_assertions)]
                debug_assert!(
                    element.current_offset() as usize + vs <= comb_values.len(),
                    "Comb current_offset out of bounds"
                );
                let cur =
                    &mut comb_values[element.current_offset() as usize..] as *mut [u8] as *mut u8;
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
                let base = if element.is_ff() { ff_base } else { comb_base };
                base.add(element.current_offset() as usize)
            };
            current_values.push(current);

            if element.is_ff() {
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

/// Collect all FF commit entries (current_offset, value_size) from module metadata.
fn collect_ff_commit_entries(
    module_meta: &ModuleVariableMeta,
    use_4state: bool,
) -> Vec<(usize, usize)> {
    let mut entries = vec![];
    for meta in module_meta.variable_meta.values() {
        for element in &meta.elements {
            if element.is_ff() {
                let vs = value_size(element.native_bytes, use_4state);
                entries.push((element.current_offset() as usize, vs));
            }
        }
    }
    for child in &module_meta.children {
        entries.extend(collect_ff_commit_entries(child, use_4state));
    }
    entries.sort_unstable();
    entries
}

impl ProtoModule {
    pub fn instantiate(&self) -> Module {
        log::trace!(
            "instantiate: module={}, ff_bytes={}, comb_bytes={}",
            self.name,
            self.ff_bytes,
            self.comb_bytes,
        );
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

        let ff_len = self.ff_bytes;
        let comb_len = self.comb_bytes;

        let event_statements = self
            .event_statements
            .iter()
            .map(|(event, stmts)| {
                let s = stmts.to_statements(ff_ptr, ff_len, comb_ptr, comb_len, self.use_4state);
                (event.clone(), batch_binary_statements(s))
            })
            .collect();

        let comb_statements = batch_binary_statements(self.comb_statements.to_statements(
            ff_ptr,
            ff_len,
            comb_ptr,
            comb_len,
            self.use_4state,
        ));

        let ff_commit_entries =
            collect_ff_commit_entries(&self.module_variable_meta, self.use_4state);

        Module {
            name: self.name,
            ports: self.ports.clone(),
            ff_values,
            comb_values,
            module_variables,

            event_statements,
            comb_statements,
            required_comb_passes: self.required_comb_passes,
            ff_commit_entries,
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

/// JIT with load_cache disabled for unified comb.
/// CompiledBlocks (child comb functions) may modify comb values between
/// cached loads, so load_cache must be disabled for correctness.
fn try_jit_no_cache(context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
    if !context.config.use_jit {
        return ProtoStatements(vec![ProtoStatementBlock::Interpreted(proto)]);
    }

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
                    // Use no_cache build for unified comb safety
                    if group.len() <= JIT_CHUNK_SIZE {
                        match cranelift::build_binary_no_cache(context, group.clone()) {
                            Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                            None => blocks.push(ProtoStatementBlock::Interpreted(group)),
                        }
                    } else {
                        for chunk in group.chunks(JIT_CHUNK_SIZE) {
                            let chunk = chunk.to_vec();
                            match cranelift::build_binary_no_cache(context, chunk.clone()) {
                                Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                                None => blocks.push(ProtoStatementBlock::Interpreted(chunk)),
                            }
                        }
                    }
                } else {
                    blocks.push(ProtoStatementBlock::Interpreted(group));
                }
            }
            current_jittable = Some(jittable);
            current_group.push(stmt);
        }
    }

    if let Some(was_jittable) = current_jittable {
        if was_jittable {
            if current_group.len() <= JIT_CHUNK_SIZE {
                match cranelift::build_binary_no_cache(context, current_group.clone()) {
                    Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                    None => blocks.push(ProtoStatementBlock::Interpreted(current_group)),
                }
            } else {
                for chunk in current_group.chunks(JIT_CHUNK_SIZE) {
                    let chunk = chunk.to_vec();
                    match cranelift::build_binary_no_cache(context, chunk.clone()) {
                        Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                        None => blocks.push(ProtoStatementBlock::Interpreted(chunk)),
                    }
                }
            }
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
        Var(VarOffset),
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

    // Helper: build DAG and attempt topological sort.
    // Returns Ok(sorted) on success, Err(failed_id) on cycle.
    let try_topo_sort =
        |table: &HashMap<usize, ProtoStatement>| -> Result<Vec<ProtoStatement>, usize> {
            let mut dag = Dag::<Node, ()>::new();
            let mut dag_nodes: HashMap<Node, _> = HashMap::default();

            let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
            sorted_keys.sort();

            for id in &sorted_keys {
                let x = &table[id];
                let mut inputs = vec![];
                let mut outputs = vec![];
                x.gather_variable_offsets(&mut inputs, &mut outputs);
                let stmt_node = Node::Statement(*id);
                let stmt = dag.add_node(stmt_node);
                dag_nodes.insert(stmt_node, stmt);

                let output_set: HashSet<VarOffset> = outputs.iter().cloned().collect();
                let mut ok = true;
                for var_key in inputs {
                    if output_set.contains(&var_key) {
                        continue;
                    }
                    let var_node = Node::Var(var_key);
                    let var = *dag_nodes
                        .entry(var_node)
                        .or_insert_with(|| dag.add_node(var_node));
                    if dag.add_edge(var, stmt, ()).is_err() {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    return Err(*id);
                }
                for var_key in outputs {
                    let var_node = Node::Var(var_key);
                    let var = *dag_nodes
                        .entry(var_node)
                        .or_insert_with(|| dag.add_node(var_node));
                    if dag.add_edge(stmt, var, ()).is_err() {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    return Err(*id);
                }
            }

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
            Ok(ret)
        };

    // Phase 1: Try with CompiledBlocks as atomic nodes.
    if let Ok(sorted) = try_topo_sort(&table) {
        return Ok(sorted);
    }

    // Phase 2: Expand all CompiledBlocks with original_stmts and retry.
    let has_expandable = table
        .values()
        .any(|x| matches!(x, ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty()));

    if has_expandable {
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

        for eid in expandable_ids {
            if let Some(ProtoStatement::CompiledBlock(cb)) = table.remove(&eid) {
                for stmt in cb.original_stmts {
                    table.insert(next_id, stmt);
                    next_id += 1;
                }
            }
        }

        if let Ok(sorted) = try_topo_sort(&table) {
            return Ok(sorted);
        }
    }

    // Phase 3: Check for genuine combinational loop vs false positive
    // from non-expandable CompiledBlocks (shared JIT cache).
    let has_non_expandable_cb = table
        .values()
        .any(|x| matches!(x, ProtoStatement::CompiledBlock(cb) if cb.original_stmts.is_empty()));
    let has_any_cb = table
        .values()
        .any(|x| matches!(x, ProtoStatement::CompiledBlock(_)));

    if !has_any_cb {
        // Genuine loop, no CBs involved — re-do analysis for error message.
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
            let output_set: HashSet<VarOffset> = outputs.iter().cloned().collect();
            for var_key in inputs {
                if output_set.contains(&var_key) {
                    continue;
                }
                let var_node = Node::Var(var_key);
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
                let var_node = Node::Var(var_key);
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

    if !has_non_expandable_cb {
        // All CBs were expandable but cycle persists — genuine loop.
        return Err(SimulatorError::combinational_loop(
            &TokenRange::default(),
            &[],
        ));
    }

    // Relaxed ordering: skip edges that would create cycles when at least
    // one endpoint is a non-expandable CompiledBlock.
    // NOTE: With original_stmts now stored in shared JIT cache, all CBs
    // should be expandable. This path should be unreachable in practice.
    log::warn!(
        "analyze_dependency: falling back to relaxed ordering for {} stmts with non-expandable CompiledBlocks",
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

        let output_set: HashSet<VarOffset> = outputs.iter().cloned().collect();
        for var_key in &inputs {
            if output_set.contains(var_key) {
                continue;
            }
            let var_node = Node::Var(*var_key);
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
            }
        }
        for var_key in &outputs {
            let var_node = Node::Var(*var_key);
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
    let mut var_last_writer: HashMap<VarOffset, usize> = HashMap::default();
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
        let output_set: HashSet<VarOffset> = outputs.iter().cloned().collect();

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
    let mut var_level: HashMap<VarOffset, usize> = HashMap::default();
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

    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in &stmts {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    let mut var_writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
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
        if context.config.disable_ff_opt {
            ff_table.force_all_ff();
        }

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
                full_comb_extra.extend(full_comb);
            }
            all_comb_statements.append(&mut proto_decl.comb_statements.clone());
            all_post_comb_fns.extend(proto_decl.post_comb_fns);
            all_child_modules.extend(proto_decl.child_modules);
        }

        context.scope_contexts.pop();

        // Build unified comb list: all sources combined.
        // CBs are kept atomic — analyze_dependency expands them in Phase 2 if needed.
        // This preserves correct internal ordering for self-referencing comb within CBs.
        let unified: Vec<ProtoStatement> = all_comb_statements
            .into_iter()
            .chain(all_post_comb_fns)
            .chain(full_comb_extra)
            .collect();

        let unified_sorted = analyze_dependency(unified)?;
        // No DCE/inlining: unified list includes internal child comb that would be incorrectly removed.
        let unified_sorted = reorder_by_level(unified_sorted);
        let required_comb_passes = compute_required_passes(&unified_sorted);
        let comb_statements = try_jit_no_cache(context, unified_sorted);

        // Event statements preserve source order (no topological sorting).
        // NBA semantics: reads come from current, writes go to next, then
        // ff_commit copies next → current. Source order must be preserved
        // for sequential writes to the same variable.
        let event_statements: HashMap<Event, ProtoStatements> = all_event_statements
            .into_iter()
            .map(|(event, stmts)| (event, try_jit(context, stmts)))
            .collect();

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
            required_comb_passes,
        })
    }
}
