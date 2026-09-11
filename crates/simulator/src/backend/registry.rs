//! Backend orchestrator.
//!
//! At build time: whole-comb / whole-event paths try `compile_whole_*`
//! on each backend in order, taking the first `Some`.  The chunk path
//! groups statements by `supports_stmt` and asks the registered
//! chunk backend to compile each jittable group.  Backends that
//! decline are skipped; the interpreter is the ultimate fallback.

use super::{Backend, ChunkArtifact, CompileCtx, CompiledWhole};
use crate::ir::{Config, Event, ProtoStatement};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex};

/// Cross-test compiled-chunk cache. Keyed by a 128-bit structural fingerprint
/// of the chunk (`Hash` of the statements + the codegen-affecting flags). A
/// chunk artifact addresses storage as `base + offset` with `base` supplied at
/// dispatch, so two chunks with identical statements — same ops AND same baked
/// offsets — compile to interchangeable code. `veryl test` lays every testbench
/// out identically (cross-test relocation delta = 0), so a DUT chunk built for
/// one test serves every later test verbatim, collapsing the per-test
/// `try_jit_no_cache` that otherwise re-JITs the whole shared DUT comb each run.
///
/// Populated on miss only; a rare concurrent double-compile just overwrites an
/// equivalent artifact. Shared by the registries and compiler helpers of one
/// `BuildSession`, so another IR or backend configuration cannot reuse it.
pub(crate) type ChunkArtifactCache = Arc<Mutex<HashMap<u128, Arc<ChunkArtifact>>>>;

/// Lets the IR's `Hash` impls feed XXH3-128 directly, replacing the old sink
/// that fed `Debug`-formatted strings (its dominant per-test cost). Keying uses
/// `finish_128`; the trait's 64-bit `finish` is unused.
struct Fp128(twox_hash::xxhash3_128::Hasher);

impl Fp128 {
    fn new() -> Self {
        Fp128(twox_hash::xxhash3_128::Hasher::new())
    }

    fn finish_128(&self) -> u128 {
        self.0.finish_128()
    }
}

impl Hasher for Fp128 {
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    fn finish(&self) -> u64 {
        self.0.finish_128() as u64
    }
}

/// Structural 128-bit fingerprint of a chunk. Every `Hash` impl mirrors its
/// `Debug` — derived where `Debug` is derived, hand-written over an exhaustive
/// destructure where `Debug` is — so a codegen-affecting field can never be
/// silently dropped (a missed field would be a false-hit miscompile). `ProtoAssignStatement`'s `token` and `ChunkArtifact`'s
/// `func` address are excluded, so the fingerprint is token- and pointer-agnostic
/// (both are codegen-irrelevant). XXH3-128 gives full 128-bit distribution, so
/// collision odds at ~10^4 unique chunks stay ~2^-100.
fn chunk_fingerprint(
    use_4state: bool,
    contains_compiled_block: bool,
    stmts: &[ProtoStatement],
) -> u128 {
    let mut h = Fp128::new();
    h.write_u8(use_4state as u8);
    h.write_u8(contains_compiled_block as u8);
    stmts.hash(&mut h);
    h.finish_128()
}

/// Structural fingerprint of a whole comb statement list plus the extra inputs
/// that determine the sort/DCE/JIT pipeline result: `use_4state` (codegen) and
/// `extra` (a caller-supplied digest of the event-statement liveness census and
/// the DCE protect set — the pieces of dead-var DCE that depend on state outside
/// the comb list). Token- and pointer-agnostic like `chunk_fingerprint`.
pub(crate) fn whole_comb_fingerprint(
    use_4state: bool,
    stmts: &[ProtoStatement],
    extra: u128,
) -> u128 {
    let mut h = Fp128::new();
    h.write_u8(use_4state as u8);
    stmts.hash(&mut h);
    // Domain-separate the extra digest from the statement bytes.
    h.write_u8(0xE5);
    h.write_u128(extra);
    h.finish_128()
}

/// `whole_comb_fingerprint` for a caller that hashed the statement list
/// earlier than it builds the key.
pub(crate) fn whole_comb_fingerprint_from(stmts_fp: u128, extra: u128) -> u128 {
    let mut h = Fp128::new();
    h.write_u128(stmts_fp);
    h.write_u8(0xE5);
    h.write_u128(extra);
    h.finish_128()
}

/// Ordered collection of backends.  Whole-module backends should come
/// before chunk backends so a successful whole-module compile elides
/// the per-chunk grouping pass.
#[derive(Default)]
pub struct BackendRegistry {
    backends: Vec<Box<dyn Backend>>,
    pub(crate) chunk_cache: ChunkArtifactCache,
}

impl BackendRegistry {
    /// Native: register AOT-C (when `config.aot_c`) then Cranelift
    /// (when `config.use_jit`).  Wasm: always empty.
    pub fn for_config(_config: &Config) -> Self {
        let mut r = Self::default();
        #[cfg(not(target_family = "wasm"))]
        {
            // AOT-C dlopens the object it compiles, so a statically linked
            // build would pay every `cc` invocation and throw the result away.
            if _config.aot_c && crate::component::loader::native_loading_supported() {
                r.register(Box::new(super::AotCBackend::new(
                    _config.aot_c_async,
                    _config.aot_c_event,
                )));
            }
            if _config.use_jit {
                r.register(Box::new(super::CraneliftBackend::new()));
            }
        }
        r
    }

    fn register(&mut self, backend: Box<dyn Backend>) {
        self.backends.push(backend);
    }

    pub fn try_compile_whole_comb(
        &mut self,
        ctx: &CompileCtx,
        stmts: &[ProtoStatement],
    ) -> Option<Arc<dyn CompiledWhole>> {
        self.backends
            .iter_mut()
            .find_map(|b| b.compile_whole_comb(ctx, stmts))
    }

    pub fn try_compile_whole_event(
        &mut self,
        ctx: &CompileCtx,
        event: &Event,
        stmts: &[ProtoStatement],
        gates: &[crate::ir::opt::event_gate::EventGate],
    ) -> Option<Arc<dyn CompiledWhole>> {
        self.backends
            .iter_mut()
            .find_map(|b| b.compile_whole_event(ctx, event, stmts, gates))
    }

    pub fn try_compile_chunk(
        &mut self,
        ctx: &CompileCtx,
        stmts: &[ProtoStatement],
    ) -> Option<Arc<ChunkArtifact>> {
        if !ctx.config.dut_reuse {
            return self
                .backends
                .iter_mut()
                .find_map(|b| b.compile_chunk(ctx, stmts));
        }
        let key = chunk_fingerprint(ctx.use_4state, ctx.contains_compiled_block, stmts);
        if let Some(artifact) = self.chunk_cache.lock().unwrap().get(&key) {
            return Some(Arc::clone(artifact));
        }
        // Compile outside the lock; a concurrent peer may compile the same
        // chunk, but both artifacts are equivalent so the last insert wins.
        let mut artifact = self
            .backends
            .iter_mut()
            .find_map(|b| b.compile_chunk(ctx, stmts));
        if let Some(artifact) = &mut artifact {
            // Stamp the fingerprint so a fingerprint over a parent chunk (or
            // whole comb) that embeds this as a nested CompiledBlock prints a
            // stable, content-derived id rather than the `func` address. The
            // Arc is fresh here (refcount 1), so `get_mut` always succeeds.
            if let Some(a) = Arc::get_mut(artifact) {
                a.content_fp = Some(key);
            }
            self.chunk_cache
                .lock()
                .unwrap()
                .insert(key, Arc::clone(artifact));
        }
        artifact
    }

    pub fn any_supports_stmt(&self, stmt: &ProtoStatement) -> bool {
        self.backends.iter().any(|b| b.supports_stmt(stmt))
    }

    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    pub fn diagnose_whole_comb_fallback(&self, stmts: &[ProtoStatement]) -> Option<String> {
        self.backends
            .iter()
            .find_map(|b| b.diagnose_whole_comb_fallback(stmts))
    }

    /// Group `stmts` by `supports_stmt` and produce `ChunkOutput`s,
    /// splitting jittable groups into pieces of at most `max_chunk_size`
    /// to bound regalloc cost.  Declined chunks become `Interpreted`.
    pub fn build_chunked(
        &mut self,
        ctx: &CompileCtx,
        proto: Vec<ProtoStatement>,
        max_chunk_size: usize,
    ) -> Vec<ChunkOutput> {
        let plans = self.plan_chunked(proto, max_chunk_size);
        self.compile_plans(ctx, plans)
            .into_iter()
            .flatten()
            .collect()
    }

    /// `build_chunked` without the compile step, so a caller with thousands of
    /// chunks can spread the compiles over threads (`compile_plans_parallel`);
    /// the cutting is a cheap walk.
    pub fn plan_chunked(
        &self,
        proto: Vec<ProtoStatement>,
        max_chunk_size: usize,
    ) -> Vec<ChunkPlan> {
        let mut out = Vec::new();
        let mut current_jittable: Option<bool> = None;
        let mut current_group: Vec<ProtoStatement> = Vec::new();

        let flush = |group: Vec<ProtoStatement>, was_jittable: bool, out: &mut Vec<ChunkPlan>| {
            if was_jittable {
                Self::plan_group(group, max_chunk_size, out);
            } else {
                out.push(ChunkPlan::Interpreted(group));
            }
        };

        for stmt in proto {
            let jittable = self.any_supports_stmt(&stmt);
            if current_jittable == Some(jittable) {
                current_group.push(stmt);
            } else {
                if let Some(was_jittable) = current_jittable {
                    let group = std::mem::take(&mut current_group);
                    flush(group, was_jittable, &mut out);
                }
                current_jittable = Some(jittable);
                current_group.push(stmt);
            }
        }
        if let Some(was_jittable) = current_jittable {
            flush(current_group, was_jittable, &mut out);
        }
        out
    }

    /// One output list per plan: a chunk the backend bisects yields several
    /// outputs, and a caller that planned several pieces must cut them apart.
    pub fn compile_plans(
        &mut self,
        ctx: &CompileCtx,
        plans: Vec<ChunkPlan>,
    ) -> Vec<Vec<ChunkOutput>> {
        plans
            .into_iter()
            .map(|plan| {
                let mut out = Vec::new();
                match plan {
                    ChunkPlan::Interpreted(stmts) => out.push(ChunkOutput::Interpreted(stmts)),
                    ChunkPlan::Compile(stmts) => self.compile_group_bisect(ctx, stmts, &mut out),
                }
                out
            })
            .collect()
    }

    /// Split `group` into chunks of at most `max_chunk_size` statements,
    /// COUNTING NESTED ONES: an `if` the IR builder fused is one entry here
    /// but a whole function to the backend.  One whose own tree exceeds the
    /// budget cannot be divided (that would duplicate its guard), so it lands
    /// alone and overshoots.
    fn plan_group(group: Vec<ProtoStatement>, max_chunk_size: usize, out: &mut Vec<ChunkPlan>) {
        let diag = std::env::var("VERYL_JIT_CHUNK_DIAG").as_deref() == Ok("1");
        // `VERYL_JIT_CHUNK_BY_ENTRIES=1` restores the old unit so a design's
        // before/after can be measured from one binary.
        let by_entries = std::env::var("VERYL_JIT_CHUNK_BY_ENTRIES").as_deref() == Ok("1");
        let mut cur: Vec<ProtoStatement> = Vec::new();
        let mut mass = 0usize;
        for stmt in group {
            let m = if by_entries { 1 } else { stmt.statement_mass() };
            // `mass > 0` keeps an oversized statement from flushing an empty
            // chunk ahead of itself; it lands alone and is reported.
            if mass > 0 && mass + m > max_chunk_size {
                out.push(ChunkPlan::Compile(std::mem::take(&mut cur)));
                mass = 0;
            }
            if diag && m > max_chunk_size {
                eprintln!(
                    "[jit_chunk] one statement is {m} statements, over the \
                     {max_chunk_size} budget; it takes a chunk alone"
                );
            }
            mass += m;
            cur.push(stmt);
        }
        if !cur.is_empty() {
            out.push(ChunkPlan::Compile(cur));
        }
    }

    /// Compile a jittable group; on a `compile_chunk` coverage bail (some
    /// statement's emitter returned `None`), bisect and retry the halves rather
    /// than dropping the WHOLE group to the interpreter — otherwise one stray
    /// emitter gap turns a whole large comb interpreted. Each sub-chunk reloads
    /// its inputs (per-chunk `load_cache`, no cross-group store elision), so the
    /// split is value-preserving. Only fires on failure.
    fn compile_group_bisect(
        &mut self,
        ctx: &CompileCtx,
        group: Vec<ProtoStatement>,
        out: &mut Vec<ChunkOutput>,
    ) {
        if group.is_empty() {
            return;
        }
        if let Some(artifact) = self.try_compile_chunk(ctx, &group) {
            out.push(ChunkOutput::Compiled(artifact));
            return;
        }
        if group.len() == 1 {
            // Genuinely un-buildable single statement: interpret just it.
            if std::env::var("VERYL_CHUNK_BISECT_DIAG").as_deref() == Ok("1") {
                eprintln!(
                    "[chunk_bisect] isolated uncovered stmt: {}",
                    classify_proto_stmt(&group[0])
                );
            }
            out.push(ChunkOutput::Interpreted(group));
            return;
        }
        let mut group = group;
        let right = group.split_off(group.len() / 2);
        self.compile_group_bisect(ctx, group, out);
        self.compile_group_bisect(ctx, right, out);
    }
}

/// One-line classification of a `ProtoStatement` for `VERYL_CHUNK_BISECT_DIAG`:
/// names the construct (and width / dynamic-select dims) a chunk backend
/// declined to emit, so emitter gaps can be prioritised by hotness.
fn classify_proto_stmt(s: &ProtoStatement) -> String {
    match s {
        ProtoStatement::Assign(a) => {
            let dynsel = a.dynamic_select.as_ref().map(|d| {
                format!(
                    " dynsel(elem={} n={} full={})",
                    d.elem_width,
                    d.num_elements,
                    d.elem_width * d.num_elements
                )
            });
            format!(
                "Assign dst_width={}{}{}",
                a.dst_width,
                if a.select.is_some() { " select" } else { "" },
                dynsel.unwrap_or_default(),
            )
        }
        ProtoStatement::AssignDynamic(a) => {
            let full = a.dst_width * a.dst_num_elements;
            format!(
                "AssignDynamic dst_width={} num_elems={} full={}",
                a.dst_width, a.dst_num_elements, full
            )
        }
        ProtoStatement::If(_) => "If".to_string(),
        ProtoStatement::Case(_) => "Case".to_string(),
        ProtoStatement::For(_) => "For".to_string(),
        ProtoStatement::Break => "Break".to_string(),
        ProtoStatement::SystemFunctionCall(_) => "SysFn".to_string(),
        ProtoStatement::CompiledBlock(_) => "CompiledBlock".to_string(),
        ProtoStatement::SequentialBlock(b) => format!("SequentialBlock(len={})", b.len()),
        ProtoStatement::TbMethodCall { .. } => "TbMethodCall".to_string(),
    }
}

pub enum ChunkOutput {
    Compiled(Arc<ChunkArtifact>),
    Interpreted(Vec<ProtoStatement>),
}

/// One chunk as `plan_chunked` cut it, before any backend saw it.
pub enum ChunkPlan {
    Interpreted(Vec<ProtoStatement>),
    Compile(Vec<ProtoStatement>),
}

/// `compile_plans` on helper threads, for the unified-comb JIT: it runs behind
/// the comb-pipeline single-flight while every other test worker waits on it,
/// so the helpers take cores that would otherwise idle.  Each helper gets its
/// own registry; the backends keep no state of their own (the executable arena
/// and the artifact cache are shared behind locks), so artifacts from any
/// thread are interchangeable.  Results stay in plan order.
pub fn compile_plans_parallel(
    registry: &mut BackendRegistry,
    ctx: &CompileCtx,
    plans: Vec<ChunkPlan>,
) -> Vec<Vec<ChunkOutput>> {
    let to_compile = plans
        .iter()
        .filter(|p| matches!(p, ChunkPlan::Compile(_)))
        .count();
    let threads = jit_parallelism().min(to_compile);
    if threads < 2 {
        return registry.compile_plans(ctx, plans);
    }

    let n = plans.len();
    let queue = Mutex::new(plans.into_iter().enumerate());
    let mut done: Vec<Option<Vec<ChunkOutput>>> = (0..n).map(|_| None).collect();

    // The compile recurses over statement trees like the workers do.
    let parts: Vec<Vec<(usize, Vec<ChunkOutput>)>> = std::thread::scope(|s| {
        let queue = &queue;
        let chunk_cache = &registry.chunk_cache;
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(crate::IR_WALK_STACK_BYTES)
                    .spawn_scoped(s, move || {
                        let mut local = BackendRegistry::for_config(ctx.config);
                        local.chunk_cache = Arc::clone(chunk_cache);
                        let mut mine = Vec::new();
                        loop {
                            let Some((i, plan)) = queue.lock().unwrap().next() else {
                                break;
                            };
                            let mut out = Vec::new();
                            match plan {
                                ChunkPlan::Interpreted(stmts) => {
                                    out.push(ChunkOutput::Interpreted(stmts))
                                }
                                ChunkPlan::Compile(stmts) => {
                                    local.compile_group_bisect(ctx, stmts, &mut out)
                                }
                            }
                            mine.push((i, out));
                        }
                        mine
                    })
                    .expect("spawn chunk compiler")
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    for (i, out) in parts.into_iter().flatten() {
        done[i] = Some(out);
    }
    done.into_iter()
        .map(|o| o.expect("every plan is claimed exactly once"))
        .collect()
}

fn jit_parallelism() -> usize {
    static N: LazyLock<usize> = LazyLock::new(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });
    *N
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        ExpressionContext, ProtoAssignStatement, ProtoExpression, ProtoIfStatement, VarOffset,
    };
    use veryl_analyzer::value::{Value, ValueU64};
    use veryl_parser::token_range::TokenRange;

    fn assign(off: isize) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(off),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Value {
                value: Value::U64(ValueU64 {
                    payload: 1,
                    mask_xz: 0,
                    width: 32,
                    signed: false,
                }),
                width: 32,
                expr_context: ExpressionContext {
                    width: 32,
                    signed: false,
                },
            },
            dst_ff_current_offset: -1,
            token: TokenRange::default(),
        })
    }

    /// An `if` holding `n` assigns per arm: 1 + 2n statements.
    fn fat_if(n: usize) -> ProtoStatement {
        ProtoStatement::If(ProtoIfStatement {
            cond: Some(ProtoExpression::Variable {
                var_offset: VarOffset::Comb(0x8000),
                select: None,
                dynamic_select: None,
                width: 1,
                var_full_width: 1,
                expr_context: ExpressionContext {
                    width: 1,
                    signed: false,
                },
            }),
            true_side: (0..n).map(|i| assign(i as isize * 4)).collect(),
            false_side: (0..n).map(|i| assign(i as isize * 4)).collect(),
        })
    }

    /// The budget is nested statements, so a handful of fat conditionals must
    /// still be split — counting top-level entries let them through as one.
    #[test]
    fn chunking_budgets_nested_statements_not_entries() {
        let config = Config {
            use_jit: true,
            aot_c: false,
            ..Default::default()
        };
        let mut r = BackendRegistry::for_config(&config);
        if r.is_empty() {
            return; // wasm: no chunk backend
        }
        let ctx = CompileCtx {
            config: &config,
            use_4state: false,
            contains_compiled_block: false,
        };
        // 20 entries x 201 statements = 4020 statements, budget 1024.
        let group: Vec<ProtoStatement> = (0..20).map(|_| fat_if(100)).collect();
        let total: usize = group.iter().map(|s| s.statement_mass()).sum();
        let out = r.build_chunked(&ctx, group, 1024);
        let chunks = out.len();
        assert!(
            chunks >= total.div_ceil(1024),
            "{total} statements in {chunks} chunk(s) under a 1024 budget: \
             the budget is counting entries, not statements",
        );
        // ...and not shattered: the budget should be roughly filled.
        assert!(
            chunks <= 2 * total.div_ceil(1024),
            "{chunks} chunks is too many"
        );
    }

    #[test]
    fn parallel_compile_matches_the_sequential_chunking() {
        // Planning then compiling on helper threads must hand back the same
        // chunks, in the same order, as the one-thread path.
        let config = Config {
            use_jit: true,
            aot_c: false,
            ..Default::default()
        };
        let mut r = BackendRegistry::for_config(&config);
        if r.is_empty() {
            return; // wasm: no chunk backend
        }
        let ctx = CompileCtx {
            config: &config,
            use_4state: false,
            contains_compiled_block: false,
        };
        let group: Vec<ProtoStatement> = (0..40).map(|_| fat_if(50)).collect();
        let shape = |out: &[ChunkOutput]| -> Vec<(bool, u128)> {
            out.iter()
                .map(|o| match o {
                    ChunkOutput::Compiled(a) => (true, a.content_fp.unwrap_or(0)),
                    ChunkOutput::Interpreted(s) => (false, s.len() as u128),
                })
                .collect()
        };
        let sequential = shape(&r.build_chunked(&ctx, group.clone(), 512));
        let plans = r.plan_chunked(group, 512);
        let parallel: Vec<ChunkOutput> = compile_plans_parallel(&mut r, &ctx, plans)
            .into_iter()
            .flatten()
            .collect();
        assert!(sequential.len() > 1, "one chunk proves nothing");
        assert_eq!(shape(&parallel), sequential);
    }

    /// A single statement bigger than the budget cannot be divided (splitting
    /// inside a conditional would duplicate its guard), so it lands alone
    /// rather than dragging neighbours into an oversized chunk.
    #[test]
    fn an_oversized_statement_takes_a_chunk_alone() {
        let config = Config {
            use_jit: true,
            aot_c: false,
            ..Default::default()
        };
        let mut r = BackendRegistry::for_config(&config);
        if r.is_empty() {
            return;
        }
        let ctx = CompileCtx {
            config: &config,
            use_4state: false,
            contains_compiled_block: false,
        };
        let group = vec![assign(0), fat_if(2000), assign(4)];
        let out = r.build_chunked(&ctx, group, 1024);
        assert!(
            out.len() >= 3,
            "the oversized statement must not share a chunk; got {} chunk(s)",
            out.len(),
        );
    }
}
