use crate::cmd_build::CmdBuild;
use crate::runner::{Cocotb, CocotbSource, Dsim, Vcs, Verilator, Vivado};
use crate::{OptBuild, OptTest};
use log::{error, info, warn};
use miette::Result;
use veryl_analyzer::symbol::TestType;
use veryl_analyzer::symbol_table;
use veryl_metadata::{FilelistType, Metadata, SimType};
use veryl_parser::resource_table;
use veryl_simulator::ir::{Config, ProtoModuleCache, build_ir_cached};
use veryl_simulator::simulator::Simulator;
use veryl_simulator::simulator_error::SimulatorError;
use veryl_simulator::testbench::{TestResult, run_native_testbench};
use veryl_simulator::wavedrom::{self, SignalKind, classify_signals, parse_wavedrom};

pub struct CmdTest {
    opt: OptTest,
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
        for (test, property) in &tests {
            match property.r#type {
                TestType::Native => {
                    let test_name =
                        veryl_parser::resource_table::get_str_value(*test).unwrap_or_default();
                    info!("Executing test ({test_name})");

                    match run_native_test(
                        &ir,
                        &test_name,
                        &property.top,
                        &self.opt,
                        &config,
                        &mut proto_cache,
                    ) {
                        Ok(()) => {
                            info!("Succeeded test ({test_name})");
                            success += 1;
                        }
                        Err(e) => {
                            error!("Failed test ({test_name}): {e}");
                            failure += 1;
                        }
                    }
                }
                _ => {
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
                        TestType::CocotbInclude(x) => {
                            Cocotb::new(CocotbSource::Include(x)).runner()
                        }
                        TestType::Native => unreachable!(),
                    };

                    if runner.run(metadata, *test, property.top, property.path, self.opt.wave)? {
                        success += 1;
                    } else {
                        failure += 1;
                    }
                }
            }
        }

        // Run doc tests
        for dt in &doc_tests {
            let module_name = resource_table::get_str_value(dt.module_name).unwrap_or_default();
            info!("Executing doc test ({module_name})");

            match run_doc_test(
                &ir,
                &module_name,
                &dt.wavedrom_json,
                &dt.ports,
                self.opt.wave,
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

fn run_native_test(
    ir: &veryl_analyzer::ir::Ir,
    test_name: &str,
    top: &Option<resource_table::StrId>,
    opt: &OptTest,
    config: &Config,
    cache: &mut ProtoModuleCache,
) -> std::result::Result<(), SimulatorError> {
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

    let dump: Option<Box<dyn std::io::Write>> = if opt.wave {
        let path = format!("{}.vcd", test_name);
        let file = std::fs::File::create(&path).map_err(|e| SimulatorError::IoError {
            message: format!("failed to create VCD file {path}: {e}"),
        })?;
        info!("  Dumping waveform to {}", path);
        Some(Box::new(file))
    } else {
        None
    };

    match run_native_testbench(sim_ir, dump)? {
        TestResult::Pass => Ok(()),
        TestResult::Fail(msg) => Err(SimulatorError::TestFailed { message: msg }),
    }
}

fn run_doc_test(
    ir: &veryl_analyzer::ir::Ir,
    module_name: &str,
    wavedrom_json: &str,
    ports: &[(String, String)],
    wave: bool,
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

    let dump: Option<Box<dyn std::io::Write>> = if wave {
        let path = format!("{}_doc.vcd", module_name);
        let file = std::fs::File::create(&path).map_err(|e| SimulatorError::IoError {
            message: format!("failed to create VCD file {path}: {e}"),
        })?;
        info!("  Dumping waveform to {}", path);
        Some(Box::new(file))
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
