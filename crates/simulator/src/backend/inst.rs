//! Per-instance JIT chunk compilation for child module declarations, and
//! the cross-test DUT reuse built on top of it.
//!
//! `try_compile_inst_chunks` compiles a child's comb / event statements via
//! the registry's chunk path and replaces them with `CompiledBlock`s that
//! reference the artifact; instances within one build reuse the compiled
//! function with adjusted byte deltas (`Context::chunk_cache`).
//!
//! Cross-test reuse goes a level up: `GLOBAL_STMT_CACHE` caches a whole
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
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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

// Each component (by `Arc` pointer) → the id of the FIRST test top that
// converted it.  Appearing later under a DIFFERENT top means it's shared across
// testbenches = the reusable DUT (a per-test wrapper gets a distinct `Arc` per
// test, so never recurs).  Replication WITHIN one top (SMP: a core per hart)
// shares the id, so it does NOT de-alias — what a single long boot wants.
static SEEN_COMPONENTS: LazyLock<Mutex<HashMap<usize, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::default()));

/// Monotonic id source for test tops; one per `ProtoModule::conv`.
static NEXT_TEST_TOP_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh test-top id (call once per testbench conv).
pub fn next_test_top_id() -> u64 {
    NEXT_TEST_TOP_ID.fetch_add(1, Ordering::Relaxed)
}

/// Record this component under `top_id` and report whether it had already been
/// converted under a DIFFERENT test top (i.e. it recurs across testbenches).
/// Atomic check-and-insert so concurrent test threads agree on the first
/// sighting; same-top re-appearances (SMP replication) return false.
fn mark_seen_and_is_recurring(component_key: *const air::Component, top_id: u64) -> bool {
    let mut seen = SEEN_COMPONENTS.lock().unwrap();
    match seen.entry(component_key as usize) {
        Entry::Occupied(e) => *e.get() != top_id,
        Entry::Vacant(e) => {
            e.insert(top_id);
            false
        }
    }
}

/// Whether a child instance's ports alias the parent slot they're wired to.
/// Aliasing bakes parent offsets into the child's chunk, blocking the single-
/// delta relocation reuse needs — so de-alias only the DUT boundary (the topmost
/// component recurring across tests) and keep everything else aliased.
pub fn port_alias_enabled(
    component_key: *const air::Component,
    own_ff_bytes: usize,
    own_comb_bytes: usize,
    in_reuse_dut: bool,
    test_top_id: u64,
    dut_reuse: bool,
) -> bool {
    if std::env::var("VERYL_DISABLE_PORT_ALIAS").as_deref() == Ok("1") {
        return false; // blunt override: de-alias every boundary (bring-up)
    }
    if !dut_reuse {
        return true; // reuse off: keep all boundaries aliased (no global state touched)
    }
    let recurring = mark_seen_and_is_recurring(component_key, test_top_id);
    let is_dut_boundary =
        recurring && !in_reuse_dut && (own_ff_bytes + own_comb_bytes) >= dut_reuse_min_bytes();
    !is_dut_boundary
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
    derived_clock_candidates: Vec<(air::VarId, VarOffset, usize)>,
}

/// Single-flight cache slot: one thread `Computing` a component blocks others
/// (waiting on `STMT_CV`) until it publishes `Done`, so parallel tests share the
/// first conv of a shared DUT instead of all converting it redundantly.
enum Slot {
    Computing,
    Done(Arc<CachedStatements>),
}

static GLOBAL_STMT_CACHE: LazyLock<Mutex<HashMap<usize, Slot>>> =
    LazyLock::new(|| Mutex::new(HashMap::default()));
static STMT_CV: LazyLock<Condvar> = LazyLock::new(Condvar::new);

/// Relocated subtree internals returned to `InstDeclaration::conv` on a cache
/// hit.  Derived-clock event ids are still keyed by the reference conv's
/// (grandchild) internal ids; the caller re-keys them to fresh ids.
pub struct ReusedStatements {
    pub event_statements: HashMap<Event, Vec<ProtoStatement>>,
    pub comb_statements: Vec<ProtoStatement>,
    pub post_comb_fns: Vec<ProtoStatement>,
    pub child_modules: Vec<ModuleVariableMeta>,
    pub derived_clock_candidates: Vec<(air::VarId, VarOffset, usize)>,
    pub ff_size: usize,
    pub comb_size: usize,
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
                stmt_deps: cb
                    .stmt_deps
                    .iter()
                    .map(|(ins, outs)| {
                        (
                            adjust_offsets_vec(ins, ff_delta, comb_delta),
                            adjust_offsets_vec(outs, ff_delta, comb_delta),
                        )
                    })
                    .collect(),
                original_stmts: reloc_stmts(&cb.original_stmts, ff_delta, comb_delta),
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
    let event_statements = entry
        .event_statements
        .iter()
        .map(|(ev, stmts)| (ev.clone(), reloc_stmts(stmts, ff_delta, comb_delta)))
        .collect();
    let child_modules = entry
        .child_modules
        .iter()
        .map(|mm| reloc_module_meta(mm, ff_delta, comb_delta))
        .collect();
    let derived_clock_candidates = entry
        .derived_clock_candidates
        .iter()
        .map(|(id, off, nb)| (*id, off.adjust(ff_delta, comb_delta), *nb))
        .collect();
    ReusedStatements {
        event_statements,
        comb_statements: reloc_stmts(&entry.comb_statements, ff_delta, comb_delta),
        post_comb_fns: reloc_stmts(&entry.post_comb_fns, ff_delta, comb_delta),
        child_modules,
        derived_clock_candidates,
        ff_size: entry.ff_size,
        comb_size: entry.comb_size,
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
        derived_clock_candidates: &[(air::VarId, VarOffset, usize)],
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
        });
        let mut cache = GLOBAL_STMT_CACHE.lock().unwrap();
        cache.insert(self.key, Slot::Done(entry));
        self.fulfilled = true;
        STMT_CV.notify_all();
    }
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if !self.fulfilled {
            let mut cache = GLOBAL_STMT_CACHE.lock().unwrap();
            cache.remove(&self.key);
            STMT_CV.notify_all();
        }
    }
}

/// Consult the cross-test cache for a component instance.  On a hit, relocate
/// the cached subtree to `(ff_start, comb_start)`.  On a miss, claim the slot
/// (single-flight): later threads requesting the same component block until this
/// one publishes via the returned guard, sharing the conv instead of redoing it.
/// Relocation runs outside the lock (the slot holds an `Arc`).
pub fn try_reuse_or_claim(
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
    let mut cache = GLOBAL_STMT_CACHE.lock().unwrap();
    loop {
        match cache.get(&key) {
            Some(Slot::Done(entry)) => {
                let entry = Arc::clone(entry);
                drop(cache);
                return ReuseOutcome::Hit(relocate_entry(&entry, ff_start, comb_start));
            }
            Some(Slot::Computing) => {
                cache = STMT_CV.wait(cache).unwrap();
            }
            None => {
                cache.insert(key, Slot::Computing);
                return ReuseOutcome::Compute(ClaimGuard {
                    key,
                    fulfilled: false,
                });
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
