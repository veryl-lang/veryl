// CodSpeed walltime benchmark: 1-core Linux boot of the heliodor RV64GC SoC.
//
// Drives the `test_soc_linux_boot` native testbench (heliodor submodule under
// testcases/heliodor) on the in-tree simulator, measuring the full boot to SBI
// shutdown. It also guards correctness: the run must pass.
//
// heliodor is a git submodule; the benchmark is skipped when it isn't checked out.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use std::time::Duration;
use veryl_analyzer::ir as air;
use veryl_analyzer::{Analyzer, AnalyzerError, Context};
use veryl_metadata::Metadata;
use veryl_parser::Parser;
use veryl_parser::resource_table;
use veryl_simulator::ir::{Config, build_ir};
use veryl_simulator::testbench::{TestResult, run_native_testbench};

const TOP: &str = "test_soc_linux_boot";

// Keep only error-severity diagnostics; heliodor emits warnings (e.g.
// missing_reset_statement on memory arrays) that the real `veryl build` tolerates.
fn assert_no_errors(stage: &str, errors: Vec<AnalyzerError>) {
    let errors: Vec<_> = errors.into_iter().filter(AnalyzerError::is_error).collect();
    assert!(errors.is_empty(), "{stage}: {errors:?}");
}

// Parse + analyze the whole heliodor project (its sources + std) into the
// analyzer IR. Done once; the simulator IR is rebuilt per measured iteration.
fn analyze_heliodor(root: &Path) -> air::Ir {
    let metadata_path = Metadata::search_from(root).unwrap();
    let mut metadata = Metadata::load(&metadata_path).unwrap();
    let paths = metadata.paths::<PathBuf>(&[], false, true).unwrap();

    let mut contexts = Vec::new();
    for path in &paths {
        let input = std::fs::read_to_string(&path.src).unwrap();
        let parser = Parser::parse(&input, &path.src).unwrap();
        let analyzer = Analyzer::new(&metadata);
        assert_no_errors("pass1", analyzer.analyze_pass1(&path.prj, &parser.veryl));
        contexts.push((path, parser, analyzer));
    }

    assert_no_errors("post_pass1", Analyzer::analyze_post_pass1());

    let mut context = Context::default();
    let mut ir = air::Ir::default();
    for (path, parser, analyzer) in &contexts {
        let errors = analyzer.analyze_pass2(&path.prj, &parser.veryl, &mut context, Some(&mut ir));
        assert_no_errors("pass2", errors);
    }

    assert_no_errors("post_pass2", Analyzer::analyze_post_pass2(&ir));

    ir
}

fn criterion_benchmark(c: &mut Criterion) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testcases/heliodor");
    let root = match root.canonicalize() {
        Ok(p) if p.join("Veryl.toml").exists() => p,
        _ => {
            eprintln!(
                "skipping heliodor_boot benchmark: submodule not checked out at {}",
                root.display()
            );
            return;
        }
    };

    // `$readmemh` in the testbench resolves firmware / kernel paths relative to
    // the current working directory.
    std::env::set_current_dir(&root).unwrap();

    let air_ir = analyze_heliodor(&root);

    // Use the `cc` backend, like `veryl test`'s default: the pure Cranelift JIT
    // is far too slow for the full boot. Synchronous compile (aot_c_async = false)
    // avoids a mid-run Cranelift -> cc hot-swap, keeping the measurement deterministic.
    let config = Config {
        use_jit: true,
        aot_c: true,
        aot_c_event: true,
        aot_c_async: false,
        aot_c_validate: false,
        aot_c_min_stmts: 0,
        ..Config::default()
    };

    let top = resource_table::get_str_id(TOP.to_string()).expect("top module not found");

    let mut group = c.benchmark_group("heliodor");
    // One iteration runs the whole boot (a single long run), so cap the sample
    // count at the criterion minimum to keep a local `cargo bench` bounded; under
    // `cargo codspeed` the CodSpeed runner controls the measurement.
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(60));
    group.bench_function("linux_boot_1core", |b| {
        b.iter_batched(
            || build_ir(&air_ir, top, &config).expect("build_ir failed"),
            |sim_ir| {
                let result =
                    run_native_testbench(sim_ir, None, TOP.to_string()).expect("testbench error");
                assert_eq!(result, TestResult::Pass, "heliodor linux boot did not pass");
            },
            BatchSize::PerIteration,
        )
    });
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
