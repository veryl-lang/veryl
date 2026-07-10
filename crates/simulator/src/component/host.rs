//! Host side of the user-defined component ABI: the context backing
//! `VrlCtx`, the `VrlHostApi` service table, and a safe wrapper around a
//! component instance's vtable.

use crate::component::loader::ComponentError;
use std::ffi::c_void;
use veryl_component_sys as sys;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PortDir {
    Input,
    Output,
}

impl PortDir {
    fn as_abi(self) -> u32 {
        match self {
            PortDir::Input => sys::VRL_DIR_INPUT,
            PortDir::Output => sys::VRL_DIR_OUTPUT,
        }
    }
}

/// Clock/reset role of an input connection, from the Veryl-side type of the
/// connected expression. `VRL_DIR_CLOCK`/`VRL_DIR_RESET` resolution matches
/// against it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PortRole {
    #[default]
    Data,
    Clock,
    Reset,
}

struct HostPort {
    name: String,
    dir: PortDir,
    role: PortRole,
    width: u32,
    /// Input: staged pre-edge value. Output: value staged by
    /// `write_output`, pushed to the write log by the caller after the hook.
    words: Vec<u64>,
    /// Four-state mask parallel to `words` (a set bit is X or Z). All-zero
    /// under a two-state simulation.
    mask_xz: Vec<u64>,
    dirty: bool,
}

/// A component-internal signal registered for waveform dumping.
pub struct HostTraceVar {
    pub name: String,
    pub width: u32,
    /// Current value, updated by `trace_write`. Allocated at registration
    /// and never resized afterwards.
    pub words: Vec<u64>,
}

/// Host-side value for parameters, method arguments and returns.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostValue {
    Bits { words: Vec<u64>, width: u32 },
    Str(String),
    Unit,
}

impl HostValue {
    pub fn bits_u64(value: u64, width: u32) -> Self {
        HostValue::Bits {
            words: vec![value],
            width,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            HostValue::Bits { words, .. } => Some(words.first().copied().unwrap_or(0)),
            _ => None,
        }
    }

    fn as_vrl(&self) -> sys::VrlValue {
        match self {
            HostValue::Bits { words, width } => sys::VrlValue {
                kind: sys::VRL_VALUE_BITS,
                width: *width,
                words: words.as_ptr(),
                nwords: words.len(),
                // Parameters, method arguments and returns are two-state.
                mask_xz: std::ptr::null(),
                str_: sys::VrlStr::from_str(""),
            },
            HostValue::Str(s) => sys::VrlValue {
                kind: sys::VRL_VALUE_STRING,
                width: 0,
                words: std::ptr::null(),
                nwords: 0,
                mask_xz: std::ptr::null(),
                str_: sys::VrlStr::from_str(s),
            },
            HostValue::Unit => sys::VrlValue::unit(),
        }
    }
}

fn words_for(width: u32) -> usize {
    (width as usize).div_ceil(64).max(1)
}

/// The concrete object behind the opaque `VrlCtx` pointer. One per
/// component instance; owns the port staging buffers and collects
/// fail/finish/log requests raised during hooks.
#[derive(Default)]
pub struct HostContext {
    ports: Vec<HostPort>,
    params: Vec<(String, HostValue)>,
    failures: Vec<String>,
    finish_requested: bool,
    logs: Vec<String>,
    pub cycle: u64,
    pub time: u64,
    pub seed: u64,
    pub fired_clock: u32,
    /// Whether the simulation is four-state, surfaced to the component via
    /// `is_4state` so it can gate X/Z checks.
    pub use_4state: bool,
    /// Instance label; when set, failure and log messages are prefixed
    /// with it and the current cycle.
    pub label: String,
    /// Ports resolved by the component via `port_index` during `create`.
    /// Connections it never asked for are load errors.
    touched: Vec<bool>,
    /// Role the component resolved the port with (`VRL_DIR_CLOCK`/`_RESET`).
    /// Only such ports fire `on_clock`/`on_reset`.
    resolved_role: Vec<Option<PortRole>>,
    /// Host-mediated files opened by the component. Handles are indices;
    /// closed slots stay occupied so handles are never reused within a
    /// test.
    files: Vec<Option<std::fs::File>>,
    /// Every path the component touched, for reproducibility reporting.
    pub touched_files: Vec<String>,
    /// Base directory for relative reads (the project root).
    pub read_base: Option<std::path::PathBuf>,
    /// Base directory for relative writes (a per-test output directory,
    /// keeping parallel tests from clobbering each other).
    pub write_base: Option<std::path::PathBuf>,
    /// Trace variables registered during `create`; the waveform header is
    /// finalized after every component is built, so later registration is
    /// rejected (`in_create` gates it).
    pub trace_vars: Vec<HostTraceVar>,
    in_create: bool,
}

impl HostContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_port(&mut self, name: &str, dir: PortDir, width: u32) -> u32 {
        self.add_port_role(name, dir, PortRole::Data, width)
    }

    pub fn add_port_role(&mut self, name: &str, dir: PortDir, role: PortRole, width: u32) -> u32 {
        self.ports.push(HostPort {
            name: name.to_string(),
            dir,
            role,
            width,
            words: vec![0; words_for(width)],
            mask_xz: vec![0; words_for(width)],
            dirty: false,
        });
        self.touched.push(false);
        self.resolved_role.push(None);
        (self.ports.len() - 1) as u32
    }

    pub fn port_touched(&self, idx: u32) -> bool {
        self.touched[idx as usize]
    }

    /// The clock/reset role the component resolved the port with, `None`
    /// for a plain `input`/`output` resolution.
    pub fn port_resolved_role(&self, idx: u32) -> Option<PortRole> {
        self.resolved_role[idx as usize]
    }

    /// Names and directions of the ports the component resolved during
    /// `create`, for the manifest self-consistency check.
    pub fn touched_port_names(&self) -> Vec<(String, PortDir)> {
        self.ports
            .iter()
            .zip(&self.touched)
            .filter(|(_, touched)| **touched)
            .map(|(p, _)| (p.name.clone(), p.dir))
            .collect()
    }

    pub fn output_dirty_idx(&self, idx: u32) -> bool {
        self.ports[idx as usize].dirty
    }

    fn decorate(&self, msg: &str) -> String {
        if self.label.is_empty() {
            msg.to_string()
        } else {
            format!("[{}] cycle {}: {}", self.label, self.cycle, msg)
        }
    }

    pub fn add_param(&mut self, name: &str, value: HostValue) {
        self.params.push((name.to_string(), value));
    }

    fn port_position(&self, name: &str, dir: PortDir) -> Option<u32> {
        self.ports
            .iter()
            .position(|p| p.dir == dir && p.name == name)
            .map(|i| i as u32)
    }

    pub fn set_input(&mut self, idx: u32, words: &[u64]) {
        let port = &mut self.ports[idx as usize];
        debug_assert_eq!(port.dir, PortDir::Input);
        let n = port.words.len();
        port.words.copy_from_slice(&words[..n]);
        port.mask_xz.iter_mut().for_each(|m| *m = 0);
    }

    pub fn set_input_masked(&mut self, idx: u32, words: &[u64], mask_xz: &[u64]) {
        let port = &mut self.ports[idx as usize];
        debug_assert_eq!(port.dir, PortDir::Input);
        let n = port.words.len();
        port.words.copy_from_slice(&words[..n]);
        port.mask_xz.copy_from_slice(&mask_xz[..n]);
    }

    pub fn set_input_u64(&mut self, name: &str, value: u64) {
        let idx = self
            .port_position(name, PortDir::Input)
            .expect("unknown input port");
        let port = &mut self.ports[idx as usize];
        port.words.fill(0);
        port.words[0] = value;
        port.mask_xz.fill(0);
    }

    pub fn output_words(&self, name: &str) -> &[u64] {
        let idx = self
            .port_position(name, PortDir::Output)
            .expect("unknown output port");
        &self.ports[idx as usize].words
    }

    pub fn output_u64(&self, name: &str) -> u64 {
        self.output_words(name)[0]
    }

    pub fn output_dirty(&self, name: &str) -> bool {
        let idx = self
            .port_position(name, PortDir::Output)
            .expect("unknown output port");
        self.ports[idx as usize].dirty
    }

    pub fn clear_output_dirty(&mut self) {
        for port in &mut self.ports {
            port.dirty = false;
        }
    }

    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    pub fn take_failures(&mut self) -> Vec<String> {
        std::mem::take(&mut self.failures)
    }

    pub fn failed(&self) -> bool {
        !self.failures.is_empty()
    }

    pub fn finish_requested(&self) -> bool {
        self.finish_requested
    }

    pub fn logs(&self) -> &[String] {
        &self.logs
    }

    pub fn take_logs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.logs)
    }

    fn as_ctx(&mut self) -> *mut sys::VrlCtx {
        self as *mut HostContext as *mut sys::VrlCtx
    }
}

// The `svc_*` methods below are the host services behind the ABI, shared by
// the native `VrlHostApi` adapters and the wasm import handlers so the two
// transports cannot drift.
impl HostContext {
    pub(crate) fn svc_port_index(&mut self, name: &str, dir: u32) -> i32 {
        let role = match dir {
            sys::VRL_DIR_CLOCK => Some(PortRole::Clock),
            sys::VRL_DIR_RESET => Some(PortRole::Reset),
            _ => None,
        };
        let dir = match dir {
            sys::VRL_DIR_CLOCK | sys::VRL_DIR_RESET => PortDir::Input.as_abi(),
            other => other,
        };
        match self.ports.iter().position(|p| {
            p.dir.as_abi() == dir && p.name == name && role.is_none_or(|role| p.role == role)
        }) {
            Some(i) => {
                self.touched[i] = true;
                if role.is_some() {
                    self.resolved_role[i] = role;
                }
                i as i32
            }
            None => -1,
        }
    }

    pub(crate) fn svc_port_width(&self, idx: u32) -> u32 {
        self.ports.get(idx as usize).map(|p| p.width).unwrap_or(0)
    }

    /// Word count of a port's staging buffer (the transfer size of
    /// `read_input`/`write_output`).
    pub(crate) fn svc_port_words_len(&self, idx: u32) -> Option<usize> {
        self.ports.get(idx as usize).map(|p| p.words.len())
    }

    pub(crate) fn svc_input_words(&self, idx: u32) -> Option<&[u64]> {
        self.ports
            .get(idx as usize)
            .filter(|p| p.dir == PortDir::Input)
            .map(|p| p.words.as_slice())
    }

    pub(crate) fn svc_input_mask_xz(&self, idx: u32) -> Option<&[u64]> {
        self.ports
            .get(idx as usize)
            .filter(|p| p.dir == PortDir::Input)
            .map(|p| p.mask_xz.as_slice())
    }

    pub(crate) fn svc_write_output(&mut self, idx: u32, words: &[u64], mask_xz: Option<&[u64]>) {
        let Some(port) = self.ports.get_mut(idx as usize) else {
            return;
        };
        if port.dir != PortDir::Output {
            return;
        }
        port.words.copy_from_slice(words);
        match mask_xz {
            Some(mask_xz) => port.mask_xz.copy_from_slice(mask_xz),
            None => port.mask_xz.iter_mut().for_each(|m| *m = 0),
        }
        port.dirty = true;
    }

    pub(crate) fn svc_is_4state(&self) -> bool {
        self.use_4state
    }

    /// Direct pointers into a port's staging buffers. The buffers are sized
    /// once in `add_port_role` and only ever copied into afterwards, so the
    /// pointers stay valid for the instance's lifetime. Outputs also expose
    /// their dirty flag, which a direct writer sets in place of the
    /// `svc_write_output` path.
    pub(crate) fn svc_port_direct(&mut self, idx: u32) -> Option<sys::VrlPortDirect> {
        let port = self.ports.get_mut(idx as usize)?;
        let dirty = if port.dir == PortDir::Output {
            &mut port.dirty as *mut bool as *mut u8
        } else {
            std::ptr::null_mut()
        };
        Some(sys::VrlPortDirect {
            words: port.words.as_mut_ptr(),
            mask_xz: port.mask_xz.as_mut_ptr(),
            dirty,
        })
    }

    pub(crate) fn svc_param(&self, name: &str) -> Option<&HostValue> {
        self.params.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    pub(crate) fn svc_fail(&mut self, msg: &str) {
        let msg = self.decorate(msg);
        self.failures.push(msg);
    }

    pub(crate) fn svc_finish(&mut self) {
        self.finish_requested = true;
    }

    pub(crate) fn svc_log(&mut self, msg: &str) {
        let msg = self.decorate(msg);
        self.logs.push(msg);
    }

    /// Opens a file on behalf of the component. Relative paths resolve
    /// against the read/write bases, but this is not a path jail: absolute
    /// paths and `..` escape them. `requires(file)` declares intent, not a
    /// sandbox boundary.
    pub(crate) fn svc_file_open(&mut self, raw: &str, mode: u32) -> i32 {
        let raw_path = std::path::Path::new(raw);
        let path = if raw_path.is_relative() {
            if mode == sys::VRL_FILE_READ {
                // Reads resolve against the project root; files the component
                // wrote earlier in the test are found via the write base.
                match (&self.read_base, &self.write_base) {
                    (Some(read), Some(write)) if !read.join(raw_path).exists() => {
                        write.join(raw_path)
                    }
                    (Some(read), _) => read.join(raw_path),
                    _ => raw_path.to_path_buf(),
                }
            } else {
                match &self.write_base {
                    Some(write) => write.join(raw_path),
                    None => raw_path.to_path_buf(),
                }
            }
        } else {
            raw_path.to_path_buf()
        };
        if (mode == sys::VRL_FILE_CREATE || mode == sys::VRL_FILE_APPEND)
            && let Some(parent) = path.parent()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = match mode {
            sys::VRL_FILE_READ => std::fs::File::open(&path),
            sys::VRL_FILE_CREATE => std::fs::File::create(&path),
            sys::VRL_FILE_APPEND => std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path),
            _ => return -1,
        };
        match file {
            Ok(file) => {
                self.touched_files.push(path.display().to_string());
                self.files.push(Some(file));
                (self.files.len() - 1) as i32
            }
            Err(_) => -1,
        }
    }

    fn svc_file(&mut self, handle: i32) -> Option<&mut std::fs::File> {
        self.files.get_mut(handle as usize)?.as_mut()
    }

    pub(crate) fn svc_file_read(&mut self, handle: i32, buf: &mut [u8]) -> i64 {
        use std::io::Read;
        let Some(file) = self.svc_file(handle) else {
            return -1;
        };
        match file.read(buf) {
            Ok(n) => n as i64,
            Err(_) => -1,
        }
    }

    pub(crate) fn svc_file_write(&mut self, handle: i32, buf: &[u8]) -> i64 {
        use std::io::Write;
        let Some(file) = self.svc_file(handle) else {
            return -1;
        };
        match file.write(buf) {
            Ok(n) => n as i64,
            Err(_) => -1,
        }
    }

    pub(crate) fn svc_file_seek(&mut self, handle: i32, pos: i64, whence: u32) -> i64 {
        use std::io::{Seek, SeekFrom};
        let Some(file) = self.svc_file(handle) else {
            return -1;
        };
        let from = match whence {
            sys::VRL_SEEK_SET => SeekFrom::Start(pos as u64),
            sys::VRL_SEEK_CUR => SeekFrom::Current(pos),
            sys::VRL_SEEK_END => SeekFrom::End(pos),
            _ => return -1,
        };
        match file.seek(from) {
            Ok(n) => n as i64,
            Err(_) => -1,
        }
    }

    pub(crate) fn svc_file_close(&mut self, handle: i32) {
        if let Some(slot) = self.files.get_mut(handle as usize) {
            *slot = None;
        }
    }

    pub(crate) fn svc_trace_var(&mut self, name: &str, width: u32) -> i32 {
        if !self.in_create || width == 0 || self.trace_vars.iter().any(|t| t.name == name) {
            return -1;
        }
        self.trace_vars.push(HostTraceVar {
            name: name.to_string(),
            width,
            words: vec![0; words_for(width)],
        });
        (self.trace_vars.len() - 1) as i32
    }

    pub(crate) fn svc_trace_words_len(&self, handle: i32) -> Option<usize> {
        self.trace_vars.get(handle as usize).map(|v| v.words.len())
    }

    pub(crate) fn svc_trace_write(&mut self, handle: i32, words: &[u64]) {
        let Some(var) = self.trace_vars.get_mut(handle as usize) else {
            return;
        };
        var.words.copy_from_slice(words);
    }
}

/// # Safety
/// `ctx` must be the pointer produced by `HostContext::as_ctx` for a context
/// that is mutably borrowed by the current vtable call and not otherwise
/// accessed for its duration.
unsafe fn host<'a>(ctx: *mut sys::VrlCtx) -> &'a mut HostContext {
    unsafe { &mut *(ctx as *mut HostContext) }
}

extern "C" fn host_port_index(ctx: *mut sys::VrlCtx, name: sys::VrlStr, dir: u32) -> i32 {
    let host = unsafe { host(ctx) };
    let name = unsafe { name.as_str() };
    host.svc_port_index(name, dir)
}

extern "C" fn host_port_width(ctx: *mut sys::VrlCtx, idx: u32) -> u32 {
    unsafe { host(ctx) }.svc_port_width(idx)
}

extern "C" fn host_read_input(ctx: *mut sys::VrlCtx, idx: u32, words: *mut u64, mask_xz: *mut u64) {
    let host = unsafe { host(ctx) };
    let Some(input) = host.svc_input_words(idx) else {
        return;
    };
    let n = input.len();
    unsafe {
        std::ptr::copy_nonoverlapping(input.as_ptr(), words, n);
    }
    if !mask_xz.is_null()
        && let Some(input_mask) = host.svc_input_mask_xz(idx)
    {
        unsafe {
            std::ptr::copy_nonoverlapping(input_mask.as_ptr(), mask_xz, input_mask.len());
        }
    }
}

extern "C" fn host_write_output(
    ctx: *mut sys::VrlCtx,
    idx: u32,
    words: *const u64,
    mask_xz: *const u64,
) {
    let host = unsafe { host(ctx) };
    let Some(n) = host.svc_port_words_len(idx) else {
        return;
    };
    let words = unsafe { std::slice::from_raw_parts(words, n) };
    let mask_xz = if mask_xz.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(mask_xz, n) })
    };
    host.svc_write_output(idx, words, mask_xz);
}

extern "C" fn host_is_4state(ctx: *mut sys::VrlCtx) -> u32 {
    u32::from(unsafe { host(ctx) }.svc_is_4state())
}

extern "C" fn host_port_direct(
    ctx: *mut sys::VrlCtx,
    idx: u32,
    out: *mut sys::VrlPortDirect,
) -> u32 {
    let host = unsafe { host(ctx) };
    let Some(direct) = host.svc_port_direct(idx) else {
        return 0;
    };
    unsafe { *out = direct };
    1
}

extern "C" fn host_param_get(
    ctx: *mut sys::VrlCtx,
    name: sys::VrlStr,
    out: *mut sys::VrlValue,
) -> i32 {
    let host = unsafe { host(ctx) };
    let name = unsafe { name.as_str() };
    match host.svc_param(name) {
        Some(value) => {
            unsafe { *out = value.as_vrl() };
            0
        }
        None => -1,
    }
}

extern "C" fn host_fail(ctx: *mut sys::VrlCtx, msg: sys::VrlStr) {
    let host = unsafe { host(ctx) };
    let msg = unsafe { msg.as_str() };
    host.svc_fail(msg);
}

extern "C" fn host_finish(ctx: *mut sys::VrlCtx) {
    unsafe { host(ctx) }.svc_finish();
}

extern "C" fn host_log(ctx: *mut sys::VrlCtx, msg: sys::VrlStr) {
    let host = unsafe { host(ctx) };
    let msg = unsafe { msg.as_str() };
    host.svc_log(msg);
}

extern "C" fn host_cycle(ctx: *mut sys::VrlCtx) -> u64 {
    unsafe { host(ctx) }.cycle
}

extern "C" fn host_sim_time(ctx: *mut sys::VrlCtx) -> u64 {
    unsafe { host(ctx) }.time
}

extern "C" fn host_seed(ctx: *mut sys::VrlCtx) -> u64 {
    unsafe { host(ctx) }.seed
}

extern "C" fn host_fired_clock(ctx: *mut sys::VrlCtx) -> u32 {
    unsafe { host(ctx) }.fired_clock
}

extern "C" fn host_file_open(ctx: *mut sys::VrlCtx, path: sys::VrlStr, mode: u32) -> i32 {
    let host = unsafe { host(ctx) };
    let raw = unsafe { path.as_str() };
    host.svc_file_open(raw, mode)
}

extern "C" fn host_file_read(ctx: *mut sys::VrlCtx, handle: i32, buf: *mut u8, len: usize) -> i64 {
    let host = unsafe { host(ctx) };
    let buf = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    host.svc_file_read(handle, buf)
}

extern "C" fn host_file_write(
    ctx: *mut sys::VrlCtx,
    handle: i32,
    buf: *const u8,
    len: usize,
) -> i64 {
    let host = unsafe { host(ctx) };
    let buf = unsafe { std::slice::from_raw_parts(buf, len) };
    host.svc_file_write(handle, buf)
}

extern "C" fn host_file_seek(ctx: *mut sys::VrlCtx, handle: i32, pos: i64, whence: u32) -> i64 {
    unsafe { host(ctx) }.svc_file_seek(handle, pos, whence)
}

extern "C" fn host_file_close(ctx: *mut sys::VrlCtx, handle: i32) {
    unsafe { host(ctx) }.svc_file_close(handle);
}

extern "C" fn host_trace_var(ctx: *mut sys::VrlCtx, name: sys::VrlStr, width: u32) -> i32 {
    let host = unsafe { host(ctx) };
    let name = unsafe { name.as_str() };
    host.svc_trace_var(name, width)
}

extern "C" fn host_trace_write(ctx: *mut sys::VrlCtx, handle: i32, words: *const u64) {
    let host = unsafe { host(ctx) };
    let Some(n) = host.svc_trace_words_len(handle) else {
        return;
    };
    let words = unsafe { std::slice::from_raw_parts(words, n) };
    host.svc_trace_write(handle, words);
}

static HOST_API: sys::VrlHostApi = sys::VrlHostApi {
    size: size_of::<sys::VrlHostApi>(),
    port_index: host_port_index,
    port_width: host_port_width,
    read_input: host_read_input,
    write_output: host_write_output,
    param_get: host_param_get,
    fail: host_fail,
    finish: host_finish,
    log: host_log,
    cycle: host_cycle,
    sim_time: host_sim_time,
    seed: host_seed,
    fired_clock: host_fired_clock,
    file_open: host_file_open,
    file_read: host_file_read,
    file_write: host_file_write,
    file_seek: host_file_seek,
    file_close: host_file_close,
    trace_var: host_trace_var,
    trace_write: host_trace_write,
    is_4state: host_is_4state,
    port_direct: host_port_direct,
};

/// Capacity of the method return buffer; larger than the 64-bit values the
/// testbench assignment form supports.
pub(crate) const METHOD_RET_WORDS: usize = 8;

/// A live component instance behind either transport. All calls are
/// serialized through `&mut self` plus the exclusive `HostContext` borrow.
pub struct ExternalInstance {
    inner: InstanceInner,
}

enum InstanceInner {
    Native {
        state: *mut c_void,
        vtable: &'static sys::VrlComponentVTable,
    },
    #[cfg(not(target_family = "wasm"))]
    Wasm(Box<crate::component::wasm::WasmInstance>),
}

// The ABI contract requires the component state to be `Send` (hooks may run
// from any thread as long as they are serialized, which `&mut` enforces).
unsafe impl Send for ExternalInstance {}

impl ExternalInstance {
    /// Port and parameter resolution happens inside the component's `new`;
    /// failures surface as `CreateFailed` with the messages the component
    /// reported.
    pub fn create(
        backend: impl Into<crate::component::loader::ComponentBackend>,
        host: &mut HostContext,
    ) -> Result<Self, ComponentError> {
        match backend.into() {
            crate::component::loader::ComponentBackend::Native(vtable) => {
                // Safety: `host.as_ctx()` is the live context for this call
                // and HOST_API outlives every instance.
                host.in_create = true;
                let state = unsafe { (vtable.create)(host.as_ctx(), &HOST_API) };
                host.in_create = false;
                if state.is_null() {
                    return Err(ComponentError::CreateFailed {
                        messages: host.take_failures().join("; "),
                    });
                }
                Ok(Self {
                    inner: InstanceInner::Native { state, vtable },
                })
            }
            #[cfg(not(target_family = "wasm"))]
            crate::component::loader::ComponentBackend::Wasm {
                library,
                type_name,
                kind,
                file_allowed,
            } => {
                host.in_create = true;
                let instance = crate::component::wasm::WasmInstance::create(
                    &library,
                    &type_name,
                    kind,
                    file_allowed,
                    host,
                );
                host.in_create = false;
                Ok(Self {
                    inner: InstanceInner::Wasm(Box::new(instance?)),
                })
            }
        }
    }

    pub fn kind(&self) -> u32 {
        match &self.inner {
            InstanceInner::Native { vtable, .. } => vtable.kind,
            #[cfg(not(target_family = "wasm"))]
            InstanceInner::Wasm(instance) => instance.kind(),
        }
    }

    pub fn on_init(&mut self, host: &mut HostContext) -> i32 {
        match &mut self.inner {
            InstanceInner::Native { state, vtable } => unsafe {
                (vtable.on_init)(*state, host.as_ctx())
            },
            #[cfg(not(target_family = "wasm"))]
            InstanceInner::Wasm(instance) => instance.on_init(host),
        }
    }

    pub fn on_reset(&mut self, host: &mut HostContext) -> i32 {
        match &mut self.inner {
            InstanceInner::Native { state, vtable } => unsafe {
                (vtable.on_reset)(*state, host.as_ctx())
            },
            #[cfg(not(target_family = "wasm"))]
            InstanceInner::Wasm(instance) => instance.on_reset(host),
        }
    }

    pub fn on_clock(&mut self, host: &mut HostContext) -> i32 {
        match &mut self.inner {
            InstanceInner::Native { state, vtable } => unsafe {
                (vtable.on_clock)(*state, host.as_ctx())
            },
            #[cfg(not(target_family = "wasm"))]
            InstanceInner::Wasm(instance) => instance.on_clock(host),
        }
    }

    pub fn on_finish(&mut self, host: &mut HostContext) -> i32 {
        match &mut self.inner {
            InstanceInner::Native { state, vtable } => unsafe {
                (vtable.on_finish)(*state, host.as_ctx())
            },
            #[cfg(not(target_family = "wasm"))]
            InstanceInner::Wasm(instance) => instance.on_finish(host),
        }
    }

    /// Zero-time method call. `None` means the component reported an error,
    /// already recorded in the context's failures.
    pub fn call_method(
        &mut self,
        host: &mut HostContext,
        name: &str,
        args: &[HostValue],
    ) -> Option<HostValue> {
        match &mut self.inner {
            InstanceInner::Native { state, vtable } => {
                let args: Vec<sys::VrlValue> = args.iter().map(|a| a.as_vrl()).collect();
                // The guest writes the return payload through `ret.words`,
                // so the buffer must be borrowed mutably here.
                let mut ret_words = [0u64; METHOD_RET_WORDS];
                let mut ret = sys::VrlValue {
                    kind: sys::VRL_VALUE_UNIT,
                    width: 0,
                    words: ret_words.as_mut_ptr(),
                    nwords: ret_words.len(),
                    // Method returns are two-state; no mask buffer.
                    mask_xz: std::ptr::null(),
                    str_: sys::VrlStr::from_str(""),
                };
                let rc = unsafe {
                    (vtable.call_method)(
                        *state,
                        host.as_ctx(),
                        sys::VrlStr::from_str(name),
                        args.as_ptr(),
                        args.len(),
                        &mut ret,
                    )
                };
                if rc != 0 {
                    return None;
                }
                Some(match ret.kind {
                    sys::VRL_VALUE_BITS => {
                        let nwords = ret.nwords.min(METHOD_RET_WORDS);
                        HostValue::Bits {
                            words: ret_words[..nwords].to_vec(),
                            width: ret.width,
                        }
                    }
                    _ => HostValue::Unit,
                })
            }
            #[cfg(not(target_family = "wasm"))]
            InstanceInner::Wasm(instance) => instance.call_method(host, name, args),
        }
    }
}

impl Drop for ExternalInstance {
    fn drop(&mut self) {
        match &mut self.inner {
            InstanceInner::Native { state, vtable } => unsafe { (vtable.destroy)(*state) },
            // The wasm instance's own Drop calls the guest destructor.
            #[cfg(not(target_family = "wasm"))]
            InstanceInner::Wasm(_) => {}
        }
    }
}

impl std::fmt::Debug for ExternalInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let transport = match &self.inner {
            InstanceInner::Native { .. } => "native",
            #[cfg(not(target_family = "wasm"))]
            InstanceInner::Wasm(_) => "wasm",
        };
        f.debug_struct("ExternalInstance")
            .field("transport", &transport)
            .field("kind", &self.kind())
            .finish()
    }
}
