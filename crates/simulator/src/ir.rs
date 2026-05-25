mod context;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod dead_var_dce;
mod declaration;
#[cfg(not(target_family = "wasm"))]
mod dup_assign_dce;
mod event;
mod expression;
pub(crate) mod inst_layout;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod load_cache_lookahead;
mod module;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod multi_write_analysis;
pub mod schedule;
pub(crate) mod site_table;
mod statement;
mod variable;
pub(crate) mod write_log;

pub use context::{Context, Conv};
pub use declaration::ProtoDeclaration;
pub use event::Event;
pub use expression::{Expression, ExpressionContext, ProtoDynamicBitSelect, ProtoExpression};
pub use module::{Module, ProtoModule};
pub use statement::{
    CompiledBlockStatement, ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoForBound,
    ProtoForRange, ProtoForStatement, ProtoIfStatement, ProtoStatement, ProtoStatementBlock,
    ProtoStatements, ProtoSystemFunctionCall, RuntimeForBound, RuntimeForRange, Statement,
    SystemFunctionCall, TbMethodKind, format_assert_message, parse_hex_content, patch_stmt_log_buf,
};
pub use variable::{
    ModuleVariableMeta, ModuleVariables, VarOffset, Variable, VariableElement, VariableMeta,
    create_variable_meta, native_bytes, read_native_value, read_payload, value_size,
    write_native_value, write_payload,
};
pub use veryl_analyzer::ir::{Op, Type, VarId, VarPath};
pub use veryl_analyzer::value::Value;

use crate::HashMap;
#[cfg(not(target_family = "wasm"))]
use crate::aot_c::EmittedModule;
use crate::simulator::SimProfile;
use crate::simulator_error::SimulatorError;
#[cfg(not(target_family = "wasm"))]
use memmap2::Mmap;
use std::sync::Arc;
use std::sync::OnceLock;
use veryl_analyzer::ir as air;
use veryl_analyzer::value::MaskCache;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

/// Lazily-published handle to a compiled AOT-C `.so`.  `cell.get()` is `None`
/// while the background compile is still running and `Some` once the module is
/// ready; callers fall back to Cranelift until then.  Shared via `Arc` so the
/// `Ir` instances built from one cached `ProtoModule` reference the same `.so`.
#[cfg(not(target_family = "wasm"))]
pub type AotCell = Arc<OnceLock<EmittedModule>>;

#[cfg(not(target_family = "wasm"))]
type BinaryStorage = Mmap;
#[cfg(target_family = "wasm")]
type BinaryStorage = ();

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
    /// pointer baked into each `Statement::Binary` at construction.
    pub write_log_buffer: Box<write_log::WriteLogBuffer>,
    /// Whether FF classification optimization is disabled.
    pub disable_ff_opt: bool,
    /// Sensitivity-fanout + topological-rank index for the seeded-worklist
    /// comb settle. Populated from the pre-JIT ProtoStatement list at
    /// ProtoModule build time.
    pub comb_schedule: schedule::IrSchedule,
    /// Runtime toggle for the seeded-worklist comb settle path. Snapshotted
    /// from Config at Ir construction time so settle_comb can dispatch
    /// without re-reading Config.
    pub use_seeded_worklist: bool,
    /// Diagnostic: number of nontrivial SCCs found in the pre-JIT comb
    /// graph.  Real combinational loops are rejected by `analyze_dependency`,
    /// so any non-zero value here indicates duplicate ProtoStatements in
    /// the simulator IR assembly.  See `Module::nontrivial_comb_scc`.
    pub nontrivial_comb_scc: usize,
    /// AOT-C comb dispatch handle (see `AotCell`).  `Some` once
    /// `ProtoModule::conv` attempts the cc backend — `Config::aot_c` set and
    /// every comb stmt covered; `None` keeps `settle_comb` on the per-chunk
    /// Cranelift loop.
    #[cfg(not(target_family = "wasm"))]
    pub aot_c_eval: Option<AotCell>,
    /// Snapshotted from `Config::aot_c_validate`: when set, `settle_comb` /
    /// `step` dual-run the AOT-C and Cranelift paths and panic on divergence.
    pub aot_c_validate: bool,
    /// AOT-C event-path dispatch handles, keyed by `Event`.  When the
    /// current event's `cell.get()` is ready, `step()` invokes the
    /// gcc-compiled FF-next + write-log function instead of the per-stmt
    /// Cranelift dispatch.  Built in `ProtoModule::conv` when
    /// `Config::aot_c_event` is set and the emitter covered every event stmt.
    #[cfg(not(target_family = "wasm"))]
    pub aot_c_event_evals: crate::HashMap<Event, AotCell>,
    /// Keeps JIT-compiled code alive. Wrapped in `Arc` so that multiple `Ir`
    /// instances created from the same cached `ProtoModule` can share the binary.
    _binary: Arc<Vec<BinaryStorage>>,
}

impl Ir {
    pub fn from_module(
        module: Module,
        binary: Vec<BinaryStorage>,
        config: &Config,
        token: TokenRange,
    ) -> Ir {
        Ir::from_module_arc(module, Arc::new(binary), config, token)
    }

    pub fn from_module_arc(
        module: Module,
        binary: Arc<Vec<BinaryStorage>>,
        config: &Config,
        token: TokenRange,
    ) -> Ir {
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
            comb_schedule: module.comb_schedule,
            use_seeded_worklist: config.use_seeded_worklist,
            nontrivial_comb_scc: module.nontrivial_comb_scc,
            #[cfg(not(target_family = "wasm"))]
            aot_c_eval: module.aot_c_eval,
            aot_c_validate: config.aot_c_validate,
            #[cfg(not(target_family = "wasm"))]
            aot_c_event_evals: module.aot_c_event_evals,
            _binary: binary,
        };
        // Bake the WriteLogBuffer's heap-stable address into every
        // JIT-dispatched Binary/BinaryBatch so emitted code can perform
        // inline log pushes without a TLS lookup.
        ir.install_write_log_ptr();
        ir
    }

    /// Walk every event/comb statement tree and overwrite the placeholder
    /// `log_buf` field in `Statement::Binary` / `Statement::BinaryBatch`
    /// with the actual heap address of `self.write_log_buffer`.
    ///
    /// Called once at the end of `from_module_arc`.  The address is
    /// stable for `self`'s lifetime because the buffer lives on the heap
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
    pub fn settle_comb(
        &self,
        mask_cache: &mut MaskCache,
        snapshot_buf: &mut Vec<u8>,
        profile: &mut SimProfile,
    ) {
        #[cfg(feature = "profile")]
        {
            profile.settle_comb_count += 1;
        }
        let _ = profile; // suppress unused warning when profile feature is off

        // Dispatch: when AOT-C built a comb eval function and
        // VERYL_AOT_C_VALIDATE is unset, swap in the gcc-compiled
        // function in place of per-chunk Cranelift dispatch.  When
        // VERYL_AOT_C_VALIDATE=1 we additionally run BOTH backends and
        // panic on first divergence so the broken stmt can be
        // identified.  Both paths fall through to Cranelift if
        // `aot_c_eval` is None.
        #[cfg(not(target_family = "wasm"))]
        if let Some(aot) = self.aot_c_eval.as_ref().and_then(|c| c.get()) {
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
                // Common case: passes == 1 (no SCC backward edges).  The
                // compiler cannot const-fold `passes` since it comes from a
                // runtime field, so without this specialization we pay the
                // loop counter+branch overhead on every cycle.
                if passes == 1 {
                    unsafe {
                        (aot.func)(ff_ptr, comb_ptr as *const u8, log_ptr);
                    }
                } else {
                    for _ in 0..passes {
                        unsafe {
                            (aot.func)(ff_ptr, comb_ptr as *const u8, log_ptr);
                        }
                    }
                }
                return;
            }

            // Validate path: snapshot inputs, run AOT-C, snapshot AOT-C
            // outputs, restore inputs, fall through to Cranelift, compare.
            let ff_snap_in: Vec<u8> = self.ff_values.to_vec();
            let comb_snap_in: Vec<u8> = self.comb_values.to_vec();
            let count_snap_in: u64 = self.write_log_buffer.count() as u64;

            for _ in 0..passes {
                unsafe {
                    (aot.func)(ff_ptr, comb_ptr as *const u8, log_ptr);
                }
            }

            let ff_aot_out: Vec<u8> = self.ff_values.to_vec();
            let comb_aot_out: Vec<u8> = self.comb_values.to_vec();
            let count_aot_out: u64 = self.write_log_buffer.count() as u64;

            // Restore inputs so the JIT path runs on the same starting state.
            // Both AOT-C and Cranelift mutate buffers via raw pointers, so
            // mirroring that pattern keeps the borrow rules consistent.
            unsafe {
                let ff_dst = self.ff_values.as_ptr() as *mut u8;
                std::ptr::copy_nonoverlapping(ff_snap_in.as_ptr(), ff_dst, ff_snap_in.len());
                let comb_dst = self.comb_values.as_ptr() as *mut u8;
                std::ptr::copy_nonoverlapping(comb_snap_in.as_ptr(), comb_dst, comb_snap_in.len());
            }
            // AOT-C comb eval does not write the log, so its entry count
            // is unchanged from the snapshot; no count restore needed.
            let _ = count_snap_in;

            // Run the JIT path on the restored inputs, then diff its result
            // against the AOT-C snapshot and panic on any mismatch.
            self.run_cranelift_settle(mask_cache, snapshot_buf, profile);

            let ff_jit_out: &[u8] = &self.ff_values;
            let comb_jit_out: &[u8] = &self.comb_values;
            let count_jit_out: u64 = self.write_log_buffer.count() as u64;

            let mut diverged = false;
            if comb_aot_out.as_slice() != comb_jit_out {
                let off = comb_aot_out
                    .iter()
                    .zip(comb_jit_out.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(usize::MAX);
                let var_info = lookup_comb_offset(&self.module_variables, comb_ptr, off);
                // Print the input snapshot bytes around the diverging
                // offset in 4-byte words so we can compare what the
                // upstream stmts read.  Both backends started from
                // `comb_snap_in`, so this is the shared input state.
                let dump_word = |snap: &[u8], byte_off: isize, name: &str| {
                    let abs = (off as isize + byte_off) as usize;
                    if abs + 4 <= snap.len() {
                        let w = u32::from_le_bytes(snap[abs..abs + 4].try_into().unwrap_or([0; 4]));
                        eprintln!("  snap[{:+}] ({}) = 0x{:08x} (u32)", byte_off, name, w,);
                    }
                };
                eprintln!(
                    "VERYL_AOT_C_VALIDATE: comb_values diverge at offset {} \
                     (AOT-C={:#x}, JIT={:#x}, len={}) var={}",
                    off,
                    comb_aot_out.get(off).copied().unwrap_or(0),
                    comb_jit_out.get(off).copied().unwrap_or(0),
                    comb_aot_out.len(),
                    var_info,
                );
                // Generic input dump: ±64 bytes around the divergence, in
                // 4-byte words.
                eprintln!("  input snapshot (relative to diverge byte):");
                for delta in (-64..=64).step_by(4) {
                    dump_word(&comb_snap_in, delta as isize, "comb");
                }
                eprintln!("  AOT-C output around diverge byte:");
                for delta in (-64..=64).step_by(4) {
                    dump_word(&comb_aot_out, delta as isize, "aot");
                }
                eprintln!("  JIT output around diverge byte:");
                for delta in (-64..=64).step_by(4) {
                    let w = comb_jit_out
                        .get(
                            ((off as isize + delta) as usize)
                                ..((off as isize + delta + 4) as usize),
                        )
                        .map(|s| u32::from_le_bytes(s.try_into().unwrap_or([0; 4])))
                        .unwrap_or(0);
                    eprintln!("  out[{:+}] (jit) = 0x{:08x} (u32)", delta, w);
                }
                // ALL diverging bytes (not just the first), grouped
                // by contiguous run.  Helps when the first byte is
                // a downstream effect of an earlier divergence.
                eprintln!("  ALL diverging byte ranges:");
                let mut run_start: Option<usize> = None;
                let mut count = 0usize;
                let max_runs = 32usize;
                let pairs: Vec<(usize, u8, u8)> = comb_aot_out
                    .iter()
                    .zip(comb_jit_out.iter())
                    .enumerate()
                    .filter_map(|(i, (a, b))| if a != b { Some((i, *a, *b)) } else { None })
                    .collect();
                for (i, &(idx, a, b)) in pairs.iter().enumerate() {
                    let is_contig = run_start.is_some() && i > 0 && pairs[i - 1].0 + 1 == idx;
                    if !is_contig {
                        if let Some(start) = run_start {
                            let end = pairs[i - 1].0;
                            let info = lookup_comb_offset(&self.module_variables, comb_ptr, start);
                            eprintln!(
                                "    [{}-{}] ({}B) at var={}",
                                start,
                                end,
                                end - start + 1,
                                info,
                            );
                            count += 1;
                            if count >= max_runs {
                                eprintln!(
                                    "    ... ({} more diverging bytes total)",
                                    pairs.len() - (i)
                                );
                                break;
                            }
                        }
                        run_start = Some(idx);
                    }
                    let _ = (a, b);
                }
                if let Some(start) = run_start
                    && count < max_runs
                    && let Some(&(end, _, _)) = pairs.last()
                {
                    let info = lookup_comb_offset(&self.module_variables, comb_ptr, start);
                    eprintln!(
                        "    [{}-{}] ({}B) at var={}",
                        start,
                        end,
                        end - start + 1,
                        info,
                    );
                }
                diverged = true;
            }
            if ff_aot_out.as_slice() != ff_jit_out {
                let off = ff_aot_out
                    .iter()
                    .zip(ff_jit_out.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(usize::MAX);
                eprintln!(
                    "VERYL_AOT_C_VALIDATE: ff_values diverge at offset {} \
                     (AOT-C={:#x}, JIT={:#x}, len={})",
                    off,
                    ff_aot_out.get(off).copied().unwrap_or(0),
                    ff_jit_out.get(off).copied().unwrap_or(0),
                    ff_aot_out.len(),
                );
                diverged = true;
            }
            if count_aot_out != count_jit_out {
                eprintln!(
                    "VERYL_AOT_C_VALIDATE: write_log count diverges \
                     (AOT-C={}, JIT={})",
                    count_aot_out, count_jit_out,
                );
                diverged = true;
            }
            if diverged {
                panic!("AOT-C / Cranelift divergence in settle_comb");
            }
            return;
        }

        self.run_cranelift_settle(mask_cache, snapshot_buf, profile);
    }

    /// Cranelift-only settle path, factored out so the validate mode can
    /// invoke it after AOT-C eval has run and the buffers have been restored.
    fn run_cranelift_settle(
        &self,
        mask_cache: &mut MaskCache,
        snapshot_buf: &mut Vec<u8>,
        profile: &mut SimProfile,
    ) {
        let _ = profile;

        // Worklist path: event-driven comb evaluation using a seeded
        // worklist built from FF dirty state.  Gated on the worklist
        // config and an alignment check.
        if self.use_seeded_worklist
            && self.comb_schedule.n_stmts as usize == self.comb_statements.len()
        {
            self.eval_comb_worklist(mask_cache, snapshot_buf, profile);
            return;
        }

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

    /// Seeded-worklist comb settle.
    ///
    /// Runs one full pass over every comb statement (the seed pass is always
    /// full — cheap compared to skipping a real dependency), then drives a
    /// worklist using the schedule's fanout index: each iteration re-evaluates
    /// only stmts whose declared-input offsets just changed, pushing their
    /// downstream readers onto the next generation's dirty set.
    ///
    /// The algorithm terminates when no outputs change (`dirty` stays empty)
    /// or when MAX_ITER is hit (logs a warning and returns false — callers
    /// currently ignore the bool so a stale state will show up in correctness
    /// tests rather than silently miscompute).
    ///
    /// Requires `self.comb_schedule.n_stmts == self.comb_statements.len()`,
    /// enforced by the dispatch check in `settle_comb`.
    pub fn eval_comb_worklist(
        &self,
        mask_cache: &mut MaskCache,
        snapshot_buf: &mut Vec<u8>,
        profile: &mut SimProfile,
    ) -> bool {
        use smallvec::SmallVec;
        let _ = profile;
        #[cfg(feature = "profile")]
        let start = std::time::Instant::now();

        let sched = &self.comb_schedule;
        let n = sched.n_stmts as usize;
        let comb_len = self.comb_values.len();

        if n == 0 {
            return true;
        }

        snapshot_buf.resize(comb_len, 0);
        snapshot_buf.copy_from_slice(&self.comb_values);

        for stmt in &self.comb_statements {
            dispatch_stmt_fast(stmt, mask_cache);
        }
        #[cfg(feature = "profile")]
        {
            profile.comb_eval_count += 1;
        }

        let mut dirty: SmallVec<[crate::ir::schedule::StmtId; 32]> = SmallVec::new();
        sched.compute_dirty_from_diff(
            &snapshot_buf[..],
            &self.comb_values[..],
            0..n as crate::ir::schedule::StmtId,
            &mut dirty,
        );

        const MAX_ITER: usize = 128;
        let mut iter = 0;
        let fallback_fullpass = worklist_fullpass_fallback();
        while !dirty.is_empty() && iter < MAX_ITER {
            dirty.sort_by_key(|&id| sched.topo_rank[id as usize]);
            snapshot_buf.copy_from_slice(&self.comb_values);

            let current_dirty = std::mem::take(&mut dirty);
            if fallback_fullpass {
                // Diagnostic: fall back to full pass to verify the first full
                // pass + fixpoint logic.  Isolates bugs in dirty-propagation
                // vs bugs elsewhere.
                for stmt in &self.comb_statements {
                    dispatch_stmt_fast(stmt, mask_cache);
                }
            } else {
                for &id in &current_dirty {
                    dispatch_stmt_fast(&self.comb_statements[id as usize], mask_cache);
                }
            }
            #[cfg(feature = "profile")]
            {
                profile.comb_eval_count += 1;
                profile.extra_pass_count += 1;
            }
            let diff_ids: Box<dyn Iterator<Item = crate::ir::schedule::StmtId>> =
                if fallback_fullpass {
                    Box::new(0..n as crate::ir::schedule::StmtId)
                } else {
                    Box::new(
                        current_dirty
                            .iter()
                            .copied()
                            .collect::<Vec<_>>()
                            .into_iter(),
                    )
                };
            sched.compute_dirty_from_diff(
                &snapshot_buf[..],
                &self.comb_values[..],
                diff_ids,
                &mut dirty,
            );
            iter += 1;
        }

        #[cfg(feature = "profile")]
        {
            profile.eval_comb_full_ns += start.elapsed().as_nanos() as u64;
        }

        if iter >= MAX_ITER {
            log::warn!(
                "worklist comb convergence failed after {} iters ({} stmts, {} initially dirty)",
                MAX_ITER,
                n,
                dirty.len()
            );
            return false;
        }

        // Safety full-pass: disabled by default; set VERYL_WORKLIST_SAFETY=1
        // to run an extra full pass after worklist converges. Keeps a
        // correctness net for cases where the schedule fanout misses an
        // edge, at a significant perf cost.
        if worklist_safety() {
            snapshot_buf.copy_from_slice(&self.comb_values);
            for stmt in &self.comb_statements {
                dispatch_stmt_fast(stmt, mask_cache);
            }
            #[cfg(feature = "profile")]
            {
                profile.comb_eval_count += 1;
                profile.extra_pass_count += 1;
            }
            dirty.clear();
            sched.compute_dirty_from_diff(
                &snapshot_buf[..],
                &self.comb_values[..],
                0..n as crate::ir::schedule::StmtId,
                &mut dirty,
            );
            let mut safety_iter = 0;
            const SAFETY_MAX_ITER: usize = 32;
            while !dirty.is_empty() && safety_iter < SAFETY_MAX_ITER {
                dirty.sort_by_key(|&id| sched.topo_rank[id as usize]);
                snapshot_buf.copy_from_slice(&self.comb_values);
                let current_dirty = std::mem::take(&mut dirty);
                for &id in &current_dirty {
                    dispatch_stmt_fast(&self.comb_statements[id as usize], mask_cache);
                }
                #[cfg(feature = "profile")]
                {
                    profile.comb_eval_count += 1;
                    profile.extra_pass_count += 1;
                }
                sched.compute_dirty_from_diff(
                    &snapshot_buf[..],
                    &self.comb_values[..],
                    current_dirty.iter().copied(),
                    &mut dirty,
                );
                safety_iter += 1;
            }
        }

        // Diagnostic: enable with VERYL_WORKLIST_VERIFY=1 to run a full pass
        // after the worklist claims convergence and warn if anything still
        // changes (indicates a missing fanout edge).
        if worklist_verify() {
            let verify_before: Vec<u8> = self.comb_values.to_vec();
            for stmt in &self.comb_statements {
                dispatch_stmt_fast(stmt, mask_cache);
            }
            if verify_before.as_slice() != &self.comb_values[..] {
                let mut first_diff = None;
                for (i, (a, b)) in verify_before
                    .iter()
                    .zip(self.comb_values.iter())
                    .enumerate()
                {
                    if a != b {
                        first_diff = Some((i, *a, *b));
                        break;
                    }
                }
                log::warn!(
                    "worklist verify: extra full pass changed comb_values (first diff at {:?}, iters={})",
                    first_diff,
                    iter
                );
            }
        }
        true
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
            if s.is_binary() {
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
                if s.is_binary() {
                    jit += 1;
                }
            }
        }
        for s in &self.comb_statements {
            total += 1;
            if s.is_binary() {
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
            if s.is_binary() {
                comb_jit += 1;
            } else {
                comb_interp += 1;
            }
        }
        for stmts in self.event_statements.values() {
            for s in stmts {
                if s.is_binary() {
                    event_jit += 1;
                } else {
                    event_interp += 1;
                }
            }
        }
        (comb_jit, comb_interp, event_jit, event_interp)
    }
}

/// Diagnostic env-var caches consulted on every `settle_comb` invocation.
/// Reading the live env each cycle would call `getenv` millions of times on
/// long runs; the `OnceLock` load amortizes that to a single probe per
/// process.  Default-off in production; the env knobs only kick in for
/// debug runs.
fn worklist_fullpass_fallback() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("VERYL_WORKLIST_FULLPASS_FALLBACK").is_ok())
}
fn worklist_safety() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("VERYL_WORKLIST_SAFETY").is_ok())
}
fn worklist_verify() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("VERYL_WORKLIST_VERIFY").is_ok())
}

/// Inline-friendly dispatch for the per-cycle hot loop.  Handles the
/// common JIT cases (Binary / BinaryBatch) with a direct indirect call
/// and falls back to `Statement::eval_step` for the interpreter path.
///
/// Inlining at the call site removes the (otherwise non-inlined)
/// `Statement::eval_step` function-call frame plus the 10-arm match
/// jump it performs.
#[inline(always)]
pub fn dispatch_stmt_fast(s: &Statement, mask_cache: &mut MaskCache) {
    match s {
        Statement::Binary(func, ff, comb, log_buf) => unsafe {
            func(*ff, *comb, *log_buf);
        },
        Statement::BinaryBatch(func, log_buf, args) => unsafe {
            let f = *func;
            let lb = *log_buf;
            for &(ff, comb) in args {
                f(ff, comb, lb);
            }
        },
        _ => {
            s.eval_step(mask_cache);
        }
    }
}

// SAFETY: Each Ir exclusively owns its ff_values/comb_values buffers.
// Raw pointers in Statements point into these buffers — no cross-Ir aliasing.
// _binary (Arc<Vec<BinaryStorage>>) keeps JIT code pages alive.
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
                ..Default::default()
            };
            let proto: ProtoModule = Conv::conv(&mut context, x)?;
            let module = proto.instantiate();
            return Ok(Ir::from_module(module, context.binary, config, token));
        }
    }
    Err(SimulatorError::TopModuleNotFound {
        module_name: top.to_string(),
    })
}

struct CacheEntry {
    proto: ProtoModule,
    binary: Arc<Vec<BinaryStorage>>,
    token: TokenRange,
}

/// Cache for `ProtoModule` and JIT binaries keyed by top module name.
#[derive(Default)]
pub struct ProtoModuleCache {
    entries: HashMap<StrId, CacheEntry>,
    /// Keeps JIT binary pages alive for the cached ProtoModules.
    shared_binaries: Vec<Arc<Vec<BinaryStorage>>>,
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
        return Ok(Ir::from_module_arc(
            module,
            Arc::clone(&entry.binary),
            config,
            entry.token,
        ));
    }

    // Cache miss: run Conv::conv
    for x in &ir.components {
        if let air::Component::Module(x) = x
            && top == x.name
        {
            let token = x.token;
            let mut context = context::Context {
                config: config.clone(),
                ..Default::default()
            };

            let proto: ProtoModule = Conv::conv(&mut context, x)?;
            let module = proto.instantiate();
            let binary = Arc::new(context.binary);

            let result = Ir::from_module_arc(module, Arc::clone(&binary), config, token);

            cache.shared_binaries.push(Arc::clone(&binary));

            cache.entries.insert(
                top,
                CacheEntry {
                    proto,
                    binary,
                    token,
                },
            );

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
    /// Replace the flat `for stmt in comb_statements` settle with a
    /// FF-seeded worklist that only re-evaluates statements whose declared
    /// inputs just changed. Default false; `Config::apply_env` flips this
    /// to true when the `VERYL_USE_SEEDED_WORKLIST` env var is set.
    pub use_seeded_worklist: bool,
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
    /// Minimum module statement count (comb + event) before `cc` is attempted.
    /// The external compile is a fixed per-module cost; on tiny modules it is
    /// pure overhead and floods the host across the fast suite.  Default 0 (no
    /// floor, so tests exercise the path); `--backend cc` raises it to 256.
    pub aot_c_min_stmts: usize,
}

impl Config {
    /// Apply environment-variable overrides on top of an existing config.
    pub fn apply_env(&mut self) {
        if std::env::var("VERYL_USE_SEEDED_WORKLIST").ok().as_deref() == Some("1") {
            self.use_seeded_worklist = true;
        }
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

/// Whether an external C compiler is available (probes `cc --version`, honoring
/// `VERYL_AOT_CC`).  Used to skip the `cc` backend in `Config::all()` test
/// matrices on hosts without a compiler.
#[cfg(not(target_family = "wasm"))]
pub fn cc_available() -> bool {
    let cc = std::env::var("VERYL_AOT_CC").unwrap_or_else(|_| "cc".to_string());
    std::process::Command::new(cc)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

impl Config {
    pub fn all() -> Vec<Config> {
        let mut ret = vec![];

        #[cfg(not(target_family = "wasm"))]
        let jit_options = [false, true];
        #[cfg(target_family = "wasm")]
        let jit_options = [false];

        for use_4state in [false, true] {
            for use_jit in jit_options {
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
        if cc_available() {
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

/// Walk the variable hierarchy looking for the variable element whose
/// `current_values[i]` raw pointer is `comb_base + offset`.  Returns a
/// human-readable description of `var.path[i]` (or `?` when no element
/// matches).  Used by `VERYL_AOT_C_VALIDATE` to map a divergence offset
/// back to a variable name without depending on `VariableMeta`'s offset
/// table being threaded into `Ir`.
#[cfg(not(target_family = "wasm"))]
fn lookup_comb_offset(vars: &ModuleVariables, comb_base: *const u8, target: usize) -> String {
    // Collect ALL matches whose byte range covers `target`, plus any
    // var whose start is within ±64 bytes of target (gives layout context
    // when the strict-cover check returns nothing or returns the wrong
    // entry).  Returns the first cover-hit as the primary name and
    // appends nearby vars for diagnostics.
    fn walk(
        vars: &ModuleVariables,
        target_addr: usize,
        cover: &mut Vec<String>,
        nearby: &mut Vec<(isize, String)>,
    ) {
        for var in vars.variables.values() {
            for (i, &ptr) in var.current_values.iter().enumerate() {
                let addr = ptr as usize;
                let end = addr + var.native_bytes;
                if (target_addr >= addr) && (target_addr < end) {
                    cover.push(format!(
                        "{}[{}]+{} (w={}, nb={})",
                        var.path,
                        i,
                        target_addr - addr,
                        var.width,
                        var.native_bytes,
                    ));
                }
                let delta = addr as isize - target_addr as isize;
                if delta.unsigned_abs() <= 64 {
                    nearby.push((
                        delta,
                        format!(
                            "{}[{}]@{:+} (w={}, nb={})",
                            var.path, i, delta, var.width, var.native_bytes,
                        ),
                    ));
                }
            }
        }
        for child in &vars.children {
            walk(child, target_addr, cover, nearby);
        }
    }
    let target_addr = comb_base as usize + target;
    let mut cover: Vec<String> = Vec::new();
    let mut nearby: Vec<(isize, String)> = Vec::new();
    walk(vars, target_addr, &mut cover, &mut nearby);
    if cover.is_empty() && nearby.is_empty() {
        return "?".to_string();
    }
    nearby.sort_by_key(|(d, _)| d.abs());
    let primary = cover.first().cloned().unwrap_or_else(|| "?".to_string());
    let cover_n = cover.len();
    let cover_extra = if cover_n > 1 {
        format!(
            " [+{} other covers: {}]",
            cover_n - 1,
            cover[1..].join("; ")
        )
    } else {
        String::new()
    };
    let nearby_str: Vec<String> = nearby.iter().take(16).map(|(_, s)| s.clone()).collect();
    format!(
        "{primary}{cover_extra} | nearby: [{}]",
        nearby_str.join(", "),
    )
}
