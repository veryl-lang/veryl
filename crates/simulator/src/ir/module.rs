use crate::FuncPtr;
use crate::HashMap;
use crate::HashSet;
#[cfg(not(target_family = "wasm"))]
use crate::cranelift;
use crate::ir::context::{Context, Conv, ScopeContext, WriteLogBuffer};
use crate::ir::declaration::stable_topo_sort;
use crate::ir::statement::ChunkActivityMeta;
use crate::ir::variable::{
    ModuleVariableMeta, ModuleVariables, VarOffset, Variable, create_variable_meta, value_size,
    write_native_value,
};
use crate::ir::{
    ColdChunk, Event, ProtoDeclaration, ProtoExpression, ProtoStatement, ProtoStatementBlock,
    ProtoStatements, Statement, VarId, VarPath,
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
    /// Size of the "hot" comb region (before large arrays).
    pub comb_hot_size: usize,
    /// Number of eval_comb passes needed for full convergence.
    /// Pre-computed from backward edges in the sorted comb statement list.
    pub required_comb_passes: usize,
    /// FF commit entries: (current_offset, value_size) pairs.
    pub ff_commit_entries: Vec<(usize, usize)>,
    /// Cold comb chunks: chunks that access cold region, with activity-skip metadata.
    pub cold_chunks: Vec<ColdChunk>,
    /// Byte offset of cold dirty flag in comb_values.
    pub cold_dirty_flag_offset: usize,
    /// Per-comb-block activity metadata (parallel to blocks in ProtoStatements).
    pub comb_activity_meta: Vec<ChunkActivityMeta>,
    /// Block→Statement index ranges: block i covers comb_statements[start..end].
    pub block_stmt_ranges: Vec<(usize, usize)>,
    /// Comb offsets written by event statements (filtered to comb-read intersection).
    pub event_comb_writes: HashSet<isize>,
    /// Byte offset in comb_values for event→comb dirty flag.
    pub event_comb_dirty_flag_offset: usize,
    /// Heap-allocated write-log buffer for sparse FF commit.
    pub write_log_buffer: Option<Box<WriteLogBuffer>>,
}

pub struct ProtoModule {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_bytes: usize,
    pub comb_bytes: usize,
    pub comb_hot_bytes: usize,
    pub use_4state: bool,
    pub module_variable_meta: ModuleVariableMeta,

    pub event_statements: HashMap<Event, ProtoStatements>,
    /// Unified comb statements: all port connections, child comb, and internal
    /// comb combined into a single dependency-sorted list.
    pub comb_statements: ProtoStatements,
    /// Number of eval_comb passes needed for full convergence.
    pub required_comb_passes: usize,
    /// Byte offset of cold dirty flag in comb_values.
    pub cold_dirty_flag_offset: i64,
    /// Comb offsets written by event statements (filtered).
    pub event_comb_writes: HashSet<isize>,
    /// Byte offset in comb_values for event→comb dirty flag (computed at instantiation).
    pub event_comb_dirty_flag_offset: usize,
    /// Heap-allocated write-log buffer for sparse FF commit.
    pub write_log_buffer: Option<Box<WriteLogBuffer>>,
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
) -> (Vec<(usize, usize)>, HashMap<usize, String>) {
    let mut entries = vec![];
    let mut name_map = HashMap::default();
    collect_ff_entries_recursive(module_meta, use_4state, &mut entries, &mut name_map);
    entries.sort_unstable();
    (entries, name_map)
}

fn collect_ff_entries_recursive(
    module_meta: &ModuleVariableMeta,
    use_4state: bool,
    entries: &mut Vec<(usize, usize)>,
    name_map: &mut HashMap<usize, String>,
) {
    for meta in module_meta.variable_meta.values() {
        for (i, element) in meta.elements.iter().enumerate() {
            if element.is_ff() {
                let vs = value_size(element.native_bytes, use_4state);
                let off = element.current_offset() as usize;
                entries.push((off, vs));
                // Skip name_map for large arrays to avoid millions of String allocations
                if meta.elements.len() <= 4096 {
                    name_map.insert(off, format!("{}[{}]", meta.path, i));
                }
            }
        }
    }
    for child in &module_meta.children {
        collect_ff_entries_recursive(child, use_4state, entries, name_map);
    }
}

impl ProtoModule {
    pub fn instantiate(&mut self) -> Module {
        log::trace!(
            "instantiate: module={}, ff_bytes={}, comb_bytes={}, comb_hot_bytes={}",
            self.name,
            self.ff_bytes,
            self.comb_bytes,
            self.comb_hot_bytes,
        );
        let (mut ff_values, mut comb_values) = create_buffers(
            &self.module_variable_meta,
            self.ff_bytes,
            self.comb_bytes,
            self.use_4state,
        );

        let ff_base = ff_values.as_mut_ptr();
        let comb_base = comb_values.as_mut_ptr();

        // Set ff_values_base so JIT-cached functions can compute ff_delta at runtime.
        if let Some(ref mut wl) = self.write_log_buffer {
            wl.ff_values_base = ff_base as u64;
        }

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
                let (batched, _) = batch_binary_statements(s);
                // Merge all Binary/BinaryBatch/BinarySequence into a single
                // BinarySequence to reduce per-cycle function call overhead.
                let sequenced = sequence_event_statements(batched);
                (event.clone(), sequenced)
            })
            .collect();

        // Resolve cold chunks: first get pre-batch indices, then remap after batching
        let pre_cold_chunks = self.comb_statements.resolve_cold_chunks();

        let (comb_statements, batch_index_map) = batch_binary_statements(
            self.comb_statements
                .to_statements(ff_ptr, ff_len, comb_ptr, comb_len, self.use_4state),
        );

        // Remap cold chunk stmt_index from pre-batch to post-batch
        let cold_chunks: Vec<ColdChunk> = pre_cold_chunks
            .into_iter()
            .map(|mut c| {
                c.stmt_index = batch_index_map[c.stmt_index];
                c
            })
            .collect();

        let (ff_commit_entries, _ff_entry_names) =
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
            comb_hot_size: self.comb_hot_bytes,
            required_comb_passes: self.required_comb_passes,
            ff_commit_entries,
            cold_chunks,
            cold_dirty_flag_offset: self.cold_dirty_flag_offset as usize,
            comb_activity_meta: self.comb_statements.activity.clone(),
            block_stmt_ranges: Self::compute_block_stmt_ranges(&self.comb_statements),
            event_comb_writes: self.event_comb_writes.clone(),
            event_comb_dirty_flag_offset: self.event_comb_dirty_flag_offset,
            write_log_buffer: self.write_log_buffer.take(),
        }
    }

    /// Compute block→statement index ranges from ProtoStatements.
    fn compute_block_stmt_ranges(stmts: &ProtoStatements) -> Vec<(usize, usize)> {
        let mut ranges = Vec::with_capacity(stmts.blocks.len());
        let mut idx = 0;
        for block in &stmts.blocks {
            let count = match block {
                ProtoStatementBlock::Interpreted(proto) => proto.len(),
                ProtoStatementBlock::Compiled(_) => 1,
            };
            ranges.push((idx, idx + count));
            idx += count;
        }
        ranges
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
            for block in &stmts.blocks {
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
#[cfg(not(target_family = "wasm"))]
const JIT_CHUNK_SIZE: usize = 256;

#[cfg(not(target_family = "wasm"))]
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
            match cranelift::build_binary_no_cache(context, chunk.clone()) {
                Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                None => blocks.push(ProtoStatementBlock::Interpreted(chunk)),
            }
        }
    }
}

#[cfg(target_family = "wasm")]
fn try_jit(_context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
    ProtoStatements {
        blocks: vec![ProtoStatementBlock::Interpreted(proto)],
        cold_chunks: vec![],
        activity: vec![],
    }
}

#[cfg(not(target_family = "wasm"))]
fn try_jit(context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
    if !context.config.use_jit {
        return ProtoStatements {
            blocks: vec![ProtoStatementBlock::Interpreted(proto)],
            cold_chunks: vec![],
            activity: vec![],
        };
    }

    // Event-phase JIT: redirect comb stores to the write-log (strict NBA).
    let saved_in_event = context.in_event;
    context.in_event = true;
    let result = try_jit_inner(context, proto);
    context.in_event = saved_in_event;
    result
}

#[cfg(not(target_family = "wasm"))]
fn try_jit_inner(context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
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

    ProtoStatements {
        blocks,
        cold_chunks: vec![],
        activity: vec![],
    }
}

/// JIT with load_cache disabled for unified comb.
/// CompiledBlocks (child comb functions) may modify comb values between
/// cached loads, so load_cache must be disabled for correctness.
#[cfg(target_family = "wasm")]
fn try_jit_no_cache(
    _context: &mut Context,
    proto: Vec<ProtoStatement>,
    _comb_hot_bytes: usize,
    _event_reads: &HashSet<isize>,
    _event_comb_writes: &HashSet<isize>,
    _event_comb_to_ff: &HashMap<isize, Vec<(usize, usize)>>,
) -> ProtoStatements {
    ProtoStatements {
        blocks: vec![ProtoStatementBlock::Interpreted(proto)],
        cold_chunks: vec![],
        activity: vec![],
    }
}

#[cfg(not(target_family = "wasm"))]
fn try_jit_no_cache(
    context: &mut Context,
    proto: Vec<ProtoStatement>,
    comb_hot_bytes: usize,
    event_reads: &HashSet<isize>,
    event_comb_writes: &HashSet<isize>,
    event_comb_to_ff: &HashMap<isize, Vec<(usize, usize)>>,
) -> ProtoStatements {
    use crate::ir::statement::ColdChunkMeta;

    if !context.config.use_jit {
        return ProtoStatements {
            blocks: vec![ProtoStatementBlock::Interpreted(proto)],
            cold_chunks: vec![],
            activity: vec![],
        };
    }

    // Try compiling ALL comb statements into a single JIT function.
    // This eliminates inter-chunk function call overhead and extends
    // load CSE scope across the entire comb evaluation.
    {
        // Compute store_elim for the entire function
        let mut global_comb_reads: HashMap<isize, usize> = HashMap::default();
        for stmt in &proto {
            let mut ins = vec![];
            let mut outs = vec![];
            stmt.gather_variable_offsets(&mut ins, &mut outs);
            for off in ins {
                if !off.is_ff() {
                    *global_comb_reads.entry(off.raw()).or_insert(0) += 1;
                }
            }
        }
        let mut store_elim_offsets: HashSet<VarOffset> = HashSet::default();
        for stmt in &proto {
            let mut outs = vec![];
            let mut ins = vec![];
            stmt.gather_variable_offsets(&mut ins, &mut outs);
            for off in outs {
                if !off.is_ff() && !event_reads.contains(&off.raw()) {
                    let global_count = global_comb_reads.get(&off.raw()).copied().unwrap_or(0);
                    let mut local_count = 0;
                    for off2 in &ins {
                        if off2.raw() == off.raw() && !off2.is_ff() {
                            local_count += 1;
                        }
                    }
                    if global_count == local_count {
                        store_elim_offsets.insert(off);
                    }
                }
            }
        }

        // Collect activity metadata for the single-block case
        let mut single_comb_to_ff = build_comb_to_ff_map(&proto);
        for (comb_off, ff_deps) in event_comb_to_ff {
            let entry = single_comb_to_ff.entry(*comb_off).or_default();
            entry.extend(ff_deps.iter().cloned());
            entry.sort();
            entry.dedup();
        }
        let activity = vec![collect_chunk_activity_with_transitive(
            &proto,
            &single_comb_to_ff,
            event_comb_writes,
            comb_hot_bytes,
        )];

        if let Some(func) = crate::cranelift::build_binary_comb_cached_with_store_elim(
            context,
            proto.clone(),
            store_elim_offsets,
        ) {
            log::info!(
                "Single-function comb JIT: {} stmts compiled into 1 function",
                proto.len()
            );
            let blocks = vec![ProtoStatementBlock::Compiled(func)];
            return ProtoStatements {
                blocks,
                cold_chunks: vec![],
                activity,
            };
        }
        log::info!("Single-function comb JIT failed, falling back to per-chunk");
    }

    // Build transitive FF dependency map for activity gating.
    let mut comb_to_ff = build_comb_to_ff_map(&proto);

    // Merge event FF→comb dependencies: for comb offsets written by event statements,
    // add their FF dependencies so activity gating can track them properly.
    for (comb_off, ff_deps) in event_comb_to_ff {
        let entry = comb_to_ff.entry(*comb_off).or_default();
        entry.extend(ff_deps.iter().cloned());
        entry.sort();
        entry.dedup();
    }

    // Pre-compute per-statement comb reads for store elimination.
    // For each comb offset, count how many statements read it.
    // This lets each JIT chunk determine which offsets are only read
    // within that chunk (and thus can skip memory stores).
    let mut global_comb_reads: HashMap<isize, usize> = HashMap::default();
    for stmt in &proto {
        let mut ins = vec![];
        let mut outs = vec![];
        stmt.gather_variable_offsets(&mut ins, &mut outs);
        for off in ins {
            if !off.is_ff() {
                *global_comb_reads.entry(off.raw()).or_insert(0) += 1;
            }
        }
    }

    let mut blocks: Vec<ProtoStatementBlock> = Vec::new();
    let mut cold_chunks: Vec<ColdChunkMeta> = Vec::new();

    /// Classify a chunk of statements: check if it accesses a large cold array
    /// (DynamicVariable with >256 elements in cold comb region).
    fn classify_chunk(stmts: &[ProtoStatement], comb_hot_bytes: usize) -> Option<ColdChunkMeta> {
        if comb_hot_bytes == 0 {
            return None;
        }
        // Check if any statement contains a DynamicVariable access to a large cold array
        let has_cold = stmts
            .iter()
            .any(|s| s.has_cold_array_access(comb_hot_bytes));
        if !has_cold {
            return None;
        }

        // Collect hot input ranges for snapshot comparison
        let mut input_ranges = vec![];
        for s in stmts {
            s.gather_input_ranges(&mut input_ranges);
        }

        // Separate into comb and FF inputs, dedup.
        // gather_input_ranges already excludes large array bases (DynamicVariable),
        // so ALL remaining comb inputs are "hot" (non-array) and should be tracked.
        let mut hot_comb_set: Vec<(usize, usize)> = vec![];
        let mut ff_set: Vec<(usize, usize)> = vec![];
        for (off, nb) in &input_ranges {
            match off {
                VarOffset::Comb(o) => {
                    let entry = (*o as usize, *nb);
                    if !hot_comb_set.contains(&entry) {
                        hot_comb_set.push(entry);
                    }
                }
                VarOffset::Ff(o) => {
                    let entry = (*o as usize, *nb);
                    if !ff_set.contains(&entry) {
                        ff_set.push(entry);
                    }
                }
            }
        }

        Some(ColdChunkMeta {
            block_index: 0, // will be set by caller
            hot_comb_inputs: hot_comb_set,
            ff_inputs: ff_set,
        })
    }

    /// Compile a chunk of hot statements with chunk-local store elimination.
    /// Comb offsets whose reads are entirely within this chunk can skip
    /// memory stores — values are forwarded via load_cache (registers).
    #[allow(clippy::too_many_arguments)]
    fn compile_hot_chunk(
        context: &mut Context,
        blocks: &mut Vec<ProtoStatementBlock>,
        activity: &mut Vec<ChunkActivityMeta>,
        chunk: Vec<ProtoStatement>,
        global_comb_reads: &HashMap<isize, usize>,
        event_reads: &HashSet<isize>,
        comb_to_ff: &HashMap<isize, Vec<(usize, usize)>>,
        event_comb_writes: &HashSet<isize>,
        comb_hot_bytes: usize,
    ) {
        if chunk.is_empty() {
            return;
        }
        activity.push(collect_chunk_activity_with_transitive(
            &chunk,
            comb_to_ff,
            event_comb_writes,
            comb_hot_bytes,
        ));

        // Compute store_elim: comb offsets written by top-level simple Assign
        // whose ALL reads are within this chunk (not read by other chunks or events).
        // Offsets written inside If blocks are excluded because store_elim values
        // are not in memory: if an If arm writes the same offset (with store_elim
        // disabled), the merge block may lose the cached value and reload stale data.
        let mut chunk_reads: HashMap<isize, usize> = HashMap::default();
        let mut chunk_writes: HashSet<VarOffset> = HashSet::default();
        let mut if_writes: HashSet<VarOffset> = HashSet::default();
        for s in &chunk {
            let mut ins = vec![];
            let mut outs = vec![];
            s.gather_variable_offsets(&mut ins, &mut outs);
            for off in ins {
                if !off.is_ff() {
                    *chunk_reads.entry(off.raw()).or_insert(0) += 1;
                }
            }
            // Only TOP-LEVEL simple Assign (no select) can be store-eliminated
            if let ProtoStatement::Assign(a) = s
                && !a.dst.is_ff()
                && a.select.is_none()
            {
                chunk_writes.insert(a.dst);
            }
            // Collect offsets referenced inside If blocks for store_elim.
            if let ProtoStatement::If(if_stmt) = s {
                let mut ins = vec![];
                let mut outs = vec![];
                for arm_s in if_stmt.true_side.iter().chain(if_stmt.false_side.iter()) {
                    arm_s.gather_variable_offsets(&mut ins, &mut outs);
                }
                for off in ins.into_iter().chain(outs) {
                    if_writes.insert(off);
                }
            }
        }

        let mut store_elim: HashSet<VarOffset> = HashSet::default();
        for dst in &chunk_writes {
            let raw = dst.raw();
            if event_reads.contains(&raw) {
                continue;
            }
            // Exclude offsets also written inside If blocks
            if if_writes.contains(dst) {
                continue;
            }
            let global = global_comb_reads.get(&raw).copied().unwrap_or(0);
            let local = chunk_reads.get(&raw).copied().unwrap_or(0);
            if global == local {
                store_elim.insert(*dst);
            }
        }

        let _store_elim_count = store_elim.len();
        match cranelift::build_binary_comb_cached(context, chunk.clone()) {
            Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
            None => blocks.push(ProtoStatementBlock::Interpreted(chunk)),
        }
    }

    /// Push a jittable group, splitting cold statements into individual 1-statement
    /// chunks for activity-skip while keeping hot statements in large chunks.
    #[allow(clippy::too_many_arguments)]
    fn push_jittable_group(
        context: &mut Context,
        blocks: &mut Vec<ProtoStatementBlock>,
        activity: &mut Vec<ChunkActivityMeta>,
        cold_chunks: &mut Vec<ColdChunkMeta>,
        group: Vec<ProtoStatement>,
        comb_hot_bytes: usize,
        global_comb_reads: &HashMap<isize, usize>,
        event_reads: &HashSet<isize>,
        comb_to_ff: &HashMap<isize, Vec<(usize, usize)>>,
        event_comb_writes: &HashSet<isize>,
    ) {
        if comb_hot_bytes == 0 {
            // No cold region — compile everything as hot chunks
            let chunks: Vec<Vec<ProtoStatement>> = if group.len() <= JIT_CHUNK_SIZE {
                vec![group]
            } else {
                group.chunks(JIT_CHUNK_SIZE).map(|c| c.to_vec()).collect()
            };
            for chunk in chunks {
                compile_hot_chunk(
                    context,
                    blocks,
                    activity,
                    chunk,
                    global_comb_reads,
                    event_reads,
                    comb_to_ff,
                    event_comb_writes,
                    comb_hot_bytes,
                );
            }
            return;
        }

        // Split at cold statement boundaries, preserving execution order.
        // Cold statements become individual 1-statement chunks with ColdChunkMeta.
        // Hot statements between them are grouped into regular-sized chunks.
        let mut hot_buf: Vec<ProtoStatement> = Vec::new();

        for stmt in group {
            if stmt.has_cold_array_access(comb_hot_bytes) {
                // Flush accumulated hot statements first
                if !hot_buf.is_empty() {
                    let hot = std::mem::take(&mut hot_buf);
                    let chunks: Vec<Vec<ProtoStatement>> = if hot.len() <= JIT_CHUNK_SIZE {
                        vec![hot]
                    } else {
                        hot.chunks(JIT_CHUNK_SIZE).map(|c| c.to_vec()).collect()
                    };
                    for chunk in chunks {
                        compile_hot_chunk(
                            context,
                            blocks,
                            activity,
                            chunk,
                            global_comb_reads,
                            event_reads,
                            comb_to_ff,
                            event_comb_writes,
                            comb_hot_bytes,
                        );
                    }
                }

                // Compile cold statement as individual 1-statement chunk
                let cold_stmt = vec![stmt];
                let block_idx = blocks.len();
                if let Some(mut meta) = classify_chunk(&cold_stmt, comb_hot_bytes) {
                    meta.block_index = block_idx;
                    cold_chunks.push(meta);
                }
                activity.push(collect_chunk_activity_with_transitive(
                    &cold_stmt,
                    comb_to_ff,
                    event_comb_writes,
                    comb_hot_bytes,
                ));
                match cranelift::build_binary_no_cache(context, cold_stmt.clone()) {
                    Some(func) => blocks.push(ProtoStatementBlock::Compiled(func)),
                    None => blocks.push(ProtoStatementBlock::Interpreted(cold_stmt)),
                }
            } else {
                hot_buf.push(stmt);
            }
        }

        // Flush remaining hot statements
        if !hot_buf.is_empty() {
            let chunks: Vec<Vec<ProtoStatement>> = if hot_buf.len() <= JIT_CHUNK_SIZE {
                vec![hot_buf]
            } else {
                hot_buf.chunks(JIT_CHUNK_SIZE).map(|c| c.to_vec()).collect()
            };
            for chunk in chunks {
                compile_hot_chunk(
                    context,
                    blocks,
                    activity,
                    chunk,
                    global_comb_reads,
                    event_reads,
                    comb_to_ff,
                    event_comb_writes,
                    comb_hot_bytes,
                );
            }
        }
    }

    let mut activity: Vec<ChunkActivityMeta> = Vec::new();
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
                    push_jittable_group(
                        context,
                        &mut blocks,
                        &mut activity,
                        &mut cold_chunks,
                        group,
                        comb_hot_bytes,
                        &global_comb_reads,
                        event_reads,
                        &comb_to_ff,
                        event_comb_writes,
                    );
                } else {
                    // Interpreted blocks: collect activity too
                    activity.push(collect_chunk_activity_with_transitive(
                        &group,
                        &comb_to_ff,
                        event_comb_writes,
                        comb_hot_bytes,
                    ));
                    blocks.push(ProtoStatementBlock::Interpreted(group));
                }
            }
            current_jittable = Some(jittable);
            current_group.push(stmt);
        }
    }

    if let Some(was_jittable) = current_jittable {
        if was_jittable {
            push_jittable_group(
                context,
                &mut blocks,
                &mut activity,
                &mut cold_chunks,
                current_group,
                comb_hot_bytes,
                &global_comb_reads,
                event_reads,
                &comb_to_ff,
                event_comb_writes,
            );
        } else {
            activity.push(collect_chunk_activity_with_transitive(
                &current_group,
                &comb_to_ff,
                event_comb_writes,
                comb_hot_bytes,
            ));
            blocks.push(ProtoStatementBlock::Interpreted(current_group));
        }
    }

    if !cold_chunks.is_empty() {
        log::info!(
            "try_jit_no_cache: {} cold chunks out of {} blocks (comb_hot_bytes={})",
            cold_chunks.len(),
            blocks.len(),
            comb_hot_bytes,
        );
    }

    // Build per-block activity metadata from the original ProtoStatements.
    // We re-scan the blocks: for Compiled blocks we don't have the original stmts,
    // so we collect metadata BEFORE compilation.
    // Actually, we already lost the original stmts after compilation above.
    // We'll collect metadata in a second pass approach below.

    if !activity.is_empty() {
        let with_dyn = activity.iter().filter(|a| a.has_dynamic_ff_read).count();
        log::debug!(
            "comb activity: {} chunks ({} with dynamic FF reads)",
            activity.len(),
            with_dyn,
        );
    }

    ProtoStatements {
        blocks,
        cold_chunks,
        activity,
    }
}

/// Build a map from comb offset → set of FF byte ranges that transitively affect it.
/// Uses the full list of comb statements (before chunking) to trace dependencies.
fn build_comb_to_ff_map(all_stmts: &[ProtoStatement]) -> HashMap<isize, Vec<(usize, usize)>> {
    // Phase 1: for each comb Assign, collect direct FF reads and comb reads
    struct StmtInfo {
        dst_offset: isize,
        direct_ff: Vec<(usize, usize)>,
        comb_deps: Vec<isize>,
    }

    let mut infos: Vec<StmtInfo> = Vec::new();

    for stmt in all_stmts {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        stmt.gather_variable_offsets(&mut inputs, &mut outputs);

        let mut direct_ff = Vec::new();
        let mut comb_deps = Vec::new();
        for off in &inputs {
            match off {
                VarOffset::Ff(o) => direct_ff.push((*o as usize, 8)),
                VarOffset::Comb(o) => comb_deps.push(*o),
            }
        }

        direct_ff.sort();
        direct_ff.dedup();
        comb_deps.sort();
        comb_deps.dedup();

        // Collect comb output offsets for this statement
        let comb_outputs: Vec<isize> = outputs
            .iter()
            .filter(|o| !o.is_ff())
            .map(|o| o.raw())
            .collect();

        // Each output gets the same FF dependency set
        for dst in comb_outputs {
            infos.push(StmtInfo {
                dst_offset: dst,
                direct_ff: direct_ff.clone(),
                comb_deps: comb_deps.clone(),
            });
        }
    }

    // Phase 2+3: build comb_offset → transitive FF map
    // Process in topological order (statements are already sorted).
    // Union with existing entries when multiple statements write the same offset.
    let mut comb_ff: HashMap<isize, Vec<(usize, usize)>> = HashMap::default();
    for info in &infos {
        let mut all_ff = info.direct_ff.clone();
        // Add transitive FF deps from comb dependencies
        for &dep in &info.comb_deps {
            if let Some(dep_ff) = comb_ff.get(&dep) {
                all_ff.extend(dep_ff.iter().cloned());
            }
        }
        // Union with existing entry (multiple stmts may write same offset)
        let entry = comb_ff.entry(info.dst_offset).or_default();
        entry.extend(all_ff);
        entry.sort();
        entry.dedup();
    }

    comb_ff
}

/// Collect activity metadata for a set of ProtoStatements (one chunk).
/// `comb_to_ff` provides transitive FF dependencies for comb offsets.
/// `event_comb_writes` are comb offsets written by event statements.
fn collect_chunk_activity_with_transitive(
    stmts: &[ProtoStatement],
    comb_to_ff: &HashMap<isize, Vec<(usize, usize)>>,
    event_comb_writes: &HashSet<isize>,
    comb_hot_bytes: usize,
) -> ChunkActivityMeta {
    let mut ff_reads: Vec<(usize, usize)> = Vec::new();
    let mut comb_writes: Vec<isize> = Vec::new();
    let mut comb_reads: Vec<isize> = Vec::new();
    let mut has_dynamic_ff_read = false;

    for stmt in stmts {
        collect_stmt_activity(
            stmt,
            &mut ff_reads,
            &mut comb_writes,
            &mut comb_reads,
            &mut has_dynamic_ff_read,
        );
    }

    // Check if any comb_read is an event-written offset, split by hot/cold
    let mut reads_hot_event_comb = false;
    let mut reads_cold_event_comb = false;
    for off in &comb_reads {
        if event_comb_writes.contains(off) {
            if *off >= 0 && (*off as usize) < comb_hot_bytes {
                reads_hot_event_comb = true;
            } else {
                reads_cold_event_comb = true;
            }
        }
    }

    // Add transitive FF deps: for each comb_read, add its FF dependencies
    for &comb_off in &comb_reads {
        if let Some(trans_ff) = comb_to_ff.get(&comb_off) {
            ff_reads.extend(trans_ff.iter().cloned());
        }
    }

    // Dedup
    ff_reads.sort();
    ff_reads.dedup();
    comb_writes.sort();
    comb_writes.dedup();
    comb_reads.sort();
    comb_reads.dedup();

    ChunkActivityMeta {
        ff_reads,
        comb_writes,
        comb_reads,
        has_dynamic_ff_read,
        reads_hot_event_comb,
        reads_cold_event_comb,
    }
}

/// Collect activity metadata without transitive deps (for event stmts).
#[allow(dead_code)]
fn collect_chunk_activity(stmts: &[ProtoStatement]) -> ChunkActivityMeta {
    collect_chunk_activity_with_transitive(stmts, &HashMap::default(), &HashSet::default(), 0)
}

fn collect_stmt_activity(
    stmt: &ProtoStatement,
    ff_reads: &mut Vec<(usize, usize)>,
    comb_writes: &mut Vec<isize>,
    comb_reads: &mut Vec<isize>,
    has_dynamic_ff: &mut bool,
) {
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    stmt.gather_variable_offsets(&mut inputs, &mut outputs);

    for off in &inputs {
        match off {
            VarOffset::Ff(o) => {
                ff_reads.push((*o as usize, 8)); // conservative size
            }
            VarOffset::Comb(o) => {
                comb_reads.push(*o);
            }
        }
    }
    for off in &outputs {
        match off {
            VarOffset::Ff(_) => {} // comb chunks don't write FF
            VarOffset::Comb(o) => {
                comb_writes.push(*o);
            }
        }
    }

    // Check for DynamicVariable with FF base
    check_dynamic_ff_reads(stmt, has_dynamic_ff);
}

fn check_dynamic_ff_reads(stmt: &ProtoStatement, has_dynamic_ff: &mut bool) {
    match stmt {
        ProtoStatement::Assign(x) => {
            check_expr_dynamic_ff(&x.expr, has_dynamic_ff);
        }
        ProtoStatement::AssignDynamic(x) => {
            check_expr_dynamic_ff(&x.expr, has_dynamic_ff);
            check_expr_dynamic_ff(&x.dst_index_expr, has_dynamic_ff);
        }
        ProtoStatement::If(x) => {
            if let Some(c) = &x.cond {
                check_expr_dynamic_ff(c, has_dynamic_ff);
            }
            for s in &x.true_side {
                check_dynamic_ff_reads(s, has_dynamic_ff);
            }
            for s in &x.false_side {
                check_dynamic_ff_reads(s, has_dynamic_ff);
            }
        }
        ProtoStatement::For(x) => {
            for s in &x.body {
                check_dynamic_ff_reads(s, has_dynamic_ff);
            }
        }
        _ => {}
    }
}

fn check_expr_dynamic_ff(expr: &ProtoExpression, has_dynamic_ff: &mut bool) {
    match expr {
        ProtoExpression::DynamicVariable { base_offset, .. } if base_offset.is_ff() => {
            *has_dynamic_ff = true;
        }
        ProtoExpression::Unary { x, .. } => check_expr_dynamic_ff(x, has_dynamic_ff),
        ProtoExpression::Binary { x, y, .. } => {
            check_expr_dynamic_ff(x, has_dynamic_ff);
            check_expr_dynamic_ff(y, has_dynamic_ff);
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            check_expr_dynamic_ff(cond, has_dynamic_ff);
            check_expr_dynamic_ff(true_expr, has_dynamic_ff);
            check_expr_dynamic_ff(false_expr, has_dynamic_ff);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (e, _, _) in elements {
                check_expr_dynamic_ff(e, has_dynamic_ff);
            }
        }
        _ => {}
    }
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

    // Phase 2: Expand all CompiledBlocks and SequentialBlocks and retry.
    let has_expandable = table.values().any(|x| {
        matches!(x, ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty())
            || matches!(x, ProtoStatement::SequentialBlock(_))
    });

    if has_expandable {
        let mut next_id = table.keys().max().copied().unwrap_or(0) + 1;
        let expandable_ids: Vec<usize> = table
            .iter()
            .filter_map(|(id, x)| {
                if matches!(x, ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty())
                    || matches!(x, ProtoStatement::SequentialBlock(_))
                {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        for eid in expandable_ids {
            match table.remove(&eid) {
                Some(ProtoStatement::CompiledBlock(cb)) => {
                    for stmt in cb.original_stmts {
                        table.insert(next_id, stmt);
                        next_id += 1;
                    }
                }
                Some(ProtoStatement::SequentialBlock(body)) => {
                    for stmt in body {
                        table.insert(next_id, stmt);
                        next_id += 1;
                    }
                }
                other => {
                    if let Some(s) = other {
                        table.insert(eid, s);
                    }
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
/// Batch consecutive Binary statements with the same func into BinaryBatch.
/// Returns (batched statements, pre→post index mapping).
/// The mapping maps each pre-batch index to the post-batch index.
fn batch_binary_statements(stmts: Vec<Statement>) -> (Vec<Statement>, Vec<usize>) {
    let mut result: Vec<Statement> = Vec::with_capacity(stmts.len());
    let mut index_map: Vec<usize> = Vec::with_capacity(stmts.len());

    for stmt in stmts {
        match stmt {
            Statement::Binary(func, ff, comb) => {
                let func_addr = func as usize;
                match result.last_mut() {
                    Some(Statement::BinaryBatch(batch_func, args))
                        if *batch_func as usize == func_addr =>
                    {
                        args.push((ff, comb));
                        index_map.push(result.len() - 1);
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
                        index_map.push(result.len() - 1);
                    }
                    _ => {
                        index_map.push(result.len());
                        result.push(Statement::Binary(func, ff, comb));
                    }
                }
            }
            other => {
                index_map.push(result.len());
                result.push(other);
            }
        }
    }

    (result, index_map)
}

/// Merge consecutive Binary statements into a single BinarySequence.
/// Non-Binary statements are kept as-is. This reduces per-cycle indirect
/// call overhead for event evaluation (16 calls → 1 tight loop).
fn sequence_event_statements(stmts: Vec<Statement>) -> Vec<Statement> {
    if stmts.len() <= 1 {
        return stmts;
    }
    let mut result: Vec<Statement> = Vec::new();
    let mut seq: Vec<(FuncPtr, *const u8, *const u8)> = Vec::new();

    let flush_seq = |seq: &mut Vec<(FuncPtr, *const u8, *const u8)>,
                     result: &mut Vec<Statement>| {
        match seq.len() {
            0 => {}
            1 => {
                let (f, ff, comb) = seq[0];
                result.push(Statement::Binary(f, ff, comb));
            }
            _ => {
                result.push(Statement::BinarySequence(std::mem::take(seq)));
            }
        }
    };

    for stmt in stmts {
        match stmt {
            Statement::Binary(func, ff, comb) => {
                seq.push((func, ff, comb));
            }
            Statement::BinaryBatch(func, args) => {
                for (ff, comb) in args {
                    seq.push((func, ff, comb));
                }
            }
            Statement::BinarySequence(s) => {
                seq.extend(s);
            }
            other => {
                flush_seq(&mut seq, &mut result);
                result.push(other);
            }
        }
    }
    flush_seq(&mut seq, &mut result);
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
        // Force all always_ff-assigned variables to FF classification when JIT is enabled.
        // With write-log sparse commit, ff_commit cost is O(writes) not O(all_entries),
        // force_all_ff disabled: reclassifying ff_opt vars back to FF
        // reduces optimizer inlining opportunities and worsens performance.

        let ff_start = context.ff_total_bytes as isize;
        let comb_start = context.comb_total_bytes as isize;
        let (variable_meta, ff_bytes, comb_bytes, comb_hot_bytes) = create_variable_meta(
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

        // The write-log buffer pointer is embedded as an immediate in JIT
        // code, so it must be allocated before child module JIT compilation.
        // Skip for small hermetic designs where the per-cycle overhead
        // exceeds the sparse-commit and bit-select NBA benefits.
        let has_child_inst = src
            .declarations
            .iter()
            .any(|d| matches!(d, air::Declaration::Inst(_)));
        let skip_write_log = !has_child_inst && ff_bytes < 256;
        if !skip_write_log {
            let write_log_buffer = Box::new(WriteLogBuffer::default());
            log::info!(
                "Write-log: heap buffer entries_ptr=0x{:x}, count_ptr=0x{:x}",
                write_log_buffer.entries.as_ptr() as i64,
                &write_log_buffer.count as *const u64 as i64,
            );
            context.write_log_buffer = Some(write_log_buffer);
        } else {
            log::info!("Write-log: skipped (small hermetic design)");
        }

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
        let unified_sorted = reorder_by_level(unified_sorted);

        // Inline CompiledBlocks: expand into their original ProtoStatements.
        // This eliminates function call overhead and extends load_cache scope
        // across module boundaries.
        let pre_expand = unified_sorted.len();
        let cb_count = unified_sorted
            .iter()
            .filter(|s| matches!(s, ProtoStatement::CompiledBlock(_)))
            .count();
        let unified_expanded: Vec<ProtoStatement> = unified_sorted
            .into_iter()
            .flat_map(|stmt| match stmt {
                ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty() => {
                    cb.original_stmts
                }
                other => vec![other],
            })
            .collect();
        let post_expand = unified_expanded.len();
        let remaining_cb = unified_expanded
            .iter()
            .filter(|s| matches!(s, ProtoStatement::CompiledBlock(_)))
            .count();
        if cb_count > 0 {
            log::info!(
                "CB inline: {} stmts ({} CBs) -> {} stmts ({} remaining)",
                pre_expand,
                cb_count,
                post_expand,
                remaining_cb
            );
        }

        // Collect comb offsets read by event statements (these must not be
        // eliminated or inlined by the comb optimizer).
        let mut event_reads: HashSet<isize> = HashSet::default();
        let mut event_comb_writes: HashSet<isize> = HashSet::default();
        // Build event FF→comb dependency map: for each comb offset written by
        // event statements, collect the FF offsets that the event reads.
        // This lets activity gating track which FF changes affect event-written comb vars.
        //
        // Build preliminary comb_to_ff from comb statements FIRST, so we can
        // transitively expand event comb reads to their FF dependencies.
        // Without this, event statements that read comb variables miss the
        // transitive FF deps, causing activity gating to miss activations.
        let preliminary_comb_to_ff = build_comb_to_ff_map(&unified_expanded);
        let mut event_comb_to_ff: HashMap<isize, Vec<(usize, usize)>> = HashMap::default();
        for stmts in all_event_statements.values() {
            // Collect all FF reads and comb writes across all statements in this event group
            let mut group_ff_reads: Vec<(usize, usize)> = Vec::new();
            let mut group_comb_writes: Vec<isize> = Vec::new();
            for s in stmts {
                let mut ins = vec![];
                let mut outs = vec![];
                s.gather_variable_offsets(&mut ins, &mut outs);
                for off in &ins {
                    if !off.is_ff() {
                        event_reads.insert(off.raw());
                        // Transitively expand comb reads to their FF dependencies
                        if let Some(ff_deps) = preliminary_comb_to_ff.get(&off.raw()) {
                            group_ff_reads.extend(ff_deps.iter().cloned());
                        }
                    } else {
                        group_ff_reads.push((off.raw() as usize, 8));
                    }
                }
                for off in &outs {
                    if !off.is_ff() {
                        event_comb_writes.insert(off.raw());
                        group_comb_writes.push(off.raw());
                    }
                }
                // CompiledBlock's gather_variable_offsets filters out FF offsets,
                // so FF reads are missing from `ins`. Extract them directly from
                // input_offsets to build correct event_comb_to_ff dependencies.
                if let ProtoStatement::CompiledBlock(cb) = s {
                    for off in &cb.input_offsets {
                        if off.is_ff() {
                            group_ff_reads.push((off.raw() as usize, 8));
                        } else if let Some(ff_deps) = preliminary_comb_to_ff.get(&off.raw()) {
                            group_ff_reads.extend(ff_deps.iter().cloned());
                        }
                    }
                }
            }
            group_ff_reads.sort();
            group_ff_reads.dedup();
            group_comb_writes.sort();
            group_comb_writes.dedup();
            // Map each comb output to its event group's FF dependencies
            for &comb_off in &group_comb_writes {
                let entry = event_comb_to_ff.entry(comb_off).or_default();
                entry.extend(group_ff_reads.iter().cloned());
                entry.sort();
                entry.dedup();
            }
        }

        if !event_comb_writes.is_empty() {
            log::info!(
                "event_comb_writes (pre-opt): {} offsets",
                event_comb_writes.len()
            );
        }

        // Merged comb+event optimization: inline single-use comb vars into
        // event expressions and DCE unused comb vars in a single unified pass.
        // This replaces the old two-phase approach (optimize_merged then
        // optimize_unified) which broke because the second pass could remove
        // comb vars that the first pass had inlined into event expressions.
        // Convert eligible If blocks to Ternary (select) expressions.
        // This eliminates basic block splits in JIT, avoiding load_cache
        // clears at If boundaries.
        #[cfg(not(target_family = "wasm"))]
        let unified_expanded = crate::ir::optimize::flatten_if_to_select(unified_expanded);

        #[cfg(not(target_family = "wasm"))]
        let (unified_expanded, all_event_statements, event_reads) = {
            // Convert events to indexed groups for the optimizer
            let event_keys: Vec<Event> = all_event_statements.keys().cloned().collect();
            let event_groups: Vec<(usize, Vec<ProtoStatement>)> = event_keys
                .iter()
                .enumerate()
                .map(|(i, k)| (i, all_event_statements.remove(k).unwrap()))
                .collect();

            let (opt_comb, opt_events) = crate::ir::optimize::optimize_top_level(
                unified_expanded,
                event_groups,
                &event_reads,
            );

            // Reconstruct HashMap
            let mut result_events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
            for (idx, stmts) in opt_events {
                result_events.insert(event_keys[idx].clone(), stmts);
            }
            // Recompute event_reads from transformed events (Phase 2 inlining
            // may have introduced new comb references that weren't in the original).
            let mut updated_reads: HashSet<isize> = HashSet::default();
            for stmts in result_events.values() {
                for s in stmts {
                    let mut ins = vec![];
                    let mut outs = vec![];
                    s.gather_variable_offsets(&mut ins, &mut outs);
                    for off in ins {
                        if !off.is_ff() {
                            updated_reads.insert(off.raw());
                        }
                    }
                }
            }

            (opt_comb, result_events, updated_reads)
        };

        #[cfg(target_family = "wasm")]
        let event_reads = event_reads; // unchanged on wasm

        let required_comb_passes = compute_required_passes(&unified_expanded);
        log::info!(
            "comb_stmts={}, required_comb_passes={}",
            unified_expanded.len(),
            required_comb_passes
        );

        // Recompute event_comb_writes and event_comb_to_ff from POST-OPTIMIZATION
        // event statements using POST-OPTIMIZATION comb_to_ff. This ensures FF
        // dependency chains aren't broken by optimize_top_level's comb→event inlining.
        #[cfg(not(target_family = "wasm"))]
        {
            let post_opt_comb_to_ff = build_comb_to_ff_map(&unified_expanded);
            event_comb_writes.clear();
            event_comb_to_ff.clear();
            for stmts in all_event_statements.values() {
                let mut group_ff_reads: Vec<(usize, usize)> = Vec::new();
                let mut group_comb_writes: Vec<isize> = Vec::new();
                for s in stmts {
                    let mut ins = vec![];
                    let mut outs = vec![];
                    s.gather_variable_offsets(&mut ins, &mut outs);
                    for off in &ins {
                        if off.is_ff() {
                            group_ff_reads.push((off.raw() as usize, 8));
                        } else if let Some(ff_deps) = post_opt_comb_to_ff.get(&off.raw()) {
                            group_ff_reads.extend(ff_deps.iter().cloned());
                        }
                    }
                    for off in &outs {
                        if !off.is_ff() {
                            event_comb_writes.insert(off.raw());
                            group_comb_writes.push(off.raw());
                        }
                    }
                    if let ProtoStatement::CompiledBlock(cb) = s {
                        for off in &cb.input_offsets {
                            if off.is_ff() {
                                group_ff_reads.push((off.raw() as usize, 8));
                            } else if let Some(ff_deps) = post_opt_comb_to_ff.get(&off.raw()) {
                                group_ff_reads.extend(ff_deps.iter().cloned());
                            }
                        }
                    }
                }
                group_ff_reads.sort();
                group_ff_reads.dedup();
                group_comb_writes.sort();
                group_comb_writes.dedup();
                for &comb_off in &group_comb_writes {
                    let entry = event_comb_to_ff.entry(comb_off).or_default();
                    entry.extend(group_ff_reads.iter().cloned());
                    entry.sort();
                    entry.dedup();
                }
            }
        }

        // Filter event_comb_writes:
        // 1. Remove offsets not read by any comb statement
        // 2. Remove offsets with complete FF deps (tracked by ff_to_chunk)
        {
            let pre_filter = event_comb_writes.len();
            let mut all_comb_reads: HashSet<isize> = HashSet::default();
            for stmt in &unified_expanded {
                let mut ins = vec![];
                let mut outs = vec![];
                stmt.gather_variable_offsets(&mut ins, &mut outs);
                for off in &ins {
                    if !off.is_ff() {
                        all_comb_reads.insert(off.raw());
                    }
                }
            }
            // Keep offset if: (a) read by comb, AND (b) has no FF deps OR is self-referencing.
            // Self-referencing: event reads AND writes the same comb offset (V = f(V_old, ...)).
            // These have temporal state that FF tracking can't capture.
            event_comb_writes.retain(|off| {
                if !all_comb_reads.contains(off) {
                    return false;
                }
                let has_ff_deps = event_comb_to_ff
                    .get(off)
                    .is_some_and(|deps| !deps.is_empty());
                let is_self_ref = event_reads.contains(off);
                // Keep if no FF deps (untraceable) or self-referencing (temporal state)
                !has_ff_deps || is_self_ref
            });
            if pre_filter > 0 || !event_comb_writes.is_empty() {
                let hot_count = event_comb_writes
                    .iter()
                    .filter(|&&o| o >= 0 && (o as usize) < comb_hot_bytes)
                    .count();
                let cold_count = event_comb_writes.len() - hot_count;
                log::info!(
                    "event_comb_writes: {} -> {} (filtered, hot={}, cold={}, comb_hot_bytes={})",
                    pre_filter,
                    event_comb_writes.len(),
                    hot_count,
                    cold_count,
                    comb_hot_bytes,
                );
                // Show first few cold event_comb_writes offsets for debugging
                let mut cold_offs: Vec<isize> = event_comb_writes
                    .iter()
                    .filter(|&&o| o < 0 || (o as usize) >= comb_hot_bytes)
                    .copied()
                    .collect();
                cold_offs.sort();
                if cold_offs.len() <= 20 {
                    log::info!("  cold event_comb_writes offsets: {:?}", cold_offs);
                } else {
                    log::info!(
                        "  cold event_comb_writes offsets (first 20): {:?}",
                        &cold_offs[..20]
                    );
                }
            }
        }
        // Reserve bytes at the end of comb_values for dirty flags.
        // These must be set BEFORE JIT compilation so event JIT can emit flag stores.
        let cold_dirty_flag_offset = context.comb_total_bytes as i64;
        context.comb_total_bytes += 1;
        let event_comb_dirty_flag_offset = context.comb_total_bytes;
        context.comb_total_bytes += 1;
        context.cold_dirty_flag_offset = Some(cold_dirty_flag_offset);
        context.event_comb_dirty_flag_offset = Some(event_comb_dirty_flag_offset as i64);
        context.comb_hot_size = comb_hot_bytes;

        // Keep the write-log buffer alive even if this module has no FF
        // variables: child-module event JIT may already have emitted log
        // appends (scheduled comb writes for strict NBA) whose pointer was
        // captured at compile time. Discarding here would leave dangling
        // references. The modest memory cost is acceptable; if needed, we
        // can later discard only when both FF commit *and* scheduled comb
        // writes are statically proven to be empty.

        let comb_statements = try_jit_no_cache(
            context,
            unified_expanded,
            comb_hot_bytes,
            &event_reads,
            &event_comb_writes,
            &event_comb_to_ff,
        );

        // Event statements preserve source order (no topological sorting).
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
            comb_hot_bytes,
            use_4state: context.config.use_4state,
            module_variable_meta,
            event_statements,
            comb_statements,
            required_comb_passes,
            cold_dirty_flag_offset,
            event_comb_writes,
            event_comb_dirty_flag_offset,
            write_log_buffer: context.write_log_buffer.take(),
        })
    }
}
