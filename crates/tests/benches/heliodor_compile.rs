// CodSpeed memory benchmark: parse + analyze of the large heliodor design, to
// profile compiler memory use on a realistic project (emit/SV generation is not
// measured). Skipped when the heliodor submodule (testcases/heliodor) isn't
// checked out.

use criterion::{Criterion, IterManualOptions, black_box, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use veryl_analyzer::ir::Ir;
use veryl_analyzer::{Analyzer, AnalyzerError, Context};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_path::PathSet;

// heliodor emits warnings (e.g. missing_reset_statement) that `veryl build`
// tolerates; only error-severity diagnostics are a failure.
fn assert_no_errors(stage: &str, errors: Vec<AnalyzerError>) {
    let errors: Vec<_> = errors.into_iter().filter(AnalyzerError::is_error).collect();
    assert!(errors.is_empty(), "{stage}: {errors:?}");
}

// Parse + fully analyze every source.
fn parse_and_analyze(metadata: &Metadata, sources: &[(PathSet, String)]) {
    let analyzer = Analyzer::new(metadata);
    let mut veryls = Vec::with_capacity(sources.len());
    for (path, input) in sources {
        let parser = Parser::parse(input, &path.src).unwrap();
        assert_no_errors("pass1", analyzer.analyze_pass1(&path.prj, &parser.veryl));
        veryls.push(parser.veryl);
    }
    assert_no_errors("post_pass1", Analyzer::analyze_post_pass1());

    let mut context = Context::default();
    let mut ir = Ir::default();
    for ((path, _), veryl) in sources.iter().zip(&veryls) {
        assert_no_errors(
            "pass2",
            analyzer.analyze_pass2(&path.prj, veryl, &mut context, Some(&mut ir)),
        );
    }
    assert_no_errors("post_pass2", Analyzer::analyze_post_pass2(&ir));
    black_box(&veryls);
}

fn criterion_benchmark(c: &mut Criterion) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testcases/heliodor");
    let root = match root.canonicalize() {
        Ok(p) if p.join("Veryl.toml").exists() => p,
        _ => {
            eprintln!(
                "skipping heliodor_compile benchmark: submodule not checked out at {}",
                root.display()
            );
            return;
        }
    };

    let metadata_path = Metadata::search_from(&root).unwrap();
    let mut metadata = Metadata::load(&metadata_path).unwrap();
    let paths = metadata.paths::<PathBuf>(&[], false, true).unwrap();
    // Read sources once (unmeasured); the benchmarks profile compilation, not I/O.
    let sources: Vec<(PathSet, String)> = paths
        .into_iter()
        .map(|p| {
            let input = std::fs::read_to_string(&p.src).unwrap();
            (p, input)
        })
        .collect();

    let mut group = c.benchmark_group("heliodor");
    group.bench_function("analyze", |b| {
        // Under CodSpeed this runs the routine exactly once (opts ignored, no
        // warmup); `iter` would run it WARMUP_RUNS+1 = 6 times, multiplying the
        // Valgrind-instrumented memory measurement. `rounds(10)` only applies to a
        // local `cargo bench` (criterion needs >1 sample for its stats).
        b.iter_manual_unstable(IterManualOptions::new().rounds(10), || {
            Analyzer::new(&metadata).clear();
            parse_and_analyze(&metadata, &sources);
        })
    });
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
