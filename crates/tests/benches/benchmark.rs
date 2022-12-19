use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use std::fs;
use veryl_formatter::formatter::Formatter;
use veryl_parser::parser::Parser;

fn criterion_benchmark(c: &mut Criterion) {
    let mut text = String::new();
    for testcase in TESTCASES {
        let input = fs::read_to_string(testcase).unwrap();
        text.push_str(&input);
    }
    let mut formatter = Formatter::new();

    let mut group = c.benchmark_group("throughput");
    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("parse", |b| b.iter(|| Parser::parse(black_box(&text), &"")));
    group.bench_function("format", |b| {
        b.iter(|| {
            let parser = Parser::parse(black_box(&text), &"").unwrap();
            formatter.format(&parser.veryl);
        })
    });
    group.finish();
}

include!(concat!(env!("OUT_DIR"), "/test.rs"));

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
