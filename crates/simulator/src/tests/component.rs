//! Harness-level tests for the user-defined component ABI: guest side built
//! with `veryl-component`, registered in-process (no dlopen), driven through
//! the host vtable wrapper.

use crate::component::host::{ExternalInstance, HostContext, HostValue, PortDir};
use crate::component::loader::{ComponentError, lookup_component, register_static_component};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use veryl_component::{
    BuildCtx, Component, ComponentKind, InputPort, OutputPort, Result, SimCtx, Value, bail, sys,
    veryl_component_export,
};

struct Counter {
    out: OutputPort,
    inc: u64,
    count: u64,
}

impl Component for Counter {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> Result<Self> {
        Ok(Self {
            out: ctx.output("out")?,
            inc: ctx.param("INC")?.as_u64()?,
            count: 0,
        })
    }

    fn on_init(&mut self, ctx: &mut SimCtx) -> Result<()> {
        ctx.write(self.out, 0u64);
        Ok(())
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
        self.count += self.inc;
        ctx.write(self.out, self.count);
        Ok(())
    }
}

struct EchoWide {
    win: InputPort,
    wout: OutputPort,
}

impl Component for EchoWide {
    fn new(ctx: &mut BuildCtx) -> Result<Self> {
        Ok(Self {
            win: ctx.input("win")?,
            wout: ctx.output("wout")?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
        let value = ctx.read(self.win);
        let Value::Bits { words, width, .. } = value else {
            bail!("expected bits");
        };
        let inverted: smallvec::SmallVec<[u64; 2]> = words.iter().map(|w| !w).collect();
        ctx.write(
            self.wout,
            Value::from_bits(inverted, Default::default(), width),
        );
        Ok(())
    }
}

struct FailFinish {
    input: InputPort,
}

impl Component for FailFinish {
    fn new(ctx: &mut BuildCtx) -> Result<Self> {
        Ok(Self {
            input: ctx.input("in")?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
        match ctx.read(self.input).as_u64()? {
            1 => ctx.fail("requested failure"),
            2 => ctx.finish(),
            _ => {}
        }
        Ok(())
    }
}

struct MethodOnly;

impl Component for MethodOnly {
    const KIND: ComponentKind = ComponentKind::MethodOnly;

    fn new(_ctx: &mut BuildCtx) -> Result<Self> {
        Ok(Self)
    }

    fn method(&mut self, name: &str, args: &[Value], ctx: &mut SimCtx) -> Result<Value> {
        match name {
            "add" => Ok(Value::from_u64(args[0].as_u64()? + args[1].as_u64()?, 64)),
            "greet" => {
                ctx.log(format!("hello {}", args[0].as_str()?));
                Ok(Value::unit())
            }
            _ => bail!("unknown method: {name}"),
        }
    }
}

struct Observer {
    clk: InputPort,
    seed: u64,
}

impl Component for Observer {
    fn new(ctx: &mut BuildCtx) -> Result<Self> {
        Ok(Self {
            clk: ctx.input("clk")?,
            seed: ctx.seed(),
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
        let fired = ctx.clock();
        ctx.log(format!(
            "cycle={} time={} seed={} fired={:?} same={}",
            ctx.cycle(),
            ctx.time(),
            self.seed,
            fired,
            fired == self.clk,
        ));
        Ok(())
    }
}

struct MissingPort;

impl Component for MissingPort {
    fn new(ctx: &mut BuildCtx) -> Result<Self> {
        ctx.input("no_such_port")?;
        Ok(Self)
    }
}

struct Panicker;

impl Component for Panicker {
    fn new(_ctx: &mut BuildCtx) -> Result<Self> {
        Ok(Self)
    }

    fn on_clock(&mut self, _ctx: &mut SimCtx) -> Result<()> {
        panic!("boom");
    }
}

static DROPPED: AtomicBool = AtomicBool::new(false);

struct Dropper;

impl Component for Dropper {
    fn new(_ctx: &mut BuildCtx) -> Result<Self> {
        Ok(Self)
    }
}

impl Drop for Dropper {
    fn drop(&mut self) {
        DROPPED.store(true, Ordering::SeqCst);
    }
}

veryl_component_export!(
    "counter" => Counter,
    "echo_wide" => EchoWide,
    "fail_finish" => FailFinish,
    "method_only" => MethodOnly,
    "observer" => Observer,
    "missing_port" => MissingPort,
    "panicker" => Panicker,
    "dropper" => Dropper,
);

static REGISTER: LazyLock<()> = LazyLock::new(|| {
    for (name, vtable) in VERYL_COMPONENT_TABLE {
        register_static_component(name, vtable);
    }
});

fn vtable(name: &str) -> &'static sys::VrlComponentVTable {
    LazyLock::force(&REGISTER);
    lookup_component(None, name).unwrap()
}

#[test]
fn counter_on_clock_roundtrip() {
    let vtable = vtable("counter");
    assert_eq!(vtable.kind, sys::VRL_KIND_CLOCKED);

    let mut host = HostContext::new();
    host.add_port("out", PortDir::Output, 16);
    host.add_param("INC", HostValue::bits_u64(3, 32));

    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();
    assert_eq!(instance.kind(), sys::VRL_KIND_CLOCKED);

    assert_eq!(instance.on_init(&mut host), 0);
    assert_eq!(host.output_u64("out"), 0);

    for i in 1..=3u64 {
        host.clear_output_dirty();
        assert_eq!(instance.on_clock(&mut host), 0);
        assert!(host.output_dirty("out"));
        assert_eq!(host.output_u64("out"), 3 * i);
    }
    assert!(!host.failed());
    assert!(!host.finish_requested());
    assert_eq!(instance.on_finish(&mut host), 0);
}

#[test]
fn output_width_is_masked() {
    let vtable = vtable("counter");
    let mut host = HostContext::new();
    host.add_port("out", PortDir::Output, 4);
    host.add_param("INC", HostValue::bits_u64(0x25, 32));

    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();
    assert_eq!(instance.on_clock(&mut host), 0);
    assert_eq!(host.output_u64("out"), 0x5);
}

#[test]
fn wide_ports_cross_the_boundary() {
    let vtable = vtable("echo_wide");
    let mut host = HostContext::new();
    let win = host.add_port("win", PortDir::Input, 100);
    host.add_port("wout", PortDir::Output, 100);

    host.set_input(win, &[0, 0]);
    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();
    assert_eq!(instance.on_clock(&mut host), 0);
    // Inverted zeros, truncated to 100 bits by the port write.
    assert_eq!(host.output_words("wout"), &[u64::MAX, (1 << 36) - 1]);

    host.set_input(win, &[0x00ff_00ff_00ff_00ff, 0xf]);
    assert_eq!(instance.on_clock(&mut host), 0);
    assert_eq!(
        host.output_words("wout"),
        &[!0x00ff_00ff_00ff_00ffu64, ((1 << 36) - 1) & !0xf]
    );
}

#[test]
fn fail_and_finish_flags() {
    let vtable = vtable("fail_finish");
    let mut host = HostContext::new();
    let input = host.add_port("in", PortDir::Input, 8);
    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();

    host.set_input(input, &[0]);
    assert_eq!(instance.on_clock(&mut host), 0);
    assert!(!host.failed());
    assert!(!host.finish_requested());

    host.set_input(input, &[2]);
    assert_eq!(instance.on_clock(&mut host), 0);
    assert!(host.finish_requested());
    assert!(!host.failed());

    host.set_input(input, &[1]);
    assert_eq!(instance.on_clock(&mut host), 0);
    assert_eq!(host.failures(), ["requested failure"]);
}

#[test]
fn method_calls() {
    let vtable = vtable("method_only");
    assert_eq!(vtable.kind, sys::VRL_KIND_METHOD_ONLY);
    let mut host = HostContext::new();
    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();

    let ret = instance
        .call_method(
            &mut host,
            "add",
            &[HostValue::bits_u64(3, 64), HostValue::bits_u64(4, 64)],
        )
        .unwrap();
    assert_eq!(ret.as_u64(), Some(7));

    let ret = instance
        .call_method(&mut host, "greet", &[HostValue::Str("world".to_string())])
        .unwrap();
    assert_eq!(ret, HostValue::Unit);
    assert_eq!(host.logs(), ["hello world"]);

    assert!(
        instance
            .call_method(&mut host, "no_such_method", &[])
            .is_none()
    );
    assert_eq!(host.failures(), ["unknown method: no_such_method"]);
}

#[test]
fn default_on_clock_is_an_error() {
    let vtable = vtable("method_only");
    let mut host = HostContext::new();
    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();

    assert_ne!(instance.on_clock(&mut host), 0);
    assert_eq!(host.failures(), ["on_clock not implemented"]);
}

#[test]
fn observer_reads_host_state() {
    let vtable = vtable("observer");
    let mut host = HostContext::new();
    let clk = host.add_port("clk", PortDir::Input, 1);
    host.cycle = 42;
    host.time = 420;
    host.seed = 7;
    host.fired_clock = clk;

    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();
    assert_eq!(instance.on_clock(&mut host), 0);
    assert_eq!(
        host.logs(),
        [format!(
            "cycle=42 time=420 seed=7 fired=InputPort {{ idx: {clk}, width: 1 }} same=true"
        )]
    );
}

#[test]
fn create_fails_on_missing_port() {
    let vtable = vtable("missing_port");
    let mut host = HostContext::new();
    let err = ExternalInstance::create(vtable, &mut host).unwrap_err();
    let ComponentError::CreateFailed { messages } = err else {
        panic!("expected CreateFailed, got {err}");
    };
    assert!(messages.contains("no input port named `no_such_port`"));
}

#[test]
fn create_fails_on_direction_mismatch() {
    // `missing_port` asks for an *input* named no_such_port; providing an
    // output with that name must not satisfy it.
    let vtable = vtable("missing_port");
    let mut host = HostContext::new();
    host.add_port("no_such_port", PortDir::Output, 1);
    let err = ExternalInstance::create(vtable, &mut host).unwrap_err();
    assert!(matches!(err, ComponentError::CreateFailed { .. }));
}

#[test]
fn create_fails_on_missing_param() {
    let vtable = vtable("counter");
    let mut host = HostContext::new();
    host.add_port("out", PortDir::Output, 16);
    let err = ExternalInstance::create(vtable, &mut host).unwrap_err();
    let ComponentError::CreateFailed { messages } = err else {
        panic!("expected CreateFailed, got {err}");
    };
    assert!(messages.contains("no parameter named `INC`"));
}

#[test]
fn panic_is_converted_to_fail() {
    let vtable = vtable("panicker");
    let mut host = HostContext::new();
    let mut instance = ExternalInstance::create(vtable, &mut host).unwrap();

    assert_ne!(instance.on_clock(&mut host), 0);
    assert_eq!(host.failures(), ["component panicked: boom"]);

    // The host (and the instance) survive; further hooks still work.
    assert_eq!(instance.on_finish(&mut host), 0);
}

#[test]
fn unknown_type_is_reported() {
    LazyLock::force(&REGISTER);
    let err = lookup_component(None, "no_such_component").unwrap_err();
    assert!(matches!(err, ComponentError::UnknownType { .. }));
}

#[test]
fn abi_version_mismatch_is_rejected() {
    static BAD_VTABLE: sys::VrlComponentVTable = {
        let mut vtable = veryl_component::export::vtable::<Counter>();
        vtable.abi_version = 9999;
        vtable
    };
    register_static_component("bad_abi", &BAD_VTABLE);
    let err = lookup_component(None, "bad_abi").unwrap_err();
    let ComponentError::AbiMismatch {
        found, expected, ..
    } = err
    else {
        panic!("expected AbiMismatch, got {err}");
    };
    assert_eq!(found, 9999);
    assert_eq!(expected, sys::VRL_COMPONENT_ABI_VERSION);
}

#[test]
fn destroy_drops_the_component() {
    let vtable = vtable("dropper");
    let mut host = HostContext::new();
    let instance = ExternalInstance::create(vtable, &mut host).unwrap();
    assert!(!DROPPED.load(Ordering::SeqCst));
    drop(instance);
    assert!(DROPPED.load(Ordering::SeqCst));
}

#[test]
fn lookup_symbol_resolves_names() {
    // Exercise the generated `veryl_component_lookup` entry point directly,
    // as the dlopen path would.
    let vtable = unsafe { veryl_component_lookup(sys::VrlStr::from_str("counter")) };
    assert!(!vtable.is_null());
    assert_eq!(
        unsafe { &*vtable }.abi_version,
        sys::VRL_COMPONENT_ABI_VERSION
    );
    let missing = unsafe { veryl_component_lookup(sys::VrlStr::from_str("nope")) };
    assert!(missing.is_null());
}
