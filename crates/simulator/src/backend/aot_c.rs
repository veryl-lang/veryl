//! AOT-C (`cc -O3` → `.so`) whole-module backend.
//!
//! Emits the entire comb or event statement list as a single C
//! function, hands it to an external compiler, dlopens the resulting
//! `.so`, and dispatches the loaded function in place of per-chunk
//! Cranelift.  Async mode publishes the compiled module via `OnceLock`;
//! `try_dispatch` returns `NotReady` until then.  See [`emit`] for the
//! pipeline.

pub(crate) mod emit;

use crate::backend::{Backend, CompileCtx, CompiledWhole, DispatchOutcome};
use crate::ir::{Event, ProtoStatement};
use std::sync::Arc;

pub struct AotCBackend {
    async_mode: bool,
    /// When false, only whole-comb compile is attempted.
    event_enabled: bool,
}

impl AotCBackend {
    pub fn new(async_mode: bool, event_enabled: bool) -> Self {
        Self {
            async_mode,
            event_enabled,
        }
    }
}

/// Probe `cc --version` (honoring `VERYL_AOT_CC`).  Used by
/// `Config::all()` to skip this backend on hosts without a compiler.
pub fn cc_available() -> bool {
    let cc = std::env::var("VERYL_AOT_CC").unwrap_or_else(|_| "cc".to_string());
    std::process::Command::new(cc)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

impl Backend for AotCBackend {
    fn name(&self) -> &str {
        "aot_c"
    }

    fn supports_4state(&self) -> bool {
        // The C emitter does not generate mask_xz tracking.
        false
    }

    fn compile_whole_comb(
        &mut self,
        ctx: &CompileCtx,
        stmts: &[ProtoStatement],
    ) -> Option<Arc<dyn CompiledWhole>> {
        if ctx.use_4state {
            return None;
        }
        let cell = emit::prepare_comb(stmts, self.async_mode)?;
        Some(Arc::new(AotCWhole { cell }))
    }

    fn compile_whole_event(
        &mut self,
        ctx: &CompileCtx,
        _event: &Event,
        stmts: &[ProtoStatement],
    ) -> Option<Arc<dyn CompiledWhole>> {
        if ctx.use_4state || !self.event_enabled {
            return None;
        }
        let cell = emit::prepare_event(stmts, self.async_mode)?;
        Some(Arc::new(AotCWhole { cell }))
    }

    fn diagnose_whole_comb_fallback(&self, stmts: &[ProtoStatement]) -> Option<String> {
        if !emit::diag_enabled() {
            return None;
        }
        Some(emit::comb_fallback_reason(stmts))
    }
}

struct AotCWhole {
    cell: emit::AotCell,
}

impl CompiledWhole for AotCWhole {
    fn try_dispatch(&self, ff: *const u8, comb: *mut u8, log: *mut u8) -> DispatchOutcome {
        match self.cell.get() {
            Some(m) => {
                // SAFETY: caller provides pointers valid for the
                // function's reads/writes; emitted code follows the
                // standard JIT ABI (ff, comb, write_log).
                unsafe {
                    (m.func)(ff, comb as *const u8, log);
                }
                DispatchOutcome::Done
            }
            None => DispatchOutcome::NotReady,
        }
    }
}
