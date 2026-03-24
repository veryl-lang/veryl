use crate::cmd_build::CmdBuild;
use crate::runner::{Cocotb, CocotbSource, Dsim, Vcs, Verilator, Vivado};
use crate::{OptBuild, OptTest};
use log::{error, info, warn};
use miette::Result;
use veryl_analyzer::symbol::TestType;
use veryl_analyzer::symbol_table;
use veryl_metadata::WaveFormFormat;
use veryl_metadata::{FilelistType, Metadata, SimType};
use veryl_parser::resource_table;
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
    test_name: String,
    module_name: String,
    sim_ir: Ir,
    dump: Option<WaveDumper>,
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

        let (tests, doc_tests) = if let Some(ref filter) = self.opt.test {
            let tests: Vec<_> = tests
                .into_iter()
                .filter(|(test, _)| {
                    let name =
                        veryl_parser::resource_table::get_str_value(*test).unwrap_or_default();
                    name.contains(filter.as_str())
                })
                .collect();

            let doc_tests: Vec<_> = doc_tests
                .into_iter()
                .filter(|dt| {
                    let name = resource_table::get_str_value(dt.module_name).unwrap_or_default();
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
            ..Config::default()
        };
        let mut proto_cache = ProtoModuleCache::default();

        let mut success = 0;
        let mut failure = 0;

        let mut native_jobs: Vec<NativeTestJob> = Vec::new();
        let mut non_native_tests = Vec::new();

        for (test, property) in &tests {
            match property.r#type {
                TestType::Native => {
                    let test_name =
                        veryl_parser::resource_table::get_str_value(*test).unwrap_or_default();
                    info!("Building IR for test ({test_name})");

                    match prepare_native_test(
                        &ir,
                        &test_name,
                        &property.top,
                        &self.opt,
                        metadata.test.waveform_format,
                        &config,
                        &mut proto_cache,
                    ) {
                        Ok(job) => native_jobs.push(job),
                        Err(e) => {
                            error!("Failed to build IR for test ({test_name}): {e}");
                            failure += 1;
                        }
                    }
                }
                _ => {
                    non_native_tests.push((test, property));
                }
            }
        }

        if !native_jobs.is_empty() {
            let num_threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(native_jobs.len());
            let job_queue = std::sync::Mutex::new(native_jobs.into_iter());

            type JobResult = (
                String,
                std::result::Result<TestResult, SimulatorError>,
                String,
            );
            let results: Vec<Vec<JobResult>> = std::thread::scope(|s| {
                let queue = &job_queue;
                let handles: Vec<_> = (0..num_threads)
                    .map(|_| {
                        s.spawn(move || {
                            let mut thread_results = Vec::new();
                            loop {
                                let job = queue.lock().unwrap().next();
                                let Some(job) = job else { break };
                                output_buffer::enable();
                                let result =
                                    run_native_testbench(job.sim_ir, job.dump, job.module_name);
                                let output = output_buffer::take();
                                thread_results.push((job.test_name, result, output));
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
                    Ok(TestResult::Pass) => {
                        info!("Succeeded test ({test_name})");
                        success += 1;
                    }
                    Ok(TestResult::Fail(msg)) => {
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
            } else {
                failure += 1;
            }
        }

        // Run doc tests
        for dt in &doc_tests {
            let module_name = resource_table::get_str_value(dt.module_name).unwrap_or_default();
            info!("Executing doc test ({module_name})");

            let wave_format = if self.opt.wave {
                Some(metadata.test.waveform_format)
            } else {
                None
            };
            match run_doc_test(
                &ir,
                &module_name,
                &dt.wavedrom_json,
                &dt.ports,
                wave_format,
                &config,
                &mut proto_cache,
            ) {
                Ok(()) => {
                    info!("Succeeded doc test ({module_name})");
                    success += 1;
                }
                Err(e) => {
                    error!("Failed doc test ({module_name}): {e}");
                    failure += 1;
                }
            }
        }

        if failure == 0 {
            info!("Completed tests : {success} passed, {failure} failed");
            Ok(true)
        } else {
            error!("Completed tests : {success} passed, {failure} failed");
            Ok(false)
        }
    }
}

fn prepare_native_test(
    ir: &veryl_analyzer::ir::Ir,
    test_name: &str,
    top: &Option<resource_table::StrId>,
    opt: &OptTest,
    waveform_format: WaveFormFormat,
    config: &Config,
    cache: &mut ProtoModuleCache,
) -> std::result::Result<NativeTestJob, SimulatorError> {
    let top_name = if let Some(top_str) = top {
        resource_table::get_str_value(*top_str).unwrap_or_default()
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

    let dump: Option<WaveDumper> = if opt.wave {
        let path = format!("{}.{}", test_name, waveform_format.extension());
        info!("  Dumping waveform to {}", path);
        match waveform_format {
            WaveFormFormat::Vcd => {
                let file = std::fs::File::create(&path).map_err(|e| SimulatorError::IoError {
                    message: format!("failed to create waveform file {path}: {e}"),
                })?;
                Some(WaveDumper::new_vcd(Box::new(file)))
            }
            WaveFormFormat::Fst => Some(WaveDumper::new_fst(&path)),
        }
    } else {
        None
    };

    Ok(NativeTestJob {
        test_name: test_name.to_string(),
        module_name,
        sim_ir,
        dump,
    })
}

fn run_doc_test(
    ir: &veryl_analyzer::ir::Ir,
    module_name: &str,
    wavedrom_json: &str,
    ports: &[(String, String)],
    wave_format: Option<WaveFormFormat>,
    config: &Config,
    cache: &mut ProtoModuleCache,
) -> std::result::Result<(), SimulatorError> {
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

    let dump: Option<WaveDumper> = if let Some(format) = wave_format {
        let path = format!("{}_doc.{}", module_name, format.extension());
        info!("  Dumping waveform to {}", path);
        match format {
            WaveFormFormat::Vcd => {
                let file = std::fs::File::create(&path).map_err(|e| SimulatorError::IoError {
                    message: format!("failed to create waveform file {path}: {e}"),
                })?;
                Some(WaveDumper::new_vcd(Box::new(file)))
            }
            WaveFormFormat::Fst => Some(WaveDumper::new_fst(&path)),
        }
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
        TestResult::Pass => Ok(()),
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
