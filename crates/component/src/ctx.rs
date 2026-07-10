use crate::value::words_for;
use crate::{Result, Value, bail, sys};
use smallvec::SmallVec;

/// Handle to a component input port, resolved once in [`BuildCtx::input`].
/// Its identity is the port index; the cached pointer is an implementation
/// detail excluded from equality and `Debug`.
#[derive(Clone, Copy)]
pub struct InputPort {
    pub(crate) idx: u32,
    pub(crate) width: u32,
    /// Host staging buffer, or null when the host cannot share memory (wasm)
    /// and the read path must fall back to a host call.
    pub(crate) words_ptr: *const u64,
}

/// Handle to a component output port, resolved once in [`BuildCtx::output`].
/// Its identity is the port index; the cached pointers are an implementation
/// detail excluded from equality and `Debug`.
#[derive(Clone, Copy)]
pub struct OutputPort {
    pub(crate) idx: u32,
    pub(crate) width: u32,
    /// Host output buffer, its X/Z mask, and its dirty flag, or null when the
    /// host cannot share memory and the write path must fall back to a call.
    pub(crate) words_ptr: *mut u64,
    pub(crate) mask_ptr: *mut u64,
    pub(crate) dirty_ptr: *mut u8,
}

// The cached pointers address host buffers that outlive the instance and are
// only ever touched by the serialized hook calls, so a handle is safe to move
// with its `Send` component even though it holds raw pointers.
unsafe impl Send for InputPort {}
unsafe impl Send for OutputPort {}

impl PartialEq for InputPort {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx && self.width == other.width
    }
}
impl Eq for InputPort {}

impl std::fmt::Debug for InputPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputPort")
            .field("idx", &self.idx)
            .field("width", &self.width)
            .finish()
    }
}

impl PartialEq for OutputPort {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx && self.width == other.width
    }
}
impl Eq for OutputPort {}

impl std::fmt::Debug for OutputPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputPort")
            .field("idx", &self.idx)
            .field("width", &self.width)
            .finish()
    }
}

impl InputPort {
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Number of 64-bit words the port spans, i.e. the buffer length
    /// [`SimCtx::read_words`] expects.
    pub fn words(&self) -> usize {
        words_for(self.width)
    }
}

impl OutputPort {
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Number of 64-bit words the port spans, i.e. the buffer length
    /// [`SimCtx::write_words`] expects.
    pub fn words(&self) -> usize {
        words_for(self.width)
    }
}

/// Handle to a clock input port, resolved once in [`BuildCtx::clock`].
/// Resolution requires the connection to be a clock, so a mis-wired
/// instance fails at load time. Always 1 bit wide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClockPort {
    pub(crate) idx: u32,
}

/// Handle to a reset input port, resolved once in [`BuildCtx::reset`].
/// Resolution requires the connection to be a reset, so a mis-wired
/// instance fails at load time. Always 1 bit wide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResetPort {
    pub(crate) idx: u32,
}

impl From<ClockPort> for InputPort {
    fn from(port: ClockPort) -> Self {
        InputPort {
            idx: port.idx,
            width: 1,
            words_ptr: std::ptr::null(),
        }
    }
}

impl From<ResetPort> for InputPort {
    fn from(port: ResetPort) -> Self {
        InputPort {
            idx: port.idx,
            width: 1,
            words_ptr: std::ptr::null(),
        }
    }
}

/// Handle to a waveform trace variable, registered once in
/// [`BuildCtx::trace_var`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TraceVar {
    pub(crate) handle: i32,
    pub(crate) width: u32,
}

impl TraceVar {
    pub fn width(&self) -> u32 {
        self.width
    }
}

struct RawCtx {
    ctx: *mut sys::VrlCtx,
    api: *const sys::VrlHostApi,
}

impl RawCtx {
    fn api(&self) -> &sys::VrlHostApi {
        unsafe { &*self.api }
    }

    fn fail(&mut self, msg: &str) {
        unsafe { (self.api().fail)(self.ctx, sys::VrlStr::from_str(msg)) };
    }

    fn finish(&mut self) {
        unsafe { (self.api().finish)(self.ctx) };
    }

    fn log(&mut self, msg: &str) {
        unsafe { (self.api().log)(self.ctx, sys::VrlStr::from_str(msg)) };
    }

    fn port(&mut self, name: &str, dir: u32) -> Option<(u32, u32)> {
        let idx = unsafe { (self.api().port_index)(self.ctx, sys::VrlStr::from_str(name), dir) };
        if idx < 0 {
            return None;
        }
        let width = unsafe { (self.api().port_width)(self.ctx, idx as u32) };
        Some((idx as u32, width))
    }

    /// Whether the host provides the trace entries (`VrlHostApi` is
    /// append-only, so a host that omits later entries reports a smaller
    /// `size`). Compared against the trace entries' extent, not the current
    /// struct size, so entries appended later do not mask trace support.
    fn has_trace_api(&self) -> bool {
        self.api().size >= sys::VRL_HOST_API_TRACE_SIZE
    }

    /// Direct pointers into a port's host buffers, or all-null when the
    /// transport cannot share memory (wasm) or the host omits the entry.
    /// Queried once per port at `create`, so the null check is off the hot
    /// path.
    fn port_direct(&self, idx: u32) -> sys::VrlPortDirect {
        #[cfg(target_family = "wasm")]
        {
            let _ = idx;
            sys::VrlPortDirect::null()
        }
        #[cfg(not(target_family = "wasm"))]
        {
            if self.api().size >= sys::VRL_HOST_API_DIRECT_SIZE {
                let mut out = sys::VrlPortDirect::null();
                if unsafe { (self.api().port_direct)(self.ctx, idx, &mut out) } != 0 {
                    return out;
                }
            }
            sys::VrlPortDirect::null()
        }
    }
}

/// Context passed to [`Component::new`](crate::Component::new). Port and
/// parameter names resolve here, once per instance.
pub struct BuildCtx {
    raw: RawCtx,
}

impl BuildCtx {
    /// # Safety
    /// `ctx` and `api` must be the live host pointers for the current
    /// `create` call.
    pub(crate) unsafe fn new(ctx: *mut sys::VrlCtx, api: *const sys::VrlHostApi) -> Self {
        Self {
            raw: RawCtx { ctx, api },
        }
    }

    /// An elaboration-time parameter given with `#()`.
    pub fn param(&mut self, name: &str) -> Result<Value> {
        let mut out = sys::VrlValue::unit();
        let rc = unsafe {
            (self.raw.api().param_get)(self.raw.ctx, sys::VrlStr::from_str(name), &mut out)
        };
        if rc != 0 {
            bail!("no parameter named `{name}`");
        }
        Ok(unsafe { Value::from_vrl(&out) })
    }

    pub fn input(&mut self, name: &str) -> Result<InputPort> {
        match self.raw.port(name, sys::VRL_DIR_INPUT) {
            Some((idx, width)) => {
                let direct = self.raw.port_direct(idx);
                Ok(InputPort {
                    idx,
                    width,
                    words_ptr: direct.words.cast_const(),
                })
            }
            None => bail!("no input port named `{name}`"),
        }
    }

    pub fn output(&mut self, name: &str) -> Result<OutputPort> {
        match self.raw.port(name, sys::VRL_DIR_OUTPUT) {
            Some((idx, width)) => {
                let direct = self.raw.port_direct(idx);
                Ok(OutputPort {
                    idx,
                    width,
                    words_ptr: direct.words,
                    mask_ptr: direct.mask_xz,
                    dirty_ptr: direct.dirty,
                })
            }
            None => bail!("no output port named `{name}`"),
        }
    }

    /// A clock input port; the connection must be a clock.
    pub fn clock(&mut self, name: &str) -> Result<ClockPort> {
        match self.raw.port(name, sys::VRL_DIR_CLOCK) {
            Some((idx, _)) => Ok(ClockPort { idx }),
            None if self.raw.port(name, sys::VRL_DIR_INPUT).is_some() => {
                bail!("port `{name}` is connected, but not with a clock")
            }
            None => bail!("no clock port named `{name}`"),
        }
    }

    /// A reset input port; the connection must be a reset.
    pub fn reset(&mut self, name: &str) -> Result<ResetPort> {
        match self.raw.port(name, sys::VRL_DIR_RESET) {
            Some((idx, _)) => Ok(ResetPort { idx }),
            None if self.raw.port(name, sys::VRL_DIR_INPUT).is_some() => {
                bail!("port `{name}` is connected, but not with a reset")
            }
            None => bail!("no reset port named `{name}`"),
        }
    }

    /// Deterministic per-instance seed: `hash(test seed, instance path)`.
    pub fn seed(&mut self) -> u64 {
        unsafe { (self.raw.api().seed)(self.raw.ctx) }
    }

    /// Registers a component-internal signal for waveform dumping as
    /// `<instance>.<name>`. Only available here (the waveform header is
    /// finalized once all components are built). The component may ignore
    /// the error if tracing is optional for it.
    pub fn trace_var(&mut self, name: &str, width: u32) -> Result<TraceVar> {
        if !self.raw.has_trace_api() {
            bail!("host does not support trace variables");
        }
        let handle =
            unsafe { (self.raw.api().trace_var)(self.raw.ctx, sys::VrlStr::from_str(name), width) };
        if handle < 0 {
            bail!("cannot register trace variable `{name}`");
        }
        Ok(TraceVar { handle, width })
    }
}

/// Context passed to every hook after construction.
pub struct SimCtx {
    raw: RawCtx,
    four_state: bool,
}

impl SimCtx {
    /// # Safety
    /// `ctx` must be the live host pointer for the current hook call and
    /// `api` the table given to `create`.
    pub(crate) unsafe fn new(
        ctx: *mut sys::VrlCtx,
        api: *const sys::VrlHostApi,
        four_state: bool,
    ) -> Self {
        Self {
            raw: RawCtx { ctx, api },
            four_state,
        }
    }

    /// Reads the pre-edge value of an input port (clock and reset ports
    /// convert into 1-bit input ports). Under a four-state simulation the
    /// returned value carries an X/Z mask; otherwise the mask is all-zero.
    pub fn read(&mut self, port: impl Into<InputPort>) -> Value {
        let port = port.into();
        let n = words_for(port.width);
        let mut words: SmallVec<[u64; 2]> = SmallVec::from_elem(0, n);
        // Two-state simulation carries no X/Z, so skip the mask buffer and let
        // the host skip its copy (it tolerates a null mask pointer).
        if self.four_state {
            let mut mask_xz: SmallVec<[u64; 2]> = SmallVec::from_elem(0, n);
            unsafe {
                (self.raw.api().read_input)(
                    self.raw.ctx,
                    port.idx,
                    words.as_mut_ptr(),
                    mask_xz.as_mut_ptr(),
                )
            };
            Value::from_bits(words, mask_xz, port.width)
        } else {
            unsafe {
                (self.raw.api().read_input)(
                    self.raw.ctx,
                    port.idx,
                    words.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            };
            Value::from_bits(words, SmallVec::new(), port.width)
        }
    }

    /// Writes an output port. The value commits with FFs (NBA semantics),
    /// zero-extended or truncated to the port width. Its X/Z mask is driven
    /// too, so a component can drive X/Z under a four-state simulation.
    ///
    /// Panics if the value is not bits (the panic surfaces as a test
    /// failure).
    pub fn write(&mut self, port: OutputPort, value: impl Into<Value>) {
        let value = value.into();
        let words = value
            .to_port_words(port.width)
            .expect("output port write requires a bits value");
        // Two-state simulation ignores X/Z, so skip the mask conversion and
        // pass a null mask pointer (the host tolerates it).
        if self.four_state {
            let mask_xz = value
                .to_port_mask_xz(port.width)
                .expect("output port write requires a bits value");
            unsafe {
                (self.raw.api().write_output)(
                    self.raw.ctx,
                    port.idx,
                    words.as_ptr(),
                    mask_xz.as_ptr(),
                )
            };
        } else {
            unsafe {
                (self.raw.api().write_output)(
                    self.raw.ctx,
                    port.idx,
                    words.as_ptr(),
                    std::ptr::null(),
                )
            };
        }
    }

    /// Reads a scalar (≤ 64-bit) input port straight into a `u64`, skipping
    /// the `Value`/mask marshalling of [`Self::read`]. X/Z is dropped, so this
    /// is for the common two-state scalar case (or a four-state port whose X/Z
    /// the component ignores).
    pub fn read_u64(&mut self, port: impl Into<InputPort>) -> u64 {
        let port = port.into();
        debug_assert!(port.width <= 64, "read_u64 on a port wider than 64 bits");
        if !port.words_ptr.is_null() {
            return unsafe { *port.words_ptr };
        }
        let mut word: u64 = 0;
        unsafe {
            (self.raw.api().read_input)(self.raw.ctx, port.idx, &mut word, std::ptr::null_mut())
        };
        word
    }

    /// Writes a scalar (≤ 64-bit) output port straight from a `u64`, skipping
    /// the `Value`/mask marshalling of [`Self::write`]. The value is masked to
    /// the port width; X/Z is not driven.
    pub fn write_u64(&mut self, port: OutputPort, value: u64) {
        debug_assert!(port.width <= 64, "write_u64 on a port wider than 64 bits");
        let word = if port.width >= 64 {
            value
        } else {
            value & (u64::MAX >> (64 - port.width))
        };
        if !port.words_ptr.is_null() {
            unsafe {
                *port.words_ptr = word;
                *port.mask_ptr = 0; // a scalar write drives no X/Z
                *port.dirty_ptr = 1;
            }
            return;
        }
        unsafe { (self.raw.api().write_output)(self.raw.ctx, port.idx, &word, std::ptr::null()) };
    }

    /// Reads a port's payload words (LSB-first) straight into `out`, which must
    /// hold at least `words_for(width)` words, skipping the `Value`/`BigUint`
    /// marshalling of [`Self::read`]. This is the wide analogue of
    /// [`Self::read_u64`]: X/Z is dropped, for the common two-state case.
    pub fn read_words(&mut self, port: impl Into<InputPort>, out: &mut [u64]) {
        let port = port.into();
        let n = words_for(port.width);
        debug_assert!(out.len() >= n, "read_words buffer smaller than the port");
        if !port.words_ptr.is_null() {
            unsafe { std::ptr::copy_nonoverlapping(port.words_ptr, out.as_mut_ptr(), n) };
            return;
        }
        unsafe {
            (self.raw.api().read_input)(
                self.raw.ctx,
                port.idx,
                out.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
    }

    /// Writes a port's payload from `words` (LSB-first, at least
    /// `words_for(width)` words), skipping the `Value`/`BigUint` marshalling of
    /// [`Self::write`]. The top word is masked to the port width; X/Z is not
    /// driven. This is the wide analogue of [`Self::write_u64`].
    pub fn write_words(&mut self, port: OutputPort, words: &[u64]) {
        let n = words_for(port.width);
        debug_assert!(words.len() >= n, "write_words buffer smaller than the port");
        let top_bits = port.width as usize - 64 * (n - 1);
        let top_mask = if top_bits >= 64 {
            u64::MAX
        } else {
            (1u64 << top_bits) - 1
        };
        if !port.words_ptr.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(words.as_ptr(), port.words_ptr, n);
                *port.words_ptr.add(n - 1) &= top_mask;
                std::ptr::write_bytes(port.mask_ptr, 0, n);
                *port.dirty_ptr = 1;
            }
            return;
        }
        // Fallback masks a copy, since the caller's slice is shared.
        let mut buf: SmallVec<[u64; 4]> = SmallVec::from_slice(&words[..n]);
        buf[n - 1] &= top_mask;
        unsafe {
            (self.raw.api().write_output)(self.raw.ctx, port.idx, buf.as_ptr(), std::ptr::null())
        };
    }

    /// True when the simulation is four-state, so input X/Z masks are
    /// meaningful and driven X/Z propagates. A component gates its X/Z checks
    /// on this: under a two-state simulation there is nothing to check.
    pub fn is_4state(&mut self) -> bool {
        unsafe { (self.raw.api().is_4state)(self.raw.ctx) != 0 }
    }

    /// Marks the test as failed. The host records every message, tagged
    /// with instance name and cycle, and stops the clock loop at the end of
    /// the current cycle.
    pub fn fail(&mut self, msg: impl AsRef<str>) {
        self.raw.fail(msg.as_ref());
    }

    /// Requests normal termination at the end of the current cycle.
    pub fn finish(&mut self) {
        self.raw.finish();
    }

    /// Logs into the per-test output buffer (direct `println!` from a
    /// component interleaves across parallel tests; use this instead).
    pub fn log(&mut self, msg: impl AsRef<str>) {
        self.raw.log(msg.as_ref());
    }

    /// Updates a trace variable registered with
    /// [`BuildCtx::trace_var`](crate::BuildCtx::trace_var). The value is
    /// zero-extended or truncated to the registered width.
    ///
    /// Panics if the value is not bits (the panic surfaces as a test
    /// failure).
    pub fn trace(&mut self, var: TraceVar, value: impl Into<Value>) {
        let words = value
            .into()
            .to_port_words(var.width)
            .expect("trace write requires a bits value");
        unsafe { (self.raw.api().trace_write)(self.raw.ctx, var.handle, words.as_ptr()) };
    }

    /// Cycle count of the fired clock.
    pub fn cycle(&self) -> u64 {
        unsafe { (self.raw.api().cycle)(self.raw.ctx) }
    }

    /// Current simulation time.
    pub fn time(&self) -> u64 {
        unsafe { (self.raw.api().sim_time)(self.raw.ctx) }
    }

    /// The clock port that fired the current hook.
    pub fn clock(&self) -> InputPort {
        let idx = unsafe { (self.raw.api().fired_clock)(self.raw.ctx) };
        let width = unsafe { (self.raw.api().port_width)(self.raw.ctx, idx) };
        InputPort {
            idx,
            width,
            words_ptr: self.raw.port_direct(idx).words.cast_const(),
        }
    }

    /// Whether the given clock port fired the current hook. Distinguishes
    /// the source on components connected to several clocks.
    pub fn fired(&self, clock: ClockPort) -> bool {
        let idx = unsafe { (self.raw.api().fired_clock)(self.raw.ctx) };
        idx == clock.idx
    }

    /// Opens a file for reading through the host. Host-mediated file I/O
    /// is the portable path (it maps onto the wasm transport); direct
    /// `std::fs` use makes a component native-only.
    pub fn open(&mut self, path: &str) -> Result<CtxFile<'_>> {
        self.file(path, sys::VRL_FILE_READ)
    }

    /// Creates (truncates) a file for writing through the host.
    pub fn create(&mut self, path: &str) -> Result<CtxFile<'_>> {
        self.file(path, sys::VRL_FILE_CREATE)
    }

    /// Opens a file for appending through the host.
    pub fn append(&mut self, path: &str) -> Result<CtxFile<'_>> {
        self.file(path, sys::VRL_FILE_APPEND)
    }

    fn file(&mut self, path: &str, mode: u32) -> Result<CtxFile<'_>> {
        let handle =
            unsafe { (self.raw.api().file_open)(self.raw.ctx, sys::VrlStr::from_str(path), mode) };
        if handle < 0 {
            bail!("cannot open `{path}`");
        }
        Ok(CtxFile {
            ctx: self.raw.ctx,
            api: self.raw.api,
            _marker: std::marker::PhantomData,
            handle,
        })
    }
}

/// A host-mediated file, closed on drop. Borrows the [`SimCtx`] it came
/// from, so it cannot outlive the current hook.
pub struct CtxFile<'a> {
    ctx: *mut sys::VrlCtx,
    api: *const sys::VrlHostApi,
    _marker: std::marker::PhantomData<&'a mut SimCtx>,
    handle: i32,
}

impl CtxFile<'_> {
    fn api(&self) -> &sys::VrlHostApi {
        unsafe { &*self.api }
    }
}

impl std::io::Read for CtxFile<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n =
            unsafe { (self.api().file_read)(self.ctx, self.handle, buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            Err(std::io::Error::other("host file read failed"))
        } else {
            Ok(n as usize)
        }
    }
}

impl std::io::Write for CtxFile<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = unsafe { (self.api().file_write)(self.ctx, self.handle, buf.as_ptr(), buf.len()) };
        if n < 0 {
            Err(std::io::Error::other("host file write failed"))
        } else {
            Ok(n as usize)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::io::Seek for CtxFile<'_> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let (offset, whence) = match pos {
            std::io::SeekFrom::Start(n) => (n as i64, sys::VRL_SEEK_SET),
            std::io::SeekFrom::Current(n) => (n, sys::VRL_SEEK_CUR),
            std::io::SeekFrom::End(n) => (n, sys::VRL_SEEK_END),
        };
        let n = unsafe { (self.api().file_seek)(self.ctx, self.handle, offset, whence) };
        if n < 0 {
            Err(std::io::Error::other("host file seek failed"))
        } else {
            Ok(n as u64)
        }
    }
}

impl Drop for CtxFile<'_> {
    fn drop(&mut self) {
        unsafe { (self.api().file_close)(self.ctx, self.handle) };
    }
}
