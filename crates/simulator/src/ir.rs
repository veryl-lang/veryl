pub(crate) mod big_array;
pub(crate) mod comb_layout;
pub(crate) mod comb_pipeline_cache;
pub(crate) mod context;
pub(crate) mod declaration;
pub mod deps;
pub mod derived_clock;
mod event;
mod expression;
pub(crate) mod external;
pub(crate) mod hier_ref;
pub(crate) mod inst_layout;
pub(crate) mod module;
pub(crate) mod opt;
pub(crate) mod partial_index;
pub(crate) mod site_table;
mod statement;
pub(crate) mod variable;
pub(crate) mod write_log;

pub use big_array::BigArrayFold;
pub use context::{Context, Conv};
pub use declaration::ProtoDeclaration;
pub use derived_clock::{DerivedClock, DerivedClockSchedule, DerivedReset, EdgeCandidate};
pub use event::Event;
pub use expression::{Expression, ExpressionContext, ProtoDynamicBitSelect, ProtoExpression};
pub use external::{
    ExternalComponentInst, ExternalConnectInst, ProtoExternalComponent, ProtoExternalConnect,
};
pub use module::{Module, ProtoModule};
pub use opt::comb_fusion::force_disable as force_disable_comb_fusion;
pub use opt::dead_var_dce::force_disable as force_disable_dead_var_dce;
pub use opt::field_unfuse::force_disable as force_disable_field_unfuse;
pub use statement::{
    CompiledBatchStmt, CompiledBlockStatement, CompiledStmt, ComponentArg,
    ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoCaseArm, ProtoCaseStatement,
    ProtoComponentArg, ProtoForBound, ProtoForRange, ProtoForStatement, ProtoIfStatement,
    ProtoStatement, ProtoStatementBlock, ProtoStatements, ProtoSystemFunctionCall, RetWidthCheck,
    RuntimeForBound, RuntimeForRange, Statement, SystemFunctionCall, TbMethodKind,
    format_assert_message, format_output, parse_hex_content, patch_stmt_log_buf,
    veryl_aot_sysfn_print,
};
pub use variable::{
    ModuleVariableMeta, ModuleVariables, VarOffset, Variable, VariableElement, VariableMeta,
    create_variable_meta, native_bytes, read_native_value, read_payload, value_size,
    write_native_value, write_payload,
};
pub use veryl_analyzer::ir::{Op, Type, VarId, VarPath};
pub use veryl_analyzer::value::Value;

use crate::backend::{self, BackendRegistry, CompiledWhole, DispatchOutcome};
use crate::residency;
use crate::simulator::SimProfile;
use crate::simulator_error::SimulatorError;
use crate::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

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
    /// See `Config::abstract_reset_active_high`.
    pub abstract_reset_active_high: bool,
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
    /// Derived (gated / FF-divided) clocks in this module; empty when none.
    pub derived_clock_schedule: DerivedClockSchedule,
    /// JIT-compiled evaluation of the derived-clock dependency closure,
    /// run by `partial_settle` independently of `comb_statements`.
    pub derived_clock_eval_stmts: Vec<Statement>,
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
    /// Snapshotted from `Config::aot_c_validate_stride`: dual-run only every
    /// Nth settle (0/1 = every cycle).
    pub aot_c_validate_stride: u64,
    /// Per-event whole-event dispatch handles.  When the current
    /// event's `try_dispatch` succeeds, `step()` invokes it instead of
    /// the per-stmt Cranelift dispatch.  Built in `ProtoModule::conv`
    /// when `Config::aot_c_event` is set and the emitter covered every
    /// event stmt.
    pub whole_events: HashMap<Event, Arc<dyn CompiledWhole>>,
    /// User-defined component instances (`$comp::<name>`), fired by
    /// the simulator around event evaluation.
    pub external_components: Vec<ExternalComponentInst>,
    /// Base seed for per-instance component seeds (from `[test] seed` or
    /// `--seed`).
    pub seed: u64,
    /// Resolved component libraries, keyed by export name. Missing
    /// entries fall back to the in-process static registry.
    pub component_libraries: std::collections::HashMap<String, ComponentLibrary>,
    /// Base directory for component file I/O (the project root). Relative
    /// reads resolve against it; relative writes go to a per-test output
    /// directory beneath it. `None` leaves paths process-CWD relative.
    pub component_file_base: Option<PathBuf>,
    /// See `Module::rtl_driven`.
    pub rtl_driven: HashSet<VarId>,
    /// See `Module::comb_touched_offsets`.  Consumed by the testbench's
    /// comb-dirty filter (`tb_dirty::TbDirtyFilter`).
    pub comb_touched_offsets: std::sync::Arc<crate::HashSet<crate::ir::VarOffset>>,
    /// See `Module::event_comb_writes`.  Consumed by the simulator's
    /// settle filter: a fire of an event whose writes can reach a comb
    /// read dirties the comb.
    pub event_comb_writes: HashMap<Event, Option<Vec<(isize, isize)>>>,
    /// See `Module::cone_state_base`.
    pub cone_state_base: u32,
    /// See `Module::settle_info`.
    pub(crate) settle_info: crate::tb_dirty::SettleInfoCache,
    /// Cone-gate segments over `comb_statements`; empty when ungated.
    /// Runtime shadows live in `cone_gate_state`.
    pub cone_segments: Vec<crate::ir::opt::cone_gate::RtSegment>,
    /// Lazily initialised per-segment shadows + auto-off counters.
    pub cone_gate_state: std::cell::RefCell<Option<crate::ir::opt::cone_gate::ConeGateState>>,
    /// See `Module::fused_comb_offsets` (diagnostic; consumed by the
    /// dual-run checker to skip storage the fusion pass retired).
    pub fused_comb_offsets: Vec<isize>,
    /// A failed compile leaves the cell empty forever, so the fallback is
    /// taken every cycle; the residency table (a mutex) must be touched once.
    whole_comb_fallback_recorded: AtomicBool,
    pub(crate) whole_event_fallback_recorded: AtomicBool,
    /// Whether the whole-comb backend's run-once constant-cone entry has
    /// executed for THIS instance.  Per-instance (not per-artifact): a
    /// shared `.so` serves many simulators, each with fresh comb buffers.
    pub(crate) const_cone_done: AtomicBool,
}

/// A built component library on disk and the type name to look up in it.
#[derive(Clone, Debug)]
pub struct ComponentLibrary {
    pub path: PathBuf,
    pub type_name: String,
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
            abstract_reset_active_high: config.abstract_reset_active_high,
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
            derived_clock_schedule: module.derived_clock_schedule,
            derived_clock_eval_stmts: module.derived_clock_eval_stmts,
            nontrivial_comb_scc: module.nontrivial_comb_scc,
            whole_comb: module.whole_comb,
            aot_c_validate: config.aot_c_validate,
            aot_c_validate_stride: config.aot_c_validate_stride,
            whole_events: module.whole_events,
            external_components: module.external_components,
            seed: config.seed,
            component_libraries: config.component_libraries.clone(),
            component_file_base: config.component_file_base.clone(),
            rtl_driven: module.rtl_driven,
            fused_comb_offsets: module.fused_comb_offsets,
            comb_touched_offsets: module.comb_touched_offsets,
            event_comb_writes: module.event_comb_writes,
            cone_state_base: module.cone_state_base,
            settle_info: module.settle_info,
            cone_segments: module.cone_segments,
            cone_gate_state: std::cell::RefCell::new(None),
            whole_comb_fallback_recorded: Default::default(),
            whole_event_fallback_recorded: Default::default(),
            const_cone_done: Default::default(),
        };
        // Bake the WriteLogBuffer's heap-stable address into every
        // JIT-dispatched Compiled/CompiledBatch so emitted code can perform
        // inline log pushes without a TLS lookup.
        ir.install_write_log_ptr();
        ir.backend_diag();
        if env::var("VERYL_DUMP_VARMAP").ok().as_deref() == Some("1") {
            ir.dump_varmap();
        }
        ir
    }

    /// True when the reset net `id` asserts LOW.  The polarity-agnostic
    /// `reset` type carries none of its own, so a declaration on the ports the
    /// net reaches decides — their `if_reset` blocks were lowered against it —
    /// and `[build] reset_type` is the fallback when none does.
    pub fn reset_active_low(&self, id: &VarId) -> bool {
        let var = self.module_variables.variables.get(id);
        match var.map(|x| &x.r#type.kind) {
            Some(air::TypeKind::ResetAsyncHigh) | Some(air::TypeKind::ResetSyncHigh) => false,
            Some(air::TypeKind::ResetAsyncLow) | Some(air::TypeKind::ResetSyncLow) => true,
            _ => var
                .and_then(|v| self.declared_reset_polarity(v))
                .unwrap_or(!self.abstract_reset_active_high),
        }
    }

    /// The polarity declared for the net `var` denotes, found through the
    /// storage it shares with connected ports.  `None` when nothing declares
    /// one or the declarations disagree — neither leaves a level to pick.
    fn declared_reset_polarity(&self, var: &Variable) -> Option<bool> {
        let &ptr = var.current_values.first()?;
        let mut found: Option<bool> = None;
        let mut stack = vec![&self.module_variables];
        while let Some(vars) = stack.pop() {
            for other in vars.variables.values() {
                if other.current_values.first() != Some(&ptr) {
                    continue;
                }
                let active_low = match other.r#type.kind {
                    air::TypeKind::ResetAsyncHigh | air::TypeKind::ResetSyncHigh => false,
                    air::TypeKind::ResetAsyncLow | air::TypeKind::ResetSyncLow => true,
                    _ => continue,
                };
                match found {
                    None => found = Some(active_low),
                    Some(prev) if prev != active_low => return None,
                    Some(_) => {}
                }
            }
            for child in &vars.children {
                stack.push(child);
            }
        }
        found
    }

    /// Reset-typed ports of the top module — the nets an external driver
    /// supplies.  Sorted by path so a caller picking one is deterministic.
    pub fn reset_ports(&self) -> Vec<VarId> {
        let mut ports: Vec<(&VarPath, &VarId)> = self
            .ports
            .iter()
            .filter(|(_, id)| {
                self.module_variables
                    .variables
                    .get(*id)
                    .is_some_and(|x| x.r#type.is_reset())
            })
            .collect();
        ports.sort_by(|a, b| a.0.cmp(b.0));
        ports.into_iter().map(|(_, id)| *id).collect()
    }

    /// `VERYL_DUMP_VARMAP=1`: every variable element's storage offset with its
    /// hierarchical path — the table an emitted-code offset is joined against
    /// to name the signal behind it.
    fn dump_varmap(&self) {
        // Millions of lines on a large design, and stderr is unbuffered: hold
        // one buffer for the whole dump.
        use std::io::Write;
        let stderr = std::io::stderr();
        let mut out = std::io::BufWriter::new(stderr.lock());
        let comb_base = self.comb_values.as_ptr() as usize;
        let comb_end = comb_base + self.comb_values.len();
        let ff_base = self.ff_values.as_ptr() as usize;
        let ff_end = ff_base + self.ff_values.len();
        let mut stack = vec![(String::new(), &self.module_variables)];
        while let Some((prefix, m)) = stack.pop() {
            let here = if prefix.is_empty() {
                m.name.to_string()
            } else {
                format!("{prefix}.{}", m.name)
            };
            for var in m.variables.values() {
                for (i, &p) in var.current_values.iter().enumerate() {
                    let p = p as usize;
                    let (kind, off) = if (comb_base..comb_end).contains(&p) {
                        ("comb", p - comb_base)
                    } else if (ff_base..ff_end).contains(&p) {
                        ("ff", p - ff_base)
                    } else {
                        continue; // external component storage
                    };
                    let _ = writeln!(
                        out,
                        "[varmap] {kind} {off:#x} w={} {here}.{}[{i}]",
                        var.width, var.path
                    );
                }
            }
            stack.extend(m.children.iter().map(|c| (here.clone(), c)));
        }
    }

    /// `VERYL_BACKEND_DIAG=1`: report per-event/comb jit vs interpreter counts
    /// and whether a whole-comb/event backend is dispatched. Chunking is
    /// identical for `cc` and `cranelift` (only dispatch differs), so a `cc`
    /// run also describes Cranelift's interp fallback. Read-only.
    fn backend_diag(&self) {
        if env::var("VERYL_BACKEND_DIAG").as_deref() != Ok("1") {
            return;
        }
        fn classify(s: &Statement) -> String {
            match s {
                Statement::Assign(a) => format!(
                    "Assign dst_width={}{}{}",
                    a.dst_width,
                    if a.select.is_some() { " select" } else { "" },
                    a.dynamic_select
                        .as_ref()
                        .map(|d| format!(
                            " dynsel(elem={} win={} n={} full={})",
                            d.elem_width,
                            d.window,
                            d.num_elements,
                            d.elem_width * d.num_elements
                        ))
                        .unwrap_or_default(),
                ),
                Statement::AssignDynamic(a) => format!(
                    "AssignDynamic dst_width={} n_elem={} full={}{}{}",
                    a.dst_width,
                    a.dst_num_elements,
                    a.dst_width * a.dst_num_elements,
                    if a.select.is_some() { " select" } else { "" },
                    a.dynamic_select
                        .as_ref()
                        .map(|d| format!(
                            " dynsel(elem={} win={} n={} full={})",
                            d.elem_width,
                            d.window,
                            d.num_elements,
                            d.elem_width * d.num_elements
                        ))
                        .unwrap_or_default(),
                ),
                Statement::If(i) => {
                    // Recurse into the bodies so the leaf store that forced the
                    // whole If onto the interpreter is visible.
                    let kids: Vec<String> = i
                        .true_side
                        .iter()
                        .chain(i.false_side.iter())
                        .map(classify)
                        .collect();
                    format!("If{{ {} }}", kids.join("; "))
                }
                Statement::Case(c) => {
                    let kids: Vec<String> = c
                        .arms
                        .iter()
                        .flat_map(|arm| arm.body.iter())
                        .chain(c.default.iter())
                        .map(classify)
                        .collect();
                    format!("Case{{ {} }}", kids.join("; "))
                }
                Statement::For(f) => {
                    let const_range = matches!(f.range.start, statement::RuntimeForBound::Const(_))
                        && matches!(f.range.end, statement::RuntimeForBound::Const(_));
                    let kids: Vec<String> = f.body.iter().map(classify).collect();
                    format!(
                        "For(const_range={}, body_len={}){{ {} }}",
                        const_range,
                        f.body.len(),
                        kids.join("; ")
                    )
                }
                Statement::SystemFunctionCall(_) => "SysFn".to_string(),
                Statement::SequentialBlock(b) => {
                    let kids: Vec<String> = b.iter().map(classify).collect();
                    format!("Seq{{ {} }}", kids.join("; "))
                }
                Statement::TbMethodCall { .. } => "TbMethodCall".to_string(),
                Statement::Break => "Break".to_string(),
                Statement::Compiled(_) | Statement::CompiledBatch(_) => "Compiled".to_string(),
            }
        }
        let report = |label: &str, stmts: &[Statement], whole: bool| {
            let (mut jit, mut interp) = (0usize, 0usize);
            for s in stmts {
                if s.is_compiled() {
                    jit += 1;
                } else if !matches!(s, Statement::Break) {
                    interp += 1;
                }
            }
            eprintln!(
                "  {label}: total={} jit={jit} interp={interp}  whole={whole}",
                stmts.len()
            );
            for s in stmts {
                if !s.is_compiled() && !matches!(s, Statement::Break) {
                    eprintln!("      interp: {}", classify(s));
                }
            }
        };
        eprintln!("=== BackendDiag for {} ===", self.name);
        report("comb", &self.comb_statements, self.whole_comb.is_some());
        let mut events: Vec<_> = self.event_statements.iter().collect();
        events.sort_by_key(|(e, _)| format!("{e:?}"));
        for (event, stmts) in events {
            report(
                &format!("event {event:?}"),
                stmts,
                self.whole_events.contains_key(event),
            );
        }
        eprintln!("==========================");
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
        for s in &mut self.derived_clock_eval_stmts {
            patch_stmt_log_buf(s, log_buf);
        }
    }

    /// Re-evaluate just the derived-clock dependency closure.
    pub fn partial_settle(&self, mask_cache: &mut MaskCache) {
        for stmt in &self.derived_clock_eval_stmts {
            dispatch_stmt_fast(stmt, mask_cache);
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
        let whole_comb = self.whole_comb.as_ref();
        if let Some(whole) = whole_comb {
            // Cache env var lookups in a process-static OnceLock: settle_comb
            // runs once per cycle, so a per-cycle `std::env::var`/getenv would
            // be a hot-path cost.
            static AOT_C_PASSES_OVERRIDE: OnceLock<Option<usize>> = OnceLock::new();
            let validate = self.aot_c_validate;
            let env_passes = *AOT_C_PASSES_OVERRIDE.get_or_init(|| {
                env::var("VERYL_AOT_C_PASSES")
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

            // Run-once constant cone (see `try_dispatch_const`).  NotReady
            // leaves the flag unset — the main dispatch below falls back to
            // Cranelift, which still evaluates the const statements.
            if !self.const_cone_done.load(Ordering::Relaxed)
                && whole.try_dispatch_const(ff_ptr, comb_ptr, log_ptr) == DispatchOutcome::Done
            {
                self.const_cone_done.store(true, Ordering::Relaxed);
            }

            if !validate {
                // Common case: passes == 1 (no SCC backward edges).
                for _ in 0..passes {
                    match whole.try_dispatch(ff_ptr, comb_ptr, log_ptr) {
                        DispatchOutcome::Done => {}
                        DispatchOutcome::NotReady => {
                            // Async compile not finished yet — drop to
                            // Cranelift for this cycle (see `residency`).
                            if !self
                                .whole_comb_fallback_recorded
                                .swap(true, Ordering::Relaxed)
                            {
                                residency::record_fallback("whole_comb", &self.name.to_string());
                            }
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
            env::var("VERYL_MIN_PASSES_OVERRIDE")
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

        if self.cone_segments.is_empty() {
            for x in &self.comb_statements {
                dispatch_stmt_fast(x, mask_cache);
            }
        } else {
            self.eval_comb_cone_gated(mask_cache);
        }

        #[cfg(feature = "profile")]
        {
            profile.eval_comb_full_ns += start.elapsed().as_nanos() as u64;
        }
    }

    /// Settle pass with cone-gate segments: at each gated range, one compare
    /// of its external inputs against the shadow of its last run decides
    /// whether the whole range can be skipped (its outputs still hold the
    /// fixpoint of those same inputs).  See `opt::cone_gate`.
    fn eval_comb_cone_gated(&self, mask_cache: &mut MaskCache) {
        // `VERYL_CONE_GATE_CHECK=1`: run every would-be-skipped segment
        // anyway and panic on the first output byte the skip would have got
        // wrong.  Debug instrument, quadratic in buffer size.
        static CHECK: OnceLock<bool> = OnceLock::new();
        let check = *CHECK.get_or_init(|| env::var("VERYL_CONE_GATE_CHECK").as_deref() == Ok("1"));
        let mut slot = self.cone_gate_state.borrow_mut();
        let state = slot.get_or_insert_with(|| {
            crate::ir::opt::cone_gate::ConeGateState::new(self.cone_segments.len())
        });
        state.tick_rearm();
        // `VERYL_CONE_GATE_DIAG=1`: periodic segment-dispatch statistics.
        static DIAG: OnceLock<bool> = OnceLock::new();
        let diag = *DIAG.get_or_init(|| env::var("VERYL_CONE_GATE_DIAG").as_deref() == Ok("1"));
        if diag {
            let total = state.skipped + state.ran;
            if total >= state.next_report {
                state.next_report = total + (1 << 18);
                eprintln!(
                    "[cone_gate] segment dispatches: skipped {:.1}% ({} of {})",
                    100.0 * state.skipped as f64 / total as f64,
                    state.skipped,
                    total,
                );
                // Every 8th report, the per-segment table.
                if total >= (1 << 21) && (total >> 18).is_multiple_of(8) {
                    for (si, &(sk, rn)) in state.per_seg.iter().enumerate() {
                        if let Some(seg) = self.cone_segments.get(si) {
                            eprintln!(
                                "[cone_gate]   seg{si} [{}..{}) sk={sk} rn={rn} ({:.1}%) {}",
                                seg.lo,
                                seg.hi,
                                100.0 * sk as f64 / (sk + rn).max(1) as f64,
                                seg.cone,
                            );
                        }
                    }
                }
            }
        }
        let n = self.comb_statements.len();
        let mut i = 0usize;
        let mut si = 0usize;
        while i < n {
            if let Some(seg) = self.cone_segments.get(si)
                && seg.lo == i
            {
                if state.check_clean(si, seg, &self.ff_values, &self.comb_values) {
                    if check {
                        // Oracle: a real run starts from the PRE-replay
                        // state (its inputs just compared clean), so run
                        // from that state and require the result to match
                        // what skip+replay produced.  Re-running on the
                        // post-replay buffer instead would feed post-run
                        // values into read-before-write chains and flag
                        // sound skips spuriously.  Diff only the FINAL
                        // state: mid-segment transients (an init store
                        // whose conditional companion overwrites it later
                        // in the segment) are not errors.
                        let pre = self.comb_values.to_vec();
                        if !seg.replay.is_empty() {
                            // SAFETY: the comb buffer outlives the settle
                            // and the spans were bounds-checked at plan
                            // time.
                            unsafe {
                                state.replay(si, seg, self.comb_values.as_ptr() as *mut u8);
                            }
                        }
                        let before = self.comb_values.to_vec();
                        // SAFETY: same buffer, same length.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                pre.as_ptr(),
                                self.comb_values.as_ptr() as *mut u8,
                                pre.len(),
                            );
                        }
                        for x in &self.comb_statements[i..seg.hi] {
                            dispatch_stmt_fast(x, mask_cache);
                        }
                        for (o, (a, b)) in before.iter().zip(self.comb_values.iter()).enumerate() {
                            if a != b {
                                panic!(
                                    "[cone_gate] WRONG SKIP seg {si} [{}..{}) {}: comb {:#x} \
                                     {:#04x} -> {:#04x}\n  compare={:x?}\n  compare_pre={:x?}\n  \
                                     replay={:x?}\n  backedge={:x?}",
                                    seg.lo,
                                    seg.hi,
                                    seg.cone,
                                    o,
                                    a,
                                    b,
                                    seg.compare,
                                    seg.compare_pre,
                                    seg.replay,
                                    seg.backedge,
                                );
                            }
                        }
                    } else if !seg.replay.is_empty() {
                        // SAFETY: the comb buffer outlives the settle and
                        // the spans were bounds-checked at plan time.
                        unsafe {
                            state.replay(si, seg, self.comb_values.as_ptr() as *mut u8);
                        }
                    }
                    i = seg.hi;
                    si += 1;
                    continue;
                }
                state.before_run(si, seg, &self.comb_values);
                for x in &self.comb_statements[i..seg.hi] {
                    dispatch_stmt_fast(x, mask_cache);
                }
                state.refresh(si, seg, &self.ff_values, &self.comb_values);
                i = seg.hi;
                si += 1;
                continue;
            }
            dispatch_stmt_fast(&self.comb_statements[i], mask_cache);
            i += 1;
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

#[inline(always)]
pub fn dispatch_stmt_fast(s: &Statement, mask_cache: &mut MaskCache) {
    match s {
        Statement::Compiled(c) => unsafe {
            (c.artifact.func)(c.ff, c.comb, c.log_buf, c.ff_delta);
        },
        Statement::CompiledBatch(c) => unsafe {
            let f = c.artifact.func;
            for &(ff, comb, ff_delta) in &c.args {
                f(ff, comb, c.log_buf, ff_delta);
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
/// initial dual-writes and multi-RMW chains.  This is only a starting
/// size: all push paths grow the pools on overflow.
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
    /// Polarity the polarity-agnostic `reset` type falls back to, from the
    /// project's `[build] reset_type`.  Declared types carry their own and
    /// ignore this.  Default false = active low, as `ResetType` defaults.
    pub abstract_reset_active_high: bool,
    /// Whether that fallback is SYNCHRONOUS.  Default false = asynchronous.
    pub abstract_reset_sync: bool,
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
    /// Stride for `aot_c_validate`: dual-run + diff only every Nth comb/event
    /// settle (off-stride cycles run Cranelift only — the ground truth that
    /// drives the sim).  0 or 1 = every cycle (full coverage); larger trades
    /// coverage for speed on long tests.
    pub aot_c_validate_stride: u64,
    /// Minimum module statement count (comb + event) before `cc` is attempted;
    /// below it the module stays on per-chunk Cranelift.  Default 0 (no floor)
    /// now that the compile pool caps concurrency; set `VERYL_AOT_C_MIN_STMTS=N`
    /// to restore a floor.
    pub aot_c_min_stmts: usize,
    /// Cross-test DUT reuse: cache a converted DUT and relocate it into later
    /// tests.  The caches are keyed by `Arc<Component>` pointer (unique only
    /// within one `air::Ir`), so it's safe only for the CLI (one analysis per
    /// process), not the parallel unit-test harness — hence default off, enabled
    /// only by `apply_env`.
    pub dut_reuse: bool,
    /// Base seed for user-defined component instances (from `[test] seed`
    /// or `--seed`).
    pub seed: u64,
    /// Component libraries built by `veryl test`, keyed by export name
    /// name. Missing entries fall back to the static registry.
    pub component_libraries: std::collections::HashMap<String, ComponentLibrary>,
    /// See `Ir::component_file_base`.
    pub component_file_base: Option<PathBuf>,
}

impl Config {
    /// Apply environment-variable overrides on top of an existing config.
    pub fn apply_env(&mut self) {
        if env::var("VERYL_DUMP_ASM").ok().as_deref() == Some("1") {
            self.dump_asm = true;
        }
        if env::var("VERYL_DUMP_CRANELIFT").ok().as_deref() == Some("1") {
            self.dump_cranelift = true;
        }
        // AOT-C ("cc" backend) env overrides.  The CLI `--backend` is the
        // primary control; these let callers force a sub-feature on/off (e.g.
        // bisect a divergence, or disable async for a deterministic profile)
        // without a flag.  `=1` enables, `=0` disables; anything else leaves
        // the value untouched.
        let env_bool = |k: &str| match env::var(k).ok().as_deref() {
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
        if let Ok(n) = env::var("VERYL_AOT_C_MIN_STMTS")
            && let Ok(n) = n.parse::<usize>()
        {
            self.aot_c_min_stmts = n;
        }
        // On by default for the CLI; `VERYL_DUT_REUSE=0` opts out.  Off in the
        // unit-test harness, which never calls `apply_env` (see `Config::dut_reuse`).
        self.dut_reuse = env::var("VERYL_DUT_REUSE").ok().as_deref() != Some("0");
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
