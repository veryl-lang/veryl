pub(crate) mod big_array;
mod build_session;
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
pub use build_session::{BuildSession, ProtoModuleCache};
pub use context::{Context, Conv};
pub use declaration::ProtoDeclaration;
pub use derived_clock::{DerivedClock, DerivedClockSchedule, DerivedReset, EdgeCandidate};
pub use event::Event;
pub use expression::{
    Expression, ExpressionContext, ProtoDynamicBitSelect, ProtoExpression, index_may_exceed,
    out_of_range_read,
};
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
pub(crate) use statement::{extend_to_i64, for_bound_reached};
pub use variable::{
    ModuleVariableMeta, ModuleVariables, VarOffset, Variable, VariableElement, VariableMeta,
    create_variable_meta, native_bytes, read_native_value, read_payload, strided_span, value_size,
    write_native_value, write_payload,
};
pub use veryl_analyzer::ir::{Op, Type, VarId, VarPath};
pub use veryl_analyzer::value::Value;

use crate::backend::CompiledWhole;
use crate::residency;
use crate::simulator::SimProfile;
use crate::simulator_error::SimulatorError;
use crate::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// See `Module::event_gates`.
    pub event_gates: HashMap<Event, opt::event_gate::RtEventGates>,
    /// See `Module::event_jit_marked`.
    pub(crate) event_jit_marked: HashSet<Event>,
    /// See `Module::event_jit_ff_marked`.
    pub(crate) event_jit_ff_marked: HashSet<Event>,
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
    /// Diagnostic: number of nontrivial SCCs found in the pre-JIT comb
    /// graph.  Real combinational loops are rejected by `analyze_dependency`,
    /// so any non-zero value here indicates duplicate ProtoStatements in
    /// the simulator IR assembly.  See `Module::nontrivial_comb_scc`.
    pub nontrivial_comb_scc: usize,
    /// See `Module::dataflow`.
    pub dataflow: Option<Arc<crate::ir::opt::dataflow::DataflowRt>>,
    /// See `Module::dataflow_interp`, by cell.
    dataflow_interp: HashMap<usize, Vec<Statement>>,
    /// Cells whose inputs changed since they last ran.
    pub(crate) dataflow_dirty: std::cell::RefCell<Vec<u64>>,
    /// Whether the dirty words record everything that moved since the last
    /// dataflow settle.  Cleared when they cannot: a settle on the regular
    /// path, `dataflow_mark_all`, a switch from the Cranelift cells to the
    /// `.so` or back from the whole-settle entry, a CHECK mismatch.  The next
    /// dataflow settle then runs every cell.
    pub(crate) dataflow_live: std::cell::Cell<bool>,
    /// Whether the last dataflow settle ran the `.so` cells (else Cranelift).
    pub(crate) dataflow_on_aot: std::cell::Cell<bool>,
    /// Per-cell dispatch or the whole-settle entry, settle by settle.
    pub(crate) dataflow_mode: DataflowMode,
    /// Bits of the cells dirty at every settle.
    pub(crate) dataflow_always_mask: std::cell::OnceCell<Vec<u64>>,
    /// The whole-settle entry is running the settles, so nothing reads the
    /// cells' bits: events and commits skip their marks and only say that
    /// something happened, in `dataflow_any`.
    pub(crate) dataflow_unmarked: std::cell::Cell<bool>,
    pub(crate) dataflow_any: std::cell::Cell<bool>,
    /// Compare every Nth dataflow settle against a full rerun (0: never);
    /// from `VERYL_DATAFLOW_CHECK`.
    pub(crate) dataflow_check: std::cell::Cell<u64>,
    dataflow_check_settles: std::cell::Cell<u64>,
    /// Panic on a mismatching check; from `VERYL_DATAFLOW_CHECK_PANIC`.
    pub(crate) dataflow_check_panic: std::cell::Cell<bool>,
    /// Variables the checks found different, summed over checks.
    pub(crate) dataflow_check_mismatches: std::cell::Cell<u64>,
    /// Snapshotted from `Config::aot_c_validate`: when set, `step` dual-runs
    /// the whole-event and Cranelift paths and panics on divergence, and the
    /// dataflow settle is checked against a full rerun.
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
    /// See `Module::event_comb_writes`.
    pub event_comb_writes: HashMap<Event, Option<Vec<(isize, isize)>>>,
    /// See `Module::gate_state_base`.
    pub gate_state_base: u32,
    /// See `Module::event_gate_flags`.
    pub event_gate_flags: Vec<u32>,
    /// See `Module::settle_info`.
    pub(crate) settle_info: crate::tb_dirty::SettleInfoCache,
    /// See `Module::fused_comb_offsets` (diagnostic; consumed by the
    /// dual-run checker to skip storage the fusion pass retired).
    pub fused_comb_offsets: Vec<isize>,
    /// A failed compile leaves the cell empty forever, so the fallback is
    /// taken every cycle; the residency table (a mutex) must be touched once.
    dataflow_fallback_recorded: AtomicBool,
    pub(crate) whole_event_fallback_recorded: AtomicBool,
    /// `[ran, fell_back]` dispatch tallies.  The `*_fallback_recorded` flags
    /// above latch on the FIRST fallback and so cannot say how much of a run
    /// was affected; these can.  Published to `residency` on drop.
    /// Dataflow settles run by the `.so` and by the Cranelift cells.
    pub(crate) dataflow_dispatch: [AtomicU64; 2],
    pub(crate) whole_event_dispatch: [AtomicU64; 2],
    /// Event gate skips on the per-statement path.  `Ir` is not `Sync` and
    /// the gates run on the simulator's own thread, so this needs no atomic.
    pub(crate) event_gate_skips: std::cell::Cell<u64>,
}

/// A built component library on disk and the type name to look up in it.
#[derive(Clone, Debug)]
pub struct ComponentLibrary {
    pub path: PathBuf,
    pub type_name: String,
}

/// Publishes the dispatch tallies once, at the end of the Ir's life, so the
/// hot path stays a relaxed atomic increment on a local field.
impl Drop for Ir {
    fn drop(&mut self) {
        for (kind, c) in [
            ("dataflow", &self.dataflow_dispatch),
            ("whole_event", &self.whole_event_dispatch),
        ] {
            let (ran, fell_back) = (c[0].load(Ordering::Relaxed), c[1].load(Ordering::Relaxed));
            // Formatting the name reads a thread-local table, which panics on a
            // thread that never imported it — and a panic here during unwinding
            // aborts.  Most drops have nothing to publish, so ask first.
            if ran == 0 && fell_back == 0 {
                continue;
            }
            residency::record_dispatch(kind, &self.name.to_string(), ran, fell_back);
        }
    }
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
            event_gates: module.event_gates,
            event_jit_marked: module.event_jit_marked,
            event_jit_ff_marked: module.event_jit_ff_marked,
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
            nontrivial_comb_scc: module.nontrivial_comb_scc,
            dataflow_dirty: std::cell::RefCell::new(vec![
                0;
                module
                    .dataflow
                    .as_ref()
                    .map_or(0, |d| d.plan.buffer_words())
            ]),
            dataflow_live: std::cell::Cell::new(false),
            dataflow_on_aot: std::cell::Cell::new(false),
            dataflow_mode: DataflowMode::default(),
            dataflow_always_mask: std::cell::OnceCell::new(),
            dataflow_unmarked: std::cell::Cell::new(false),
            dataflow_any: std::cell::Cell::new(false),
            // The dataflow comb has no second backend to dual-run, so
            // `--backend-validate` checks it against a full rerun instead.
            // That covers the cells only: the validated AOT-C event runs
            // without a dirty buffer, so its reader marks go unexercised.
            dataflow_check: std::cell::Cell::new(if config.aot_c_validate {
                config.aot_c_validate_stride.max(1)
            } else {
                dataflow_check_stride()
            }),
            dataflow_check_settles: std::cell::Cell::new(0),
            dataflow_check_panic: std::cell::Cell::new(
                config.aot_c_validate || dataflow_check_panic(),
            ),
            dataflow_check_mismatches: std::cell::Cell::new(0),
            dataflow: module.dataflow,
            dataflow_interp: module.dataflow_interp.into_iter().collect(),
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
            gate_state_base: module.gate_state_base,
            event_gate_flags: module.event_gate_flags,
            settle_info: module.settle_info,
            dataflow_fallback_recorded: Default::default(),
            whole_event_fallback_recorded: Default::default(),
            dataflow_dispatch: [AtomicU64::new(0), AtomicU64::new(0)],
            whole_event_dispatch: [AtomicU64::new(0), AtomicU64::new(0)],
            event_gate_skips: std::cell::Cell::new(0),
        };
        // Bake the WriteLogBuffer's heap-stable address into every
        // JIT-dispatched Compiled/CompiledBatch so emitted code can perform
        // inline log pushes without a TLS lookup.
        ir.install_write_log_ptr();
        if ir.dataflow.is_some() {
            ir.write_log_buffer.df_dirty = ir.dataflow_dirty.borrow_mut().as_mut_ptr();
        }
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
    /// and whether a dataflow comb / whole-event backend is dispatched.
    /// Chunking is identical for `cc` and `cranelift` (only dispatch differs),
    /// so a `cc` run also describes Cranelift's interp fallback. Read-only.
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
                    let const_range =
                        matches!(f.range.start, statement::RuntimeForBound::Const(..))
                            && matches!(f.range.end, statement::RuntimeForBound::Const(..));
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
        let report = |label: &str, stmts: &[Statement], tag: &str, accel: bool| {
            let (mut jit, mut interp) = (0usize, 0usize);
            for s in stmts {
                if s.is_compiled() {
                    jit += 1;
                } else if !matches!(s, Statement::Break) {
                    interp += 1;
                }
            }
            eprintln!(
                "  {label}: total={} jit={jit} interp={interp}  {tag}={accel}",
                stmts.len()
            );
            for s in stmts {
                if !s.is_compiled() && !matches!(s, Statement::Break) {
                    eprintln!("      interp: {}", classify(s));
                }
            }
        };
        eprintln!("=== BackendDiag for {} ===", self.name);
        report(
            "comb",
            &self.comb_statements,
            "dataflow",
            self.dataflow.is_some(),
        );
        let mut events: Vec<_> = self.event_statements.iter().collect();
        events.sort_by_key(|(e, _)| format!("{e:?}"));
        for (event, stmts) in events {
            report(
                &format!("event {event:?}"),
                stmts,
                "whole",
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
    }

    /// Settle the comb: through the dataflow cells when the module has a
    /// ready dataflow settle (only the cells something marked run, once), else
    /// every comb statement for `required_comb_passes` passes.
    ///
    /// Real combinational loops are rejected by `analyze_dependency`
    /// (error: `combinational_loop`), so neither path tests for convergence:
    /// the cells run once in topological order, the chunked settle runs its
    /// precomputed pass count.
    pub fn settle_comb(&self, mask_cache: &mut MaskCache, profile: &mut SimProfile) {
        #[cfg(feature = "profile")]
        {
            profile.settle_comb_count += 1;
        }
        #[cfg(not(target_family = "wasm"))]
        let settled = self.settle_dataflow();
        #[cfg(target_family = "wasm")]
        let settled = false;
        if !settled {
            self.run_chunked_settle(mask_cache, profile);
        }
        if settle_converge_check() {
            self.check_settled(mask_cache, settled);
        }
    }

    /// `VERYL_SETTLE_CONVERGE_CHECK=1`: one extra pass must change nothing.
    /// `dataflow` says the settle just checked ran through the dataflow cells.
    ///
    /// Every side effect of the extra pass is rewound.
    fn check_settled(&self, mask_cache: &mut MaskCache, dataflow: bool) {
        let before = self.comb_values.clone();
        // The extra pass has side effects -- the is_ff refinement pushes
        // write-log entries and `$display` writes the output buffer -- so
        // rewind both.
        let ff_before = self.ff_values.clone();
        let narrow_before = self.write_log_buffer.narrow_count();
        let wide_before = self.write_log_buffer.wide_count();
        let out_mark = crate::output_buffer::mark();
        for x in &self.comb_statements {
            dispatch_stmt_fast(x, mask_cache);
        }
        let after = self.comb_values.clone();
        // Restore before reporting, so a run under the check that survives the
        // first divergence still simulates what it would have without it.
        unsafe {
            std::ptr::copy_nonoverlapping(
                before.as_ptr(),
                self.comb_values.as_ptr() as *mut u8,
                before.len(),
            );
            std::ptr::copy_nonoverlapping(
                ff_before.as_ptr(),
                self.ff_values.as_ptr() as *mut u8,
                ff_before.len(),
            );
            let buf =
                (&*self.write_log_buffer) as *const _ as *mut crate::ir::write_log::WriteLogBuffer;
            (*buf).narrow_count = narrow_before;
            (*buf).wide_count = wide_before;
        }
        crate::output_buffer::truncate_to(out_mark);
        // Bytes the dataflow module leaves deliberately stale (chunk-local, no
        // external reader), so a difference there is this check's artefact.
        // The dataflow CHECK skips the same set.
        let stale = self
            .dataflow
            .as_ref()
            .map(|d| d.localized.as_slice())
            .unwrap_or(&[]);
        let localized = |i: usize| {
            stale
                .iter()
                .any(|&(off, nb)| off >= 0 && i >= off as usize && i < off as usize + nb)
        };
        for (i, (a, b)) in before.iter().zip(after.iter()).enumerate() {
            if a != b && !localized(i) {
                if dataflow {
                    panic!(
                        "dataflow settle did not converge ({}): comb byte {i} changed \
                         {a:#x} -> {b:#x} on an extra pass",
                        self.name,
                    );
                }
                panic!(
                    "settle did not converge in {} pass(es) ({}): comb byte {i} changed \
                     {a:#x} -> {b:#x} on an extra pass -- required_comb_passes is too low",
                    self.required_comb_passes, self.name,
                );
            }
        }
    }

    /// Run the dirty cells through the dataflow module.  `false` when there is
    /// none or it has not landed yet; the caller then settles everything.
    #[cfg(not(target_family = "wasm"))]
    fn settle_dataflow(&self) -> bool {
        let Some(df) = self.dataflow.as_ref() else {
            return false;
        };
        // The `.so` once it lands, the Cranelift cells until then.  Both run
        // the same plan on the same dirty words.
        let aot = df.cell.get();
        let jit = if aot.is_none() { df.jit.as_ref() } else { None };
        if aot.is_none() && jit.is_none() {
            self.dataflow_live.set(false);
            return false;
        }
        if aot.is_some() {
            self.dataflow_dispatch[0].fetch_add(1, Ordering::Relaxed);
        } else if !df.jit_only {
            self.dataflow_dispatch[1].fetch_add(1, Ordering::Relaxed);
            if !self
                .dataflow_fallback_recorded
                .swap(true, Ordering::Relaxed)
            {
                residency::record_fallback("dataflow", &self.name.to_string());
            }
        }
        // The Cranelift cells keep no boundary shadow, so on the switch to the
        // `.so` every shadow is stale: a cell whose new value happens to equal
        // it would mark nothing.  Running every cell once refreshes them all.
        if self.dataflow_on_aot.replace(aot.is_some()) != aot.is_some() {
            self.dataflow_live.set(false);
        }
        let coarse_fn = aot.and_then(|m| m.coarse_func);
        // With no `.so` coming, the whole settle is the chunked one.
        let (coarse, probing) = if coarse_fn.is_some() || df.jit_only {
            self.dataflow_mode
                .next(|| df.plan.cells.iter().map(Vec::len).sum())
        } else {
            (false, false)
        };
        // Under the whole-settle entry no event or commit needs to mark;
        // the Cranelift events find no dirty words to mark in.
        if self.dataflow_unmarked.replace(coarse) != coarse {
            let buf =
                (&*self.write_log_buffer) as *const _ as *mut crate::ir::write_log::WriteLogBuffer;
            // SAFETY: the buffer is heap-stable and nothing else holds it
            // between dispatches.
            unsafe {
                (*buf).df_dirty = if !coarse {
                    self.dataflow_dirty.borrow_mut().as_mut_ptr()
                } else {
                    std::ptr::null_mut()
                };
            }
        }
        // The whole-settle entry leaves the cells' boundary shadows behind,
        // so back on the cells every one of them runs once.
        if self.dataflow_mode.was_coarse.replace(coarse) && !coarse {
            self.dataflow_live.set(false);
        }
        if coarse && aot.is_none() {
            return false;
        }
        static LANDED: std::sync::Once = std::sync::Once::new();
        LANDED.call_once(|| {
            if crate::ir::opt::dataflow::diag() {
                eprintln!("[dataflow] {} cells live", df.plan.cells.len());
            }
        });
        let mut dirty = self.dataflow_dirty.borrow_mut();
        let diag = crate::ir::opt::dataflow::diag();
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static FULL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static SEEDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static NANOS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let t0 = if diag {
            if !self.dataflow_live.get() {
                FULL.fetch_add(1, Ordering::Relaxed);
            }
            let seeded: u32 = dirty[..df.plan.words()]
                .iter()
                .map(|w| w.count_ones())
                .sum();
            SEEDED.fetch_add(seeded as u64, Ordering::Relaxed);
            Some(std::time::Instant::now())
        } else {
            None
        };
        if !self.dataflow_live.replace(true) {
            let nw = df.plan.words();
            dirty[..nw].iter_mut().for_each(|w| *w = u64::MAX);
            let tail = df.plan.cells.len() % 64;
            if tail != 0 {
                dirty[nw - 1] = (1u64 << tail) - 1;
            }
        }
        for &k in &df.plan.always_run {
            dirty[k as usize / 64] |= 1u64 << (k % 64);
        }
        let ff_ptr = self.ff_values.as_ptr();
        let comb_ptr = self.comb_values.as_ptr();
        let log_ptr = (&*self.write_log_buffer as *const _ as *const u8) as *mut u8;
        // A sampled check sees a missed mark only while the stale value
        // lasts: a later mark of the same cell can repair it between samples.
        let stride = self.dataflow_check.get();
        let check = stride != 0 && {
            let n = self.dataflow_check_settles.get();
            self.dataflow_check_settles.set(n.wrapping_add(1));
            n.is_multiple_of(stride)
        };
        let pre = if check {
            Some((
                gather_ranges(&self.comb_values, &df.plan.written),
                gather_ranges(&self.ff_values, &df.plan.written_ff),
                dirty.clone(),
            ))
        } else {
            None
        };
        let counted = aot.and_then(|m| m.counted).filter(|_| probing);
        let count_jit = probing && aot.is_none();
        // Through the log argument, which the per-cell entries leave unused.
        let mut counts = [0u64; 2];
        // SAFETY: the dataflow entry has the `FuncPtr` ABI, reading and
        // writing the same buffers; `ff_delta` carries the dirty words,
        // which it clears as it runs.
        if let Some(m) = aot {
            let nw = df.plan.words();
            let always = self.dataflow_always_mask.get_or_init(|| {
                let mut v = vec![0u64; nw];
                for &k in &df.plan.always_run {
                    v[k as usize / 64] |= 1u64 << (k % 64);
                }
                v
            });
            let any = self.dataflow_any.replace(false);
            let whole =
                coarse && (any || dirty[..nw].iter().zip(always).any(|(&d, &a)| d & !a != 0));
            match coarse_fn {
                Some(f) if whole => {
                    unsafe {
                        f(ff_ptr, comb_ptr, log_ptr, dirty.as_mut_ptr() as isize);
                    }
                    dirty[..nw].fill(0);
                }
                _ => unsafe {
                    match counted {
                        Some(f) => f(
                            ff_ptr,
                            comb_ptr,
                            counts.as_mut_ptr().cast(),
                            dirty.as_mut_ptr() as isize,
                        ),
                        None => (m.func)(ff_ptr, comb_ptr, log_ptr, dirty.as_mut_ptr() as isize),
                    }
                },
            }
        } else if let Some(j) = jit {
            let bits = dirty.as_mut_ptr();
            for w in 0..df.plan.words() {
                // SAFETY: `w` is inside the dirty words; a cell only sets
                // bits of later cells, through the same pointer.
                unsafe {
                    loop {
                        let b = *bits.add(w);
                        if b == 0 {
                            break;
                        }
                        *bits.add(w) = b & (b - 1);
                        let k = w * 64 + b.trailing_zeros() as usize;
                        if count_jit {
                            counts[0] += df.plan.cells[k].len() as u64;
                            counts[1] += 1;
                        }
                        match j.funcs[k] {
                            Some(f) => f(ff_ptr, comb_ptr, log_ptr, bits as isize),
                            None => self.interpret_cell(df, k, bits),
                        }
                    }
                }
            }
        }
        if counted.is_some() || count_jit {
            self.dataflow_mode.record(counts[0], counts[1]);
        }
        drop(dirty);
        if let Some(t0) = t0 {
            NANOS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            let n = N.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() && n >= 1024 {
                eprintln!(
                    "[dataflow] settles {n}: every cell in {}, seeded cells/settle {:.1}, ns/settle {:.0}",
                    FULL.load(Ordering::Relaxed),
                    SEEDED.load(Ordering::Relaxed) as f64 / n as f64,
                    NANOS.load(Ordering::Relaxed) as f64 / n as f64,
                );
            }
        }
        // The buffer now holds the reference values, which the `.so` cells'
        // boundary shadows have not seen.
        if let Some((pre, pre_ff, bits)) = pre
            && self.check_dataflow(df, &pre, &pre_ff, &bits) != 0
        {
            self.dataflow_live.set(false);
        }
        true
    }

    /// A cell Cranelift could not compile: its statements through the
    /// interpreter, with the boundary watch the compiled cells carry.
    ///
    /// # Safety
    /// `bits` points at the dirty words.
    #[cfg(not(target_family = "wasm"))]
    unsafe fn interpret_cell(
        &self,
        df: &crate::ir::opt::dataflow::DataflowRt,
        k: usize,
        bits: *mut u64,
    ) {
        let watch = &df.plan.boundary[k];
        let before: Vec<u8> = watch
            .iter()
            .flat_map(|&(a, b)| self.comb_values[a as usize..b as usize].iter().copied())
            .collect();
        let mut mc = MaskCache::default();
        for x in &self.dataflow_interp[&k] {
            dispatch_stmt_fast(x, &mut mc);
        }
        let mut pos = 0usize;
        let changed = df.plan.mark_always[k]
            || watch.iter().any(|&(a, b)| {
                let (a, b) = (a as usize, b as usize);
                let moved = before[pos..pos + (b - a)] != self.comb_values[a..b];
                pos += b - a;
                moved
            });
        if changed {
            for &(w, m) in &df.plan.consumers[k] {
                // SAFETY: the caller's dirty words cover every consumer.
                unsafe { *bits.add(w as usize) |= m };
            }
        }
    }

    /// `VERYL_DATAFLOW_CHECK=N`: rerun every comb statement after putting
    /// back the settle's starting values of the ranges the plan says it
    /// writes (other bytes stay as the settle left them), and report each
    /// variable a cell writes that came out different.  The reference result
    /// is kept.  Returns the number of differing variables.
    #[cfg(not(target_family = "wasm"))]
    fn check_dataflow(
        &self,
        df: &crate::ir::opt::dataflow::DataflowRt,
        pre: &[u8],
        pre_ff: &[u8],
        bits: &[u64],
    ) -> u64 {
        let mut mismatches = 0u64;
        static REPORTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static CHECKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        // A settle writes only the ranges the plan tracks (anything else is a
        // declined plan), so those are all that go back and get compared.
        let (written, written_ff) = (&df.plan.written, &df.plan.written_ff);
        let got = gather_ranges(&self.comb_values, written);
        let got_ff = gather_ranges(&self.ff_values, written_ff);
        // SAFETY: the ranges lie inside the buffers they were gathered from.
        unsafe {
            scatter_ranges(self.comb_values.as_ptr() as *mut u8, written, pre);
            scatter_ranges(self.ff_values.as_ptr() as *mut u8, written_ff, pre_ff);
        }
        // The settle already ran the effect cells; the rerun is for values.
        let mute = crate::output_buffer::Mute::begin();
        let mut mc = MaskCache::default();
        for x in &self.comb_statements {
            dispatch_stmt_fast(x, &mut mc);
        }
        drop(mute);
        let mut pos = 0usize;
        for &(a, b) in written_ff {
            let (a, b) = (a as usize, b as usize);
            let mine = &got_ff[pos..pos + (b - a)];
            pos += b - a;
            if mine != &self.ff_values[a..b] {
                mismatches += 1;
                let n = REPORTS.fetch_add(1, Ordering::Relaxed);
                if n < 20 {
                    eprintln!(
                        "[dataflow] CHECK MISMATCH {} [ff {a:#x}..{b:#x}) dataflow={:02x?} reference={:02x?}",
                        self.var_name_at(VarOffset::new(true, a as isize))
                            .unwrap_or_default(),
                        &mine[..mine.len().min(16)],
                        &self.ff_values[a..b.min(a + 16)],
                    );
                }
            }
        }
        let local = |a: usize, b: usize| {
            df.localized
                .iter()
                .any(|&(o, w)| o >= 0 && (o as usize) < b && a < o as usize + w)
        };
        // A byte's value when the settle began: the saved copy where the
        // settle wrote, the buffer itself everywhere else.
        let at_start = |is_ff: bool, ia: usize, ib: usize| -> Vec<u8> {
            let (ranges, saved, buf) = if is_ff {
                (written_ff, pre_ff, &self.ff_values[..])
            } else {
                (written, pre, &self.comb_values[..])
            };
            let mut p = 0usize;
            for &(a, b) in ranges.iter() {
                let (a, b) = (a as usize, b as usize);
                if a <= ia && ib <= b {
                    return saved[p + ia - a..p + ib - a].to_vec();
                }
                p += b - a;
            }
            buf[ia..ib].to_vec()
        };
        let mut pos = 0usize;
        for (wi, &(a, b)) in written.iter().enumerate() {
            let (a, b) = (a as usize, b as usize);
            let mine = &got[pos..pos + (b - a)];
            pos += b - a;
            if mine != &self.comb_values[a..b] && !local(a, b) {
                mismatches += 1;
                let n = REPORTS.fetch_add(1, Ordering::Relaxed);
                if n < 20 {
                    let wc = df.plan.writer_cell[wi] as usize;
                    eprintln!(
                        "[dataflow] CHECK MISMATCH {} [{a:#x}..{b:#x}) dataflow={:02x?} reference={:02x?} writer cell {wc} dirty_at_start={}",
                        self.var_name_at(VarOffset::new(false, a as isize))
                            .unwrap_or_default(),
                        &mine[..mine.len().min(16)],
                        &self.comb_values[a..b.min(a + 16)],
                        bits[wc / 64] >> (wc % 64) & 1,
                    );
                    // Which of the writer cell's inputs differ from the last
                    // settle's end?  A changed input nobody marked is the hole.
                    if n < 3 {
                        for &(is_ff, ia, ib) in &df.plan.cell_inputs[wc] {
                            let (ia, ib) = (ia as usize, ib as usize);
                            let cur = if is_ff {
                                &self.ff_values[ia..ib]
                            } else {
                                &self.comb_values[ia..ib]
                            };
                            let prev = at_start(is_ff, ia, ib);
                            eprintln!(
                                "[dataflow]     input {} {} [{ia:#x}..{ib:#x}) at_start={:02x?} reference_end={:02x?}",
                                if is_ff { "ff" } else { "comb" },
                                self.var_name_at(VarOffset::new(is_ff, ia as isize))
                                    .unwrap_or_default(),
                                &prev[..prev.len().min(8)],
                                &cur[..cur.len().min(8)],
                            );
                        }
                    }
                }
            }
        }
        self.dataflow_check_mismatches
            .set(self.dataflow_check_mismatches.get() + mismatches);
        if mismatches != 0 && self.dataflow_check_panic.get() {
            panic!("[dataflow] CHECK MISMATCH");
        }
        let c = CHECKS.fetch_add(1, Ordering::Relaxed) + 1;
        if c.is_power_of_two() {
            eprintln!(
                "[dataflow] checked {c} settles, {} mismatching variables",
                REPORTS.load(Ordering::Relaxed)
            );
        }
        mismatches
    }

    /// Mark the cells reading or writing comb bytes `[a, b)`.
    pub(crate) fn dataflow_mark_comb(&self, a: usize, b: usize) {
        if let Some(df) = self.dataflow.as_ref() {
            mark_readers(
                &df.plan.comb_marked,
                &mut self.dataflow_dirty.borrow_mut(),
                a,
                b,
            );
        }
    }

    /// Mark the cells reading or writing FF bytes `[a, b)`.
    pub(crate) fn dataflow_mark_ff(&self, a: usize, b: usize) {
        if let Some(df) = self.dataflow.as_ref() {
            mark_readers(
                &df.plan.ff_marked,
                &mut self.dataflow_dirty.borrow_mut(),
                a,
                b,
            );
        }
    }

    /// Run every cell on the next dataflow settle.
    #[track_caller]
    pub(crate) fn dataflow_mark_all(&self) {
        if self.dataflow.is_some() && crate::ir::opt::dataflow::diag() {
            static SITES: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());
            let here = std::panic::Location::caller().to_string();
            let mut v = SITES.lock().unwrap();
            let n = match v.iter_mut().find(|x| x.0 == here) {
                Some(x) => {
                    x.1 += 1;
                    x.1
                }
                None => {
                    v.push((here.clone(), 1));
                    1
                }
            };
            if n.is_power_of_two() && n >= 64 {
                eprintln!("[dataflow] mark_all from {here}: {n}");
            }
        }
        self.dataflow_live.set(false);
    }

    /// Mark the readers of `len` bytes stored at `ptr`, in whichever buffer
    /// holds them; anywhere else marks everything.
    pub(crate) fn dataflow_mark_ptr(&self, ptr: *const u8, len: usize) {
        if self.dataflow.is_none() {
            return;
        }
        let p = ptr as usize;
        let comb = self.comb_values.as_ptr() as usize;
        let ff = self.ff_values.as_ptr() as usize;
        if (comb..comb + self.comb_values.len()).contains(&p) {
            self.dataflow_mark_comb(p - comb, p - comb + len);
        } else if (ff..ff + self.ff_values.len()).contains(&p) {
            self.dataflow_mark_ff(p - ff, p - ff + len);
        } else {
            self.dataflow_mark_all();
        }
    }

    /// Whether settles currently run through the dataflow module.
    pub(crate) fn dataflow_ready(&self) -> bool {
        #[cfg(not(target_family = "wasm"))]
        {
            self.dataflow
                .as_ref()
                .is_some_and(|d| d.cell.get().is_some() || d.jit.is_some())
        }
        #[cfg(target_family = "wasm")]
        {
            false
        }
    }

    /// The per-chunk settle: every comb statement, once per required pass.
    fn run_chunked_settle(&self, mask_cache: &mut MaskCache, profile: &mut SimProfile) {
        let _ = profile;

        // `VERYL_MIN_PASSES_OVERRIDE` is a debug knob of this path only.
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

    /// Evaluate unified comb once: one pass of the per-chunk settle.
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

    /// Hierarchical name of the variable element whose storage holds `off`
    /// (diagnostics).
    pub fn var_name_at(&self, off: VarOffset) -> Option<String> {
        let raw = off.raw();
        if raw < 0 {
            return None;
        }
        let base = if off.is_ff() {
            self.ff_values.as_ptr()
        } else {
            self.comb_values.as_ptr()
        } as usize
            + raw as usize;
        fn find(module: &ModuleVariables, addr: usize, prefix: &str) -> Option<String> {
            for v in module.variables.values() {
                for (k, &p) in v.current_values.iter().enumerate() {
                    let p = p as usize;
                    if (p..p + v.native_bytes.max(1)).contains(&addr) {
                        return Some(format!("{prefix}{}[{k}]", v.path));
                    }
                }
            }
            for child in &module.children {
                if let Some(n) = find(child, addr, &format!("{prefix}{}.", child.name)) {
                    return Some(n);
                }
            }
            None
        }
        find(&self.module_variables, base, "")
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

/// `VERYL_SETTLE_CONVERGE_CHECK=1` — see [`Ir::check_settled`].  Read once:
/// `settle_comb` runs every cycle.
fn settle_converge_check() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| env::var("VERYL_SETTLE_CONVERGE_CHECK").as_deref() == Ok("1"))
}

/// `VERYL_DATAFLOW_CHECK[=N]`: every Nth settle, every 64th without a number.
fn dataflow_check_stride() -> u64 {
    static CHECK: OnceLock<u64> = OnceLock::new();
    *CHECK.get_or_init(|| match env::var("VERYL_DATAFLOW_CHECK") {
        Ok(v) => v.parse().unwrap_or(64),
        Err(_) => 0,
    })
}

fn dataflow_check_panic() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| env::var("VERYL_DATAFLOW_CHECK_PANIC").as_deref() == Ok("1"))
}

/// Build a single top in an isolated session. Use `BuildSession` to share DUT
/// conversion across multiple tops from the same analysis IR.
pub fn build_ir(ir: &air::Ir, top: StrId, config: &Config) -> Result<Ir, SimulatorError> {
    BuildSession::new(ir, config, &[top]).build_ir(top)
}

/// Build using a cache already bound to its session's IR and configuration.
pub fn build_ir_cached(top: StrId, cache: &mut ProtoModuleCache<'_>) -> Result<Ir, SimulatorError> {
    cache.build_ir(top)
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
    /// Settle a comb list that has no one-pass order in as many passes as it
    /// takes, instead of rejecting it.  A synthesisable design has such an
    /// order bit by bit; the analysis works per statement and may not find
    /// it, and this trades the error for extra settle passes.
    /// `VERYL_ALLOW_MULTI_PASS=1` or `=0` overrides it, including where no
    /// flag reaches.
    pub allow_multi_pass: bool,
    /// `cc` backend: emit the dataflow comb settle as C, compile externally,
    /// and dispatch the `.so` instead of the Cranelift loop (which still
    /// covers modules the settle declines, so keep `use_jit` true).  Default
    /// false; `--backend cc` enables it.
    pub aot_c: bool,
    /// `cc` backend event path: also emit the per-event FF-next + write-log.
    /// Requires `aot_c`.
    pub aot_c_event: bool,
    /// Compile the `.so` on a background thread and hot-swap from Cranelift
    /// once ready, hiding the cold compile latency.  Requires `aot_c`; forced
    /// off under `aot_c_validate` (validation must dual-run from cycle 0).
    pub aot_c_async: bool,
    /// Dual-run each whole-event `.so` against Cranelift and check each
    /// dataflow settle against a full rerun, panicking on the first divergence
    /// (correctness check).  Implies a synchronous compile.
    pub aot_c_validate: bool,
    /// Stride for `aot_c_validate`: check only every Nth dataflow settle /
    /// event (an off-stride event runs Cranelift only, the ground truth that
    /// drives the sim).  0 or 1 = every cycle (full coverage); larger trades
    /// coverage for speed on long tests.
    pub aot_c_validate_stride: u64,
    /// Minimum module statement count (comb + event) before `cc` is attempted;
    /// below it the module stays on per-chunk Cranelift.  Default 0 (no floor)
    /// now that the compile pool caps concurrency; set `VERYL_AOT_C_MIN_STMTS=N`
    /// to restore a floor.
    pub aot_c_min_stmts: usize,
    /// Cache a converted DUT and relocate it into later builds in the same
    /// `BuildSession`. Different sessions never share these caches. Default
    /// off; `apply_env` enables it for the CLI unless explicitly disabled.
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
    /// Run on Cranelift alone, for a host where AOT-C modules cannot load.
    pub fn fall_back_to_cranelift(&mut self) {
        self.use_jit = true;
        self.aot_c = false;
        self.aot_c_event = false;
        self.aot_c_async = false;
        self.aot_c_validate = false;
    }

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
        if let Ok(n) = env::var("VERYL_AOT_C_MIN_STMTS")
            && let Ok(n) = n.parse::<usize>()
        {
            self.aot_c_min_stmts = n;
        }
        if let Some(v) = env_bool("VERYL_ALLOW_MULTI_PASS") {
            self.allow_multi_pass = v;
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
        if crate::backend::aot_c::cc_available() {
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

/// The bytes of `ranges` in `buf`, back to back.
fn gather_ranges(buf: &[u8], ranges: &[(u32, u32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ranges.iter().map(|&(a, b)| (b - a) as usize).sum());
    for &(a, b) in ranges {
        out.extend_from_slice(&buf[a as usize..b as usize]);
    }
    out
}

/// The inverse of `gather_ranges`.
///
/// # Safety
/// Every range lies inside the buffer `buf` points to.
unsafe fn scatter_ranges(buf: *mut u8, ranges: &[(u32, u32)], data: &[u8]) {
    let mut p = 0usize;
    for &(a, b) in ranges {
        let n = (b - a) as usize;
        // SAFETY: the caller guarantees the range is in bounds.
        unsafe { std::ptr::copy_nonoverlapping(data[p..p + n].as_ptr(), buf.add(a as usize), n) };
        p += n;
    }
}

pub(crate) fn mark_readers(
    table: &crate::ir::opt::dataflow::Readers,
    dirty: &mut [u64],
    a: usize,
    b: usize,
) {
    let (a, b) = (a as u32, b as u32);
    let first = table.partition_point(|t| t.1 <= a);
    for (s, _, readers) in &table[first..] {
        if *s >= b {
            break;
        }
        for &c in readers {
            dirty[(c / 64) as usize] |= 1 << (c % 64);
        }
    }
}

/// Chooses between a dataflow comb's per-cell and whole-settle entries.
/// Which pays depends on how many cells a settle dirties, which only a run
/// shows, so a probe window at the start of each period counts the per-cell
/// entry's work.  Counting rather than timing keeps the choice the same on
/// any machine.  A choice that holds doubles the next period, so a design
/// that keeps its character stops paying for probes.
#[derive(Default)]
pub(crate) struct DataflowMode {
    settles: std::cell::Cell<u64>,
    /// `log2` of the current period, past `MIN_PERIOD_LOG2`.
    backoff: std::cell::Cell<u32>,
    decided: std::cell::Cell<bool>,
    coarse: std::cell::Cell<bool>,
    pub(crate) was_coarse: std::cell::Cell<bool>,
    counted: [std::cell::Cell<u64>; 3],
}

impl DataflowMode {
    const PROBE: u64 = 1024;
    const MIN_PERIOD_LOG2: u32 = 18;
    const MAX_PERIOD_LOG2: u32 = 26;

    /// What a cell call costs beyond its statements, in statements: the
    /// dirty-bit scan, the call and the boundary compare.  Calibrated on
    /// forced runs of both entries: on every reference CPU core and GPU design
    /// measured, any value from 17 to 40 picks the entry that ran fewer
    /// instructions.
    const CALL_COST: f64 = 24.0;

    /// This settle's mode, and whether it is a probe.
    fn next(&self, stmts: impl FnOnce() -> usize) -> (bool, bool) {
        let i = self.settles.get();
        let period = 1u64 << (Self::MIN_PERIOD_LOG2 + self.backoff.get());
        self.settles.set(if i + 1 == period { 0 } else { i + 1 });
        // The first settle of a period may rerun every cell (back from the
        // whole-settle entry, or as the `.so` lands), which says nothing
        // about the design.
        if i <= Self::PROBE {
            return (false, i > 0);
        }
        if i == Self::PROBE + 1 {
            let [s, c, n] = self.counted.each_ref().map(|x| x.get());
            let fine = (s as f64 + Self::CALL_COST * c as f64) / n.max(1) as f64;
            let whole = stmts() as f64;
            let held = self.decided.replace(true) && (whole < fine) == self.coarse.get();
            self.backoff.set(if held {
                (self.backoff.get() + 1).min(Self::MAX_PERIOD_LOG2 - Self::MIN_PERIOD_LOG2)
            } else {
                0
            });
            self.coarse.set(whole < fine);
            if crate::ir::opt::dataflow::diag() {
                eprintln!(
                    "[dataflow] mode: cells {fine:.0} ({:.0} stmts, {:.1} calls), whole {whole:.0} -> {}",
                    s as f64 / n.max(1) as f64,
                    c as f64 / n.max(1) as f64,
                    if whole < fine { "whole" } else { "cells" }
                );
            }
            for x in &self.counted {
                x.set(0);
            }
        }
        (self.coarse.get(), false)
    }

    /// Start a new period, whose probe window runs the per-cell entry.
    #[cfg(test)]
    pub(crate) fn restart_period(&self) {
        self.settles.set(0);
    }

    fn record(&self, stmts: u64, calls: u64) {
        for (x, v) in self.counted.iter().zip([stmts, calls, 1]) {
            x.set(x.get() + v);
        }
    }
}
