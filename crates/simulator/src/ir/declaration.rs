use crate::HashMap;
use crate::HashSet;
#[cfg(not(target_family = "wasm"))]
use crate::cranelift;
use crate::ir::context::{Context, Conv, ScopeContext};
#[cfg(not(target_family = "wasm"))]
use crate::ir::context::{JitCacheEntry, JitCachedFunc};
use crate::ir::expression::{ExpressionContext, build_dynamic_bit_select};
#[cfg(not(target_family = "wasm"))]
use crate::ir::statement::CompiledBlockStatement;
use crate::ir::statement::ProtoAssignStatement;
use crate::ir::variable::{ModuleVariableMeta, VarOffset, create_variable_meta};
use crate::ir::{Event, ProtoExpression, ProtoStatement};
use crate::simulator_error::SimulatorError;
use std::collections::VecDeque;
use veryl_analyzer::ir as air;
use veryl_parser::token_range::TokenRange;

/// Collect variable offsets from statements, filtering out internal variables
/// (those that appear in both inputs and outputs) to avoid dependency cycles
/// when the compiled block is used in analyze_dependency.
#[cfg(not(target_family = "wasm"))]
type VarOffsets = Vec<VarOffset>;

/// Collect canonical (current) FF offsets written by these statements.
#[cfg(not(target_family = "wasm"))]
fn gather_ff_canonical(stmts: &[ProtoStatement]) -> Vec<isize> {
    let mut result = HashSet::default();
    for s in stmts {
        result.extend(s.gather_ff_canonical_offsets());
    }
    result.into_iter().collect()
}

#[cfg(not(target_family = "wasm"))]
fn gather_external_offsets(stmts: &[ProtoStatement]) -> (VarOffsets, VarOffsets) {
    let mut all_inputs = vec![];
    let mut all_outputs = vec![];
    for s in stmts {
        s.gather_variable_offsets(&mut all_inputs, &mut all_outputs);
    }

    let input_set: HashSet<VarOffset> = all_inputs.iter().cloned().collect();
    let output_set: HashSet<VarOffset> = all_outputs.iter().cloned().collect();
    // Remove internal variables (both read and written) from inputs only.
    // Outputs are kept so dependent blocks see the dependency edge.
    let internal: HashSet<VarOffset> = input_set.intersection(&output_set).cloned().collect();
    all_inputs.retain(|x| !internal.contains(x));
    all_inputs.dedup();
    all_outputs.dedup();

    (all_inputs, all_outputs)
}

/// Stable topological sort of comb statements using Kahn's algorithm (BFS/FIFO).
///
/// Builds Read-After-Write (RAW) dependency edges: for each variable written by
/// statement A and read by statement B (where B != A), add edge A → B.
/// Self-references (a statement that both reads and writes the same variable)
/// are skipped to avoid false cycles.
///
/// Falls back to source order if a cycle is detected.
pub(crate) fn stable_topo_sort(statements: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    let n = statements.len();
    if n <= 1 {
        return statements;
    }

    // Gather per-statement inputs and outputs (variable offsets).
    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in &statements {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    // Build a map: variable → set of statement indices that write it.
    let mut writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        for &key in outs {
            writers.entry(key).or_default().push(i);
        }
    }

    // Build adjacency list and in-degree count for Kahn's algorithm.
    // Edge: writer_stmt → reader_stmt (RAW dependency).
    // For variables with multiple writers (sequential reassignment from inlined
    // functions), only the most recent writer before the reader is relevant.
    let mut adj: Vec<HashSet<usize>> = vec![HashSet::default(); n];
    let mut in_degree: Vec<usize> = vec![0; n];

    for (reader_idx, ins) in stmt_inputs.iter().enumerate() {
        for key in ins {
            if let Some(writer_indices) = writers.get(key) {
                if writer_indices.len() == 1 {
                    let writer_idx = writer_indices[0];
                    if writer_idx != reader_idx && adj[writer_idx].insert(reader_idx) {
                        in_degree[reader_idx] += 1;
                    }
                } else if let Some(&writer_idx) =
                    writer_indices.iter().rev().find(|&&w| w < reader_idx)
                    && adj[writer_idx].insert(reader_idx)
                {
                    in_degree[reader_idx] += 1;
                }
            }
        }
    }

    // WAW ordering: chain consecutive writers of the same variable so that
    // bit-select assigns to a packed variable keep source order.
    // Skip when next already reaches prev (would create a cycle).
    for writer_indices in writers.values() {
        for pair in writer_indices.windows(2) {
            let (prev, next) = (pair[0], pair[1]);
            let mut reachable = false;
            let mut stack = vec![next];
            let mut visited = HashSet::default();
            while let Some(node) = stack.pop() {
                if node == prev {
                    reachable = true;
                    break;
                }
                if visited.insert(node) {
                    for &succ in &adj[node] {
                        stack.push(succ);
                    }
                }
            }
            if !reachable && adj[prev].insert(next) {
                in_degree[next] += 1;
            }
        }
    }

    // Kahn's algorithm with FIFO queue (VecDeque) for stable ordering.
    // Initialize queue with zero-in-degree nodes in source order.
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, &deg) in in_degree.iter().enumerate() {
        if deg == 0 {
            queue.push_back(i);
        }
    }

    let mut sorted_indices: Vec<usize> = Vec::with_capacity(n);
    while let Some(idx) = queue.pop_front() {
        sorted_indices.push(idx);
        // Collect successors in index order for determinism
        let mut successors: Vec<usize> = adj[idx].iter().cloned().collect();
        successors.sort_unstable();
        for succ in successors {
            in_degree[succ] -= 1;
            if in_degree[succ] == 0 {
                queue.push_back(succ);
            }
        }
    }

    // If not all nodes were processed, a cycle was detected — fall back to source order.
    if sorted_indices.len() != n {
        return statements;
    }

    // Reconstruct statement list in sorted order.
    let mut result: Vec<Option<ProtoStatement>> = statements.into_iter().map(Some).collect();
    sorted_indices
        .into_iter()
        .map(|i| result[i].take().unwrap())
        .collect()
}

pub struct ProtoDeclaration {
    pub event_statements: HashMap<Event, Vec<ProtoStatement>>,
    pub comb_statements: Vec<ProtoStatement>,
    /// Post-comb functions: child comb-only JIT functions for pre-event eval.
    pub post_comb_fns: Vec<ProtoStatement>,
    pub child_modules: Vec<ModuleVariableMeta>,
    /// Full internal comb statements (before merge optimization removed them).
    /// Present only when merged comb+event functions are used.
    pub full_internal_comb: Option<Vec<ProtoStatement>>,
}

impl Conv<&air::Declaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::Declaration) -> Result<Self, SimulatorError> {
        match src {
            air::Declaration::Comb(x) => {
                let mut comb_statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    comb_statements.extend(stmts);
                }
                let comb_statements = if comb_statements.len() > 1 {
                    vec![ProtoStatement::SequentialBlock(comb_statements)]
                } else {
                    comb_statements
                };
                Ok(ProtoDeclaration {
                    event_statements: HashMap::default(),
                    comb_statements,
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    full_internal_comb: None,
                })
            }
            air::Declaration::Ff(x) => {
                let mut statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    statements.extend(stmts);
                }

                let clock_event = Event::Clock(x.clock.id);
                let mut event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();

                if let Some(reset) = &x.reset {
                    let reset_event = Event::Reset(reset.id);
                    let head = statements.remove(0);
                    let (true_side, false_side) = head.split_if_reset().unwrap();
                    event_statements.insert(reset_event, true_side);
                    event_statements.insert(clock_event, false_side);
                } else {
                    event_statements.insert(clock_event, statements);
                }

                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    full_internal_comb: None,
                })
            }
            air::Declaration::Inst(x) => Conv::conv(context, x.as_ref()),
            air::Declaration::Initial(x) => {
                context.in_initial = true;
                let mut initial_statements = vec![];
                let mut conv_err = None;
                for stmt in &x.statements {
                    match Conv::conv(context, stmt) {
                        Ok(stmts) => {
                            let stmts: Vec<ProtoStatement> = stmts;
                            initial_statements.extend(stmts);
                        }
                        Err(e) => {
                            conv_err = Some(e);
                            break;
                        }
                    }
                }
                context.in_initial = false;
                if let Some(e) = conv_err {
                    return Err(e);
                }
                let mut event_statements = HashMap::default();
                event_statements.insert(Event::Initial, initial_statements);
                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    full_internal_comb: None,
                })
            }
            air::Declaration::Final(x) => {
                let mut final_statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    final_statements.extend(stmts);
                }
                let mut event_statements = HashMap::default();
                event_statements.insert(Event::Final, final_statements);
                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    full_internal_comb: None,
                })
            }
            air::Declaration::Unsupported(token) => {
                Err(SimulatorError::unsupported_description(token))
            }
            air::Declaration::Null => Ok(ProtoDeclaration {
                event_statements: HashMap::default(),
                comb_statements: vec![],
                post_comb_fns: vec![],
                child_modules: vec![],
                full_internal_comb: None,
            }),
        }
    }
}

impl Conv<&air::InstDeclaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::InstDeclaration) -> Result<Self, SimulatorError> {
        let air::Component::Module(child_module) = &src.component else {
            panic!("InstDeclaration for non-Module component");
        };

        let mut child_analyzer_context = veryl_analyzer::conv::Context::default();
        child_analyzer_context.variables = child_module.variables.clone();
        child_analyzer_context.functions = child_module.functions.clone();
        let mut child_ff_table = air::FfTable::default();
        child_module.gather_ff(&mut child_analyzer_context, &mut child_ff_table);
        child_ff_table.update_is_ff();
        if context.config.disable_ff_opt {
            child_ff_table.force_all_ff();
        }

        let ff_start = context.ff_total_bytes as isize;
        let comb_start = context.comb_total_bytes as isize;
        let (child_variable_meta, child_ff_count, child_comb_count) = create_variable_meta(
            &child_module.variables,
            &child_ff_table,
            context.config.use_4state,
            ff_start,
            comb_start,
        )
        .unwrap();

        context.ff_total_bytes += child_ff_count;
        context.comb_total_bytes += child_comb_count;

        let child_scope = ScopeContext {
            variable_meta: child_variable_meta.clone(),
            analyzer_context: child_analyzer_context,
        };
        context.scope_contexts.push(child_scope);

        let mut all_event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        let mut all_comb_statements: Vec<ProtoStatement> = vec![];
        let mut all_post_comb_fns: Vec<ProtoStatement> = vec![];
        let mut all_child_modules: Vec<ModuleVariableMeta> = vec![];
        // Track comb offsets written by Inst declarations (child modules).
        // Own Comb declarations (assign) that write to offsets NOT in this set
        // are "new" assigns (e.g., `assign o_port = var`) that need special
        // handling when merged JIT clears all_comb_statements.
        let mut inst_written_offsets: HashSet<isize> = HashSet::default();
        let mut own_new_assigns: Vec<ProtoStatement> = vec![];

        for decl in &child_module.declarations {
            let proto_decl: ProtoDeclaration = Conv::conv(context, decl)?;

            for (event, mut stmts) in proto_decl.event_statements {
                all_event_statements
                    .entry(event)
                    .and_modify(|v| v.append(&mut stmts))
                    .or_insert(stmts);
            }
            // Track which offsets Inst declarations write to
            if matches!(decl, air::Declaration::Inst(_)) {
                for s in &proto_decl.comb_statements {
                    let mut outs = vec![];
                    let mut ins = vec![];
                    s.gather_variable_offsets(&mut ins, &mut outs);
                    for off in outs {
                        inst_written_offsets.insert(off.raw());
                    }
                }
            }
            // Save own Comb assign statements that write to offsets NOT
            // already written by any child Inst (port connection).
            if matches!(decl, air::Declaration::Comb(_)) {
                fn collect_assigns(
                    s: &ProtoStatement,
                    inst_written_offsets: &HashSet<isize>,
                    out: &mut Vec<ProtoStatement>,
                ) {
                    match s {
                        ProtoStatement::Assign(a)
                            if !a.dst.is_ff() && !inst_written_offsets.contains(&a.dst.raw()) =>
                        {
                            out.push(s.clone());
                        }
                        ProtoStatement::SequentialBlock(body) => {
                            for s in body {
                                collect_assigns(s, inst_written_offsets, out);
                            }
                        }
                        _ => {}
                    }
                }
                for s in &proto_decl.comb_statements {
                    collect_assigns(s, &inst_written_offsets, &mut own_new_assigns);
                }
            }
            all_comb_statements.append(&mut proto_decl.comb_statements.clone());
            all_post_comb_fns.extend(proto_decl.post_comb_fns);
            all_child_modules.extend(proto_decl.child_modules);
        }

        context.scope_contexts.pop();

        // JIT cache: reuse compiled code across instances of the same module type.
        // ff_start and comb_start are already byte offsets.
        #[allow(unused_mut)]
        let mut full_internal_comb: Option<Vec<ProtoStatement>> = None;
        #[cfg(not(target_family = "wasm"))]
        if context.config.use_jit {
            let ff_start_bytes = ff_start;
            let comb_start_bytes = comb_start;
            let module_name = child_module.name;

            if let Some(cache_entry) = context.jit_cache.get(&module_name) {
                // Cache hit: replace internal logic with CompiledBlocks using delta
                let ff_delta = ff_start_bytes - cache_entry.ref_ff_start_bytes;
                let comb_delta = comb_start_bytes - cache_entry.ref_comb_start_bytes;

                let adjust = |offsets: &[VarOffset]| -> Vec<VarOffset> {
                    offsets
                        .iter()
                        .map(|off| off.adjust(ff_delta, comb_delta))
                        .collect()
                };

                let adjust_stmts = |stmts: &[ProtoStatement]| -> Vec<ProtoStatement> {
                    let mut adjusted = stmts.to_vec();
                    for s in &mut adjusted {
                        s.adjust_offsets(ff_delta, comb_delta);
                    }
                    adjusted
                };

                for (event, stmts) in all_event_statements.iter_mut() {
                    // Prefer merged function (comb+event combined) over event-only
                    let cached = cache_entry
                        .merged_funcs
                        .get(event)
                        .or_else(|| cache_entry.event_funcs.get(event));
                    if let Some(cached) = cached {
                        let adjusted_canonical: Vec<isize> = cached
                            .ff_canonical_offsets
                            .iter()
                            .map(|off| off + ff_delta)
                            .collect();
                        *stmts = vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                            func: cached.func,
                            ff_delta_bytes: ff_delta,
                            comb_delta_bytes: comb_delta,
                            input_offsets: adjust(&cached.input_offsets),
                            output_offsets: adjust(&cached.output_offsets),
                            ff_canonical_offsets: adjusted_canonical,
                            stmt_deps: vec![],
                            original_stmts: adjust_stmts(&cached.original_stmts),
                        })];
                    }
                }

                full_internal_comb = if !cache_entry.merged_funcs.is_empty() {
                    let full = std::mem::take(&mut all_comb_statements);
                    Some(full)
                } else {
                    None
                };

                if !cache_entry.merged_funcs.is_empty() {
                    // Re-add own assign statements whose dst is NOT
                    // already handled by the merged JIT (full_internal_comb).
                    if let Some(ref full) = full_internal_comb {
                        let mut full_outputs = HashSet::default();
                        for s in full {
                            let mut outs = vec![];
                            let mut ins = vec![];
                            s.gather_variable_offsets(&mut ins, &mut outs);
                            for off in outs {
                                full_outputs.insert(off.raw());
                            }
                        }
                        for s in &own_new_assigns {
                            if let ProtoStatement::Assign(a) = s
                                && !full_outputs.contains(&a.dst.raw())
                            {
                                all_post_comb_fns.push(s.clone());
                            }
                        }
                    }
                    // Internal comb already cleared above.
                    // Add comb-only JIT function to post_comb_fns so child comb
                    // is evaluated before events fire (without going through
                    // analyze_dependency on the parent level).
                    if let Some(cached) = &cache_entry.comb_func {
                        let adjusted_deps: Vec<_> = cached
                            .stmt_deps
                            .iter()
                            .map(|(ins, outs)| (adjust(ins), adjust(outs)))
                            .collect();
                        all_post_comb_fns.push(ProtoStatement::CompiledBlock(
                            CompiledBlockStatement {
                                func: cached.func,
                                ff_delta_bytes: ff_delta,
                                comb_delta_bytes: comb_delta,
                                input_offsets: adjust(&cached.input_offsets),
                                output_offsets: adjust(&cached.output_offsets),
                                ff_canonical_offsets: vec![],
                                stmt_deps: adjusted_deps,
                                original_stmts: adjust_stmts(&cached.original_stmts),
                            },
                        ));
                    } else if let Some(ref full) = full_internal_comb {
                        // comb_func was None (not all statements JIT-compilable).
                        // Re-add interpreted comb statements to post_comb_fns.
                        all_post_comb_fns.extend(full.iter().cloned());
                    }
                } else if let Some(cached) = &cache_entry.comb_func {
                    let adjusted_deps: Vec<_> = cached
                        .stmt_deps
                        .iter()
                        .map(|(ins, outs)| (adjust(ins), adjust(outs)))
                        .collect();
                    all_comb_statements =
                        vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                            func: cached.func,
                            ff_delta_bytes: ff_delta,
                            comb_delta_bytes: comb_delta,
                            input_offsets: adjust(&cached.input_offsets),
                            output_offsets: adjust(&cached.output_offsets),
                            ff_canonical_offsets: vec![],
                            stmt_deps: adjusted_deps,
                            original_stmts: adjust_stmts(&cached.original_stmts),
                        })];
                }
            } else {
                // Cache miss: save originals before individual compilation
                let original_comb = all_comb_statements.clone();
                let original_events: HashMap<Event, Vec<ProtoStatement>> =
                    all_event_statements.clone();

                // Compile events individually
                let mut event_funcs = HashMap::default();
                for (event, stmts) in all_event_statements.iter_mut() {
                    if stmts.iter().all(|s| s.can_build_binary())
                        && !stmts.is_empty()
                        && let Some(func) = cranelift::build_binary(context, stmts.clone())
                    {
                        // Event blocks use NBA semantics, so a variable
                        // that is both read and written is not purely
                        // internal; keep all inputs so sort_ff_event sees
                        // the dependency.
                        let mut all_inputs = vec![];
                        let mut all_outputs = vec![];
                        for s in stmts.iter() {
                            s.gather_variable_offsets(&mut all_inputs, &mut all_outputs);
                        }
                        all_inputs.dedup();
                        all_outputs.dedup();
                        let (input_offsets, output_offsets) = (all_inputs, all_outputs);
                        let ff_canonical = gather_ff_canonical(stmts);

                        let event_original = stmts.clone();
                        event_funcs.insert(
                            event.clone(),
                            JitCachedFunc {
                                func,
                                input_offsets: input_offsets.clone(),
                                output_offsets: output_offsets.clone(),
                                ff_canonical_offsets: ff_canonical.clone(),
                                stmt_deps: vec![],
                                original_stmts: event_original.clone(),
                            },
                        );

                        *stmts = vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                            func,
                            ff_delta_bytes: 0,
                            comb_delta_bytes: 0,
                            input_offsets,
                            output_offsets,
                            ff_canonical_offsets: ff_canonical,
                            stmt_deps: vec![],
                            original_stmts: event_original,
                        })];
                    }
                }

                // Compile comb individually
                let all_can_build = all_comb_statements.iter().all(|s| s.can_build_binary());
                let comb_func = if all_can_build && !all_comb_statements.is_empty() {
                    // Sort statements topologically (RAW dependencies) so that
                    // output port connections run before assigns that read them.
                    let sorted_comb_for_func = stable_topo_sort(all_comb_statements.clone());

                    if let Some(func) =
                        cranelift::build_binary(context, sorted_comb_for_func.clone())
                    {
                        let (input_offsets, output_offsets) =
                            gather_external_offsets(&sorted_comb_for_func);

                        let stmt_deps: Vec<_> = sorted_comb_for_func
                            .iter()
                            .map(|s| {
                                let mut ins = vec![];
                                let mut outs = vec![];
                                s.gather_variable_offsets(&mut ins, &mut outs);
                                (ins, outs)
                            })
                            .collect();

                        let original_stmts = sorted_comb_for_func.clone();
                        all_comb_statements =
                            vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                                func,
                                ff_delta_bytes: 0,
                                comb_delta_bytes: 0,
                                input_offsets: input_offsets.clone(),
                                output_offsets: output_offsets.clone(),
                                ff_canonical_offsets: vec![],
                                stmt_deps: stmt_deps.clone(),
                                original_stmts,
                            })];

                        Some(JitCachedFunc {
                            func,
                            input_offsets,
                            output_offsets,
                            ff_canonical_offsets: vec![],
                            stmt_deps,
                            original_stmts: sorted_comb_for_func,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Compile merged comb+event functions using saved originals.
                // The merged function computes comb then event in one JIT call,
                // allowing load_cache to forward comb stores to event loads.
                let comb_jittable =
                    !original_comb.is_empty() && original_comb.iter().all(|s| s.can_build_binary());
                let mut merged_funcs = HashMap::default();

                if comb_jittable {
                    // Sort comb for merged function inlining (deterministic order).
                    let sorted_comb = Some(stable_topo_sort(original_comb.clone()));

                    // Compute external reads: output port comb offsets that are
                    // read by port connections after the merged function returns
                    let mut external_reads = HashSet::default();
                    for output in &src.outputs {
                        for child_var_id in &output.id {
                            if let Some(child_meta) = child_variable_meta.get(child_var_id) {
                                let element = &child_meta.elements[0];
                                if !element.is_ff() {
                                    external_reads.insert(element.current_offset());
                                }
                            }
                        }
                    }

                    for (event, orig_stmts) in &original_events {
                        if orig_stmts.is_empty() || !orig_stmts.iter().all(|s| s.can_build_binary())
                        {
                            continue;
                        }

                        // Inline single-use comb variables into event statements
                        let (opt_comb, opt_events) = if let Some(sorted) = &sorted_comb {
                            super::optimize::optimize_merged(
                                sorted.clone(),
                                orig_stmts.clone(),
                                &external_reads,
                            )
                        } else {
                            (original_comb.clone(), orig_stmts.clone())
                        };

                        // Check that optimized statements are still jittable
                        let all_jittable = opt_comb
                            .iter()
                            .chain(opt_events.iter())
                            .all(|s| s.can_build_binary());
                        if !all_jittable {
                            continue;
                        }

                        // Collect comb offsets read by embedded CompiledBlocks
                        // (child module merged functions in the event part).
                        // These must NOT be store-eliminated because the
                        // CompiledBlock reads from memory, not load_cache.
                        let mut event_reads = HashSet::default();
                        for s in &opt_events {
                            let mut ins = vec![];
                            let mut outs = vec![];
                            s.gather_variable_offsets(&mut ins, &mut outs);
                            for off in ins {
                                event_reads.insert(off.raw());
                            }
                        }

                        // Compute store elimination set: internal comb offsets
                        // that are not externally read (port connections, etc.)
                        // and not read by embedded CompiledBlocks.
                        let mut store_elim = HashSet::default();
                        for s in &opt_comb {
                            if let ProtoStatement::Assign(a) = s
                                && !a.dst.is_ff()
                                && a.select.is_none()
                                && !external_reads.contains(&a.dst.raw())
                                && !event_reads.contains(&a.dst.raw())
                            {
                                store_elim.insert(a.dst);
                            }
                        }

                        let mut merged = opt_comb;
                        merged.extend(opt_events);

                        if let Some(func) = cranelift::build_binary_with_store_elim_and_no_cache(
                            context,
                            merged.clone(),
                            store_elim,
                        ) {
                            let (input_offsets, output_offsets) = gather_external_offsets(&merged);
                            let ff_canonical = gather_ff_canonical(&merged);

                            // Replace event_statements with merged CompiledBlock
                            all_event_statements.insert(
                                event.clone(),
                                vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                                    func,
                                    ff_delta_bytes: 0,
                                    comb_delta_bytes: 0,
                                    input_offsets: input_offsets.clone(),
                                    output_offsets: output_offsets.clone(),
                                    ff_canonical_offsets: ff_canonical.clone(),
                                    stmt_deps: vec![],
                                    original_stmts: merged.clone(),
                                })],
                            );

                            merged_funcs.insert(
                                event.clone(),
                                JitCachedFunc {
                                    func,
                                    input_offsets,
                                    output_offsets,
                                    ff_canonical_offsets: ff_canonical,
                                    stmt_deps: vec![],
                                    original_stmts: merged,
                                },
                            );
                        }
                    }
                }

                // If any merged functions were compiled, save full internal comb
                // and clear it from comb_statements. Port connections are added
                // after this block so they remain in comb_statements.
                // The full comb is needed by get()/dump() for correctness.
                full_internal_comb = if !merged_funcs.is_empty() {
                    let full = std::mem::take(&mut all_comb_statements);
                    // Re-add own assign statements whose dst is NOT
                    // already in the full internal comb (merged JIT handles those).
                    {
                        let mut full_outputs = HashSet::default();
                        for s in &full {
                            let mut outs = vec![];
                            let mut ins = vec![];
                            s.gather_variable_offsets(&mut ins, &mut outs);
                            for off in outs {
                                full_outputs.insert(off.raw());
                            }
                        }
                        for s in &own_new_assigns {
                            if let ProtoStatement::Assign(a) = s
                                && !full_outputs.contains(&a.dst.raw())
                            {
                                all_post_comb_fns.push(s.clone());
                            }
                        }
                    }
                    // When merged comb+event is used, add the comb-only function
                    // to post_comb_fns so child comb is evaluated before events fire.
                    if let Some(ref cf) = comb_func {
                        // Preserve original_stmts so the parent module can
                        // expand this CB into individual statements for
                        // fine-grained dependency analysis in the unified comb list.
                        let original_stmts = if full.len() == 1 {
                            if let ProtoStatement::CompiledBlock(cb) = &full[0] {
                                cb.original_stmts.clone()
                            } else {
                                full.clone()
                            }
                        } else {
                            full.clone()
                        };
                        all_post_comb_fns.push(ProtoStatement::CompiledBlock(
                            CompiledBlockStatement {
                                func: cf.func,
                                ff_delta_bytes: 0,
                                comb_delta_bytes: 0,
                                input_offsets: cf.input_offsets.clone(),
                                output_offsets: cf.output_offsets.clone(),
                                ff_canonical_offsets: vec![],
                                stmt_deps: cf.stmt_deps.clone(),
                                original_stmts,
                            },
                        ));
                    } else {
                        // comb_func is None (some statements can't be JIT-compiled).
                        // Add all comb statements as interpreted to post_comb_fns
                        // so they still execute after merged event JIT functions.
                        all_post_comb_fns.extend(full.iter().cloned());
                    }
                    Some(full)
                } else {
                    None
                };

                context.jit_cache.insert(
                    module_name,
                    JitCacheEntry {
                        ref_ff_start_bytes: ff_start_bytes,
                        ref_comb_start_bytes: comb_start_bytes,
                        event_funcs,
                        comb_func,
                        merged_funcs,
                    },
                );
            }
        }

        // When child modules have merged JIT (post_comb_fns non-empty),
        // parent-level own assigns need to also run after events so that
        // multi-hop propagation (child output → var → parent output) works.
        // Without this, the intermediate assign (var → parent output) only
        // runs in eval_comb (before events) and misses the new values.
        if !all_post_comb_fns.is_empty() && full_internal_comb.is_none() {
            let mut post_comb_written: HashSet<isize> = HashSet::default();
            // Track output offsets already in all_post_comb_fns to avoid duplicates.
            let mut already_added: HashSet<isize> = HashSet::default();
            for s in &all_post_comb_fns {
                let mut outs = vec![];
                let mut ins = vec![];
                s.gather_variable_offsets(&mut ins, &mut outs);
                for off in &outs {
                    post_comb_written.insert(off.raw());
                    already_added.insert(off.raw());
                }
            }
            // Add own assigns that READ from post_comb-written offsets
            for s in &own_new_assigns {
                if let ProtoStatement::Assign(a) = s {
                    if already_added.contains(&a.dst.raw()) {
                        continue;
                    }
                    let mut ins = vec![];
                    let mut outs = vec![];
                    s.gather_variable_offsets(&mut ins, &mut outs);
                    let reads_post_comb =
                        ins.iter().any(|off| post_comb_written.contains(&off.raw()));
                    if reads_post_comb {
                        for off in &outs {
                            already_added.insert(off.raw());
                        }
                        all_post_comb_fns.push(s.clone());
                    }
                }
            }
            // Add comb statements that read from post_comb-written offsets.
            for s in &all_comb_statements {
                let mut ins = vec![];
                let mut outs = vec![];
                s.gather_variable_offsets(&mut ins, &mut outs);
                if outs.iter().all(|off| already_added.contains(&off.raw())) {
                    continue;
                }
                let reads_post_comb = ins.iter().any(|off| post_comb_written.contains(&off.raw()));
                if reads_post_comb {
                    for off in &outs {
                        already_added.insert(off.raw());
                    }
                    if let ProtoStatement::CompiledBlock(cb) = s
                        && !cb.original_stmts.is_empty()
                    {
                        all_post_comb_fns.extend(cb.original_stmts.iter().cloned());
                        continue;
                    }
                    all_post_comb_fns.push(s.clone());
                }
            }
        }

        // Input ports: parent expr → child port var
        for input in &src.inputs {
            for child_var_id in &input.id {
                let child_meta = child_variable_meta.get(child_var_id).unwrap();

                // Array port with a simple variable expression: expand per-element
                if child_meta.elements.len() > 1
                    && let air::Expression::Term(factor) = &input.expr
                    && let air::Factor::Variable(parent_id, index, select, _) = factor.as_ref()
                    && index.0.is_empty()
                    && select.is_empty()
                {
                    let parent_scope = context.scope();
                    let parent_meta = parent_scope.variable_meta.get(parent_id).unwrap();
                    for i in 0..child_meta.elements.len() {
                        let child_element = &child_meta.elements[i];
                        let parent_element = &parent_meta.elements[i];
                        let parent_expr = ProtoExpression::Variable {
                            var_offset: parent_element.current,
                            select: None,
                            dynamic_select: None,
                            width: child_meta.width,
                            expr_context: ExpressionContext {
                                width: child_meta.width,
                                signed: false,
                            },
                        };
                        all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                            dst: child_element.current,
                            dst_width: child_meta.width,
                            select: None,
                            dynamic_select: None,
                            rhs_select: None,
                            expr: parent_expr,
                            dst_ff_current_offset: 0, // not FF
                            token: TokenRange::default(),
                        }));
                    }
                    continue;
                }

                let proto_expr: ProtoExpression = Conv::conv(context, &input.expr)?;
                let element = &child_meta.elements[0];
                all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst: element.current,
                    dst_width: child_meta.width,
                    select: None,
                    dynamic_select: None,
                    rhs_select: None,
                    expr: proto_expr.clone(),
                    dst_ff_current_offset: 0, // not FF
                    token: TokenRange::default(),
                }));
            }
        }

        // Output ports: child port var → parent dst
        // When merged functions exist, also add output port connections to
        // post_comb_fns so that child comb values (computed by post_comb)
        // propagate to parent variables before events fire.
        let needs_post_comb_propagation =
            full_internal_comb.is_some() || !all_post_comb_fns.is_empty();
        for output in &src.outputs {
            for (child_var_id, parent_dst) in output.id.iter().zip(output.dst.iter()) {
                let child_meta = child_variable_meta.get(child_var_id).unwrap();

                let (
                    parent_index,
                    parent_select,
                    parent_width,
                    parent_need_dynamic,
                    parent_width_shape,
                    parent_kind_width,
                ) = {
                    let parent_scope = context.scope();
                    let parent_meta = parent_scope.variable_meta.get(&parent_dst.id).unwrap();
                    let parent_index = parent_dst
                        .index
                        .eval_value(&mut parent_scope.analyzer_context)
                        .unwrap();

                    let select = if !parent_dst.select.is_empty() {
                        parent_dst.select.eval_value(
                            &mut parent_scope.analyzer_context,
                            &parent_dst.comptime.r#type,
                            false,
                        )
                    } else {
                        None
                    };
                    let need_dynamic =
                        !parent_dst.select.is_empty() && !parent_dst.select.is_const();
                    let select = if need_dynamic { None } else { select };
                    let width = parent_meta.width;
                    let width_shape = parent_meta.r#type.width().clone();
                    let kind_width = parent_meta.r#type.kind.width().unwrap_or(1);
                    (
                        parent_index,
                        select,
                        width,
                        need_dynamic,
                        width_shape,
                        kind_width,
                    )
                };

                let parent_dynamic_select = if parent_need_dynamic {
                    Some(build_dynamic_bit_select(
                        context,
                        &parent_width_shape,
                        &parent_dst.select,
                        parent_kind_width,
                    )?)
                } else {
                    None
                };

                let parent_scope = context.scope();
                let parent_meta = parent_scope.variable_meta.get(&parent_dst.id).unwrap();

                // Determine which parent elements to connect.
                // When the parent destination has no index and the variable is an
                // array, connect each element individually (array-to-array port).
                let parent_element_indices: Vec<usize> = if let Some(idx) =
                    parent_meta.r#type.array.calc_index(&parent_index)
                {
                    vec![idx]
                } else if parent_index.is_empty() && !parent_meta.r#type.array.is_empty() {
                    (0..parent_meta.elements.len()).collect()
                } else {
                    panic!(
                        "calc_index failed for output port destination (index {:?}, array {:?})",
                        parent_index, parent_meta.r#type.array,
                    );
                };

                for (elem_idx, &parent_elem_idx) in parent_element_indices.iter().enumerate() {
                    let child_element = &child_meta.elements[elem_idx];
                    let parent_element = &parent_meta.elements[parent_elem_idx];

                    let child_expr = ProtoExpression::Variable {
                        var_offset: child_element.current,
                        select: None,
                        dynamic_select: None,
                        width: child_meta.width,
                        expr_context: ExpressionContext {
                            width: child_meta.width,
                            signed: false,
                        },
                    };

                    let dst_var = if parent_element.is_ff() {
                        VarOffset::Ff(parent_element.next_offset)
                    } else {
                        VarOffset::Comb(parent_element.current_offset())
                    };

                    let stmt = ProtoStatement::Assign(ProtoAssignStatement {
                        dst: dst_var,
                        dst_width: parent_width,
                        select: parent_select,
                        dynamic_select: parent_dynamic_select.clone(),
                        rhs_select: None,
                        expr: child_expr,
                        dst_ff_current_offset: parent_element.current_offset(),
                        token: TokenRange::default(),
                    });

                    all_comb_statements.push(stmt.clone());

                    // When this module has merged functions, also add comb
                    // output port connections to post_comb_fns so that child
                    // comb values propagate to parent before events fire.
                    if needs_post_comb_propagation && !dst_var.is_ff() {
                        all_post_comb_fns.push(stmt);
                    }
                }
            }
        }

        // Remap child event keys (clock/reset) to parent VarIds via input port connections
        let mut child_to_parent_var: HashMap<air::VarId, air::VarId> = HashMap::default();
        for input in &src.inputs {
            if let air::Expression::Term(factor) = &input.expr
                && let air::Factor::Variable(parent_var_id, _, _, _) = factor.as_ref()
            {
                for child_var_id in &input.id {
                    child_to_parent_var.insert(*child_var_id, *parent_var_id);
                }
            }
        }

        let mut remapped_events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        for (event, stmts) in all_event_statements {
            let new_event = match &event {
                Event::Clock(child_id) => {
                    if let Some(parent_id) = child_to_parent_var.get(child_id) {
                        Event::Clock(*parent_id)
                    } else {
                        event.clone()
                    }
                }
                Event::Reset(child_id) => {
                    if let Some(parent_id) = child_to_parent_var.get(child_id) {
                        Event::Reset(*parent_id)
                    } else {
                        event.clone()
                    }
                }
                _ => event.clone(),
            };
            remapped_events
                .entry(new_event)
                .and_modify(|v| v.extend(stmts.clone()))
                .or_insert(stmts);
        }

        let child_module_meta = ModuleVariableMeta {
            name: src.name,
            variable_meta: child_variable_meta,
            children: all_child_modules,
        };

        Ok(ProtoDeclaration {
            event_statements: remapped_events,
            comb_statements: all_comb_statements,
            post_comb_fns: all_post_comb_fns,
            child_modules: vec![child_module_meta],
            full_internal_comb,
        })
    }
}
