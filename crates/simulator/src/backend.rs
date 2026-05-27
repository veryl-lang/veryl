//! Backend abstraction for the simulator.
//!
//! A `Backend` may compile a slice of `ProtoStatement`s into a callable
//! artifact.  Two granularities:
//! - **chunk** (Cranelift): one function per contiguous group, slotted
//!   into a per-comb / per-event dispatch list.
//! - **whole** (AOT-C): one function per comb / event statement list,
//!   invoked in place of per-chunk dispatch.
//!
//! The interpreter is the fallback when every backend declines.  The
//! orchestrator lives in [`registry`].

use crate::FuncPtr;
use crate::ir::{Config, Event, ProtoStatement};
use std::sync::Arc;

#[cfg(not(target_family = "wasm"))]
pub mod aot_c;
#[cfg(not(target_family = "wasm"))]
pub mod cranelift;
pub mod inst;
pub mod registry;
pub mod validate;

#[cfg(not(target_family = "wasm"))]
pub use aot_c::AotCBackend;
#[cfg(not(target_family = "wasm"))]
pub use cranelift::CraneliftBackend;
pub use registry::{BackendRegistry, ChunkOutput};

/// A backend that may compile pre-JIT `ProtoStatement`s into native code.
/// All compile methods return `Option` so a backend can decline any
/// input; the orchestrator tries backends in registration order.
pub trait Backend: Send {
    fn name(&self) -> &str;

    /// Whether this backend can emit 4-state (`x`/`z`) arithmetic.
    fn supports_4state(&self) -> bool {
        false
    }

    /// Per-statement support predicate used by chunk-grouping to split
    /// a statement list into jittable / interpreter spans.
    fn supports_stmt(&self, _stmt: &ProtoStatement) -> bool {
        false
    }

    fn compile_chunk(
        &mut self,
        _ctx: &CompileCtx,
        _stmts: &[ProtoStatement],
    ) -> Option<Arc<ChunkArtifact>> {
        None
    }

    fn compile_whole_comb(
        &mut self,
        _ctx: &CompileCtx,
        _stmts: &[ProtoStatement],
    ) -> Option<Arc<dyn CompiledWhole>> {
        None
    }

    fn compile_whole_event(
        &mut self,
        _ctx: &CompileCtx,
        _event: &Event,
        _stmts: &[ProtoStatement],
    ) -> Option<Arc<dyn CompiledWhole>> {
        None
    }

    /// Diagnostic hook: `Some(reason)` produces an eprintln after a
    /// failed `compile_whole_comb`.  Backends typically gate this on
    /// their own env var (e.g. `VERYL_AOT_C_DIAG`).
    fn diagnose_whole_comb_fallback(&self, _stmts: &[ProtoStatement]) -> Option<String> {
        None
    }
}

/// A compiled whole-module / whole-event function dispatched in place
/// of per-statement evaluation.  Shared via `Arc` across instances.
pub trait CompiledWhole: Send + Sync {
    /// `Done`: function ran.  `NotReady`: artifact unavailable (e.g.
    /// async compile pending) — caller must fall back.
    fn try_dispatch(&self, ff: *const u8, comb: *mut u8, log: *mut u8) -> DispatchOutcome;
}

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    Done,
    NotReady,
}

/// Output of a successful `Backend::compile_chunk`.  Shared via
/// `Arc<ChunkArtifact>` across consumers so the keepalive lives until
/// the last reference drops.
pub struct ChunkArtifact {
    pub func: FuncPtr,
    /// Backing resources (mmap, .so handle, ...) the runtime must keep
    /// alive while `func` is callable.
    pub keepalive: Option<Box<dyn Send + Sync>>,
}

impl std::fmt::Debug for ChunkArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkArtifact")
            .field("func", &(self.func as *const ()))
            .field("keepalive", &self.keepalive.is_some())
            .finish()
    }
}

pub struct CompileCtx<'a> {
    pub config: &'a Config,
    pub use_4state: bool,
    /// True when the chunk may contain nested `CompiledBlock` statements.
    /// Cranelift disables load-cache CSE in this case because nested
    /// helpers can mutate comb storage between cached loads.
    pub contains_compiled_block: bool,
}
