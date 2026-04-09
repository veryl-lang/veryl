use crate::cmd_build::CmdBuild;
use crate::runner::{Cocotb, CocotbSource, Dsim, Vcs, Verilator, Vivado};
use crate::{OptBuild, OptTest};
use log::{error, info, warn};
use miette::Result;
use std::path::PathBuf;
use veryl_analyzer::symbol::TestType;
use veryl_analyzer::symbol_table;
use veryl_metadata::WaveFormFormat;
use veryl_metadata::{FilelistType, Metadata, SimType, WaveFormTarget};
use veryl_parser::resource_table::{self, PathId};
use veryl_simulator::ir::{Config, Ir, ProtoModuleCache, build_ir_cached};
use veryl_simulator::output_buffer;
use veryl_simulator::simulator::Simulator;
use veryl_simulator::simulator_error::SimulatorError;
use veryl_simulator::testbench::{TestResult, run_native_testbench};
use veryl_simulator::wave_dumper::WaveDumper;
use veryl_simulator::wavedrom::{self, SignalKind, classify_signals, parse_wavedrom};

pub struct CmdTest {
    opt: OptTest,
}

struct NativeTestJob {
    module_name: String,
    sim_ir: Ir,
    dump: Option<WaveDumper>,
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
        });

        let mut ir = veryl_analyzer::ir::Ir::default();
        build.exec(metadata, true, false, Some(&mut ir))?;

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

        let config = Config {
            use_jit: !self.opt.disable_jit,
            disable_ff_opt: self.opt.disable_ff_opt,
            ..Config::default()
        };
        let mut proto_cache = ProtoModuleCache::default();

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
            let num_threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(pending_native.len());
            let pending_queue = std::sync::Mutex::new(pending_native.into_iter());
            let table_snapshot = resource_table::export_tables();

            type JobResult = (
                String,
                std::result::Result<(TestResult, Option<PathBuf>), SimulatorError>,
                String,
            );
            let ir_ref = &ir;
            let config_ref = &config;
            let opt_ref = &self.opt;
            let metadata_ref: &Metadata = metadata;
            let results: Vec<Vec<JobResult>> = std::thread::scope(|s| {
                let queue = &pending_queue;
                let snapshot = &table_snapshot;
                let handles: Vec<_> = (0..num_threads)
                    .map(|_| {
                        s.spawn(move || {
                            resource_table::import_tables(snapshot);
                            // Per-thread cache avoids locking; cross-test
                            // reuse is rare since each top name is unique.
                            let mut thread_cache = ProtoModuleCache::default();
                            let mut thread_results = Vec::new();
                            loop {
                                let pending = queue.lock().unwrap().next();
                                let Some(pending) = pending else { break };
                                output_buffer::enable();
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
                                let run_result = match build_result {
                                    Ok(job) => {
                                        let wave_path =
                                            job.dump.as_ref().and_then(|d| d.path().cloned());
                                        run_native_testbench(job.sim_ir, job.dump, job.module_name)
                                            .map(|r| (r, wave_path))
                                    }
                                    Err(e) => Err(e),
                                };
                                let output = output_buffer::take();
                                thread_results.push((pending.test_name, run_result, output));
                            }
                            thread_results
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });

            for (test_name, result, output) in results.into_iter().flatten() {
                info!("Executing test ({test_name})");
                if !output.is_empty() {
                    print!("{output}");
                }
                match result {
                    Ok((TestResult::Pass, wave_path)) => {
                        info!("Succeeded test ({test_name})");
                        success += 1;
                        if let Some(path) = wave_path {
                            metadata.add_generated_file(path);
                        }
                    }
                    Ok((TestResult::Fail(msg), _)) => {
                        error!("Failed test ({test_name}): {msg}");
                        failure += 1;
                    }
                    Err(e) => {
                        error!("Failed test ({test_name}): {e}");
                        failure += 1;
                    }
                }
            }
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

            if runner.run(metadata, *test, property.top, property.path, self.opt.wave)? {
                success += 1;
                if self.opt.wave {
                    let test_name = test.to_string();
                    let path = wave_output_path(&test_name, property.path, metadata);
                    metadata.add_generated_file(path);
                }
            } else {
                failure += 1;
            }
        }

        for dt in &doc_tests {
            let module_name = dt.module_name.to_string();
            info!("Executing doc test ({module_name})");

            match run_doc_test(
                &ir,
                &module_name,
                &dt.wavedrom_json,
                &dt.ports,
                self.opt.wave,
                dt.path,
                metadata,
                &config,
                &mut proto_cache,
            ) {
                Ok(wave_path) => {
                    info!("Succeeded doc test ({module_name})");
                    success += 1;
                    if let Some(path) = wave_path {
                        metadata.add_generated_file(path);
                    }
                }
                Err(e) => {
                    error!("Failed doc test ({module_name}): {e}");
                    failure += 1;
                }
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

        if failure == 0 {
            info!("{summary}");
            Ok(true)
        } else {
            error!("{summary}");
            Ok(false)
        }
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
