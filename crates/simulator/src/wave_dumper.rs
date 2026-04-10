use crate::ir::{ModuleVariables, Value, read_native_value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use vcd::{self, SimulationCommand, TimescaleUnit};

/// Write adapter backed by a shared `Vec<u8>`, used in tests to capture VCD output.
pub struct SharedVec(pub Arc<Mutex<Vec<u8>>>);

impl Write for SharedVec {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum VarHandle {
    Vcd(vcd::IdCode),
    Fst(fst_writer::FstSignalId),
}

pub struct WaveDumper {
    kind: WaveDumperKind,
    path: Option<PathBuf>,
}

enum WaveDumperKind {
    Vcd(VcdDumper),
    Fst(Box<FstDumper>),
}

struct VcdDumper {
    writer: vcd::Writer<Box<dyn Write + Send>>,
}

struct FstDumper {
    state: FstState,
}

enum FstState {
    Header(fst_writer::FstHeaderWriter<std::io::BufWriter<std::fs::File>>),
    Body(fst_writer::FstBodyWriter<std::io::BufWriter<std::fs::File>>),
    Transitioning,
}

impl WaveDumper {
    pub fn new_vcd(io: Box<dyn Write + Send>) -> Self {
        WaveDumper {
            kind: WaveDumperKind::Vcd(VcdDumper {
                writer: vcd::Writer::new(io),
            }),
            path: None,
        }
    }

    pub fn new_fst(path: &str) -> Self {
        let info = fst_writer::FstInfo {
            start_time: 0,
            timescale_exponent: -6, // 1us
            version: "Veryl Simulator".to_string(),
            date: String::new(),
            file_type: fst_writer::FstFileType::Verilog,
        };
        let header = fst_writer::open_fst(path, &info).expect("failed to create FST file");
        WaveDumper {
            kind: WaveDumperKind::Fst(Box::new(FstDumper {
                state: FstState::Header(header),
            })),
            path: None,
        }
    }

    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn into_path(self) -> Option<PathBuf> {
        self.path
    }

    pub fn timescale(&mut self) {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                v.writer.timescale(1, TimescaleUnit::US).unwrap();
            }
            WaveDumperKind::Fst(_) => {
                // Already set in FstInfo during construction
            }
        }
    }

    pub fn add_module(&mut self, name: &str) {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                v.writer.add_module(name).unwrap();
            }
            WaveDumperKind::Fst(f) => match &mut f.state {
                FstState::Header(h) => {
                    h.scope(name, "", fst_writer::FstScopeType::Module).unwrap();
                }
                _ => panic!("FST: add_module called after header finished"),
            },
        }
    }

    pub fn add_wire(&mut self, width: u32, name: &str) -> VarHandle {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                let code = v.writer.add_wire(width, name).unwrap();
                VarHandle::Vcd(code)
            }
            WaveDumperKind::Fst(f) => match &mut f.state {
                FstState::Header(h) => {
                    let id = h
                        .var(
                            name,
                            fst_writer::FstSignalType::bit_vec(width),
                            fst_writer::FstVarType::Wire,
                            fst_writer::FstVarDirection::Implicit,
                            None,
                        )
                        .unwrap();
                    VarHandle::Fst(id)
                }
                _ => panic!("FST: add_wire called after header finished"),
            },
        }
    }

    pub fn upscope(&mut self) {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                v.writer.upscope().unwrap();
            }
            WaveDumperKind::Fst(f) => match &mut f.state {
                FstState::Header(h) => {
                    h.up_scope().unwrap();
                }
                _ => panic!("FST: upscope called after header finished"),
            },
        }
    }

    pub fn finish_header(&mut self) {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                v.writer.enddefinitions().unwrap();
            }
            WaveDumperKind::Fst(f) => {
                let old = std::mem::replace(&mut f.state, FstState::Transitioning);
                match old {
                    FstState::Header(h) => {
                        let body = h.finish().expect("failed to finish FST header");
                        f.state = FstState::Body(body);
                    }
                    _ => panic!("FST: finish_header called in wrong state"),
                }
            }
        }
    }

    pub fn begin_dumpvars(&mut self) {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                v.writer.begin(SimulationCommand::Dumpvars).unwrap();
            }
            WaveDumperKind::Fst(_) => {
                // no-op for FST
            }
        }
    }

    pub fn end_dumpvars(&mut self) {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                v.writer.end().unwrap();
            }
            WaveDumperKind::Fst(_) => {
                // no-op for FST
            }
        }
    }

    pub fn timestamp(&mut self, time: u64) {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                v.writer.timestamp(time).unwrap();
            }
            WaveDumperKind::Fst(f) => match &mut f.state {
                FstState::Body(b) => {
                    b.time_change(time).unwrap();
                }
                _ => panic!("FST: timestamp called before header finished"),
            },
        }
    }

    pub fn change_vector(&mut self, handle: VarHandle, value: &Value) {
        match &mut self.kind {
            WaveDumperKind::Vcd(v) => {
                let VarHandle::Vcd(code) = handle else {
                    panic!("VCD dumper received non-VCD handle");
                };
                v.writer.change_vector(code, value).unwrap();
            }
            WaveDumperKind::Fst(f) => {
                let VarHandle::Fst(id) = handle else {
                    panic!("FST dumper received non-FST handle");
                };
                match &mut f.state {
                    FstState::Body(b) => {
                        let bits = value.to_fst_bits();
                        b.signal_change(id, &bits).unwrap();
                    }
                    _ => panic!("FST: change_vector called before header finished"),
                }
            }
        }
    }

    pub fn setup_module(&mut self, module_vars: &ModuleVariables, dump_vars: &mut Vec<DumpVar>) {
        self.add_module(&sanitize_wave_name(&module_vars.name.to_string()));

        for x in module_vars.variables.values() {
            let name = sanitize_wave_name(&x.path.to_string());
            let width = x.width as u32;
            let handle = self.add_wire(width, &name);
            dump_vars.push(DumpVar {
                handle,
                ptr: x.current_values[0],
                native_bytes: x.native_bytes,
                width: x.width,
            });
        }

        for child in &module_vars.children {
            self.setup_module(child, dump_vars);
        }

        self.upscope();
    }

    pub fn dump_all_vars(&mut self, dump_vars: &[DumpVar], use_4state: bool) {
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
            value.trunc(entry.width);
            self.change_vector(entry.handle, &value);
        }
    }
}

impl Drop for FstDumper {
    fn drop(&mut self) {
        let old = std::mem::replace(&mut self.state, FstState::Transitioning);
        if let FstState::Body(body) = old {
            let _ = body.finish();
        }
    }
}

fn sanitize_wave_name(name: &str) -> String {
    name.replace("::<", "_").replace(">", "").replace("::", "_")
}

pub struct DumpVar {
    pub handle: VarHandle,
    pub ptr: *const u8,
    pub native_bytes: usize,
    pub width: usize,
}

// SAFETY: Same as Statement — see statement.rs.
unsafe impl Send for DumpVar {}
