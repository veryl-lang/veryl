use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use std::fs;
use veryl_formatter::formatter::Formatter;
use veryl_parser::veryl_grammar::VerylGrammar;
use veryl_parser::veryl_parser::parse;

fn criterion_benchmark(c: &mut Criterion) {
    let mut text = String::new();
    for testcase in TESTCASES {
        let input = fs::read_to_string(testcase).unwrap();
        text.push_str(&input);
    }
    let text = text.repeat(10);
    let mut grammar = VerylGrammar::new();
    let mut formatter = Formatter::new();

    let mut group = c.benchmark_group("throughput");
    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("parse", |b| {
        b.iter(|| parse(black_box(&text), "", &mut grammar))
    });
    group.bench_function("format", |b| {
        b.iter(|| {
            let mut grammar = VerylGrammar::new();
            let _ = parse(black_box(&text), "", &mut grammar);
            formatter.format(&grammar.veryl.unwrap());
        })
    });
    group.finish();
}

include!(concat!(env!("OUT_DIR"), "/test.rs"));

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
