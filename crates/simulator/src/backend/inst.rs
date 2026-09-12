//! Per-instance JIT chunk compilation for child module declarations, and
//! the cross-test DUT reuse built on top of it.
//!
//! `try_compile_inst_chunks` compiles a child's comb / event statements via
//! the registry's chunk path and replaces them with `CompiledBlock`s that
//! reference the artifact; instances within one build reuse the compiled
//! function with adjusted byte deltas (`Context::chunk_cache`).
//!
//! Cross-test reuse goes a level up: `DutReuseCache` caches a whole
//! converted subtree (single-flight) and relocates it into later tests by a
//! single `(ff_delta, comb_delta)`, skipping IR assembly and codegen.
//! `port_alias_enabled` picks which boundary becomes the reuse DUT — the
//! topmost component recurring across tests — and de-aliases only that one,
//! so its internals relocate uniformly with it.

use crate::backend::CompileCtx;
use crate::ir::context::{CachedChunk, ChunkCacheEntry, Context};
use crate::ir::declaration::stable_topo_sort;
use crate::ir::variable::{ModuleVariableMeta, VarOffset, VariableElement, VariableMeta};
use crate::ir::{CompiledBlockStatement, Event, ProtoStatement};
use crate::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::{Condvar, LazyLock, Mutex};
use veryl_analyzer::ir as air;

/// Size floor (ff + comb bytes) below which a recurring component is treated as
/// trivial infra ($tb clock/reset, interface shims) not worth a de-aliased
/// boundary.  Recurrence is the real selector.  `VERYL_DUT_REUSE_MIN_BYTES`.
fn dut_reuse_min_bytes() -> usize {
    static V: LazyLock<usize> = LazyLock::new(|| {
        std::env::var("VERYL_DUT_REUSE_MIN_BYTES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(256)
    });
    *V
}

/// Reuse state for one immutable analysis IR and one configuration, owned by
/// `BuildSession`. Its borrow of the IR keeps every component address live for
/// all builds using this cache. No pointer key escapes into a process-wide map.
/// The recurring set is fixed before workers start; replication within one top
/// never makes a component recurring.
pub(crate) struct DutReuseCache {
    recurring: HashSet<usize>,
    disable_port_alias: bool,
    statements: Mutex<HashMap<usize, Slot>>,
    ready: Condvar,
}

impl Default for DutReuseCache {
    fn default() -> Self {
        Self {
            recurring: HashSet::default(),
            disable_port_alias: std::env::var("VERYL_DISABLE_PORT_ALIAS").as_deref() == Ok("1"),
            statements: Mutex::default(),
            ready: Condvar::new(),
        }
    }
}

impl DutReuseCache {
    pub(crate) fn new(ir: &air::Ir, tops: &[veryl_parser::resource_table::StrId]) -> Self {
        Self {
            recurring: compute_recurring_set(ir, tops),
            ..Self::default()
        }
    }

    /// De-alias only the topmost recurring DUT boundary. Its internals remain
    /// aliased so the cached subtree can relocate by a single byte delta.
    pub(crate) fn port_alias_enabled(
        &self,
        component_key: *const air::Component,
        own_ff_bytes: usize,
        own_comb_bytes: usize,
        in_reuse_dut: bool,
        dut_reuse: bool,
    ) -> bool {
        if self.disable_port_alias {
            return false;
        }
        let is_dut_boundary = dut_reuse
            && self.recurring.contains(&(component_key as usize))
            && !in_reuse_dut
            && (own_ff_bytes + own_comb_bytes) >= dut_reuse_min_bytes();
        !is_dut_boundary
    }
}

/// Deterministically compute which components are instantiated under two or more
/// of `tops` — the DUT(s) shared across testbenches. Early-terminating: once a
/// component is known to recur its whole subtree is marked once and later tops
/// skip it, so the (large, shared) DUT subtree is walked ~twice total rather than
/// once per top.
fn compute_recurring_set(
    ir: &air::Ir,
    tops: &[veryl_parser::resource_table::StrId],
) -> HashSet<usize> {
    let mut owner: HashMap<usize, veryl_parser::resource_table::StrId> = HashMap::default();
    let mut recurring: HashSet<usize> = HashSet::default();

    // Mark `m`'s whole subtree recurring.
    fn mark_subtree(m: &air::Module, recurring: &mut HashSet<usize>) {
        for decl in &m.declarations {
            if let air::Declaration::Inst(inst) = decl {
                let key = Arc::as_ptr(&inst.component) as usize;
                if recurring.insert(key)
                    && let air::Component::Module(child) = inst.component.as_ref()
                {
                    mark_subtree(child, recurring);
                }
            }
        }
    }

    fn walk(
        m: &air::Module,
        top: veryl_parser::resource_table::StrId,
        owner: &mut HashMap<usize, veryl_parser::resource_table::StrId>,
        recurring: &mut HashSet<usize>,
    ) {
        for decl in &m.declarations {
            if let air::Declaration::Inst(inst) = decl {
                let key = Arc::as_ptr(&inst.component) as usize;
                let air::Component::Module(child) = inst.component.as_ref() else {
                    continue;
                };
                if recurring.contains(&key) {
                    continue; // already recurring — subtree is too
                }
                match owner.get(&key) {
                    Some(&o) if o != top => {
                        // Seen under a different top: this component and its whole
                        // subtree recur.
                        recurring.insert(key);
                        mark_subtree(child, recurring);
                    }
                    Some(_) => {} // same top (SMP replication): not recurring, already walked
                    None => {
                        owner.insert(key, top);
                        walk(child, top, owner, recurring);
                    }
                }
            }
        }
    }

    for &top in tops {
        if let Some(air::Component::Module(m)) = ir
            .components
            .iter()
            .find(|c| matches!(c, air::Component::Module(m) if m.name == top))
        {
            walk(m, top, &mut owner, &mut recurring);
        }
    }
    recurring
}

// Caches a component's fully-converted subtree (statements, child_modules,
// derived-clock candidates) so later instances/tests skip IR assembly AND
// codegen, relocating it by a single `(ff_delta, comb_delta)` like `chunk_cache`.
// Keyed by component `Arc` pointer (stable within one `air::Ir`); the per-
// instance `child_variable_meta` and port copies are rebuilt fresh.
struct CachedStatements {
    ref_ff_start: isize,
    ref_comb_start: isize,
    /// Total ff/comb bytes the reference conv consumed (incl. function-local
    /// temps); the reuse path advances the running totals by these to reserve the
    /// region a full re-run would, so a sibling instance can't overlap it.
    ff_size: usize,
    comb_size: usize,
    event_statements: HashMap<Event, Vec<ProtoStatement>>,
    comb_statements: Vec<ProtoStatement>,
    post_comb_fns: Vec<ProtoStatement>,
    child_modules: Vec<ModuleVariableMeta>,
    derived_clock_candidates: Vec<crate::ir::EdgeCandidate>,
    /// `Context::comb_reloc` entries the reference conv recorded inside its
    /// region; a copy needs them relocated too or its temps stay unowned.
    comb_reloc: Vec<(isize, isize, usize)>,
}

/// Single-flight cache slot: one thread `Computing` a component blocks others
/// (waiting on `ready`) until it publishes `Done`, so parallel tests share the
/// first conv of a shared DUT instead of all converting it redundantly.
enum Slot {
    Computing,
    Done(Arc<CachedStatements>),
}

/// Relocated subtree internals returned to `InstDeclaration::conv` on a cache
/// hit.  Derived-clock event ids are still keyed by the reference conv's
/// (grandchild) internal ids; the caller re-keys them to fresh ids.
pub struct ReusedStatements {
    pub event_statements: HashMap<Event, Vec<ProtoStatement>>,
    pub comb_statements: Vec<ProtoStatement>,
    pub post_comb_fns: Vec<ProtoStatement>,
    pub child_modules: Vec<ModuleVariableMeta>,
    pub derived_clock_candidates: Vec<crate::ir::EdgeCandidate>,
    pub ff_size: usize,
    pub comb_size: usize,
    pub comb_reloc: Vec<(isize, isize, usize)>,
}

fn adjust_offsets_vec(offs: &[VarOffset], ff_delta: isize, comb_delta: isize) -> Vec<VarOffset> {
    offs.iter()
        .map(|o| o.adjust(ff_delta, comb_delta))
        .collect()
}

/// Relocate a single statement by byte deltas.  CompiledBlocks accumulate the
/// delta into their runtime base shift and have every baked offset adjusted;
/// interpreted statements use `adjust_offsets`.
fn reloc_stmt(s: &ProtoStatement, ff_delta: isize, comb_delta: isize) -> ProtoStatement {
    match s {
        ProtoStatement::CompiledBlock(cb) => {
            ProtoStatement::CompiledBlock(CompiledBlockStatement {
                artifact: Arc::clone(&cb.artifact),
                ff_delta_bytes: cb.ff_delta_bytes + ff_delta,
                comb_delta_bytes: cb.comb_delta_bytes + comb_delta,
                input_offsets: adjust_offsets_vec(&cb.input_offsets, ff_delta, comb_delta),
                output_offsets: adjust_offsets_vec(&cb.output_offsets, ff_delta, comb_delta),
                ff_canonical_offsets: cb
                    .ff_canonical_offsets
                    .iter()
                    .map(|o| o + ff_delta)
                    .collect(),
                stmt_deps: Arc::new(
                    cb.stmt_deps
                        .iter()
                        .map(|(ins, outs)| {
                            (
                                adjust_offsets_vec(ins, ff_delta, comb_delta),
                                adjust_offsets_vec(outs, ff_delta, comb_delta),
                            )
                        })
                        .collect(),
                ),
                original_stmts: Arc::new(reloc_stmts(&cb.original_stmts, ff_delta, comb_delta)),
            })
        }
        other => {
            let mut c = other.clone();
            c.adjust_offsets(ff_delta, comb_delta);
            c
        }
    }
}

fn reloc_stmts(
    stmts: &[ProtoStatement],
    ff_delta: isize,
    comb_delta: isize,
) -> Vec<ProtoStatement> {
    stmts
        .iter()
        .map(|s| reloc_stmt(s, ff_delta, comb_delta))
        .collect()
}

fn reloc_var_meta(m: &VariableMeta, ff_delta: isize, comb_delta: isize) -> VariableMeta {
    let mut nm = m.clone();
    nm.elements = m
        .elements
        .iter()
        .map(|e| VariableElement {
            native_bytes: e.native_bytes,
            current: e.current.adjust(ff_delta, comb_delta),
            next_offset: if e.current.is_ff() {
                e.next_offset + ff_delta
            } else {
                e.next_offset
            },
        })
        .collect();
    nm
}

fn reloc_module_meta(
    mm: &ModuleVariableMeta,
    ff_delta: isize,
    comb_delta: isize,
) -> ModuleVariableMeta {
    ModuleVariableMeta {
        name: mm.name,
        hierarchy: mm.hierarchy.clone(),
        variable_meta: mm
            .variable_meta
            .iter()
            .map(|(k, v)| (*k, reloc_var_meta(v, ff_delta, comb_delta)))
            .collect(),
        children: mm
            .children
            .iter()
            .map(|c| reloc_module_meta(c, ff_delta, comb_delta))
            .collect(),
    }
}

fn relocate_entry(
    entry: &CachedStatements,
    ff_start: isize,
    comb_start: isize,
) -> ReusedStatements {
    let ff_delta = ff_start - entry.ref_ff_start;
    let comb_delta = comb_start - entry.ref_comb_start;
    // Testbenches lay out identically up to the DUT, so a reused subtree
    // usually lands where the reference conv put it and its baked offsets
    // need no rewrite.
    let zero = ff_delta == 0 && comb_delta == 0;
    let reloc = |stmts: &[ProtoStatement]| -> Vec<ProtoStatement> {
        if zero {
            stmts.to_vec()
        } else {
            reloc_stmts(stmts, ff_delta, comb_delta)
        }
    };
    let event_statements = entry
        .event_statements
        .iter()
        .map(|(ev, stmts)| (ev.clone(), reloc(stmts)))
        .collect();
    let child_modules = entry
        .child_modules
        .iter()
        .map(|mm| reloc_module_meta(mm, ff_delta, comb_delta))
        .collect();
    let derived_clock_candidates = entry
        .derived_clock_candidates
        .iter()
        .map(|(id, off, nb, pol, neg)| (*id, off.adjust(ff_delta, comb_delta), *nb, *pol, *neg))
        .collect();
    let comb_reloc = entry
        .comb_reloc
        .iter()
        .map(|&(from, to, vs)| (from + comb_delta, to + comb_delta, vs))
        .collect();
    ReusedStatements {
        event_statements,
        comb_statements: reloc(&entry.comb_statements),
        post_comb_fns: reloc(&entry.post_comb_fns),
        child_modules,
        derived_clock_candidates,
        ff_size: entry.ff_size,
        comb_size: entry.comb_size,
        comb_reloc,
    }
}

/// Outcome of consulting the cross-test cache for a component instance.
pub enum ReuseOutcome {
    /// Cache hit — the subtree relocated to this instance, ready to use.
    Hit(ReusedStatements),
    /// Cache miss and we claimed it (single-flight): convert fully, then call
    /// `guard.store(...)` to publish.  Dropping the guard without storing
    /// (e.g. on a conv error) releases the claim so waiters retry.
    Compute(ClaimGuard),
    /// Reuse disabled for this component — convert fully, don't cache.
    Disabled,
}

/// Single-flight claim on a component's cache slot.  Held by the converting
/// thread across the conv; `store` publishes the result, `Drop` releases an
/// unfulfilled claim.
pub struct ClaimGuard {
    cache: Arc<DutReuseCache>,
    key: usize,
    fulfilled: bool,
}

impl ClaimGuard {
    #[allow(clippy::too_many_arguments)]
    pub fn store(
        mut self,
        ff_start: isize,
        comb_start: isize,
        ff_size: usize,
        comb_size: usize,
        event_statements: &HashMap<Event, Vec<ProtoStatement>>,
        comb_statements: &[ProtoStatement],
        post_comb_fns: &[ProtoStatement],
        child_modules: &[ModuleVariableMeta],
        derived_clock_candidates: &[crate::ir::EdgeCandidate],
        comb_reloc: &[(isize, isize, usize)],
    ) {
        let entry = Arc::new(CachedStatements {
            ref_ff_start: ff_start,
            ref_comb_start: comb_start,
            ff_size,
            comb_size,
            event_statements: event_statements.clone(),
            comb_statements: comb_statements.to_vec(),
            post_comb_fns: post_comb_fns.to_vec(),
            child_modules: child_modules.to_vec(),
            derived_clock_candidates: derived_clock_candidates.to_vec(),
            comb_reloc: comb_reloc.to_vec(),
        });
        let mut cache = self.cache.statements.lock().unwrap();
        cache.insert(self.key, Slot::Done(entry));
        self.fulfilled = true;
        self.cache.ready.notify_all();
    }
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if !self.fulfilled {
            let mut cache = self.cache.statements.lock().unwrap();
            cache.remove(&self.key);
            self.cache.ready.notify_all();
        }
    }
}

impl DutReuseCache {
    /// Consult the cross-test cache for a component instance. On a hit, relocate
    /// the cached subtree to `(ff_start, comb_start)`. On a miss, claim the slot
    /// single-flight: peers wait for the guard to publish or abandon the claim.
    /// Relocation runs outside the lock (the slot holds an `Arc`).
    pub(crate) fn try_reuse_or_claim(
        self: &Arc<Self>,
        component_key: *const air::Component,
        alias_enabled: bool,
        ff_start: isize,
        comb_start: isize,
        dut_reuse: bool,
    ) -> ReuseOutcome {
        if !dut_reuse || alias_enabled {
            return ReuseOutcome::Disabled;
        }
        let key = component_key as usize;
        let mut cache = self.statements.lock().unwrap();
        loop {
            match cache.get(&key) {
                Some(Slot::Done(entry)) => {
                    let entry = Arc::clone(entry);
                    drop(cache);
                    return ReuseOutcome::Hit(relocate_entry(&entry, ff_start, comb_start));
                }
                Some(Slot::Computing) => {
                    cache = self.ready.wait(cache).unwrap();
                }
                None => {
                    cache.insert(key, Slot::Computing);
                    return ReuseOutcome::Compute(ClaimGuard {
                        cache: Arc::clone(self),
                        key,
                        fulfilled: false,
                    });
                }
            }
        }
    }
}

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
    // With the comb layout pass on, the parent expands every inst chunk back
    // to its statements (`comb_layout::expand_compiled_blocks`) and the
    // pipeline compiles those; an artifact built here would never run.
    if crate::ir::comb_layout::enabled(context.config.use_4state) {
        return;
    }
    let ff_start_bytes = ff_start;
    let comb_start_bytes = comb_start;
    let component_key: *const air::Component = Arc::as_ptr(&src.component);

    // Input-port aliasing bakes parent-specific offsets into the compiled chunk,
    // so the cache (keyed only by child component) cannot be shared across
    // instances.
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
                    stmt_deps: Arc::new(vec![]),
                    original_stmts: Arc::new(adjust_stmts(&cached.original_stmts)),
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
                stmt_deps: Arc::new(adjusted_deps),
                original_stmts: Arc::new(adjust_stmts(&cached.original_stmts)),
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
                // internal; keep all inputs so analyze_dependency sees the
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

                let event_original = Arc::new(stmts.clone());
                event_funcs.insert(
                    event.clone(),
                    CachedChunk {
                        artifact: Arc::clone(&artifact),
                        input_offsets: input_offsets.clone(),
                        output_offsets: output_offsets.clone(),
                        ff_canonical_offsets: ff_canonical.clone(),
                        stmt_deps: Arc::new(vec![]),
                        original_stmts: Arc::clone(&event_original),
                    },
                );

                *stmts = vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                    artifact,
                    ff_delta_bytes: 0,
                    comb_delta_bytes: 0,
                    input_offsets,
                    output_offsets,
                    ff_canonical_offsets: ff_canonical,
                    stmt_deps: Arc::new(vec![]),
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
            crate::ir::module::dump_stmt_order("child-presort", src.name, all_comb_statements);
            let sorted_comb_for_func = stable_topo_sort(all_comb_statements.clone());
            crate::ir::module::dump_stmt_order("child-postsort", src.name, &sorted_comb_for_func);

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

                let stmt_deps: Arc<Vec<_>> = Arc::new(
                    sorted_comb_for_func
                        .iter()
                        .map(|s| {
                            let mut ins = vec![];
                            let mut outs = vec![];
                            s.gather_variable_offsets(&mut ins, &mut outs);
                            (ins, outs)
                        })
                        .collect(),
                );

                let original_stmts = Arc::new(sorted_comb_for_func.clone());
                *all_comb_statements =
                    vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                        artifact: Arc::clone(&artifact),
                        ff_delta_bytes: 0,
                        comb_delta_bytes: 0,
                        input_offsets: input_offsets.clone(),
                        output_offsets: output_offsets.clone(),
                        ff_canonical_offsets: vec![],
                        stmt_deps: Arc::clone(&stmt_deps),
                        original_stmts,
                    })];

                Some(CachedChunk {
                    artifact,
                    input_offsets,
                    output_offsets,
                    ff_canonical_offsets: vec![],
                    stmt_deps,
                    original_stmts: Arc::new(sorted_comb_for_func),
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
