pub(crate) mod comb_layout;
pub(crate) mod comb_pipeline_cache;
pub(crate) mod context;
pub(crate) mod declaration;
pub mod derived_clock;
mod event;
mod expression;
pub(crate) mod external;
pub(crate) mod hier_ref;
pub mod incremental;
pub(crate) mod inst_layout;
pub(crate) mod module;
pub(crate) mod opt;
pub(crate) mod partial_index;
pub(crate) mod site_table;
mod statement;
pub(crate) mod variable;
pub(crate) mod write_log;

pub use context::{Context, Conv};
pub use declaration::ProtoDeclaration;
pub use derived_clock::{DerivedClock, DerivedClockSchedule};
pub use event::Event;
pub use expression::{Expression, ExpressionContext, ProtoDynamicBitSelect, ProtoExpression};
pub use external::{
    ExternalComponentInst, ExternalConnectInst, ProtoExternalComponent, ProtoExternalConnect,
};
pub use module::{Module, ProtoModule};
pub use statement::{
    CompiledBatchStmt, CompiledBlockStatement, CompiledStmt, ComponentArg,
    ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoCaseStatement, ProtoComponentArg,
    ProtoForBound, ProtoForRange, ProtoForStatement, ProtoIfStatement, ProtoStatement,
    ProtoStatementBlock, ProtoStatements, ProtoSystemFunctionCall, RetWidthCheck, RuntimeForBound,
    RuntimeForRange, Statement, SystemFunctionCall, TbMethodKind, format_assert_message,
    format_output, parse_hex_content, patch_stmt_log_buf, veryl_aot_sysfn_print,
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
    /// See `Module::fused_comb_offsets` (diagnostic; consumed by the
    /// dual-run checker to skip storage the fusion pass retired).
    pub fused_comb_offsets: Vec<isize>,
    /// A failed compile leaves the cell empty forever, so the fallback is
    /// taken every cycle; the residency table (a mutex) must be touched once.
    whole_comb_fallback_recorded: AtomicBool,
    pub(crate) whole_event_fallback_recorded: AtomicBool,
    /// Change-driven settle plan (`None` unless the incremental settle is on
    /// and the module is supported); see `ir::incremental`.
    pub incr_plan: Option<Arc<incremental::IncrPlan>>,
    /// Deferred whole-module AOT-C (`Some` only under the incremental
    /// configuration): compile inputs + landing slots, spawned when the
    /// plan is declined at build time or auto-abandoned at runtime.  See
    /// `backend::late`.
    pub late_aotc: Option<Arc<crate::backend::late::LateAotc>>,
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
            whole_comb_fallback_recorded: Default::default(),
            whole_event_fallback_recorded: Default::default(),
            incr_plan: module.incr_plan,
            late_aotc: module.late_aotc,
        };
        // Bake the WriteLogBuffer's heap-stable address into every
        // JIT-dispatched Compiled/CompiledBatch so emitted code can perform
        // inline log pushes without a TLS lookup.
        ir.install_write_log_ptr();
        ir.backend_diag();
        // Incremental plan declined at build time: the full-settle fallback
        // is permanent from cycle 0, so start the deferred whole-module
        // AOT-C compile right away (see `backend::late`).
        if ir.incr_plan.is_none()
            && let Some(late) = ir.late_aotc.as_ref()
        {
            crate::backend::late::LateAotc::spawn(late);
        }
        ir
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
                            " dynsel(elem={} n={} full={})",
                            d.elem_width,
                            d.num_elements,
                            d.elem_width * d.num_elements
                        ))
                        .unwrap_or_default(),
                ),
                Statement::AssignDynamic(a) => format!(
                    "AssignDynamic dst_width={} n_elem={} full={}{}",
                    a.dst_width,
                    a.dst_num_elements,
                    a.dst_width * a.dst_num_elements,
                    a.dynamic_select
                        .as_ref()
                        .map(|d| format!(
                            " dynsel(elem={} n={} full={})",
                            d.elem_width,
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

    /// Mark-driven event skip: diff the words `partial_settle` writes and
    /// mark event chunks watching a changed one.  Those writes happen
    /// between the full settle and the event evals of the same step, so
    /// the seed scans can't see them in time (the ext scan would deliver
    /// them one event pass late).  Call right after each `partial_settle`.
    pub fn mark_event_partial(&self, state: &mut incremental::IncrState) {
        let Some(plan) = self.incr_plan.as_ref() else {
            return;
        };
        if state.abandoned || plan.event_chunk_count == 0 || plan.partial_out_words.is_empty() {
            return;
        }
        // Pre-first-settle fires run under all-dirty semantics; the bitmap
        // isn't sized yet.
        if state.event_dirty.is_empty() {
            return;
        }
        if state.prev_partial.len() != plan.partial_out_words.len() {
            // First use: zeros make every watched word "changed", which
            // only re-marks chunks that start all-dirty anyway.
            state.prev_partial = vec![0u64; plan.partial_out_words.len()];
        }
        let comb: &[u8] = &self.comb_values;
        let ff: &[u8] = &self.ff_values;
        let read = |w: usize| -> u64 {
            if w < plan.comb_words {
                incremental::read_word(comb, w)
            } else {
                incremental::read_word(ff, w - plan.comb_words)
            }
        };
        let words = &plan.partial_out_words;
        let mut i = 0usize;
        while i < words.len() {
            let end = (i + 8).min(words.len());
            let mut acc = 0u64;
            for (&w, &p) in words[i..end].iter().zip(&state.prev_partial[i..end]) {
                acc |= read(w as usize) ^ p;
            }
            if acc != 0 {
                // Index-based so the `prev_partial` borrow ends before the
                // mark; the marks feed the next settle's dirty-seed diff
                // too, so comb consumers of the closure's writes wake
                // without a full scan.
                for (k, &wid) in words.iter().enumerate().take(end).skip(i) {
                    let w = wid as usize;
                    let v = read(w);
                    let p = state.prev_partial[k];
                    if v != p {
                        let dgm = incremental::byte_nonzero_mask(v ^ p);
                        state.prev_partial[k] = v;
                        plan.mark_word(&mut state.sink(), incremental::MarkSource::Partial, w, dgm);
                    }
                }
            }
            i = end;
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
        // `whole_comb` is populated at conv time (default pipeline); the
        // late slot lands asynchronously after an incremental-plan decline
        // or auto-abandon (see `backend::late`) — one atomic load to poll.
        let whole_comb = self
            .whole_comb
            .as_ref()
            .or_else(|| self.late_aotc.as_ref().and_then(|l| l.whole_comb()));
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

    /// Change-driven settle (`VERYL_INCR=1`): seed dirtiness from
    /// FF-buffer / external-word diffs against the previous settle, then
    /// sweep the topologically-ordered statement list running only dirty
    /// entries, diffing their outputs to propagate.  Semantics match the
    /// baseline fixed-pass evaluation (see `ir::incremental`).
    pub fn settle_comb_incremental(
        &self,
        state: &mut incremental::IncrState,
        mask_cache: &mut MaskCache,
        profile: &mut SimProfile,
    ) {
        use incremental::read_word;
        // Auto-abandoned plan (see `incremental::abandon_threshold_pct`):
        // the DUT's activity makes change-driven bookkeeping a net loss —
        // run the baseline full sweep instead.  (Event skip is disabled by
        // the same flag in `eval_event_stmts`.)
        if state.abandoned {
            self.settle_comb(mask_cache, profile);
            return;
        }
        let plan = self
            .incr_plan
            .as_ref()
            .expect("settle_comb_incremental requires incr_plan");
        debug_assert_eq!(plan.n_entries, self.comb_statements.len());
        // Every id-indexed structure below belongs to one generation; a
        // state carried over from another plan would index silently wrong
        // (fused contract v2 §1).
        debug_assert_eq!(
            state.bound_plan,
            std::ptr::from_ref::<incremental::IncrPlan>(plan) as usize,
            "incremental state is bound to a different plan generation"
        );

        #[cfg(feature = "profile")]
        {
            profile.settle_comb_count += 1;
        }
        let _ = profile;

        let comb: &[u8] = &self.comb_values;
        let ff: &[u8] = &self.ff_values;
        let ff_words = plan.total_words - plan.comb_words;

        let first_settle = !state.inited;
        if first_settle {
            // First settle: run everything through the sweep below (marking
            // every entry each pass) so the per-entry output snapshots are
            // populated with each entry's own values in schedule order.
            state.prev_ff = vec![0u64; ff_words];
            state.prev_ext = vec![0u64; plan.ext_comb_words.len()];
            state.dirty = vec![0u64; plan.n_entries.div_ceil(64)];
            // All-dirty so every event chunk runs its first fire.
            state.event_dirty = vec![!0u64; plan.event_chunk_count.div_ceil(64)];
            state.sub_mask = vec![0u8; plan.n_entries];
            state.build_flat(plan, &self.comb_statements);
            state.inited = true;
        }

        let mut seed_words = 0u64;
        // Full scans when an untracked writer may have touched the buffers
        // (or on the first settle); the dirty-seed lists cover the tracked
        // every-cycle paths.
        let force_full = first_settle || state.seed_full;
        if force_full {
            if state.seed_full {
                state.stats_seed_full += 1;
            }
            state.seed_full = false;
            state.pending_ff.clear();
            state.pending_ext.clear();
            // Both seed scans below run in blocks of 8 with an xor-or reduction
            // and only fall into the per-word marking path when the block
            // differs: ~99.5% of words are unchanged in a typical settle, so the
            // scans are dominated by confirming "no change" (branchless in the
            // common case; the changed block re-reads at most 8 words).
            //
            // Seed: external comb words (event writes, testbench/root vars).
            // Wakes both readers and writers: the settled version must be
            // re-established over an external write, like the baseline's
            // unconditional evaluation would.
            let ext_words = &plan.ext_comb_words;
            let mut i = 0usize;
            while i < ext_words.len() {
                let end = (i + 8).min(ext_words.len());
                let mut acc = 0u64;
                for (&w, &p) in ext_words[i..end].iter().zip(&state.prev_ext[i..end]) {
                    acc |= read_word(comb, w as usize) ^ p;
                }
                if acc != 0 {
                    for (k, &wid) in ext_words.iter().enumerate().take(end).skip(i) {
                        let w = wid as usize;
                        let v = read_word(comb, w);
                        let p = state.prev_ext[k];
                        if v != p {
                            let dgm = incremental::byte_nonzero_mask(v ^ p);
                            state.prev_ext[k] = v;
                            seed_words += 1;
                            plan.mark_word(
                                &mut state.sink(),
                                incremental::MarkSource::ExtSeed,
                                w,
                                dgm,
                            );
                        }
                    }
                }
                i = end;
            }
            // Seed: FF words (event/commit writes since the previous settle).
            let mut w0 = 0usize;
            while w0 < ff_words {
                let end = (w0 + 8).min(ff_words);
                let mut acc = 0u64;
                for w in w0..end {
                    acc |= read_word(ff, w) ^ state.prev_ff[w];
                }
                if acc != 0 {
                    for lw in w0..end {
                        let v = read_word(ff, lw);
                        let p = state.prev_ff[lw];
                        if v != p {
                            let dgm = incremental::byte_nonzero_mask(v ^ p);
                            state.prev_ff[lw] = v;
                            seed_words += 1;
                            plan.mark_word(
                                &mut state.sink(),
                                incremental::MarkSource::FfSeed,
                                plan.comb_words + lw,
                                dgm,
                            );
                        }
                    }
                }
                w0 = end;
            }
        } else {
            // Dirty-seed path: only words some tracked writer touched since
            // the last settle (commit compare-on-apply, event on-run diff,
            // partial-settle diff, input-clock toggles).  Diff semantics
            // and marking are identical to the full scans; duplicates are
            // harmless (the first diff updates prev, the second no-ops).
            let mut pend = std::mem::take(&mut state.pending_ext);
            for &w in &pend {
                let w = w as usize;
                let pi = plan.ext_pos[w] as usize;
                if pi == 0 {
                    continue;
                }
                let v = read_word(comb, w);
                let p = state.prev_ext[pi - 1];
                if v != p {
                    let dgm = incremental::byte_nonzero_mask(v ^ p);
                    state.prev_ext[pi - 1] = v;
                    seed_words += 1;
                    plan.mark_word(&mut state.sink(), incremental::MarkSource::ExtSeed, w, dgm);
                }
            }
            pend.clear();
            state.pending_ext = pend;

            let mut pend = std::mem::take(&mut state.pending_ff);
            for &lw in &pend {
                let lw = lw as usize;
                if lw >= ff_words {
                    continue;
                }
                let v = read_word(ff, lw);
                let p = state.prev_ff[lw];
                if v != p {
                    let dgm = incremental::byte_nonzero_mask(v ^ p);
                    state.prev_ff[lw] = v;
                    seed_words += 1;
                    plan.mark_word(
                        &mut state.sink(),
                        incremental::MarkSource::FfSeed,
                        plan.comb_words + lw,
                        dgm,
                    );
                }
            }
            pend.clear();
            state.pending_ff = pend;
        }
        state.stats_seed_words += seed_words;

        for _pass in 0..plan.required_passes {
            if first_settle {
                for d in state.dirty.iter_mut() {
                    *d = !0;
                }
            } else {
                for &e in &plan.always_run {
                    state.dirty[e as usize / 64] |= 1u64 << (e % 64);
                }
            }
            let ran = self.incr_sweep_pass(state, plan, mask_cache, first_settle);
            if !ran {
                break;
            }
        }
        // Leftover dirty bits are marks that landed BEHIND the sweep in the
        // last pass — backward edges.  Baseline stops after its fixed pass
        // count too, but it re-runs EVERY entry next settle, so a backward
        // reader picks the new value up one settle late; the equivalent
        // here is to CARRY the marks (dirty + sub-mask) into the next
        // settle's first pass, not to drop them.  Dropping is only
        // value-neutral when every backward mark targets an always-run
        // entry (the versioned-word protection) — pe_core happens to
        // satisfy that, but a schedule with a reader ordered before a
        // same-word writer (relaxed false-cycle SCCs, under-covered
        // dynamic-array edges) produces plain backward dataflow marks,
        // and dropping those freezes the reader on a stale value forever
        // (found on heliodor's OoO dcache at fine chunk granularity).
        // Running the leftovers with extra sweeps in THIS settle is wrong
        // in the other direction: they would read values the baseline only
        // sees next settle.
        // Entries may have rewritten external words; refresh the snapshot so
        // the next seed diff doesn't re-trigger on our own writes.  A full
        // re-read beats keeping every ext word in the diff records (the
        // records are the sweep's hottest data).
        for (i, &w) in plan.ext_comb_words.iter().enumerate() {
            state.prev_ext[i] = read_word(comb, w as usize);
        }
        // Match the seed baselines to the settled state (first settle).
        if first_settle {
            for (w, p) in state.prev_ff.iter_mut().enumerate() {
                *p = read_word(ff, w);
            }
        }
        state.stats_settles += 1;
        state.gen_settles += 1;
        // Diagnostic (`VERYL_INCR_REBIND`): queue an identity generation
        // swap so the suites exercise the swap path.
        {
            let iv = incremental::rebind_interval();
            if iv > 0 && state.gen_settles.is_multiple_of(iv) {
                state.request_swap(Some(std::sync::Arc::clone(plan)));
            }
        }
        // Auto-abandon evaluation: entry-run fraction over the warmup
        // window (see `incremental::abandon_threshold_pct`).  One-shot and
        // permanent — a high-activity DUT does not become low-activity.
        // The clock is generation-local: a swap re-arms the window, since
        // the run fraction it measures is a property of the plan.
        {
            use incremental::{ABANDON_WARMUP, ABANDON_WINDOW, abandon_threshold_pct};
            let pct = abandon_threshold_pct();
            if pct > 0 && !state.abandoned {
                if state.gen_settles == ABANDON_WARMUP {
                    state.abandon_runs0 = state.stats_runs;
                } else if state.gen_settles == ABANDON_WARMUP + ABANDON_WINDOW {
                    let runs = state.stats_runs - state.abandon_runs0;
                    let possible = ABANDON_WINDOW * plan.n_entries as u64;
                    if runs * 100 > possible * pct {
                        state.abandoned = true;
                        log::info!(
                            "incremental plan abandoned: run fraction {:.1}% over settles \
                             {ABANDON_WARMUP}..{} exceeds {pct}% (falling back to full settle)",
                            runs as f64 * 100.0 / possible as f64,
                            ABANDON_WARMUP + ABANDON_WINDOW,
                        );
                        // Under the generation model this verdict is a
                        // retirement (`request_swap(None)` at the swap
                        // point); it stays a flag until P6, because the
                        // event dispatch must keep honouring it in the
                        // window between the verdict and the swap.
                        //
                        // The fallback is permanent — start the deferred
                        // whole-module AOT-C compile so the full settle /
                        // full event eval get their default-pipeline
                        // backends back (see `backend::late`).
                        if let Some(late) = self.late_aotc.as_ref() {
                            crate::backend::late::LateAotc::spawn(late);
                        }
                    }
                }
            }
        }
    }

    /// One sweep over the dirty-entry bitmap in schedule order; returns
    /// whether any entry ran.  See `settle_comb_incremental` for the
    /// surrounding pass/drain structure.
    fn incr_sweep_pass(
        &self,
        state: &mut incremental::IncrState,
        plan: &incremental::IncrPlan,
        mask_cache: &mut MaskCache,
        force_full_subs: bool,
    ) -> bool {
        use incremental::read_word;
        let comb: &[u8] = &self.comb_values;
        let ff: &[u8] = &self.ff_values;
        let incremental::IncrState {
            dirty,
            sub_mask,
            blob,
            blob_off,
            arg_sets,
            prev_ff,
            prev_ext,
            event_dirty,
            pending_ff,
            pending_ext,
            seed_full,
            last_run,
            stats_runs,
            stats_runs_nochange,
            stats_settles,
            stats_sub_exec,
            stats_sub_possible,
            stats_sub_full_runs,
            stats_sub_masked_exec,
            stats_sub_masked_possible,
            stats_sub_raw_exec,
            stats_event_marks,
            ..
        } = state;
        {
            let mut any = false;
            for widx in 0..dirty.len() {
                // Re-read after each run so forward marks into this same
                // word are picked up within the pass.  `processed` blocks
                // everything at or below the scan position: entry id equals
                // schedule position, so a mark that lands BEHIND the scan
                // (same word or an earlier one) can only come from a
                // later-scheduled entry — a backward edge, which under the
                // baseline's fixed pass count is next-pass work.  Running
                // it late in this pass instead would read future values
                // and re-clobber later writers' final versions (this was a
                // real divergence on pe_core: a mid-chain writer of a
                // versioned word woken by a later reader's output change
                // overwrote the chain's final version).  The bit stays set
                // in `dirty` for the next pass, matching baseline pass
                // semantics.
                let mut processed = 0u64;
                loop {
                    let b = dirty[widx] & !processed;
                    if b == 0 {
                        break;
                    }
                    let t = b.trailing_zeros() as usize;
                    dirty[widx] &= !(1u64 << t);
                    processed |= (1u64 << t) | ((1u64 << t) - 1);
                    let e = widx * 64 + t;
                    if e >= plan.n_entries {
                        break;
                    }
                    any = true;
                    *stats_runs += 1;
                    if incremental::validate_enabled() {
                        last_run.resize(plan.n_entries, 0);
                        last_run[e] = *stats_settles + 1;
                    }

                    // Resolve the requested-sub mask: expand each requested
                    // sub through the intra-chunk solidarity closure; 0
                    // (always-run marks, first settle) means the whole
                    // entry.  `sub_m` is stored into the JIT-side mask slot
                    // below (WRITE_LOG_OFFSET_INCR_SUB_MASK) and drives the
                    // post-run `ran_m` diff coverage.
                    let full = plan.full_mask[e];
                    let raw = std::mem::take(&mut sub_mask[e]);
                    let sub_m = if force_full_subs || raw == 0 {
                        // The first settle populates every entry's output
                        // snapshot in schedule order — a mark accumulated
                        // mid-sweep must not narrow it to a partial run.
                        *stats_sub_full_runs += 1;
                        full
                    } else {
                        let ex = &plan.sub_expand[e];
                        let mut m = raw & full;
                        *stats_sub_raw_exec += m.count_ones() as u64;
                        let mut bits = m;
                        while bits != 0 {
                            let t = bits.trailing_zeros() as usize;
                            bits &= bits - 1;
                            m |= ex[t];
                        }
                        m &= full;
                        *stats_sub_masked_exec += m.count_ones() as u64;
                        *stats_sub_masked_possible += full.count_ones() as u64;
                        m
                    };
                    *stats_sub_exec += sub_m.count_ones() as u64;
                    *stats_sub_possible += full.count_ones() as u64;

                    // Software-pipelined prefetch: the next dirty entry in
                    // this word is known now, so pull its record line in
                    // while the current entry runs, and its code line in
                    // before its call.  The per-run fixed cost is
                    // miss-dominated (record + call target), and the call
                    // below gives the prefetches time to land.
                    let next_rec: Option<usize> = {
                        let b2 = dirty[widx] & !processed;
                        if b2 != 0 {
                            let e2 = widx * 64 + b2.trailing_zeros() as usize;
                            if e2 < plan.n_entries {
                                let r = blob_off[e2] as usize;
                                prefetch_read(blob[r..].as_ptr());
                                Some(r)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    };

                    // Run + bookkeeping off the flat record (see
                    // `IncrState::build_flat` for the layout): one
                    // sequential blob stream per entry — call target and
                    // args, unconditional repair marks, then per-output
                    // `{src, prev, marks}` with the diff baseline stored
                    // inline and consumer wakes pre-merged into
                    // (dirty word, mask) OR pairs.
                    let off0 = blob_off[e] as usize;
                    let func_bits = blob[off0];
                    let counts = blob[off0 + 1];
                    if func_bits != 0 {
                        // SAFETY: bits were captured from a live
                        // `Statement::Compiled` whose artifact (and the
                        // buffers its args point into) is kept alive by
                        // `self.comb_statements` for `self`'s lifetime.
                        unsafe {
                            let a = arg_sets[(counts >> 56) as usize];
                            if func_bits & 1 != 0 {
                                // Sub-guarded chunk (flag in bit 0): hand the
                                // requested-sub mask to the chunk prologue
                                // through the write-log header slot.
                                let slot = (a.2 as usize
                                    + write_log::WRITE_LOG_OFFSET_INCR_SUB_MASK as usize)
                                    as *mut u32;
                                *slot = sub_m as u32;
                            }
                            let f: crate::FuncPtr = std::mem::transmute((func_bits & !1) as usize);
                            f(
                                a.0 as usize as *const u8,
                                a.1 as usize as *const u8,
                                a.2 as usize as *mut u8,
                                a.3 as isize,
                            );
                        }
                    } else {
                        dispatch_stmt_fast(&self.comb_statements[e], mask_cache);
                    }
                    // The next record line should have arrived during the
                    // call; now warm the next entry's code so its call
                    // doesn't stall on icache misses (chunks span several
                    // lines and the settle is icache-fetch bound).  Warm a
                    // fixed leading window of the chunk body.
                    if let Some(r) = next_rec {
                        let nf = blob[r] & !1;
                        if nf != 0 {
                            let c = nf as usize as *const u8;
                            let mut o = 0usize;
                            while o < 256 {
                                prefetch_read(c.wrapping_add(o));
                                o += 64;
                            }
                        }
                    }
                    // Which subs actually executed: the requested mask for a
                    // guarded call, everything otherwise (interpreted entry,
                    // arg-set fallback, or an unguarded artifact) — the diff
                    // below must cover exactly the words that may have been
                    // written, or the own-output snapshots go stale.
                    let ran_m = if func_bits & 1 != 0 {
                        sub_m
                    } else {
                        plan.full_mask[e]
                    };
                    let n_repair = counts as u32 as usize;
                    let n_out = (counts >> 32) as usize & 0xff_ffff;
                    let p = off0 + 2;
                    // The run may have put an intermediate version on top of
                    // a later writer's value; re-establish those regardless
                    // of any value change.
                    for m in 0..n_repair {
                        let enc = (blob[p + (m >> 1)] >> ((m & 1) * 32)) as u32;
                        let id = (enc & incremental::ENTRY_ID_MASK) as usize;
                        dirty[id / 64] |= 1u64 << (id % 64);
                        sub_mask[id] |= (enc >> incremental::SUB_MASK_SHIFT) as u8;
                    }
                    // Diff against this entry's own previous outputs, off
                    // the three fixed-stride arrays (`build_flat` layout) —
                    // iterations are address-independent so loads pipeline;
                    // the mark pool is only touched when a word changed.
                    let hdr = p + n_repair.div_ceil(2);
                    let idx = hdr + n_out;
                    let prev = hdr + 2 * n_out;
                    let mut any_out_changed = false;
                    for i in 0..n_out {
                        let h = blob[hdr + i];
                        // A word none of the executed subs write is skipped:
                        // it may hold another entry's version, and diffing it
                        // would corrupt this entry's own-output snapshot.
                        if (h >> 56) as u8 & ran_m == 0 {
                            continue;
                        }
                        let src = h as u32;
                        let nm = (h >> 32) as usize & 0xff_ffff;
                        let tail = src & incremental::OUT_SRC_TAIL != 0;
                        let v = if !tail {
                            let base = if src & incremental::OUT_SRC_FF == 0 {
                                comb.as_ptr()
                            } else {
                                ff.as_ptr()
                            };
                            let off = (src & !incremental::OUT_SRC_FF) as usize;
                            // SAFETY: plan build verified off + 8 <= buf len.
                            u64::from_le(unsafe { (base.add(off) as *const u64).read_unaligned() })
                        } else {
                            let w = (blob[idx + i] >> 32) as usize;
                            if w < plan.comb_words {
                                read_word(comb, w)
                            } else {
                                read_word(ff, w - plan.comb_words)
                            }
                        };
                        if v != blob[prev + i] {
                            any_out_changed = true;
                            // Wake only consumers whose read bytes overlap
                            // the changed bytes: packed struct fields share
                            // words, and a neighbour-field change is not a
                            // trigger for this reader.
                            let dgm = incremental::byte_nonzero_mask(v ^ blob[prev + i]);
                            blob[prev + i] = v;
                            let h1 = blob[idx + i];
                            let w = (h1 >> 32) as usize;
                            if w >= plan.comb_words {
                                // Keep the FF snapshot current so the next
                                // settle doesn't double-trigger.
                                prev_ff[w - plan.comb_words] = v;
                            } else {
                                // Same for ext words: the settle's own write
                                // must not re-trigger the next seed diff.
                                let ep = plan.ext_pos[w] as usize;
                                if ep != 0 {
                                    prev_ext[ep - 1] = v;
                                }
                            }
                            let marks = h1 as u32 as usize;
                            let gm_base = marks + nm.div_ceil(2);
                            for m in 0..nm {
                                let gm = (blob[gm_base + (m >> 3)] >> ((m & 7) * 8)) as u8;
                                if gm & dgm == 0 {
                                    continue;
                                }
                                let enc = (blob[marks + (m >> 1)] >> ((m & 1) * 32)) as u32;
                                let id = (enc & incremental::ENTRY_ID_MASK) as usize;
                                dirty[id / 64] |= 1u64 << (id % 64);
                                sub_mask[id] |= (enc >> incremental::SUB_MASK_SHIFT) as u8;
                            }
                            plan.mark_word(
                                &mut incremental::MarkSink {
                                    dirty: &mut *dirty,
                                    sub_mask: &mut *sub_mask,
                                    event_dirty: &mut *event_dirty,
                                    pending_ff: &mut *pending_ff,
                                    pending_ext: &mut *pending_ext,
                                    seed_full: &mut *seed_full,
                                    stats_event_marks: &mut *stats_event_marks,
                                },
                                incremental::MarkSource::OutDiff,
                                w,
                                dgm,
                            );
                        }
                    }
                    if !any_out_changed {
                        *stats_runs_nochange += 1;
                    }
                }
            }
            any
        }
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
/// Best-effort read prefetch of the cache line holding `p` (no-op off x86).
#[inline(always)]
pub(crate) fn prefetch_read<T>(p: *const T) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: prefetch never faults, even on invalid addresses.
    unsafe {
        core::arch::x86_64::_mm_prefetch(p as *const i8, core::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = p;
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
    /// File persisting the runtime-infeasible comb keys across processes
    /// (see `backend::late`): a DUT whose incremental plan was abandoned
    /// once skips the incremental conv configuration in every later run.
    /// Keys are structural fingerprints, so a changed DUT (or compiler)
    /// simply never matches — stale entries are inert.  `None` (unit
    /// tests, wasm) keeps the verdict process-local.
    pub incr_feedback_path: Option<PathBuf>,
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
