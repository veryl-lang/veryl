mod context;
mod declaration;
mod event;
mod expression;
mod module;
#[cfg(not(target_family = "wasm"))]
mod optimize;
mod statement;
mod variable;

pub use context::{
    Context, Conv, LOG_KIND_COMB, LOG_KIND_FF, MAX_WRITE_LOG_ENTRIES, WRITE_LOG_ENTRY_STRIDE,
    WRITE_LOG_ENTRY_STRIDE_LOG2, WriteLogBuffer, WriteLogEntry,
};
pub use declaration::ProtoDeclaration;
pub use event::Event;
pub use expression::{Expression, ProtoExpression};
pub use module::{Module, ProtoModule};
pub use statement::{
    ColdChunk, CompiledBlockStatement, ProtoStatement, ProtoStatementBlock, ProtoStatements,
    RuntimeForBound, RuntimeForRange, Statement, SystemFunctionCall, TbMethodKind,
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
use crate::HashSet;
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

/// Precomputed tables for FF-change-based comb activity gating.
/// Maps ff_commit_entry changes to comb chunk activation masks.
pub struct CombActivityGating {
    /// For each ff_commit_entry, bitmask of comb chunks directly affected.
    /// `ff_to_chunk[i]` has bit j set if chunk j reads FF bytes in entry i's range.
    pub ff_to_chunk: Vec<u32>,
    /// For each comb chunk, bitmask of later chunks that depend on it
    /// (via comb read/write overlap). Used for transitive propagation.
    pub chunk_dependents: Vec<u32>,
    /// Bitmask of chunks that must always be evaluated (has DynamicVariable FF reads).
    pub always_active: u32,
    /// Bitmask of chunks activated when cold_dirty flag is set
    /// (reads event-written comb in cold region).
    pub cold_event_active: u32,
    /// Bitmask of chunks activated when event→comb dirty flag is set.
    pub event_comb_changed_active: u32,
    /// Number of comb chunks.
    pub num_chunks: usize,
}

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
    /// Whether FF classification optimization is disabled.
    pub disable_ff_opt: bool,
    /// Cold comb chunks with activity-skip metadata.
    pub cold_chunks: Vec<ColdChunk>,
    /// Size of the hot comb region in bytes.
    pub comb_hot_size: usize,
    /// Byte offset of cold dirty flag in comb_values.
    pub cold_dirty_flag_offset: usize,
    /// Byte offset of event→comb dirty flag in comb_values.
    /// Set by event JIT when writing to any comb variable.
    pub event_comb_dirty_flag_offset: usize,
    /// FF-change-based comb activity gating tables.
    pub comb_activity: Option<CombActivityGating>,
    /// Block-to-statement index mapping for activity gating.
    /// `block_stmt_ranges[i] = (start, end)` means block i covers
    /// comb_statements[start..end].
    pub block_stmt_ranges: Vec<(usize, usize)>,
    /// Heap-allocated write-log buffer for sparse FF commit.
    pub write_log_buffer: Option<Box<WriteLogBuffer>>,
    /// Reverse map: current_offset → ff_commit_entries index.
    pub ff_offset_to_entry: HashMap<usize, usize>,
    /// Comb offsets written by event statements (for snapshot comparison).
    pub event_comb_write_offsets: Vec<usize>,
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
        let comb_activity = Self::build_activity_gating(
            &module.comb_activity_meta,
            &module.ff_commit_entries,
            &module.event_comb_writes,
        );
        // Build block→statement range mapping from activity metadata count.
        // This must match the comb_statements structure.
        // We don't have block structure here, so we rely on Module providing it.
        let block_stmt_ranges = module.block_stmt_ranges.clone();
        let ff_offset_to_entry: HashMap<usize, usize> = module
            .ff_commit_entries
            .iter()
            .enumerate()
            .map(|(i, &(off, _))| (off, i))
            .collect();

        // Disable write-log if any event statement is not JIT-compiled.
        // Interpreted statements write directly to ff_values and bypass the log.
        // Check that all non-Initial event statements are JIT-compiled.
        // Initial events run once at startup and don't go through ff_commit.
        let all_events_jit = module
            .event_statements
            .iter()
            .filter(|(event, _)| !matches!(event, Event::Initial))
            .all(|(_, stmts)| stmts.iter().all(|s| s.is_binary()));
        let _has_write_log = module.write_log_buffer.is_some();
        let write_log_buffer = if all_events_jit {
            module.write_log_buffer
        } else {
            for (event, stmts) in &module.event_statements {
                for (i, s) in stmts.iter().enumerate() {
                    if !s.is_binary() {
                        log::info!(
                            "Write-log: non-JIT event stmt [{:?}][{}]: {}",
                            event,
                            i,
                            s.type_name(),
                        );
                    }
                }
            }
            log::info!("Write-log disabled: not all event statements are JIT-compiled");
            None
        };

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
            disable_ff_opt: config.disable_ff_opt,
            cold_chunks: module.cold_chunks,
            comb_hot_size: module.comb_hot_size,
            cold_dirty_flag_offset: module.cold_dirty_flag_offset,
            event_comb_dirty_flag_offset: module.event_comb_dirty_flag_offset,
            comb_activity,
            block_stmt_ranges,
            write_log_buffer,
            ff_offset_to_entry,
            event_comb_write_offsets: {
                let mut v: Vec<usize> = module
                    .event_comb_writes
                    .iter()
                    .map(|&off| off as usize)
                    .collect();
                v.sort();
                v
            },
            _binary: binary,
        }
    }

    /// Build activity gating tables from per-chunk metadata and ff_commit_entries.
    fn build_activity_gating(
        chunk_meta: &[statement::ChunkActivityMeta],
        ff_commit_entries: &[(usize, usize)],
        _event_comb_writes: &HashSet<isize>,
    ) -> Option<CombActivityGating> {
        if chunk_meta.is_empty() || chunk_meta.len() > 32 || ff_commit_entries.is_empty() {
            return None; // Too many chunks, or no FF entries (all vars comb-classified)
        }

        // Skip when ff_commit_entries is very large (e.g., 32MB DRAM with force_all_ff).
        // The O(entries × ff_reads) ff_to_chunk computation would be too slow.
        if ff_commit_entries.len() > 100_000 {
            log::info!(
                "Activity gating: skipped ({} ff_entries too large)",
                ff_commit_entries.len()
            );
            return None;
        }

        let num_chunks = chunk_meta.len();

        // Build ff_to_chunk: for each ff_commit_entry, which chunks read from its range
        let ff_to_chunk: Vec<u32> = ff_commit_entries
            .iter()
            .map(|&(entry_offset, entry_size)| {
                let entry_end = entry_offset + entry_size;
                let mut mask = 0u32;
                for (ci, cm) in chunk_meta.iter().enumerate() {
                    // Check if any of chunk's FF reads overlap with this entry
                    for &(ff_off, ff_nb) in &cm.ff_reads {
                        let read_end = ff_off + ff_nb;
                        if ff_off < entry_end && read_end > entry_offset {
                            mask |= 1 << ci;
                            break;
                        }
                    }
                }
                mask
            })
            .collect();

        // Build chunk_dependents: for each chunk i, which later chunks j depend on i's comb output
        let mut chunk_dependents = vec![0u32; num_chunks];
        for j in 0..num_chunks {
            for i in 0..j {
                // Check if chunk j reads any comb offset that chunk i writes
                let reads = &chunk_meta[j].comb_reads;
                let writes = &chunk_meta[i].comb_writes;
                // Both are sorted, use merge-intersection
                let mut ri = 0;
                let mut wi = 0;
                let mut depends = false;
                while ri < reads.len() && wi < writes.len() {
                    if reads[ri] == writes[wi] {
                        depends = true;
                        break;
                    } else if reads[ri] < writes[wi] {
                        ri += 1;
                    } else {
                        wi += 1;
                    }
                }
                if depends {
                    chunk_dependents[i] |= 1 << j;
                }
            }
        }

        // Compute transitive closure for chunk_dependents
        // (if i activates j, and j activates k, then i should also activate k)
        for i in 0..num_chunks {
            for j in 0..num_chunks {
                if chunk_dependents[i] & (1 << j) != 0 {
                    chunk_dependents[i] |= chunk_dependents[j];
                }
            }
        }

        // always_active: chunks with dynamic FF reads or hot event-written comb reads
        let mut always_active = 0u32;
        let mut cold_event_active = 0u32;
        let mut event_comb_changed_active = 0u32;
        for (ci, cm) in chunk_meta.iter().enumerate() {
            if cm.has_dynamic_ff_read || cm.reads_hot_event_comb {
                always_active |= 1 << ci;
            }
            if cm.reads_cold_event_comb {
                cold_event_active |= 1 << ci;
            }
            if cm.reads_hot_event_comb || cm.reads_cold_event_comb {
                event_comb_changed_active |= 1 << ci;
            }
        }
        // Propagate always_active through dependents
        for (ci, &dep) in chunk_dependents.iter().enumerate().take(num_chunks) {
            if always_active & (1 << ci) != 0 {
                always_active |= dep;
            }
        }

        log::info!(
            "Activity gating: {} chunks, {} ff_entries, always_active=0x{:x} ({}/{}), event_comb_changed=0x{:x} ({}/{})",
            num_chunks,
            ff_commit_entries.len(),
            always_active,
            always_active.count_ones(),
            num_chunks,
            event_comb_changed_active,
            event_comb_changed_active.count_ones(),
            num_chunks,
        );

        let all_mask = (1u32 << num_chunks) - 1;
        if always_active == all_mask {
            log::info!("  Activity gating disabled: all chunks always_active");
            None
        } else {
            Some(CombActivityGating {
                ff_to_chunk,
                chunk_dependents,
                always_active,
                cold_event_active,
                event_comb_changed_active,
                num_chunks,
            })
        }
    }

    /// Evaluate comb until convergence.
    /// Returns true if converged on the first extra pass (or no extra needed).
    pub fn settle_comb(
        &self,
        mask_cache: &mut MaskCache,
        snapshot_buf: &mut Vec<u8>,
        profile: &mut SimProfile,
        skip_convergence_check: bool,
    ) -> bool {
        #[cfg(feature = "profile")]
        {
            profile.settle_comb_count += 1;
        }
        let _ = profile; // suppress unused warning when profile feature is off

        let min_passes = self.required_comb_passes;
        self.eval_comb_full(mask_cache, profile);
        #[cfg(feature = "profile")]
        {
            profile.comb_eval_count += 1;
        }
        for _ in 1..min_passes {
            self.eval_comb_full(mask_cache, profile);
            #[cfg(feature = "profile")]
            {
                profile.comb_eval_count += 1;
            }
        }
        // In debug builds, verify that required_comb_passes is sufficient
        // by checking that one more eval doesn't change comb_values.
        #[cfg(debug_assertions)]
        if !skip_convergence_check {
            snapshot_buf.resize(self.comb_values.len(), 0);
            snapshot_buf.copy_from_slice(&self.comb_values);
            self.eval_comb_full(mask_cache, profile);
            debug_assert!(
                self.comb_values[..] == snapshot_buf[..],
                "comb did not converge after {} required passes ({} stmts) — \
                 required_comb_passes may be underestimated",
                min_passes,
                self.comb_statements.len()
            );
        }
        if min_passes <= 1 {
            return true;
        }
        if skip_convergence_check {
            return true;
        }
        const MAX_EXTRA: usize = 4;
        for _i in 0..MAX_EXTRA {
            snapshot_buf.resize(self.comb_values.len(), 0);
            snapshot_buf.copy_from_slice(&self.comb_values);
            self.eval_comb_full(mask_cache, profile);
            #[cfg(feature = "profile")]
            {
                profile.comb_eval_count += 1;
                profile.extra_pass_count += 1;
            }
            if self.comb_values[..] == snapshot_buf[..] {
                #[cfg(feature = "profile")]
                if _i == 0 {
                    profile.converged_first_try += 1;
                }
                return _i == 0;
            }
        }
        log::warn!(
            "comb convergence failed after {} + {} passes ({} stmts)",
            min_passes,
            MAX_EXTRA,
            self.comb_statements.len()
        );
        false
    }

    /// Evaluate unified comb once.
    /// Called by settle_comb() for each required pass.
    pub fn eval_comb_full(&self, mask_cache: &mut MaskCache, profile: &mut SimProfile) {
        let _ = profile;
        #[cfg(feature = "profile")]
        let start = std::time::Instant::now();

        for x in &self.comb_statements {
            x.eval_step(mask_cache);
        }

        #[cfg(feature = "profile")]
        {
            profile.eval_comb_full_ns += start.elapsed().as_nanos() as u64;
        }
    }

    /// Evaluate unified comb with activity gating: only evaluate blocks whose
    /// bit is set in `active_mask`. Uses block_stmt_ranges to map block index
    /// to statement index range.
    pub fn eval_comb_activity_gated(
        &self,
        mask_cache: &mut MaskCache,
        profile: &mut SimProfile,
        active_mask: u32,
    ) {
        let _ = profile;
        #[cfg(feature = "profile")]
        let start = std::time::Instant::now();

        for (block_idx, &(start_idx, end_idx)) in self.block_stmt_ranges.iter().enumerate() {
            if active_mask & (1 << block_idx) != 0 {
                for stmt in &self.comb_statements[start_idx..end_idx] {
                    stmt.eval_step(mask_cache);
                }
            }
        }

        #[cfg(feature = "profile")]
        {
            profile.eval_comb_full_ns += start.elapsed().as_nanos() as u64;
        }
    }

    /// Compute active_mask from ff_changed_entries using activity gating tables.
    pub fn compute_active_mask(&self, ff_changed: &[bool]) -> u32 {
        let gating = match &self.comb_activity {
            Some(g) => g,
            None => return u32::MAX, // No gating, evaluate all
        };

        let mut active = gating.always_active;

        // Direct FF dependencies
        for (i, &changed) in ff_changed.iter().enumerate() {
            if changed {
                active |= gating.ff_to_chunk[i];
            }
        }

        // Transitive comb dependencies (propagate through dependents)
        for i in 0..gating.num_chunks {
            if active & (1 << i) != 0 {
                active |= gating.chunk_dependents[i];
            }
        }

        active
    }

    /// Evaluate unified comb once, skipping cold chunks whose indices are in
    /// `skip_sorted` (must be sorted ascending).
    pub fn eval_comb_with_cold_skip(
        &self,
        mask_cache: &mut MaskCache,
        profile: &mut SimProfile,
        skip_sorted: &[usize],
    ) {
        let _ = profile;
        #[cfg(feature = "profile")]
        let start = std::time::Instant::now();

        if skip_sorted.is_empty() {
            for x in &self.comb_statements {
                x.eval_step(mask_cache);
            }
        } else {
            let mut skip_idx = 0;
            for (i, x) in self.comb_statements.iter().enumerate() {
                if skip_idx < skip_sorted.len() && skip_sorted[skip_idx] == i {
                    skip_idx += 1;
                    continue;
                }
                x.eval_step(mask_cache);
            }
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

// SAFETY: Each Ir exclusively owns its ff_values/comb_values buffers.
// Raw pointers in Statements point into these buffers — no cross-Ir aliasing.
// _binary (Arc<Vec<BinaryStorage>>) keeps JIT code pages alive.
// NOTE: Ir is intentionally NOT Sync. Sharing &Ir across threads would allow
// concurrent mutation of ff_values/comb_values via interior raw pointers.
unsafe impl Send for Ir {}

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
            let mut proto: ProtoModule = Conv::conv(&mut context, x)?;
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
    if let Some(entry) = cache.entries.get_mut(&top) {
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

            let mut proto: ProtoModule = Conv::conv(&mut context, x)?;
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
