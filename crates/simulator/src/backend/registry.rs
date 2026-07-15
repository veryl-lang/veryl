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
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::sync::{Arc, LazyLock, Mutex};

/// Cross-test compiled-chunk cache. Keyed by a 128-bit structural fingerprint
/// of the chunk (Debug of the statements + the codegen-affecting flags). A
/// chunk artifact addresses storage as `base + offset` with `base` supplied at
/// dispatch, so two chunks with identical statements — same ops AND same baked
/// offsets — compile to interchangeable code. `veryl test` lays every testbench
/// out identically (cross-test relocation delta = 0), so a DUT chunk built for
/// one test serves every later test verbatim, collapsing the per-test
/// `try_jit_no_cache` that otherwise re-JITs the whole shared DUT comb each run.
///
/// Populated on miss only; a rare concurrent double-compile just overwrites an
/// equivalent artifact. Never cleared — a `veryl test` process is one-shot and
/// the entries stay hot for its whole run. Gated to `config.dut_reuse` (CLI
/// only), so the unit-test harness (many transient `air::Ir`s) never touches it.
static CHUNK_ARTIFACT_CACHE: LazyLock<Mutex<HashMap<u128, Arc<ChunkArtifact>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Sink that feeds `Debug` bytes into two differently-seeded hashers, yielding a
/// 128-bit fingerprint without materialising the formatted string.
struct FingerprintWriter {
    a: DefaultHasher,
    b: DefaultHasher,
}

impl std::fmt::Write for FingerprintWriter {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.a.write(s.as_bytes());
        self.b.write(s.as_bytes());
        Ok(())
    }
}

/// Structural 128-bit fingerprint of a chunk. `Debug` carries every field of
/// every statement, so this never drifts out of sync with the IR the way a
/// hand-written per-variant hash would — a missed field there would be a silent
/// false-hit miscompile. Collision odds at ~10^4 unique chunks are ~2^-100.
fn chunk_fingerprint(
    use_4state: bool,
    contains_compiled_block: bool,
    stmts: &[ProtoStatement],
) -> u128 {
    use std::fmt::Write;
    let mut w = FingerprintWriter {
        a: DefaultHasher::new(),
        b: DefaultHasher::new(),
    };
    // Seed the second half so the two 64-bit lanes are independent.
    w.b.write_u64(0x9E37_79B9_7F4A_7C15);
    w.a.write_u8(use_4state as u8);
    w.b.write_u8(use_4state as u8);
    w.a.write_u8(contains_compiled_block as u8);
    w.b.write_u8(contains_compiled_block as u8);
    let _ = write!(w, "{stmts:?}");
    ((w.a.finish() as u128) << 64) | w.b.finish() as u128
}

/// Ordered collection of backends.  Whole-module backends should come
/// before chunk backends so a successful whole-module compile elides
/// the per-chunk grouping pass.
#[derive(Default)]
pub struct BackendRegistry {
    backends: Vec<Box<dyn Backend>>,
}

impl BackendRegistry {
    /// Native: register AOT-C (when `config.aot_c`) then Cranelift
    /// (when `config.use_jit`).  Wasm: always empty.
    pub fn for_config(_config: &Config) -> Self {
        let mut r = Self::default();
        #[cfg(not(target_family = "wasm"))]
        {
            if _config.aot_c {
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
    ) -> Option<Arc<dyn CompiledWhole>> {
        self.backends
            .iter_mut()
            .find_map(|b| b.compile_whole_event(ctx, event, stmts))
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
        if let Some(artifact) = CHUNK_ARTIFACT_CACHE.lock().unwrap().get(&key) {
            return Some(Arc::clone(artifact));
        }
        // Compile outside the lock; a concurrent peer may compile the same
        // chunk, but both artifacts are equivalent so the last insert wins.
        let artifact = self
            .backends
            .iter_mut()
            .find_map(|b| b.compile_chunk(ctx, stmts));
        if let Some(artifact) = &artifact {
            CHUNK_ARTIFACT_CACHE
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
        let mut out = Vec::new();
        let mut current_jittable: Option<bool> = None;
        let mut current_group: Vec<ProtoStatement> = Vec::new();

        let flush =
            |group: Vec<ProtoStatement>, was_jittable: bool, this: &mut Self, out: &mut Vec<_>| {
                if was_jittable {
                    Self::compile_group(this, ctx, group, max_chunk_size, out);
                } else {
                    out.push(ChunkOutput::Interpreted(group));
                }
            };

        for stmt in proto {
            let jittable = self.any_supports_stmt(&stmt);
            if current_jittable == Some(jittable) {
                current_group.push(stmt);
            } else {
                if let Some(was_jittable) = current_jittable {
                    let group = std::mem::take(&mut current_group);
                    flush(group, was_jittable, self, &mut out);
                }
                current_jittable = Some(jittable);
                current_group.push(stmt);
            }
        }
        if let Some(was_jittable) = current_jittable {
            flush(current_group, was_jittable, self, &mut out);
        }
        out
    }

    fn compile_group(
        &mut self,
        ctx: &CompileCtx,
        group: Vec<ProtoStatement>,
        max_chunk_size: usize,
        out: &mut Vec<ChunkOutput>,
    ) {
        if group.len() <= max_chunk_size {
            self.compile_group_bisect(ctx, group, out);
        } else {
            for chunk in group.chunks(max_chunk_size) {
                self.compile_group_bisect(ctx, chunk.to_vec(), out);
            }
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
