//! Per-instance JIT chunk compilation for child module declarations.
//!
//! Compiles the child's comb / event statements via the registry's
//! chunk path and replaces them with `CompiledBlock` statements that
//! reference the artifact.  Subsequent instances of the same component
//! reuse the same compiled function with adjusted byte deltas
//! (`Context::chunk_cache`).

use crate::backend::CompileCtx;
use crate::ir::context::{CachedChunk, ChunkCacheEntry, Context};
use crate::ir::declaration::stable_topo_sort;
use crate::ir::variable::VarOffset;
use crate::ir::{CompiledBlockStatement, Event, ProtoStatement};
use crate::{HashMap, HashSet};
use std::sync::Arc;
use veryl_analyzer::ir as air;

type VarOffsets = Vec<VarOffset>;

fn gather_ff_canonical(stmts: &[ProtoStatement]) -> Vec<isize> {
    let mut result = HashSet::default();
    for s in stmts {
        result.extend(s.gather_ff_canonical_offsets());
    }
    result.into_iter().collect()
}

/// Filter out internal variables (read+written) from inputs to avoid
/// false dependency cycles in `analyze_dependency`.
fn gather_external_offsets(stmts: &[ProtoStatement]) -> (VarOffsets, VarOffsets) {
    let mut all_inputs = vec![];
    let mut all_outputs = vec![];
    for s in stmts {
        s.gather_variable_offsets(&mut all_inputs, &mut all_outputs);
    }

    let input_set: HashSet<VarOffset> = all_inputs.iter().cloned().collect();
    let output_set: HashSet<VarOffset> = all_outputs.iter().cloned().collect();
    // Outputs are kept so dependent blocks see the dependency edge.
    let internal: HashSet<VarOffset> = input_set.intersection(&output_set).cloned().collect();
    all_inputs.retain(|x| !internal.contains(x));
    all_inputs.dedup();
    all_outputs.dedup();

    (all_inputs, all_outputs)
}

/// Reuse a cached compiled chunk or compile fresh via the registry,
/// rewriting jittable groups in `all_*_statements` to single
/// `ProtoStatement::CompiledBlock`s.  No-op when JIT is disabled.
///
/// Pre-JIT originals are preserved in `CompiledBlock::original_stmts`
/// for `analyze_dependency` Phase 2 expansion.  Avoiding a parallel
/// copy outside the CB keeps the parent's `unified` list free of
/// false 2-stmt SCCs.
pub fn try_compile_inst_chunks(
    context: &mut Context,
    src: &air::InstDeclaration,
    ff_start: isize,
    comb_start: isize,
    alias_enabled: bool,
    all_event_statements: &mut HashMap<Event, Vec<ProtoStatement>>,
    all_comb_statements: &mut Vec<ProtoStatement>,
) {
    if !context.config.use_jit {
        return;
    }
    let ff_start_bytes = ff_start;
    let comb_start_bytes = comb_start;
    let component_key: *const air::Component = Arc::as_ptr(&src.component);

    // Input-port aliasing bakes parent-specific offsets into the
    // compiled chunk, so the cache (keyed only by child component)
    // cannot be shared across instances.
    let cache_lookup = if alias_enabled {
        None
    } else {
        context.chunk_cache.get(&component_key)
    };
    if let Some(cache_entry) = cache_lookup {
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
            if let Some(cached) = cache_entry.event_funcs.get(event) {
                let adjusted_canonical: Vec<isize> = cached
                    .ff_canonical_offsets
                    .iter()
                    .map(|off| off + ff_delta)
                    .collect();
                *stmts = vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                    artifact: Arc::clone(&cached.artifact),
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

        if let Some(cached) = &cache_entry.comb_func {
            let adjusted_deps: Vec<_> = cached
                .stmt_deps
                .iter()
                .map(|(ins, outs)| (adjust(ins), adjust(outs)))
                .collect();
            *all_comb_statements = vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                artifact: Arc::clone(&cached.artifact),
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
        let mut event_funcs = HashMap::default();
        for (event, stmts) in all_event_statements.iter_mut() {
            if stmts.is_empty() || !stmts.iter().all(|s| context.backends.any_supports_stmt(s)) {
                continue;
            }
            let artifact = {
                let ctx = CompileCtx {
                    config: &context.config,
                    use_4state: context.config.use_4state,
                    contains_compiled_block: false,
                };
                context.backends.try_compile_chunk(&ctx, stmts.as_slice())
            };
            if let Some(artifact) = artifact {
                // NBA semantics: a read+written variable is not purely
                // internal; keep all inputs so sort_ff_event sees the
                // dependency.
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
                    CachedChunk {
                        artifact: Arc::clone(&artifact),
                        input_offsets: input_offsets.clone(),
                        output_offsets: output_offsets.clone(),
                        ff_canonical_offsets: ff_canonical.clone(),
                        stmt_deps: vec![],
                        original_stmts: event_original.clone(),
                    },
                );

                *stmts = vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                    artifact,
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

        let all_can_build = all_comb_statements
            .iter()
            .all(|s| context.backends.any_supports_stmt(s));
        let comb_func = if all_can_build && !all_comb_statements.is_empty() {
            // Topo-sort by RAW dependencies so output-port connections
            // run before assigns that read them.
            let sorted_comb_for_func = stable_topo_sort(all_comb_statements.clone());

            let artifact = {
                let ctx = CompileCtx {
                    config: &context.config,
                    use_4state: context.config.use_4state,
                    contains_compiled_block: false,
                };
                context
                    .backends
                    .try_compile_chunk(&ctx, sorted_comb_for_func.as_slice())
            };
            if let Some(artifact) = artifact {
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
                *all_comb_statements =
                    vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                        artifact: Arc::clone(&artifact),
                        ff_delta_bytes: 0,
                        comb_delta_bytes: 0,
                        input_offsets: input_offsets.clone(),
                        output_offsets: output_offsets.clone(),
                        ff_canonical_offsets: vec![],
                        stmt_deps: stmt_deps.clone(),
                        original_stmts,
                    })];

                Some(CachedChunk {
                    artifact,
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

        if !alias_enabled {
            context.chunk_cache.insert(
                component_key,
                ChunkCacheEntry {
                    ref_ff_start_bytes: ff_start_bytes,
                    ref_comb_start_bytes: comb_start_bytes,
                    event_funcs,
                    comb_func,
                },
            );
        }
    }
}
