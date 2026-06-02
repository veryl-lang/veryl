pub(crate) mod context;
pub(crate) mod declaration;
mod event;
mod expression;
pub(crate) mod inst_layout;
mod module;
pub(crate) mod opt;
pub(crate) mod site_table;
mod statement;
pub(crate) mod variable;
pub(crate) mod write_log;

pub use context::{Context, Conv};
pub use declaration::ProtoDeclaration;
pub use event::Event;
pub use expression::{Expression, ExpressionContext, ProtoDynamicBitSelect, ProtoExpression};
pub use module::{Module, ProtoModule};
pub use statement::{
    CompiledBatchStmt, CompiledBlockStatement, CompiledStmt, ProtoAssignDynamicStatement,
    ProtoAssignStatement, ProtoForBound, ProtoForRange, ProtoForStatement, ProtoIfStatement,
    ProtoStatement, ProtoStatementBlock, ProtoStatements, ProtoSystemFunctionCall, RuntimeForBound,
    RuntimeForRange, Statement, SystemFunctionCall, TbMethodKind, format_assert_message,
    parse_hex_content, patch_stmt_log_buf, veryl_aot_sysfn_print,
};
pub use variable::{
    ModuleVariableMeta, ModuleVariables, VarOffset, Variable, VariableElement, VariableMeta,
    create_variable_meta, native_bytes, read_native_value, read_payload, value_size,
    write_native_value, write_payload,
};
pub use veryl_analyzer::ir::{Op, Type, VarId, VarPath};
pub use veryl_analyzer::value::Value;

use crate::HashMap;
use crate::backend::{self, BackendRegistry, CompiledWhole, DispatchOutcome};
use crate::simulator::SimProfile;
use crate::simulator_error::SimulatorError;
use std::sync::Arc;
use std::sync::OnceLock;
use veryl_analyzer::ir as air;
use veryl_analyzer::value::MaskCache;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

pub struct Ir {
    pub name: StrId,
    pub token: TokenRange,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_values: Box<[u8]>,
    pub comb_values: Box<[u8]>,
    pub use_4state: bool,
    pub module_variables: ModuleVariables,
    pub event_statements: HashMap<Event, Vec<Statement>>,
    /// Unified comb statements: all port connections, child comb, and internal
    /// comb combined into a single dependency-sorted list.
    pub comb_statements: Vec<Statement>,
    /// Number of eval_comb passes needed for full convergence.
    /// Pre-computed from backward edges in the sorted comb statement list.
    pub required_comb_passes: usize,
    /// FF write site table: compile-time metadata for each FF write site,
    /// built at ProtoModule conv time.  Consumed by phases that need to
    /// reason about FF writes statically (write-log buffer sizing, NBA
    /// invariant checks, per-Inst metadata for MT-ready commit).
    pub site_table: site_table::SiteTable,
    /// Per-top-level-Inst FF byte range metadata.  Foundation for
    /// cache-line aligned padding and per-Inst independent commit.
    pub inst_layout: inst_layout::InstLayout,
    /// FF write log buffer.  Sized at Ir construction time from
    /// `site_table.len()`; FF writes (JIT + interpret) push entries
    /// during event evaluation and `ff_commit_from_log` applies them
    /// at cycle end.
    ///
    /// Heap-allocated (`Box`) so the buffer's address is stable across
    /// moves of the surrounding `Ir`/`Simulator` — JIT code holds a raw
    /// pointer baked into each `Statement::Compiled` at construction.
    pub write_log_buffer: Box<write_log::WriteLogBuffer>,
    /// Whether FF classification optimization is disabled.
    pub disable_ff_opt: bool,
    /// Diagnostic: number of nontrivial SCCs found in the pre-JIT comb
    /// graph.  Real combinational loops are rejected by `analyze_dependency`,
    /// so any non-zero value here indicates duplicate ProtoStatements in
    /// the simulator IR assembly.  See `Module::nontrivial_comb_scc`.
    pub nontrivial_comb_scc: usize,
    /// Whole-comb dispatch handle.  `Some` when a backend (today:
    /// AOT-C) committed to a one-function compile via
    /// `Backend::compile_whole_comb`; `settle_comb` invokes its
    /// `try_dispatch` in place of per-chunk Cranelift.  `None` keeps
    /// the per-chunk loop.
    pub whole_comb: Option<Arc<dyn CompiledWhole>>,
    /// Snapshotted from `Config::aot_c_validate`: when set, `settle_comb` /
    /// `step` dual-run the AOT-C and Cranelift paths and panic on divergence.
    pub aot_c_validate: bool,
    /// Per-event whole-event dispatch handles.  When the current
    /// event's `try_dispatch` succeeds, `step()` invokes it instead of
    /// the per-stmt Cranelift dispatch.  Built in `ProtoModule::conv`
    /// when `Config::aot_c_event` is set and the emitter covered every
    /// event stmt.
    pub whole_events: HashMap<Event, Arc<dyn CompiledWhole>>,
}

impl Ir {
    pub fn from_module(module: Module, config: &Config, token: TokenRange) -> Ir {
        let mut ir = Ir {
            name: module.name,
            token,
            ports: module.ports,
            ff_values: module.ff_values,
            comb_values: module.comb_values,
            use_4state: config.use_4state,
            module_variables: module.module_variables,
            event_statements: module.event_statements,
            comb_statements: module.comb_statements,
            required_comb_passes: module.required_comb_passes,
            write_log_buffer: {
                let (narrow_cap, wide_cap) = write_log_capacity(&module.site_table);
                Box::new(write_log::WriteLogBuffer::with_capacity(
                    narrow_cap, wide_cap,
                ))
            },
            site_table: module.site_table,
            inst_layout: module.inst_layout,
            disable_ff_opt: config.disable_ff_opt,
            nontrivial_comb_scc: module.nontrivial_comb_scc,
            whole_comb: module.whole_comb,
            aot_c_validate: config.aot_c_validate,
            whole_events: module.whole_events,
        };
        // Bake the WriteLogBuffer's heap-stable address into every
        // JIT-dispatched Compiled/CompiledBatch so emitted code can perform
        // inline log pushes without a TLS lookup.
        ir.install_write_log_ptr();
        ir
    }

    /// Walk every event/comb statement tree and overwrite the placeholder
    /// `log_buf` field in `Statement::Compiled` / `Statement::CompiledBatch`
    /// with the actual heap address of `self.write_log_buffer`.
    ///
    /// Called once at the end of `from_module`.  The address is stable
    /// for `self`'s lifetime because the buffer lives on the heap
    /// inside a `Box`.
    fn install_write_log_ptr(&mut self) {
        let log_buf =
            (&*self.write_log_buffer) as *const _ as *mut write_log::WriteLogBuffer as *mut u8;
        for stmts in self.event_statements.values_mut() {
            for s in stmts {
                patch_stmt_log_buf(s, log_buf);
            }
        }
        for s in &mut self.comb_statements {
            patch_stmt_log_buf(s, log_buf);
        }
    }

    /// Evaluate comb for `required_comb_passes` passes.
    ///
    /// Real combinational loops are rejected by `analyze_dependency`
    /// (error: `combinational_loop`), so once control reaches this function
    /// the stmt-level graph is an acyclic DAG whose depth determines how
    /// many passes are needed to settle.  No iteration-to-convergence is
    /// required, and no runtime "did anything change?" check is performed.
    pub fn settle_comb(&self, mask_cache: &mut MaskCache, profile: &mut SimProfile) {
        #[cfg(feature = "profile")]
        {
            profile.settle_comb_count += 1;
        }
        let _ = profile; // suppress unused warning when profile feature is off

        // Dispatch: when a whole-comb backend (today: AOT-C) is ready,
        // invoke it in place of per-chunk Cranelift dispatch.  When
        // VERYL_AOT_C_VALIDATE=1 (`self.aot_c_validate`) we additionally
        // dual-run the whole-comb and the per-chunk path and panic on
        // first divergence.  Both paths fall through to Cranelift if
        // the whole-comb backend declines (`whole_comb == None`) or
        // returns `NotReady` (async compile pending).
        if let Some(whole) = self.whole_comb.as_ref() {
            // Cache env var lookups in a process-static OnceLock: settle_comb
            // runs once per cycle, so a per-cycle `std::env::var`/getenv would
            // be a hot-path cost.
            static AOT_C_PASSES_OVERRIDE: OnceLock<Option<usize>> = OnceLock::new();
            let validate = self.aot_c_validate;
            let env_passes = *AOT_C_PASSES_OVERRIDE.get_or_init(|| {
                std::env::var("VERYL_AOT_C_PASSES")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
            });
            let ff_ptr = self.ff_values.as_ptr();
            let comb_ptr = self.comb_values.as_ptr() as *mut u8;
            // AOT-C comb eval never writes the log (the emitted C does
            // `(void)write_log`), so the pointer is unused.  Pass the real
            // heap-stable buffer address anyway to satisfy the FuncPtr
            // contract (3rd arg is `*mut u8`).
            let log_ptr = (&*self.write_log_buffer as *const _ as *const u8) as *mut u8;
            let passes = env_passes.unwrap_or(self.required_comb_passes).max(1);

            if !validate {
                // Common case: passes == 1 (no SCC backward edges).
                for _ in 0..passes {
                    match whole.try_dispatch(ff_ptr, comb_ptr, log_ptr) {
                        DispatchOutcome::Done => {}
                        DispatchOutcome::NotReady => {
                            // Async compile not finished yet — drop to
                            // Cranelift for this cycle.
                            self.run_chunked_settle(mask_cache, profile);
                            return;
                        }
                    }
                }
                return;
            }

            // Validate path: delegate to backend::validate, which
            // snapshots inputs, runs whole-comb, restores, runs
            // Cranelift, and diffs.  Panics on divergence.
            backend::validate::settle_comb(self, whole.as_ref(), passes, mask_cache, profile);
            return;
        }

        self.run_chunked_settle(mask_cache, profile);
    }

    /// Cranelift-only settle path, factored out so the validate mode can
    /// invoke it after AOT-C eval has run and the buffers have been restored.
    pub(crate) fn run_chunked_settle(&self, mask_cache: &mut MaskCache, profile: &mut SimProfile) {
        let _ = profile;

        // `VERYL_MIN_PASSES_OVERRIDE` is still honoured as a debug knob.
        static MIN_PASSES_OVERRIDE: OnceLock<Option<usize>> = OnceLock::new();
        let min_override = *MIN_PASSES_OVERRIDE.get_or_init(|| {
            std::env::var("VERYL_MIN_PASSES_OVERRIDE")
                .ok()
                .and_then(|s| s.parse().ok())
        });
        let passes = min_override.unwrap_or(self.required_comb_passes);
        for _ in 0..passes {
            self.eval_comb_full(mask_cache, profile);
            #[cfg(feature = "profile")]
            {
                profile.comb_eval_count += 1;
            }
        }
    }

    /// Evaluate unified comb once.
    /// Called by settle_comb() for each required pass.
    pub fn eval_comb_full(&self, mask_cache: &mut MaskCache, profile: &mut SimProfile) {
        let _ = profile;
        #[cfg(feature = "profile")]
        let start = std::time::Instant::now();

        for x in &self.comb_statements {
            dispatch_stmt_fast(x, mask_cache);
        }

        #[cfg(feature = "profile")]
        {
            profile.eval_comb_full_ns += start.elapsed().as_nanos() as u64;
        }
    }

    /// Number of statements in comb_statements (for profiling).
    pub fn comb_stmt_count(&self) -> (usize, usize, usize) {
        let mut binary = 0;
        let mut interp = 0;
        let mut total = 0;
        for s in &self.comb_statements {
            total += 1;
            if s.is_compiled() {
                binary += 1;
            } else {
                interp += 1;
            }
        }
        (total, binary, interp)
    }

    pub fn dump_variables(&self) -> String {
        format!("{}", self.module_variables)
    }

    /// Returns (jit_count, total_count) of top-level statements across all events and comb.
    pub fn jit_stats(&self) -> (usize, usize) {
        let mut jit = 0;
        let mut total = 0;
        for stmts in self.event_statements.values() {
            for s in stmts {
                total += 1;
                if s.is_compiled() {
                    jit += 1;
                }
            }
        }
        for s in &self.comb_statements {
            total += 1;
            if s.is_compiled() {
                jit += 1;
            }
        }
        (jit, total)
    }

    /// Returns detailed stats: (comb_jit, comb_interp, event_jit, event_interp)
    pub fn detailed_stats(&self) -> (usize, usize, usize, usize) {
        let mut comb_jit = 0;
        let mut comb_interp = 0;
        let mut event_jit = 0;
        let mut event_interp = 0;
        for s in &self.comb_statements {
            if s.is_compiled() {
                comb_jit += 1;
            } else {
                comb_interp += 1;
            }
        }
        for stmts in self.event_statements.values() {
            for s in stmts {
                if s.is_compiled() {
                    event_jit += 1;
                } else {
                    event_interp += 1;
                }
            }
        }
        (comb_jit, comb_interp, event_jit, event_interp)
    }
}

/// Inline-friendly dispatch for the per-cycle hot loop.  Handles the
/// common JIT cases (Compiled / CompiledBatch) with a direct indirect call
/// and falls back to `Statement::eval_step` for the interpreter path.
///
/// Inlining at the call site removes the (otherwise non-inlined)
/// `Statement::eval_step` function-call frame plus the 10-arm match
/// jump it performs.
#[inline(always)]
pub fn dispatch_stmt_fast(s: &Statement, mask_cache: &mut MaskCache) {
    match s {
        Statement::Compiled(c) => unsafe {
            (c.artifact.func)(c.ff, c.comb, c.log_buf);
        },
        Statement::CompiledBatch(c) => unsafe {
            let f = c.artifact.func;
            for &(ff, comb) in &c.args {
                f(ff, comb, c.log_buf);
            }
        },
        _ => {
            s.eval_step(mask_cache);
        }
    }
}

// SAFETY: Each Ir exclusively owns its ff_values/comb_values buffers.
// Raw pointers in Statements point into these buffers — no cross-Ir aliasing.
// `Arc<ChunkArtifact>` handles inside `Statement::Compiled` / `CompiledBlockStatement`
// keep JIT code pages alive (via the artifact's keepalive field).
// NOTE: Ir is intentionally NOT Sync. Sharing &Ir across threads would allow
// concurrent mutation of ff_values/comb_values via interior raw pointers.
unsafe impl Send for Ir {}

/// Initial WriteLogBuffer capacities derived from the FF write site table.
/// Returns `(narrow_cap, wide_cap)`.  Narrow FFs (`native_bytes ≤ 8`) emit
/// at most 2 entries per cycle (payload + 4-state mask); wide FFs emit at
/// most 2 wide entries per cycle (one per payload/mask).  Each contributes
/// to its respective pool, with a ×2 over-provisioning headroom for
/// initial dual-writes and multi-RMW chains.
fn write_log_capacity(site_table: &site_table::SiteTable) -> (usize, usize) {
    let mut narrow: usize = 0;
    let mut wide: usize = 0;
    let mut any_wide = false;
    for s in &site_table.sites {
        let nb = s.native_bytes as usize;
        if nb <= 8 {
            narrow += 2 * 2;
        } else {
            any_wide = true;
            // Number of wide entries needed (≤56 byte payload per entry).
            let chunks = nb.div_ceil(write_log::WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES);
            wide += 2 * chunks * 2;
        }
    }
    // Narrow floor avoids tiny designs ending up with zero capacity; the
    // wide pool stays empty when no wide sites exist so designs that only
    // use narrow FFs skip the 64-byte-aligned wide allocation altogether.
    let narrow_cap = narrow.max(4096);
    let wide_cap = if any_wide { wide.max(64) } else { 0 };
    (narrow_cap, wide_cap)
}

pub fn build_ir(ir: &air::Ir, top: StrId, config: &Config) -> Result<Ir, SimulatorError> {
    for x in &ir.components {
        if let air::Component::Module(x) = x
            && top == x.name
        {
            let token = x.token;
            let mut context = context::Context {
                config: config.clone(),
                backends: BackendRegistry::for_config(config),
                ..Default::default()
            };
            let proto: ProtoModule = Conv::conv(&mut context, x)?;
            let module = proto.instantiate();
            return Ok(Ir::from_module(module, config, token));
        }
    }
    Err(SimulatorError::TopModuleNotFound {
        module_name: top.to_string(),
    })
}

struct CacheEntry {
    proto: ProtoModule,
    token: TokenRange,
}

/// Cache for `ProtoModule` keyed by top module name.  JIT binaries are
/// kept alive via shared `Arc<ChunkArtifact>` handles embedded in the
/// cached `ProtoModule`'s `CompiledBlock` statements, so the cache no
/// longer needs a separate keepalive vector.
#[derive(Default)]
pub struct ProtoModuleCache {
    entries: HashMap<StrId, CacheEntry>,
}

pub fn build_ir_cached(
    ir: &air::Ir,
    top: StrId,
    config: &Config,
    cache: &mut ProtoModuleCache,
) -> Result<Ir, SimulatorError> {
    // Cache hit: reuse ProtoModule, just instantiate with fresh buffers
    if let Some(entry) = cache.entries.get(&top) {
        let module = entry.proto.instantiate();
        return Ok(Ir::from_module(module, config, entry.token));
    }

    // Cache miss: run Conv::conv
    for x in &ir.components {
        if let air::Component::Module(x) = x
            && top == x.name
        {
            let token = x.token;
            let mut context = context::Context {
                config: config.clone(),
                backends: BackendRegistry::for_config(config),
                ..Default::default()
            };

            let proto: ProtoModule = Conv::conv(&mut context, x)?;
            let module = proto.instantiate();

            let result = Ir::from_module(module, config, token);

            cache.entries.insert(top, CacheEntry { proto, token });

            return Ok(result);
        }
    }
    Err(SimulatorError::TopModuleNotFound {
        module_name: top.to_string(),
    })
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub use_4state: bool,
    pub use_jit: bool,
    pub dump_cranelift: bool,
    pub dump_asm: bool,
    /// Force all always_ff variables to FF (disable is_ff refinement).
    pub disable_ff_opt: bool,
    /// `cc` backend: emit comb as C, compile externally, and dispatch the
    /// `.so` instead of the Cranelift loop (which still covers stmts it can't
    /// emit, so keep `use_jit` true).  Default false; `--backend cc` enables it.
    pub aot_c: bool,
    /// `cc` backend event path: also emit the per-event FF-next + write-log.
    /// Requires `aot_c`.
    pub aot_c_event: bool,
    /// Compile the `.so` on a background thread and hot-swap from Cranelift
    /// once ready, hiding the cold compile latency.  Requires `aot_c`; forced
    /// off under `aot_c_validate` (validation must dual-run from cycle 0).
    pub aot_c_async: bool,
    /// Dual-run `cc` and Cranelift every cycle, panicking on the first
    /// divergence (correctness check).  Implies a synchronous compile.
    pub aot_c_validate: bool,
    /// Minimum module statement count (comb + event) before `cc` is attempted;
    /// below it the module stays on per-chunk Cranelift.  Default 0 (no floor)
    /// now that the compile pool caps concurrency; set `VERYL_AOT_C_MIN_STMTS=N`
    /// to restore a floor.
    pub aot_c_min_stmts: usize,
}

impl Config {
    /// Apply environment-variable overrides on top of an existing config.
    pub fn apply_env(&mut self) {
        if std::env::var("VERYL_DUMP_ASM").ok().as_deref() == Some("1") {
            self.dump_asm = true;
        }
        if std::env::var("VERYL_DUMP_CRANELIFT").ok().as_deref() == Some("1") {
            self.dump_cranelift = true;
        }
        // AOT-C ("cc" backend) env overrides.  The CLI `--backend` is the
        // primary control; these let callers force a sub-feature on/off (e.g.
        // bisect a divergence, or disable async for a deterministic profile)
        // without a flag.  `=1` enables, `=0` disables; anything else leaves
        // the value untouched.
        let env_bool = |k: &str| match std::env::var(k).ok().as_deref() {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        };
        if let Some(v) = env_bool("VERYL_AOT_C") {
            self.aot_c = v;
        }
        if let Some(v) = env_bool("VERYL_AOT_C_EVENT") {
            self.aot_c_event = v;
        }
        if let Some(v) = env_bool("VERYL_AOT_C_ASYNC") {
            self.aot_c_async = v;
        }
        if let Some(v) = env_bool("VERYL_AOT_C_VALIDATE") {
            self.aot_c_validate = v;
        }
        if let Ok(n) = std::env::var("VERYL_AOT_C_MIN_STMTS")
            && let Ok(n) = n.parse::<usize>()
        {
            self.aot_c_min_stmts = n;
        }
    }
}

// `cc_available()` has moved to `crate::backend::aot_c`.

impl Config {
    pub fn all() -> Vec<Config> {
        let mut ret = vec![];

        // `use_jit = true` is meaningful only when the Cranelift backend
        // is built in; wasm has no chunk backend, so dropping the `true`
        // arm is purely an optimization (Config::default() already sets
        // use_jit = false).
        let jit_options: &[bool] = if cfg!(target_family = "wasm") {
            &[false]
        } else {
            &[false, true]
        };

        for use_4state in [false, true] {
            for &use_jit in jit_options {
                for disable_ff_opt in [false, true] {
                    ret.push(Config {
                        use_4state,
                        use_jit,
                        disable_ff_opt,
                        ..Default::default()
                    });
                }
            }
        }

        // `cc` backend variants: 2-state only, Cranelift fallback for uncovered
        // stmts (use_jit stays true).  Sync compile — async's swap point varies
        // with timing, but tests must dual-check cc deterministically vs the
        // golden output.  Gated on cc_available so cc-less hosts still run.
        #[cfg(not(target_family = "wasm"))]
        if backend::aot_c::cc_available() {
            for disable_ff_opt in [false, true] {
                ret.push(Config {
                    use_4state: false,
                    use_jit: true,
                    disable_ff_opt,
                    aot_c: true,
                    aot_c_event: true,
                    aot_c_async: false,
                    ..Default::default()
                });
            }
        }

        ret
    }
}

// `lookup_comb_offset` has moved to `backend::validate`.
