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
            match self.try_compile_chunk(ctx, &group) {
                Some(artifact) => out.push(ChunkOutput::Compiled(artifact)),
                None => out.push(ChunkOutput::Interpreted(group)),
            }
        } else {
            for chunk in group.chunks(max_chunk_size) {
                let chunk = chunk.to_vec();
                match self.try_compile_chunk(ctx, &chunk) {
                    Some(artifact) => out.push(ChunkOutput::Compiled(artifact)),
                    None => out.push(ChunkOutput::Interpreted(chunk)),
                }
            }
        }
    }
}

pub enum ChunkOutput {
    Compiled(Arc<ChunkArtifact>),
    Interpreted(Vec<ProtoStatement>),
}
