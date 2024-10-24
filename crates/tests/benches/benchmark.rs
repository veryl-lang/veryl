use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use std::fs;
use veryl_analyzer::Analyzer;
use veryl_formatter::Formatter;
use veryl_metadata::Metadata;
use veryl_parser::Parser;

#[cfg(target_os = "linux")]
mod perf;

const EXCLUDES: [&str; 1] = [r"52_include.veryl"];

fn criterion_benchmark(c: &mut Criterion) {
    let mut text = String::new();
    for testcase in TESTCASES {
        if EXCLUDES.iter().any(|x| testcase.contains(x)) {
            continue;
        }
        let input = fs::read_to_string(testcase).unwrap();
        text.push_str(&input);
    }

    let metadata_path = Metadata::search_from_current().unwrap();
    let metadata = Metadata::load(&metadata_path).unwrap();

    let mut group = c.benchmark_group("throughput");
    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("parse", |b| {
        b.iter_with_large_drop(|| Parser::parse(black_box(&text), &""))
    });
    group.bench_function("analyze", |b| {
        b.iter_with_large_drop(|| {
            let parser = Parser::parse(black_box(&text), &"").unwrap();
            let prj = &metadata.project.name;
            let analyzer = Analyzer::new(black_box(&metadata));
            analyzer.analyze_pass1(prj, black_box(&text), &"", &parser.veryl);
            Analyzer::analyze_post_pass1();
            analyzer.analyze_pass2(prj, black_box(&text), &"", &parser.veryl);
            analyzer.analyze_pass3(prj, black_box(&text), &"", &parser.veryl);
        })
    });
    group.bench_function("format", |b| {
        b.iter_with_large_drop(|| {
            let parser = Parser::parse(black_box(&text), &"").unwrap();
            let mut formatter = Formatter::new(&metadata);
            formatter.format(&parser.veryl);
        })
    });
    group.finish();
}

include!(concat!(env!("OUT_DIR"), "/test.rs"));

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
