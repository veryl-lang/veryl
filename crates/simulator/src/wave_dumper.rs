use crate::ir::{ModuleVariables, Value, read_native_value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
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
    /// Each region's storage bytes as last written: what the waveform carries,
    /// and for a region the write log does not describe, the compare state.
    shadow: Vec<u8>,
    regions: Vec<ScanRegion>,
    /// `DumpVar` indices, grouped by region and ordered by storage address.
    order: Vec<u32>,
    dirty: DirtyMap,
    /// The granules a step has to look at, refilled per region.
    marks: Vec<u32>,
    /// `DIRECT_WRITES` as of the last step.
    direct_writes: u64,
}

/// A stretch of storage holding the variables of one arena.  A step decides
/// it a granule at a time, so an untouched granule costs a bit or a word
/// compare rather than a lookup per variable.
struct ScanRegion {
    base: *const u8,
    len: usize,
    /// Start of the region's mirror in `shadow`.
    off: usize,
    /// `[first, first + count)` into `order`.
    first: u32,
    count: u32,
    /// The `order` range of the variables overlapping each granule.
    granule: Vec<(u32, u32)>,
    /// Whether the write log describes every move of this region.
    logged: bool,
    /// The region's first granule in the write log's map.
    arena_g0: usize,
}

// SAFETY: Same as DumpVar.
unsafe impl Send for ScanRegion {}

/// A bit per granule of the FF arena, set where a write changed bytes since
/// the last step.  A granule no bit covers cannot have moved, so the step
/// skips it without reading it; a bit that turns out to cover nothing new
/// costs one compare, which is what the step did anyway.
#[derive(Default)]
struct DirtyMap {
    base: usize,
    len: usize,
    bits: Vec<u64>,
}

/// Variables sit every 8 bytes in the arenas, so a bit per 8 bytes is what
/// keeps a marked granule down to the one variable that moved.
const GRANULE_LOG2: usize = 3;

impl DirtyMap {
    /// Point the map at the arena the write log addresses, everything marked:
    /// nothing is known about what moved before the first step.
    fn cover(&mut self, base: *const u8, len: usize) {
        self.base = base as usize;
        self.len = len;
        self.bits.clear();
        self.bits.resize((len >> GRANULE_LOG2) / 64 + 1, !0);
    }

    #[inline]
    fn mark(&mut self, off: usize, len: usize) {
        if len == 0 || off >= self.len {
            return;
        }
        let lo = off >> GRANULE_LOG2;
        let hi = ((off + len).min(self.len) - 1) >> GRANULE_LOG2;
        for g in lo..=hi {
            self.bits[g >> 6] |= 1u64 << (g & 63);
        }
    }

    fn mark_all(&mut self) {
        self.bits.fill(!0);
    }

    fn clear(&mut self) {
        self.bits.fill(0);
    }

    /// The marked granules of `[g0, g0 + n)`, numbered from `g0`.
    fn collect_marked(&self, g0: usize, n: usize, out: &mut Vec<u32>) {
        let end = (g0 + n).min(self.bits.len() * 64);
        if end <= g0 {
            return;
        }
        for wi in (g0 >> 6)..end.div_ceil(64) {
            let mut word = self.bits[wi];
            if wi == g0 >> 6 {
                word &= !0u64 << (g0 & 63);
            }
            if wi == (end - 1) >> 6 && end & 63 != 0 {
                word &= !0u64 >> (64 - (end & 63));
            }
            while word != 0 {
                out.push((wi * 64 + word.trailing_zeros() as usize - g0) as u32);
                word &= word - 1;
            }
        }
    }
}

/// Bumped by the paths that write variable storage without a log entry: a
/// memory image load, a component write-back.  They are rare and nothing else
/// could tell the gate their bytes moved, so a step that sees this move
/// compares everything again.
static DIRECT_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn note_direct_write() {
    DIRECT_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// `VERYL_WAVE_GATE=0` compares every granule as before; `VERYL_WAVE_GATE_CHECK=1`
/// fails on a variable that moved without its granule marked, which is what
/// proves the write log describes every move of the FF arena.
fn wave_gate() -> (bool, bool) {
    static MODE: OnceLock<(bool, bool)> = OnceLock::new();
    *MODE.get_or_init(|| {
        (
            std::env::var("VERYL_WAVE_GATE").as_deref() != Ok("0"),
            std::env::var("VERYL_WAVE_GATE_CHECK").as_deref() == Ok("1"),
        )
    })
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
            regions: Vec::new(),
            order: Vec::new(),
            dirty: DirtyMap::default(),
            marks: Vec::new(),
            direct_writes: 0,
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
            regions: Vec::new(),
            order: Vec::new(),
            dirty: DirtyMap::default(),
            marks: Vec::new(),
            direct_writes: 0,
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

    pub fn into_path(mut self) -> Option<PathBuf> {
        self.finish();
        self.path
    }

    /// Flush what is buffered.  The sink's `Drop` would do it too, but it
    /// discards the error, and a waveform truncated by a full disk has to be
    /// as loud as it was before the buffer went in.
    pub fn finish(&mut self) {
        if let WaveDumperKind::Vcd(v) = &mut self.kind {
            v.flush_line();
            v.writer
                .writer()
                .flush()
                .expect("failed to write the waveform");
        }
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

    /// Name the FF arena the write log addresses; the gate applies to the
    /// regions inside it and nowhere else.
    pub fn set_gate_arena(&mut self, arena: &[u8]) {
        self.dirty.cover(arena.as_ptr(), arena.len());
    }

    /// Mark a byte range of the FF arena the commit changed.
    #[inline]
    pub fn mark_ff(&mut self, off: usize, len: usize) {
        self.dirty.mark(off, len);
    }

    /// Mark a write that did not go through the log.
    pub fn mark_written(&mut self, ptr: *const u8, len: usize) {
        let p = ptr as usize;
        if p >= self.dirty.base {
            self.dirty.mark(p - self.dirty.base, len);
        }
    }

    /// Group the variables into the storage regions they live in and index
    /// each region's variables by granule.
    fn build_scan(&mut self, dump_vars: &[DumpVar], use_4state: bool) {
        // Wider than any padding between two variables and far narrower than
        // the distance between two arenas, so the regions come out as the
        // arenas themselves.
        const REGION_GAP: usize = 4096;

        let span = |e: &DumpVar| {
            if use_4state {
                e.native_bytes * 2
            } else {
                e.native_bytes
            }
        };
        self.order.clear();
        self.order.extend(0..dump_vars.len() as u32);
        self.order
            .sort_unstable_by_key(|&i| dump_vars[i as usize].ptr as usize);

        self.regions.clear();
        for (k, &i) in self.order.iter().enumerate() {
            let entry = &dump_vars[i as usize];
            let ptr = entry.ptr as usize;
            let end = ptr + span(entry);
            match self.regions.last_mut() {
                Some(r) if ptr <= r.base as usize + r.len + REGION_GAP => {
                    r.len = r.len.max(end - r.base as usize);
                    r.count += 1;
                }
                _ => self.regions.push(ScanRegion {
                    base: entry.ptr,
                    len: end - ptr,
                    off: 0,
                    first: k as u32,
                    count: 1,
                    granule: Vec::new(),
                    logged: false,
                    arena_g0: 0,
                }),
            }
        }

        let (arena, arena_len) = (self.dirty.base, self.dirty.len);
        let mut shadow_off = 0usize;
        for r in &mut self.regions {
            let base = r.base as usize;
            r.logged = arena_len != 0 && base >= arena && base + r.len <= arena + arena_len;
            if r.logged {
                // The map's granules are cut from the arena's start, so the
                // region has to begin on one of them for a mark to name the
                // same bytes here.
                let back = (base - arena) & ((1 << GRANULE_LOG2) - 1);
                r.base = unsafe { r.base.sub(back) };
                r.len += back;
                r.arena_g0 = (r.base as usize - arena) >> GRANULE_LOG2;
            }
            r.off = shadow_off;
            shadow_off += r.len;
            let base = r.base as usize;
            // A variable straddling a boundary belongs to both granules, which
            // is why the ranges overlap.
            let (mut lo, mut hi) = (r.first as usize, r.first as usize);
            let last = (r.first + r.count) as usize;
            for g in 0..r.len.div_ceil(1 << GRANULE_LOG2) {
                let start = base + (g << GRANULE_LOG2);
                let end = start + (1 << GRANULE_LOG2);
                while lo < last && {
                    let e = &dump_vars[self.order[lo] as usize];
                    e.ptr as usize + span(e) <= start
                } {
                    lo += 1;
                }
                hi = hi.max(lo);
                while hi < last && (dump_vars[self.order[hi] as usize].ptr as usize) < end {
                    hi += 1;
                }
                r.granule.push((lo as u32, hi as u32));
            }
        }

        let total = self.regions.last().map_or(0, |r| r.off + r.len);
        self.shadow.clear();
        self.shadow.resize(total, 0);
    }

    /// Write the variables whose storage moved since the last call.  `force`
    /// writes all of them, as the opening `$dumpvars` must.
    pub fn dump_all_vars(&mut self, dump_vars: &[DumpVar], use_4state: bool, force: bool) {
        let force = force || self.order.len() != dump_vars.len();
        if force {
            self.build_scan(dump_vars, use_4state);
        }
        let direct = DIRECT_WRITES.load(std::sync::atomic::Ordering::Relaxed);
        if direct != self.direct_writes {
            self.direct_writes = direct;
            self.dirty.mark_all();
        }
        let Self {
            kind,
            shadow,
            regions,
            order,
            dirty,
            marks,
            ..
        } = self;
        let (gate, check) = wave_gate();
        for r in regions.iter() {
            // SAFETY: the region spans the storage its variables point into,
            // built from their `ptr` and native size.
            let cur = unsafe { std::slice::from_raw_parts(r.base, r.len) };
            let old = &mut shadow[r.off..r.off + r.len];
            let members = r.first as usize..(r.first + r.count) as usize;
            if force {
                for &i in &order[members.clone()] {
                    emit_if_moved(kind, old, r.base, &dump_vars[i as usize], use_4state, true);
                }
                old.copy_from_slice(cur);
                continue;
            }
            let logged = gate && r.logged;
            marks.clear();
            if logged {
                dirty.collect_marked(r.arena_g0, r.granule.len(), marks);
                if check {
                    // Only the bytes a variable covers matter: the padding
                    // between them moves without the log saying so, and the
                    // waveform never reads it.
                    for &i in &order[members.clone()] {
                        let e = &dump_vars[i as usize];
                        let lo = e.ptr as usize - r.base as usize;
                        let hi = lo
                            + if use_4state {
                                e.native_bytes * 2
                            } else {
                                e.native_bytes
                            };
                        assert!(
                            cur[lo..hi] == old[lo..hi]
                                || ((lo >> GRANULE_LOG2)..=((hi - 1) >> GRANULE_LOG2))
                                    .any(|g| marks.contains(&(g as u32))),
                            "wave gate missed a write at {:#x}",
                            r.base as usize + lo,
                        );
                    }
                }
            } else {
                diff_granules(cur, old, marks);
            }
            // A variable straddling a boundary is in every granule it
            // touches, and the granules come in order, so a cursor over
            // `order` is what keeps it to one visit.
            let mut cursor = 0u32;
            for &g in marks.iter() {
                let (lo, hi) = r.granule[g as usize];
                for &i in &order[lo.max(cursor) as usize..hi as usize] {
                    emit_if_moved(kind, old, r.base, &dump_vars[i as usize], use_4state, false);
                }
                cursor = cursor.max(hi);
            }
            if logged {
                for &g in marks.iter() {
                    let lo = (g as usize) << GRANULE_LOG2;
                    let hi = (lo + (1 << GRANULE_LOG2)).min(r.len);
                    old[lo..hi].copy_from_slice(&cur[lo..hi]);
                }
            } else {
                old.copy_from_slice(cur);
            }
        }
        if let WaveDumperKind::Vcd(v) = &mut self.kind {
            v.flush_line();
        }
        self.dirty.clear();
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

/// Write one variable if its storage moved.  The mirror is refreshed by the
/// caller once the granule is walked, so variables that alias one another's
/// storage all see the values the waveform last carried.
fn emit_if_moved(
    kind: &mut WaveDumperKind,
    old: &[u8],
    base: *const u8,
    entry: &DumpVar,
    use_4state: bool,
    force: bool,
) {
    // 4-state keeps the x/z mask right behind the payload; both decide the
    // value, so both belong to the compare.
    let nb = entry.native_bytes;
    let span = if use_4state { nb * 2 } else { nb };
    let lo = entry.ptr as usize - base as usize;
    // SAFETY: `ptr` is the variable's storage, valid for its native span.
    let cur = unsafe { std::slice::from_raw_parts(entry.ptr, span) };
    if !force && held(&old[lo..lo + span], cur) {
        return;
    }
    match kind {
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
            let mut value =
                unsafe { read_native_value(entry.ptr, nb, use_4state, entry.width as u32, false) };
            value.trunc(entry.width);
            fst.change_vector(entry.handle, &value);
        }
    }
}

/// The granules of `cur` that differ from `old`, checked eight at a time so an
/// untouched stretch costs one vector compare.
fn diff_granules(cur: &[u8], old: &[u8], out: &mut Vec<u32>) {
    const CHUNK: usize = 8;
    let word = |b: &[u8], g: usize| {
        u64::from_ne_bytes(
            b[g << GRANULE_LOG2..(g + 1) << GRANULE_LOG2]
                .try_into()
                .unwrap(),
        )
    };
    let full = cur.len() >> GRANULE_LOG2;
    let mut g = 0;
    while g + CHUNK <= full {
        let mut acc = 0u64;
        for k in 0..CHUNK {
            acc |= word(cur, g + k) ^ word(old, g + k);
        }
        if acc != 0 {
            for k in g..g + CHUNK {
                if word(cur, k) != word(old, k) {
                    out.push(k as u32);
                }
            }
        }
        g += CHUNK;
    }
    for k in g..full {
        if word(cur, k) != word(old, k) {
            out.push(k as u32);
        }
    }
    let rest = full << GRANULE_LOG2;
    if rest < cur.len() && cur[rest..] != old[rest..] {
        out.push(full as u32);
    }
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
