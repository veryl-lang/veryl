use crate::cmd_build::CmdBuild;
use crate::runner::{Cocotb, CocotbSource, Vcs, Verilator, Vivado};
use crate::{OptBuild, OptTest};
use log::{error, info};
use miette::Result;
use veryl_analyzer::symbol::{SymbolKind, TestType};
use veryl_analyzer::symbol_table;
use veryl_metadata::{FilelistType, Metadata, SimType};

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
        });
        build.exec(metadata, true)?;

        let tests: Vec<_> = symbol_table::get_all()
            .into_iter()
            .filter_map(|symbol| {
                if symbol.namespace.to_string() == metadata.project.name {
                    if let SymbolKind::Test(x) = symbol.kind {
                        Some((symbol.token.text, x))
                    } else {
                        None
                    }
                } else {
                    None
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
            let mut runner = match property.r#type {
                TestType::Inline => match sim_type {
                    SimType::Verilator => Verilator::new().runner(),
                    SimType::Vcs => Vcs::new().runner(),
                    SimType::Vivado => Vivado::new().runner(),
                },
                TestType::CocotbEmbed(x) => Cocotb::new(CocotbSource::Embed(x)).runner(),
                TestType::CocotbInclude(x) => Cocotb::new(CocotbSource::Include(x)).runner(),
            };

            if runner.run(metadata, *test, property.top, property.path, self.opt.wave)? {
                success += 1;
            } else {
                failure += 1;
            }
        }

        if failure == 0 {
            info!("Completed tests : {} passed, {} failed", success, failure);
            Ok(true)
        } else {
            error!("Completed tests : {} passed, {} failed", success, failure);
            Ok(false)
        }
    }
}
