use crate::cmd_build::CmdBuild;
use crate::runner::{Vcs, Verilator, Vivado};
use crate::{OptBuild, OptTest};
use log::{error, info};
use miette::Result;
use veryl_analyzer::attribute::Attribute;
use veryl_analyzer::attribute_table;
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
        build.exec(metadata)?;

        let tests: Vec<_> = attribute_table::get_all()
            .into_iter()
            .filter_map(|(_, attr)| {
                if let Attribute::Test(x) = attr {
                    Some(x)
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

        let mut runner = match sim_type {
            SimType::Verilator => Verilator::new().runner(),
            SimType::Vcs => Vcs::new().runner(),
            SimType::Vivado => Vivado::new().runner(),
        };

        let mut success = 0;
        let mut failure = 0;
        for test in &tests {
            if runner.run(metadata, *test)? {
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
