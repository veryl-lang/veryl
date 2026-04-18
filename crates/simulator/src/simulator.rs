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
}

impl Simulator {
    pub fn new(ir: Ir, dump: Option<WaveDumper>) -> Self {
        let comb_snapshot_buf = vec![0u8; ir.comb_values.len()];
        let needs_convergence_check = ir.required_comb_passes > 1;
        let disable_ff_opt = ir.disable_ff_opt;
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
        };

        if let Some(dumper) = dump {
            ret.setup_dump(dumper);
        }

        ret
    }

    fn do_settle_comb(&mut self) {
        let skip = self.convergence_verified;
        let converged_first = self.ir.settle_comb(
            &mut self.mask_cache,
            &mut self.comb_snapshot_buf,
            &mut self.profile,
            skip,
        );
        if !skip && self.convergence_warmup > 0 {
            if converged_first {
                self.convergence_warmup -= 1;
                if self.convergence_warmup == 0 {
                    self.convergence_verified = true;
                }
            } else {
                // Reset warmup if convergence failed on first try
                self.convergence_warmup = CONVERGENCE_WARMUP;
            }
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
            for x in statements {
                x.eval_step(&mut self.mask_cache);
            }
        }

        #[cfg(feature = "profile")]
        {
            self.profile.event_eval_ns += event_start.elapsed().as_nanos() as u64;
        }

        #[cfg(feature = "profile")]
        let ff_start = std::time::Instant::now();

        Self::ff_commit(&mut self.ir.ff_values, &self.ir.ff_commit_entries);

        #[cfg(feature = "profile")]
        {
            self.profile.ff_swap_ns += ff_start.elapsed().as_nanos() as u64;
        }

        self.comb_dirty = true;

        self.dump_variables();
    }

    /// Commit FF updates: copy next → current for all FF variables.
    /// Constant-size copies allow LLVM to inline as MOV instructions.
    #[inline(always)]
    fn ff_commit(ff_values: &mut [u8], entries: &[(usize, usize)]) {
        let ptr = ff_values.as_mut_ptr();
        for &(current_offset, value_size) in entries {
            unsafe {
                let dst = ptr.add(current_offset);
                let src = ptr.add(current_offset + value_size);
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
