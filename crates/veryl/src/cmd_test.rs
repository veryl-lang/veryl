use crate::cmd_build::CmdBuild;
use crate::runner::{Runner, Vcs, Verilator, Vivado};
use crate::{OptBuild, OptTest};
use miette::Result;
use veryl_analyzer::attribute::Attribute;
use veryl_analyzer::attribute_table;
use veryl_metadata::Metadata;
use veryl_metadata::SimType;

pub struct CmdTest {
    opt: OptTest,
}

impl CmdTest {
    pub fn new(opt: OptTest) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
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
            SimType::Verilator => Box::new(Verilator) as Box<dyn Runner>,
            SimType::Vcs => Box::new(Vcs) as Box<dyn Runner>,
            SimType::Vivado => Box::new(Vivado) as Box<dyn Runner>,
        };

        let mut ret = true;
        for test in &tests {
            ret &= runner.run(metadata, *test)?;
        }

        Ok(ret)
    }
}
