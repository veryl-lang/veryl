use crate::ir::{Event, Ir, Value, VarId, VarPath};
use std::collections::HashMap;
use std::str::FromStr;
use vcd::{self, IdCode, SimulationCommand, TimescaleUnit};
use veryl_analyzer::value::MaskCache;

pub struct Simulator<T: std::io::Write> {
    pub ir: Ir,
    pub time: u64,
    pub dump: Option<vcd::Writer<T>>,
    pub dump_code: HashMap<VarId, IdCode>,
    pub mask_cache: MaskCache,
}

impl<T: std::io::Write> Simulator<T> {
    pub fn new(ir: Ir, dump: Option<T>) -> Self {
        //let _ = rayon::ThreadPoolBuilder::new()
        //    .num_threads(2)
        //    .build_global();

        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_code: HashMap::default(),
            mask_cache: MaskCache::default(),
        };

        if let Some(x) = dump {
            ret.setup_dump(x);
        }

        ret
    }

    pub fn set(&mut self, port: &str, value: Value) {
        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.ir.ports.get(&port)
            && let Some(x) = self.ir.variables.get_mut(id)
        {
            unsafe {
                (*x.current_values[0]).set_value(value);
            }
        }
    }

    pub fn get(&mut self, port: &str) -> Option<Value> {
        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.ir.ports.get(&port)
            && let Some(x) = self.ir.variables.get_mut(id)
        {
            let value = unsafe { (*x.current_values[0]).clone() };
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
        self.ir.eval_step(event, &mut self.mask_cache);

        for x in &mut self.ir.ff_values {
            x.swap()
        }

        self.dump_variables();
        self.time += 1;
    }

    pub fn dump_start(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.begin(SimulationCommand::Dumpvars).unwrap();
            for (id, x) in &self.ir.variables {
                let code = self.dump_code.get(id).unwrap();
                let value = unsafe { &*x.current_values[0] };
                dump.change_vector(*code, value).unwrap();
            }
            dump.end().unwrap();
        }
    }

    fn dump_variables(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.timestamp(self.time).unwrap();
            for (id, x) in &self.ir.variables {
                let code = self.dump_code.get(id).unwrap();
                let value = unsafe { &*x.current_values[0] };
                dump.change_vector(*code, value).unwrap();
            }
        }
    }

    fn setup_dump(&mut self, io: T) {
        let mut dump = vcd::Writer::new(io);

        dump.timescale(1, TimescaleUnit::US).unwrap();
        dump.add_module(&self.ir.name.to_string()).unwrap();

        for (id, x) in &self.ir.variables {
            let name = x.path.to_string();
            let width = x.width as u32;
            let code = dump.add_wire(width, &name).unwrap();
            self.dump_code.insert(*id, code);
        }

        dump.upscope().unwrap();
        dump.enddefinitions().unwrap();

        self.dump = Some(dump);
    }
}
