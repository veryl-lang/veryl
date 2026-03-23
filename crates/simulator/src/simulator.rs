use crate::ir::{
    Event, Ir, ModuleVariables, Value, VarId, VarPath, read_native_value, write_native_value,
};
use std::str::FromStr;
use vcd::{self, IdCode, SimulationCommand, TimescaleUnit};
use veryl_analyzer::value::MaskCache;

/// A single variable entry for VCD dump, collected during header setup
/// and replayed on every timestamp.
struct DumpVar {
    code: IdCode,
    ptr: *const u8,
    native_bytes: usize,
    width: usize,
}

pub struct Simulator<T: std::io::Write> {
    pub ir: Ir,
    pub time: u64,
    pub dump: Option<vcd::Writer<T>>,
    dump_vars: Vec<DumpVar>,
    pub mask_cache: MaskCache,
    comb_dirty: bool,
}

impl<T: std::io::Write> Simulator<T> {
    pub fn new(ir: Ir, dump: Option<T>) -> Self {
        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_vars: Vec::new(),
            mask_cache: MaskCache::default(),
            comb_dirty: true,
        };

        if let Some(x) = dump {
            ret.setup_dump(x);
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
        self.time += 1;
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
    /// into the variable storage so they appear in VCD dumps.
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

    /// Emit an additional VCD timestamp with the current variable state.
    /// Does NOT execute events or FF swap — used for negedge dump points.
    pub fn dump_and_advance_time(&mut self) {
        self.dump_variables();
        self.time += 1;
    }

    pub fn dump_start(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.begin(SimulationCommand::Dumpvars).unwrap();
            Self::dump_all_vars(&self.dump_vars, dump, self.ir.use_4state);
            dump.end().unwrap();
        }
    }

    fn dump_variables(&mut self) {
        if let Some(dump) = &mut self.dump {
            if self.comb_dirty {
                self.ir.settle_comb(&mut self.mask_cache);
                self.comb_dirty = false;
            }
            dump.timestamp(self.time).unwrap();
            Self::dump_all_vars(&self.dump_vars, dump, self.ir.use_4state);
        }
    }

    fn dump_all_vars(dump_vars: &[DumpVar], dump: &mut vcd::Writer<T>, use_4state: bool) {
        for entry in dump_vars {
            let mut value = unsafe {
                read_native_value(
                    entry.ptr,
                    entry.native_bytes,
                    use_4state,
                    entry.width as u32,
                    false,
                )
            };
            // Mask off upper bits beyond declared width so VCD output
            // matches the $var header.
            value.trunc(entry.width);
            dump.change_vector(entry.code, &value).unwrap();
        }
    }

    fn setup_dump(&mut self, io: T) {
        let mut dump = vcd::Writer::new(io);

        dump.timescale(1, TimescaleUnit::US).unwrap();

        Self::setup_dump_module(&self.ir.module_variables, &mut dump, &mut self.dump_vars);

        dump.enddefinitions().unwrap();

        self.dump = Some(dump);
    }

    fn setup_dump_module(
        module_vars: &ModuleVariables,
        dump: &mut vcd::Writer<T>,
        dump_vars: &mut Vec<DumpVar>,
    ) {
        dump.add_module(&module_vars.name.to_string()).unwrap();

        for x in module_vars.variables.values() {
            let name = x.path.to_string();
            let width = x.width as u32;
            let code = dump.add_wire(width, &name).unwrap();
            dump_vars.push(DumpVar {
                code,
                ptr: x.current_values[0],
                native_bytes: x.native_bytes,
                width: x.width,
            });
        }

        for child in &module_vars.children {
            Self::setup_dump_module(child, dump, dump_vars);
        }

        dump.upscope().unwrap();
    }
}
