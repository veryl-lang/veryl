//! Fixture for the simulator's real-cargo integration tests: the dlopen
//! path builds it natively, the wasm transport tests build it for
//! wasm32-unknown-unknown.

use veryl_component::*;

/// Counts clock edges into its output.
#[derive(Component)]
#[component(kind = clocked)]
struct FixtureCounter {
    /// Edge count, one increment per clock.
    out: OutputPort,
    clk: ClockPort,
    /// Finish the test after this many edges.
    #[param]
    limit: Option<u64>,
    count: u64,
}

#[component_impl]
impl FixtureCounter {
    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
        let _ = ctx.fired(self.clk);
        self.count += 1;
        ctx.write(self.out, self.count);
        if self.limit.is_some_and(|limit| self.count >= limit) {
            ctx.finish();
        }
        Ok(())
    }
}

/// Mirrors an input to an output with one cycle delay. Widths are left
/// undeclared so tests can connect any width (including multi-word).
/// The role-typed clock port exercises `VRL_DIR_CLOCK` resolution across
/// the transport.
#[derive(Component)]
#[component(kind = clocked)]
struct FixtureMirror {
    clk: ClockPort,
    d: InputPort,
    q: OutputPort,
}

#[component_impl]
impl FixtureMirror {
    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
        let _ = ctx.fired(self.clk);
        let value = ctx.read(self.d);
        ctx.write(self.q, value);
        Ok(())
    }
}

/// Method-only component exercising params, method arguments and returns.
#[derive(Component)]
#[component(kind = method_only)]
struct FixtureEcho {
    #[param]
    scale: Option<u64>,
    #[param]
    tag: Option<String>,
    stored: u64,
}

#[component_impl]
impl FixtureEcho {
    fn set(&mut self, _ctx: &mut SimCtx, value: u64) -> Result<()> {
        self.stored = value * self.scale.unwrap_or(1);
        Ok(())
    }

    fn get(&mut self, ctx: &mut SimCtx) -> Result<u64> {
        ctx.log(&format!("get -> {}", self.stored));
        Ok(self.stored)
    }

    fn name_len(&mut self, _ctx: &mut SimCtx, name: &str) -> Result<u64> {
        Ok(name.len() as u64)
    }

    fn tag_len(&mut self, _ctx: &mut SimCtx) -> Result<u64> {
        Ok(self.tag.as_ref().map_or(0, |t| t.len() as u64))
    }
}

/// Failure paths: explicit error, explicit fail and a panic (trap on wasm).
#[derive(Component)]
#[component(kind = method_only)]
struct FixturePanicker;

#[component_impl]
impl FixturePanicker {
    fn boom(&mut self, _ctx: &mut SimCtx) -> Result<()> {
        panic!("kaboom");
    }

    fn err(&mut self, _ctx: &mut SimCtx) -> Result<()> {
        bail!("deliberate error");
    }
}

/// Host-mediated file I/O round-trip.
#[derive(Component)]
#[component(kind = method_only, requires(file))]
struct FixtureFiler;

#[component_impl]
impl FixtureFiler {
    fn write_note(&mut self, ctx: &mut SimCtx, path: &str, text: &str) -> Result<()> {
        use std::io::Write;
        let mut file = ctx.create(path)?;
        file.write_all(text.as_bytes())?;
        Ok(())
    }

    fn note_len(&mut self, ctx: &mut SimCtx, path: &str) -> Result<u64> {
        use std::io::Read;
        let mut file = ctx.open(path)?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok(text.len() as u64)
    }
}

/// Does file I/O without declaring `requires(file)`; the wasm host must
/// deny it at run time.
#[derive(Component)]
#[component(kind = method_only)]
struct FixtureSneakyFiler;

#[component_impl]
impl FixtureSneakyFiler {
    fn write_note(&mut self, ctx: &mut SimCtx, path: &str) -> Result<()> {
        let _ = ctx.create(path)?;
        Ok(())
    }
}

/// Declares `requires(native)`; the wasm host must refuse to load it.
#[derive(Component)]
#[component(kind = method_only, requires(native))]
struct FixtureNativeDecl;

#[component_impl]
impl FixtureNativeDecl {
    fn ping(&mut self, _ctx: &mut SimCtx) -> Result<()> {
        Ok(())
    }
}

/// Allocates on demand, to exercise the store memory limit.
#[derive(Component)]
#[component(kind = method_only)]
struct FixtureHog;

#[component_impl]
impl FixtureHog {
    fn hog(&mut self, _ctx: &mut SimCtx, bytes: u64) -> Result<u64> {
        let buf = vec![1u8; bytes as usize];
        Ok(buf.iter().map(|b| *b as u64).sum())
    }
}

/// Checks `#()` parameters (string and numeric) received through the
/// host `param_get` protocol and exposes the numeric one on its output.
#[derive(Component)]
#[component(kind = clocked)]
struct FixtureParamProbe {
    clk: ClockPort,
    out: OutputPort,
    #[param(name = "NAME")]
    name: Option<String>,
    #[param(name = "WIDTH")]
    width: Option<u64>,
}

#[component_impl]
impl FixtureParamProbe {
    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
        let _ = self.clk;
        if self.name.as_deref() != Some("hello") {
            bail!("bad NAME: {:?}", self.name);
        }
        ctx.write(self.out, self.width.unwrap_or(0));
        Ok(())
    }
}

/// Traces the current cycle into a waveform variable on every edge.
#[derive(Component)]
#[component(kind = clocked)]
struct FixtureTracer {
    clk: ClockPort,
    #[state]
    var: Option<TraceVar>,
}

#[component_impl]
impl FixtureTracer {
    fn on_build(&mut self, ctx: &mut BuildCtx) -> Result<()> {
        self.var = Some(ctx.trace_var("cycle", 32)?);
        Ok(())
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {
        let _ = ctx.fired(self.clk);
        let cycle = ctx.cycle();
        ctx.trace(self.var.unwrap(), cycle);
        Ok(())
    }
}

veryl_component_export!(
    "fixture_counter" => FixtureCounter,
    "fixture_mirror" => FixtureMirror,
    "fixture_echo" => FixtureEcho,
    "fixture_panicker" => FixturePanicker,
    "fixture_filer" => FixtureFiler,
    "fixture_sneaky_filer" => FixtureSneakyFiler,
    "fixture_native_decl" => FixtureNativeDecl,
    "fixture_hog" => FixtureHog,
    "fixture_tracer" => FixtureTracer,
    "fixture_param_probe" => FixtureParamProbe,
);
