use crate::OptCheck;
use crate::pipeline::{self, AnalyzeOptions, AnalyzeOutput};
use miette::Result;
use veryl_metadata::Metadata;

pub struct CmdCheck {
    opt: OptCheck,
}

impl CmdCheck {
    pub fn new(opt: OptCheck) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, true)?;

        let options = AnalyzeOptions {
            defines: &[],
            emit_mode: false,
            incremental: true,
            fail_fast: true,
        };
        let AnalyzeOutput {
            incremental,
            check_error,
            ..
        } = pipeline::analyze(metadata, &paths, options, None, None)?;

        // Save clean files before failing on warnings, so a second check warms.
        if let Some(mut inc) = incremental {
            inc.save(&pipeline::collect_diagnosed(&check_error));
        }

        // check fails on warnings, not just errors.
        let _ = check_error.check_all()?;
        Ok(true)
    }
}
