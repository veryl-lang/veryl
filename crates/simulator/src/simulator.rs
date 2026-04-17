use crate::ir::{
    Event, Ir, ModuleVariables, Value, VarId, VarPath, read_native_value, write_native_value,
};
use crate::wave_dumper::{DumpVar, WaveDumper};
use std::str::FromStr;
use veryl_analyzer::value::MaskCache;

#[cfg(feature = "profile")]
#[derive(Default, Debug)]
pub struct SimProfile {
    pub step_count: u64,
    pub settle_comb_count: u64,
    pub comb_eval_count: u64,
    pub extra_pass_count: u64,
    pub converged_first_try: u64,
    pub settle_comb_ns: u64,
    pub event_eval_ns: u64,
    pub ff_swap_ns: u64,
    pub eval_comb_full_ns: u64,
    /// Cycles where event_comb_changed flag was set (dirty flag fired).
    pub event_comb_dirty_cycles: u64,
    /// Cycles where event-written comb values actually changed.
    pub event_comb_value_changed_cycles: u64,
    /// Per-event-statement cumulative time (ns).
    pub event_stmt_ns: Vec<u64>,
}

#[cfg(not(feature = "profile"))]
#[derive(Default, Debug)]
pub struct SimProfile;

/// Number of consecutive first-try convergences needed before skipping
/// the convergence check loop in settle_comb.
const CONVERGENCE_WARMUP: u32 = 100;

pub struct Simulator {
    pub ir: Ir,
    pub time: u64,
    pub dump: Option<WaveDumper>,
    dump_vars: Vec<DumpVar>,
    pub mask_cache: MaskCache,
    comb_dirty: bool,
    comb_snapshot_buf: Vec<u8>,
    pub profile: SimProfile,
    /// When true, settle_comb skips the convergence check loop.
    convergence_verified: bool,
    /// Remaining warmup iterations before convergence can be trusted.
    convergence_warmup: u32,
    /// Set by ff_commit when any FF value actually changed.
    ff_changed: bool,
    /// Set when comb_values is modified externally (set/mark_comb_dirty/Stmt eval).
    /// When true, activity gating falls back to full eval.
    comb_dirty_external: bool,
    /// Per-ff_commit_entry change tracking for activity gating.
    ff_entry_changed: Vec<bool>,
    /// Activity-skip: per-cold-chunk input snapshots for comparison.
    cold_snapshots: Vec<Vec<u8>>,
    /// Activity-skip: whether each cold chunk has been evaluated at least once.
    cold_initialized: Vec<bool>,
    /// Whether event JIT wrote to any comb variable in the last event_eval.
    event_comb_changed: bool,
    /// Snapshot of event-written comb offsets for value-change detection (profile only).
    #[cfg(feature = "profile")]
    event_comb_snapshot: Vec<u64>,
    /// Sorted event_comb_writes offsets for snapshot comparison (profile only).
    #[cfg(feature = "profile")]
    event_comb_offsets: Vec<usize>,
}

impl Simulator {
    pub fn new(ir: Ir, dump: Option<WaveDumper>) -> Self {
        let comb_snapshot_buf = vec![0u8; ir.comb_values.len()];
        let needs_convergence_check = ir.required_comb_passes > 1;
        let disable_ff_opt = ir.disable_ff_opt;
        let num_cold = ir.cold_chunks.len();
        let num_ff_entries = ir.ff_commit_entries.len();
        let cold_snapshots = ir
            .cold_chunks
            .iter()
            .map(|c| vec![0u8; c.snapshot_size])
            .collect();
        #[cfg(feature = "profile")]
        let num_event_comb = ir.event_comb_write_offsets.len();
        #[cfg(feature = "profile")]
        let event_comb_offsets = ir.event_comb_write_offsets.clone();
        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_vars: Vec::new(),
            mask_cache: MaskCache::default(),
            comb_dirty: true,
            comb_snapshot_buf,
            profile: Default::default(),
            convergence_verified: false,
            // When disable_ff_opt is set, always perform full convergence
            // checks to help detect comb evaluation inconsistencies.
            convergence_warmup: if needs_convergence_check && !disable_ff_opt {
                CONVERGENCE_WARMUP
            } else {
                0
            },
            ff_changed: true,
            comb_dirty_external: true,
            ff_entry_changed: vec![true; num_ff_entries],
            cold_snapshots,
            cold_initialized: vec![false; num_cold],
            event_comb_changed: true, // Initially true to force full eval on first cycle
            #[cfg(feature = "profile")]
            event_comb_snapshot: vec![0u64; num_event_comb],
            #[cfg(feature = "profile")]
            event_comb_offsets,
        };

        if let Some(dumper) = dump {
            ret.setup_dump(dumper);
        }

        ret
    }

    fn do_settle_comb(&mut self) {
        // Activity gating: skip comb chunks whose FF inputs haven't changed.
        // Currently auto-disabled when single-function JIT produces 1 chunk.
        if self.ir.required_comb_passes == 1
            && let Some(comb_activity) = self.ir.comb_activity.as_ref()
        {
            if self.comb_dirty_external {
                self.ir
                    .eval_comb_full(&mut self.mask_cache, &mut self.profile);
                self.comb_dirty_external = false;
            } else {
                let mut active_mask = self.ir.compute_active_mask(&self.ff_entry_changed);
                let event_comb_changed_active = comb_activity.event_comb_changed_active;
                let num_chunks = comb_activity.num_chunks;
                let all_mask = (1u32 << num_chunks) - 1;

                // When event→comb dirty flag was set, activate affected chunks
                if self.event_comb_changed && event_comb_changed_active != 0 {
                    self.event_comb_changed = false; // Consume the flag
                    active_mask |= event_comb_changed_active;
                    // Propagate through dependents
                    let gating = comb_activity;
                    for ci in 0..num_chunks {
                        if active_mask & (1 << ci) != 0 {
                            active_mask |= gating.chunk_dependents[ci];
                        }
                    }
                }

                if active_mask == all_mask {
                    self.ir
                        .eval_comb_full(&mut self.mask_cache, &mut self.profile);
                } else if active_mask != 0 {
                    self.ir.eval_comb_activity_gated(
                        &mut self.mask_cache,
                        &mut self.profile,
                        active_mask,
                    );
                }
            }
            return;
        }

        let has_cold_chunks = !self.ir.cold_chunks.is_empty();
        // Skip is safe when:
        // 1. All cold chunks have been initialized
        // 2. No event wrote to cold region (dirty flag is 0)
        // 3. Convergence is verified (single-pass sufficient)
        let cold_dirty = self.read_and_clear_cold_dirty_flag();
        let can_skip_cold = has_cold_chunks
            && !cold_dirty
            && self.convergence_verified
            && self.cold_initialized.iter().all(|&b| b);

        if can_skip_cold {
            // Compute which cold chunks can be skipped
            let skip_indices = self.compute_cold_skip();
            if !skip_indices.is_empty() {
                // Use the skip path for the first required pass
                let skip_conv = self.convergence_verified;
                #[cfg(feature = "profile")]
                {
                    self.profile.settle_comb_count += 1;
                }
                self.ir.eval_comb_with_cold_skip(
                    &mut self.mask_cache,
                    &mut self.profile,
                    &skip_indices,
                );
                #[cfg(feature = "profile")]
                {
                    self.profile.comb_eval_count += 1;
                }
                // Additional passes (if required) run without skip for correctness
                for _ in 1..self.ir.required_comb_passes {
                    self.ir
                        .eval_comb_full(&mut self.mask_cache, &mut self.profile);
                    #[cfg(feature = "profile")]
                    {
                        self.profile.comb_eval_count += 1;
                    }
                }
                // Update snapshots for non-skipped cold chunks
                self.update_cold_snapshots(&skip_indices);
                // Convergence warmup (simplified: treat as converged first try)
                if !skip_conv && self.convergence_warmup > 0 {
                    self.convergence_warmup -= 1;
                    if self.convergence_warmup == 0 {
                        self.convergence_verified = true;
                    }
                }
                return;
            }
        }

        // Full evaluation path (no cold skip)
        let skip_conv = self.convergence_verified;
        let converged_first = self.ir.settle_comb(
            &mut self.mask_cache,
            &mut self.comb_snapshot_buf,
            &mut self.profile,
            skip_conv,
        );
        if !skip_conv && self.convergence_warmup > 0 {
            if converged_first {
                self.convergence_warmup -= 1;
                if self.convergence_warmup == 0 {
                    self.convergence_verified = true;
                }
            } else {
                self.convergence_warmup = CONVERGENCE_WARMUP;
            }
        }

        // Only update snapshots when skip might be possible next cycle.
        // Snapshot update is expensive (scattered reads) so avoid it
        // when skip conditions can't be met.
        if has_cold_chunks && self.convergence_verified {
            self.update_all_cold_snapshots();
        }
    }

    /// Check which cold chunks can be skipped (inputs unchanged since last eval).
    /// Returns sorted list of statement indices to skip.
    fn compute_cold_skip(&self) -> Vec<usize> {
        let mut skip = Vec::new();
        for (i, cold) in self.ir.cold_chunks.iter().enumerate() {
            if self.cold_inputs_match(i, cold) {
                skip.push(cold.stmt_index);
            }
        }
        skip
    }

    /// Compare current input bytes with snapshot for cold chunk `idx`.
    fn cold_inputs_match(&self, idx: usize, cold: &crate::ir::ColdChunk) -> bool {
        let snapshot = &self.cold_snapshots[idx];
        let mut offset = 0;
        // Compare hot comb inputs
        for &(comb_off, nb) in &cold.hot_comb_ranges {
            let current = &self.ir.comb_values[comb_off..comb_off + nb];
            if current != &snapshot[offset..offset + nb] {
                return false;
            }
            offset += nb;
        }
        // Compare FF inputs
        for &(ff_off, nb) in &cold.ff_ranges {
            let current = &self.ir.ff_values[ff_off..ff_off + nb];
            if current != &snapshot[offset..offset + nb] {
                return false;
            }
            offset += nb;
        }
        true
    }

    /// Update snapshots for cold chunks that were NOT skipped.
    fn update_cold_snapshots(&mut self, skip_indices: &[usize]) {
        let n = self.ir.cold_chunks.len();
        for i in 0..n {
            let stmt_idx = self.ir.cold_chunks[i].stmt_index;
            if !skip_indices.contains(&stmt_idx) {
                Self::snapshot_cold_chunk_at(
                    &self.ir.cold_chunks[i],
                    &mut self.cold_snapshots[i],
                    &self.ir.comb_values,
                    &self.ir.ff_values,
                );
                self.cold_initialized[i] = true;
            }
        }
    }

    /// Update ALL cold chunk snapshots (after full evaluation).
    fn update_all_cold_snapshots(&mut self) {
        let n = self.ir.cold_chunks.len();
        for i in 0..n {
            Self::snapshot_cold_chunk_at(
                &self.ir.cold_chunks[i],
                &mut self.cold_snapshots[i],
                &self.ir.comb_values,
                &self.ir.ff_values,
            );
            self.cold_initialized[i] = true;
        }
    }

    /// Read and clear the cold dirty flag from comb_values.
    /// Returns true if event JIT code set the flag (cold region was written).
    fn read_and_clear_cold_dirty_flag(&mut self) -> bool {
        let offset = self.ir.cold_dirty_flag_offset;
        if offset >= self.ir.comb_values.len() {
            return false;
        }
        let val = self.ir.comb_values[offset];
        if val != 0 {
            self.ir.comb_values[offset] = 0;
            true
        } else {
            false
        }
    }

    /// Read and clear the event→comb dirty flag from comb_values.
    /// Returns true if event JIT code wrote to any comb variable.
    fn read_and_clear_event_comb_dirty_flag(&mut self) -> bool {
        let offset = self.ir.event_comb_dirty_flag_offset;
        if offset >= self.ir.comb_values.len() {
            return false;
        }
        let val = self.ir.comb_values[offset];
        if val != 0 {
            self.ir.comb_values[offset] = 0;
            true
        } else {
            false
        }
    }

    /// Snapshot current input bytes for a cold chunk into `snapshot`.
    fn snapshot_cold_chunk_at(
        cold: &crate::ir::ColdChunk,
        snapshot: &mut [u8],
        comb_values: &[u8],
        ff_values: &[u8],
    ) {
        let mut offset = 0;
        for &(comb_off, nb) in &cold.hot_comb_ranges {
            snapshot[offset..offset + nb].copy_from_slice(&comb_values[comb_off..comb_off + nb]);
            offset += nb;
        }
        for &(ff_off, nb) in &cold.ff_ranges {
            snapshot[offset..offset + nb].copy_from_slice(&ff_values[ff_off..ff_off + nb]);
            offset += nb;
        }
    }

    pub fn set(&mut self, port: &str, value: Value) {
        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.ir.ports.get(&port)
            && let Some(x) = self.ir.module_variables.variables.get_mut(id)
        {
            let mut value = value;
            value.trunc(x.width);
            unsafe {
                write_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    &value,
                );
            }
            self.comb_dirty = true;
            self.comb_dirty_external = true;
        }
    }

    pub fn get(&mut self, port: &str) -> Option<Value> {
        self.ensure_comb_updated();

        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.ir.ports.get(&port)
            && let Some(x) = self.ir.module_variables.variables.get(id)
        {
            let value = unsafe {
                read_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    x.width as u32,
                    false,
                )
            };
            Some(value)
        } else {
            None
        }
    }

    /// Get a variable value by hierarchical path (e.g., "dut.cnt").
    /// Searches all module variables including children.
    pub fn get_var(&mut self, path: &str) -> Option<Value> {
        self.ensure_comb_updated();

        let target = VarPath::from_str(path).unwrap();
        Self::find_var_in_module(&self.ir.module_variables, &target, self.ir.use_4state)
    }

    fn find_var_in_module(
        module: &ModuleVariables,
        target: &VarPath,
        use_4state: bool,
    ) -> Option<Value> {
        // If target has multiple segments, try matching child module by name first
        if target.0.len() > 1 {
            for child in &module.children {
                if child.name == target.0[0] {
                    let sub = VarPath::from_slice(&target.0[1..]);
                    if let Some(v) = Self::find_var_in_module(child, &sub, use_4state) {
                        return Some(v);
                    }
                }
            }
        }

        // Look for a variable whose path matches exactly
        for var in module.variables.values() {
            if var.path == *target {
                let value = unsafe {
                    read_native_value(
                        var.current_values[0],
                        var.native_bytes,
                        use_4state,
                        var.width as u32,
                        false,
                    )
                };
                return Some(value);
            }
        }
        None
    }

    pub fn ensure_comb_updated(&mut self) {
        if self.comb_dirty {
            #[cfg(feature = "profile")]
            let start = std::time::Instant::now();

            self.do_settle_comb();
            self.comb_dirty = false;

            #[cfg(feature = "profile")]
            {
                self.profile.settle_comb_ns += start.elapsed().as_nanos() as u64;
            }
        }
    }

    pub fn mark_comb_dirty(&mut self) {
        self.comb_dirty = true;
        self.comb_dirty_external = true;
    }

    pub fn get_clock(&self, port: &str) -> Option<Event> {
        let port = VarPath::from_str(port).unwrap();
        self.ir.ports.get(&port).map(|id| Event::Clock(*id))
    }

    pub fn get_reset(&self, port: &str) -> Option<Event> {
        let port = VarPath::from_str(port).unwrap();
        self.ir.ports.get(&port).map(|id| Event::Reset(*id))
    }

    pub fn step(&mut self, event: &Event) {
        #[cfg(feature = "profile")]
        {
            self.profile.step_count += 1;
        }

        if self.comb_dirty {
            #[cfg(feature = "profile")]
            let start = std::time::Instant::now();

            self.do_settle_comb();
            self.comb_dirty = false;

            #[cfg(feature = "profile")]
            {
                self.profile.settle_comb_ns += start.elapsed().as_nanos() as u64;
            }
        }

        #[cfg(feature = "profile")]
        let event_start = std::time::Instant::now();

        if let Some(statements) = self.ir.event_statements.get(event) {
            #[cfg(feature = "profile")]
            {
                if self.profile.event_stmt_ns.len() < statements.len() {
                    self.profile.event_stmt_ns.resize(statements.len(), 0);
                }
                for (i, x) in statements.iter().enumerate() {
                    let t = std::time::Instant::now();
                    x.eval_step(&mut self.mask_cache);
                    self.profile.event_stmt_ns[i] += t.elapsed().as_nanos() as u64;
                }
            }
            #[cfg(not(feature = "profile"))]
            for x in statements {
                x.eval_step(&mut self.mask_cache);
            }
        }

        // Fast path for simple designs: no write-log means no bit-select
        // NBA, no sparse commit, no event→comb propagation. Skip dirty-flag
        // check and drain, use direct ff_commit.
        if self.ir.write_log_buffer.is_none() {
            #[cfg(feature = "profile")]
            {
                self.profile.event_eval_ns += event_start.elapsed().as_nanos() as u64;
            }
            self.ff_changed = Self::ff_commit(&mut self.ir.ff_values, &self.ir.ff_commit_entries);
            self.comb_dirty = true;
            self.dump_variables();
            return;
        }

        // Read and clear event→comb dirty flag (set by event JIT when writing to comb).
        // Only set to true, never reset to false here — the flag persists until
        // settle_comb consumes it. This handles the case where negedge has no event
        // but posedge event_comb_changed should still be true from the previous posedge.
        if self.read_and_clear_event_comb_dirty_flag() {
            self.event_comb_changed = true;

            #[cfg(feature = "profile")]
            {
                self.profile.event_comb_dirty_cycles += 1;
                // Compare event-written comb values with snapshot to detect actual changes
                let comb_ptr = self.ir.comb_values.as_ptr();
                let mut any_changed = false;
                for (i, &off) in self.event_comb_offsets.iter().enumerate() {
                    let val = unsafe { (comb_ptr.add(off) as *const u64).read_unaligned() };
                    if val != self.event_comb_snapshot[i] {
                        any_changed = true;
                        self.event_comb_snapshot[i] = val;
                    }
                }
                if any_changed {
                    self.profile.event_comb_value_changed_cycles += 1;
                }
            }
        }

        #[cfg(feature = "profile")]
        {
            self.profile.event_eval_ns += event_start.elapsed().as_nanos() as u64;
        }

        // Apply scheduled comb writes atomically (strict SV NBA).
        // Event-phase JIT stores to comb were routed to the write-log; drain
        // them into `comb_values` now that all events for this cycle have
        // finished. This must run BEFORE `ff_commit_from_log` so FF entries
        // still reflect only FF work for the sparse commit.
        if let Some(write_log) = self.ir.write_log_buffer.as_ref() {
            Self::drain_event_comb_writes(&mut self.ir.comb_values, write_log);
            // Any comb update makes settle_comb necessary next step;
            // force the activity-gating fallback to full eval.
            self.comb_dirty_external = true;
        }

        #[cfg(feature = "profile")]
        let ff_start = std::time::Instant::now();

        let write_log = self.ir.write_log_buffer.as_mut().unwrap();
        // Sparse commit: only process write-log entries.
        self.ff_entry_changed.iter_mut().for_each(|x| *x = false);
        self.ff_changed = Self::ff_commit_from_log(
            &mut self.ir.ff_values,
            write_log,
            Some(&mut self.ff_entry_changed),
            &self.ir.ff_offset_to_entry,
        );

        #[cfg(feature = "profile")]
        {
            self.profile.ff_swap_ns += ff_start.elapsed().as_nanos() as u64;
        }

        self.comb_dirty = true;

        self.dump_variables();
    }

    /// Commit FF updates: copy next → current for all FF variables.
    /// Returns true if any FF value actually changed.
    /// Constant-size copies allow LLVM to inline as MOV instructions.
    #[inline(always)]
    fn ff_commit(ff_values: &mut [u8], entries: &[(usize, usize)]) -> bool {
        let ptr = ff_values.as_mut_ptr();
        let mut changed = false;
        for &(current_offset, value_size) in entries {
            unsafe {
                let dst = ptr.add(current_offset);
                let src = ptr.add(current_offset + value_size);
                // Compare before copy to detect changes
                let differ = match value_size {
                    4 => {
                        (dst as *const u32).read_unaligned() != (src as *const u32).read_unaligned()
                    }
                    8 => {
                        (dst as *const u64).read_unaligned() != (src as *const u64).read_unaligned()
                    }
                    _ => {
                        std::slice::from_raw_parts(dst, value_size)
                            != std::slice::from_raw_parts(src, value_size)
                    }
                };
                if differ {
                    changed = true;
                    match value_size {
                        1 => std::ptr::copy_nonoverlapping(src, dst, 1),
                        2 => std::ptr::copy_nonoverlapping(src, dst, 2),
                        4 => std::ptr::copy_nonoverlapping(src, dst, 4),
                        8 => std::ptr::copy_nonoverlapping(src, dst, 8),
                        16 => std::ptr::copy_nonoverlapping(src, dst, 16),
                        32 => std::ptr::copy_nonoverlapping(src, dst, 32),
                        n => std::ptr::copy_nonoverlapping(src, dst, n),
                    }
                }
            }
        }
        changed
    }

    /// Apply scheduled comb writes logged during `event_eval` to
    /// `comb_values`, giving event writes atomic (strict NBA) visibility.
    ///
    /// Reads `LOG_KIND_COMB` entries from the write-log. Each entry carries
    /// the final stored value inline (`entry.value[..entry.value_size]`),
    /// which is memcpy'd to `comb_values[entry.current_offset..]`. FF-kind
    /// entries are left in place for `ff_commit_from_log` to consume.
    #[inline(always)]
    fn drain_event_comb_writes(comb_values: &mut [u8], log_buf: &crate::ir::WriteLogBuffer) {
        let count = (log_buf.count as usize).min(crate::ir::MAX_WRITE_LOG_ENTRIES);
        let ptr = comb_values.as_mut_ptr();
        for i in 0..count {
            let entry = &log_buf.entries[i];
            if entry.kind != crate::ir::LOG_KIND_COMB {
                continue;
            }
            let offset = entry.current_offset as usize;
            let size = entry.value_size as usize;
            // Bit-merge: memory = (memory & ~mask) | (value & mask).
            // This preserves bits outside `mask`, giving SV bit-select
            // NBA composition for multiple writes to the same variable.
            unsafe {
                match size {
                    1 => {
                        let m = entry.mask[0];
                        let v = entry.value[0];
                        let p = ptr.add(offset);
                        *p = (*p & !m) | (v & m);
                    }
                    2 => {
                        let m = u16::from_le_bytes([entry.mask[0], entry.mask[1]]);
                        let v = u16::from_le_bytes([entry.value[0], entry.value[1]]);
                        let p = ptr.add(offset) as *mut u16;
                        p.write_unaligned((p.read_unaligned() & !m) | (v & m));
                    }
                    4 => {
                        let m = u32::from_le_bytes(entry.mask[0..4].try_into().unwrap());
                        let v = u32::from_le_bytes(entry.value[0..4].try_into().unwrap());
                        let p = ptr.add(offset) as *mut u32;
                        p.write_unaligned((p.read_unaligned() & !m) | (v & m));
                    }
                    8 => {
                        let m = u64::from_le_bytes(entry.mask[0..8].try_into().unwrap());
                        let v = u64::from_le_bytes(entry.value[0..8].try_into().unwrap());
                        let p = ptr.add(offset) as *mut u64;
                        p.write_unaligned((p.read_unaligned() & !m) | (v & m));
                    }
                    16 => {
                        let m_lo = u64::from_le_bytes(entry.mask[0..8].try_into().unwrap());
                        let m_hi = u64::from_le_bytes(entry.mask[8..16].try_into().unwrap());
                        let v_lo = u64::from_le_bytes(entry.value[0..8].try_into().unwrap());
                        let v_hi = u64::from_le_bytes(entry.value[8..16].try_into().unwrap());
                        let p_lo = ptr.add(offset) as *mut u64;
                        let p_hi = ptr.add(offset + 8) as *mut u64;
                        p_lo.write_unaligned((p_lo.read_unaligned() & !m_lo) | (v_lo & m_lo));
                        p_hi.write_unaligned((p_hi.read_unaligned() & !m_hi) | (v_hi & m_hi));
                    }
                    _ => {
                        // Fallback: byte-wise bit-merge for unusual sizes.
                        let max = size.min(entry.value.len());
                        for b in 0..max {
                            let m = entry.mask[b];
                            let v = entry.value[b];
                            let p = ptr.add(offset + b);
                            *p = (*p & !m) | (v & m);
                        }
                    }
                }
            }
        }
    }

    /// Sparse FF commit using write-log: only process entries logged by JIT.
    /// Reads from the heap-allocated WriteLogBuffer directly.
    /// Returns true if any FF value actually changed.
    ///
    /// Skips entries with `kind != LOG_KIND_FF`. Comb-kind entries are
    /// consumed separately by `drain_event_comb_writes`.
    #[inline(always)]
    fn ff_commit_from_log(
        ff_values: &mut [u8],
        log_buf: &mut crate::ir::WriteLogBuffer,
        mut entry_changed: Option<&mut [bool]>,
        offset_to_entry: &crate::HashMap<usize, usize>,
    ) -> bool {
        let count = (log_buf.count as usize).min(crate::ir::MAX_WRITE_LOG_ENTRIES);
        let ptr = ff_values.as_mut_ptr();

        let mut changed = false;
        for i in 0..count {
            let entry = &log_buf.entries[i];
            if entry.kind != crate::ir::LOG_KIND_FF {
                continue;
            }
            let current_offset = entry.current_offset as usize;
            let value_size = entry.value_size as usize;

            unsafe {
                let dst = ptr.add(current_offset);
                let src = ptr.add(current_offset + value_size);
                let differ = match value_size {
                    4 => {
                        (dst as *const u32).read_unaligned() != (src as *const u32).read_unaligned()
                    }
                    8 => {
                        (dst as *const u64).read_unaligned() != (src as *const u64).read_unaligned()
                    }
                    _ => {
                        std::slice::from_raw_parts(dst, value_size)
                            != std::slice::from_raw_parts(src, value_size)
                    }
                };
                if differ {
                    changed = true;
                    match value_size {
                        1 => std::ptr::copy_nonoverlapping(src, dst, 1),
                        2 => std::ptr::copy_nonoverlapping(src, dst, 2),
                        4 => std::ptr::copy_nonoverlapping(src, dst, 4),
                        8 => std::ptr::copy_nonoverlapping(src, dst, 8),
                        16 => std::ptr::copy_nonoverlapping(src, dst, 16),
                        32 => std::ptr::copy_nonoverlapping(src, dst, 32),
                        n => std::ptr::copy_nonoverlapping(src, dst, n),
                    }
                }
                if let Some(ref mut ec) = entry_changed
                    && let Some(&idx) = offset_to_entry.get(&current_offset)
                {
                    ec[idx] = differ;
                }
            }
        }

        // Clear log count
        log_buf.count = 0;

        changed
    }

    /// Set a variable value by VarId. Used to write clock/reset signal values
    /// into the variable storage so they appear in wave dumps.
    pub fn set_var_by_id(&mut self, var_id: &VarId, val: Value) {
        if let Some(x) = self.ir.module_variables.variables.get_mut(var_id) {
            let mut val = val;
            val.trunc(x.width);
            unsafe {
                write_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    &val,
                );
            }
            self.comb_dirty = true;
            self.comb_dirty_external = true;
        }
    }

    pub fn dump_start(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.begin_dumpvars();
            dump.dump_all_vars(&self.dump_vars, self.ir.use_4state);
            dump.end_dumpvars();
        }
    }

    pub fn dump_variables(&mut self) {
        if self.dump.is_some() {
            if self.comb_dirty {
                self.do_settle_comb();
                self.comb_dirty = false;
            }
            let dump = self.dump.as_mut().unwrap();
            dump.timestamp(self.time);
            dump.dump_all_vars(&self.dump_vars, self.ir.use_4state);
        }
    }

    fn setup_dump(&mut self, mut dumper: WaveDumper) {
        dumper.timescale();
        dumper.setup_module(&self.ir.module_variables, &mut self.dump_vars);
        dumper.finish_header();
        self.dump = Some(dumper);
    }
}
