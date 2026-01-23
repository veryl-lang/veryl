use std::collections::HashMap;
use std::str::FromStr;
use vcd::{self, IdCode, SimulationCommand, TimescaleUnit};
use veryl_analyzer::ir::{Component, Event, Ir, Module, VarId, VarPath};
use veryl_analyzer::value::Value;
use veryl_parser::resource_table;

pub struct Simulator<T: std::io::Write> {
    pub top: Module,
    pub time: u64,
    pub dump: Option<vcd::Writer<T>>,
    pub dump_code: HashMap<VarId, IdCode>,
}

impl<T: std::io::Write> Simulator<T> {
    pub fn new(top: &str, ir: Ir, dump: Option<T>) -> Option<Self> {
        let top = resource_table::insert_str(top);
        for x in ir.components {
            if let Component::Module(x) = x
                && x.name == top
            {
                let mut ret = Self {
                    top: x,
                    time: 0,
                    dump: None,
                    dump_code: HashMap::default(),
                };

                if let Some(dump) = dump {
                    ret.setup_dump(dump);
                }

                return Some(ret);
            }
        }
        None
    }

    pub fn set(&mut self, port: &str, value: Value) {
        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.top.ports.get(&port)
            && let Some(x) = self.top.variables.get_mut(id)
        {
            x.value[0] = value;
        }
    }

    pub fn get(&mut self, port: &str) -> Option<Value> {
        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.top.ports.get(&port)
            && let Some(x) = self.top.variables.get_mut(id)
        {
            Some(x.value[0].clone())
        } else {
            None
        }
    }

    pub fn get_clock(&self, port: &str) -> Option<Event> {
        let port = VarPath::from_str(port).unwrap();

        self.top.ports.get(&port).map(|id| Event::Clock(*id))
    }

    pub fn get_reset(&self, port: &str) -> Option<Event> {
        let port = VarPath::from_str(port).unwrap();

        self.top.ports.get(&port).map(|id| Event::Reset(*id))
    }

    pub fn step(&mut self, event: &Event) {
        self.top.eval_step(event);
        self.dump_variables();
        self.time += 1;
    }

    pub fn dump_start(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.begin(SimulationCommand::Dumpvars).unwrap();
            for (id, x) in &self.top.variables {
                let code = self.dump_code.get(id).unwrap();
                let value = &x.value[0];
                dump.change_vector(*code, value).unwrap();
            }
            dump.end().unwrap();
        }
    }

    fn dump_variables(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.timestamp(self.time).unwrap();
            for (id, x) in &self.top.variables {
                let code = self.dump_code.get(id).unwrap();
                let value = &x.value[0];
                dump.change_vector(*code, value).unwrap();
            }
        }
    }

    fn setup_dump(&mut self, io: T) {
        let mut dump = vcd::Writer::new(io);

        dump.timescale(1, TimescaleUnit::US).unwrap();
        dump.add_module(&self.top.name.to_string()).unwrap();

        for (id, x) in &self.top.variables {
            let name = x.path.to_string();
            let width = x.r#type.total_width().unwrap() as u32;
            let code = dump.add_wire(width, &name).unwrap();
            self.dump_code.insert(*id, code);
        }

        dump.upscope().unwrap();
        dump.enddefinitions().unwrap();

        self.dump = Some(dump);
    }
}
