use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use veryl_analyzer::ir as air;
use veryl_analyzer::{Analyzer, Context};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_simulator::ir::{self, Ir};
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
    let parser = Parser::parse(black_box(&code), &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut ir = air::Ir::default();
    analyzer.analyze_pass1(&"prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir));

    analyzer.clear();

    let mut config = Config::default();
    config.use_jit = true;

    ir::build_ir(ir, top.into(), &config).unwrap()
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput");
    for design in DESIGNS {
        group.throughput(Throughput::Elements(design.cycle));
        group.bench_function(design.name, |b| {
            b.iter_with_large_drop(|| {
                let ir = build(black_box(design.code), "Top");
                let mut sim = Simulator::<std::io::Empty>::new(ir, None);
                let clk = sim.get_clock("clk").unwrap();
                let rst = sim.get_reset("rst").unwrap();
                sim.step(&rst);
                for _ in 0..design.cycle {
                    sim.step(&clk);
                }
            })
        });
    }
    group.finish();
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
