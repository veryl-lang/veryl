use crate::cmd_build::CmdBuild;
use crate::runner::{Cocotb, CocotbSource, Dsim, Vcs, Verilator, Vivado};
use crate::{OptBuild, OptTest};
use log::{error, info};
use miette::Result;
use veryl_analyzer::symbol::{SymbolKind, TestType};
use veryl_analyzer::symbol_table;
use veryl_metadata::{FilelistType, Metadata, SimType};
use veryl_parser::resource_table;
use veryl_simulator::ir::{Config, build_ir};
use veryl_simulator::simulator_error::SimulatorError;
use veryl_simulator::testbench::{TestResult, run_native_testbench};

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

        let tests: Vec<_> = symbol_table::get_all()
            .into_iter()
            .filter_map(|symbol| {
                if symbol.namespace.to_string() != metadata.project.name {
                    return None;
                }
                match symbol.kind {
                    SymbolKind::Module(ref x) if x.test.is_some() => {
                        Some((symbol.token.text, x.test.clone().unwrap()))
                    }
                    SymbolKind::Test(x) => Some((symbol.token.text, x)),
                    _ => None,
                }
            })
            .collect();

        let sim_type = if let Some(x) = self.opt.sim {
            x.into()
        } else {
            metadata.test.simulator
        };

        let mut success = 0;
        let mut failure = 0;
        for (test, property) in &tests {
            match property.r#type {
                TestType::Native => {
                    let test_name =
                        veryl_parser::resource_table::get_str_value(*test).unwrap_or_default();
                    info!("Executing test ({test_name})");

                    match run_native_test(&ir, &test_name, &property.top, self.opt.wave) {
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
    wave: bool,
) -> std::result::Result<(), SimulatorError> {
    let top_name = if let Some(top_str) = top {
        resource_table::get_str_value(*top_str).unwrap_or_default()
    } else {
        test_name.to_string()
    };

    let config = Config::default();
    let top_str_id = resource_table::get_str_id(top_name.clone()).ok_or_else(|| {
        SimulatorError::TopModuleNotFound {
            module_name: top_name.clone(),
        }
    })?;
    let sim_ir = build_ir(ir.clone(), top_str_id, &config)?;

    let dump: Option<Box<dyn std::io::Write>> = if wave {
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
