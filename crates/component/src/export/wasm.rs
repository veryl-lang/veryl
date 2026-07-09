//! Guest-side glue for the wasm transport.
//!
//! Host services are imported from the `"veryl"` wasm module and adapted to
//! a static [`sys::VrlHostApi`] table, so contexts and the native ABI
//! wrappers run unchanged on wasm. The entry points here are instantiated
//! as `veryl_component_*` wasm exports by `veryl_component_export!`,
//! mirroring the native vtable contract with a `u32` instance handle.

use crate::sys;
use std::alloc::Layout;
use std::cell::RefCell;
use std::ffi::c_void;

/// Raw host imports. Pointers are guest linear-memory offsets and strings
/// are `(ptr, len)` UTF-8, so every signature is scalar-only.
///
/// `param_get` writes the value payload (LSB-first 64-bit words or UTF-8
/// bytes) into the guest buffer `buf` and points `out.words` / `out.str_`
/// at it. It returns -1 if the parameter is absent, otherwise the required
/// payload size in bytes; the call succeeded iff that fits `buf_cap` (the
/// host must not write beyond `buf_cap`).
mod host {
    use crate::sys;

    #[link(wasm_import_module = "veryl")]
    unsafe extern "C" {
        pub fn port_index(name_ptr: *const u8, name_len: usize, dir: u32) -> i32;
        pub fn port_width(idx: u32) -> u32;
        pub fn read_input(idx: u32, words: *mut u64, mask_xz: *mut u64);
        pub fn write_output(idx: u32, words: *const u64, mask_xz: *const u64);
        pub fn param_get(
            name_ptr: *const u8,
            name_len: usize,
            out: *mut sys::VrlValue,
            buf: *mut u8,
            buf_cap: usize,
        ) -> i64;
        pub fn fail(msg_ptr: *const u8, msg_len: usize);
        pub fn finish();
        pub fn log(msg_ptr: *const u8, msg_len: usize);
        pub fn cycle() -> u64;
        pub fn sim_time() -> u64;
        pub fn seed() -> u64;
        pub fn is_4state() -> u32;
        pub fn fired_clock() -> u32;
        pub fn file_open(path_ptr: *const u8, path_len: usize, mode: u32) -> i32;
        pub fn file_read(handle: i32, buf: *mut u8, len: usize) -> i64;
        pub fn file_write(handle: i32, buf: *const u8, len: usize) -> i64;
        pub fn file_seek(handle: i32, pos: i64, whence: u32) -> i64;
        pub fn file_close(handle: i32);
        pub fn trace_var(name_ptr: *const u8, name_len: usize, width: u32) -> i32;
        pub fn trace_write(handle: i32, words: *const u64);
    }
}

// Shims adapting the imports to the `VrlHostApi` signatures. The `VrlCtx`
// pointer is unused: on wasm the host identifies the instance by the store
// the call comes from, so entry points pass NULL.

unsafe extern "C" fn port_index(_: *mut sys::VrlCtx, name: sys::VrlStr, dir: u32) -> i32 {
    unsafe { host::port_index(name.ptr, name.len, dir) }
}

unsafe extern "C" fn port_width(_: *mut sys::VrlCtx, idx: u32) -> u32 {
    unsafe { host::port_width(idx) }
}

unsafe extern "C" fn read_input(_: *mut sys::VrlCtx, idx: u32, words: *mut u64, mask_xz: *mut u64) {
    unsafe { host::read_input(idx, words, mask_xz) }
}

unsafe extern "C" fn write_output(
    _: *mut sys::VrlCtx,
    idx: u32,
    words: *const u64,
    mask_xz: *const u64,
) {
    unsafe { host::write_output(idx, words, mask_xz) }
}

thread_local! {
    /// Payload buffer for `param_get`, `u64` so word payloads stay aligned.
    /// The returned `VrlValue` points into it, which stays valid until the
    /// next `param_get` — long enough for the immediate copy the context
    /// layer performs.
    static PARAM_SCRATCH: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn param_get(
    _: *mut sys::VrlCtx,
    name: sys::VrlStr,
    out: *mut sys::VrlValue,
) -> i32 {
    PARAM_SCRATCH.with(|scratch| {
        let mut buf = scratch.borrow_mut();
        loop {
            let cap = buf.len() * 8;
            let required = unsafe {
                host::param_get(name.ptr, name.len, out, buf.as_mut_ptr() as *mut u8, cap)
            };
            if required < 0 {
                return -1;
            }
            if required as usize <= cap {
                return 0;
            }
            buf.resize((required as usize).div_ceil(8), 0);
        }
    })
}

unsafe extern "C" fn fail(_: *mut sys::VrlCtx, msg: sys::VrlStr) {
    unsafe { host::fail(msg.ptr, msg.len) }
}

unsafe extern "C" fn finish(_: *mut sys::VrlCtx) {
    unsafe { host::finish() }
}

unsafe extern "C" fn log(_: *mut sys::VrlCtx, msg: sys::VrlStr) {
    unsafe { host::log(msg.ptr, msg.len) }
}

unsafe extern "C" fn cycle(_: *mut sys::VrlCtx) -> u64 {
    unsafe { host::cycle() }
}

unsafe extern "C" fn sim_time(_: *mut sys::VrlCtx) -> u64 {
    unsafe { host::sim_time() }
}

unsafe extern "C" fn seed(_: *mut sys::VrlCtx) -> u64 {
    unsafe { host::seed() }
}

unsafe extern "C" fn is_4state(_: *mut sys::VrlCtx) -> u32 {
    unsafe { host::is_4state() }
}

unsafe extern "C" fn fired_clock(_: *mut sys::VrlCtx) -> u32 {
    unsafe { host::fired_clock() }
}

unsafe extern "C" fn file_open(_: *mut sys::VrlCtx, path: sys::VrlStr, mode: u32) -> i32 {
    unsafe { host::file_open(path.ptr, path.len, mode) }
}

unsafe extern "C" fn file_read(_: *mut sys::VrlCtx, handle: i32, buf: *mut u8, len: usize) -> i64 {
    unsafe { host::file_read(handle, buf, len) }
}

unsafe extern "C" fn file_write(
    _: *mut sys::VrlCtx,
    handle: i32,
    buf: *const u8,
    len: usize,
) -> i64 {
    unsafe { host::file_write(handle, buf, len) }
}

unsafe extern "C" fn file_seek(_: *mut sys::VrlCtx, handle: i32, pos: i64, whence: u32) -> i64 {
    unsafe { host::file_seek(handle, pos, whence) }
}

unsafe extern "C" fn file_close(_: *mut sys::VrlCtx, handle: i32) {
    unsafe { host::file_close(handle) }
}

unsafe extern "C" fn trace_var(_: *mut sys::VrlCtx, name: sys::VrlStr, width: u32) -> i32 {
    unsafe { host::trace_var(name.ptr, name.len, width) }
}

unsafe extern "C" fn trace_write(_: *mut sys::VrlCtx, handle: i32, words: *const u64) {
    unsafe { host::trace_write(handle, words) }
}

static HOST_API: sys::VrlHostApi = sys::VrlHostApi {
    size: size_of::<sys::VrlHostApi>(),
    port_index,
    port_width,
    read_input,
    write_output,
    param_get,
    fail,
    finish,
    log,
    cycle,
    sim_time,
    seed,
    fired_clock,
    file_open,
    file_read,
    file_write,
    file_seek,
    file_close,
    trace_var,
    trace_write,
    is_4state,
};

/// One live component instance; its box pointer is the `u32` handle
/// crossing the wasm boundary.
struct Instance {
    state: *mut c_void,
    vtable: &'static sys::VrlComponentVTable,
}

fn lookup(
    table: &'static [(&'static str, sys::VrlComponentVTable)],
    name_ptr: *const u8,
    name_len: usize,
) -> Option<&'static sys::VrlComponentVTable> {
    let name = if name_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(name_ptr, name_len) }
    };
    let name = std::str::from_utf8(name).ok()?;
    table.iter().find(|(n, _)| *n == name).map(|(_, vt)| vt)
}

/// # Safety
/// `name_ptr`/`name_len` must reference valid guest memory.
pub unsafe fn kind(
    table: &'static [(&'static str, sys::VrlComponentVTable)],
    name_ptr: *const u8,
    name_len: usize,
) -> u32 {
    lookup(table, name_ptr, name_len).map_or(u32::MAX, |vt| vt.kind)
}

/// Reports panics through `fail` before the abort trap reaches the host,
/// preserving the message like the native catch_unwind wrappers do.
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let msg = format!("component panicked: {info}");
            unsafe { host::fail(msg.as_ptr(), msg.len()) };
        }));
    });
}

/// Returns the instance handle, or 0 for an unknown type / failed
/// construction (already reported via `fail`).
///
/// # Safety
/// `name_ptr`/`name_len` must reference valid guest memory.
pub unsafe fn create(
    table: &'static [(&'static str, sys::VrlComponentVTable)],
    name_ptr: *const u8,
    name_len: usize,
) -> u32 {
    install_panic_hook();
    let Some(vtable) = lookup(table, name_ptr, name_len) else {
        return 0;
    };
    let state = unsafe { (vtable.create)(std::ptr::null_mut(), &HOST_API) };
    if state.is_null() {
        return 0;
    }
    Box::into_raw(Box::new(Instance { state, vtable })) as usize as u32
}

/// # Safety
/// `handle` must come from [`create`] and not be used afterwards.
pub unsafe fn destroy(handle: u32) {
    if handle == 0 {
        return;
    }
    let instance = unsafe { Box::from_raw(handle as usize as *mut Instance) };
    unsafe { (instance.vtable.destroy)(instance.state) };
}

/// # Safety
/// `handle` must be a live handle from [`create`].
pub unsafe fn on_init(handle: u32) -> i32 {
    let instance = unsafe { &*(handle as usize as *const Instance) };
    unsafe { (instance.vtable.on_init)(instance.state, std::ptr::null_mut()) }
}

/// # Safety
/// `handle` must be a live handle from [`create`].
pub unsafe fn on_reset(handle: u32) -> i32 {
    let instance = unsafe { &*(handle as usize as *const Instance) };
    unsafe { (instance.vtable.on_reset)(instance.state, std::ptr::null_mut()) }
}

/// # Safety
/// `handle` must be a live handle from [`create`].
pub unsafe fn on_clock(handle: u32) -> i32 {
    let instance = unsafe { &*(handle as usize as *const Instance) };
    unsafe { (instance.vtable.on_clock)(instance.state, std::ptr::null_mut()) }
}

/// # Safety
/// `handle` must be a live handle from [`create`].
pub unsafe fn on_finish(handle: u32) -> i32 {
    let instance = unsafe { &*(handle as usize as *const Instance) };
    unsafe { (instance.vtable.on_finish)(instance.state, std::ptr::null_mut()) }
}

/// # Safety
/// `handle` must be a live handle from [`create`]; `name`, `args` and `ret`
/// must reference valid guest memory, `ret.words` pre-allocated with
/// `ret.nwords` capacity as in the native contract.
pub unsafe fn call_method(
    handle: u32,
    name_ptr: *const u8,
    name_len: usize,
    args: *const sys::VrlValue,
    nargs: usize,
    ret: *mut sys::VrlValue,
) -> i32 {
    let instance = unsafe { &*(handle as usize as *const Instance) };
    let name = sys::VrlStr {
        ptr: name_ptr,
        len: name_len,
    };
    unsafe {
        (instance.vtable.call_method)(instance.state, std::ptr::null_mut(), name, args, nargs, ret)
    }
}

/// Allocates a guest buffer for the host (method args/returns, `param_get`
/// payloads). 8-byte aligned so it can hold `u64` words.
pub fn alloc(size: usize) -> *mut u8 {
    let Ok(layout) = Layout::from_size_align(size.max(1), 8) else {
        return std::ptr::null_mut();
    };
    unsafe { std::alloc::alloc(layout) }
}

/// # Safety
/// `ptr` must come from [`alloc`] with the same `size`.
pub unsafe fn free(ptr: *mut u8, size: usize) {
    if ptr.is_null() {
        return;
    }
    let layout = Layout::from_size_align(size.max(1), 8).expect("layout was valid in alloc");
    unsafe { std::alloc::dealloc(ptr, layout) };
}
