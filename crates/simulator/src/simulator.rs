use crate::backend::CompiledWhole;
use crate::component::loader::ComponentError;
use crate::component::runtime::{RuntimeComponent, build_components};
use crate::ir::write_log::{
    WriteLogBuffer, clear_event_write_log, ff_commit_from_log, set_event_write_log,
};
use crate::ir::{
    Event, Ir, ModuleVariables, Statement, Value, VarId, VarPath, dispatch_stmt_fast,
    read_native_value, write_native_value,
};
use crate::wave_dumper::{DumpVar, WaveDumper};
use smallvec::SmallVec;
use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::Arc;
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
    pub profile: SimProfile,
    last_event: Option<Event>,
    last_event_stmts: *const Vec<Statement>,
    /// Whole-event AOT-C handle for `last_event`, cached alongside
    /// `last_event_stmts` (same predicate, same post-construction-immutable
    /// `whole_events` invariant) so the hot path skips a per-cycle
    /// `whole_events` HashMap probe + `Arc` clone.  `None` = no whole-event
    /// backend for `last_event`.  Points into `self.ir.whole_events`'s `Arc`,
    /// which is never mutated after `Ir` construction.
    last_whole_event: Option<*const dyn CompiledWhole>,
    /// Previous-step derived-clock values (sampled at master=0).  Empty
    /// when no derived clocks; otherwise used for 0→1 edge detection.
    prev_derived_clock_values: Vec<u8>,
    /// Env-gated `VERYL_WRITE_LOG_DIAG=1` diagnostics for the write-log
    /// commit path.  Accumulated across the run; `dump` is invoked
    /// automatically when the cycle counter crosses a logarithmic
    /// checkpoint (doubling cadence, capped at 1 M cycles).
    pub write_log_diag: WriteLogDiag,
    /// Stop the testbench after this many clock cycles; `None` runs to completion.
    pub cycle_limit: Option<u64>,
    pub cycle_count: u64,
    /// Env-gated `VERYL_STEP_WATCH=path1,path2` debug watch: resolved
    /// variable pointers printed at each phase of `step_with_derived_clocks`.
    watch_vars: Vec<WatchVar>,
    /// User-defined component instances, created by `init_components`.
    pub components: Vec<RuntimeComponent>,
    /// True while the IR declares components but `init_components` has not
    /// run; stepping in that state would silently skip every hook.
    components_pending: bool,
    /// Waveform handles for component trace variables:
    /// (handle, component index, trace variable index).
    trace_dump_vars: Vec<(crate::wave_dumper::VarHandle, usize, usize)>,
}

struct WatchVar {
    label: String,
    ptr: *const u8,
    native_bytes: usize,
    width: u32,
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
        let n_derived = ir.derived_clock_schedule.clocks.len();
        let components_pending = !ir.external_components.is_empty();
        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_vars: Vec::new(),
            mask_cache: MaskCache::default(),
            comb_dirty: true,
            profile: Default::default(),
            last_event: None,
            last_event_stmts: std::ptr::null(),
            last_whole_event: None,
            prev_derived_clock_values: vec![0u8; n_derived],
            write_log_diag: WriteLogDiag {
                enabled: std::env::var("VERYL_WRITE_LOG_DIAG").as_deref() == Ok("1"),
                next_print_cycle: 1_000_000,
                ..Default::default()
            },
            cycle_limit: None,
            cycle_count: 0,
            watch_vars: Vec::new(),
            components: Vec::new(),
            components_pending,
            trace_dump_vars: Vec::new(),
        };

        if std::env::var("VERYL_DERIVED_CLOCK_DUMP").as_deref() == Ok("1") {
            fn find_var_by_ptr(
                module: &ModuleVariables,
                ptr: *const u8,
                prefix: &str,
            ) -> Option<String> {
                for v in module.variables.values() {
                    if v.current_values.first().copied() == Some(ptr as *mut u8) {
                        return Some(format!("{prefix}{}", v.path));
                    }
                }
                for child in &module.children {
                    if let Some(n) =
                        find_var_by_ptr(child, ptr, &format!("{prefix}{}.", child.name))
                    {
                        return Some(n);
                    }
                }
                None
            }
            for (i, clk) in ret.ir.derived_clock_schedule.clocks.iter().enumerate() {
                let raw = clk.current_offset.raw();
                let ptr = if raw >= 0 {
                    if clk.current_offset.is_ff() {
                        unsafe { ret.ir.ff_values.as_ptr().add(raw as usize) }
                    } else {
                        unsafe { ret.ir.comb_values.as_ptr().add(raw as usize) }
                    }
                } else {
                    std::ptr::null()
                };
                let name = find_var_by_ptr(&ret.ir.module_variables, ptr, "")
                    .unwrap_or_else(|| format!("{:?}@{raw}", clk.var_id));
                eprintln!(
                    "[derived_clock] [{i}] {name} is_ff={} has_events={}",
                    clk.current_offset.is_ff(),
                    ret.ir
                        .event_statements
                        .contains_key(&Event::Clock(clk.var_id)),
                );
            }
        }

        if std::env::var("VERYL_STEP_WATCH_LIST").as_deref() == Ok("1") {
            fn dump_tree(module: &ModuleVariables, prefix: &str) {
                for var in module.variables.values() {
                    eprintln!("[step_watch_list] {prefix}{}", var.path);
                }
                for child in &module.children {
                    dump_tree(child, &format!("{prefix}{}.", child.name));
                }
            }
            dump_tree(&ret.ir.module_variables, "");
        }

        if let Ok(watch) = std::env::var("VERYL_STEP_WATCH") {
            for path in watch.split(',').filter(|s| !s.is_empty()) {
                match Self::find_var_meta_in_module(&ret.ir.module_variables, path) {
                    Some((ptr, native_bytes, width)) => {
                        ret.watch_vars.push(WatchVar {
                            label: path.to_string(),
                            ptr,
                            native_bytes,
                            width,
                        });
                    }
                    None => eprintln!("[step_watch] UNRESOLVED: {path}"),
                }
            }
        }

        if let Some(dumper) = dump {
            ret.setup_dump(dumper);
        }

        // Seed prev values from the initial post-settle state.
        if n_derived > 0 {
            ret.do_settle_comb();
            ret.comb_dirty = false;
            for i in 0..n_derived {
                let clk = &ret.ir.derived_clock_schedule.clocks[i];
                ret.prev_derived_clock_values[i] = ret.read_derived_clock_bit(clk);
            }
        }

        ret
    }

    /// LSB of a 1-bit derived clock.  X/Z → 0 (matches posedge SV rule).
    fn read_derived_clock_bit(&self, clk: &crate::ir::DerivedClock) -> u8 {
        let raw = clk.current_offset.raw();
        if raw < 0 {
            return 0;
        }
        let off = raw as usize;
        let buf: &[u8] = if clk.current_offset.is_ff() {
            &self.ir.ff_values
        } else {
            &self.ir.comb_values
        };
        if off >= buf.len() {
            return 0;
        }
        let payload_bit = buf[off] & 1;
        if self.ir.use_4state {
            let mask_off = off + clk.native_bytes;
            if mask_off < buf.len() && (buf[mask_off] & 1) != 0 {
                return 0;
            }
        }
        payload_bit
    }

    fn set_input_clock_bit(&mut self, var_id: VarId, value: u8) {
        if let Some(var) = self.ir.module_variables.variables.get(&var_id) {
            let ptr = var.current_values[0];
            if ptr.is_null() {
                return;
            }
            // SAFETY: ptr is heap-stable for `self.ir`'s lifetime.
            // Writes LSB only (clocks are 1-bit).
            unsafe {
                let v = if value != 0 { 1u8 } else { 0u8 };
                *ptr = v;
                if self.ir.use_4state {
                    *ptr.add(var.native_bytes) = 0;
                }
            }
            self.comb_dirty = true;
        }
    }

    fn do_settle_comb(&mut self) {
        self.ir.settle_comb(&mut self.mask_cache, &mut self.profile);
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

    fn find_var_meta_in_module(
        module: &ModuleVariables,
        target: &str,
    ) -> Option<(*const u8, usize, u32)> {
        if let Some((head, rest)) = target.split_once('.') {
            for child in &module.children {
                if child.name.to_string() == head
                    && let Some(v) = Self::find_var_meta_in_module(child, rest)
                {
                    return Some(v);
                }
            }
        }
        for var in module.variables.values() {
            if var.path.to_string() == target {
                return Some((var.current_values[0], var.native_bytes, var.width as u32));
            }
        }
        None
    }

    fn dump_watch(&self, tag: &str) {
        if self.watch_vars.is_empty() {
            return;
        }
        let mut line = format!("[step_watch] t={} {tag}:", self.time);
        for w in &self.watch_vars {
            let value = unsafe {
                read_native_value(w.ptr, w.native_bytes, self.ir.use_4state, w.width, false)
            };
            line.push_str(&format!(" {}={:x?}", w.label, value));
        }
        eprintln!("{line}");
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
        // A missing init_components call would let the run pass vacuously
        // (no hook ever fires); catch that bug in debug builds without
        // paying an assert on every step.
        debug_assert!(
            !self.components_pending,
            "simulator has user-defined components but init_components was not called"
        );

        #[cfg(feature = "profile")]
        {
            self.profile.step_count += 1;
        }

        // Common case (no derived clocks) skips the edge-detect loop.
        if self.ir.derived_clock_schedule.is_empty() {
            self.step_legacy(event);
        } else {
            self.step_with_derived_clocks(event);
        }
    }

    fn step_legacy(&mut self, event: &Event) {
        // Install before settle_comb so comb-scope FF writes
        // (`--disable-ff-opt` path) hit a live log.
        // SAFETY: buffer outlives every dispatch_stmt_fast call below
        // and is cleared before this frame returns.
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

        self.step_event_inner(event);

        clear_event_write_log();
        self.comb_dirty = true;

        self.dump_variables();
    }

    /// Fire `event_statements[event]` then `ff_commit_from_log`.  The
    /// caller is responsible for `set_event_write_log`, `settle_comb`,
    /// and `dump_variables`.
    fn step_event_inner(&mut self, event: &Event) {
        let has_components = !self.components.is_empty();
        if has_components {
            self.stage_components(event);
        }
        self.eval_event_stmts(event);
        self.commit_event_log();
        if has_components {
            self.fire_components(event);
        }
    }

    /// Loads and creates every user-defined component in the IR, then runs
    /// `on_init` so initial output values are visible from the first
    /// settle. A returned error is a test failure.
    pub fn init_components(
        &mut self,
        seed_base: u64,
        test_name: &str,
    ) -> Result<(), ComponentError> {
        self.components_pending = false;
        if self.ir.external_components.is_empty() {
            return Ok(());
        }
        let mut components = build_components(&self.ir, seed_base, test_name)?;
        for c in &mut components {
            c.on_init();
            c.drain_logs();
            c.apply_outputs(&mut self.ir.module_variables, self.ir.use_4state);
        }
        self.comb_dirty = true;
        for c in &components {
            if c.host.failed() {
                let mut msgs = vec![];
                for c in &mut components {
                    msgs.extend(c.host.take_failures());
                }
                return Err(ComponentError::InitFailed {
                    messages: msgs.join("\n"),
                });
            }
        }
        self.components = components;
        Ok(())
    }

    /// Stages pre-edge input values for every component listening to
    /// `event`. Must run before `commit_event_log`.
    fn stage_components(&mut self, event: &Event) {
        if self.components.is_empty() {
            return;
        }
        let mut components = std::mem::take(&mut self.components);
        for c in &mut components {
            if c.listens_to(event) {
                c.stage_inputs(&mut self.mask_cache);
            }
        }
        self.components = components;
    }

    /// Fires component hooks for `event` and writes dirty outputs back.
    /// Must run after `commit_event_log` (the same edge's RTL then never
    /// observes component outputs — NBA semantics).
    fn fire_components(&mut self, event: &Event) {
        if self.components.is_empty() {
            return;
        }
        let mut components = std::mem::take(&mut self.components);
        let mut wrote = false;
        for c in &mut components {
            if c.listens_to(event) {
                c.fire(event, self.time);
                c.drain_logs();
                wrote |= c.apply_outputs(&mut self.ir.module_variables, self.ir.use_4state);
            }
        }
        self.components = components;
        if wrote {
            self.comb_dirty = true;
        }
    }

    pub fn component_finish_requested(&self) -> bool {
        self.components.iter().any(|c| c.host.finish_requested())
    }

    pub fn components_failed(&self) -> bool {
        self.components.iter().any(|c| c.host.failed())
    }

    pub fn take_component_failures(&mut self) -> Vec<String> {
        let mut msgs = vec![];
        for c in &mut self.components {
            msgs.extend(c.host.take_failures());
        }
        msgs
    }

    /// Zero-time method call on a component instance. An `Err` carries the
    /// failure messages the component reported (the test fails
    /// immediately).
    pub fn call_component_method(
        &mut self,
        inst: veryl_parser::resource_table::StrId,
        method: veryl_parser::resource_table::StrId,
        args: &[crate::component::host::HostValue],
    ) -> Result<crate::component::host::HostValue, String> {
        let Some(idx) = self.components.iter().position(|c| c.name_id == inst) else {
            let name = veryl_parser::resource_table::get_str_value(inst).unwrap_or_default();
            return Err(format!("unknown component instance `{name}`"));
        };
        let method_name = veryl_parser::resource_table::get_str_value(method).unwrap_or_default();
        let mut components = std::mem::take(&mut self.components);
        let c = &mut components[idx];
        c.host.time = self.time;
        let result = c.instance.call_method(&mut c.host, &method_name, args);
        c.drain_logs();
        let failures = c.host.take_failures();
        self.components = components;
        match result {
            Some(value) if failures.is_empty() => Ok(value),
            // Both `ctx.fail` during the call and a component error stop
            // the test immediately.
            None if failures.is_empty() => Err(format!("component method `{method_name}` failed")),
            _ => Err(failures.join("\n")),
        }
    }

    /// Fires `on_finish` on every component (end-of-test checks may still
    /// fail the test).
    pub fn finish_components(&mut self) {
        let mut components = std::mem::take(&mut self.components);
        for c in &mut components {
            c.on_finish();
            c.drain_logs();
        }
        self.components = components;
    }

    /// Evaluate `event_statements[event]` into the write log without
    /// committing, so simultaneous events (master + gated clocks) share
    /// one pre-commit state and one commit.
    fn eval_event_stmts(&mut self, event: &Event) {
        #[cfg(feature = "profile")]
        let event_start = std::time::Instant::now();

        // Cache both the per-stmt list AND the whole-event AOT-C handle for
        // the current event, keyed on `last_event`.  `event_statements` and
        // `whole_events` are both immutable after `Ir` construction, so the
        // raw pointers stay valid; this turns the per-cycle `whole_events`
        // HashMap probe + `Arc` clone into a single predicate check that the
        // per-stmt cache already pays for.
        let (stmts_ptr, whole_event_ptr) = if self.last_event.as_ref() == Some(event) {
            (self.last_event_stmts, self.last_whole_event)
        } else {
            let ptr: *const Vec<Statement> = match self.ir.event_statements.get(event) {
                Some(v) => v as *const _,
                None => std::ptr::null(),
            };
            let wptr: Option<*const dyn CompiledWhole> =
                self.ir.whole_events.get(event).map(Arc::as_ptr);
            self.last_event = Some(event.clone());
            self.last_event_stmts = ptr;
            self.last_whole_event = wptr;
            (ptr, wptr)
        };

        // Whole-event backend (today: AOT-C): if a backend committed to
        // a one-function compile for this event, invoke it in place of
        // the per-stmt Cranelift dispatch.  The function reads ff/comb
        // current values and pushes WriteLogEntries into the buffer
        // (3rd arg), exactly as the Cranelift event JIT does;
        // `ff_commit_from_log` below applies them.
        use crate::backend::DispatchOutcome;
        let dispatched = if let Some(wptr) = whole_event_ptr {
            // SAFETY: `wptr` = `Arc::as_ptr` of an `Arc` owned by
            // `self.ir.whole_events`, which is never mutated after `Ir`
            // construction, so the pointee outlives this call.  Same
            // invariant the `last_event_stmts` raw pointer relies on.
            let whole: &dyn CompiledWhole = unsafe { &*wptr };
            let ff_ptr = self.ir.ff_values.as_ptr();
            let comb_ptr = self.ir.comb_values.as_ptr() as *mut u8;
            let log_ptr = (&*self.ir.write_log_buffer) as *const _ as *mut u8;

            // VERYL_AOT_C_VALIDATE=1: dual-run paths and diff.  Default-off.
            let validate = self.ir.aot_c_validate;

            if !validate {
                matches!(
                    whole.try_dispatch(ff_ptr, comb_ptr, log_ptr),
                    DispatchOutcome::Done,
                )
            } else {
                // For validate, the wrapper compares the whole-event
                // dispatch against the per-stmt Cranelift path and panics
                // on divergence.  The whole-event backend only exists on
                // native (BackendRegistry stays empty on wasm), so this
                // branch is effectively native-only at runtime.  NotReady
                // returns false → normal per-stmt fallback below.
                self.validate_event_aot(whole, stmts_ptr)
            }
        } else {
            false
        };

        if !dispatched && !stmts_ptr.is_null() {
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
    }

    /// Apply the accumulated write log to FF storage and reset the buffer.
    fn commit_event_log(&mut self) {
        #[cfg(feature = "profile")]
        let ff_start = std::time::Instant::now();

        ff_commit_from_log(&mut self.ir.ff_values, &self.ir.write_log_buffer);

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
    }

    /// Toggles master 0→1, fires the event + chained derived-clock
    /// events, then restores master=0 so `prev_derived_clock_values`
    /// samples on a consistent baseline.
    fn step_with_derived_clocks(&mut self, event: &Event) {
        // SAFETY: same as `step_legacy`; one install covers settle_comb
        // plus every step_event_inner fire in this step.
        unsafe {
            set_event_write_log(&mut self.ir.write_log_buffer);
        }

        // Hoisted like `has_eval_chunk` so loops test a local bool.
        let watch_enabled = !self.watch_vars.is_empty();

        // Subsequent partial_settle only refreshes the dep subset, so
        // the rest of the design must already be settled.
        if self.comb_dirty {
            self.do_settle_comb();
            self.comb_dirty = false;
        }
        if watch_enabled {
            self.dump_watch("after_settle");
        }

        let master_id_opt = match event {
            Event::Clock(id) | Event::Reset(id) => {
                let id = *id;
                let is_master = self
                    .ir
                    .derived_clock_schedule
                    .master_input_clocks
                    .contains(&id);
                if is_master { Some(id) } else { None }
            }
            _ => None,
        };

        let has_eval_chunk = !self.ir.derived_clock_eval_stmts.is_empty();

        // Master high → gated-clock exprs see the rising edge.
        if let Some(id) = master_id_opt {
            self.set_input_clock_bit(id, 1);
            if has_eval_chunk {
                self.ir.partial_settle(&mut self.mask_cache);
            }
        }

        // Two-phase firing.  A master-gated clock's edge IS the master
        // edge qualified by the pre-commit enable (ICG semantics), so
        // those clocks fire here with the master event, sharing its
        // pre-commit state and write-log commit; a same-cycle enable
        // change waits for the next edge (the post-commit loop skips
        // them).  FF-driven clocks fire post-commit instead, matching
        // SV's NBA-driven edge propagation.
        let n = self.ir.derived_clock_schedule.clocks.len();
        let mut fired_mask: Vec<bool> = vec![false; n];
        let mut pre_fire: SmallVec<[usize; 8]> = SmallVec::new();
        if master_id_opt.is_some() {
            for i in 0..n {
                let clk = &self.ir.derived_clock_schedule.clocks[i];
                if clk.current_offset.is_ff() || !clk.master_gated {
                    continue;
                }
                if self.prev_derived_clock_values[i] == 0 && self.read_derived_clock_bit(clk) == 1 {
                    pre_fire.push(i);
                }
            }
        }

        self.stage_components(event);
        for &i in &pre_fire {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            self.stage_components(&Event::Clock(vid));
        }
        self.eval_event_stmts(event);
        for &i in &pre_fire {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            if watch_enabled {
                self.dump_watch(&format!("pre_fire[{i}]"));
            }
            self.eval_event_stmts(&Event::Clock(vid));
            fired_mask[i] = true;
        }
        self.commit_event_log();
        self.fire_components(event);
        for &i in &pre_fire {
            let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
            self.fire_components(&Event::Clock(vid));
        }
        if watch_enabled {
            self.dump_watch("after_master_event");
        }

        // Detect remaining 0→1 edges (caused by this step's FF commits)
        // and chain-fire one at a time, re-evaluating after each fire so
        // NBA glitch suppression works (a transient edge cancelled by a
        // same-cycle FF write must not trigger).
        // Convergence: each clock fires at most once (`fired_mask`) and
        // `analyze_dependency` rejects comb cycles, so n+1 iterations
        // suffice; the debug_assert catches bookkeeping regressions.
        let mut new_values: SmallVec<[u8; 8]> = SmallVec::new();
        new_values.resize(n, 0);
        let max_iters = n + 1;
        let mut iters = 0;
        loop {
            if has_eval_chunk {
                self.ir.partial_settle(&mut self.mask_cache);
            }
            for i in 0..n {
                let clk = &self.ir.derived_clock_schedule.clocks[i];
                new_values[i] = self.read_derived_clock_bit(clk);
            }

            // Earliest unfired clock with a real 0→1 edge.  An edge on a
            // master-gated comb clock here is a committed enable change,
            // which must not pulse (see the pre-commit phase above).
            let mut next_fire: Option<usize> = None;
            for i in 0..n {
                if fired_mask[i] {
                    continue;
                }
                let clk = &self.ir.derived_clock_schedule.clocks[i];
                if !clk.current_offset.is_ff() && clk.master_gated {
                    continue;
                }
                if self.prev_derived_clock_values[i] == 0 && new_values[i] == 1 {
                    next_fire = Some(i);
                    break;
                }
            }

            match next_fire {
                Some(i) => {
                    // The partial settle only refreshed the clock closure
                    // and the fired domain reads arbitrary comb, so settle
                    // fully before firing (paid only when an edge fires).
                    self.do_settle_comb();
                    // Re-verify on the fully settled state: the partial
                    // closure can show a transient that full settling
                    // cancels.  Not marked fired — the next iteration
                    // re-reads a consistent 0, so the loop still ends.
                    let clk = &self.ir.derived_clock_schedule.clocks[i];
                    if self.read_derived_clock_bit(clk) != 1 {
                        continue;
                    }
                    iters += 1;
                    debug_assert!(
                        iters <= max_iters,
                        "derived clock fixpoint exceeded n+1 iterations (n={n})",
                    );
                    let vid = self.ir.derived_clock_schedule.clocks[i].var_id;
                    if watch_enabled {
                        self.dump_watch(&format!("before_derived[{i}]"));
                    }
                    self.step_event_inner(&Event::Clock(vid));
                    if watch_enabled {
                        self.dump_watch(&format!("after_derived[{i}]"));
                    }
                    fired_mask[i] = true;
                }
                None => break,
            }
        }

        // master=0 + resettle so the prev snapshot matches the next
        // step's starting baseline.
        if let Some(id) = master_id_opt {
            self.set_input_clock_bit(id, 0);
            if has_eval_chunk {
                self.ir.partial_settle(&mut self.mask_cache);
            }
        }

        for i in 0..n {
            let clk = &self.ir.derived_clock_schedule.clocks[i];
            self.prev_derived_clock_values[i] = self.read_derived_clock_bit(clk);
        }

        clear_event_write_log();
        self.comb_dirty = true;
        self.dump_variables();
    }

    /// VERYL_AOT_C_VALIDATE event-path check: run the AOT-C event function and
    /// the Cranelift per-stmt dispatch on identical inputs, compare the
    /// WriteLogEntries they push plus any direct ff/comb writes, and panic on
    /// first divergence.  Leaves the Cranelift result live (ground truth).
    /// Slow (clones ff/comb each event) — diagnostics only.  Unreachable on
    /// wasm since no whole-event backend ever registers there.
    ///
    /// Returns true when the AOT function ran (and was compared).  NotReady —
    /// the compile hasn't landed (or failed) — writes nothing, so comparing
    /// would diff an empty log against the real Cranelift effect and report a
    /// phantom divergence; mirror the non-validate path instead: return false
    /// and let the caller run the per-stmt Cranelift dispatch.
    fn validate_event_aot(
        &mut self,
        whole: &dyn CompiledWhole,
        stmts_ptr: *const Vec<Statement>,
    ) -> bool {
        // On off-stride cycles return false (skipping the AOT-C compare) so the
        // caller runs the per-stmt Cranelift dispatch — the ground truth.
        let stride = self.ir.aot_c_validate_stride;
        if stride > 1 {
            thread_local! {
                static EV_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
            }
            let sample = EV_COUNT.with(|c| {
                let v = c.get();
                c.set(v.wrapping_add(1));
                v % stride == 0
            });
            if !sample {
                return false;
            }
        }
        let ff_ptr = self.ir.ff_values.as_ptr();
        let comb_ptr = self.ir.comb_values.as_ptr() as *mut u8;
        let log_ptr = (&*self.ir.write_log_buffer) as *const _ as *mut u8;

        let ff_snap = self.ir.ff_values.to_vec();
        let comb_snap = self.ir.comb_values.to_vec();
        let count_before = self.ir.write_log_buffer.narrow_count as usize;
        let wide_count_before = self.ir.write_log_buffer.wide_count as usize;

        // Whole-event backend, then capture its pushed entries + ff/comb.
        if matches!(
            whole.try_dispatch(ff_ptr, comb_ptr, log_ptr),
            crate::backend::DispatchOutcome::NotReady,
        ) {
            return false;
        }
        // The committed FF effect is what `ff_commit_from_log` writes: all
        // narrow entries first (typed stores of `width_class` bytes), then all
        // wide entries (memcpy of `native_bytes`), last-write-wins per byte.
        // The SAME committed value can be routed through DIFFERENT pools by
        // different backends — a 65-128 bit FF is ONE wide entry for AOT-C /
        // interpret but TWO narrow u64 entries for the Cranelift JIT — so we
        // must compare the RESOLVED per-byte effect, not pool-specific entry
        // maps, or a byte-identical commit would false-positive.  (The dual-slot
        // "next slot" direct writes are vestigial; only the log drives commit.)
        let committed_bytes =
            |buf: &WriteLogBuffer, nlo: usize, nhi: usize, wlo: usize, whi: usize| {
                let mut m: HashMap<u32, u8> = Default::default();
                for e in &buf.narrow_entries_slice()[nlo..nhi] {
                    let nb = (e.width_class as usize).min(8);
                    let bytes = e.payload.to_le_bytes();
                    for (i, &b) in bytes.iter().take(nb).enumerate() {
                        m.insert(e.offset + i as u32, b);
                    }
                }
                for e in &buf.wide_entries_slice()[wlo..whi] {
                    let nb = (e.native_bytes as usize).min(e.payload.len());
                    for (i, &b) in e.payload.iter().take(nb).enumerate() {
                        m.insert(e.offset + i as u32, b);
                    }
                }
                m
            };
        let aot_count = self.ir.write_log_buffer.narrow_count as usize;
        let aot_wide_count = self.ir.write_log_buffer.wide_count as usize;
        let aot_bytes = committed_bytes(
            &self.ir.write_log_buffer,
            count_before,
            aot_count,
            wide_count_before,
            aot_wide_count,
        );

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
        self.ir.write_log_buffer.wide_count = wide_count_before as u32;
        if !stmts_ptr.is_null() {
            let statements: &Vec<Statement> = unsafe { &*stmts_ptr };
            for x in statements {
                dispatch_stmt_fast(x, &mut self.mask_cache);
            }
        }
        let cr_count = self.ir.write_log_buffer.narrow_count as usize;
        let cr_wide_count = self.ir.write_log_buffer.wide_count as usize;
        let cr_bytes = committed_bytes(
            &self.ir.write_log_buffer,
            count_before,
            cr_count,
            wide_count_before,
            cr_wide_count,
        );

        if aot_bytes != cr_bytes {
            eprintln!(
                "[aot_event_validate] DIVERGENCE module={} event={:?}: committed-FF bytes differ (aot {} bytes, cranelift {} bytes)",
                self.ir.name,
                self.last_event,
                aot_bytes.len(),
                cr_bytes.len(),
            );
            let mut offs: BTreeSet<u32> = Default::default();
            offs.extend(aot_bytes.keys());
            offs.extend(cr_bytes.keys());
            for off in offs {
                let a = aot_bytes.get(&off);
                let c = cr_bytes.get(&off);
                if a != c {
                    eprintln!("  byte off={off:#x}: aot={a:?} cranelift={c:?}");
                }
            }
            panic!("AOT-C event validate divergence (see above)");
        }
        true
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
            Self::dump_trace_vars(dump, &self.trace_dump_vars, &self.components);
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
            Self::dump_trace_vars(dump, &self.trace_dump_vars, &self.components);
        }
    }

    fn dump_trace_vars(
        dump: &mut WaveDumper,
        trace_dump_vars: &[(crate::wave_dumper::VarHandle, usize, usize)],
        components: &[RuntimeComponent],
    ) {
        for &(handle, comp_idx, trace_idx) in trace_dump_vars {
            let var = &components[comp_idx].host.trace_vars[trace_idx];
            let mut value = crate::component::runtime::words_to_value(&var.words, var.width);
            // Excess high bits written by the component must not leak
            // into the waveform.
            value.trunc(var.width as usize);
            dump.change_vector(handle, &value);
        }
    }

    /// Sets up waveform dumping. Called via `Simulator::new` when no
    /// components are involved; the native-test flow calls `attach_dump`
    /// after `init_components` instead, so component trace variables
    /// (registered during `create`) make it into the header.
    fn setup_dump(&mut self, mut dumper: WaveDumper) {
        dumper.timescale();
        dumper.setup_module(&self.ir.module_variables, &mut self.dump_vars);
        for (comp_idx, comp) in self.components.iter().enumerate() {
            if comp.host.trace_vars.is_empty() {
                continue;
            }
            dumper.add_module(&comp.name);
            for (trace_idx, var) in comp.host.trace_vars.iter().enumerate() {
                let handle = dumper.add_wire(var.width, &var.name);
                self.trace_dump_vars.push((handle, comp_idx, trace_idx));
            }
            dumper.upscope();
        }
        dumper.finish_header();
        self.dump = Some(dumper);
    }

    /// See `setup_dump`; the public entry used after `init_components`.
    pub fn attach_dump(&mut self, dumper: WaveDumper) {
        self.setup_dump(dumper);
    }
}
