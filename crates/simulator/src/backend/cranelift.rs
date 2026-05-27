//! Cranelift JIT backend.  Per-chunk: each jittable group of
//! `ProtoStatement`s becomes one function.  Stateless — all
//! configuration flows through `CompileCtx` per compile call.

pub(crate) mod expression;
pub(crate) mod helpers;
pub(crate) mod runtime;
pub(crate) mod statement;

use crate::backend::{Backend, ChunkArtifact, CompileCtx};
use crate::ir::ProtoStatement;
use std::sync::Arc;

pub struct CraneliftBackend;

impl CraneliftBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CraneliftBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for CraneliftBackend {
    fn name(&self) -> &str {
        "cranelift"
    }

    fn supports_4state(&self) -> bool {
        true
    }

    fn supports_stmt(&self, stmt: &ProtoStatement) -> bool {
        stmt.can_build_binary()
    }

    fn compile_chunk(
        &mut self,
        ctx: &CompileCtx,
        stmts: &[ProtoStatement],
    ) -> Option<Arc<ChunkArtifact>> {
        // Disable load-cache CSE when the chunk may contain nested
        // CompiledBlock helpers that mutate comb storage.
        let result = if ctx.contains_compiled_block {
            runtime::build_binary_no_cache(ctx.config, stmts.to_vec())
        } else {
            runtime::build_binary(ctx.config, stmts.to_vec())
        };
        let (func, mmap) = result?;
        Some(Arc::new(ChunkArtifact {
            func,
            keepalive: Some(Box::new(mmap)),
        }))
    }
}
