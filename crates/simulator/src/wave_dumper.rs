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
    Vcd(VcdId),
    Fst(fst_writer::FstSignalId),
}

/// A VCD identifier together with the bytes it is written as, so a value line
/// is assembled without formatting the `IdCode` again at every change.
#[derive(Clone, Copy)]
pub struct VcdId {
    code: vcd::IdCode,
    bytes: [u8; 8],
    len: u8,
}

impl VcdId {
    fn new(code: vcd::IdCode) -> Self {
        let text = code.to_string();
        let src = text.as_bytes();
        let mut bytes = [0u8; 8];
        bytes[..src.len()].copy_from_slice(src);
        VcdId {
            code,
            bytes,
            len: src.len() as u8,
        }
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

pub struct WaveDumper {
    kind: WaveDumperKind,
    path: Option<PathBuf>,
    /// The storage bytes behind each `DumpVar` as last written, so a step
    /// reads and formats only what moved.  Comparing the storage rather than
    /// the value keeps the unchanged case free of a `Value` per variable.
    shadow: Vec<u8>,
    /// Start of each `DumpVar`'s bytes in `shadow`, with its end last.
    shadow_at: Vec<usize>,
}

enum WaveDumperKind {
    Vcd(VcdDumper),
    Fst(Box<FstDumper>),
}

impl WaveDumperKind {
    fn change_vector(&mut self, handle: VarHandle, value: &Value) {
        match self {
            WaveDumperKind::Vcd(v) => {
                let VarHandle::Vcd(id) = handle else {
                    panic!("VCD dumper received non-VCD handle");
                };
                v.flush_line();
                v.writer.change_vector(id.code, value).unwrap();
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
}

struct VcdDumper {
    writer: vcd::Writer<Box<dyn Write + Send>>,
    /// Value lines are assembled here and handed to the sink in blocks, which
    /// keeps the per-variable work free of dynamic dispatch.
    line: Vec<u8>,
}

impl VcdDumper {
    fn flush_line(&mut self) {
        if !self.line.is_empty() {
            self.writer.writer().write_all(&self.line).unwrap();
            self.line.clear();
        }
    }
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
                line: Vec::with_capacity(LINE_BUF_CAPACITY),
            }),
            path: None,
            shadow: Vec::new(),
            shadow_at: Vec::new(),
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
            shadow: Vec::new(),
            shadow_at: Vec::new(),
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
                VarHandle::Vcd(VcdId::new(code))
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
        self.kind.change_vector(handle, value);
    }

    pub fn setup_module(&mut self, module_vars: &ModuleVariables, dump_vars: &mut Vec<DumpVar>) {
        self.add_module(&sanitize_wave_name(&module_vars.name.to_string()));

        for x in module_vars.variables.values() {
            let name = sanitize_wave_name(&x.path.to_string());
            let width = x.width as u32;
            // One pointer per array element (scalar = one). Dump a `name[i]`
            // wire per element, not just element [0].
            let num_elements = x.current_values.len();
            for (i, &ptr) in x.current_values.iter().enumerate() {
                let elem_name = if num_elements > 1 {
                    format!("{name}[{i}]")
                } else {
                    name.clone()
                };
                let handle = self.add_wire(width, &elem_name);
                dump_vars.push(DumpVar {
                    handle,
                    ptr,
                    native_bytes: x.native_bytes,
                    width: x.width,
                });
            }
        }

        for child in &module_vars.children {
            self.setup_module(child, dump_vars);
        }

        self.upscope();
    }

    /// Write the variables whose storage moved since the last call.  `force`
    /// writes all of them, as the opening `$dumpvars` must.
    pub fn dump_all_vars(&mut self, dump_vars: &[DumpVar], use_4state: bool, force: bool) {
        // 4-state keeps the x/z mask right behind the payload; both decide the
        // value, so both belong to the compare.
        let span = |nb: usize| if use_4state { nb * 2 } else { nb };
        let force = force || self.shadow_at.len() != dump_vars.len() + 1;
        if force {
            self.shadow_at.clear();
            let mut at = 0usize;
            for entry in dump_vars {
                self.shadow_at.push(at);
                at += span(entry.native_bytes);
            }
            self.shadow_at.push(at);
            self.shadow.clear();
            self.shadow.resize(at, 0);
        }
        let Self {
            kind,
            shadow,
            shadow_at,
            ..
        } = self;
        for (i, entry) in dump_vars.iter().enumerate() {
            let (lo, hi) = (shadow_at[i], shadow_at[i + 1]);
            // SAFETY: `ptr` is the variable's storage, valid for `span` bytes
            // (`read_native_value` reads the same range).
            let cur = unsafe { std::slice::from_raw_parts(entry.ptr, hi - lo) };
            if !force && held(&shadow[lo..hi], cur) {
                continue;
            }
            shadow[lo..hi].copy_from_slice(cur);
            let nb = entry.native_bytes;
            match &mut *kind {
                WaveDumperKind::Vcd(v) => {
                    let VarHandle::Vcd(id) = &entry.handle else {
                        panic!("VCD dumper received non-VCD handle");
                    };
                    let (payload, mask_xz) = if use_4state {
                        let (p, m) = cur.split_at(nb);
                        (p, Some(m))
                    } else {
                        (cur, None)
                    };
                    push_change(&mut v.line, payload, mask_xz, entry.width, id);
                    if v.line.len() >= LINE_BUF_CAPACITY {
                        v.writer.writer().write_all(&v.line).unwrap();
                        v.line.clear();
                    }
                }
                fst @ WaveDumperKind::Fst(_) => {
                    let mut value = unsafe {
                        read_native_value(entry.ptr, nb, use_4state, entry.width as u32, false)
                    };
                    value.trunc(entry.width);
                    fst.change_vector(entry.handle, &value);
                }
            }
        }
        if let WaveDumperKind::Vcd(v) = &mut self.kind {
            v.flush_line();
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

/// Most variables are a machine word or less, and this runs for every one of
/// them at every step, so the common widths take a typed load instead of the
/// call a slice compare comes down to.
#[inline]
fn held(a: &[u8], b: &[u8]) -> bool {
    match a.len() {
        1 => a[0] == b[0],
        2 => u16::from_ne_bytes([a[0], a[1]]) == u16::from_ne_bytes([b[0], b[1]]),
        4 => {
            u32::from_ne_bytes(a[..4].try_into().unwrap())
                == u32::from_ne_bytes(b[..4].try_into().unwrap())
        }
        8 => {
            u64::from_ne_bytes(a[..8].try_into().unwrap())
                == u64::from_ne_bytes(b[..8].try_into().unwrap())
        }
        _ => a == b,
    }
}

/// The eight ASCII bit characters of a byte, MSB first, packed so a whole
/// byte of a 2-state value becomes one store.
const BIT_CHARS: [u64; 256] = bit_chars();

const fn bit_chars() -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut b = 0usize;
    while b < 256 {
        let mut packed = 0u64;
        let mut i = 0;
        while i < 8 {
            let bit = ((b >> (7 - i)) & 1) as u64;
            packed |= (b'0' as u64 + bit) << (8 * i);
            i += 1;
        }
        table[b] = packed;
        b += 1;
    }
    table
}

/// Size at which the assembled value lines are handed to the sink.
const LINE_BUF_CAPACITY: usize = 1 << 15;

#[inline]
fn bit_char(payload: u8, mask_xz: u8, k: usize) -> u8 {
    let p = (payload >> k) & 1;
    if (mask_xz >> k) & 1 == 1 {
        if p == 1 { b'z' } else { b'x' }
    } else {
        b'0' + p
    }
}

/// Append one value change, written from the variable's storage rather than
/// through a `Value` and its `Display`.
fn push_change(
    out: &mut Vec<u8>,
    payload: &[u8],
    mask_xz: Option<&[u8]>,
    width: usize,
    id: &VcdId,
) {
    if width == 1 {
        out.push(bit_char(payload[0], mask_xz.map_or(0, |m| m[0]), 0));
    } else {
        out.push(b'b');
        let full = width / 8;
        for k in (0..width % 8).rev() {
            out.push(bit_char(payload[full], mask_xz.map_or(0, |m| m[full]), k));
        }
        for i in (0..full).rev() {
            match mask_xz {
                Some(m) if m[i] != 0 => {
                    for k in (0..8).rev() {
                        out.push(bit_char(payload[i], m[i], k));
                    }
                }
                _ => out.extend_from_slice(&BIT_CHARS[payload[i] as usize].to_le_bytes()),
            }
        }
        out.push(b' ');
    }
    out.extend_from_slice(id.as_bytes());
    out.push(b'\n');
}

pub struct DumpVar {
    pub handle: VarHandle,
    pub ptr: *const u8,
    pub native_bytes: usize,
    pub width: usize,
}

// SAFETY: Same as Statement — see statement.rs.
unsafe impl Send for DumpVar {}
