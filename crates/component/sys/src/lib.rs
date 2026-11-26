//! C ABI between the Veryl simulator (host) and user-defined verification
//! components (guest libraries).
//!
//! This is an *internal* stable boundary: the `veryl_component_export!`
//! macro in `veryl-component` generates the guest side, and the simulator
//! implements the host side. It is not a public contract for third parties.
//!
//! Stability policy:
//! - The host requires an exact [`VRL_COMPONENT_ABI_VERSION`] match at load
//!   time.
//! - [`VrlHostApi`] is append-only; its leading `size` field lets guests
//!   detect which entries the host provides.

use std::ffi::c_void;

/// Incremented on any breaking layout change. Checked at load time.
pub const VRL_COMPONENT_ABI_VERSION: u32 = 1;

/// The component crate family's version, for tooling that pins a matching
/// `veryl-component` dependency (e.g. the `veryl new --component` scaffold).
/// The family is released in lockstep, so this stands in for every member.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Host-port name of an interface-group member: the key generated guest
/// code resolves and the host synthesizes when expanding a modport
/// connection. Changing the format is an ABI break, like any other.
pub fn member_port_name(group: &str, member: &str) -> String {
    format!("{group}.{member}")
}

/// Whether `name` is a valid component export name: a Veryl identifier
/// (`[a-zA-Z_][0-9a-zA-Z_]*`). The name becomes the `$comp::<name>` path,
/// so anything else would be unreferencable — and a `::` inside would be
/// mistaken for a dependency prefix.
pub const fn is_valid_component_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let ok = b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit());
        if !ok {
            return false;
        }
        i += 1;
    }
    true
}

/// Component kind carried in [`VrlComponentVTable::kind`].
pub const VRL_KIND_UNSPECIFIED: u32 = 0;
/// Clocked component, instantiated with `inst`.
pub const VRL_KIND_CLOCKED: u32 = 1;
/// Method-only component, declared with `var`.
pub const VRL_KIND_METHOD_ONLY: u32 = 2;

/// Port direction for [`VrlHostApi::port_index`]. `VRL_DIR_INPUT` matches
/// any input connection; `VRL_DIR_CLOCK`/`VRL_DIR_RESET` additionally
/// require the connection to be a clock/reset, making a wrong connection a
/// load-time error.
pub const VRL_DIR_INPUT: u32 = 0;
pub const VRL_DIR_OUTPUT: u32 = 1;
pub const VRL_DIR_CLOCK: u32 = 2;
pub const VRL_DIR_RESET: u32 = 3;

/// [`VrlValue::kind`] discriminants.
pub const VRL_VALUE_BITS: u32 = 0;
pub const VRL_VALUE_STRING: u32 = 1;
pub const VRL_VALUE_UNIT: u32 = 2;

/// File open modes for [`VrlHostApi::file_open`].
pub const VRL_FILE_READ: u32 = 0;
pub const VRL_FILE_CREATE: u32 = 1;
pub const VRL_FILE_APPEND: u32 = 2;

/// Seek origins for [`VrlHostApi::file_seek`].
pub const VRL_SEEK_SET: u32 = 0;
pub const VRL_SEEK_CUR: u32 = 1;
pub const VRL_SEEK_END: u32 = 2;

/// Opaque host-side context. Valid only for the duration of the host call
/// that passed it; guests must not retain it across calls.
#[repr(C)]
#[derive(Debug)]
pub struct VrlCtx {
    _private: [u8; 0],
}

/// Borrowed UTF-8 string. Not NUL-terminated.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VrlStr {
    pub ptr: *const u8,
    pub len: usize,
}

impl VrlStr {
    /// # Safety
    /// `s` must outlive every use of the returned `VrlStr`.
    pub const fn from_str(s: &str) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: s.len(),
        }
    }

    /// # Safety
    /// `ptr`/`len` must reference valid UTF-8 for the lifetime `'a`.
    pub unsafe fn as_str<'a>(&self) -> &'a str {
        if self.len == 0 {
            return "";
        }
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.ptr, self.len)) }
    }
}

/// Value crossing the boundary (parameters, method arguments and returns).
///
/// For `kind == VRL_VALUE_BITS`, `words` points to `nwords` LSB-first 64-bit
/// words holding `width` bits; excess high bits are zero. When used as a
/// method return slot, the host pre-allocates `words` with `nwords` capacity
/// and the guest writes the actual word count back into `nwords`. The guest
/// must write the payload into that host-provided buffer; repointing `words`
/// at guest storage is ignored — the host reads the buffer it allocated.
///
/// `mask_xz` is the parallel four-state mask (`nwords` LSB-first words): a set bit
/// marks the corresponding payload bit as X (payload 0) or Z (payload 1). A
/// null `mask_xz` means the value is fully two-state. Only meaningful under a
/// four-state simulation ([`VrlHostApi::is_4state`]).
///
/// Aggregate port connections flatten as the simulator stores them, which
/// matches the SystemVerilog packed layout: a struct's first member
/// occupies the most-significant bits. Unpacked-array connections are not
/// supported.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VrlValue {
    pub kind: u32,
    pub width: u32,
    pub words: *const u64,
    pub nwords: usize,
    /// Four-state mask, `nwords` words parallel to `words`, or null for a
    /// two-state value. See the type-level docs.
    pub mask_xz: *const u64,
    pub str_: VrlStr,
}

impl VrlValue {
    pub const fn unit() -> Self {
        Self {
            kind: VRL_VALUE_UNIT,
            width: 0,
            words: std::ptr::null(),
            nwords: 0,
            mask_xz: std::ptr::null(),
            str_: VrlStr {
                ptr: std::ptr::null(),
                len: 0,
            },
        }
    }
}

/// Direct pointers into a port's host-side buffers, filled by
/// [`VrlHostApi::port_direct`]. They let a hot path read or write a port
/// without a per-access indirect host call. Only the in-process (native)
/// transport can share host memory; other transports report no direct access.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VrlPortDirect {
    /// Port value words (LSB-first), sized by the port width.
    pub words: *mut u64,
    /// Four-state mask parallel to `words`; a set bit is X or Z.
    pub mask_xz: *mut u64,
    /// Output ports only: the dirty flag the host reads when applying
    /// outputs. A direct writer must set it (`1`) itself, since it bypasses
    /// [`VrlHostApi::write_output`]. Null for input ports.
    pub dirty: *mut u8,
}

impl VrlPortDirect {
    pub fn null() -> Self {
        Self {
            words: core::ptr::null_mut(),
            mask_xz: core::ptr::null_mut(),
            dirty: core::ptr::null_mut(),
        }
    }
}

/// Host services table passed to `create` and retained by the guest wrapper.
/// Append-only: new entries go at the end and `size` tells the guest how far
/// the table extends.
#[repr(C)]
#[derive(Debug)]
pub struct VrlHostApi {
    pub size: usize,
    // Ports. Name resolution is expected only while `create` runs.
    /// Returns the port index, or -1 if no port has this name and direction.
    pub port_index: unsafe extern "C" fn(*mut VrlCtx, name: VrlStr, dir: u32) -> i32,
    pub port_width: unsafe extern "C" fn(*mut VrlCtx, idx: u32) -> u32,
    /// Copies the pre-edge input value into `words` and its four-state mask
    /// into `mask_xz` (both caller-sized from `port_width`). Under a two-state
    /// simulation `mask_xz` is filled with zeros. A null `mask_xz` skips it.
    pub read_input: unsafe extern "C" fn(*mut VrlCtx, idx: u32, words: *mut u64, mask_xz: *mut u64),
    /// Writes the output payload from `words` and its four-state mask from
    /// `mask_xz` (a null `mask_xz` drives a fully two-state value).
    pub write_output:
        unsafe extern "C" fn(*mut VrlCtx, idx: u32, words: *const u64, mask_xz: *const u64),
    /// Returns 0 and fills `out` on success, -1 if the parameter is absent.
    /// `out` borrows host storage valid until the host call returns.
    pub param_get: unsafe extern "C" fn(*mut VrlCtx, name: VrlStr, out: *mut VrlValue) -> i32,
    // Control and observation.
    pub fail: unsafe extern "C" fn(*mut VrlCtx, msg: VrlStr),
    pub finish: unsafe extern "C" fn(*mut VrlCtx),
    pub log: unsafe extern "C" fn(*mut VrlCtx, msg: VrlStr),
    pub cycle: unsafe extern "C" fn(*mut VrlCtx) -> u64,
    pub sim_time: unsafe extern "C" fn(*mut VrlCtx) -> u64,
    pub seed: unsafe extern "C" fn(*mut VrlCtx) -> u64,
    /// Input port index of the clock that fired the current hook.
    pub fired_clock: unsafe extern "C" fn(*mut VrlCtx) -> u32,
    // Host-mediated file service.
    /// Returns a handle, or -1 on failure.
    pub file_open: unsafe extern "C" fn(*mut VrlCtx, path: VrlStr, mode: u32) -> i32,
    /// Returns bytes read, or -1 on failure.
    pub file_read: unsafe extern "C" fn(*mut VrlCtx, handle: i32, buf: *mut u8, len: usize) -> i64,
    /// Returns bytes written, or -1 on failure.
    pub file_write:
        unsafe extern "C" fn(*mut VrlCtx, handle: i32, buf: *const u8, len: usize) -> i64,
    /// Returns the new position, or -1 on failure.
    pub file_seek: unsafe extern "C" fn(*mut VrlCtx, handle: i32, pos: i64, whence: u32) -> i64,
    pub file_close: unsafe extern "C" fn(*mut VrlCtx, handle: i32),
    // Waveform tracing of component-internal signals.
    /// Registers a trace variable. Only allowed while `create` runs (the
    /// waveform header is finalized right after all components are built);
    /// returns a handle, or -1 outside `create` / on a duplicate name.
    pub trace_var: unsafe extern "C" fn(*mut VrlCtx, name: VrlStr, width: u32) -> i32,
    /// Updates a trace variable's value with `words` LSB-first 64-bit words
    /// (sized by the registered width).
    pub trace_write: unsafe extern "C" fn(*mut VrlCtx, handle: i32, words: *const u64),
    /// Nonzero when the simulation is four-state, so input masks and driven X/Z
    /// are meaningful. A component gates its X/Z checks on this.
    pub is_4state: unsafe extern "C" fn(*mut VrlCtx) -> u32,
    /// Fills `out` with direct pointers into the port's host buffers and
    /// returns 1, or returns 0 when the transport cannot share host memory
    /// (e.g. the wasm sandbox). The pointers stay valid for the instance's
    /// lifetime, so the guest caches them at `create` time and then reads or
    /// writes the buffers without a per-access host call.
    pub port_direct: unsafe extern "C" fn(*mut VrlCtx, idx: u32, out: *mut VrlPortDirect) -> u32,
    // Runtime arguments (`veryl test --runtime-arg`), in the order given.
    /// Number of runtime arguments, overridden ones included.
    pub runtime_arg_count: unsafe extern "C" fn(*mut VrlCtx) -> u32,
    /// Copies the UTF-8 name of argument `idx` into `buf` and returns its
    /// length in bytes, or -1 if `idx` is out of range. Nothing is written
    /// when the length exceeds `cap`; the caller retries with a larger buffer.
    pub runtime_arg_name:
        unsafe extern "C" fn(*mut VrlCtx, idx: u32, buf: *mut u8, cap: usize) -> i64,
    /// Like [`VrlHostApi::runtime_arg_name`] for the value of argument `idx`.
    /// Returns -1 if `idx` is out of range or the argument is a flag.
    pub runtime_arg_value:
        unsafe extern "C" fn(*mut VrlCtx, idx: u32, buf: *mut u8, cap: usize) -> i64,
}

/// Least [`VrlHostApi::size`] that includes the trace entries. Derived from
/// the last trace field's extent, not the current struct size, so appending
/// future entries does not change it.
pub const VRL_HOST_API_TRACE_SIZE: usize = std::mem::offset_of!(VrlHostApi, trace_write)
    + size_of::<unsafe extern "C" fn(*mut VrlCtx, i32, *const u64)>();

/// Least [`VrlHostApi::size`] that includes [`VrlHostApi::port_direct`]. A
/// guest checks its `size` against this before caching direct port pointers,
/// so a host that omits the entry falls back to the copy path.
pub const VRL_HOST_API_DIRECT_SIZE: usize = std::mem::offset_of!(VrlHostApi, port_direct)
    + size_of::<unsafe extern "C" fn(*mut VrlCtx, u32, *mut VrlPortDirect) -> u32>();

/// Least [`VrlHostApi::size`] that includes the runtime argument entries.
pub const VRL_HOST_API_RUNTIME_ARG_SIZE: usize =
    std::mem::offset_of!(VrlHostApi, runtime_arg_value)
        + size_of::<unsafe extern "C" fn(*mut VrlCtx, u32, *mut u8, usize) -> i64>();

/// An integer read from a runtime argument's text: a sign and the
/// magnitude as LSB-first 64-bit words (no trailing zero words).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeArgInt {
    pub negative: bool,
    pub magnitude: Vec<u64>,
}

impl RuntimeArgInt {
    /// Whether the value is representable in `width` bits, as unsigned
    /// (`signed == false`) or two's complement (`signed == true`). A
    /// negative value never fits an unsigned width.
    pub fn fits(&self, width: usize, signed: bool) -> bool {
        let bits = self.magnitude_bits();
        if self.is_zero() {
            return true;
        }
        if !signed {
            return !self.negative && bits <= width;
        }
        if width == 0 {
            return false;
        }
        if self.negative {
            // -(2^(width-1)) is the least value.
            bits < width || (bits == width && self.is_power_of_two())
        } else {
            bits < width
        }
    }

    /// The value as LSB-first words of `width` bits in two's complement.
    /// The caller checks [`RuntimeArgInt::fits`] first.
    pub fn to_words(&self, width: usize) -> Vec<u64> {
        let len = width.div_ceil(64).max(1);
        let mut words = self.magnitude.clone();
        words.resize(len, 0);
        if self.negative {
            let mut carry = 1u64;
            for w in &mut words {
                let (v, c) = (!*w).overflowing_add(carry);
                *w = v;
                carry = c as u64;
            }
        }
        let rem = width % 64;
        if rem != 0 {
            words[len - 1] &= (1u64 << rem) - 1;
        }
        words
    }

    fn is_zero(&self) -> bool {
        self.magnitude.is_empty()
    }

    fn magnitude_bits(&self) -> usize {
        match self.magnitude.last() {
            Some(top) => (self.magnitude.len() - 1) * 64 + (64 - top.leading_zeros() as usize),
            None => 0,
        }
    }

    fn is_power_of_two(&self) -> bool {
        self.magnitude.iter().map(|w| w.count_ones()).sum::<u32>() == 1
    }
}

/// Reads a runtime argument's text as an integer: decimal, or hexadecimal /
/// binary with a `0x` / `0b` prefix, optionally preceded by `-`, with `_`
/// allowed between digits (`4`, `-3`, `0x1f`, `0xdead_beef`). Returns
/// `None` for any other text. Shared by the simulator and the component
/// SDK so both read numbers alike.
pub fn parse_runtime_arg_int(text: &str) -> Option<RuntimeArgInt> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (radix, digits) = if let Some(rest) = body.strip_prefix("0x") {
        (16u32, rest)
    } else if let Some(rest) = body.strip_prefix("0b") {
        (2u32, rest)
    } else {
        (10u32, body)
    };

    let bytes = digits.as_bytes();
    if bytes.is_empty() || bytes[0] == b'_' || bytes[bytes.len() - 1] == b'_' {
        return None;
    }

    let mut magnitude: Vec<u64> = Vec::new();
    let mut prev_underscore = false;
    for &b in bytes {
        if b == b'_' {
            if prev_underscore {
                return None;
            }
            prev_underscore = true;
            continue;
        }
        prev_underscore = false;
        let digit = (b as char).to_digit(radix)? as u64;
        // magnitude = magnitude * radix + digit
        let mut carry = digit;
        for w in &mut magnitude {
            let v = (*w as u128) * (radix as u128) + carry as u128;
            *w = v as u64;
            carry = (v >> 64) as u64;
        }
        if carry != 0 {
            magnitude.push(carry);
        }
    }
    while magnitude.last() == Some(&0) {
        magnitude.pop();
    }

    Some(RuntimeArgInt {
        negative: negative && !magnitude.is_empty(),
        magnitude,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_name_validity() {
        assert!(is_valid_component_name("mirror"));
        assert!(is_valid_component_name("_rv_iss2"));
        assert!(!is_valid_component_name(""));
        assert!(!is_valid_component_name("1bad"));
        assert!(!is_valid_component_name("bus::monitor"));
        assert!(!is_valid_component_name("has-dash"));
    }

    #[test]
    fn trace_size_covers_trace_entries_only() {
        assert!(VRL_HOST_API_TRACE_SIZE <= size_of::<VrlHostApi>());
        assert!(VRL_HOST_API_TRACE_SIZE > std::mem::offset_of!(VrlHostApi, trace_var));
    }

    #[test]
    fn direct_size_covers_port_direct() {
        assert!(VRL_HOST_API_DIRECT_SIZE <= size_of::<VrlHostApi>());
        assert!(VRL_HOST_API_DIRECT_SIZE > std::mem::offset_of!(VrlHostApi, port_direct));
    }

    #[test]
    fn runtime_arg_size_covers_runtime_arg_entries() {
        assert_eq!(VRL_HOST_API_RUNTIME_ARG_SIZE, size_of::<VrlHostApi>());
        const { assert!(VRL_HOST_API_RUNTIME_ARG_SIZE > VRL_HOST_API_DIRECT_SIZE) };
    }

    fn int(negative: bool, magnitude: &[u64]) -> Option<RuntimeArgInt> {
        Some(RuntimeArgInt {
            negative,
            magnitude: magnitude.to_vec(),
        })
    }

    #[test]
    fn parse_runtime_arg_int_forms() {
        assert_eq!(parse_runtime_arg_int("0"), int(false, &[]));
        assert_eq!(parse_runtime_arg_int("-0"), int(false, &[]));
        assert_eq!(parse_runtime_arg_int("4"), int(false, &[4]));
        assert_eq!(parse_runtime_arg_int("-3"), int(true, &[3]));
        assert_eq!(parse_runtime_arg_int("1_000"), int(false, &[1000]));
        assert_eq!(parse_runtime_arg_int("0x1f"), int(false, &[0x1f]));
        assert_eq!(
            parse_runtime_arg_int("0xDEAD_beef"),
            int(false, &[0xdead_beef])
        );
        assert_eq!(parse_runtime_arg_int("0b1010"), int(false, &[0b1010]));
        assert_eq!(parse_runtime_arg_int("-0x10"), int(true, &[0x10]));
        assert_eq!(
            parse_runtime_arg_int("0x1_0000_0000_0000_0000"),
            int(false, &[0, 1])
        );
        assert_eq!(
            parse_runtime_arg_int("18446744073709551616"),
            int(false, &[0, 1])
        );

        for bad in [
            "", "-", "abc", "0x", "0b", "0b12", "_1", "1_", "1__0", "0x_1", "+1", "1.0", "'h1f",
            " 1", "0X1f", "--1",
        ] {
            assert_eq!(parse_runtime_arg_int(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn runtime_arg_int_fits_and_words() {
        let v = |s: &str| parse_runtime_arg_int(s).unwrap();

        assert!(v("255").fits(8, false));
        assert!(!v("256").fits(8, false));
        assert!(!v("-1").fits(8, false));
        assert!(v("127").fits(8, true));
        assert!(!v("128").fits(8, true));
        assert!(v("-128").fits(8, true));
        assert!(!v("-129").fits(8, true));
        assert!(v("0").fits(1, false));
        assert!(v("-1").fits(1, true));
        assert!(!v("1").fits(1, true));

        assert_eq!(v("-1").to_words(8), vec![0xff]);
        assert_eq!(v("-128").to_words(8), vec![0x80]);
        assert_eq!(v("-1").to_words(64), vec![!0]);
        assert_eq!(v("-1").to_words(70), vec![!0, 0x3f]);
        assert_eq!(v("0x1f").to_words(32), vec![0x1f]);
    }
}

/// Per-component-type entry points. Hook return value: 0 = ok, non-zero =
/// internal component error (the guest wrapper has already reported it via
/// `fail`; the host treats the test as failed).
#[repr(C)]
#[derive(Debug)]
pub struct VrlComponentVTable {
    pub abi_version: u32,
    /// One of `VRL_KIND_*`.
    pub kind: u32,
    /// Returns the instance state, or NULL on failure (already reported via
    /// `fail`). The `VrlHostApi` pointer must stay valid for the lifetime of
    /// the instance.
    pub create: unsafe extern "C" fn(*mut VrlCtx, *const VrlHostApi) -> *mut c_void,
    pub destroy: unsafe extern "C" fn(*mut c_void),
    pub on_init: unsafe extern "C" fn(*mut c_void, *mut VrlCtx) -> i32,
    pub on_reset: unsafe extern "C" fn(*mut c_void, *mut VrlCtx) -> i32,
    pub on_clock: unsafe extern "C" fn(*mut c_void, *mut VrlCtx) -> i32,
    pub call_method: unsafe extern "C" fn(
        *mut c_void,
        *mut VrlCtx,
        name: VrlStr,
        args: *const VrlValue,
        nargs: usize,
        ret: *mut VrlValue,
    ) -> i32,
    pub on_finish: unsafe extern "C" fn(*mut c_void, *mut VrlCtx) -> i32,
}

/// Name of the symbol every component library exports:
/// `extern "C" fn(VrlStr) -> *const VrlComponentVTable` (NULL = unknown type).
pub const VRL_LOOKUP_SYMBOL: &str = "veryl_component_lookup";

/// Signature of the exported lookup symbol.
pub type VrlLookupFn = unsafe extern "C" fn(VrlStr) -> *const VrlComponentVTable;

/// Name of the optional manifest symbol: `extern "C" fn() -> VrlStr`
/// returning the aggregated JSON manifest of the exported types
/// (`{"types":{"<name>":{...},...}}`). Hosts must tolerate its absence:
/// hand-written exports may not provide it.
pub const VRL_MANIFEST_SYMBOL: &str = "veryl_component_manifest";

/// Signature of the exported manifest symbol.
pub type VrlManifestFn = unsafe extern "C" fn() -> VrlStr;

// ---------------------------------------------------------------------------
// wasm transport
// ---------------------------------------------------------------------------
// On wasm the vtable indirection is replaced by `veryl_component_*` guest
// exports keyed on a `u32` instance handle, and the host services become
// imports from the `VRL_WASM_IMPORT_MODULE` module (scalar-only signatures;
// pointers are guest linear-memory offsets). The guest side is generated by
// `veryl_component_export!`; the host side lives in the simulator.

/// wasm import module providing the host services.
pub const VRL_WASM_IMPORT_MODULE: &str = "veryl";

/// Custom section holding the aggregated JSON manifest, embedded by
/// `veryl_component_export!` at build time.
pub const VRL_WASM_MANIFEST_SECTION: &str = "veryl.manifest";

/// Custom section holding the source hash, appended by `veryl publish`.
pub const VRL_WASM_SOURCE_HASH_SECTION: &str = "veryl.source_hash";

/// Guest (wasm32) memory layout of the boundary structs, for the host-side
/// marshalling layer. On wasm32 pointers and `usize` are 4 bytes, so
/// [`VrlStr`]/[`VrlValue`] have a different layout than on a 64-bit host;
/// every offset is spelled out here and nowhere else.
pub mod wasm32 {
    /// `VrlStr` as laid out in guest memory: `ptr: u32, len: u32`.
    pub const STR_SIZE: u32 = 8;

    /// `VrlValue` as laid out in guest memory.
    pub const VALUE_SIZE: u32 = 28;

    /// Decoded guest-side `VrlValue`. Pointer fields are guest
    /// linear-memory offsets.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct VrlValue32 {
        pub kind: u32,
        pub width: u32,
        pub words: u32,
        pub nwords: u32,
        pub mask_xz: u32,
        pub str_ptr: u32,
        pub str_len: u32,
    }

    impl VrlValue32 {
        pub fn from_le_bytes(bytes: &[u8; VALUE_SIZE as usize]) -> Self {
            let field = |i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
            Self {
                kind: field(0),
                width: field(1),
                words: field(2),
                nwords: field(3),
                mask_xz: field(4),
                str_ptr: field(5),
                str_len: field(6),
            }
        }

        pub fn to_le_bytes(self) -> [u8; VALUE_SIZE as usize] {
            let mut out = [0u8; VALUE_SIZE as usize];
            let fields = [
                self.kind,
                self.width,
                self.words,
                self.nwords,
                self.mask_xz,
                self.str_ptr,
                self.str_len,
            ];
            for (i, f) in fields.iter().enumerate() {
                out[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
            }
            out
        }
    }

    // On a wasm guest the native structs must have exactly this layout;
    // checked here so any drift fails the guest build.
    #[cfg(target_family = "wasm")]
    const _: () = {
        assert!(size_of::<super::VrlStr>() == STR_SIZE as usize);
        assert!(size_of::<super::VrlValue>() == VALUE_SIZE as usize);
        assert!(core::mem::offset_of!(super::VrlValue, kind) == 0);
        assert!(core::mem::offset_of!(super::VrlValue, width) == 4);
        assert!(core::mem::offset_of!(super::VrlValue, words) == 8);
        assert!(core::mem::offset_of!(super::VrlValue, nwords) == 12);
        assert!(core::mem::offset_of!(super::VrlValue, mask_xz) == 16);
        assert!(core::mem::offset_of!(super::VrlValue, str_) == 20);
    };

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn value32_roundtrip() {
            let v = VrlValue32 {
                kind: 1,
                width: 2,
                words: 3,
                nwords: 4,
                mask_xz: 7,
                str_ptr: 5,
                str_len: 6,
            };
            let bytes = v.to_le_bytes();
            let r = VrlValue32::from_le_bytes(&bytes);
            assert_eq!(bytes, r.to_le_bytes());
            assert_eq!(r.kind, 1);
            assert_eq!(r.mask_xz, 7);
            assert_eq!(r.str_len, 6);
        }
    }
}
