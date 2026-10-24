//! AOT-C (`cc -O3` → `.so`) backend.
//!
//! Emits an event statement list as a single C function, and a module's
//! dataflow comb settle as a set of cell functions (`emit::prepare_dataflow`),
//! hands the source to an external compiler, dlopens the resulting `.so`, and
//! dispatches the loaded function in place of per-chunk Cranelift.  Async mode
//! publishes the compiled module via `OnceLock`; `try_dispatch` returns
//! `NotReady` until then.  See [`emit`] for the pipeline.

pub(crate) mod emit;

use crate::backend::{Backend, CompileCtx, CompiledWhole, DispatchOutcome};
use crate::ir::{Event, ProtoStatement};
use std::sync::Arc;

/// Turn chunk-local comb localization off for this process.
///
/// Localization keeps a comb signal in a C local and leaves its `comb_values`
/// word holding whatever it last held.  Nothing outside the chunk reads it,
/// so the simulation is unaffected and the dataflow CHECK skips those
/// bytes — but a waveform dump shows them, and then disagrees with a run that
/// took the Cranelift or interpreted path.  Waveform dumping is the caller.
///
/// Latches: once off, localization never comes back on.  Must be called
/// before analysis, since the blocklist is computed during conv.
pub fn force_disable_localize() {
    emit::force_disable_localize();
}

/// Hands the AOT-C compiles still queued to a detached runner; call at exit.
pub fn detach_pending_compiles() {
    emit::detach_pending_compiles();
}

pub struct AotCBackend {
    async_mode: bool,
    /// When false, no event is compiled; the dataflow comb settle, which the
    /// module builder emits directly, is unaffected.
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

    fn compile_whole_event(
        &mut self,
        ctx: &CompileCtx,
        event: &Event,
        stmts: &[ProtoStatement],
        gates: &[crate::ir::opt::event_gate::EventGate],
    ) -> Option<Arc<dyn CompiledWhole>> {
        if ctx.use_4state || !self.event_enabled {
            return None;
        }
        let (cell, dataflow_marked, dataflow_ff_marked) = emit::prepare_event(
            stmts,
            self.async_mode,
            matches!(event, Event::Clock(..)),
            gates,
        )?;
        Some(Arc::new(AotCWhole {
            cell,
            dataflow_marked,
            dataflow_ff_marked,
        }))
    }
}

struct AotCWhole {
    cell: emit::AotCell,
    /// See `CompiledWhole::dataflow_marked`.
    dataflow_marked: bool,
    /// See `CompiledWhole::dataflow_ff_marked`.
    dataflow_ff_marked: bool,
}

impl CompiledWhole for AotCWhole {
    fn try_dispatch_delta(
        &self,
        ff: *const u8,
        comb: *mut u8,
        log: *mut u8,
        delta: isize,
    ) -> DispatchOutcome {
        match self.cell.get() {
            Some(m) => {
                // SAFETY: as `try_dispatch`; a marked event reads `delta` as
                // the dirty words, the others ignore it.
                unsafe {
                    (m.func)(ff, comb as *const u8, log, delta);
                }
                DispatchOutcome::Done
            }
            None => DispatchOutcome::NotReady,
        }
    }

    fn dataflow_marked(&self) -> bool {
        self.dataflow_marked
    }

    fn dataflow_ff_marked(&self) -> bool {
        self.dataflow_ff_marked
    }

    fn try_dispatch(&self, ff: *const u8, comb: *mut u8, log: *mut u8) -> DispatchOutcome {
        match self.cell.get() {
            Some(m) => {
                // SAFETY: caller provides pointers valid for the
                // function's reads/writes; emitted code follows the
                // standard JIT ABI (ff, comb, write_log).
                unsafe {
                    // Whole-event functions are never relocated: ff_delta = 0.
                    (m.func)(ff, comb as *const u8, log, 0);
                }
                DispatchOutcome::Done
            }
            None => DispatchOutcome::NotReady,
        }
    }
}
