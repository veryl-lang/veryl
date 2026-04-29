mod context;
mod declaration;
#[cfg(not(target_family = "wasm"))]
mod dup_assign_dce;
mod event;
mod expression;
mod module;
pub mod schedule;
mod statement;
mod variable;

pub use context::{Context, Conv};
pub use declaration::ProtoDeclaration;
pub use event::Event;
pub use expression::{Expression, ProtoExpression};
pub use module::{Module, ProtoModule};
pub use statement::{
    CompiledBlockStatement, ProtoStatement, ProtoStatementBlock, ProtoStatements, RuntimeForBound,
    RuntimeForRange, Statement, SystemFunctionCall, TbMethodKind, format_assert_message,
    parse_hex_content,
};
pub use variable::{
    ModuleVariableMeta, ModuleVariables, VarOffset, Variable, VariableElement, VariableMeta,
    create_variable_meta, native_bytes, read_native_value, read_payload, value_size,
    write_native_value, write_payload,
};
pub use veryl_analyzer::ir::{Op, Type, VarId, VarPath};
pub use veryl_analyzer::value::Value;

use crate::HashMap;
use crate::simulator::SimProfile;
use crate::simulator_error::SimulatorError;
#[cfg(not(target_family = "wasm"))]
use memmap2::Mmap;
use std::sync::Arc;
use veryl_analyzer::ir as air;
use veryl_analyzer::value::MaskCache;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

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
    /// FF commit entries: (current_offset, value_size) pairs.
    /// After event execution, next → current copy is performed for each entry.
    pub ff_commit_entries: Vec<(usize, usize)>,
    /// Runs of consecutive FFs sharing the specialized size with
    /// stride = 2 * size, as `(start_current_offset, count)`.  The
    /// compile-time stride lets LLVM auto-vectorize the inner loop.
    pub ff_commit_u32_runs: Vec<(u32, u32)>,
    pub ff_commit_u64_runs: Vec<(u32, u32)>,
    /// Entries that don't fit u32/u64 specialization (rare: size 1, 2, 16, 32, etc.).
    pub ff_commit_other: Vec<(usize, usize)>,
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
    /// Runtime feature detection: if true, ff_commit uses an AVX2 shuffle
    /// fast path; otherwise a scalar fallback (which LLVM auto-vectorizes
    /// for the base SSE2 target).
    pub ff_commit_use_avx2: bool,
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
        let mut ff_commit_u32_runs: Vec<(u32, u32)> = Vec::new();
        let mut ff_commit_u64_runs: Vec<(u32, u32)> = Vec::new();
        let mut ff_commit_other: Vec<(usize, usize)> = Vec::new();
        {
            let entries = &module.ff_commit_entries;
            let mut i = 0;
            while i < entries.len() {
                let (off, sz) = entries[i];
                if sz != 4 && sz != 8 {
                    ff_commit_other.push((off, sz));
                    i += 1;
                    continue;
                }
                let stride = sz * 2;
                let mut j = i + 1;
                while j < entries.len() {
                    let (off_j, sz_j) = entries[j];
                    if sz_j != sz || off_j != off + stride * (j - i) {
                        break;
                    }
                    j += 1;
                }
                let count = (j - i) as u32;
                if sz == 4 {
                    ff_commit_u32_runs.push((off as u32, count));
                } else {
                    ff_commit_u64_runs.push((off as u32, count));
                }
                i = j;
            }
        }
        Ir {
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
            ff_commit_entries: module.ff_commit_entries,
            ff_commit_u32_runs,
            ff_commit_u64_runs,
            ff_commit_other,
            ff_commit_use_avx2: detect_avx2(),
            disable_ff_opt: config.disable_ff_opt,
            comb_schedule: module.comb_schedule,
            use_seeded_worklist: config.use_seeded_worklist,
            nontrivial_comb_scc: module.nontrivial_comb_scc,
            _binary: binary,
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
        use std::sync::OnceLock;
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
        let fallback_fullpass = std::env::var("VERYL_WORKLIST_FULLPASS_FALLBACK").is_ok();
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
        if std::env::var("VERYL_WORKLIST_SAFETY").is_ok() {
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
        if std::env::var("VERYL_WORKLIST_VERIFY").is_ok() {
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
        Statement::Binary(func, ff, comb) => unsafe {
            func(*ff, *comb);
        },
        Statement::BinaryBatch(func, args) => unsafe {
            let f = *func;
            for &(ff, comb) in args {
                f(ff, comb);
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

#[cfg(target_arch = "x86_64")]
fn detect_avx2() -> bool {
    std::is_x86_feature_detected!("avx2")
}
#[cfg(not(target_arch = "x86_64"))]
fn detect_avx2() -> bool {
    false
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
}

impl Config {
    /// Apply environment-variable overrides on top of an existing config.
    /// Currently handles `VERYL_USE_SEEDED_WORKLIST`.
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
    }
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

        ret
    }
}
