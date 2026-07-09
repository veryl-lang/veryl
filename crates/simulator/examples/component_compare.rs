//! Veryl-side driver for the `compare/component` benchmark: analyzes a
//! `#[test]` module that instantiates `$comp::accumulator`, then drives it
//! through the real component + testbench machinery. Its SystemVerilog
//! counterpart reaches the identical model through DPI-C, so the two runs
//! measure the user-component boundary against the DPI boundary.
//!
//! The model is embedded here (a static, dlopen-free component) rather than
//! loaded from a cdylib: DPI-C is itself an in-process call, so the static
//! path is the like-for-like comparison, and it keeps the harness free of a
//! separate component crate build.

use std::path::PathBuf;
use veryl_analyzer::ir as air;
use veryl_analyzer::{Analyzer, Context, symbol_table};
use veryl_component::{
    BuildCtx, Component, ComponentKind, InputPort, OutputPort, Result as CompResult, SimCtx,
};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_simulator::component::loader::register_static_component;
use veryl_simulator::ir::{self, Event};
use veryl_simulator::testbench::{
    build_clock_periods, build_event_map, convert_initial_to_testbench, run_testbench,
};
use veryl_simulator::{Config, Simulator};

const TOP: &str = "comp_test";

/// Accumulates its input across clock edges — the exact counterpart of the
/// C model reached through DPI in `accumulator.c`, kept deliberately cheap
/// so the boundary crossing dominates the measurement.
struct Accumulator {
    d: InputPort,
    q: OutputPort,
    acc: u64,
}

impl Component for Accumulator {
    const KIND: ComponentKind = ComponentKind::Clocked;

    fn new(ctx: &mut BuildCtx) -> CompResult<Self> {
        ctx.clock("clk")?;
        Ok(Self {
            d: ctx.input("d")?,
            q: ctx.output("q")?,
            acc: 0,
        })
    }

    fn on_clock(&mut self, ctx: &mut SimCtx) -> CompResult<()> {
        let d = ctx.read(self.d).as_u64()?;
        self.acc = self.acc.wrapping_add(d) & 0xffff_ffff;
        ctx.write(self.q, self.acc);
        Ok(())
    }
}

static ACCUMULATOR: veryl_component::sys::VrlComponentVTable =
    veryl_component::export::vtable::<Accumulator>();

#[derive(clap::Parser)]
struct Opt {
    path: PathBuf,
    #[arg(long)]
    use_4state: bool,
    #[arg(long)]
    use_jit: bool,
}

fn main() {
    let opt = <Opt as clap::Parser>::parse();

    register_static_component("accumulator", &ACCUMULATOR);
    symbol_table::clear();

    let code = std::fs::read_to_string(&opt.path).unwrap();

    let metadata = Metadata::create_default("prj").unwrap();

    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    veryl_analyzer::tb_component::insert_external_components(&["accumulator"]);
    let mut context = Context::default();

    let mut ir = air::Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir));
    Analyzer::analyze_post_pass2(&ir);

    let config = Config {
        use_jit: opt.use_jit,
        use_4state: opt.use_4state,
        ..Default::default()
    };

    let built = ir::build_ir(&ir, TOP.into(), &config).expect("failed to build component IR");

    let mut sim = Simulator::new(built, None);
    sim.init_components(0, TOP).unwrap();

    let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
    let clock_periods = build_clock_periods(&sim.ir.event_statements);
    let stmts = sim.ir.event_statements.get(&Event::Initial).unwrap();
    let tb_stmts = convert_initial_to_testbench(stmts, &event_map, &clock_periods, 3);
    run_testbench(&mut sim, &tb_stmts);

    print!("{}", sim.ir.dump_variables());
}
