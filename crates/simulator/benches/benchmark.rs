#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::sync::LazyLock;
use veryl_analyzer::ir as air;
use veryl_analyzer::{Analyzer, Context, symbol_table};
use veryl_component::{
    BuildCtx, Component, ComponentKind, InputPort, OutputPort, Result as CompResult, SimCtx,
};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_simulator::component::loader::register_static_component;
use veryl_simulator::ir::{self, Event, Ir};
use veryl_simulator::testbench::{
    build_clock_periods, build_event_map, convert_initial_to_testbench, run_testbench,
};
use veryl_simulator::{Config, Simulator};

#[cfg(target_os = "linux")]
mod perf;

const COUNTER_CODE: &str = include_str!("../compare/counter/test.veryl");
const WALLACE_CODE: &str = include_str!("../compare/wallace/test.veryl");
const LFSR256_CODE: &str = include_str!("../compare/lfsr256/test.veryl");

struct BenchDesign {
    name: &'static str,
    code: &'static str,
    cycle: u64,
}

const DESIGNS: &[BenchDesign] = &[
    BenchDesign {
        name: "counter",
        code: COUNTER_CODE,
        cycle: 100_000,
    },
    BenchDesign {
        name: "wallace",
        code: WALLACE_CODE,
        cycle: 100_000,
    },
    BenchDesign {
        name: "lfsr256",
        code: LFSR256_CODE,
        cycle: 100_000,
    },
];

fn build(code: &str, top: &str) -> Ir {
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(black_box(code), &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut ir = air::Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir));

    analyzer.clear();

    let config = Config {
        use_jit: true,
        ..Default::default()
    };

    ir::build_ir(&ir, top.into(), &config).expect("Failed to build IR")
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput");
    for design in DESIGNS {
        group.throughput(Throughput::Elements(design.cycle));
        group.bench_function(design.name, |b| {
            b.iter_with_large_drop(|| {
                let ir = build(black_box(design.code), "Top");
                let mut sim = Simulator::new(ir, None);
                let clk = sim.get_clock("clk").unwrap();
                let rst = sim.get_reset("rst").unwrap();
                sim.step(&rst);
                for _ in 0..design.cycle {
                    sim.step(&clk);
                }
            })
        });
    }

    // Component-driven design: every edge additionally crosses the user
    // component boundary (input staging + output apply), unlike the
    // pure-RTL designs above.
    const COMPONENT_CYCLES: u64 = 100_000;
    let component = component_code(COMPONENT_CYCLES);
    group.throughput(Throughput::Elements(COMPONENT_CYCLES));
    group.bench_function("component", |b| {
        b.iter_with_large_drop(|| drive_component(build_component(black_box(&component))))
    });

    group.finish();
}

/// In-process mirror component: reads its input each edge and drives it
/// straight back out, so every clock crosses the component boundary in
/// both directions (input staging + output apply).
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

static MIRROR: veryl_component::sys::VrlComponentVTable =
    veryl_component::export::vtable::<Mirror>();

static REGISTER: LazyLock<()> = LazyLock::new(|| register_static_component("mirror", &MIRROR));

const COMPONENT_WIDTH: u32 = 128;

/// A counter feeding a mirror through a user component: the mirror's port
/// is `COMPONENT_WIDTH` bits, so the four-state masking gate (`use_4state`)
/// governs a multi-word copy per edge — building the same design in two-
/// and four-state form measures what that gate costs.
fn component_code(cycles: u64) -> String {
    let width = COMPONENT_WIDTH;
    format!(
        r#"
    module Counter (
        clk: input clock,
        rst: input reset,
        cnt: output logic<{width}>,
    ) {{
        always_ff {{
            if_reset {{ cnt = 0; }}
            else {{ cnt += 1; }}
        }}
    }}

    #[test(comp_test)]
    module comp_test {{
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var cnt: logic<{width}>;
        var q: logic<{width}>;

        inst dut: Counter (clk, rst, cnt);
        inst mirror: $comp::mirror ( clk, d: cnt, q );

        initial {{
            rst.assert();
            clk.next({cycles});
            $finish();
        }}
    }}
    "#
    )
}

fn build_component(code: &str) -> Ir {
    LazyLock::force(&REGISTER);
    symbol_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();

    let parser = Parser::parse(black_box(code), &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    veryl_analyzer::tb_component::insert_external_components(&["mirror"]);
    let mut context = Context::default();

    let mut ir = air::Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir));
    Analyzer::analyze_post_pass2(&ir);

    let config = Config {
        use_jit: true,
        ..Default::default()
    };

    ir::build_ir(&ir, "comp_test".into(), &config).expect("Failed to build component IR")
}

/// Drives the `#[test]` module's initial block (a long `clk.next`) through
/// the real testbench machinery, returning the simulator so its teardown
/// falls outside the timed region.
fn drive_component(ir: Ir) -> Simulator {
    let mut sim = Simulator::new(ir, None);
    sim.init_components(0, "comp_test").unwrap();

    let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
    let clock_periods = build_clock_periods(&sim.ir.event_statements);
    let stmts = sim.ir.event_statements.get(&Event::Initial).unwrap();
    let tb_stmts = convert_initial_to_testbench(stmts, &event_map, &clock_periods, 3);

    black_box(run_testbench(&mut sim, &tb_stmts));
    sim
}

#[cfg(target_os = "linux")]
criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(perf::FlamegraphProfiler::new(100));
    targets = criterion_benchmark
}

#[cfg(not(target_os = "linux"))]
criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = criterion_benchmark
}

criterion_main!(benches);
