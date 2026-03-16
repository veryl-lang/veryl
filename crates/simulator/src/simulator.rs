use crate::HashMap;
use crate::ir::{
    Event, Ir, ModuleVariables, Value, VarId, VarPath, read_native_value, write_native_value,
};
use std::str::FromStr;
use vcd::{self, IdCode, SimulationCommand, TimescaleUnit};
use veryl_analyzer::value::MaskCache;

pub struct Simulator<T: std::io::Write> {
    pub ir: Ir,
    pub time: u64,
    pub dump: Option<vcd::Writer<T>>,
    pub dump_code: HashMap<VarId, IdCode>,
    pub mask_cache: MaskCache,
    comb_dirty: bool,
}

impl<T: std::io::Write> Simulator<T> {
    pub fn new(ir: Ir, dump: Option<T>) -> Self {
        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_code: HashMap::default(),
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
        if self.comb_dirty {
            self.ir.eval_comb_full(&mut self.mask_cache);
            self.comb_dirty = false;
        }

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
            self.ir.eval_comb(&mut self.mask_cache);
            self.comb_dirty = false;
        }

        if let Some(statements) = self.ir.event_statements.get(event) {
            for x in statements {
                x.eval_step(&mut self.mask_cache);
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

    pub fn dump_start(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.begin(SimulationCommand::Dumpvars).unwrap();
            Self::dump_module_variables_values(
                &self.ir.module_variables,
                &self.dump_code,
                dump,
                self.ir.use_4state,
            );
            dump.end().unwrap();
        }
    }

    fn dump_variables(&mut self) {
        if let Some(dump) = &mut self.dump {
            if self.comb_dirty {
                self.ir.eval_comb_full(&mut self.mask_cache);
                self.comb_dirty = false;
            }
            dump.timestamp(self.time).unwrap();
            Self::dump_module_variables_values(
                &self.ir.module_variables,
                &self.dump_code,
                dump,
                self.ir.use_4state,
            );
        }
    }

    fn dump_module_variables_values(
        module_vars: &ModuleVariables,
        dump_code: &HashMap<VarId, IdCode>,
        dump: &mut vcd::Writer<T>,
        use_4state: bool,
    ) {
        for (id, x) in &module_vars.variables {
            if let Some(code) = dump_code.get(id) {
                let value = unsafe {
                    read_native_value(
                        x.current_values[0],
                        x.native_bytes,
                        use_4state,
                        x.width as u32,
                        false,
                    )
                };
                dump.change_vector(*code, &value).unwrap();
            }
        }
        for child in &module_vars.children {
            Self::dump_module_variables_values(child, dump_code, dump, use_4state);
        }
    }

    fn setup_dump(&mut self, io: T) {
        let mut dump = vcd::Writer::new(io);

        dump.timescale(1, TimescaleUnit::US).unwrap();

        Self::setup_dump_module(&self.ir.module_variables, &mut dump, &mut self.dump_code);

        dump.enddefinitions().unwrap();

        self.dump = Some(dump);
    }

    fn setup_dump_module(
        module_vars: &ModuleVariables,
        dump: &mut vcd::Writer<T>,
        dump_code: &mut HashMap<VarId, IdCode>,
    ) {
        dump.add_module(&module_vars.name.to_string()).unwrap();

        for (id, x) in &module_vars.variables {
            let name = x.path.to_string();
            let width = x.width as u32;
            let code = dump.add_wire(width, &name).unwrap();
            dump_code.insert(*id, code);
        }

        for child in &module_vars.children {
            Self::setup_dump_module(child, dump, dump_code);
        }

        dump.upscope().unwrap();
    }
}
