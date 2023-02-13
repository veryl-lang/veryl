use crate::OptPublish;
use log::debug;
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

        if let Some(kind) = self.opt.bump {
            metadata.bump_version(kind.into()).into_diagnostic()?;
        }

        let mut metadata = metadata.clone();

        metadata.publish()?;
        metadata.save_pubdata()?;

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());
        Ok(true)
    }
}
