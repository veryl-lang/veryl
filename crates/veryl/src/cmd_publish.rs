use crate::OptPublish;
use log::{debug, warn};
use miette::{IntoDiagnostic, Result};
use std::time::Instant;
use veryl_metadata::Metadata;

pub struct CmdPublish {
    opt: OptPublish,
}

impl CmdPublish {
    pub fn new(opt: OptPublish) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &Metadata) -> Result<bool> {
        let now = Instant::now();

        let mut metadata = metadata.clone();

        if let Some(kind) = self.opt.bump {
            metadata.bump_version(kind.into()).into_diagnostic()?;
            if !metadata.publish.bump_commit {
                warn!("Please git add and commit: Veryl.toml");
                return Ok(true);
            }
        }

        metadata.publish()?;

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());
        Ok(true)
    }
}
