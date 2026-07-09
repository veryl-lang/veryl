//! Guest-side ABI wrappers instantiated by `veryl_component_export!`.
//!
//! Every entry point catches unwinds so no panic crosses the FFI boundary:
//! panics and `Err` returns are reported through the host `fail` service and
//! folded into the non-zero return code.

use crate::{BuildCtx, Component, SimCtx, Value, sys};
use std::any::Any;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

struct Instance<T> {
    api: *const sys::VrlHostApi,
    inner: T,
}

/// # Safety
/// `ctx` and `api` must be the live host pointers for the current call.
unsafe fn host_fail(ctx: *mut sys::VrlCtx, api: *const sys::VrlHostApi, msg: &str) {
    unsafe { ((*api).fail)(ctx, sys::VrlStr::from_str(msg)) };
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        format!("component panicked: {s}")
    } else if let Some(s) = payload.downcast_ref::<String>() {
        format!("component panicked: {s}")
    } else {
        "component panicked".to_string()
    }
}

pub unsafe extern "C" fn create<T: Component>(
    ctx: *mut sys::VrlCtx,
    api: *const sys::VrlHostApi,
) -> *mut c_void {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut build = unsafe { BuildCtx::new(ctx, api) };
        T::new(&mut build)
    }));
    match result {
        Ok(Ok(inner)) => Box::into_raw(Box::new(Instance { api, inner })) as *mut c_void,
        Ok(Err(e)) => {
            unsafe { host_fail(ctx, api, &format!("{e:#}")) };
            std::ptr::null_mut()
        }
        Err(payload) => {
            unsafe { host_fail(ctx, api, &panic_message(payload)) };
            std::ptr::null_mut()
        }
    }
}

pub unsafe extern "C" fn destroy<T: Component>(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let instance = unsafe { Box::from_raw(state as *mut Instance<T>) };
    // A panicking Drop must not unwind into the host.
    let _ = catch_unwind(AssertUnwindSafe(move || drop(instance)));
}

unsafe fn run_hook<T: Component>(
    state: *mut c_void,
    ctx: *mut sys::VrlCtx,
    hook: impl FnOnce(&mut T, &mut SimCtx) -> crate::Result<()>,
) -> i32 {
    let instance = unsafe { &mut *(state as *mut Instance<T>) };
    let api = instance.api;
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut sim = unsafe { SimCtx::new(ctx, api) };
        hook(&mut instance.inner, &mut sim)
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            unsafe { host_fail(ctx, api, &format!("{e:#}")) };
            1
        }
        Err(payload) => {
            unsafe { host_fail(ctx, api, &panic_message(payload)) };
            1
        }
    }
}

pub unsafe extern "C" fn on_init<T: Component>(state: *mut c_void, ctx: *mut sys::VrlCtx) -> i32 {
    unsafe { run_hook::<T>(state, ctx, |c, sim| c.on_init(sim)) }
}

pub unsafe extern "C" fn on_reset<T: Component>(state: *mut c_void, ctx: *mut sys::VrlCtx) -> i32 {
    unsafe { run_hook::<T>(state, ctx, |c, sim| c.on_reset(sim)) }
}

pub unsafe extern "C" fn on_clock<T: Component>(state: *mut c_void, ctx: *mut sys::VrlCtx) -> i32 {
    unsafe { run_hook::<T>(state, ctx, |c, sim| c.on_clock(sim)) }
}

pub unsafe extern "C" fn on_finish<T: Component>(state: *mut c_void, ctx: *mut sys::VrlCtx) -> i32 {
    unsafe { run_hook::<T>(state, ctx, |c, sim| c.on_finish(sim)) }
}

/// Writes a method return value into the host-provided slot. The host
/// pre-allocates `ret.words` with `ret.nwords` capacity; the guest shrinks
/// `nwords` to the actual count.
fn write_return(value: Value, ret: &mut sys::VrlValue) -> crate::Result<()> {
    match value {
        Value::Unit => {
            ret.kind = sys::VRL_VALUE_UNIT;
            ret.width = 0;
            ret.nwords = 0;
            Ok(())
        }
        Value::Bits {
            words,
            mask_xz,
            width,
        } => {
            if words.len() > ret.nwords {
                crate::bail!(
                    "method return value of {width} bits exceeds the host buffer ({} words)",
                    ret.nwords
                );
            }
            unsafe {
                std::ptr::copy_nonoverlapping(words.as_ptr(), ret.words as *mut u64, words.len());
                // The host allocates a parallel mask buffer only for a
                // four-state return slot; a null `mask_xz` means it wants payload
                // only.
                if !ret.mask_xz.is_null() {
                    std::ptr::copy_nonoverlapping(
                        mask_xz.as_ptr(),
                        ret.mask_xz as *mut u64,
                        mask_xz.len(),
                    );
                }
            }
            ret.kind = sys::VRL_VALUE_BITS;
            ret.width = width;
            ret.nwords = words.len();
            Ok(())
        }
        Value::Str(_) => crate::bail!("string method return values are not supported"),
    }
}

pub unsafe extern "C" fn call_method<T: Component>(
    state: *mut c_void,
    ctx: *mut sys::VrlCtx,
    name: sys::VrlStr,
    args: *const sys::VrlValue,
    nargs: usize,
    ret: *mut sys::VrlValue,
) -> i32 {
    let instance = unsafe { &mut *(state as *mut Instance<T>) };
    let api = instance.api;
    let result = catch_unwind(AssertUnwindSafe(|| {
        let name = unsafe { name.as_str() };
        let args: Vec<Value> = if nargs == 0 {
            vec![]
        } else {
            unsafe { std::slice::from_raw_parts(args, nargs) }
                .iter()
                .map(|v| unsafe { Value::from_vrl(v) })
                .collect()
        };
        let mut sim = unsafe { SimCtx::new(ctx, api) };
        let value = instance.inner.method(name, &args, &mut sim)?;
        write_return(value, unsafe { &mut *ret })
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            unsafe { host_fail(ctx, api, &format!("{e:#}")) };
            1
        }
        Err(payload) => {
            unsafe { host_fail(ctx, api, &panic_message(payload)) };
            1
        }
    }
}

/// Referenced from the `veryl_component_export!` expansion.
pub const fn vtable<T: Component>() -> sys::VrlComponentVTable {
    sys::VrlComponentVTable {
        abi_version: sys::VRL_COMPONENT_ABI_VERSION,
        kind: T::KIND as u32,
        create: create::<T>,
        destroy: destroy::<T>,
        on_init: on_init::<T>,
        on_reset: on_reset::<T>,
        on_clock: on_clock::<T>,
        call_method: call_method::<T>,
        on_finish: on_finish::<T>,
    }
}

// ---------------------------------------------------------------------------
// Compile-time manifest assembly
// ---------------------------------------------------------------------------
// The manifest must exist as a constant so `veryl_component_export!` can
// serve it from the native symbol and embed it verbatim in the
// `veryl.manifest` wasm custom section, readable without executing the
// guest.

const fn copy_str(out: &mut [u8], mut pos: usize, s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        out[pos] = bytes[i];
        pos += 1;
        i += 1;
    }
    pos
}

pub const fn concat_len(parts: &[&str]) -> usize {
    let mut len = 0;
    let mut i = 0;
    while i < parts.len() {
        len += parts[i].len();
        i += 1;
    }
    len
}

/// Concatenates `parts`; `N` must be [`concat_len`] of the same parts.
pub const fn concat_bytes<const N: usize>(parts: &[&str]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut pos = 0;
    let mut i = 0;
    while i < parts.len() {
        pos = copy_str(&mut out, pos, parts[i]);
        i += 1;
    }
    out
}

/// `&str` view of a byte array assembled by the helpers above; invalid
/// UTF-8 fails at compile time.
pub const fn bytes_as_str(bytes: &[u8]) -> &str {
    match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => panic!("generated manifest is not valid UTF-8"),
    }
}

/// Length of the aggregated `{"types":{...}}` manifest document over the
/// exported `(name, manifest)` entries; `None` entries are skipped.
pub const fn manifest_json_len(entries: &[(&str, Option<&str>)]) -> usize {
    let mut len = r#"{"types":{"#.len() + "}}".len();
    let mut first = true;
    let mut i = 0;
    while i < entries.len() {
        if let Some(m) = entries[i].1 {
            if !first {
                len += 1;
            }
            first = false;
            len += 1 + entries[i].0.len() + 2 + m.len();
        }
        i += 1;
    }
    len
}

/// Aggregated manifest document; `N` must be [`manifest_json_len`] of the
/// same entries.
pub const fn manifest_json_bytes<const N: usize>(entries: &[(&str, Option<&str>)]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut pos = copy_str(&mut out, 0, r#"{"types":{"#);
    let mut first = true;
    let mut i = 0;
    while i < entries.len() {
        if let Some(m) = entries[i].1 {
            if !first {
                pos = copy_str(&mut out, pos, ",");
            }
            first = false;
            pos = copy_str(&mut out, pos, "\"");
            pos = copy_str(&mut out, pos, entries[i].0);
            pos = copy_str(&mut out, pos, "\":");
            pos = copy_str(&mut out, pos, m);
        }
        i += 1;
    }
    let _ = copy_str(&mut out, pos, "}}");
    out
}

#[cfg(target_family = "wasm")]
pub mod wasm;
