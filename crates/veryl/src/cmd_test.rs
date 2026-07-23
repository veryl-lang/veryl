use crate::cmd_build::CmdBuild;
use crate::runner::{Cocotb, CocotbSource, Dsim, Vcs, Verilator, Vivado};
use crate::{Format, OptBuild, OptTest, check_format_version};
use log::{error, info, warn};
use miette::Result;
use std::path::PathBuf;
use veryl_analyzer::symbol::TestType;
use veryl_analyzer::symbol_table;
use veryl_metadata::WaveFormFormat;
use veryl_metadata::{ComponentBackendKind, FilelistType, Metadata, SimType, WaveFormTarget};
use veryl_parser::resource_table::{self, PathId};
use veryl_parser::text_table;
use veryl_simulator::ir::{ComponentLibrary, Config, Ir, ProtoModuleCache, build_ir_cached};
use veryl_simulator::output_buffer;
use veryl_simulator::simulator::Simulator;
use veryl_simulator::simulator_error::SimulatorError;
use veryl_simulator::testbench::{TestResult, run_native_testbench};
use veryl_simulator::wave_dumper::WaveDumper;
use veryl_simulator::wavedrom::{self, SignalKind, classify_signals, parse_wavedrom};

/// A fresh random base seed for a test run, used when neither `--seed` nor
/// `[test].seed` pins one. A new `RandomState` draws OS entropy at process
/// start, so this avoids pulling in an RNG dependency.
fn random_seed() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    RandomState::new().hash_one(std::process::id())
}

/// Emitted by `veryl test --format json`.
#[derive(serde::Serialize)]
struct TestSuiteReport {
    /// Bump on any breaking change to the report shape.
    format_version: u32,
    backend: String,
    passed: i32,
    failed: i32,
    ignored: usize,
    tests: Vec<TestReport>,
}

#[derive(serde::Serialize)]
struct TestReport {
    name: String,
    /// "pass" | "fail" | "error"
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    runtime_s: f64,
    /// Captured `$display`/`$write` output.
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
}

/// Load per-test run times recorded by a prior `veryl test`, used to dispatch
/// the slowest tests first. A missing or corrupt file (or bad line) is ignored,
/// so the first run falls back to declaration order.
fn load_test_timings(path: &std::path::Path) -> std::collections::HashMap<String, f64> {
    let mut map = std::collections::HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in content.lines() {
        let mut it = line.split_whitespace();
        if let (Some(name), Some(secs), None) = (it.next(), it.next(), it.next())
            && let Ok(secs) = secs.parse::<f64>()
        {
            map.insert(name.to_string(), secs);
        }
    }
    map
}

/// Persist per-test run times for the next run's scheduling.  `fresh` overwrites
/// `prior` per test, so entries for tests skipped by a `--test` filter survive.
fn save_test_timings(
    path: &std::path::Path,
    prior: &std::collections::HashMap<String, f64>,
    fresh: &[(String, f64)],
) {
    let mut merged = prior.clone();
    for (name, secs) in fresh {
        merged.insert(name.clone(), *secs);
    }
    let mut lines: Vec<String> = merged
        .iter()
        .map(|(name, secs)| format!("{name} {secs:.6}"))
        .collect();
    lines.sort();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, lines.join("\n"));
}

pub struct CmdTest {
    opt: OptTest,
}

struct NativeTestJob {
    module_name: String,
    sim_ir: Ir,
    dump: Option<WaveDumper>,
}

enum NativeOutcome {
    ElaborateFailed(SimulatorError),
    Ran {
        result: std::result::Result<TestResult, SimulatorError>,
        wave_path: Option<PathBuf>,
    },
}

struct PendingNativeTest {
    test_name: String,
    top: Option<resource_table::StrId>,
    test_path: PathId,
}

fn wave_output_path(name: &str, test_path: PathId, metadata: &Metadata) -> PathBuf {
    let target_name = format!("{}.{}", name, metadata.test.waveform_format.extension());
    match &metadata.test.waveform_target {
        WaveFormTarget::Target => PathBuf::from(test_path.to_string())
            .parent()
            .unwrap()
            .join(target_name),
        WaveFormTarget::Directory { path } => path.join(target_name),
    }
}

fn create_wave_dumper(
    name: &str,
    test_path: PathId,
    metadata: &Metadata,
) -> std::result::Result<WaveDumper, SimulatorError> {
    let path = wave_output_path(name, test_path, metadata);
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|e| SimulatorError::IoError {
            message: format!("failed to create directory {}: {e}", parent.display()),
        })?;
    }
    let path_str = path.to_string_lossy().to_string();
    info!("  Dumping waveform to {}", path_str);
    let dumper = match metadata.test.waveform_format {
        WaveFormFormat::Vcd => {
            let file = std::fs::File::create(&path).map_err(|e| SimulatorError::IoError {
                message: format!("failed to create waveform file {}: {e}", path.display()),
            })?;
            WaveDumper::new_vcd(Box::new(file))
        }
        WaveFormFormat::Fst => WaveDumper::new_fst(&path_str),
    };
    Ok(dumper.with_path(path))
}

impl CmdTest {
    pub fn new(opt: OptTest) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        // force filelist_type to absolute which can be refered from temporary directory
        metadata.build.filelist_type = FilelistType::Absolute;

        let build = CmdBuild::new(OptBuild {
            files: self.opt.files.clone(),
            check: false,
            out_dir: None,
        });

        // Mutate metadata so external simulator runners (which read
        // metadata.test.defines directly) also see CLI --define overrides.
        for name in &self.opt.define {
            if !metadata.test.defines.contains(name) {
                metadata.test.defines.push(name.clone());
            }
        }
        let combined_defines = metadata.test.defines.clone();

        // `[[components]]` cargo packages build before analysis: the analyzer
        // reads their interface manifests, so building afterwards would
        // leave the checks one edit behind — and a stale-manifest error
        // would then block the very run that refreshes the manifest.
        // Dependency components live behind the lockfile, which `Metadata`
        // loads lazily; resolve it now (the later build reuses the result).
        if let Err(e) = metadata.update_lockfile() {
            warn!("Failed to update lockfile: {e}");
        }
        let dependency_components = match metadata.collect_dependency_components() {
            Ok(x) => x,
            Err(e) => {
                warn!("Failed to collect dependency components: {e}");
                vec![]
            }
        };
        let component_libraries =
            if !metadata.components.is_empty() || !dependency_components.is_empty() {
                Some(build_component_libraries(metadata, &dependency_components))
            } else {
                None
            };

        let mut ir = veryl_analyzer::ir::Ir::default();
        build.exec(
            metadata,
            true,
            false,
            Some(&mut ir),
            self.opt.test.as_deref(),
            &combined_defines,
        )?;

        let tests = symbol_table::get_tests(&metadata.project.name);
        let doc_tests = symbol_table::get_doc_tests(&metadata.project.name);

        let total_tests = tests.len();
        let tests: Vec<_> = if self.opt.include_ignored {
            tests
        } else if self.opt.ignored {
            tests
                .into_iter()
                .filter(|(_, property)| property.ignored)
                .collect()
        } else {
            tests
                .into_iter()
                .filter(|(_, property)| !property.ignored)
                .collect()
        };
        let ignored_count = total_tests - tests.len();

        if ignored_count > 0 {
            info!("{ignored_count} test(s) ignored");
        }

        let (tests, doc_tests) = if let Some(ref filter) = self.opt.test {
            let tests: Vec<_> = tests
                .into_iter()
                .filter(|(test, _)| {
                    let name = test.to_string();
                    name.contains(filter.as_str())
                })
                .collect();

            let doc_tests: Vec<_> = doc_tests
                .into_iter()
                .filter(|dt| {
                    let name = dt.module_name.to_string();
                    name.contains(filter.as_str())
                })
                .collect();

            if tests.is_empty() && doc_tests.is_empty() {
                warn!("No tests matched filter '{filter}'");
            }

            (tests, doc_tests)
        } else {
            (tests, doc_tests)
        };

        let sim_type = if let Some(x) = self.opt.sim {
            x.into()
        } else {
            metadata.test.simulator
        };

        // `cc` keeps use_jit=true so Cranelift covers stmts it can't emit;
        // --backend-validate forces the synchronous dual-run.
        use crate::Backend;
        let validate = self.opt.backend_validate.is_some();
        let validate_stride = self.opt.backend_validate.unwrap_or(0);
        let (use_jit, aot_c) = match self.opt.backend {
            Backend::Interpret => (false, false),
            Backend::Cranelift => (true, false),
            Backend::Cc => (true, true),
        };
        let mut config = Config {
            use_jit,
            disable_ff_opt: self.opt.disable_ff_opt,
            aot_c,
            aot_c_event: aot_c,
            aot_c_async: aot_c && !validate,
            aot_c_validate: aot_c && validate,
            aot_c_validate_stride: validate_stride,
            // No size floor: the compile pool (emit.rs) now caps concurrent
            // `cc`, so the old 256 flood workaround is obsolete — and a small
            // module that runs long benefits from `cc` too.  Override with
            // VERYL_AOT_C_MIN_STMTS to restore a floor.
            aot_c_min_stmts: 0,
            seed: self
                .opt
                .seed
                .or(metadata.test.seed)
                .unwrap_or_else(random_seed),
            use_4state: self.opt.four_state || metadata.test.four_state,
            ..Config::default()
        };
        config.apply_env();
        // Warn once if cc is requested but absent; the fallback is otherwise silent.
        #[cfg(not(target_family = "wasm"))]
        if config.aot_c && !veryl_simulator::backend::aot_c::cc_available() {
            warn!(
                "--backend cc: no C compiler found (set VERYL_AOT_CC, or install cc/gcc); \
                 falling back to the Cranelift JIT backend"
            );
        }
        let mut proto_cache = ProtoModuleCache::default();

        check_format_version(self.opt.format, self.opt.format_version)?;
        let json = matches!(self.opt.format, Format::Json);
        let backend_name = match self.opt.backend {
            Backend::Interpret => "interpret",
            Backend::Cranelift => "cranelift",
            Backend::Cc => "cc",
        };
        // Native workers push concurrently, so guard the per-test results.
        let reports = std::sync::Mutex::new(Vec::<TestReport>::new());

        let mut success = 0;
        let mut failure = 0;

        let mut pending_native: Vec<PendingNativeTest> = Vec::new();
        let mut non_native_tests = Vec::new();

        for (test, property) in &tests {
            match property.r#type {
                TestType::Native => {
                    pending_native.push(PendingNativeTest {
                        test_name: test.to_string(),
                        top: property.top,
                        test_path: property.path,
                    });
                }
                _ => {
                    non_native_tests.push((test, property));
                }
            }
        }

        if !pending_native.is_empty() {
            info!("Test seed: {} (reproduce with --seed)", config.seed);
            if let Some(libraries) = component_libraries {
                config.component_libraries = libraries;
                config.component_file_base = Some(metadata.project_path());
            }

            let num_threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(pending_native.len());
            // Buffer `$display` output to keep concurrent tests from interleaving.
            // A single worker can't interleave, so stream live; `--no-capture`
            // forces streaming even in parallel.
            // JSON mode also buffers: capture output into the report, keep stdout clean.
            let buffered = (num_threads > 1 && !self.opt.no_capture) || json;
            if buffered {
                info!("Building simulation model");
            }
            // Longest-first scheduling: dispatch the slowest tests first so a
            // long test can't land in the final wave and stretch the tail.
            // Reordering is safe — `instance_seed` derives from test/instance
            // name, not dispatch order. Tests with no history sort first, so a
            // possibly-slow new test is measured early.
            let timings_path = metadata.project_dot_build_path().join("test_timings");
            let prior_timings = load_test_timings(&timings_path);
            pending_native.sort_by(|a, b| {
                let ta = prior_timings.get(&a.test_name).copied();
                let tb = prior_timings.get(&b.test_name).copied();
                match (ta, tb) {
                    (None, None) => a.test_name.cmp(&b.test_name),
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
                }
            });
            if config.dut_reuse {
                let tops: Vec<_> = pending_native.iter().filter_map(|p| p.top).collect();
                veryl_simulator::backend::inst::compute_recurring_set(&ir, &tops);
            }
            let pending_queue = std::sync::Mutex::new(pending_native.into_iter());
            let resource_snapshot = resource_table::export_tables();
            // `SimulatorError` snapshots source text from `text_table` eagerly
            // at construction time; without this, worker errors carry no source.
            let text_snapshot = text_table::export_tables();

            let ir_ref = &ir;
            let config_ref = &config;
            let opt_ref = &self.opt;
            let metadata_ref: &Metadata = metadata;
            // Workers print each finished test's block under this lock, so results
            // stream as tests complete without interleaving between workers.
            let print_lock = std::sync::Mutex::new(());
            type ThreadTally = (i32, i32, Vec<PathBuf>, Vec<(String, f64)>);
            let results: Vec<ThreadTally> = std::thread::scope(|s| {
                let queue = &pending_queue;
                let resource_snap = &resource_snapshot;
                let text_snap = &text_snapshot;
                let print_lock = &print_lock;
                let reports = &reports;
                let handles: Vec<_> = (0..num_threads)
                    .map(|_| {
                        s.spawn(move || {
                            resource_table::import_tables(resource_snap);
                            text_table::import_tables(text_snap);
                            // Per-thread cache avoids locking; cross-test
                            // reuse is rare since each top name is unique.
                            let mut thread_cache = ProtoModuleCache::default();
                            let (mut tally_pass, mut tally_fail) = (0, 0);
                            let mut tally_waves: Vec<PathBuf> = Vec::new();
                            let mut tally_timings: Vec<(String, f64)> = Vec::new();
                            loop {
                                let pending = queue.lock().unwrap().next();
                                let Some(pending) = pending else { break };
                                if buffered {
                                    output_buffer::enable();
                                }
                                // With the `profile` feature, report the build
                                // (IR build + conv/AOT/dlopen) vs run (Simulator::new
                                // + hex + cycle sim) split.  The run boundary matches
                                // what a warm Verilator binary re-runs (model build
                                // mtime-skipped), so the two are comparable.  Zero
                                // overhead when the feature is off.
                                #[cfg(feature = "profile")]
                                let t_build = std::time::Instant::now();
                                let t0 = std::time::Instant::now();
                                let build_result = prepare_native_test(
                                    ir_ref,
                                    &pending.test_name,
                                    &pending.top,
                                    opt_ref,
                                    pending.test_path,
                                    metadata_ref,
                                    config_ref,
                                    &mut thread_cache,
                                );
                                #[cfg(feature = "profile")]
                                let build_el = t_build.elapsed();
                                #[cfg(feature = "profile")]
                                let t_run = std::time::Instant::now();
                                let mut run_secs: Option<f64> = None;
                                let outcome = match build_result {
                                    Ok(job) => {
                                        let wave_path =
                                            job.dump.as_ref().and_then(|d| d.path().cloned());
                                        if !buffered {
                                            info!("Executing test ({})", pending.test_name);
                                        }
                                        // Time the run (not the build) — the build is
                                        // amortized by the cross-test chunk cache, so
                                        // run time is the stable per-test cost that
                                        // longest-first scheduling sorts on.
                                        let t_run_sched = std::time::Instant::now();
                                        let result = run_native_testbench(
                                            job.sim_ir,
                                            job.dump,
                                            job.module_name,
                                        );
                                        run_secs = Some(t_run_sched.elapsed().as_secs_f64());
                                        NativeOutcome::Ran { result, wave_path }
                                    }
                                    Err(e) => NativeOutcome::ElaborateFailed(e),
                                };
                                if let Some(secs) = run_secs {
                                    tally_timings.push((pending.test_name.clone(), secs));
                                }
                                #[cfg(feature = "profile")]
                                {
                                    let run_el = t_run.elapsed();
                                    eprintln!(
                                        "PROFILE_SPLIT test={} build_ms={:.1} run_ms={:.1}",
                                        pending.test_name,
                                        build_el.as_secs_f64() * 1e3,
                                        run_el.as_secs_f64() * 1e3
                                    );
                                }
                                let runtime_s = t0.elapsed().as_secs_f64();
                                let output = output_buffer::take();
                                let test_name = &pending.test_name;
                                let _print = print_lock.lock().unwrap();
                                let mut rep_status: &'static str = "error";
                                let mut rep_message: Option<String> = None;
                                match outcome {
                                    NativeOutcome::ElaborateFailed(e) => {
                                        // Buffered output is from IR build; emit before the diag.
                                        if !output.is_empty() && !json {
                                            print!("{output}");
                                        }
                                        error!("Failed to elaborate test ({test_name})");
                                        let rendered = format!("{:?}", miette::Report::new(e));
                                        eprintln!("{rendered}");
                                        rep_status = "error";
                                        rep_message = Some(rendered);
                                        tally_fail += 1;
                                    }
                                    NativeOutcome::Ran { result, wave_path } => {
                                        if buffered {
                                            info!("Executing test ({test_name})");
                                        }
                                        if !output.is_empty() && !json {
                                            print!("{output}");
                                        }
                                        match result {
                                            Ok(TestResult::Pass) => {
                                                info!("Succeeded test ({test_name})");
                                                rep_status = "pass";
                                                tally_pass += 1;
                                                if let Some(path) = wave_path {
                                                    tally_waves.push(path);
                                                }
                                            }
                                            Ok(TestResult::Fail(msg)) => {
                                                error!("Failed test ({test_name}): {msg}");
                                                rep_status = "fail";
                                                rep_message = Some(msg);
                                                tally_fail += 1;
                                            }
                                            Err(e) => {
                                                error!("Failed test ({test_name})");
                                                let rendered =
                                                    format!("{:?}", miette::Report::new(e));
                                                eprintln!("{rendered}");
                                                rep_status = "error";
                                                rep_message = Some(rendered);
                                                tally_fail += 1;
                                            }
                                        }
                                    }
                                }
                                if json {
                                    reports.lock().unwrap().push(TestReport {
                                        name: test_name.to_string(),
                                        status: rep_status,
                                        message: rep_message,
                                        runtime_s,
                                        output: if output.is_empty() {
                                            None
                                        } else {
                                            Some(output)
                                        },
                                    });
                                }
                                use std::io::Write;
                                let _ = std::io::stdout().flush();
                            }
                            (tally_pass, tally_fail, tally_waves, tally_timings)
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });

            // Workers already printed each result as it finished; fold their
            // tallies and register generated waveforms (needs `&mut metadata`).
            let mut fresh_timings: Vec<(String, f64)> = Vec::new();
            for (passed, failed, waves, timings) in results {
                success += passed;
                failure += failed;
                for path in waves {
                    metadata.add_generated_file(path);
                }
                fresh_timings.extend(timings);
            }
            // Persist this run's per-test times for the next run's ordering.
            save_test_timings(&timings_path, &prior_timings, &fresh_timings);
        }

        for (test, property) in non_native_tests {
            let mut runner = match property.r#type {
                TestType::Inline => match sim_type {
                    SimType::Verilator => Verilator::new().runner(),
                    SimType::Vcs => Vcs::new().runner(),
                    SimType::Dsim => Dsim::new().runner(),
                    SimType::Vivado => Vivado::new().runner(),
                },
                TestType::CocotbEmbed(ref x) => {
                    Cocotb::new(CocotbSource::Embed(x.clone())).runner()
                }
                TestType::CocotbInclude(x) => Cocotb::new(CocotbSource::Include(x)).runner(),
                TestType::Native => unreachable!(),
            };

            let t0 = std::time::Instant::now();
            let ok = runner.run(metadata, *test, property.top, property.path, self.opt.wave)?;
            let runtime_s = t0.elapsed().as_secs_f64();
            if ok {
                success += 1;
                if self.opt.wave {
                    let test_name = test.to_string();
                    let path = wave_output_path(&test_name, property.path, metadata);
                    metadata.add_generated_file(path);
                }
            } else {
                failure += 1;
            }
            if json {
                reports.lock().unwrap().push(TestReport {
                    name: test.to_string(),
                    status: if ok { "pass" } else { "fail" },
                    message: None,
                    runtime_s,
                    output: None,
                });
            }
        }

        for dt in &doc_tests {
            let module_name = dt.module_name.to_string();
            info!("Executing doc test ({module_name})");

            let t0 = std::time::Instant::now();
            let result = run_doc_test(
                &ir,
                &module_name,
                &dt.wavedrom_json,
                &dt.ports,
                self.opt.wave,
                dt.path,
                metadata,
                &config,
                &mut proto_cache,
            );
            let runtime_s = t0.elapsed().as_secs_f64();
            let (status, message) = match result {
                Ok(wave_path) => {
                    info!("Succeeded doc test ({module_name})");
                    success += 1;
                    if let Some(path) = wave_path {
                        metadata.add_generated_file(path);
                    }
                    ("pass", None)
                }
                Err(e) => {
                    let msg = e.to_string();
                    error!("Failed doc test ({module_name}): {msg}");
                    failure += 1;
                    ("fail", Some(msg))
                }
            };
            if json {
                reports.lock().unwrap().push(TestReport {
                    name: module_name.clone(),
                    status,
                    message,
                    runtime_s,
                    output: None,
                });
            }
        }

        let ignored_msg = if ignored_count > 0 {
            format!(", {ignored_count} ignored")
        } else {
            String::new()
        };
        let summary = format!("Completed tests : {success} passed, {failure} failed{ignored_msg}");

        if self.opt.wave {
            metadata
                .save_build_info()
                .map_err(|e| miette::miette!("{e}"))?;
        }

        if json {
            let report = TestSuiteReport {
                format_version: 1,
                backend: backend_name.to_string(),
                passed: success,
                failed: failure,
                ignored: ignored_count,
                tests: reports.into_inner().unwrap(),
            };
            match serde_json::to_string_pretty(&report) {
                Ok(s) => println!("{s}"),
                Err(e) => eprintln!("failed to serialize test report: {e}"),
            }
            return Ok(failure == 0);
        }

        if failure == 0 {
            info!("{summary}");
            Ok(true)
        } else {
            error!("{summary}");
            Ok(false)
        }
    }
}

/// Freshens the project's own `[[components]]` interface manifests before
/// analysis, so `$comp` names resolve on the first run of any analyzing
/// command. Dependencies are left to their committed manifests, which
/// match the locked sources by construction. A failed build drops the
/// package's stale sidecar, so analysis falls back to the committed
/// manifest rather than pre-edit interface data.
pub fn build_component_manifests(metadata: &Metadata) {
    if metadata.components.is_empty()
        || metadata.test.component_backend == Some(ComponentBackendKind::Wasm)
        || !cargo_available()
    {
        return;
    }
    let project_root = metadata.project_path();
    let target_dir = project_root.join("target/veryl-components");
    for def in &metadata.components {
        let crate_dir = project_root.join(&def.path);
        if !crate_dir.is_dir() {
            continue;
        }
        let label = def.path.display().to_string();
        if build_component_artifact(&label, &crate_dir, &target_dir, false).is_none()
            && let Some(sidecar) = veryl_metadata::component_crate_name(&crate_dir)
                .map(|n| veryl_metadata::sidecar_manifest_path(&target_dir, &n))
        {
            let _ = std::fs::remove_file(&sidecar);
        }
    }
}

fn cargo_available() -> bool {
    std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Builds every `[[components]]` cargo package (release, shared target
/// dir) of the project and its direct dependencies, and returns the
/// resolved library paths keyed by export name (`<project>::<name>` for
/// dependency components), enumerated from the chosen artifact's manifest.
/// Backend selection: `[test] component_backend` pins it; otherwise source
/// when cargo is available, with a fallback to a committed `wasm =`
/// prebuilt. A package that cannot be resolved is reported and left out,
/// so tests using its components fail at load time with an
/// unknown-component error.
fn build_component_libraries(
    metadata: &Metadata,
    dependencies: &[veryl_metadata::DependencyComponents],
) -> std::collections::HashMap<String, ComponentLibrary> {
    let project_root = metadata.project_path();
    let project_target_dir = project_root.join("target/veryl-components");

    struct ComponentJob {
        /// Package identity in messages: the declared path, prefixed by
        /// the dependency project for dependency packages.
        label: String,
        /// `$comp` name prefix (`<project>::`) for dependency components.
        prefix: String,
        crate_dir: PathBuf,
        target_dir: PathBuf,
        wasm: Option<PathBuf>,
        from_dependency: bool,
    }

    let mut jobs: Vec<ComponentJob> = Vec::new();
    for def in &metadata.components {
        jobs.push(ComponentJob {
            label: def.path.display().to_string(),
            prefix: String::new(),
            crate_dir: project_root.join(&def.path),
            target_dir: project_target_dir.clone(),
            wasm: def.wasm.as_ref().map(|w| project_root.join(w)),
            from_dependency: false,
        });
    }
    for dep in dependencies {
        for def in &dep.components {
            jobs.push(ComponentJob {
                label: format!("{}::{}", dep.project, def.path.display()),
                prefix: format!("{}::", dep.project),
                crate_dir: dep.root.join(&def.path),
                target_dir: dep.target_dir.clone(),
                wasm: def.wasm.as_ref().map(|w| dep.root.join(w)),
                from_dependency: true,
            });
        }
    }

    let choice = metadata.test.component_backend;
    let cargo_available = cargo_available();

    let mut libraries = std::collections::HashMap::new();
    for ComponentJob {
        label,
        prefix,
        crate_dir,
        target_dir,
        wasm,
        from_dependency,
    } in jobs
    {
        let use_wasm = match choice {
            Some(ComponentBackendKind::Native) => false,
            Some(ComponentBackendKind::Wasm) => {
                if wasm.is_none() {
                    error!("Component package ({label}) declares no `wasm =` prebuilt binary");
                    continue;
                }
                true
            }
            None => {
                if cargo_available {
                    false
                } else if wasm.is_some() {
                    info!("Component package ({label}): cargo not found, using the prebuilt wasm");
                    true
                } else {
                    false
                }
            }
        };

        // When the artifact itself yields no export list, the committed
        // manifest names the exports instead — the names the analyzer
        // resolved — so e.g. a wasm with stripped custom sections still
        // loads. Collisions are first-declaration-wins, mirroring
        // `Metadata::collect_component_manifests`, so the analyzer and
        // the simulator agree on which package a name means.
        let register = |libraries: &mut std::collections::HashMap<String, ComponentLibrary>,
                        exports: Option<Vec<String>>,
                        path: &PathBuf| {
            let exports = exports.or_else(|| {
                let committed = veryl_metadata::read_committed_manifests(&crate_dir)?;
                warn!(
                    "Component package ({label}) artifact carries no veryl manifest; using the committed manifest's export names"
                );
                Some(committed.into_keys().collect())
            });
            let Some(exports) = exports else {
                warn!(
                    "Component package ({label}) exports no veryl manifest; its components are unavailable"
                );
                return;
            };
            for type_name in exports {
                let key = format!("{prefix}{type_name}");
                if libraries.contains_key(&key) {
                    warn!(
                        "component `{type_name}` is exported by more than one [[components]] package; the first declaration wins"
                    );
                    continue;
                }
                libraries.insert(
                    key,
                    ComponentLibrary {
                        path: path.clone(),
                        type_name,
                    },
                );
            }
        };

        if use_wasm {
            let path = wasm.unwrap();
            if !path.exists() {
                error!(
                    "Component package ({label}) prebuilt wasm not found ({}); run `veryl publish` to generate it",
                    path.display()
                );
                continue;
            }
            // The fallback path above already announced itself.
            if choice == Some(ComponentBackendKind::Wasm) {
                info!("Component package ({label}): using the prebuilt wasm");
            }
            let Ok(bytes) = std::fs::read(&path) else {
                error!(
                    "Component package ({label}) prebuilt wasm is unreadable ({})",
                    path.display()
                );
                continue;
            };
            // Freshness is best-effort: when the sources are checked out
            // next to the prebuilt, a stale binary is worth a warning. A
            // veryl-version-only difference matters just for the project's
            // own components; a dependency's prebuilt cannot be
            // regenerated by the consumer.
            if crate_dir.is_dir()
                && let Ok(hash) = crate::component_publish::component_source_hash(&crate_dir)
                && let Some(stored) = veryl_metadata::wasm_custom_section(
                    &bytes,
                    veryl_component_sys::VRL_WASM_SOURCE_HASH_SECTION,
                )
            {
                use crate::component_publish::PrebuiltFreshness;
                match crate::component_publish::prebuilt_freshness(
                    stored,
                    &hash,
                    env!("CARGO_PKG_VERSION"),
                ) {
                    PrebuiltFreshness::Fresh => {}
                    PrebuiltFreshness::SourcesChanged if from_dependency => warn!(
                        "Component package ({label}) prebuilt wasm does not match the dependency sources; the dependency needs republishing"
                    ),
                    PrebuiltFreshness::SourcesChanged => warn!(
                        "Component package ({label}) prebuilt wasm is stale (sources changed); run `veryl publish` to regenerate it"
                    ),
                    PrebuiltFreshness::VerylVersionChanged if !from_dependency => warn!(
                        "Component package ({label}) prebuilt wasm was generated by a different veryl version; run `veryl publish` to regenerate it"
                    ),
                    PrebuiltFreshness::VerylVersionChanged => {}
                }
            }
            let exports = veryl_metadata::ComponentManifest::parse_all_from_wasm(&bytes)
                .map(|m| m.into_keys().collect());
            register(&mut libraries, exports, &path);
            continue;
        }

        match build_component_artifact(&label, &crate_dir, &target_dir, false) {
            Some((path, manifest_json)) => {
                let exports = manifest_json
                    .as_deref()
                    .map(veryl_metadata::parse_library_manifest)
                    .filter(|m| !m.is_empty())
                    .map(|m| m.into_keys().collect());
                register(&mut libraries, exports, &path);
            }
            None => {
                error!("Component package ({label}) is not available due to the build failure");
            }
        }
    }
    libraries
}

fn wasm_std_available() -> bool {
    std::process::Command::new("rustc")
        .args([
            "--print",
            "target-libdir",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .output()
        .is_ok_and(|out| {
            out.status.success()
                && PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()).exists()
        })
}

/// Runs `cargo build --release` for one component package and returns the
/// path of the produced artifact — the native cdylib, or the `.wasm`
/// binary when `wasm` is set — together with the library's aggregated
/// manifest JSON (native builds only; a wasm binary carries it in a
/// custom section). Failures are reported and return `None`.
pub(crate) fn build_component_artifact(
    name: &str,
    crate_dir: &std::path::Path,
    target_dir: &std::path::Path,
    wasm: bool,
) -> Option<(PathBuf, Option<String>)> {
    if !crate_dir.is_dir() {
        error!(
            "Component ({name}) path does not exist ({})",
            crate_dir.display()
        );
        return None;
    }
    if wasm && !wasm_std_available() {
        error!(
            "Component ({name}): the wasm32-unknown-unknown standard library is not installed; run `rustup target add wasm32-unknown-unknown`"
        );
        return None;
    }
    info!("Building component ({name})");
    let mut args = vec!["build", "--release", "--message-format=json"];
    if wasm {
        args.extend(["--target", "wasm32-unknown-unknown"]);
    }
    let output = std::process::Command::new("cargo")
        .args(&args)
        .current_dir(crate_dir)
        .env("CARGO_TARGET_DIR", target_dir)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(e) => {
            error!("Failed to run cargo for component ({name}): {e}");
            return None;
        }
    };
    if !output.status.success() {
        error!("Failed to build component ({name})");
        // In --message-format=json mode the compiler diagnostics travel on
        // stdout as JSON; render them, or the failure is opaque (stderr
        // carries only the "could not compile" summary).
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line)
                && msg["reason"] == "compiler-message"
                && let Some(rendered) = msg["message"]["rendered"].as_str()
            {
                eprint!("{rendered}");
            }
        }
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return None;
    }
    let extensions: &[&str] = if wasm {
        &[".wasm"]
    } else {
        &[".so", ".dylib", ".dll"]
    };
    let artifact = select_component_artifact(
        &String::from_utf8_lossy(&output.stdout),
        &crate_dir.join("Cargo.toml"),
        extensions,
    );
    if artifact.is_none() {
        error!(
            "Component ({name}) did not produce a cdylib; add crate-type cdylib to its Cargo.toml"
        );
    }
    // Sidecar manifest for analysis-time interface checks: extracted once
    // right after the build, while the library is fresh. Prebuilt wasm
    // carries its manifest in a custom section instead. A library without
    // a manifest removes any leftover sidecar, so a since-deleted export
    // cannot survive through a stale file.
    let mut manifest_json = None;
    if !wasm && let Some(path) = &artifact {
        manifest_json = veryl_simulator::component::loader::library_manifest(path);
        if manifest_json.is_none() {
            warn!(
                "Component ({name}) library does not export a veryl manifest; analysis-time interface checks are disabled"
            );
        }
        if let Some(sidecar) = veryl_metadata::component_crate_name(crate_dir)
            .map(|n| veryl_metadata::sidecar_manifest_path(target_dir, &n))
        {
            match &manifest_json {
                Some(json) => {
                    if let Err(e) = std::fs::write(&sidecar, json) {
                        warn!(
                            "Failed to write component manifest ({}): {e}",
                            sidecar.display()
                        );
                    }
                }
                None => {
                    let _ = std::fs::remove_file(&sidecar);
                }
            }
        }
    }
    artifact.map(|path| (path, manifest_json))
}

/// Picks the component package's own cdylib artifact from cargo's
/// `--message-format=json` output, matching by manifest path so that a
/// dependency's cdylib is never selected in its place.
fn select_component_artifact(
    stdout: &str,
    manifest_path: &std::path::Path,
    extensions: &[&str],
) -> Option<PathBuf> {
    let mut artifact: Option<PathBuf> = None;
    for line in stdout.lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if msg["reason"] != "compiler-artifact" {
            continue;
        }
        let is_own_package = msg["manifest_path"]
            .as_str()
            .is_some_and(|p| paths_refer_to_same_file(std::path::Path::new(p), manifest_path));
        if !is_own_package {
            continue;
        }
        let is_cdylib = msg["target"]["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|k| k == "cdylib"));
        if !is_cdylib {
            continue;
        }
        if let Some(files) = msg["filenames"].as_array() {
            for file in files {
                if let Some(path) = file.as_str()
                    && extensions.iter().any(|ext| path.ends_with(ext))
                {
                    artifact = Some(PathBuf::from(path));
                }
            }
        }
    }
    artifact
}

fn paths_refer_to_same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

#[allow(clippy::too_many_arguments)]
fn prepare_native_test(
    ir: &veryl_analyzer::ir::Ir,
    test_name: &str,
    top: &Option<resource_table::StrId>,
    opt: &OptTest,
    test_path: PathId,
    metadata: &Metadata,
    config: &Config,
    cache: &mut ProtoModuleCache,
) -> std::result::Result<NativeTestJob, SimulatorError> {
    let top_name = if let Some(top_str) = top {
        top_str.to_string()
    } else {
        test_name.to_string()
    };

    let top_str_id = resource_table::get_str_id(top_name.clone()).ok_or_else(|| {
        SimulatorError::TopModuleNotFound {
            module_name: top_name.clone(),
        }
    })?;
    let sim_ir = build_ir_cached(ir, top_str_id, config, cache)?;

    let module_name = sim_ir.name.to_string();

    let dump = if opt.wave {
        Some(create_wave_dumper(test_name, test_path, metadata)?)
    } else {
        None
    };

    Ok(NativeTestJob {
        module_name,
        sim_ir,
        dump,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_doc_test(
    ir: &veryl_analyzer::ir::Ir,
    module_name: &str,
    wavedrom_json: &str,
    ports: &[(String, String)],
    wave: bool,
    source_path: PathId,
    metadata: &Metadata,
    config: &Config,
    cache: &mut ProtoModuleCache,
) -> std::result::Result<Option<PathBuf>, SimulatorError> {
    let mut scenario = parse_wavedrom(wavedrom_json).map_err(|e| SimulatorError::TestFailed {
        message: format!("WaveDrom parse error in {module_name}: {e}"),
    })?;

    classify_signals(&mut scenario, ports);
    let top_str_id = resource_table::get_str_id(module_name.to_string()).ok_or_else(|| {
        SimulatorError::TopModuleNotFound {
            module_name: module_name.to_string(),
        }
    })?;
    let sim_ir = build_ir_cached(ir, top_str_id, config, cache)?;

    let dump = if wave {
        let doc_name = format!("{}_doc", module_name);
        Some(create_wave_dumper(&doc_name, source_path, metadata)?)
    } else {
        None
    };

    let mut sim = Simulator::new(sim_ir, dump);

    // Resolve clock event
    let fallback_clock = || {
        sim.ir
            .event_statements
            .keys()
            .find(|e| matches!(e, veryl_simulator::ir::Event::Clock(_)))
            .cloned()
            .unwrap_or(veryl_simulator::ir::Event::Initial)
    };
    let clock_event = scenario
        .signals
        .iter()
        .find(|s| s.kind == SignalKind::Clock)
        .and_then(|s| {
            sim.get_clock(&s.name)
                .or_else(|| sim.get_clock(&format!("i_{}", s.name)))
        })
        .unwrap_or_else(fallback_clock);

    // Resolve reset event
    let reset_event = sim
        .ir
        .event_statements
        .keys()
        .find(|e| matches!(e, veryl_simulator::ir::Event::Reset(_)))
        .cloned();

    remap_signal_names(&mut scenario, ports);

    // Build port width map (after remap so signal names match port names)
    let mut port_widths = std::collections::HashMap::new();
    for (port_name, _) in ports {
        let var_path: veryl_simulator::ir::VarPath = port_name.parse().unwrap();
        if let Some(var_id) = sim.ir.ports.get(&var_path)
            && let Some(var) = sim.ir.module_variables.variables.get(var_id)
        {
            port_widths.insert(port_name.clone(), var.width);
        }
    }

    let result = wavedrom::run_wavedrom_test(
        &mut sim,
        &scenario,
        &clock_event,
        &reset_event,
        3,
        &port_widths,
    );

    match result {
        TestResult::Pass => {
            let wave_path = sim.dump.and_then(|d| d.into_path());
            Ok(wave_path)
        }
        TestResult::Fail(msg) => Err(SimulatorError::TestFailed { message: msg }),
    }
}

/// Remap WaveDrom signal names to actual port names (e.g. "clk" -> "i_clk").
fn remap_signal_names(scenario: &mut wavedrom::WaveScenario, ports: &[(String, String)]) {
    for signal in &mut scenario.signals {
        if ports.iter().any(|(name, _)| name == &signal.name) {
            continue;
        }
        if let Some((port_name, _)) = ports
            .iter()
            .find(|(port_name, _)| wavedrom::strip_port_prefix(port_name) == signal.name)
        {
            signal.name = port_name.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn artifact_line(manifest: &str, kind: &str, file: &str) -> String {
        format!(
            r#"{{"reason":"compiler-artifact","manifest_path":"{manifest}","target":{{"kind":["{kind}"]}},"filenames":["{file}"]}}"#
        )
    }

    #[test]
    fn artifact_selection_ignores_dependency_cdylibs() {
        let stdout = format!(
            "{}\n{}\nnot json\n{}\n",
            artifact_line("/dep/Cargo.toml", "cdylib", "/t/libdep.so"),
            artifact_line("/pkg/Cargo.toml", "lib", "/t/libpkg.rlib"),
            artifact_line("/pkg/Cargo.toml", "cdylib", "/t/libpkg.so"),
        );
        let manifest = Path::new("/pkg/Cargo.toml");
        let extensions = &[".so", ".dylib", ".dll"];
        assert_eq!(
            select_component_artifact(&stdout, manifest, extensions),
            Some(PathBuf::from("/t/libpkg.so"))
        );

        // A dependency-only cdylib must not mask the missing-cdylib error.
        let stdout = artifact_line("/dep/Cargo.toml", "cdylib", "/t/libdep.so");
        assert_eq!(
            select_component_artifact(&stdout, manifest, extensions),
            None
        );
    }

    #[test]
    fn artifact_selection_filters_extensions() {
        let stdout = artifact_line("/pkg/Cargo.toml", "cdylib", "/t/pkg.wasm");
        let manifest = Path::new("/pkg/Cargo.toml");
        assert_eq!(
            select_component_artifact(&stdout, manifest, &[".wasm"]),
            Some(PathBuf::from("/t/pkg.wasm"))
        );
        assert_eq!(
            select_component_artifact(&stdout, manifest, &[".so", ".dylib", ".dll"]),
            None
        );
    }
}
