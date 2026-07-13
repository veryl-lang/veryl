//! Standalone mock harness: unit-test a [`Component`](crate::Component)
//! without the Veryl simulator.
//!
//! ```ignore
//! use veryl_component::testing::*;
//!
//! let mut sim = MockSim::new()
//!     .param("XLEN", 32u64)
//!     .input("retire_valid", 1)
//!     .input("retire_pc", 32);
//! let mut c = sim.build::<RvIss>()?;
//! sim.set("retire_valid", 1u64);
//! sim.set("retire_pc", 0x8000_0000u64);
//! sim.clock(&mut c)?;
//! assert!(sim.failed());
//! ```
//!
//! Hooks are driven through the same `SimCtx`/`BuildCtx` API the simulator
//! uses, backed by an in-process implementation of the host services.
//! Unlike the real host, panics are not caught — they surface directly in
//! the unit test.

use crate::value::words_for;
use crate::{BuildCtx, Component, Result, SimCtx, Value, sys};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Input,
    Output,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Role {
    #[default]
    Data,
    Clock,
    Reset,
}

struct MockPort {
    name: String,
    dir: Dir,
    role: Role,
    width: u32,
    words: Vec<u64>,
    mask_xz: Vec<u64>,
    /// Set by the direct scalar write path so its pointer is valid; the mock
    /// reads output words directly, so the flag is otherwise unused.
    dirty: bool,
}

struct MockTraceVar {
    name: String,
    width: u32,
    words: Vec<u64>,
}

/// In-process stand-in for the simulator host. Owns port staging buffers,
/// parameters and the fail/finish/log state.
#[derive(Default)]
pub struct MockSim {
    ports: Vec<MockPort>,
    params: Vec<(String, Value)>,
    failures: Vec<String>,
    finish_requested: bool,
    logs: Vec<String>,
    files: Vec<Option<std::fs::File>>,
    trace_vars: Vec<MockTraceVar>,
    in_build: bool,
    four_state: bool,
    pub cycle: u64,
    pub time: u64,
    pub seed: u64,
    pub fired_clock: u32,
}

impl MockSim {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares an elaboration parameter (`#()` equivalent).
    pub fn param(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.params.push((name.to_string(), value.into()));
        self
    }

    /// Reports the simulation as four-state, so [`SimCtx::is_4state`] returns
    /// true and input X/Z masks are delivered.
    pub fn four_state(mut self, on: bool) -> Self {
        self.four_state = on;
        self
    }

    /// Declares an input port connection of the given bit width.
    pub fn input(mut self, name: &str, width: u32) -> Self {
        self.add_port(name, Dir::Input, Role::Data, width);
        self
    }

    /// Declares an output port connection of the given bit width.
    pub fn output(mut self, name: &str, width: u32) -> Self {
        self.add_port(name, Dir::Output, Role::Data, width);
        self
    }

    /// Declares a clock input connection (resolvable by
    /// [`BuildCtx::clock`]).
    pub fn clock_port(mut self, name: &str) -> Self {
        self.add_port(name, Dir::Input, Role::Clock, 1);
        self
    }

    /// Declares a reset input connection (resolvable by
    /// [`BuildCtx::reset`]).
    pub fn reset_port(mut self, name: &str) -> Self {
        self.add_port(name, Dir::Input, Role::Reset, 1);
        self
    }

    fn add_port(&mut self, name: &str, dir: Dir, role: Role, width: u32) {
        self.ports.push(MockPort {
            name: name.to_string(),
            dir,
            role,
            width,
            words: vec![0; words_for(width)],
            mask_xz: vec![0; words_for(width)],
            dirty: false,
        });
    }

    fn port_position(&self, name: &str, dir: Dir) -> Option<usize> {
        self.ports
            .iter()
            .position(|p| p.dir == dir && p.name == name)
    }

    /// Constructs the component, resolving its ports and parameters against
    /// the declared connections.
    pub fn build<T: Component>(&mut self) -> Result<T> {
        let mut ctx = unsafe { BuildCtx::new(self.as_ctx(), &MOCK_API) };
        self.in_build = true;
        let ret = T::new(&mut ctx);
        self.in_build = false;
        ret
    }

    /// Sets an input port's pre-edge value.
    pub fn set(&mut self, name: &str, value: impl Into<Value>) {
        let idx = self
            .port_position(name, Dir::Input)
            .unwrap_or_else(|| panic!("no input port named `{name}`"));
        let width = self.ports[idx].width;
        let value = value.into();
        let words = value
            .to_port_words(width)
            .expect("input value must be bits");
        let mask_xz = value
            .to_port_mask_xz(width)
            .expect("input value must be bits");
        self.ports[idx].words.copy_from_slice(&words);
        self.ports[idx].mask_xz.copy_from_slice(&mask_xz);
    }

    /// Reads an output port's last written value (with its X/Z mask).
    pub fn get(&mut self, name: &str) -> Value {
        let idx = self
            .port_position(name, Dir::Output)
            .unwrap_or_else(|| panic!("no output port named `{name}`"));
        let port = &self.ports[idx];
        Value::from_bits(
            port.words.as_slice().into(),
            port.mask_xz.as_slice().into(),
            port.width,
        )
    }

    /// Drives one `on_clock` hook (increments `cycle`).
    pub fn clock<T: Component>(&mut self, component: &mut T) -> Result<()> {
        self.cycle += 1;
        let mut ctx = unsafe { SimCtx::new(self.as_ctx(), &MOCK_API, self.four_state) };
        component.on_clock(&mut ctx)
    }

    /// Selects which port `SimCtx::clock`/`SimCtx::fired` report for
    /// subsequent hooks.
    pub fn set_fired_clock(&mut self, name: &str) {
        self.fired_clock =
            self.port_position(name, Dir::Input)
                .unwrap_or_else(|| panic!("no input port named `{name}`")) as u32;
    }

    /// Drives one `on_reset` hook.
    pub fn reset<T: Component>(&mut self, component: &mut T) -> Result<()> {
        let mut ctx = unsafe { SimCtx::new(self.as_ctx(), &MOCK_API, self.four_state) };
        component.on_reset(&mut ctx)
    }

    /// Drives the `on_init` hook.
    pub fn init<T: Component>(&mut self, component: &mut T) -> Result<()> {
        let mut ctx = unsafe { SimCtx::new(self.as_ctx(), &MOCK_API, self.four_state) };
        component.on_init(&mut ctx)
    }

    /// Drives the `on_finish` hook.
    pub fn finish<T: Component>(&mut self, component: &mut T) -> Result<()> {
        let mut ctx = unsafe { SimCtx::new(self.as_ctx(), &MOCK_API, self.four_state) };
        component.on_finish(&mut ctx)
    }

    /// Calls a zero-time method.
    pub fn call<T: Component>(
        &mut self,
        component: &mut T,
        name: &str,
        args: &[Value],
    ) -> Result<Value> {
        let mut ctx = unsafe { SimCtx::new(self.as_ctx(), &MOCK_API, self.four_state) };
        component.method(name, args, &mut ctx)
    }

    /// True when the component reported a failure via `ctx.fail`.
    pub fn failed(&self) -> bool {
        !self.failures.is_empty()
    }

    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    pub fn finish_requested(&self) -> bool {
        self.finish_requested
    }

    pub fn logs(&self) -> &[String] {
        &self.logs
    }

    /// Reads a trace variable's last written value.
    pub fn trace_value(&self, name: &str) -> Value {
        let var = self
            .trace_vars
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no trace variable named `{name}`"));
        Value::from_bits(var.words.as_slice().into(), Default::default(), var.width)
    }

    fn as_ctx(&mut self) -> *mut sys::VrlCtx {
        self as *mut MockSim as *mut sys::VrlCtx
    }
}

/// # Safety
/// `ctx` must be the pointer produced by `MockSim::as_ctx` for a context
/// mutably borrowed by the current hook call.
unsafe fn mock<'a>(ctx: *mut sys::VrlCtx) -> &'a mut MockSim {
    unsafe { &mut *(ctx as *mut MockSim) }
}

extern "C" fn mock_port_index(ctx: *mut sys::VrlCtx, name: sys::VrlStr, dir: u32) -> i32 {
    let sim = unsafe { mock(ctx) };
    let name = unsafe { name.as_str() };
    let (dir, role) = match dir {
        sys::VRL_DIR_INPUT => (Dir::Input, None),
        sys::VRL_DIR_OUTPUT => (Dir::Output, None),
        sys::VRL_DIR_CLOCK => (Dir::Input, Some(Role::Clock)),
        sys::VRL_DIR_RESET => (Dir::Input, Some(Role::Reset)),
        _ => return -1,
    };
    sim.ports
        .iter()
        .position(|p| p.dir == dir && p.name == name && role.is_none_or(|role| p.role == role))
        .map(|i| i as i32)
        .unwrap_or(-1)
}

extern "C" fn mock_port_width(ctx: *mut sys::VrlCtx, idx: u32) -> u32 {
    let sim = unsafe { mock(ctx) };
    sim.ports.get(idx as usize).map(|p| p.width).unwrap_or(0)
}

extern "C" fn mock_read_input(ctx: *mut sys::VrlCtx, idx: u32, words: *mut u64, mask_xz: *mut u64) {
    let sim = unsafe { mock(ctx) };
    let Some(port) = sim.ports.get(idx as usize) else {
        return;
    };
    let n = port.words.len();
    unsafe {
        std::ptr::copy_nonoverlapping(port.words.as_ptr(), words, n);
        if !mask_xz.is_null() {
            // Mirror the real host: X/Z only crosses under a four-state run.
            if sim.four_state {
                std::ptr::copy_nonoverlapping(port.mask_xz.as_ptr(), mask_xz, n);
            } else {
                std::ptr::write_bytes(mask_xz, 0, n);
            }
        }
    }
}

extern "C" fn mock_write_output(
    ctx: *mut sys::VrlCtx,
    idx: u32,
    words: *const u64,
    mask_xz: *const u64,
) {
    let sim = unsafe { mock(ctx) };
    let Some(port) = sim.ports.get_mut(idx as usize) else {
        return;
    };
    if port.dir != Dir::Output {
        return;
    }
    let n = port.words.len();
    unsafe {
        std::ptr::copy_nonoverlapping(words, port.words.as_mut_ptr(), n);
        if mask_xz.is_null() {
            port.mask_xz.iter_mut().for_each(|m| *m = 0);
        } else {
            std::ptr::copy_nonoverlapping(mask_xz, port.mask_xz.as_mut_ptr(), n);
        }
    }
}

extern "C" fn mock_port_direct(
    ctx: *mut sys::VrlCtx,
    idx: u32,
    out: *mut sys::VrlPortDirect,
) -> u32 {
    let sim = unsafe { mock(ctx) };
    let Some(port) = sim.ports.get_mut(idx as usize) else {
        return 0;
    };
    let dirty = if port.dir == Dir::Output {
        &mut port.dirty as *mut bool as *mut u8
    } else {
        std::ptr::null_mut()
    };
    unsafe {
        *out = sys::VrlPortDirect {
            words: port.words.as_mut_ptr(),
            mask_xz: port.mask_xz.as_mut_ptr(),
            dirty,
        }
    };
    1
}

extern "C" fn mock_is_4state(ctx: *mut sys::VrlCtx) -> u32 {
    u32::from(unsafe { mock(ctx) }.four_state)
}

extern "C" fn mock_param_get(
    ctx: *mut sys::VrlCtx,
    name: sys::VrlStr,
    out: *mut sys::VrlValue,
) -> i32 {
    let sim = unsafe { mock(ctx) };
    let name = unsafe { name.as_str() };
    match sim.params.iter().find(|(n, _)| n == name) {
        Some((
            _,
            Value::Bits {
                words,
                mask_xz,
                width,
            },
        )) => {
            unsafe {
                *out = sys::VrlValue {
                    kind: sys::VRL_VALUE_BITS,
                    width: *width,
                    words: words.as_ptr(),
                    nwords: words.len(),
                    mask_xz: mask_xz.as_ptr(),
                    str_: sys::VrlStr::from_str(""),
                };
            }
            0
        }
        Some((_, Value::Str(s))) => {
            unsafe {
                *out = sys::VrlValue {
                    kind: sys::VRL_VALUE_STRING,
                    width: 0,
                    words: std::ptr::null(),
                    nwords: 0,
                    mask_xz: std::ptr::null(),
                    str_: sys::VrlStr::from_str(s),
                };
            }
            0
        }
        _ => -1,
    }
}

extern "C" fn mock_fail(ctx: *mut sys::VrlCtx, msg: sys::VrlStr) {
    let sim = unsafe { mock(ctx) };
    let msg = unsafe { msg.as_str() };
    sim.failures.push(msg.to_string());
}

extern "C" fn mock_finish(ctx: *mut sys::VrlCtx) {
    unsafe { mock(ctx) }.finish_requested = true;
}

extern "C" fn mock_log(ctx: *mut sys::VrlCtx, msg: sys::VrlStr) {
    let sim = unsafe { mock(ctx) };
    let msg = unsafe { msg.as_str() };
    sim.logs.push(msg.to_string());
}

extern "C" fn mock_cycle(ctx: *mut sys::VrlCtx) -> u64 {
    unsafe { mock(ctx) }.cycle
}

extern "C" fn mock_sim_time(ctx: *mut sys::VrlCtx) -> u64 {
    unsafe { mock(ctx) }.time
}

extern "C" fn mock_seed(ctx: *mut sys::VrlCtx) -> u64 {
    unsafe { mock(ctx) }.seed
}

extern "C" fn mock_fired_clock(ctx: *mut sys::VrlCtx) -> u32 {
    unsafe { mock(ctx) }.fired_clock
}

extern "C" fn mock_file_open(ctx: *mut sys::VrlCtx, path: sys::VrlStr, mode: u32) -> i32 {
    let sim = unsafe { mock(ctx) };
    let path = unsafe { path.as_str() };
    let file = match mode {
        sys::VRL_FILE_READ => std::fs::File::open(path),
        sys::VRL_FILE_CREATE => std::fs::File::create(path),
        sys::VRL_FILE_APPEND => std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path),
        _ => return -1,
    };
    match file {
        Ok(file) => {
            sim.files.push(Some(file));
            (sim.files.len() - 1) as i32
        }
        Err(_) => -1,
    }
}

fn mock_file(sim: &mut MockSim, handle: i32) -> Option<&mut std::fs::File> {
    sim.files.get_mut(handle as usize)?.as_mut()
}

extern "C" fn mock_file_read(ctx: *mut sys::VrlCtx, handle: i32, buf: *mut u8, len: usize) -> i64 {
    use std::io::Read;
    let sim = unsafe { mock(ctx) };
    let Some(file) = mock_file(sim, handle) else {
        return -1;
    };
    let buf = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    match file.read(buf) {
        Ok(n) => n as i64,
        Err(_) => -1,
    }
}

extern "C" fn mock_file_write(
    ctx: *mut sys::VrlCtx,
    handle: i32,
    buf: *const u8,
    len: usize,
) -> i64 {
    use std::io::Write;
    let sim = unsafe { mock(ctx) };
    let Some(file) = mock_file(sim, handle) else {
        return -1;
    };
    let buf = unsafe { std::slice::from_raw_parts(buf, len) };
    match file.write(buf) {
        Ok(n) => n as i64,
        Err(_) => -1,
    }
}

extern "C" fn mock_file_seek(ctx: *mut sys::VrlCtx, handle: i32, pos: i64, whence: u32) -> i64 {
    use std::io::{Seek, SeekFrom};
    let sim = unsafe { mock(ctx) };
    let Some(file) = mock_file(sim, handle) else {
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

extern "C" fn mock_file_close(ctx: *mut sys::VrlCtx, handle: i32) {
    let sim = unsafe { mock(ctx) };
    if let Some(slot) = sim.files.get_mut(handle as usize) {
        *slot = None;
    }
}

extern "C" fn mock_trace_var(ctx: *mut sys::VrlCtx, name: sys::VrlStr, width: u32) -> i32 {
    let sim = unsafe { mock(ctx) };
    let name = unsafe { name.as_str() };
    if !sim.in_build || width == 0 || sim.trace_vars.iter().any(|t| t.name == name) {
        return -1;
    }
    sim.trace_vars.push(MockTraceVar {
        name: name.to_string(),
        width,
        words: vec![0; words_for(width)],
    });
    (sim.trace_vars.len() - 1) as i32
}

extern "C" fn mock_trace_write(ctx: *mut sys::VrlCtx, handle: i32, words: *const u64) {
    let sim = unsafe { mock(ctx) };
    let Some(var) = sim.trace_vars.get_mut(handle as usize) else {
        return;
    };
    let n = var.words.len();
    unsafe {
        std::ptr::copy_nonoverlapping(words, var.words.as_mut_ptr(), n);
    }
}

static MOCK_API: sys::VrlHostApi = sys::VrlHostApi {
    size: size_of::<sys::VrlHostApi>(),
    port_index: mock_port_index,
    port_width: mock_port_width,
    read_input: mock_read_input,
    write_output: mock_write_output,
    param_get: mock_param_get,
    fail: mock_fail,
    finish: mock_finish,
    log: mock_log,
    cycle: mock_cycle,
    sim_time: mock_sim_time,
    seed: mock_seed,
    fired_clock: mock_fired_clock,
    file_open: mock_file_open,
    file_read: mock_file_read,
    file_write: mock_file_write,
    file_seek: mock_file_seek,
    file_close: mock_file_close,
    trace_var: mock_trace_var,
    trace_write: mock_trace_write,
    is_4state: mock_is_4state,
    port_direct: mock_port_direct,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClockPort, ComponentKind, InputPort, OutputPort, ResetPort, bail};

    fn unreachable_panic() -> crate::Error {
        unreachable!("expected build failure")
    }

    struct Adder {
        a: InputPort,
        b: InputPort,
        sum: OutputPort,
        limit: u64,
    }

    impl Component for Adder {
        const KIND: ComponentKind = ComponentKind::Clocked;

        fn new(ctx: &mut BuildCtx) -> Result<Self> {
            Ok(Self {
                a: ctx.input("a")?,
                b: ctx.input("b")?,
                sum: ctx.output("sum")?,
                limit: ctx.param("LIMIT")?.as_u64()?,
            })
        }

        fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
            let a = ctx.read(self.a).as_u64()?;
            let b = ctx.read(self.b).as_u64()?;
            let sum = a + b;
            if sum > self.limit {
                ctx.fail(format!("sum {sum} exceeds limit {}", self.limit));
            }
            ctx.write(self.sum, sum);
            Ok(())
        }

        fn method(&mut self, name: &str, _args: &[Value], ctx: &mut SimCtx) -> Result<Value> {
            match name {
                "cycle" => Ok(Value::from_u64(ctx.cycle(), 64)),
                _ => bail!("unknown method: {name}"),
            }
        }
    }

    #[test]
    fn mock_sim_drives_a_component() {
        let mut sim = MockSim::new()
            .param("LIMIT", 100u64)
            .input("a", 8)
            .input("b", 8)
            .output("sum", 9);
        let mut c = sim.build::<Adder>().unwrap();

        sim.set("a", 3u64);
        sim.set("b", 4u64);
        sim.clock(&mut c).unwrap();
        assert_eq!(sim.get("sum").as_u64().unwrap(), 7);
        assert!(!sim.failed());

        sim.set("a", 90u64);
        sim.set("b", 20u64);
        sim.clock(&mut c).unwrap();
        assert!(sim.failed());
        assert_eq!(sim.failures(), ["sum 110 exceeds limit 100"]);

        let cycle = sim.call(&mut c, "cycle", &[]).unwrap();
        assert_eq!(cycle.as_u64().unwrap(), 2);
    }

    #[test]
    fn mock_sim_reports_missing_ports() {
        let mut sim = MockSim::new().param("LIMIT", 1u64).input("a", 8);
        let err = match sim.build::<Adder>() {
            Err(e) => e,
            Ok(_) => unreachable_panic(),
        };
        assert!(err.to_string().contains("no input port named `b`"));
    }

    /// Records whether an input carried X, but only under a four-state run.
    struct XChecker {
        d: InputPort,
        is_4state: bool,
        saw_x: bool,
    }

    impl Component for XChecker {
        const KIND: ComponentKind = ComponentKind::Clocked;

        fn new(ctx: &mut BuildCtx) -> Result<Self> {
            Ok(Self {
                d: ctx.input("d")?,
                is_4state: false,
                saw_x: false,
            })
        }

        fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
            self.is_4state = ctx.is_4state();
            if self.is_4state && ctx.read(self.d).has_x() {
                self.saw_x = true;
            }
            Ok(())
        }
    }

    #[test]
    fn component_sees_input_x_only_under_four_state() {
        // Bit 0 driven X (mask_xz set, payload clear).
        let x_in = || Value::from_bits([0].into_iter().collect(), [1].into_iter().collect(), 4);

        let mut sim = MockSim::new().input("d", 4).four_state(true);
        let mut c = sim.build::<XChecker>().unwrap();
        sim.set("d", x_in());
        sim.clock(&mut c).unwrap();
        assert!(c.is_4state);
        assert!(c.saw_x);

        // Same driver without four-state: the mask is dropped and the check
        // is inert.
        let mut sim = MockSim::new().input("d", 4);
        let mut c = sim.build::<XChecker>().unwrap();
        sim.set("d", x_in());
        sim.clock(&mut c).unwrap();
        assert!(!c.is_4state);
        assert!(!c.saw_x);
    }

    struct Tracer {
        state: crate::TraceVar,
    }

    impl Component for Tracer {
        const KIND: ComponentKind = ComponentKind::Clocked;

        fn new(ctx: &mut BuildCtx) -> Result<Self> {
            ctx.input("clk")?;
            Ok(Self {
                state: ctx.trace_var("state", 8)?,
            })
        }

        fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
            let value = ctx.cycle() * 3;
            ctx.trace(self.state, value);
            Ok(())
        }
    }

    #[test]
    fn mock_sim_trace_var_roundtrip() {
        let mut sim = MockSim::new().input("clk", 1);
        let mut c = sim.build::<Tracer>().unwrap();

        sim.clock(&mut c).unwrap();
        assert_eq!(sim.trace_value("state").as_u64().unwrap(), 3);
        sim.clock(&mut c).unwrap();
        assert_eq!(sim.trace_value("state").as_u64().unwrap(), 6);
    }

    /// Component under manifest test.
    #[derive(veryl_component::Component)]
    #[component(kind = clocked, requires(file))]
    struct Manifested {
        /// Sampling clock.
        clk: ClockPort,
        d: InputPort,
        #[port(name = "bus_awvalid")]
        awvalid: OutputPort,
        #[param]
        limit: u64,
        loaded: Option<String>,
    }

    #[veryl_component::component_impl]
    impl Manifested {
        fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
            let _ = ctx.read(self.clk);
            let _ = ctx.read(self.d);
            Ok(())
        }

        /// Load an ELF file.
        fn load(&mut self, ctx: &mut SimCtx, path: &str) -> Result<()> {
            ctx.write(self.awvalid, true);
            self.loaded = Some(path.to_string());
            Ok(())
        }

        fn get(&mut self, _ctx: &mut SimCtx) -> Result<u64> {
            Ok(self.limit)
        }
    }

    #[test]
    fn derive_emits_manifest_json() {
        let json = Manifested::manifest().expect("manifest missing");
        let expected = r#"{"doc":"Component under manifest test.","kind":"clocked","ports":[{"name":"clk","dir":"input","role":"clock","doc":"Sampling clock."},{"name":"d","dir":"input"},{"name":"bus_awvalid","dir":"output"}],"params":[{"name":"limit","type":"u64"}],"requires":["file"],"groups":[],"methods":[{"name":"load","args":[{"name":"path","type":"str"}],"doc":"Load an ELF file."},{"name":"get","args":[],"ret":"u64"}]}"#;
        assert_eq!(json, expected);
    }

    #[test]
    fn derived_component_builds_and_dispatches() {
        let mut sim = MockSim::new()
            .param("limit", 5u64)
            .clock_port("clk")
            .input("d", 8)
            .output("bus_awvalid", 1);
        let mut c = sim.build::<Manifested>().unwrap();
        assert_eq!(c.limit, 5);
        assert_eq!(c.loaded, None);

        let ret = sim.call(&mut c, "get", &[]).unwrap();
        assert_eq!(ret.as_u64().unwrap(), 5);

        let ret = sim.call(&mut c, "load", &[Value::from("x.elf")]).unwrap();
        assert_eq!(ret, Value::Unit);
        assert_eq!(c.loaded.as_deref(), Some("x.elf"));
        assert_eq!(sim.get("bus_awvalid").as_u64().unwrap(), 1);

        let err = sim.call(&mut c, "load", &[]).unwrap_err();
        assert!(err.to_string().contains("expects 1 argument(s)"));
        let err = sim.call(&mut c, "nope", &[]).unwrap_err();
        assert!(err.to_string().contains("unknown method: nope"));
    }

    #[test]
    fn component_without_derive_has_no_manifest() {
        assert!(Adder::manifest().is_none());
    }

    /// Component with role-typed clock/reset ports.
    #[derive(veryl_component::Component, Debug)]
    struct Edged {
        clk: ClockPort,
        #[port(name = "rst_n")]
        rst: ResetPort,
        d: InputPort,
    }

    #[veryl_component::component_impl]
    impl Edged {
        fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
            if ctx.fired(self.clk) && !ctx.read(self.rst).as_bool() {
                let _ = ctx.read(self.d);
            }
            Ok(())
        }
    }

    #[test]
    fn clock_reset_ports_in_manifest_and_build() {
        let json = Edged::manifest().expect("manifest missing");
        let expected = r#"{"doc":"Component with role-typed clock/reset ports.","kind":"clocked","ports":[{"name":"clk","dir":"input","role":"clock"},{"name":"rst_n","dir":"input","role":"reset"},{"name":"d","dir":"input"}],"params":[],"requires":[],"groups":[],"methods":[]}"#;
        assert_eq!(json, expected);

        let mut sim = MockSim::new()
            .clock_port("clk")
            .reset_port("rst_n")
            .input("d", 8);
        let mut c = sim.build::<Edged>().unwrap();
        sim.set_fired_clock("clk");
        sim.clock(&mut c).unwrap();
        assert!(!sim.failed());
    }

    /// Port set of the monitored bus.
    #[derive(veryl_component::VerylInterface)]
    #[interface(path = "$std::axi4_if", modport = "monitor")]
    struct AxiMon {
        /// Write-address valid.
        awvalid: InputPort,
        r#in: InputPort,
    }

    /// Component embedding an interface port set.
    #[derive(veryl_component::Component)]
    #[component(kind = clocked)]
    struct Bound {
        clk: ClockPort,
        #[interface]
        axi: AxiMon,
        special: InputPort,
    }

    #[veryl_component::component_impl]
    impl Bound {
        fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
            let _ = self.clk;
            let _ = ctx.read(self.axi.awvalid);
            let _ = ctx.read(self.axi.r#in);
            let _ = ctx.read(self.special);
            Ok(())
        }

        #[ret_width(axi.DATA_WIDTH_BYTES * 8)]
        fn snoop(&mut self, ctx: &mut SimCtx) -> Result<Value> {
            Ok(ctx.read(self.axi.awvalid))
        }
    }

    #[test]
    fn interface_field_manifest_and_resolution() {
        let json = Bound::manifest().expect("manifest missing");
        // The group is assembled from the VerylInterface constants by
        // const concatenation; members live under it.
        assert!(
            json.contains(
                r#""groups":[{"name":"axi","interface":"$std::axi4_if","modport":"monitor","members":[{"member":"awvalid","dir":"input","doc":"Write-address valid."},{"member":"in","dir":"input"}]}]"#
            ),
            "{json}"
        );
        // Loose and role ports stay in the flat ports list.
        assert!(
            json.contains(r#""ports":[{"name":"clk","dir":"input","role":"clock"},{"name":"special","dir":"input"}]"#),
            "{json}"
        );
        // A group-qualified return width is a dotted parameter name.
        assert!(
            json.contains(
                r#"{"name":"snoop","args":[],"ret":"value","ret_width":{"op":"*","lhs":"axi.DATA_WIDTH_BYTES","rhs":8}}"#
            ),
            "{json}"
        );

        // Members resolve under `<group>.<member>` connection names.
        let mut sim = MockSim::new()
            .clock_port("clk")
            .input("axi.awvalid", 1)
            .input("axi.in", 1)
            .input("special", 8);
        let mut c = sim.build::<Bound>().unwrap();
        sim.set("axi.awvalid", 1u64);
        sim.clock(&mut c).unwrap();
        assert!(!sim.failed());
        assert_eq!(sim.call(&mut c, "snoop", &[]).unwrap().as_u64().unwrap(), 1);
    }

    #[test]
    fn clock_port_rejects_a_plain_input_connection() {
        let mut sim = MockSim::new()
            .input("clk", 1)
            .reset_port("rst_n")
            .input("d", 8);
        let err = sim.build::<Edged>().unwrap_err();
        assert!(err.to_string().contains("not with a clock"));
    }

    #[test]
    fn reset_port_reports_a_missing_connection() {
        let mut sim = MockSim::new().clock_port("clk").input("d", 8);
        let err = sim.build::<Edged>().unwrap_err();
        assert!(err.to_string().contains("no reset port named `rst_n`"));
    }

    struct ScalarMirror {
        d: InputPort,
        q: OutputPort,
    }

    impl Component for ScalarMirror {
        const KIND: ComponentKind = ComponentKind::Clocked;

        fn new(ctx: &mut BuildCtx) -> Result<Self> {
            ctx.clock("clk")?;
            Ok(Self {
                d: ctx.input("d")?,
                q: ctx.output("q")?,
            })
        }

        fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
            let v = ctx.read_u64(self.d);
            ctx.write_u64(self.q, v.wrapping_add(0x100));
            Ok(())
        }
    }

    #[test]
    fn scalar_u64_fast_path_reads_and_masks_to_width() {
        let mut sim = MockSim::new()
            .clock_port("clk")
            .input("d", 8)
            .output("q", 8);
        let mut c = sim.build::<ScalarMirror>().unwrap();
        sim.set("d", Value::from_u64(0x2a, 8));
        sim.clock(&mut c).unwrap();
        // read_u64 saw 0x2a; write_u64(0x12a) masks back to the 8-bit width.
        assert_eq!(sim.get("q").as_u64().unwrap(), 0x2a);
    }

    struct WideMirror {
        d: InputPort,
        q: OutputPort,
        buf: Vec<u64>,
    }

    impl Component for WideMirror {
        const KIND: ComponentKind = ComponentKind::Clocked;

        fn new(ctx: &mut BuildCtx) -> Result<Self> {
            ctx.clock("clk")?;
            let d = ctx.input("d")?;
            let q = ctx.output("q")?;
            Ok(Self {
                d,
                q,
                buf: vec![0; d.words()],
            })
        }

        fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
            ctx.read_words(self.d, &mut self.buf);
            ctx.write_words(self.q, &self.buf);
            Ok(())
        }
    }

    #[test]
    fn wide_words_fast_path_roundtrips_and_masks_top_word() {
        let mut sim = MockSim::new()
            .clock_port("clk")
            .input("d", 100)
            .output("q", 100);
        let mut c = sim.build::<WideMirror>().unwrap();
        // 100-bit value across two words; the high word carries bits above 100
        // that write_words must mask off (100 - 64 = 36 kept bits).
        sim.set(
            "d",
            Value::from_bits(
                [0xdead_beef_cafe_f00d, 0xffff_ffff_ffff_ffff]
                    .into_iter()
                    .collect(),
                Default::default(),
                100,
            ),
        );
        sim.clock(&mut c).unwrap();
        let expected = Value::from_bits(
            [0xdead_beef_cafe_f00d, 0x0000_000f_ffff_ffff]
                .into_iter()
                .collect(),
            Default::default(),
            100,
        );
        assert_eq!(sim.get("q"), expected);
    }

    #[derive(veryl_component::Component, Debug)]
    #[component(kind = method_only)]
    struct Conversions {
        #[param]
        offset: i32,
        #[param]
        enabled: bool,
    }

    #[veryl_component::component_impl]
    impl Conversions {
        fn signed(&mut self, _ctx: &mut SimCtx, v: i32) -> Result<i64> {
            Ok(v as i64)
        }

        fn narrow(&mut self, _ctx: &mut SimCtx, v: i8) -> Result<i32> {
            Ok(v as i32)
        }

        fn flag(&mut self, _ctx: &mut SimCtx, v: bool) -> Result<bool> {
            Ok(v)
        }
    }

    fn conversions_sim() -> MockSim {
        MockSim::new()
            .param("offset", Value::from_u64(0xFFFF_FFFF, 32))
            .param("enabled", Value::from_u64(1, 1))
    }

    #[test]
    fn signed_params_sign_extend_by_width() {
        let mut sim = conversions_sim();
        let c = sim.build::<Conversions>().unwrap();
        assert_eq!(c.offset, -1);
        assert!(c.enabled);
    }

    #[test]
    fn bool_params_require_bits() {
        let mut sim = MockSim::new()
            .param("offset", Value::from_u64(0, 32))
            .param("enabled", "yes");
        let err = sim.build::<Conversions>().unwrap_err();
        assert!(err.to_string().contains("parameter `enabled`"));
    }

    #[test]
    fn signed_arguments_sign_extend_by_width() {
        let mut sim = conversions_sim();
        let mut c = sim.build::<Conversions>().unwrap();

        let ret = sim
            .call(&mut c, "signed", &[Value::from_u64(0xFFFF_FFFF, 32)])
            .unwrap();
        assert_eq!(ret, Value::from(-1i64));

        let ret = sim
            .call(&mut c, "narrow", &[Value::from_u64(0xF, 4)])
            .unwrap();
        assert_eq!(ret, Value::from(-1i32));

        // The same bit pattern at 64 bits is a large positive number and
        // must not fit in i32.
        let err = sim
            .call(&mut c, "signed", &[Value::from_u64(0xFFFF_FFFF, 64)])
            .unwrap_err();
        assert!(err.to_string().contains("argument `v`"));
    }

    #[test]
    fn bool_arguments_require_bits() {
        let mut sim = conversions_sim();
        let mut c = sim.build::<Conversions>().unwrap();

        let ret = sim.call(&mut c, "flag", &[Value::from_u64(1, 1)]).unwrap();
        assert_eq!(ret, Value::from(true));

        let err = sim.call(&mut c, "flag", &[Value::from("x")]).unwrap_err();
        assert!(err.to_string().contains("argument `v`"));
    }
}
