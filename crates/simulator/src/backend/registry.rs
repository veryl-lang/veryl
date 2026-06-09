//! Backend orchestrator.
//!
//! At build time: whole-comb / whole-event paths try `compile_whole_*`
//! on each backend in order, taking the first `Some`.  The chunk path
//! groups statements by `supports_stmt` and asks the registered
//! chunk backend to compile each jittable group.  Backends that
//! decline are skipped; the interpreter is the ultimate fallback.

use super::{Backend, ChunkArtifact, CompileCtx, CompiledWhole};
use crate::ir::{Config, Event, ProtoStatement};
use std::sync::Arc;

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
        self.backends
            .iter_mut()
            .find_map(|b| b.compile_chunk(ctx, stmts))
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
