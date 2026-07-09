//! End-to-end tests for user-defined components: Veryl source with
//! `inst x: $comp::<name>` analyzed, converted to simulator IR, and
//! driven through the real step/commit machinery. Components register
//! through the static (dlopen-free) path.

use super::*;
use crate::component::loader::register_static_component;
use std::sync::LazyLock;
use veryl_component::{
    BuildCtx, ClockPort, Component, ComponentKind, InputPort, OutputPort, ResetPort,
    Result as CompResult, SimCtx, bail, export, sys,
};

/// Mirrors its input into its output like `always_ff { q = d; }`:
/// the equality with an RTL FF exercises pre-edge reads + NBA commit.
struct Mirror {
    d: InputPort,
    q: OutputPort,
}

impl Component for Mirror {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            d: ctx.input("d")?,
            q: ctx.output("q")?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let value = ctx.read(self.d);
        ctx.write(self.q, value);
        Ok(())
    }
}

/// Writes a fixed initial value in `on_init`; has no clock.
struct Init {
    out: OutputPort,
}

impl Component for Init {
    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        Ok(Self {
            out: ctx.output("out")?,
        })
    }

    fn on_init(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        ctx.write(self.out, 0xA5u64);
        Ok(())
    }

    fn on_clock(&mut self, _ctx: &mut SimCtx) -> CompResult<()> {
        Ok(())
    }
}

/// Fails the test when its trigger input is high on a clock edge.
struct Failer {
    trigger: InputPort,
}

impl Component for Failer {
    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            trigger: ctx.input("trigger")?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        if ctx.read(self.trigger).as_bool() {
            ctx.fail("triggered");
        }
        Ok(())
    }
}

/// Requests normal termination after `STOP` clock edges.
struct Finisher {
    stop: u64,
}

impl Component for Finisher {
    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            stop: ctx.param("STOP")?.as_u64()?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        if ctx.cycle() >= self.stop {
            ctx.finish();
        }
        Ok(())
    }
}

/// ISS-style checker: observes a value (typically a DUT-internal signal via
/// a hierarchical reference) and finishes when the expected value appears;
/// fails if it never does within `LIMIT` cycles.
struct HierChecker {
    val: InputPort,
    expect: u64,
    limit: u64,
}

impl Component for HierChecker {
    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            val: ctx.input("val")?,
            expect: ctx.param("EXPECT")?.as_u64()?,
            limit: ctx.param("LIMIT")?.as_u64()?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let val = ctx.read(self.val).as_u64()?;
        if val == self.expect {
            ctx.finish();
        } else if ctx.cycle() >= self.limit {
            ctx.fail(format!(
                "expected {:#x}, still {:#x} after {} cycles",
                self.expect,
                val,
                ctx.cycle()
            ));
        }
        Ok(())
    }
}

/// Method-only component; instantiating it with `inst` must be
/// rejected at load time.
struct MethodOnly;

impl Component for MethodOnly {
    const KIND: ComponentKind = ComponentKind::MethodOnly;

    fn new(_ctx: &mut BuildCtx) -> CompResult<Self> {
        Ok(Self)
    }

    fn method(
        &mut self,
        name: &str,
        _args: &[veryl_component::Value],
        _ctx: &mut SimCtx,
    ) -> CompResult<veryl_component::Value> {
        bail!("unknown method: {name}")
    }
}

/// Method-only golden model exercising numeric and string
/// method arguments, host-mediated file I/O, and per-call failure.
struct Golden {
    stored: u64,
}

impl Component for Golden {
    const KIND: ComponentKind = ComponentKind::MethodOnly;

    fn new(_ctx: &mut BuildCtx) -> CompResult<Self> {
        Ok(Self { stored: 0 })
    }

    fn method(
        &mut self,
        name: &str,
        args: &[veryl_component::Value],
        ctx: &mut SimCtx,
    ) -> CompResult<veryl_component::Value> {
        use std::io::{Read, Write};
        match name {
            "set" => {
                self.stored = args[0].as_u64()?;
                Ok(veryl_component::Value::unit())
            }
            "check" => {
                let value = args[0].as_u64()?;
                if value != self.stored {
                    ctx.fail(format!("expected {}, got {value}", self.stored));
                }
                Ok(veryl_component::Value::unit())
            }
            "get" => Ok(veryl_component::Value::from_u64(self.stored, 64)),
            "bump" => {
                self.stored += 1;
                Ok(veryl_component::Value::from_u64(self.stored, 64))
            }
            "wide" => Ok(veryl_component::Value::from_bits(
                [self.stored, 1].into_iter().collect(),
                Default::default(),
                128,
            )),
            "save" => {
                let mut file = ctx.create(args[0].as_str()?)?;
                write!(file, "{}", self.stored)?;
                Ok(veryl_component::Value::unit())
            }
            "load" => {
                let mut file = ctx.open(args[0].as_str()?)?;
                let mut text = String::new();
                file.read_to_string(&mut text)?;
                self.stored = text.trim().parse()?;
                Ok(veryl_component::Value::unit())
            }
            _ => bail!("unknown method: {name}"),
        }
    }
}

/// Modport-connected master BFM: watches `bus.ready`, drives `bus.valid`;
/// leaves `bus.data` untouched (unused interface members are tolerated).
struct MpMaster {
    ready: InputPort,
    valid: OutputPort,
}

impl Component for MpMaster {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            ready: ctx.input("bus.ready")?,
            valid: ctx.output("bus.valid")?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let ready = ctx.read(self.ready);
        ctx.write(self.valid, ready);
        Ok(())
    }
}

/// Clocked entirely through a modport: fires on the `bus.clk` member and
/// drives `bus.q` with the cycle count.
struct MpClocked {
    q: OutputPort,
}

impl Component for MpClocked {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("bus.clk")?;
        Ok(Self {
            q: ctx.output("bus.q")?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let cycle = ctx.cycle();
        ctx.write(self.q, cycle);
        Ok(())
    }
}

/// Wide-value methods with declared widths (fixed, parameter-dependent
/// and arithmetic), for the static width checks.
#[derive(veryl_component::Component)]
struct Wide {
    #[param(name = "WIDTH")]
    width: u64,
    stored: Option<veryl_component::Value>,
}

#[veryl_component::component_impl]
impl Wide {
    fn put(&mut self, _ctx: &mut SimCtx, v: &veryl_component::Value) -> CompResult<()> {
        self.stored = Some(v.clone());
        Ok(())
    }

    #[ret_width(WIDTH)]
    fn get(&mut self, _ctx: &mut SimCtx) -> CompResult<veryl_component::Value> {
        Ok(self
            .stored
            .clone()
            .unwrap_or(veryl_component::Value::from_bits(
                [1].into_iter().collect(),
                Default::default(),
                self.width as u32,
            )))
    }

    #[ret_width(WIDTH * 2)]
    fn doubled(&mut self, _ctx: &mut SimCtx) -> CompResult<veryl_component::Value> {
        Ok(veryl_component::Value::from_bits(
            [3, 0, 0].into_iter().collect(),
            Default::default(),
            self.width as u32 * 2,
        ))
    }

    /// Deliberately violates its own declaration.
    #[ret_width(32)]
    fn lying(&mut self, _ctx: &mut SimCtx) -> CompResult<veryl_component::Value> {
        Ok(veryl_component::Value::from_bits(
            [1].into_iter().collect(),
            Default::default(),
            64,
        ))
    }
}

/// Port set of the monitored wide bus.
#[derive(veryl_component::VerylInterface)]
#[interface(path = "WideIf", modport = "mon")]
struct WideMon {
    data: InputPort,
}

/// Interface-bound wide method: its declared return width follows the
/// `WIDTH` constant of the interface the `bus` group is connected to.
#[derive(veryl_component::Component)]
#[component(kind = clocked)]
struct WideBus {
    clk: ClockPort,
    #[interface]
    bus: WideMon,
}

#[veryl_component::component_impl]
impl WideBus {
    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let _ = (self.clk, ctx.read(self.bus.data));
        Ok(())
    }

    #[ret_width(bus.WIDTH)]
    fn snoop(&mut self, ctx: &mut SimCtx) -> CompResult<veryl_component::Value> {
        Ok(ctx.read(self.bus.data))
    }
}

/// Two groups of the same interface type: the port set is defined once and
/// embedded twice.
#[derive(veryl_component::Component)]
#[component(kind = clocked)]
struct DualBus {
    clk: ClockPort,
    #[interface]
    bus0: WideMon,
    #[interface]
    bus1: WideMon,
}

#[veryl_component::component_impl]
impl DualBus {
    fn on_clock(&mut self, _ctx: &mut SimCtx) -> CompResult<()> {
        let _ = self.clk;
        Ok(())
    }

    #[ret_width(bus0.WIDTH)]
    fn snoop0(&mut self, ctx: &mut SimCtx) -> CompResult<veryl_component::Value> {
        Ok(ctx.read(self.bus0.data))
    }

    #[ret_width(bus1.WIDTH)]
    fn snoop1(&mut self, ctx: &mut SimCtx) -> CompResult<veryl_component::Value> {
        Ok(ctx.read(self.bus1.data))
    }
}

/// Declares a member (`extra`) the demo interface does not provide, for
/// the member-missing diagnostics.
#[derive(veryl_component::VerylInterface)]
#[interface(path = "WideIf", modport = "mon")]
struct WideMonExtra {
    data: InputPort,
    extra: InputPort,
}

#[derive(veryl_component::Component)]
#[component(kind = clocked)]
struct ExtraBus {
    clk: ClockPort,
    #[interface]
    bus: WideMonExtra,
}

#[veryl_component::component_impl]
impl ExtraBus {
    fn on_clock(&mut self, _ctx: &mut SimCtx) -> CompResult<()> {
        let _ = self.clk;
        Ok(())
    }

    fn extra(&mut self, ctx: &mut SimCtx) -> CompResult<u64> {
        let _ = ctx.read(self.bus.data);
        ctx.read(self.bus.extra).as_u64()
    }
}

/// Keyword member spelled as a raw identifier; the manifest and the
/// connection use the unrawed name `in`.
#[derive(veryl_component::VerylInterface)]
#[interface(path = "RawIf", modport = "mon")]
struct RawMon {
    r#in: InputPort,
}

/// Raw identifiers at every declaration site: interface member, loose
/// port, parameter, and method (a Rust-only keyword, plain in Veryl).
#[derive(veryl_component::Component)]
#[component(kind = clocked)]
struct RawNames {
    clk: ClockPort,
    r#as: InputPort,
    #[param]
    r#pub: u64,
    #[interface]
    bus: RawMon,
}

#[veryl_component::component_impl]
impl RawNames {
    fn on_clock(&mut self, _ctx: &mut SimCtx) -> CompResult<()> {
        let _ = self.clk;
        Ok(())
    }

    fn r#match(&mut self, ctx: &mut SimCtx) -> CompResult<u64> {
        Ok(ctx.read(self.bus.r#in).as_u64()? + ctx.read(self.r#as).as_u64()? + self.r#pub)
    }
}

/// Mirror with a derived manifest, for load-time interface checks.
#[derive(veryl_component::Component)]
#[component(kind = clocked)]
struct Manifested {
    clk: ClockPort,
    d: InputPort,
    q: OutputPort,
    #[param(name = "STEP")]
    step: u64,
}

#[veryl_component::component_impl]
impl Manifested {
    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let _ = (self.clk, self.step);
        let value = ctx.read(self.d);
        ctx.write(self.q, value);
        Ok(())
    }

    fn poke(&mut self, _ctx: &mut SimCtx, _v: u64) -> CompResult<()> {
        Ok(())
    }

    fn peek(&mut self, _ctx: &mut SimCtx) -> CompResult<u64> {
        Ok(self.step)
    }
}

/// Mirror declaring role-typed clock/reset ports through the derive; its
/// manifest carries the port kinds for the interface checks.
#[derive(veryl_component::Component)]
#[component(kind = clocked)]
struct EdgePorts {
    clk: ClockPort,
    rst: ResetPort,
    d: InputPort,
    q: OutputPort,
}

#[veryl_component::component_impl]
impl EdgePorts {
    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        if !ctx.fired(self.clk) {
            ctx.fail("unexpected clock fired");
        }
        let _ = ctx.read(self.rst);
        let value = ctx.read(self.d);
        ctx.write(self.q, value);
        Ok(())
    }
}

/// Resolves its ports with the role-checked API but publishes no manifest;
/// wiring a plain signal to them must still fail at load time.
struct RolePorts {
    rst: ResetPort,
}

impl Component for RolePorts {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            rst: ctx.reset("rst")?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let _ = ctx.read(self.rst);
        Ok(())
    }
}

/// Resolves a clock connection as plain data; the host must reject the
/// wiring instead of silently never firing `on_clock`.
struct PlainClk {
    _clk: InputPort,
}

impl Component for PlainClk {
    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        Ok(Self {
            _clk: ctx.input("clk")?,
        })
    }

    fn on_clock(&mut self, _ctx: &mut SimCtx) -> CompResult<()> {
        Ok(())
    }
}

/// Registers a trace variable and writes the doubled cycle count on
/// every clock.
struct Tracer {
    state: veryl_component::TraceVar,
}

impl Component for Tracer {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            state: ctx.trace_var("state", 8)?,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let value = ctx.cycle() * 2;
        ctx.trace(self.state, value);
        Ok(())
    }
}

/// Requests a direction the modport forbids (`bus.ready` is an input
/// member, asked for as an output).
struct MpWrongDir {
    _ready: OutputPort,
}

impl Component for MpWrongDir {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            _ready: ctx.output("bus.ready")?,
        })
    }

    fn on_clock(&mut self, _ctx: &mut SimCtx) -> CompResult<()> {
        Ok(())
    }
}

/// Counts `on_reset`/`on_clock` firings into its output and fails when a
/// clock hook ever precedes the first reset hook.
struct ResetWatch {
    q: OutputPort,
    resets: u64,
    clocks: u64,
}

impl Component for ResetWatch {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        ctx.reset("rst")?;
        Ok(Self {
            q: ctx.output("q")?,
            resets: 0,
            clocks: 0,
        })
    }

    fn on_reset(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        if self.clocks != 0 {
            ctx.fail("on_clock fired before on_reset");
        }
        self.resets += 1;
        ctx.write(self.q, self.resets);
        Ok(())
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        if self.resets == 0 {
            ctx.fail("on_clock fired before any on_reset");
        }
        self.clocks += 1;
        ctx.write(self.q, 100 + self.clocks);
        Ok(())
    }
}

/// Clocked component that never resolves a clock port; instantiating it
/// without a firing clock connection must fail at load time.
struct NoClock {
    _d: InputPort,
}

impl Component for NoClock {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        Ok(Self {
            _d: ctx.input("d")?,
        })
    }

    fn on_clock(&mut self, _ctx: &mut SimCtx) -> CompResult<()> {
        Ok(())
    }
}

static MIRROR: sys::VrlComponentVTable = export::vtable::<Mirror>();
static INIT: sys::VrlComponentVTable = export::vtable::<Init>();
static FAILER: sys::VrlComponentVTable = export::vtable::<Failer>();
static FINISHER: sys::VrlComponentVTable = export::vtable::<Finisher>();
static HIER_CHECKER: sys::VrlComponentVTable = export::vtable::<HierChecker>();
static METHOD_ONLY: sys::VrlComponentVTable = export::vtable::<MethodOnly>();
static MP_MASTER: sys::VrlComponentVTable = export::vtable::<MpMaster>();
static MP_CLOCKED: sys::VrlComponentVTable = export::vtable::<MpClocked>();
static TRACER: sys::VrlComponentVTable = export::vtable::<Tracer>();
static MANIFESTED: sys::VrlComponentVTable = export::vtable::<Manifested>();
static WIDE: sys::VrlComponentVTable = export::vtable::<Wide>();
static WIDE_BUS: sys::VrlComponentVTable = export::vtable::<WideBus>();
static DUAL_BUS: sys::VrlComponentVTable = export::vtable::<DualBus>();
static EXTRA_BUS: sys::VrlComponentVTable = export::vtable::<ExtraBus>();
static RAW_NAMES: sys::VrlComponentVTable = export::vtable::<RawNames>();
static MP_WRONGDIR: sys::VrlComponentVTable = export::vtable::<MpWrongDir>();
static GOLDEN: sys::VrlComponentVTable = export::vtable::<Golden>();
static RESET_WATCH: sys::VrlComponentVTable = export::vtable::<ResetWatch>();
static NO_CLOCK: sys::VrlComponentVTable = export::vtable::<NoClock>();
static EDGE_PORTS: sys::VrlComponentVTable = export::vtable::<EdgePorts>();
static ROLE_PORTS: sys::VrlComponentVTable = export::vtable::<RolePorts>();
static PLAIN_CLK: sys::VrlComponentVTable = export::vtable::<PlainClk>();

const COMPONENTS: &[&str] = &[
    "mirror",
    "init",
    "failer",
    "finisher",
    "hier_checker",
    "method_only",
    "unregistered",
    "golden",
    "gap_param",
    "mp_master",
    "mp_clocked",
    "mp_wrongdir",
    "tracer",
    "reset_watch",
    "no_clock",
    "edge",
    "edge_rt",
    "role",
    "plain_clk",
    "manifested",
    "manifested_rt",
    "svc_manifested",
    "svc_allunit",
    "wide",
    "wide_bus",
    "wide_bus_rt",
    "dual_bus",
    "extra_bus",
    "raw_names",
    // wasm transport e2e fixtures; only looked up when a test routes them
    // to a wasm library via `Config::component_libraries`.
    "fixture_mirror",
    "fixture_echo",
    "fixture_panicker",
    "fixture_param_probe",
];

fn register_manifested(name: &str, vt: &'static sys::VrlComponentVTable, manifest: &str) {
    register_static_component(name, vt);
    crate::component::loader::register_static_manifest(name, manifest);
}

static REGISTER: LazyLock<()> = LazyLock::new(|| {
    register_static_component("mirror", &MIRROR);
    register_static_component("init", &INIT);
    register_static_component("failer", &FAILER);
    register_static_component("finisher", &FINISHER);
    register_static_component("hier_checker", &HIER_CHECKER);
    register_static_component("method_only", &METHOD_ONLY);
    register_static_component("golden", &GOLDEN);
    register_static_component("mp_master", &MP_MASTER);
    register_static_component("mp_clocked", &MP_CLOCKED);
    register_static_component("tracer", &TRACER);
    register_static_component("reset_watch", &RESET_WATCH);
    register_static_component("no_clock", &NO_CLOCK);
    register_static_component("manifested", &MANIFESTED);
    register_static_component("wide", &WIDE);
    // Manifested components register vtable and runtime manifest together:
    // group-member ports are wired through the manifest's (group, member)
    // record, so omitting the manifest would silently degrade the wiring.
    register_manifested("wide_bus", &WIDE_BUS, &WideBus::manifest().unwrap());
    register_manifested("dual_bus", &DUAL_BUS, &DualBus::manifest().unwrap());
    register_manifested("extra_bus", &EXTRA_BUS, &ExtraBus::manifest().unwrap());
    register_manifested("raw_names", &RAW_NAMES, &RawNames::manifest().unwrap());
    crate::component::loader::register_static_manifest(
        "manifested",
        &Manifested::manifest().unwrap(),
    );
    // Load-time-only twins: their manifest is visible to the runtime but
    // not to the analyzer, exercising the load-time checks in isolation.
    register_manifested(
        "manifested_rt",
        &MANIFESTED,
        &Manifested::manifest().unwrap(),
    );
    register_manifested("wide_bus_rt", &WIDE_BUS, &WideBus::manifest().unwrap());
    register_manifested("edge", &EDGE_PORTS, &EdgePorts::manifest().unwrap());
    register_manifested("edge_rt", &EDGE_PORTS, &EdgePorts::manifest().unwrap());
    register_static_component("role", &ROLE_PORTS);
    register_static_component("plain_clk", &PLAIN_CLK);
    register_static_component("mp_wrongdir", &MP_WRONGDIR);
    // "unregistered" is declared in metadata but intentionally not
    // registered, to exercise the unknown-type load error.
    // A dependency-provided component resolves under the composite
    // `<project>::<name>` key.
    register_static_component("dep_vip::mirror", &MIRROR);
});

/// Components provided by the simulated dependency project `dep_vip`.
const DEP_COMPONENTS: &[&str] = &["mirror"];

/// Registers analysis-time manifests, mirroring the sidecar loading done
/// by `Analyzer::new` in the real flow.
fn insert_test_manifests() {
    let insert = |name: &str, json: &str| {
        veryl_analyzer::component_manifest_table::insert(
            veryl_parser::resource_table::insert_str(name),
            veryl_metadata::ComponentManifest::parse(json).unwrap(),
        );
    };
    insert("manifested", &Manifested::manifest().unwrap());
    insert("wide", &Wide::manifest().unwrap());
    insert("wide_bus", &WideBus::manifest().unwrap());
    insert("dual_bus", &DualBus::manifest().unwrap());
    insert("extra_bus", &ExtraBus::manifest().unwrap());
    insert("raw_names", &RawNames::manifest().unwrap());
    insert("edge", &EdgePorts::manifest().unwrap());
    // Synthetic kind-only manifest for the declaration-form check.
    insert("svc_manifested", r#"{"kind":"method_only"}"#);
    // Every method returns unit: a value-position call must still be rejected.
    insert(
        "svc_allunit",
        r#"{"kind":"method_only","methods":[{"name":"poke","args":[]}]}"#,
    );
}

/// Analyze with every `COMPONENTS` name declared in the metadata (the
/// real injection path through `Analyzer::new`).
#[track_caller]
fn analyze_component_top(code: &str, config: &Config, top: &str) -> Result<Ir, SimulatorError> {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    veryl_analyzer::tb_component::insert_external_components(COMPONENTS);
    veryl_analyzer::tb_component::insert_dependency_components("dep_vip", DEP_COMPONENTS);
    insert_test_manifests();
    let mut context = Context::default();

    let mut errors = vec![];
    let mut ir = air::Ir::default();
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));

    let errors: Vec<_> = errors
        .into_iter()
        .filter(|x| {
            !matches!(
                x,
                AnalyzerError::UnusedVariable { .. } | AnalyzerError::UnassignVariable { .. }
            )
        })
        .collect();
    assert!(errors.is_empty(), "analyzer errors: {errors:?}");

    let top = veryl_parser::resource_table::insert_str(top);
    crate::ir::build_ir(&ir, top, config)
}

/// Rendered (Display) text of every diagnostic, joined; used to assert on
/// message content that lives in the error's `Display`, not its `Debug`.
fn errors_text(errors: &[AnalyzerError]) -> String {
    errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[track_caller]
fn analyze_component_errors(code: &str) -> Vec<AnalyzerError> {
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    veryl_analyzer::tb_component::insert_external_components(COMPONENTS);
    veryl_analyzer::tb_component::insert_dependency_components("dep_vip", DEP_COMPONENTS);
    insert_test_manifests();
    let mut context = Context::default();

    let mut errors = vec![];
    let mut ir = air::Ir::default();
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));
    errors
}

/// Build, init components, and run the `#[test]` module's initial block.
#[track_caller]
fn run_component_test(code: &str, top: &str) -> Vec<(Config, TestResult, Simulator)> {
    run_component_test_configured(code, top, |_| {})
}

/// Like `run_component_test`, letting the caller adjust each `Config`
/// (e.g. pointing component names at a wasm library).
#[track_caller]
fn run_component_test_configured(
    code: &str,
    top: &str,
    configure: impl Fn(&mut Config),
) -> Vec<(Config, TestResult, Simulator)> {
    LazyLock::force(&REGISTER);
    let mut ret = vec![];
    for mut config in Config::all() {
        configure(&mut config);
        let ir = analyze_component_top(code, &config, top)
            .unwrap_or_else(|x| panic!("build failed for {config:?}: {x:?}"));
        let mut sim = Simulator::new(ir, None);

        if let Err(err) = sim.init_components(0, top) {
            ret.push((config, TestResult::Fail(err.to_string()), sim));
            continue;
        }

        let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
        let clock_periods = build_clock_periods(&sim.ir.event_statements);
        let stmts = sim.ir.event_statements.get(&Event::Initial).unwrap();
        let tb_stmts = convert_initial_to_testbench(stmts, &event_map, &clock_periods, 3);

        let result = run_testbench(&mut sim, &tb_stmts);
        ret.push((config, result, sim));
    }
    ret
}

#[track_caller]
fn assert_all_pass(results: &[(Config, TestResult, Simulator)]) {
    for (config, result, _) in results {
        assert_eq!(*result, TestResult::Pass, "failed for {config:?}");
    }
}

#[test]
fn component_mirrors_rtl_ff_semantics() {
    // The component's on_clock must observe pre-edge values and commit
    // outputs like an NBA write: `q_comp` equals the RTL FF `q_rtl` at
    // every observation point.
    let code = r#"
    module MirrorDut (
        clk: input clock,
        rst: input reset,
        cnt: output logic<8>,
        q_rtl: output logic<8>,
    ) {
        always_ff {
            if_reset {
                cnt   = 0;
                q_rtl = 0;
            } else {
                cnt   += 1;
                q_rtl  = cnt;
            }
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var cnt: logic<8>;
        var q_rtl: logic<8>;
        var q_comp: logic<8>;

        inst dut: MirrorDut (clk, rst, cnt, q_rtl);

        inst mirror: $comp::mirror (
            clk,
            d: cnt,
            q: q_comp,
        );

        initial {
            rst.assert();
            clk.next();
            $assert(cnt == 1, "cnt after 1 edge");
            $assert(q_rtl == 0, "rtl saw pre-edge cnt");
            $assert(q_comp == 0, "component saw pre-edge cnt");
            clk.next();
            $assert(cnt == 2, "cnt after 2 edges");
            $assert(q_rtl == 1, "rtl mirrors previous cnt");
            $assert(q_comp == 1, "component mirrors previous cnt");
            clk.next();
            $assert(q_rtl == q_comp, "component == rtl");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn component_on_init_visible_before_initial() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var init_out: logic<8>;

        inst i: $comp::init ( out: init_out );

        initial {
            $assert(init_out == 8'ha5, "on_init value visible");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn component_fail_stops_clock_loop() {
    let code = r#"
    module Counter (
        clk: input clock,
        rst: input reset,
        cnt: output logic<16>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        var trigger: logic;
        var cnt: logic<16>;

        inst dut: Counter (clk, rst, cnt);

        inst f: $comp::failer ( clk, trigger );

        initial {
            rst.assert();
            trigger = 0;
            clk.next();
            trigger = 1;
            clk.next(100);
            $finish();
        }
    }
    "#;
    for (config, result, mut sim) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("[f] cycle 2: triggered"),
            "unexpected message for {config:?}: {msg}"
        );
        // The 100-cycle loop stopped at the failing edge.
        assert_eq!(sim.get_var("cnt").unwrap(), Value::new(2, 16, false));
    }
}

#[test]
fn component_finish_stops_clock_loop() {
    let code = r#"
    module Counter (
        clk: input clock,
        rst: input reset,
        cnt: output logic<16>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        var cnt: logic<16>;

        inst dut: Counter (clk, rst, cnt);

        inst fin: $comp::finisher #( STOP: 5 ) ( clk );

        initial {
            rst.assert();
            clk.next(1000);
            $finish();
        }
    }
    "#;
    for (config, result, mut sim) in run_component_test(code, "comp_test") {
        assert_eq!(result, TestResult::Pass, "failed for {config:?}");
        assert_eq!(sim.get_var("cnt").unwrap(), Value::new(5, 16, false));
    }
}

#[test]
fn component_reads_dut_internal_signal() {
    // ISS-style end-to-end: the checker's input is connected to a DUT
    // internal register through a hierarchical reference.
    let code = r#"
    module Sub (
        clk: input clock,
        rst: input reset,
        din: input logic<4>,
    ) {
        #[allow(unused_variable)]
        var internal_reg: logic<4>;
        always_ff {
            if_reset { internal_reg = 0; }
            else { internal_reg = din + 1; }
        }
    }

    module Top (
        clk: input clock,
        rst: input reset,
        din: input logic<4>,
    ) {
        inst u_sub: Sub (clk, rst, din);
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var din: logic<4>;

        inst dut: Top (clk, rst, din);

        inst chk: $comp::hier_checker #( EXPECT: 4, LIMIT: 10 ) (
            clk,
            val: dut.u_sub.internal_reg,
        );

        initial {
            rst.assert();
            din = 3;
            clk.next(20);
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn multiple_component_instances_are_independent() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;

        var a: logic<8>;
        var b: logic<8>;
        var qa: logic<8>;
        var qb: logic<8>;

        inst m0: $comp::mirror ( clk, d: a, q: qa );
        inst m1: $comp::mirror ( clk, d: b, q: qb );

        initial {
            a = 8'h11;
            b = 8'h22;
            clk.next();
            $assert(qa == 8'h11, "instance 0");
            $assert(qb == 8'h22, "instance 1");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn component_on_gated_clock_fires_with_the_gate() {
    // A component clocked by a derived (gated) clock must fire exactly
    // when the gated RTL FF does, with the same pre-edge observation.
    let code = r#"
    module GatedCounter (
        clk: input clock,
        rst: input reset,
        cnt: output logic<8>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var en: logic;
        let clk_g: '_ clock = clk & en;

        var cnt: logic<8>;
        var q_comp: logic<8>;

        inst dut: GatedCounter (clk: clk_g, rst, cnt);

        inst mirror: $comp::mirror ( clk: clk_g, d: cnt, q: q_comp );

        initial {
            en = 0;
            q_comp = 0;
            rst.assert();
            clk.next(3);
            $assert(cnt == 0, "gate closed: rtl");
            $assert(q_comp == 0, "gate closed: component");
            en = 1;
            clk.next(2);
            $assert(cnt == 2, "gate open: rtl");
            $assert(q_comp == 1, "gate open: component mirrors pre-edge cnt");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn component_on_reset_fires_on_reset_edges() {
    // A connected reset port fires `on_reset` on every reset edge, and all
    // of them precede the first `on_clock` (the component asserts the
    // ordering itself).
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        var q: logic<8>;

        inst w: $comp::reset_watch ( clk, rst, q );

        initial {
            rst.assert();
            $assert(q == 3, "on_reset fired on every reset edge");
            clk.next();
            $assert(q == 101, "on_clock fired after reset");
            clk.next();
            $assert(q == 102, "on_clock keeps firing");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn component_on_hierarchical_derived_clock_fires() {
    // The component clock is a divided clock generated inside the DUT and
    // reached through a hierarchical reference; the component must fire on
    // exactly the divided edges, mirroring the pre-edge counter value.
    let code = r#"
    module DivDut (
        clk: input clock,
        rst: input reset,
        cnt: output logic<8>,
    ) {
        var toggle: logic;
        always_ff (clk, rst) {
            if_reset { toggle = 0; } else { toggle = ~toggle; }
        }
        let div_clk: '_ clock = clk & toggle;
        always_ff (div_clk, rst) {
            if_reset { cnt = 0; } else { cnt += 1; }
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var cnt: logic<8>;
        var q_comp: logic<8>;

        inst dut: DivDut (clk, rst, cnt);

        inst mirror: $comp::mirror ( clk: dut.div_clk, d: cnt, q: q_comp );

        initial {
            q_comp = 0;
            rst.assert();
            clk.next(10);
            $assert(cnt == 5, "divided clock ticked 5 times");
            $assert(q_comp == 4, "component mirrors pre-edge cnt on the divided clock");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn clocked_component_without_clock_connection_fails() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var x: logic<8>;

        inst n: $comp::no_clock ( d: x );

        initial {
            x = 0;
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("clocked component but resolved no clock port"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn unknown_component_type_fails_the_test() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var x: logic<8>;

        inst u: $comp::unregistered ( out: x );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("unregistered") && msg.contains("not found"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn unused_connection_fails_the_test() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;
        var extra: logic<8>;

        inst mirror: $comp::mirror ( clk, d, q, extra );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("does not use a port named `extra`"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn method_only_component_rejects_inst_form() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;

        inst s: $comp::method_only ( clk );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("method-only component"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn var_form_method_only_component_with_methods() {
    // `var g: $comp::golden;` + zero-time methods with numeric
    // arguments (literal and testbench-variable).
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var x: logic<8>;

        initial {
            g.set(42);
            g.check(42);
            x = 42;
            g.check(x);
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn method_failure_fails_the_test() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;

        initial {
            g.set(1);
            g.check(2);
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("[g]") && msg.contains("expected 1, got 2"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn unknown_method_fails_the_test() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;

        initial {
            g.no_such_method(1);
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("unknown method: no_such_method"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn string_argument_and_host_file_service() {
    // `iss.load("x.elf")`-shaped calls: a string method argument travels
    // to the component, which round-trips state through host-mediated
    // file I/O.
    let dir = std::env::temp_dir().join(format!("veryl_m3_file_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("golden.txt");
    let path_str = path.to_str().unwrap().replace('\\', "/");
    let code = format!(
        r#"
    #[test(comp_test)]
    module comp_test {{
        var g: $comp::golden;

        initial {{
            g.set(42);
            g.save("{path_str}");
            g.set(0);
            g.load("{path_str}");
            g.check(42);
            $finish();
        }}
    }}
    "#
    );
    assert_all_pass(&run_component_test(&code, "comp_test"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "42");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clocked_component_rejects_var_form() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var m: $comp::mirror;

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("clocked component"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn inst_component_accepts_methods() {
    // Methods are not limited to the var form: an `inst` component takes
    // zero-time calls too (the rv_iss `load` pattern). method_only's
    // default method rejects everything, which must fail the test.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;

        inst mirror: $comp::mirror ( clk, d, q );

        initial {
            mirror.load(1);
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("unknown method: load"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn method_return_value_assignment() {
    // `x = g.get();` — the assignment form receives the method's return
    // value; assigning a method that returns unit is a failure.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var x: logic<8>;

        initial {
            g.set(42);
            x = g.get();
            $assert(x == 42, "return value");
            g.check(x);
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn method_returning_unit_fails_assignment() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var x: logic<8>;

        initial {
            x = g.set(1);
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("returned no value"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

/// Parameter-only component checking string and shorthand-bound numeric
/// parameters at create time.
struct GapParam;

impl Component for GapParam {
    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        let name = ctx.param("NAME")?;
        let name = name.as_str()?;
        if name != "hello" {
            bail!("bad NAME: {name}");
        }
        let width = ctx.param("WIDTH")?.as_u64()?;
        if width != 8 {
            bail!("bad WIDTH: {width}");
        }
        Ok(Self)
    }
}

#[test]
fn string_and_shorthand_parameters() {
    // Registration is idempotent; keyed separately from COMPONENTS so
    // the metadata in analyze_component_top must know the name.
    static GAP_PARAM: sys::VrlComponentVTable = export::vtable::<GapParam>();
    register_static_component("gap_param", &GAP_PARAM);

    let code = r#"
    #[test(comp_test)]
    module comp_test {
        const WIDTH: u32 = 8;

        inst p: $comp::gap_param #( NAME: "hello", WIDTH );

        initial {
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn named_method_arguments_are_rejected() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;

        initial {
            g.set(value: 42);
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidFactor { .. })),
        "expected InvalidFactor, got {errors:?}"
    );
}
#[test]
fn component_output_conflicting_with_rtl_driver_fails() {
    let code = r#"
    module Driver (
        clk: input clock,
        rst: input reset,
        q: output logic<8>,
    ) {
        always_ff {
            if_reset { q = 0; }
            else { q += 1; }
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        var d: logic<8>;
        var q: logic<8>;

        inst dut: Driver (clk, rst, q);
        inst mirror: $comp::mirror ( clk, d, q );

        initial {
            rst.assert();
            clk.next(2);
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("conflicts with an RTL driver"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn component_outputs_conflicting_with_each_other_fail() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var a: logic<8>;
        var b: logic<8>;
        var q: logic<8>;

        inst m0: $comp::mirror ( clk, d: a, q );
        inst m1: $comp::mirror ( clk, d: b, q );

        initial {
            clk.next();
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("conflicts with component `m0`"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn struct_connection_flattens_like_packed_struct() {
    // Pins the ABI flattening rule: the first struct member occupies the
    // most-significant bits (SystemVerilog packed-struct layout), so
    // {hi: 0xab, lo: 0x5} crosses the boundary as 12'habs = 0xab5.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        struct Pair {
            hi: logic<8>,
            lo: logic<4>,
        }

        inst clk: $tb::clock_gen;
        var p: Pair;

        inst chk: $comp::hier_checker #( EXPECT: 2741, LIMIT: 3 ) ( clk, val: p );

        initial {
            p.hi = 8'hab;
            p.lo = 4'h5;
            clk.next(5);
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

const HANDSHAKE_IF: &str = r#"
    interface HsIf {
        var ready: logic;
        var valid: logic;
        var data: logic<8>;
        modport master {
            ready: input,
            valid: output,
            data: output,
        }
    }
"#;

#[test]
fn modport_connection_expands_members_with_directions() {
    // `bus: bus.master` expands into `bus.<member>` connections whose
    // directions come from the modport: the component reads `bus.ready`
    // (testbench-driven) and drives `bus.valid`. The unused `bus.data`
    // member does not fail the unused-port check.
    let code = &format!(
        r#"
    {HANDSHAKE_IF}
    #[test(comp_test)]
    module comp_test {{
        inst clk: $tb::clock_gen;
        inst bus: HsIf;

        inst bfm: $comp::mp_master ( clk, bus: bus.master );

        initial {{
            bus.ready = 0;
            clk.next();
            $assert(bus.valid == 0, "not ready yet");
            bus.ready = 1;
            clk.next();
            clk.next();
            $assert(bus.valid == 1, "valid follows ready");
            $finish();
        }}
    }}
    "#
    );
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn modport_clock_member_fires_component() {
    // A clock-typed modport input member registers the component's clock
    // event, so the component fires without a separate plain clk port.
    let code = r#"
    interface ClkIf {
        var clk: clock;
        var q: logic<8>;
        modport comp {
            clk: input,
            q: output,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: ClkIf;

        inst c: $comp::mp_clocked ( bus: bus.comp );

        assign bus.clk = clk;

        initial {
            clk.next();
            $assert(bus.q == 1, "fired once");
            clk.next();
            $assert(bus.q == 2, "fired twice");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn modport_output_member_cannot_be_input() {
    // `bus.ready` is an input member of the modport, so a component
    // requesting it as an output fails at load time.
    let code = &format!(
        r#"
    {HANDSHAKE_IF}
    #[test(comp_test)]
    module comp_test {{
        inst clk: $tb::clock_gen;
        inst bus: HsIf;

        inst bfm: $comp::mp_wrongdir ( clk, bus: bus.master );

        initial {{
            clk.next();
            $finish();
        }}
    }}
    "#
    );
    for (config, result, _) in run_component_test(code, "comp_test") {
        match result {
            TestResult::Fail(msg) => {
                assert!(msg.contains("bus.ready"), "unexpected failure: {msg}")
            }
            other => panic!("expected load failure for {config:?}, got {other:?}"),
        }
    }
}

#[test]
fn modport_group_fully_unused_is_error() {
    // The component never touches any `bus.*` member, so the whole
    // modport connection is reported as unused.
    let code = &format!(
        r#"
    {HANDSHAKE_IF}
    #[test(comp_test)]
    module comp_test {{
        inst clk: $tb::clock_gen;
        inst bus: HsIf;
        var trigger: logic;

        inst f: $comp::failer ( clk, trigger, bus: bus.master );

        initial {{
            trigger = 0;
            clk.next();
            $finish();
        }}
    }}
    "#
    );
    for (config, result, _) in run_component_test(code, "comp_test") {
        match result {
            TestResult::Fail(msg) => assert!(
                msg.contains("does not use any member of the `bus` connection"),
                "unexpected failure: {msg}"
            ),
            other => panic!("expected load failure for {config:?}, got {other:?}"),
        }
    }
}

#[test]
fn interface_connection_without_modport_is_error() {
    let code = &format!(
        r#"
    {HANDSHAKE_IF}
    #[test(comp_test)]
    module comp_test {{
        inst clk: $tb::clock_gen;
        inst bus: HsIf;

        inst bfm: $comp::mp_master ( clk, bus );

        initial {{
            $finish();
        }}
    }}
    "#
    );
    let errors = analyze_component_errors(code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InvalidFactor { .. })),
        "expected invalid factor, got: {errors:?}"
    );
}

#[test]
fn manifest_checked_connections_pass() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;

        inst m: $comp::manifested #( STEP: 1 ) ( clk, d, q );

        initial {
            d = 8'h11;
            clk.next();
            clk.next();
            $assert(q == 8'h11, "manifested mirror");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn manifest_rejects_undeclared_port() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var dd: logic<8>;

        inst m: $comp::manifested_rt ( clk, dd );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        match result {
            TestResult::Fail(msg) => assert!(
                msg.contains("declares no port named `dd`"),
                "unexpected failure: {msg}"
            ),
            other => panic!("expected load failure for {config:?}, got {other:?}"),
        }
    }
}

#[test]
fn manifest_rejects_undeclared_parameter() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;

        inst m: $comp::manifested_rt #( GAIN: 2 ) ( clk, d, q );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        match result {
            TestResult::Fail(msg) => assert!(
                msg.contains("declares no parameter named `GAIN`"),
                "unexpected failure: {msg}"
            ),
            other => panic!("expected load failure for {config:?}, got {other:?}"),
        }
    }
}

#[test]
fn clock_reset_ports_drive_hooks() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        var d: logic<8>;
        var q: logic<8>;

        inst m: $comp::edge ( clk, rst, d, q );

        initial {
            d = 8'h3c;
            clk.next();
            clk.next();
            $assert(q == 8'h3c, "edge mirror");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn role_port_rejects_plain_connection_without_manifest() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var r: logic;

        inst m: $comp::role ( clk, rst: r );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        match result {
            TestResult::Fail(msg) => assert!(
                msg.contains("port `rst` is connected, but not with a reset"),
                "unexpected failure: {msg}"
            ),
            other => panic!("expected load failure for {config:?}, got {other:?}"),
        }
    }
}

#[test]
fn clock_connection_resolved_as_plain_data_is_rejected() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;

        inst m: $comp::plain_clk ( clk );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        match result {
            TestResult::Fail(msg) => assert!(
                msg.contains(
                    "the `clk` connection is a clock but `plain_clk` does not declare the port as one (a ClockPort field)"
                ),
                "unexpected failure: {msg}"
            ),
            other => panic!("expected load failure for {config:?}, got {other:?}"),
        }
    }
}

#[test]
fn manifest_rejects_plain_connection_to_clock_port() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var c: logic;
        var d: logic<8>;
        var q: logic<8>;

        inst m: $comp::edge_rt ( clk: c, rst: c, d, q );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        match result {
            TestResult::Fail(msg) => assert!(
                msg.contains("declared as a clock but the connected expression is not a clock"),
                "unexpected failure: {msg}"
            ),
            other => panic!("expected load failure for {config:?}, got {other:?}"),
        }
    }
}

#[test]
fn analyzer_diagnoses_plain_connection_to_clock_and_reset_ports() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var c: logic;
        var d: logic<8>;
        var q: logic<8>;

        inst m: $comp::edge ( clk: c, rst: c, d, q );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    for role in ["clock", "reset"] {
        assert!(
            errors.iter().any(|x| matches!(
                x,
                AnalyzerError::ComponentInterfaceMismatch { kind, .. }
                    if kind.to_string().contains(&format!(
                        "declared as a {role} but the connected expression is not a {role}"
                    ))
            )),
            "expected {role} mismatch, got: {errors:?}"
        );
    }
}

#[test]
fn analyzer_diagnoses_clock_connection_to_data_port() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        var q: logic<8>;

        inst m: $comp::edge ( clk, rst, d: clk, q );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors.iter().any(|x| matches!(
            x,
            AnalyzerError::ComponentInterfaceMismatch { kind, .. }
                if kind.to_string().contains(
                    "the `d` connection is a clock but the component does not declare the port as a clock"
                )
        )),
        "expected data-port clock mismatch, got: {errors:?}"
    );
}

#[test]
fn analyzer_diagnoses_unconnected_clock_and_reset_ports() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;

        inst m: $comp::edge ( clk, d, q );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors.iter().any(|x| matches!(
            x,
            AnalyzerError::ComponentInterfaceMismatch { kind, .. }
                if kind.to_string().contains("reset port `rst` is not connected")
        )),
        "expected unconnected reset port, got: {errors:?}"
    );
}

#[test]
fn analyzer_diagnoses_unconnected_data_port() {
    // A missing data-port connection is an analysis-time error; an entirely
    // unconnected group is one diagnostic.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;

        inst m: $comp::manifested #( STEP: 1 ) ( clk, d );
        inst c: $comp::wide_bus ( clk );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let text = errors_text(&errors);
    assert!(
        text.contains("port `q` is not connected"),
        "expected unconnected data port, got {errors:?}"
    );
    assert!(
        text.contains("interface port `bus` is not connected"),
        "expected unconnected group, got {errors:?}"
    );
    assert!(
        !text.contains("port `bus.data` is not connected"),
        "group members should collapse into the group diagnostic, got {errors:?}"
    );
}

#[test]
fn written_but_failed_connection_is_not_reported_unconnected() {
    // `q` is written as a connection but undeclared: the primary
    // undefined-identifier diagnostic must not be joined by a contradictory
    // "not connected" one.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;

        inst m: $comp::manifested #( STEP: 1 ) ( clk, d, q );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let text = errors_text(&errors);
    assert!(
        text.contains("undefined"),
        "expected the primary error, got {errors:?}"
    );
    assert!(
        !text.contains("is not connected"),
        "a written connection must not also be reported unconnected, got {errors:?}"
    );
}

#[test]
fn undeclared_member_of_connected_interface_is_an_error() {
    // `WideMonExtra` declares member `extra`, which `WideIf` does not
    // provide: declared members are requirements, so this is diagnosed.
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: WideIf;

        inst c: $comp::extra_bus ( clk, bus: bus.mon );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let text = errors_text(&errors);
    assert!(
        text.contains("port `bus.extra` is not connected")
            && text.contains("has no member `extra`"),
        "expected the missing-member diagnostic, got {errors:?}"
    );
}

#[test]
fn analyzer_rejects_wrong_interface_on_bound_group() {
    // The declared binding (`WideIf`) is a project-local interface, resolved
    // in the defining project's namespace; connecting a structurally-similar
    // but different interface with the same modport name is rejected.
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    interface OtherIf {
        var clk : clock    ;
        var data: logic<32>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: OtherIf;

        inst c: $comp::wide_bus ( clk, bus: bus.mon );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors)
            .contains("bound to interface `WideIf`, but a different interface is connected"),
        "expected interface mismatch, got {errors:?}"
    );
}

#[test]
fn analyzer_rejects_wrong_modport_on_bound_group() {
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
        modport drv {
            clk : input ,
            data: output,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: WideIf;

        inst c: $comp::wide_bus ( clk, bus: bus.drv );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("bound to modport `mon`, but modport `drv` is connected"),
        "expected modport mismatch, got {errors:?}"
    );
}

#[test]
fn analyzer_diagnoses_group_member_missing_from_interface() {
    // With the group connected, a declared member the interface does not
    // provide is reported individually.
    let code = r#"
    interface ThinIf {
        var clk: clock;
        modport mon {
            clk: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: ThinIf;

        inst c: $comp::wide_bus ( clk, bus: bus.mon );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let text = errors_text(&errors);
    assert!(
        text.contains("port `bus.data` is not connected") && text.contains("has no member `data`"),
        "expected unconnected member diagnostic, got {errors:?}"
    );
}

#[test]
fn analyzer_diagnoses_undeclared_port() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var dd: logic<8>;

        inst m: $comp::manifested ( clk, dd );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors.iter().any(|x| matches!(
            x,
            AnalyzerError::ComponentInterfaceMismatch { kind, .. }
                if kind.to_string().contains("no port named `dd`")
        )),
        "expected interface mismatch, got: {errors:?}"
    );
}

#[test]
fn dependency_manifest_error_help_hints_veryl_update() {
    // A dependency component's manifest reflects the locked dependency
    // version, so its interface diagnostics point at `veryl update`.
    veryl_analyzer::component_manifest_table::insert(
        veryl_parser::resource_table::insert_str("dep_vip::mirror"),
        veryl_metadata::ComponentManifest::parse(&Manifested::manifest().unwrap()).unwrap(),
    );
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var dd: logic<8>;

        inst m: $comp::dep_vip::mirror ( clk, dd );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let help = errors
        .iter()
        .find_map(|x| match x {
            AnalyzerError::ComponentInterfaceMismatch { kind, help, .. }
                if kind.to_string().contains("no port named `dd`") =>
            {
                Some(help.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected interface mismatch, got: {errors:?}"));
    assert!(
        help.contains("run `veryl update`"),
        "unexpected help: {help}"
    );

    // The same diagnostic on the project's own component keeps the plain help.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var dd: logic<8>;

        inst m: $comp::manifested ( clk, dd );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let help = errors
        .iter()
        .find_map(|x| match x {
            AnalyzerError::ComponentInterfaceMismatch { kind, help, .. }
                if kind.to_string().contains("no port named `dd`") =>
            {
                Some(help.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected interface mismatch, got: {errors:?}"));
    assert!(!help.contains("veryl update"), "unexpected help: {help}");
}

#[test]
fn analyzer_diagnoses_unknown_method_and_arity() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;

        inst m: $comp::manifested ( clk, d, q );

        initial {
            m.nothere();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors.iter().any(|x| matches!(
            x,
            AnalyzerError::ComponentInterfaceMismatch { kind, .. }
                if kind.to_string().contains("no method named `nothere`")
        )),
        "expected interface mismatch, got: {errors:?}"
    );
}

#[test]
fn analyzer_diagnoses_method_only_inst_form() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;

        inst m: $comp::svc_manifested ( clk );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors.iter().any(|x| matches!(
            x,
            AnalyzerError::ComponentInterfaceMismatch { kind, .. }
                if kind.to_string().contains("method-only component")
        )),
        "expected interface mismatch, got: {errors:?}"
    );
}

#[test]
fn analyzer_diagnoses_unit_method_in_value_position() {
    // Calling a unit-returning method in value position is a misuse even when
    // every method of the interface returns unit.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::svc_allunit;
        var x: logic<8>;

        initial {
            x = g.poke();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors.iter().any(|x| matches!(
            x,
            AnalyzerError::ComponentInterfaceMismatch { kind, .. }
                if kind.to_string().contains("returns no value")
        )),
        "expected the returns-no-value diagnostic, got: {errors:?}"
    );
}

#[test]
fn trace_var_appears_in_vcd() {
    // `ctx.trace_var` values registered during create land in the waveform
    // under a `<instance>` scope; the header is finalized after
    // `init_components` (attach_dump), so registration during create works.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;

        inst tracer: $comp::tracer ( clk );

        initial {
            clk.next();
            clk.next();
            clk.next();
            $finish();
        }
    }
    "#;
    LazyLock::force(&REGISTER);
    for config in Config::all() {
        let ir = analyze_component_top(code, &config, "comp_test")
            .unwrap_or_else(|x| panic!("build failed for {config:?}: {x:?}"));

        use crate::wave_dumper::{SharedVec, WaveDumper};
        let dump_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let dumper = WaveDumper::new_vcd(Box::new(SharedVec(dump_buf.clone())));

        let mut sim = Simulator::new(ir, None);
        sim.init_components(0, "comp_test").unwrap();
        sim.attach_dump(dumper);

        let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
        let clock_periods = build_clock_periods(&sim.ir.event_statements);
        let stmts = sim.ir.event_statements.get(&Event::Initial).unwrap();
        let tb_stmts = convert_initial_to_testbench(stmts, &event_map, &clock_periods, 3);
        let result = run_testbench(&mut sim, &tb_stmts);
        assert_eq!(result, TestResult::Pass, "failed for {config:?}");

        drop(sim);
        let dump = std::sync::Arc::try_unwrap(dump_buf)
            .unwrap()
            .into_inner()
            .unwrap();
        let mut parser = vcd::Parser::new(dump.as_slice());
        let header = parser.parse_header().unwrap();
        let state_var = header
            .find_var(&["tracer", "state"])
            .expect("trace variable not found in VCD");
        assert_eq!(state_var.size, 8);

        // The last recorded value is cycle 3 doubled.
        let state_code = state_var.code;
        let mut last = None;
        for cmd in parser {
            if let Ok(vcd::Command::ChangeVector(code, value)) = cmd
                && code == state_code
            {
                let bits: u64 = value
                    .iter()
                    .fold(0, |acc, b| (acc << 1) | u64::from(b == vcd::Value::V1));
                last = Some(bits);
            }
        }
        assert_eq!(last, Some(6), "failed for {config:?}");
    }
}

#[test]
fn dependency_component_two_level_namespace() {
    // A component provided by a dependency is instantiated as
    // `$comp::<project>::<name>` and resolves through the composite
    // library key.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;

        inst mirror: $comp::dep_vip::mirror ( clk, d, q );

        initial {
            d = 8'h3c;
            clk.next();
            clk.next();
            $assert(q == 8'h3c, "dependency component mirrors input");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn dependency_component_unknown_name_is_error() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;

        inst x: $comp::dep_vip::no_such ( clk );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::UnknownMember { .. })),
        "expected unknown member, got: {errors:?}"
    );
}

#[test]
fn method_call_in_expression_position() {
    // Calls embedded in expressions hoist to zero-time statements ahead of
    // the enclosing one: the return value participates in arithmetic,
    // `$assert` arguments, branch conditions and nested method arguments.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var x: logic<8>;

        initial {
            g.set(41);
            x = g.get() + 1;
            $assert(x == 42, "arith on return value");
            $assert(g.get() == 41, "call inside assert");
            if g.get() == 41 {
                g.check(g.get());
            } else {
                g.check(0);
            }
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn method_call_reexecutes_per_loop_iteration() {
    // A call hoisted inside a loop body belongs to the statement it came
    // from and re-runs every iteration (the polling form).
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var sum: logic<16>;

        initial {
            g.set(0);
            sum = 0;
            for i in 0..5 {
                sum += g.bump();
            }
            $assert(sum == 15, "bump ran per iteration");
            $assert(g.get() == 5, "five bumps");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn method_call_in_let_initializer() {
    // Hoists out of `let` initializers attach to the `let` itself, both at
    // the top level of an initial block and inside loop bodies.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var sum: logic<16>;

        initial {
            g.set(41);
            let x: logic<8> = g.get() + 1;
            $assert(x == 42, "let initializer at initial top level");
            let y: logic<8> = g.get();
            $assert(y == 41, "bare call initializer");
            g.set(0);
            sum = 0;
            for i in 0..3 {
                let v: logic<16> = g.bump() + 0;
                sum += v;
            }
            $assert(sum == 6, "let initializer re-runs per iteration");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn method_return_assignment_counts_as_assigned() {
    // `x = g.get();` must mark `x` assigned; `veryl check`/`publish` treat
    // the unassigned warning as an error otherwise.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var x: logic<8>;

        initial {
            g.set(1);
            x = g.get();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnassignVariable { .. })),
        "return assignment left the variable unassigned: {errors:?}"
    );
}

#[test]
fn method_return_into_indexed_destination() {
    // An indexed/selected destination desugars into the expression-position
    // hoist plus an ordinary assignment.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var arr: logic<8> [4];

        initial {
            for i in 0..4 {
                arr[i] = 0;
            }
            g.set(42);
            arr[1] = g.get();
            $assert(arr[1] == 42, "indexed destination");
            $assert(arr[0] == 0, "untouched element");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn wide_return_fails_expression_form_but_not_direct() {
    // The expression-form hoist temporary is 64 bits; a wider dynamic
    // return value must fail loudly instead of truncating silently.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var x: logic<64>;

        initial {
            g.set(7);
            x = g.wide() + 0;
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("returned 128 bits"),
            "unexpected message for {config:?}: {msg}"
        );
    }

    // The direct assignment form carries the declared width of the
    // destination; a 64-bit destination keeps the low word (SV semantics).
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var x: logic<64>;

        initial {
            g.set(7);
            x = g.wide();
            $assert(x == 7, "low word");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn var_form_takes_generic_parameters() {
    // The var form takes parameters as generic arguments (positionally,
    // in manifest order); constants resolve through the ordinary path and
    // declared widths see the values.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        const W: u32 = 96;
        var w: $comp::wide::<W>;
        var x: logic<96>;

        initial {
            w.put(96'h55);
            x = w.get();
            $assert(x == 96'h55, "generic parameter reaches the component");
            $assert(w.get() == x, "declared width resolves from generics");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn var_generic_parameter_diagnostics() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var a: $comp::wide::<96, 1>;
        var b: $comp::wide;

        initial {
            a.get();
            b.get();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let messages = errors_text(&errors);
    assert!(
        messages.contains("declares 1 parameter(s), but 2 generic argument(s)"),
        "too-many-arguments diagnostic missing: {messages}"
    );
    assert!(
        messages.contains("required parameter `WIDTH` is not given"),
        "required-parameter diagnostic missing: {messages}"
    );
}

#[test]
fn var_generic_args_accept_scoped_and_based_constants() {
    // Any constant expression the ordinary evaluation path can resolve is
    // accepted as a var-form generic argument.
    let code = r#"
    package cfg_pkg {
        const W: u32 = 96;
    }

    #[test(comp_test)]
    module comp_test {
        var a: $comp::wide::<cfg_pkg::W>;
        var b: $comp::wide::<32'd96>;
        var x: logic<96>;
        var y: logic<96>;

        initial {
            a.put(96'h11);
            x = a.get();
            $assert(x == 96'h11, "package constant generic argument");
            b.put(96'h22);
            y = b.get();
            $assert(y == 96'h22, "based literal generic argument");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn tb_builtin_generic_arguments_are_rejected() {
    // The builtin `$tb` components declare no generic parameters; only
    // user-defined components take parameters as generic arguments.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen::<3>;

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchGenericsArity { .. })),
        "expected MismatchGenericsArity for `$tb::clock_gen::<3>`, got {errors:?}"
    );

    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var f: $tb::file::<8>;

        initial {
            f.open("out.txt");
            f.close();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::MismatchGenericsArity { .. })),
        "expected MismatchGenericsArity for `$tb::file::<8>`, got {errors:?}"
    );
}

#[test]
fn inst_form_rejects_generic_arguments() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;
        inst m: $comp::mirror::<8> ( clk, d, q );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("takes parameters with `#()`"),
        "expected inst-form generic argument diagnostic, got {errors:?}"
    );
}

#[test]
fn method_call_path_shape_is_checked() {
    // An intermediate path segment would be silently dropped otherwise.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;

        initial {
            g.bogus.get();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("instance.method"),
        "expected path shape diagnostic, got {errors:?}"
    );

    // Generic arguments on a method have no meaning across the ABI.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;

        initial {
            g::get::<8>();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("no generic arguments"),
        "expected method generic argument diagnostic, got {errors:?}"
    );
}

#[test]
fn initial_before_inst_resolves_declared_widths() {
    // Module items are order-free: an initial block written before the
    // component instantiation still sees its parameters.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var x: logic<96>;

        initial {
            w.put(96'h55);
            x = w.get();
            $assert(x == 96'h55, "initial before inst");
            $finish();
        }

        inst w: $comp::wide #( WIDTH: 96 );
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn select_destination_checks_declared_width() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst w: $comp::wide #( WIDTH: 96 );
        var x: logic<128>;

        initial {
            x[7:0] = w.get();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("96 bit(s) but the destination is 8"),
        "expected width mismatch for selected destination, got {errors:?}"
    );
}

#[test]
fn declared_return_width_over_abi_limit_is_rejected() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst w: $comp::wide #( WIDTH: 512 );
        var y: logic<1024>;

        initial {
            y = w.doubled();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("at most 512 bits"),
        "expected ABI return width diagnostic, got {errors:?}"
    );
}

#[test]
fn declared_widths_size_and_check_statically() {
    // Parameter-dependent and arithmetic width declarations resolve
    // against `#()`: the expression form carries the exact width (96 and
    // 192 bits here) and the direct form checks the destination width.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst w: $comp::wide #( WIDTH: 96 );
        var x: logic<96>;
        var y: logic<192>;

        initial {
            w.put(96'h1_0000_0000_0000_0022);
            x = w.get();
            $assert(x == 96'h1_0000_0000_0000_0022, "declared-width return");
            $assert(w.get() == x, "expression form carries 96 bits");
            y = w.doubled();
            $assert(y == 3, "arithmetic width");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn interface_param_return_width_resolves() {
    // `#[ret_width(bus.WIDTH)]` resolves against the interface instance
    // the `bus` group is connected to: a constant declared directly in
    // the interface body.
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: WideIf;
        var x: logic<96>;

        inst c: $comp::wide_bus ( clk, bus: bus.mon );

        assign bus.clk = clk;

        initial {
            bus.data = 96'h1_0000_0000_0000_0022;
            clk.next();
            x = c.snoop();
            $assert(x == 96'h1_0000_0000_0000_0022, "interface-width return");
            $assert(c.snoop() == x, "expression form carries 96 bits");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn raw_identifier_names_bind_and_dispatch() {
    // Rust raw identifiers (`r#in`, `r#as`, `r#pub`, `fn r#match`) and
    // Veryl raw identifiers meet at the manifest under the unrawed names,
    // across an interface member, a loose port, a parameter, and a method
    // call (`match` is a Rust-only keyword, so Veryl calls it plainly).
    let code = r#"
    interface RawIf {
        var r#in: logic<8>;
        modport mon {
            r#in: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: RawIf;
        var s: logic<8>;
        var x: logic<64>;

        inst c: $comp::raw_names #( r#pub: 3 ) ( clk, r#as: s, bus: bus.mon );

        initial {
            s = 5;
            bus.r#in = 7;
            clk.next();
            x = c.match();
            $assert(x == 15, "unrawed names bind end to end");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn multiple_groups_of_the_same_interface() {
    // Two ports of the same interface: one port set embedded under two
    // group names, and each group's `#[ret_width]` parameter resolves
    // against its own connected instance.
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst a: WideIf;
        inst b: WideIf;

        inst c: $comp::dual_bus ( clk, bus0: a.mon, bus1: b.mon );

        assign a.clk = clk;
        assign b.clk = clk;

        initial {
            a.data = 96'h1_0000_0000_0000_0011;
            b.data = 96'h2_0000_0000_0000_0022;
            clk.next();
            $assert(c.snoop0() == 96'h1_0000_0000_0000_0011, "bus0 member");
            $assert(c.snoop1() == 96'h2_0000_0000_0000_0022, "bus1 member");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn loose_connection_cannot_alias_a_group_member() {
    // Members have no loose connection names; an extra item is simply an
    // unknown port.
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: WideIf;
        var other: logic<96>;

        inst c: $comp::wide_bus ( clk, bus: bus.mon, bus_data: other );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("declares no port named `bus_data`"),
        "expected an unknown-port diagnostic, got {errors:?}"
    );
}

#[test]
fn runtime_rejects_loose_alias_of_a_group_member() {
    // Load-time twin of the analyzer check, through the runtime-only
    // manifest of `wide_bus_rt`.
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: WideIf;
        var other: logic<96>;

        inst c: $comp::wide_bus_rt ( clk, bus: bus.mon, bus_data: other );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _sim) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("declares no port named `bus_data`"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn duplicate_group_connection_is_rejected() {
    // Two connections to one group would race for the same host ports, and
    // only the first interface would be identity-checked.
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst a: WideIf;
        inst b: WideIf;

        inst c: $comp::wide_bus ( clk, bus: a.mon, bus: b.mon );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("port `bus` is connected more than once"),
        "expected a duplicate-group diagnostic, got {errors:?}"
    );
}

#[test]
fn runtime_rejects_duplicate_group_connection() {
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst a: WideIf;
        inst b: WideIf;

        inst c: $comp::wide_bus_rt ( clk, bus: a.mon, bus: b.mon );

        initial {
            $finish();
        }
    }
    "#;
    for (config, result, _sim) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("connected more than once"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn unknown_group_connection_is_rejected_at_analysis() {
    // A typo'd group item name is answered at analysis time, matching the
    // load-time GroupUnused backstop.
    let code = r#"
    interface WideIf {
        const WIDTH: u32 = 96;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst a: WideIf;
        inst b: WideIf;

        inst c: $comp::wide_bus ( clk, bus: a.mon, buz: b.mon );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("declares no interface port named `buz`"),
        "expected an unknown-group diagnostic, got {errors:?}"
    );
}

#[test]
fn empty_modport_expansion_still_reports_missing_members() {
    // An import-only modport expands to zero member connects; the group
    // still counts as connected, so its declared members are reported.
    let code = r#"
    interface ApiIf {
        var data: logic<8>;
        function get () -> logic<8> {
            return data;
        }
        modport mon {
            get: import,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: ApiIf;

        inst c: $comp::wide_bus ( clk, bus: bus.mon );

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let text = errors_text(&errors);
    assert!(
        text.contains("port `bus.data` is not connected"),
        "expected the missing-member diagnostic, got {errors:?}"
    );
}

#[test]
fn interface_param_return_width_through_generic_package() {
    // The constant lives in the interface's generic package argument
    // (visible in the interface through `import PKG::*`), the shape of
    // the std AXI interfaces.
    let code = r#"
    proto package wp_proto {
        const WIDTH: u32;
        type data_t;
    }

    package wp_pkg::<W: u32> for wp_proto {
        const WIDTH: u32    = W;
        type data_t = logic<WIDTH>;
    }

    interface WideIf::<PKG: wp_proto> {
        import PKG::*;
        var clk : clock ;
        var data: data_t;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: WideIf::<wp_pkg::<96>>;
        var x: logic<96>;

        inst c: $comp::wide_bus ( clk, bus: bus.mon );

        assign bus.clk = clk;

        initial {
            bus.data = 96'h8000_0000_0000_0000_0000_0001;
            clk.next();
            x = c.snoop();
            $assert(x == 96'h8000_0000_0000_0000_0000_0001, "package-width return");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn interface_param_width_uses_instance_value_generic() {
    // A constant defined from the interface's own value generic parameter
    // evaluates with the connected instance's argument, not the parameter
    // default.
    let code = r#"
    interface WideIf::<W: u32 = 32> {
        const WIDTH: u32    = W;
        var clk : clock       ;
        var data: logic<WIDTH>;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: WideIf::<96>;
        var x: logic<96>;

        inst c: $comp::wide_bus ( clk, bus: bus.mon );

        assign bus.clk = clk;

        initial {
            bus.data = 96'h4000_0000_0000_0000_0000_0003;
            clk.next();
            x = c.snoop();
            $assert(x == 96'h4000_0000_0000_0000_0000_0003, "instance-argument width");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn interface_param_width_follows_import() {
    // With two generic package arguments declaring the same constant, the
    // width follows the one the interface actually imports — not whichever
    // argument happens to match first.
    let code = r#"
    proto package wp_proto {
        const WIDTH: u32;
        type data_t;
    }

    package wp_pkg::<W: u32> for wp_proto {
        const WIDTH: u32    = W;
        type data_t = logic<WIDTH>;
    }

    interface WideIf::<APKG: wp_proto, BPKG: wp_proto> {
        import BPKG::*;
        var clk : clock ;
        var data: data_t;
        modport mon {
            clk : input,
            data: input,
        }
    }

    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        inst bus: WideIf::<wp_pkg::<32>, wp_pkg::<96>>;
        var x: logic<96>;

        inst c: $comp::wide_bus ( clk, bus: bus.mon );

        assign bus.clk = clk;

        initial {
            bus.data = 96'h8000_0000_0000_0000_0000_0001;
            clk.next();
            x = c.snoop();
            $assert(x == 96'h8000_0000_0000_0000_0000_0001, "imported-package width");
            $finish();
        }
    }
    "#;
    assert_all_pass(&run_component_test(code, "comp_test"));
}

#[test]
fn interface_param_width_without_connection_is_unresolvable() {
    // With no `bus` group connection there is no interface to resolve
    // `bus.WIDTH` against; calling the method reports the declared width
    // as unresolvable.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var x: logic<96>;

        inst c: $comp::wide_bus ( clk );

        initial {
            x = c.snoop();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("cannot resolve the declared width `bus.WIDTH`"),
        "expected unresolvable width diagnostic, got {errors:?}"
    );
}

#[test]
fn analyzer_diagnoses_width_declaration_violations() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst w: $comp::wide #( WIDTH: 96 );
        var narrow: logic<64>;

        initial {
            narrow = w.get();
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    let messages = errors_text(&errors);
    // The return width is a property of the method, so a too-narrow
    // destination is still diagnosed; argument widths are inferred, not
    // declared, so there is no argument-width check.
    assert!(
        messages.contains("96 bit(s) but the destination is 64"),
        "return width mismatch missing: {messages}"
    );

    // A required parameter cannot be omitted.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst w: $comp::wide;

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors_text(&errors).contains("required parameter `WIDTH` is not given"),
        "missing required-parameter diagnostic: {errors:?}"
    );
}

#[test]
fn declared_width_violation_by_component_fails_at_runtime() {
    // The component itself is held to its declaration.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst w: $comp::wide #( WIDTH: 96 );
        var x: logic<32>;

        initial {
            x = w.lying();
            $finish();
        }
    }
    "#;
    for (config, result, _) in run_component_test(code, "comp_test") {
        let TestResult::Fail(msg) = &result else {
            panic!("expected failure for {config:?}, got {result:?}");
        };
        assert!(
            msg.contains("declares a 32-bit return value but returned 64 bits"),
            "unexpected message for {config:?}: {msg}"
        );
    }
}

#[test]
fn analyzer_diagnoses_return_value_misuse() {
    // With return types in the manifest: dropping a value-returning method
    // in statement position and assigning a unit method are both caught at
    // analysis time.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;
        inst m: $comp::manifested #( STEP: 1 ) (clk, d, q);
        var x: logic<8>;

        initial {
            m.peek();
            x = m.poke(1);
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::UnusedReturn { .. })),
        "expected UnusedReturn for dropped `peek()`, got {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::ComponentInterfaceMismatch { .. })),
        "expected mismatch for assigned `poke()`, got {errors:?}"
    );

    // The correct forms produce neither diagnostic.
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        inst clk: $tb::clock_gen;
        var d: logic<8>;
        var q: logic<8>;
        inst m: $comp::manifested #( STEP: 1 ) (clk, d, q);
        var x: logic<8>;

        initial {
            x = m.peek();
            m.poke(1);
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        !errors.iter().any(|e| matches!(
            e,
            AnalyzerError::UnusedReturn { .. } | AnalyzerError::ComponentInterfaceMismatch { .. }
        )),
        "unexpected diagnostics: {errors:?}"
    );
}

#[test]
fn method_call_outside_testbench_statement_is_rejected() {
    let code = r#"
    #[test(comp_test)]
    module comp_test {
        var g: $comp::golden;
        var y: logic<8>;

        always_comb {
            y = g.get() + 1;
        }

        initial {
            $finish();
        }
    }
    "#;
    let errors = analyze_component_errors(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::InvalidFactor { .. })),
        "expected InvalidFactor, got {errors:?}"
    );
}

/// End-to-end runs of the wasm transport: the fixture crate is built for
/// wasm32-unknown-unknown with the real cargo and driven through the full
/// pipeline (analyzer, IR, step loop) by routing component names to the
/// `.wasm` via `Config::component_libraries`.
#[cfg(not(target_family = "wasm"))]
mod wasm_transport {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    static FIXTURE_WASM: LazyLock<Option<PathBuf>> = LazyLock::new(build_fixture_wasm);

    fn build_fixture_wasm() -> Option<PathBuf> {
        if Command::new("cargo").arg("--version").output().is_err() {
            eprintln!("skipping: cargo not available");
            return None;
        }
        let libdir = Command::new("rustc")
            .args([
                "--print",
                "target-libdir",
                "--target",
                "wasm32-unknown-unknown",
            ])
            .output();
        let has_wasm_std = libdir.is_ok_and(|out| {
            out.status.success()
                && PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()).exists()
        });
        if !has_wasm_std {
            eprintln!("skipping: wasm32-unknown-unknown std not installed");
            return None;
        }

        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test_component");
        // Shared fixture build cache; see tests/wasm_guest.rs.
        let target_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm-fixture");
        let output = Command::new("cargo")
            .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
            .current_dir(fixture)
            .env("CARGO_TARGET_DIR", &target_dir)
            .output()
            .expect("failed to run cargo");
        assert!(
            output.status.success(),
            "fixture wasm build failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(
            target_dir
                .join("wasm32-unknown-unknown")
                .join("release")
                .join("test_component.wasm"),
        )
    }

    /// Runs with the listed component names routed to the fixture wasm.
    /// Empty result = toolchain unavailable, the test skips.
    fn run_wasm_component_test(
        code: &str,
        top: &str,
        components: &[&str],
    ) -> Vec<(Config, TestResult, Simulator)> {
        let Some(wasm) = FIXTURE_WASM.as_ref() else {
            return vec![];
        };
        run_component_test_configured(code, top, |config| {
            for name in components {
                config.component_libraries.insert(
                    name.to_string(),
                    crate::ir::ComponentLibrary {
                        path: wasm.clone(),
                        type_name: name.to_string(),
                    },
                );
            }
        })
    }

    #[test]
    fn wasm_component_mirrors_rtl_ff_semantics() {
        // The golden semantics test of the native transport, run over wasm:
        // pre-edge observation and NBA-equivalent output commit.
        let code = r#"
        module MirrorDut (
            clk: input clock,
            rst: input reset,
            cnt: output logic<8>,
            q_rtl: output logic<8>,
        ) {
            always_ff {
                if_reset {
                    cnt   = 0;
                    q_rtl = 0;
                } else {
                    cnt   += 1;
                    q_rtl  = cnt;
                }
            }
        }

        #[test(comp_test)]
        module comp_test {
            inst clk: $tb::clock_gen;
            inst rst: $tb::reset_gen(clk);

            var cnt: logic<8>;
            var q_rtl: logic<8>;
            var q_comp: logic<8>;

            inst dut: MirrorDut (clk, rst, cnt, q_rtl);

            inst mirror: $comp::fixture_mirror (
                clk,
                d: cnt,
                q: q_comp,
            );

            initial {
                rst.assert();
                clk.next();
                $assert(cnt == 1, "cnt after 1 edge");
                $assert(q_comp == 0, "component saw pre-edge cnt");
                clk.next();
                $assert(cnt == 2, "cnt after 2 edges");
                $assert(q_comp == 1, "component mirrors previous cnt");
                clk.next();
                $assert(q_rtl == q_comp, "component == rtl");
                $finish();
            }
        }
        "#;
        assert_all_pass(&run_wasm_component_test(
            code,
            "comp_test",
            &["fixture_mirror"],
        ));
    }

    #[test]
    fn wasm_method_only_component_roundtrip() {
        let code = r#"
        #[test(comp_test)]
        module comp_test {
            var g: $comp::fixture_echo;
            var x: logic<64>;

            initial {
                g.set(42);
                x = g.get();
                $assert(x == 42, "get returns stored value");
                $finish();
            }
        }
        "#;
        assert_all_pass(&run_wasm_component_test(
            code,
            "comp_test",
            &["fixture_echo"],
        ));
    }

    #[test]
    fn wasm_component_receives_hash_parameters() {
        // `#()` parameters (string and numeric) travel the full pipeline —
        // analyzer, IR, wasm marshalling including the `param_get` retry
        // protocol — into the guest, which checks the string and exposes
        // the numeric value on its output.
        let code = r#"
        #[test(comp_test)]
        module comp_test {
            inst clk: $tb::clock_gen;
            var x: logic<8>;

            inst p: $comp::fixture_param_probe #( NAME: "hello", WIDTH: 8 ) ( clk, out: x );

            initial {
                clk.next();
                $assert(x == 8, "parameters reached the wasm guest");
                $finish();
            }
        }
        "#;
        assert_all_pass(&run_wasm_component_test(
            code,
            "comp_test",
            &["fixture_param_probe"],
        ));
    }

    #[test]
    fn wasm_component_panic_fails_the_test() {
        let code = r#"
        #[test(comp_test)]
        module comp_test {
            var p: $comp::fixture_panicker;

            initial {
                p.boom();
                $finish();
            }
        }
        "#;
        for (config, result, _) in run_wasm_component_test(code, "comp_test", &["fixture_panicker"])
        {
            let TestResult::Fail(msg) = &result else {
                panic!("expected failure for {config:?}, got {result:?}");
            };
            assert!(
                msg.contains("[p]") && msg.contains("kaboom"),
                "unexpected message for {config:?}: {msg}"
            );
        }
    }
}
