use crate::HashMap;
use crate::HashSet;
#[cfg(not(target_family = "wasm"))]
use crate::cranelift;
use crate::ir::context::{Context, Conv, ScopeContext};
use crate::ir::declaration::stable_topo_sort;
use crate::ir::schedule::IrSchedule;
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
use daggy::petgraph::Direction::Outgoing;
use daggy::petgraph::algo;
use std::collections::VecDeque;
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;

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
    /// Sensitivity-fanout + topological-rank index for the seeded-worklist
    /// scheduler. Populated from the topologically-sorted comb list;
    /// `eval_comb_worklist` uses it when `Config::use_seeded_worklist` is on.
    pub comb_schedule: IrSchedule,
    /// Diagnostic: number of non-trivial strongly-connected components in
    /// the pre-JIT `unified_sorted` dataflow graph.  Real RTL combinational
    /// loops are rejected up-front by `analyze_dependency`, so any non-zero
    /// value here is a duplication artifact in the simulator IR assembly.
    /// Exposed for regression tests.
    pub nontrivial_comb_scc: usize,
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
    /// See `Module::comb_schedule`.
    pub comb_schedule: IrSchedule,
    /// See `Module::nontrivial_comb_scc`.
    pub nontrivial_comb_scc: usize,
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
        // Single-entry initial_values on a multi-element variable is the
        // compact template form used for large arrays.
        let template_mode = meta.initial_values.len() == 1 && meta.elements.len() > 1;
        for (i, element) in meta.elements.iter().enumerate() {
            let initial = if template_mode {
                &meta.initial_values[0]
            } else {
                match meta.initial_values.get(i) {
                    Some(v) => v,
                    None => continue,
                }
            };
            let nb = element.native_bytes;
            let _vs = value_size(nb, use_4state);
            if element.is_ff() {
                #[cfg(debug_assertions)]
                {
                    let off = element.current_offset() as usize;
                    debug_assert!(
                        off + _vs <= ff_values.len(),
                        "FF current_offset out of bounds"
                    );
                    debug_assert!(
                        element.next_offset as usize + _vs <= ff_values.len(),
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
                    element.current_offset() as usize + _vs <= comb_values.len(),
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

        #[cfg(debug_assertions)]
        self.validate_offsets();

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
            comb_schedule: self.comb_schedule.clone(),
            nontrivial_comb_scc: self.nontrivial_comb_scc,
        }
    }

    /// Validate that all variable offsets in statements are within buffer bounds.
    #[cfg(debug_assertions)]
    fn validate_offsets(&self) {
        let ff_bytes = self.ff_bytes;
        let comb_bytes = self.comb_bytes;
        let use_4state = self.use_4state;

        let check = |off: &VarOffset, context: &str| {
            let raw = off.raw() as usize;
            if off.is_ff() {
                assert!(
                    raw < ff_bytes || ff_bytes == 0,
                    "validate_offsets [{}]: ff offset {} >= ff_bytes {} (module={})",
                    context,
                    raw,
                    ff_bytes,
                    self.name,
                );
            } else {
                assert!(
                    raw < comb_bytes || comb_bytes == 0,
                    "validate_offsets [{}]: comb offset {} >= comb_bytes {} (module={})",
                    context,
                    raw,
                    comb_bytes,
                    self.name,
                );
            }
        };

        let validate_stmts = |stmts: &ProtoStatements, label: &str| {
            for block in &stmts.0 {
                if let ProtoStatementBlock::Interpreted(proto) = block {
                    for s in proto {
                        let mut ins = vec![];
                        let mut outs = vec![];
                        s.gather_variable_offsets(&mut ins, &mut outs);
                        for off in ins.iter().chain(outs.iter()) {
                            check(off, label);
                        }
                    }
                }
            }
        };

        for (event, stmts) in &self.event_statements {
            validate_stmts(stmts, &format!("event {event:?}"));
        }
        validate_stmts(&self.comb_statements, "comb");

        // Validate variable metadata offsets
        validate_meta_offsets(&self.module_variable_meta, ff_bytes, comb_bytes, use_4state);
    }
}

#[cfg(debug_assertions)]
fn validate_meta_offsets(
    meta: &ModuleVariableMeta,
    ff_bytes: usize,
    comb_bytes: usize,
    use_4state: bool,
) {
    for (id, var_meta) in &meta.variable_meta {
        let vs = crate::ir::variable::value_size(var_meta.native_bytes, use_4state);
        for (i, elem) in var_meta.elements.iter().enumerate() {
            let off = elem.current_offset() as usize;
            if elem.is_ff() {
                assert!(
                    off + vs * 2 <= ff_bytes,
                    "validate_meta: ff var {:?}[{}] offset {} + vs*2 {} > ff_bytes {}",
                    id,
                    i,
                    off,
                    vs * 2,
                    ff_bytes,
                );
            } else {
                assert!(
                    off + vs <= comb_bytes,
                    "validate_meta: comb var {:?}[{}] offset {} + vs {} > comb_bytes {}",
                    id,
                    i,
                    off,
                    vs,
                    comb_bytes,
                );
            }
        }
    }
    for child in &meta.children {
        validate_meta_offsets(child, ff_bytes, comb_bytes, use_4state);
    }
}

/// Maximum number of statements per JIT function.
/// Keeps regalloc2 cost manageable (O(N^2) in SSA variable count).
/// Sized large enough to fuse a typical design's comb body into a single
/// JIT function — shrinking num_chunks reduces per-step enum-match
/// dispatch overhead in `eval_comb_full` proportionally.
/// Overridable via `VERYL_JIT_CHUNK_SIZE` env var for sweeps.
const JIT_CHUNK_SIZE_DEFAULT: usize = 8192;

fn jit_chunk_size() -> usize {
    std::env::var("VERYL_JIT_CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(JIT_CHUNK_SIZE_DEFAULT)
}

/// Build an IrSchedule whose stmt ids align 1:1 with the post-JIT
/// `comb_statements` slice. Mirrors the chunking logic of
/// `try_jit_no_cache` so each post-JIT Statement index maps to a
/// contiguous [start..end) range of pre-JIT ProtoStatements, and
/// aggregates gather_variable_offsets across that range.
#[cfg(not(target_family = "wasm"))]
fn build_aligned_schedule(
    pre_jit_stmts: &[ProtoStatement],
    use_jit: bool,
    meta: &ModuleVariableMeta,
    use_4state: bool,
) -> IrSchedule {
    use smallvec::SmallVec;
    let mut per_stmt_inputs: Vec<SmallVec<[VarOffset; 4]>> = Vec::new();
    let mut per_stmt_outputs: Vec<SmallVec<[VarOffset; 4]>> = Vec::new();

    let emit_chunk = |chunk: &[ProtoStatement],
                      per_in: &mut Vec<SmallVec<[VarOffset; 4]>>,
                      per_out: &mut Vec<SmallVec<[VarOffset; 4]>>| {
        let mut ins_all: Vec<VarOffset> = Vec::new();
        let mut outs_all: Vec<VarOffset> = Vec::new();
        for s in chunk {
            s.gather_variable_offsets_expanded(&mut ins_all, &mut outs_all);
        }
        let mut ins: SmallVec<[VarOffset; 4]> = SmallVec::new();
        for off in ins_all {
            if !ins.contains(&off) {
                ins.push(off);
            }
        }
        let mut outs: SmallVec<[VarOffset; 4]> = SmallVec::new();
        for off in outs_all {
            if !outs.contains(&off) {
                outs.push(off);
            }
        }
        per_in.push(ins);
        per_out.push(outs);
    };

    if !use_jit {
        // JIT-off: try_jit_no_cache returns a single Interpreted block
        // holding all pre-JIT stmts — one aligned slot covers everything.
        emit_chunk(pre_jit_stmts, &mut per_stmt_inputs, &mut per_stmt_outputs);
    } else {
        // Mirror try_jit_no_cache: contiguous groups of same can_build_binary,
        // jittable groups further split by JIT_CHUNK_SIZE.
        let mut current_jittable: Option<bool> = None;
        let mut group_start: usize = 0;

        let flush = |group_start: usize,
                     group_end: usize,
                     was_jittable: bool,
                     per_in: &mut Vec<SmallVec<[VarOffset; 4]>>,
                     per_out: &mut Vec<SmallVec<[VarOffset; 4]>>| {
            let group = &pre_jit_stmts[group_start..group_end];
            let chunk_size = jit_chunk_size();
            if was_jittable && group.len() > chunk_size {
                for chunk in group.chunks(chunk_size) {
                    emit_chunk(chunk, per_in, per_out);
                }
            } else {
                emit_chunk(group, per_in, per_out);
            }
        };

        for (i, stmt) in pre_jit_stmts.iter().enumerate() {
            let jittable = stmt.can_build_binary();
            match current_jittable {
                None => {
                    current_jittable = Some(jittable);
                    group_start = i;
                }
                Some(prev) if prev == jittable => {}
                Some(prev) => {
                    flush(
                        group_start,
                        i,
                        prev,
                        &mut per_stmt_inputs,
                        &mut per_stmt_outputs,
                    );
                    current_jittable = Some(jittable);
                    group_start = i;
                }
            }
        }
        if let Some(prev) = current_jittable
            && group_start < pre_jit_stmts.len()
        {
            flush(
                group_start,
                pre_jit_stmts.len(),
                prev,
                &mut per_stmt_inputs,
                &mut per_stmt_outputs,
            );
        }
    }

    let n = per_stmt_inputs.len();
    let mut sched = IrSchedule {
        n_stmts: n as u32,
        stmt_inputs: per_stmt_inputs,
        stmt_outputs: per_stmt_outputs,
        output_to_readers: crate::HashMap::default(),
        topo_rank: (0..n as crate::ir::schedule::StmtId).collect(),
        offset_sizes: crate::HashMap::default(),
    };
    sched.rebuild_fanout();
    sched.attach_offset_sizes(meta, use_4state);
    sched
}

#[cfg(not(target_family = "wasm"))]
fn try_jit_group(
    context: &mut Context,
    blocks: &mut Vec<ProtoStatementBlock>,
    group: Vec<ProtoStatement>,
) {
    // Split large groups into chunks to avoid regalloc2 O(N^2) scaling
    if group.len() <= jit_chunk_size() {
        match cranelift::build_binary(context, group.clone()) {
            Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
            None => blocks.push(ProtoStatementBlock::Interpreted(group)),
        }
    } else {
        for chunk in group.chunks(jit_chunk_size()) {
            let chunk = chunk.to_vec();
            match cranelift::build_binary(context, chunk.clone()) {
                Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                None => blocks.push(ProtoStatementBlock::Interpreted(chunk)),
            }
        }
    }
}

#[cfg(target_family = "wasm")]
fn try_jit(_context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
    ProtoStatements(vec![ProtoStatementBlock::Interpreted(proto)])
}

#[cfg(not(target_family = "wasm"))]
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
#[cfg(target_family = "wasm")]
fn try_jit_no_cache(_context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
    ProtoStatements(vec![ProtoStatementBlock::Interpreted(proto)])
}

#[cfg(not(target_family = "wasm"))]
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
                    if group.len() <= jit_chunk_size() {
                        match cranelift::build_binary_no_cache(context, group.clone()) {
                            Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                            None => blocks.push(ProtoStatementBlock::Interpreted(group)),
                        }
                    } else {
                        for chunk in group.chunks(jit_chunk_size()) {
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
            if current_group.len() <= jit_chunk_size() {
                match cranelift::build_binary_no_cache(context, current_group.clone()) {
                    Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                    None => blocks.push(ProtoStatementBlock::Interpreted(current_group)),
                }
            } else {
                for chunk in current_group.chunks(jit_chunk_size()) {
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

    // Helper: build DAG and attempt stable topological sort (Kahn's algorithm).
    // Returns Ok(sorted) on success, Err(failed_id) on cycle.
    // Uses FIFO queue initialized in source order to preserve source ordering
    // for statements that have no explicit dependency between them.
    let try_topo_sort =
        |table: &HashMap<usize, ProtoStatement>| -> Result<Vec<ProtoStatement>, usize> {
            let mut dag = Dag::<Node, ()>::new();
            let mut dag_nodes: HashMap<Node, _> = HashMap::default();

            let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
            sorted_keys.sort();

            let mut node_to_stmt: HashMap<daggy::NodeIndex, usize> = HashMap::default();

            for id in &sorted_keys {
                let x = &table[id];
                let mut inputs = vec![];
                let mut outputs = vec![];
                x.gather_variable_offsets(&mut inputs, &mut outputs);
                let stmt_node = Node::Statement(*id);
                let stmt = dag.add_node(stmt_node);
                dag_nodes.insert(stmt_node, stmt);
                node_to_stmt.insert(stmt, *id);

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

            let graph = dag.graph();
            let node_count = graph.node_count();
            let mut in_degree: HashMap<daggy::NodeIndex, usize> = HashMap::default();
            for idx in graph.node_indices() {
                in_degree.insert(idx, 0);
            }
            for edge in graph.edge_indices() {
                if let Some((_src, tgt)) = graph.edge_endpoints(edge) {
                    *in_degree.entry(tgt).or_insert(0) += 1;
                }
            }

            let mut queue: VecDeque<daggy::NodeIndex> = VecDeque::new();
            let mut zero_nodes: Vec<daggy::NodeIndex> = in_degree
                .iter()
                .filter(|&(_, &deg)| deg == 0)
                .map(|(&idx, _)| idx)
                .collect();
            zero_nodes.sort_by_key(|&idx| node_to_stmt.get(&idx).copied().unwrap_or(usize::MAX));
            for idx in zero_nodes {
                queue.push_back(idx);
            }

            let mut ret = vec![];
            let mut t = table.clone();
            let mut visited = 0;
            while let Some(idx) = queue.pop_front() {
                visited += 1;
                if let Node::Statement(x) = graph[idx]
                    && let Some(s) = t.remove(&x)
                {
                    ret.push(s);
                }
                let mut successors: Vec<daggy::NodeIndex> =
                    graph.neighbors_directed(idx, Outgoing).collect();
                successors.sort_by_key(|&s| node_to_stmt.get(&s).copied().unwrap_or(usize::MAX));
                for succ in successors {
                    let deg = in_degree.get_mut(&succ).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(succ);
                    }
                }
            }

            if visited != node_count {
                return Err(sorted_keys[0]);
            }

            Ok(ret)
        };

    // Phase 1: Try with CompiledBlocks as atomic nodes.
    if let Ok(sorted) = try_topo_sort(&table) {
        return Ok(sorted);
    }

    // Phase 2: Expand CompiledBlocks and SequentialBlocks and retry.
    // Rebuild the table with fresh sequential IDs so expanded sub-statements
    // keep their parent's position; Phase 3's fallback sorts by ID and relies
    // on that ordering for `let x = expr` vs `always_comb { x = expr; }` to
    // produce equivalent schedules when the block participates in a cycle.
    let has_expandable = table.values().any(|x| {
        matches!(x, ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty())
            || matches!(x, ProtoStatement::SequentialBlock(_))
    });

    if has_expandable {
        let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
        sorted_keys.sort();

        let mut new_table: HashMap<usize, ProtoStatement> = HashMap::default();
        let mut new_id = 0usize;
        for key in sorted_keys {
            let stmt = table.remove(&key).unwrap();
            match stmt {
                ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty() => {
                    for sub in cb.original_stmts {
                        new_table.insert(new_id, sub);
                        new_id += 1;
                    }
                }
                ProtoStatement::SequentialBlock(body) => {
                    for sub in body {
                        new_table.insert(new_id, sub);
                        new_id += 1;
                    }
                }
                other => {
                    new_table.insert(new_id, other);
                    new_id += 1;
                }
            }
        }
        table = new_table;

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

    if !has_any_cb || !has_non_expandable_cb {
        // DAG-based sort failed (false cycle from inlined function bodies).
        // Fall back to direct statement-level sort.
        let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
        sorted_keys.sort();
        let stmts: Vec<ProtoStatement> = sorted_keys.iter().map(|k| table[k].clone()).collect();
        let sorted = stable_topo_sort(stmts);
        // Verify no genuine combinational loop remains.
        let n = sorted.len();
        let mut s_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
        let mut s_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
        for s in &sorted {
            let mut ins = vec![];
            let mut outs = vec![];
            s.gather_variable_offsets(&mut ins, &mut outs);
            s_inputs.push(ins);
            s_outputs.push(outs);
        }
        let mut w: HashMap<VarOffset, Vec<usize>> = HashMap::default();
        for (i, outs) in s_outputs.iter().enumerate() {
            for &key in outs {
                w.entry(key).or_default().push(i);
            }
        }
        let mut a: Vec<HashSet<usize>> = vec![HashSet::default(); n];
        let mut deg: Vec<usize> = vec![0; n];
        for (ri, ins) in s_inputs.iter().enumerate() {
            for key in ins {
                if let Some(wis) = w.get(key) {
                    if wis.len() == 1 {
                        let wi = wis[0];
                        if wi != ri && a[wi].insert(ri) {
                            deg[ri] += 1;
                        }
                    } else if let Some(&wi) = wis.iter().rev().find(|&&w| w < ri)
                        && a[wi].insert(ri)
                    {
                        deg[ri] += 1;
                    }
                }
            }
        }
        let mut q: VecDeque<usize> = VecDeque::new();
        for (i, &d) in deg.iter().enumerate() {
            if d == 0 {
                q.push_back(i);
            }
        }
        let mut cnt = 0;
        while let Some(idx) = q.pop_front() {
            cnt += 1;
            for &succ in &a[idx] {
                deg[succ] -= 1;
                if deg[succ] == 0 {
                    q.push_back(succ);
                }
            }
        }
        if cnt == n {
            return Ok(sorted);
        }
        // Collect tokens from statements involved in the cycle (deg > 0).
        let mut tokens: Vec<_> = deg
            .iter()
            .enumerate()
            .filter(|(_, d)| **d > 0)
            .filter_map(|(i, _)| sorted[i].token())
            .filter(|t| *t != Default::default())
            .collect();
        let trigger = tokens.pop().unwrap_or_default();
        return Err(SimulatorError::combinational_loop(&trigger, &tokens));
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
/// Bit range for a partial comb assignment. `None` = full-width write/read.
/// Pair is (high_bit_inclusive, low_bit_inclusive) matching Veryl's
/// `Assign.select` convention. Overlap: two ranges overlap if their
/// bit intervals intersect.
type BitRange = Option<(usize, usize)>;

fn ranges_overlap(a: BitRange, b: BitRange) -> bool {
    match (a, b) {
        (None, _) | (_, None) => true,
        (Some((a_hi, a_lo)), Some((b_hi, b_lo))) => a_lo <= b_hi && b_lo <= a_hi,
    }
}

/// Collect (offset, bit_range) outputs for bit-aware SCC analysis.
/// Only captures writes that are precisely bit-ranged (via Assign.select);
/// everything else falls back to full-width (None).
fn gather_bit_aware_outputs(stmt: &ProtoStatement, out: &mut Vec<(VarOffset, BitRange)>) {
    match stmt {
        ProtoStatement::Assign(x) => out.push((x.dst, x.select)),
        ProtoStatement::AssignDynamic(x) => {
            out.push((x.dst_base, None));
            if x.dst_num_elements > 1 {
                let last = VarOffset::new(
                    x.dst_base.is_ff(),
                    x.dst_base.raw() + x.dst_stride * (x.dst_num_elements as isize - 1),
                );
                out.push((last, None));
            }
        }
        ProtoStatement::If(x) => {
            for s in &x.true_side {
                gather_bit_aware_outputs(s, out);
            }
            for s in &x.false_side {
                gather_bit_aware_outputs(s, out);
            }
        }
        ProtoStatement::For(x) => {
            for s in &x.body {
                gather_bit_aware_outputs(s, out);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                gather_bit_aware_outputs(s, out);
            }
        }
        ProtoStatement::CompiledBlock(x) => {
            if !x.original_stmts.is_empty() {
                for s in &x.original_stmts {
                    gather_bit_aware_outputs(s, out);
                }
            } else {
                for &off in &x.output_offsets {
                    if !off.is_ff() {
                        out.push((VarOffset::Comb(off.raw()), None));
                    }
                }
            }
        }
        _ => {}
    }
}

/// Diagnostic: compute strongly-connected components of the stmt-level
/// dataflow graph (stmt A → stmt B when A writes a variable B reads).
/// Returns (num_nontrivial_sccs, max_scc_size, total_stmts_in_sccs).
/// Enabled by VERYL_SCC_DIAG=1.
///
/// When VERYL_SCC_NARROW=1, uses the conservative base+last array
/// dependency encoding so that cycles formed only by array-element
/// overlap are filtered out — what remains is scalar comb cycles
/// that would be flagged by a logic synthesis tool.
///
/// When VERYL_SCC_BITAWARE=1, treats partial-width writes (via
/// Assign.select bit ranges) as independent edges: a write to x[7:4]
/// does not create an edge to readers of x[3:0].  This eliminates
/// SCCs formed only by bit-lane overlap in the VarOffset-level IR.
fn compute_scc_stats(sorted: &[ProtoStatement]) -> (usize, usize, usize) {
    use daggy::petgraph::Graph;
    use daggy::petgraph::algo::tarjan_scc;

    let n = sorted.len();
    if n == 0 {
        return (0, 0, 0);
    }

    // Gather per-stmt I/O. Expanded by default (captures per-element
    // array deps); narrow mode uses base+last (what synthesis tools see).
    let narrow = std::env::var("VERYL_SCC_NARROW").is_ok();
    let bitaware = std::env::var("VERYL_SCC_BITAWARE").is_ok();
    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_output_bits: Vec<Vec<(VarOffset, BitRange)>> = Vec::with_capacity(n);
    for s in sorted {
        let mut ins = vec![];
        let mut outs = vec![];
        if narrow {
            s.gather_variable_offsets(&mut ins, &mut outs);
        } else {
            s.gather_variable_offsets_expanded(&mut ins, &mut outs);
        }
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
        if bitaware {
            let mut bit_outs = vec![];
            gather_bit_aware_outputs(s, &mut bit_outs);
            stmt_output_bits.push(bit_outs);
        } else {
            stmt_output_bits.push(vec![]);
        }
    }

    // var → list of (writer stmt index, bit range for bit-aware mode).
    // In non-bitaware mode, bit_range is always None and overlap is trivial.
    let mut writers: HashMap<VarOffset, Vec<(usize, BitRange)>> = HashMap::default();
    if bitaware {
        for (i, outs) in stmt_output_bits.iter().enumerate() {
            for &(off, br) in outs {
                if off.is_ff() {
                    continue;
                }
                writers.entry(off).or_default().push((i, br));
            }
        }
    } else {
        for (i, outs) in stmt_outputs.iter().enumerate() {
            for &off in outs {
                if off.is_ff() {
                    continue;
                }
                writers.entry(off).or_default().push((i, None));
            }
        }
    }

    let mut graph: Graph<usize, ()> = Graph::new();
    let nodes: Vec<_> = (0..n).map(|i| graph.add_node(i)).collect();
    let mut edge_set: HashSet<(usize, usize)> = HashSet::default();
    // For bit-aware mode, we need to know the reader's bit range for this
    // offset. Currently ProtoExpression::Variable reads don't expose
    // per-field select in gather_variable_offsets, so we conservatively
    // treat reader ranges as None (= full width).  This still filters
    // out false cycles that arise from multiple writers on non-overlapping
    // bit slices, which is the common IR artifact.
    for (reader, ins) in stmt_inputs.iter().enumerate() {
        for &off in ins {
            if off.is_ff() {
                continue;
            }
            if let Some(ws) = writers.get(&off) {
                for &(w, wbr) in ws {
                    if w == reader {
                        continue;
                    }
                    if bitaware && !ranges_overlap(wbr, None) {
                        // Reader range is None (full-width) so this should
                        // never trigger, but kept for structural clarity.
                        continue;
                    }
                    if edge_set.insert((w, reader)) {
                        graph.add_edge(nodes[w], nodes[reader], ());
                    }
                }
            }
        }
    }

    let sccs = tarjan_scc(&graph);
    let mut nontrivial = 0usize;
    let mut max_size = 0usize;
    let mut total = 0usize;
    let mut size_hist: Vec<usize> = Vec::new();
    for scc in &sccs {
        if scc.len() > 1 {
            nontrivial += 1;
            total += scc.len();
            if scc.len() > max_size {
                max_size = scc.len();
            }
            size_hist.push(scc.len());
        }
    }
    size_hist.sort_unstable();
    size_hist.reverse();
    if nontrivial > 0 && std::env::var("VERYL_SCC_DIAG").is_ok() {
        let top: Vec<String> = size_hist.iter().take(10).map(|s| s.to_string()).collect();
        log::info!(
            "SCC stats: {} nontrivial SCCs, max={}, total_stmts_in_SCCs={}, top sizes=[{}]",
            nontrivial,
            max_size,
            total,
            top.join(", ")
        );
        // Position ranges of the largest SCCs to verify contiguity.
        let mut sccs_sorted = sccs.clone();
        sccs_sorted.sort_by_key(|scc| std::cmp::Reverse(scc.len()));
        for (i, scc) in sccs_sorted.iter().take(5).enumerate() {
            if scc.len() <= 1 {
                break;
            }
            let mut positions: Vec<usize> = scc.iter().map(|&idx| idx.index()).collect();
            positions.sort_unstable();
            let min_pos = positions[0];
            let max_pos = positions[positions.len() - 1];
            let range_span = max_pos - min_pos + 1;
            let density = scc.len() as f64 / range_span as f64;
            log::info!(
                "  SCC[{}]: size={}, position range=[{}..{}], span={}, density={:.2}",
                i,
                scc.len(),
                min_pos,
                max_pos,
                range_span,
                density
            );
        }

        // Count unique comb-output offsets written by the SCC stmts.
        // This tells us how narrow a snapshot/compare scope for
        // SCC-only convergence would be.
        let mut all_scc_outs: HashSet<VarOffset> = HashSet::default();
        for scc in &sccs {
            if scc.len() > 1 {
                for node in scc {
                    let idx = node.index();
                    for &off in &stmt_outputs[idx] {
                        if !off.is_ff() {
                            all_scc_outs.insert(off);
                        }
                    }
                }
            }
        }
        log::info!(
            "SCC comb outputs: {} unique offsets ({} stmts in SCCs)",
            all_scc_outs.len(),
            total
        );

        // Kind histogram + in/out offset histogram for the largest SCC.
        let largest = sccs_sorted.first().filter(|s| s.len() > 1);
        if let Some(scc) = largest {
            let mut kind_hist: HashMap<&'static str, usize> = HashMap::default();
            let mut out_counts: HashMap<VarOffset, usize> = HashMap::default();
            let mut in_counts: HashMap<VarOffset, usize> = HashMap::default();
            let mut source_hist: HashMap<String, usize> = HashMap::default();
            let mut line_samples: Vec<(String, u32)> = Vec::new();
            for node in scc {
                let idx = node.index();
                if let ProtoStatement::Assign(x) = &sorted[idx] {
                    let src = x.token.beg.source.to_string();
                    let line = x.token.beg.line;
                    *source_hist.entry(src.clone()).or_insert(0) += 1;
                    line_samples.push((src, line));
                }
            }
            let mut sources: Vec<_> = source_hist.into_iter().collect();
            sources.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            let src_str: Vec<String> = sources
                .iter()
                .take(15)
                .map(|(s, c)| format!("{}={}", s, c))
                .collect();
            log::info!("SCC[0] source file distribution: {}", src_str.join(", "));
            line_samples.sort();
            let uniq: Vec<String> = line_samples
                .iter()
                .take(10)
                .map(|(s, l)| format!("{}:{}", s, l))
                .collect();
            log::info!("SCC[0] first 10 (src:line): {:?}", uniq);

            for node in scc {
                let idx = node.index();
                let kind = match &sorted[idx] {
                    ProtoStatement::Assign(_) => "Assign",
                    ProtoStatement::AssignDynamic(_) => "AssignDynamic",
                    ProtoStatement::If(_) => "If",
                    ProtoStatement::For(_) => "For",
                    ProtoStatement::Break => "Break",
                    ProtoStatement::SystemFunctionCall(_) => "SystemFunctionCall",
                    ProtoStatement::CompiledBlock(_) => "CompiledBlock",
                    ProtoStatement::SequentialBlock(_) => "SequentialBlock",
                    ProtoStatement::TbMethodCall { .. } => "TbMethodCall",
                };
                *kind_hist.entry(kind).or_insert(0) += 1;
                for &off in &stmt_outputs[idx] {
                    if !off.is_ff() {
                        *out_counts.entry(off).or_insert(0) += 1;
                    }
                }
                for &off in &stmt_inputs[idx] {
                    if !off.is_ff() {
                        *in_counts.entry(off).or_insert(0) += 1;
                    }
                }
            }
            let mut kinds: Vec<_> = kind_hist.into_iter().collect();
            kinds.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            let kstr: Vec<String> = kinds.iter().map(|(k, c)| format!("{}={}", k, c)).collect();
            log::info!("SCC[0] kind histogram: {}", kstr.join(", "));

            // Find offsets that are BOTH written and read many times within
            // the SCC — these are the "pivots" forming the cycles.
            let mut pivots: Vec<(VarOffset, usize, usize)> = out_counts
                .iter()
                .filter_map(|(&off, &wc)| in_counts.get(&off).map(|&rc| (off, wc, rc)))
                .collect();
            pivots.sort_by_key(|(_, wc, rc)| std::cmp::Reverse(wc * rc));
            log::info!("SCC[0] top pivots (offset, writers, readers):");
            for (off, wc, rc) in pivots.iter().take(10) {
                // Collect the bit ranges of writers to this offset within the SCC.
                let mut bit_writers: Vec<BitRange> = Vec::new();
                for node in scc {
                    let idx = node.index();
                    let mut outs: Vec<(VarOffset, BitRange)> = Vec::new();
                    gather_bit_aware_outputs(&sorted[idx], &mut outs);
                    for (w_off, br) in &outs {
                        if w_off == off {
                            bit_writers.push(*br);
                        }
                    }
                }
                let full_count = bit_writers.iter().filter(|b| b.is_none()).count();
                let partial_count = bit_writers.len() - full_count;
                let ranges: Vec<String> = bit_writers
                    .iter()
                    .filter_map(|b| b.map(|(hi, lo)| format!("[{}:{}]", hi, lo)))
                    .take(8)
                    .collect();
                log::info!(
                    "    {:?}: {} writers ({} full, {} partial), {} readers; partial ranges: {:?}",
                    off,
                    wc,
                    full_count,
                    partial_count,
                    rc,
                    ranges
                );
            }
        }
    }
    (nontrivial, max_size, total)
}

/// Build a map from comb VarOffset → human-readable variable path.
/// Walks ModuleVariableMeta recursively and records the offset of each
/// VariableElement's `current` slot together with the module hierarchy
/// prefix.
fn build_offset_path_map(meta: &ModuleVariableMeta) -> HashMap<VarOffset, String> {
    let mut map = HashMap::default();
    fn walk(meta: &ModuleVariableMeta, prefix: &str, out: &mut HashMap<VarOffset, String>) {
        let name = meta.name.to_string();
        let mod_prefix = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{}.{}", prefix, name)
        };
        for var_meta in meta.variable_meta.values() {
            let var_name = var_meta
                .path
                .0
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(".");
            for (i, element) in var_meta.elements.iter().enumerate() {
                let display = if var_meta.elements.len() > 1 {
                    format!("{}.{}[{}]", mod_prefix, var_name, i)
                } else {
                    format!("{}.{}", mod_prefix, var_name)
                };
                out.insert(element.current, display);
            }
        }
        for child in &meta.children {
            walk(child, &mod_prefix, out);
        }
    }
    walk(meta, "", &mut map);
    map
}

/// Diagnostic: trace a concrete cycle in the largest SCC of the comb
/// dataflow graph and print it as a sequence of variable names.
/// Helps pinpoint the exact combinational loop in source.
fn trace_scc_cycles(sorted: &[ProtoStatement], meta: &ModuleVariableMeta) {
    use daggy::petgraph::Graph;
    use daggy::petgraph::algo::tarjan_scc;

    let n = sorted.len();
    if n == 0 {
        return;
    }

    let path_map = build_offset_path_map(meta);

    // Build stmt-level dataflow graph (same as compute_scc_stats).
    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in sorted {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    let mut writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        for &off in outs {
            if off.is_ff() {
                continue;
            }
            writers.entry(off).or_default().push(i);
        }
    }

    // adj[reader] = list of writer-stmt indices whose outputs the reader reads.
    // (Edge direction in trace: reader ← writer, but for cycle finding we
    // walk writer → reader.)
    let mut adj: Vec<Vec<(usize, VarOffset)>> = vec![vec![]; n];
    let mut graph: Graph<usize, ()> = Graph::new();
    let nodes: Vec<_> = (0..n).map(|i| graph.add_node(i)).collect();
    let mut edge_set: HashSet<(usize, usize)> = HashSet::default();
    for (reader, ins) in stmt_inputs.iter().enumerate() {
        for &off in ins {
            if off.is_ff() {
                continue;
            }
            if let Some(ws) = writers.get(&off) {
                for &w in ws {
                    if w != reader && edge_set.insert((w, reader)) {
                        graph.add_edge(nodes[w], nodes[reader], ());
                        adj[w].push((reader, off));
                    }
                }
            }
        }
    }

    let sccs = tarjan_scc(&graph);
    let mut sccs_sorted = sccs.clone();
    sccs_sorted.sort_by_key(|s| std::cmp::Reverse(s.len()));

    for (scc_idx, scc) in sccs_sorted.iter().enumerate() {
        if scc.len() <= 1 {
            break;
        }
        let member_set: HashSet<usize> = scc.iter().map(|n| n.index()).collect();

        // Find a concrete cycle: BFS from the first member, through
        // edges confined to the SCC, find shortest path back to start.
        let start = scc[0].index();
        let mut parent: HashMap<usize, (usize, VarOffset)> = HashMap::default();
        let mut queue: std::collections::VecDeque<usize> = Default::default();
        queue.push_back(start);
        let mut found_back_to_start = None;
        while let Some(u) = queue.pop_front() {
            for &(v, off) in &adj[u] {
                if !member_set.contains(&v) {
                    continue;
                }
                if v == start {
                    // Found cycle: start → ... → u → (off) → start
                    found_back_to_start = Some((u, off));
                    break;
                }
                if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(v) {
                    e.insert((u, off));
                    queue.push_back(v);
                }
            }
            if found_back_to_start.is_some() {
                break;
            }
        }

        log::info!("SCC[{}] cycle trace (size={}):", scc_idx, scc.len());
        if let Some((last, last_off)) = found_back_to_start {
            // Rebuild path from start → last.
            let mut path: Vec<(usize, VarOffset)> = vec![(last, last_off)];
            let mut cur = last;
            while cur != start {
                if let Some(&(p, off)) = parent.get(&cur) {
                    path.push((p, off));
                    cur = p;
                } else {
                    break;
                }
            }
            path.reverse();
            let describe_offset = |off: VarOffset| -> String {
                path_map
                    .get(&off)
                    .cloned()
                    .unwrap_or_else(|| format!("{:?}", off))
            };
            let describe_stmt = |idx: usize| -> String {
                let (tok_beg, kind) = match &sorted[idx] {
                    ProtoStatement::Assign(x) => (Some(x.token.beg), "Assign"),
                    _ => (
                        None,
                        match &sorted[idx] {
                            ProtoStatement::If(_) => "If",
                            ProtoStatement::AssignDynamic(_) => "AssignDynamic",
                            ProtoStatement::For(_) => "For",
                            ProtoStatement::SequentialBlock(_) => "SeqBlock",
                            ProtoStatement::CompiledBlock(_) => "CompiledBlock",
                            ProtoStatement::SystemFunctionCall(_) => "SysFn",
                            ProtoStatement::TbMethodCall { .. } => "TbCall",
                            ProtoStatement::Break => "Break",
                            _ => "?",
                        },
                    ),
                };
                if let Some(tok) = tok_beg {
                    let src = tok.source.to_string();
                    let file = src.rsplit('/').next().unwrap_or(&src);
                    format!("[{}] {}:{}", kind, file, tok.line)
                } else {
                    format!("[{}] #{}", kind, idx)
                }
            };
            log::info!("  start at stmt {} ({})", start, describe_stmt(start));
            for (stmt_idx, via_off) in &path {
                log::info!(
                    "    ── writes {} ──→ stmt {} ({})",
                    describe_offset(*via_off),
                    stmt_idx,
                    describe_stmt(*stmt_idx)
                );
            }
            log::info!(
                "    ── writes {} ──→ back to start",
                describe_offset(last_off)
            );
        } else {
            log::info!("  (no cycle found from start; graph error?)");
        }

        // Also list the top pivot variables by name.
        let mut out_counts: HashMap<VarOffset, usize> = HashMap::default();
        let mut in_counts: HashMap<VarOffset, usize> = HashMap::default();
        for &idx in &member_set {
            for &off in &stmt_outputs[idx] {
                if !off.is_ff() {
                    *out_counts.entry(off).or_insert(0) += 1;
                }
            }
            for &off in &stmt_inputs[idx] {
                if !off.is_ff() {
                    *in_counts.entry(off).or_insert(0) += 1;
                }
            }
        }
        let mut pivots: Vec<(VarOffset, usize, usize)> = out_counts
            .iter()
            .filter_map(|(&off, &wc)| in_counts.get(&off).map(|&rc| (off, wc, rc)))
            .collect();
        pivots.sort_by_key(|(_, wc, rc)| std::cmp::Reverse(wc * rc));
        log::info!("  top pivot variables (by writers × readers):");
        for (off, wc, rc) in pivots.iter().take(10) {
            let name = path_map
                .get(off)
                .cloned()
                .unwrap_or_else(|| format!("{:?}", off));
            log::info!("    {}: {} writers, {} readers", name, wc, rc);
        }

        if scc_idx >= 3 {
            break; // Print at most a few SCCs.
        }
    }
}

fn compute_required_passes(sorted: &[ProtoStatement]) -> usize {
    let n = sorted.len();
    if n == 0 {
        return 1;
    }

    if std::env::var("VERYL_SCC_DIAG").is_ok() {
        compute_scc_stats(sorted);
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
    // SCC iteration depth is computed only as a diagnostic — `passes`
    // returned above uses DAG depth alone.  Counting backward edges into
    // the pass total would penalise every `settle_comb` call on designs
    // with false SCCs (e.g. multi-driver array writes the IR can't
    // disambiguate), and the regression is severe for large-memory
    // designs where each extra full pass walks every comb byte.
    if std::env::var("VERYL_SCC_DIAG").is_ok() {
        let scc_depth = compute_scc_iteration_depth(sorted);
        log::info!("  (diagnostic) SCC iteration depth: {}", scc_depth);
    }
    passes
}

/// Compute the max backward-edge chain depth inside any non-trivial SCC
/// of the comb dataflow graph. Returns 0 if no non-trivial SCCs exist.
///
/// Intuition: within an SCC, some edges must run "backward" in any topo
/// order (that's what makes it an SCC). The longest chain of such
/// backward edges is how many extra full passes the design needs to
/// settle the cycle.
fn compute_scc_iteration_depth(sorted: &[ProtoStatement]) -> usize {
    use daggy::petgraph::Graph;
    use daggy::petgraph::algo::tarjan_scc;

    let n = sorted.len();
    if n == 0 {
        return 0;
    }

    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in sorted {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets_expanded(&mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    // Build stmt-level DAG edges (comb writer → comb reader) and find
    // SCCs.
    let mut writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        for &off in outs {
            if !off.is_ff() {
                writers.entry(off).or_default().push(i);
            }
        }
    }

    let mut graph: Graph<usize, ()> = Graph::new();
    let nodes: Vec<_> = (0..n).map(|i| graph.add_node(i)).collect();
    let mut edge_set: HashSet<(usize, usize)> = HashSet::default();
    for (reader, ins) in stmt_inputs.iter().enumerate() {
        for &off in ins {
            if off.is_ff() {
                continue;
            }
            if let Some(ws) = writers.get(&off) {
                for &w in ws {
                    if w != reader && edge_set.insert((w, reader)) {
                        graph.add_edge(nodes[w], nodes[reader], ());
                    }
                }
            }
        }
    }

    let sccs = tarjan_scc(&graph);
    let mut max_depth = 0usize;

    for scc in &sccs {
        if scc.len() <= 1 {
            continue;
        }
        // SCC members and their original positions.
        let member_positions: Vec<usize> = scc.iter().map(|&idx| idx.index()).collect();
        let mut member_set: HashSet<usize> = HashSet::default();
        for &p in &member_positions {
            member_set.insert(p);
        }

        // Within the SCC subgraph (restricted to members and their
        // edges), compute backward-chain depth using the same algorithm
        // as the DAG case. Use original sorted position as the topo
        // order — this is a topo order of the whole graph but may
        // include many "backward" edges inside the SCC (which is
        // expected; that's what iteration resolves).
        let mut sorted_positions = member_positions.clone();
        sorted_positions.sort_unstable();

        // Build a writer map restricted to SCC members.
        let mut scc_writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
        for &p in &sorted_positions {
            for &off in &stmt_outputs[p] {
                if !off.is_ff() {
                    scc_writers.entry(off).or_default().push(p);
                }
            }
        }

        // Map original position → internal order (0, 1, 2, ...).
        let mut pos_to_ord: HashMap<usize, usize> = HashMap::default();
        for (ord, &p) in sorted_positions.iter().enumerate() {
            pos_to_ord.insert(p, ord);
        }

        let scc_n = sorted_positions.len();
        let mut delay = vec![0usize; scc_n];
        // Reverse scan by internal order.
        for ord in (0..scc_n).rev() {
            let p = sorted_positions[ord];
            let output_set: HashSet<VarOffset> = stmt_outputs[p].iter().cloned().collect();
            for key in &stmt_inputs[p] {
                if output_set.contains(key) {
                    continue;
                }
                if let Some(ws) = scc_writers.get(key) {
                    // A backward edge exists if any writer's internal
                    // order is strictly greater than this stmt's order.
                    for &wp in ws {
                        if wp == p {
                            continue;
                        }
                        if !member_set.contains(&wp) {
                            continue;
                        }
                        if let Some(&wo) = pos_to_ord.get(&wp)
                            && wo > ord
                        {
                            delay[ord] = delay[ord].max(delay[wo] + 1);
                        }
                    }
                }
            }
        }

        // Add 1 for the safety of propagation through the cycle head:
        // the reverse-scan counts "backward edges along longest path"
        // but the cycle head needs one extra iteration for its own
        // stale-input read to settle.  Empirically matches heliodor
        // (measured K_runtime = 4, algo without margin returns 3).
        let scc_max = delay.iter().copied().max().unwrap_or(0) + 1;
        if scc_max > max_depth {
            max_depth = scc_max;
        }
    }

    max_depth
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
            ProtoStatement::SequentialBlock(_) => 1,
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
            ProtoStatement::SequentialBlock(_) => 1,
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

        for decl in &src.declarations {
            let proto_decl: ProtoDeclaration = Conv::conv(context, decl)?;

            for (event, mut stmts) in proto_decl.event_statements {
                all_event_statements
                    .entry(event)
                    .and_modify(|v| v.append(&mut stmts))
                    .or_insert(stmts);
            }
            all_comb_statements.append(&mut proto_decl.comb_statements.clone());
            all_post_comb_fns.extend(proto_decl.post_comb_fns);
            all_child_modules.extend(proto_decl.child_modules);
        }

        context.scope_contexts.pop();

        // Build unified comb list: execution-side only.
        // Merged-JIT children contribute their comb-only CB via `post_comb_fns`;
        // the originals are preserved inside each CB's `original_stmts` and
        // expanded on demand by `analyze_dependency` Phase 2 when fine-grained
        // ordering is needed.  This eliminates the false SCC artifact from
        // keeping both CB and its originals in the parent's `unified` list.
        let unified: Vec<ProtoStatement> = all_comb_statements
            .into_iter()
            .chain(all_post_comb_fns)
            .collect();

        let unified_sorted = analyze_dependency(unified)?;
        // No DCE/inlining: unified list includes internal child comb that would be incorrectly removed.
        let unified_sorted = reorder_by_level(unified_sorted);
        let required_comb_passes = compute_required_passes(&unified_sorted);

        // Invariant: `analyze_dependency` rejects real combinational loops
        // via `combinational_loop` error, so reaching this point means the
        // stmt-level graph is a well-formed DAG.  Any remaining non-trivial
        // SCC in the expanded dataflow view indicates duplicate
        // ProtoStatements in the simulator IR assembly.  We assert SCC == 0
        // in debug builds so any future regression that reintroduces this
        // class of bug surfaces immediately, and expose the count on
        // `Module`/`Ir` for test-local assertion.
        let nontrivial_comb_scc = compute_scc_stats(&unified_sorted).0;
        debug_assert_eq!(
            nontrivial_comb_scc, 0,
            "ProtoModule {:?}: {} nontrivial SCC(s) in unified_sorted. \
             analyze_dependency would have rejected a real combinational loop, \
             so this indicates duplicate ProtoStatements in the simulator IR.",
            src.name, nontrivial_comb_scc,
        );

        // Snapshot unified_sorted before JIT consumes it: the worklist
        // schedule (built below) needs to walk the pre-JIT ProtoStatement
        // list because JIT CompiledBlocks don't expose stmt-level I/O to
        // `gather_variable_offsets`.
        let pre_jit_stmts = unified_sorted.clone();
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

        if std::env::var("VERYL_SCC_TRACE").is_ok() {
            trace_scc_cycles(&pre_jit_stmts, &module_variable_meta);
        }

        // Build an IrSchedule whose stmt ids align with the post-JIT
        // comb_statements.  Only build it when the worklist is actually
        // enabled — the build walks `gather_variable_offsets_expanded`,
        // which emits per-element offsets for DynamicVariable/AssignDynamic,
        // and for designs with large memory arrays (multi-MB DRAMs) this
        // costs seconds per ProtoModule even when the worklist is disabled.
        #[cfg(not(target_family = "wasm"))]
        let comb_schedule = if context.config.use_seeded_worklist {
            build_aligned_schedule(
                &pre_jit_stmts,
                context.config.use_jit,
                &module_variable_meta,
                context.config.use_4state,
            )
        } else {
            IrSchedule::empty()
        };
        #[cfg(target_family = "wasm")]
        let comb_schedule = IrSchedule::empty();

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
            comb_schedule,
            nontrivial_comb_scc,
        })
    }
}
