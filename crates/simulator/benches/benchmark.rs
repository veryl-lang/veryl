use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use veryl_analyzer::ir::Ir;
use veryl_analyzer::{Analyzer, Context};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_simulator::Simulator;

#[cfg(target_os = "linux")]
mod perf;

fn build_ir(code: &str) -> Ir {
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(black_box(&code), &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut ir = Ir::default();
    analyzer.analyze_pass1(&"prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir));

    analyzer.clear();

    ir
}

fn criterion_benchmark(c: &mut Criterion) {
    let code = r#"
    module Top #(
        param N: u32 = 10,
    )(
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>[N],
    ) {
        for i in 0..N: g {
            always_ff {
                if_reset {
                    cnt[i] = 0;
                } else {
                    cnt[i] += 1;
                }
            }
        }
    }
    "#;

    let mut group = c.benchmark_group("startup");
    group.throughput(Throughput::Bytes(code.len() as u64));
    group.bench_function("simple counter", |b| {
        b.iter_with_large_drop(|| {
            let _ = build_ir(black_box(&code));
        })
    });
    group.finish();

    let cycle = 100000;

    let mut group = c.benchmark_group("throughput");
    group.throughput(Throughput::Elements(cycle));
    group.bench_function("simple counter", |b| {
        b.iter_with_large_drop(|| {
            let ir = build_ir(&code);
            let mut sim = Simulator::<std::io::Empty>::new("Top", ir, None).unwrap();
            let clk = sim.get_clock("clk").unwrap();
            let rst = sim.get_reset("rst").unwrap();

            sim.step(&rst);

            for _ in 0..cycle {
                sim.step(&clk);
            }
        })
    });
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
