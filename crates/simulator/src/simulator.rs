use crate::ir::{
    Event, Ir, ModuleVariables, Value, VarId, VarPath, read_native_value, write_native_value,
};
use crate::wave_dumper::{DumpVar, WaveDumper};
use std::str::FromStr;
use veryl_analyzer::value::MaskCache;

pub struct Simulator {
    pub ir: Ir,
    pub time: u64,
    pub dump: Option<WaveDumper>,
    dump_vars: Vec<DumpVar>,
    pub mask_cache: MaskCache,
    comb_dirty: bool,
}

impl Simulator {
    pub fn new(ir: Ir, dump: Option<WaveDumper>) -> Self {
        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_vars: Vec::new(),
            mask_cache: MaskCache::default(),
            comb_dirty: true,
        };

        if let Some(dumper) = dump {
            ret.setup_dump(dumper);
        }

        ret
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
            self.ir.settle_comb(&mut self.mask_cache);
            self.comb_dirty = false;
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
        if self.comb_dirty {
            self.ir.settle_comb(&mut self.mask_cache);
            self.comb_dirty = false;
        }

        if let Some(statements) = self.ir.event_statements.get(event) {
            for x in statements {
                x.eval_step(&mut self.mask_cache);
            }
            // After events, re-settle comb when merged comb+event functions
            // or multi-level hierarchies need their outputs propagated through
            // port connections before ff_swap. Simple designs skip this —
            // comb will be re-evaluated at the start of the next step().
            if self.ir.use_full_comb_in_step || !self.ir.post_comb_fns.is_empty() {
                self.ir.settle_comb(&mut self.mask_cache);
            }
        }

        Self::ff_swap(&mut self.ir.ff_values, &self.ir.ff_swap_entries);

        self.comb_dirty = true;

        self.dump_variables();
    }

    #[inline(always)]
    fn ff_swap(ff_values: &mut [u8], entries: &[(usize, usize)]) {
        let ptr = ff_values.as_mut_ptr();
        for &(current_offset, value_size) in entries {
            let next_offset = current_offset + value_size;
            unsafe {
                match value_size {
                    4 => {
                        let a = ptr.add(current_offset) as *mut u32;
                        let b = ptr.add(next_offset) as *mut u32;
                        std::ptr::swap(a, b);
                    }
                    8 => {
                        let a = ptr.add(current_offset) as *mut u64;
                        let b = ptr.add(next_offset) as *mut u64;
                        std::ptr::swap(a, b);
                    }
                    _ => {
                        std::ptr::swap_nonoverlapping(
                            ptr.add(current_offset),
                            ptr.add(next_offset),
                            value_size,
                        );
                    }
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
        if let Some(dump) = &mut self.dump {
            if self.comb_dirty {
                self.ir.settle_comb(&mut self.mask_cache);
                self.comb_dirty = false;
            }
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
