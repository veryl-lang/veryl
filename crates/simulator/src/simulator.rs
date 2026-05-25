use crate::ir::write_log::{clear_event_write_log, ff_commit_from_log, set_event_write_log};
use crate::ir::{
    Event, Ir, ModuleVariables, Statement, Value, VarId, VarPath, dispatch_stmt_fast,
    read_native_value, write_native_value,
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
}

#[cfg(not(feature = "profile"))]
#[derive(Default, Debug)]
pub struct SimProfile;

pub struct Simulator {
    pub ir: Ir,
    pub time: u64,
    pub dump: Option<WaveDumper>,
    dump_vars: Vec<DumpVar>,
    pub mask_cache: MaskCache,
    comb_dirty: bool,
    /// Scratch buffer used only by the worklist (`eval_comb_worklist`)
    /// evaluation path; the default settle_comb path does not touch it.
    comb_snapshot_buf: Vec<u8>,
    pub profile: SimProfile,
    last_event: Option<Event>,
    last_event_stmts: *const Vec<Statement>,
    /// Env-gated `VERYL_WRITE_LOG_DIAG=1` diagnostics for the write-log
    /// commit path.  Accumulated across the run; `dump` is invoked
    /// automatically when the cycle counter crosses a logarithmic
    /// checkpoint (doubling cadence, capped at 1 M cycles).
    pub write_log_diag: WriteLogDiag,
}

#[derive(Default)]
pub struct WriteLogDiag {
    pub enabled: bool,
    pub total_cycles: u64,
    pub total_entries: u64,
    pub max_entries_per_cycle: u32,
    pub cycles_with_entries: u64,
    next_print_cycle: u64,
}

impl WriteLogDiag {
    fn maybe_print(&mut self) {
        if !self.enabled {
            return;
        }
        if self.total_cycles >= self.next_print_cycle {
            self.next_print_cycle = self.next_print_cycle.saturating_mul(2).max(1_000_000);
            self.dump();
        }
    }

    pub fn dump(&self) {
        let avg = if self.cycles_with_entries > 0 {
            self.total_entries as f64 / self.cycles_with_entries as f64
        } else {
            0.0
        };
        eprintln!(
            "[write_log_diag] cycles={} cycles_with_entries={} total_entries={} max_per_cycle={} avg_per_active_cycle={:.2}",
            self.total_cycles,
            self.cycles_with_entries,
            self.total_entries,
            self.max_entries_per_cycle,
            avg,
        );
    }
}

impl Simulator {
    pub fn new(ir: Ir, dump: Option<WaveDumper>) -> Self {
        let comb_snapshot_buf = vec![0u8; ir.comb_values.len()];
        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_vars: Vec::new(),
            mask_cache: MaskCache::default(),
            comb_dirty: true,
            comb_snapshot_buf,
            profile: Default::default(),
            last_event: None,
            last_event_stmts: std::ptr::null(),
            write_log_diag: WriteLogDiag {
                enabled: std::env::var("VERYL_WRITE_LOG_DIAG").as_deref() == Ok("1"),
                next_print_cycle: 1_000_000,
                ..Default::default()
            },
        };

        if let Some(dumper) = dump {
            ret.setup_dump(dumper);
        }

        ret
    }

    fn do_settle_comb(&mut self) {
        self.ir.settle_comb(
            &mut self.mask_cache,
            &mut self.comb_snapshot_buf,
            &mut self.profile,
        );
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

        // Install the per-Ir WriteLogBuffer before settle_comb so that
        // comb-scope FF writes (which appear under `--disable-ff-opt`'s
        // force_all_ff path and never go through the event scope) also
        // emit log entries and get committed alongside event-scope writes
        // at cycle end.  Without this install, settle_comb's FF stores
        // hit `event_write_log_push_static`'s "no active log" branch and
        // turn into no-ops, leaving the FF current slot stale.
        // SAFETY: the buffer outlives the call to dispatch_stmt_fast and
        // is cleared before this stack frame returns.
        unsafe {
            set_event_write_log(&mut self.ir.write_log_buffer);
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

        let stmts_ptr = if self.last_event.as_ref() == Some(event) {
            self.last_event_stmts
        } else {
            let ptr: *const Vec<Statement> = match self.ir.event_statements.get(event) {
                Some(v) => v as *const _,
                None => std::ptr::null(),
            };
            self.last_event = Some(event.clone());
            self.last_event_stmts = ptr;
            ptr
        };

        // AOT-C: if this event was lowered to a gcc-compiled FF-next +
        // write-log function, invoke it instead of the per-stmt Cranelift
        // dispatch.  The function reads ff/comb current values and pushes
        // WriteLogEntries into the buffer (3rd arg), exactly as the Cranelift
        // event JIT does; `ff_commit_from_log` below applies them.
        #[cfg(not(target_family = "wasm"))]
        let aot_event_func = self
            .ir
            .aot_c_event_evals
            .get(event)
            .and_then(|cell| cell.get())
            .map(|m| m.func);
        #[cfg(target_family = "wasm")]
        let aot_event_func: Option<crate::FuncPtr> = None;

        if let Some(func) = aot_event_func {
            let ff_ptr = self.ir.ff_values.as_ptr();
            let comb_ptr = self.ir.comb_values.as_ptr();
            let log_ptr = (&*self.ir.write_log_buffer) as *const _ as *mut u8;

            // VERYL_AOT_C_VALIDATE=1: run BOTH the AOT-C event function and the
            // Cranelift per-stmt dispatch on the same inputs and compare the
            // WriteLogEntries they push plus any direct ff/comb writes; panic
            // on first divergence so the broken event stmt can be identified.
            // (Mirrors settle_comb's comb-side validate.)  Default-off.
            #[cfg(not(target_family = "wasm"))]
            let validate = self.ir.aot_c_validate;
            #[cfg(target_family = "wasm")]
            let validate = false;

            if !validate {
                // SAFETY: pointers valid for the call; emitted code only reads
                // ff/comb and pushes into the WriteLogBuffer.
                unsafe {
                    func(ff_ptr, comb_ptr, log_ptr);
                }
            } else {
                // `validate` is always false on wasm (the method is non-wasm).
                #[cfg(not(target_family = "wasm"))]
                self.validate_event_aot(func, stmts_ptr);
            }
        } else if !stmts_ptr.is_null() {
            // SAFETY: event_statements is never mutated after Ir construction.
            let statements: &Vec<Statement> = unsafe { &*stmts_ptr };
            for x in statements {
                dispatch_stmt_fast(x, &mut self.mask_cache);
            }
        }

        #[cfg(feature = "profile")]
        {
            self.profile.event_eval_ns += event_start.elapsed().as_nanos() as u64;
        }

        #[cfg(feature = "profile")]
        let ff_start = std::time::Instant::now();

        ff_commit_from_log(&mut self.ir.ff_values, &self.ir.write_log_buffer);

        clear_event_write_log();
        if self.write_log_diag.enabled {
            let n = self.ir.write_log_buffer.count();
            self.write_log_diag.total_cycles += 1;
            if n > 0 {
                self.write_log_diag.total_entries += n as u64;
                self.write_log_diag.cycles_with_entries += 1;
                if n > self.write_log_diag.max_entries_per_cycle {
                    self.write_log_diag.max_entries_per_cycle = n;
                }
            }
            self.write_log_diag.maybe_print();
        }
        self.ir.write_log_buffer.reset();

        #[cfg(feature = "profile")]
        {
            self.profile.ff_swap_ns += ff_start.elapsed().as_nanos() as u64;
        }

        self.comb_dirty = true;

        self.dump_variables();
    }

    /// VERYL_AOT_C_VALIDATE event-path check: run the AOT-C event function and
    /// the Cranelift per-stmt dispatch on identical inputs, compare the
    /// WriteLogEntries they push plus any direct ff/comb writes, and panic on
    /// first divergence.  Leaves the Cranelift result live (ground truth).
    /// Slow (clones ff/comb each event) — diagnostics only.
    #[cfg(not(target_family = "wasm"))]
    fn validate_event_aot(&mut self, func: crate::FuncPtr, stmts_ptr: *const Vec<Statement>) {
        let ff_ptr = self.ir.ff_values.as_ptr();
        let comb_ptr = self.ir.comb_values.as_ptr();
        let log_ptr = (&*self.ir.write_log_buffer) as *const _ as *mut u8;

        let ff_snap = self.ir.ff_values.to_vec();
        let comb_snap = self.ir.comb_values.to_vec();
        let count_before = self.ir.write_log_buffer.narrow_count as usize;

        // AOT-C event, then capture its pushed entries + ff/comb.
        unsafe { func(ff_ptr, comb_ptr, log_ptr) };
        // The committed FF effect is `ff_commit_from_log`'s last-write-wins per
        // offset, so compare offset -> (width_class, last payload) maps, not the
        // raw entry order or the pre-commit ff_values (the dual-slot "next slot"
        // direct writes are vestigial — ff_commit applies the *log* to the
        // current slots, so those transient writes don't affect correctness).
        let lww_map = |buf: &crate::ir::write_log::WriteLogBuffer, lo: usize, hi: usize| {
            let mut m: std::collections::HashMap<u32, (u16, u64)> = Default::default();
            for e in &buf.narrow_entries_slice()[lo..hi] {
                m.insert(e.offset, (e.width_class, e.payload));
            }
            m
        };
        let aot_count = self.ir.write_log_buffer.narrow_count as usize;
        let aot_map = lww_map(&self.ir.write_log_buffer, count_before, aot_count);

        // Restore inputs + log count, then run the Cranelift event.
        unsafe {
            std::ptr::copy_nonoverlapping(
                ff_snap.as_ptr(),
                self.ir.ff_values.as_ptr() as *mut u8,
                ff_snap.len(),
            );
            std::ptr::copy_nonoverlapping(
                comb_snap.as_ptr(),
                self.ir.comb_values.as_ptr() as *mut u8,
                comb_snap.len(),
            );
        }
        self.ir.write_log_buffer.narrow_count = count_before as u32;
        if !stmts_ptr.is_null() {
            let statements: &Vec<Statement> = unsafe { &*stmts_ptr };
            for x in statements {
                dispatch_stmt_fast(x, &mut self.mask_cache);
            }
        }
        let cr_count = self.ir.write_log_buffer.narrow_count as usize;
        let cr_map = lww_map(&self.ir.write_log_buffer, count_before, cr_count);

        if aot_map != cr_map {
            eprintln!(
                "[aot_event_validate] DIVERGENCE event={:?}: committed-FF maps differ (aot {} offsets, cranelift {} offsets)",
                self.last_event,
                aot_map.len(),
                cr_map.len(),
            );
            // Offsets present in only one side, or with differing value.
            for (off, av) in &aot_map {
                match cr_map.get(off) {
                    None => eprintln!("  off={off:#x}: aot={av:?} cranelift=<absent>"),
                    Some(cv) if cv != av => {
                        eprintln!("  off={off:#x}: aot={av:?} cranelift={cv:?}")
                    }
                    _ => {}
                }
            }
            for off in cr_map.keys() {
                if !aot_map.contains_key(off) {
                    eprintln!("  off={off:#x}: aot=<absent> cranelift={:?}", cr_map[off]);
                }
            }
            panic!("AOT-C event validate divergence (see above)");
        }
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
