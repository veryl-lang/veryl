use crate::OptUpdate;
use log::debug;
use miette::Result;
use std::time::Instant;
use veryl_metadata::{Lockfile, Metadata};

pub struct CmdUpdate {
    _opt: OptUpdate,
}

impl CmdUpdate {
    pub fn new(opt: OptUpdate) -> Self {
        Self { _opt: opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let now = Instant::now();

        let mut lockfile = Lockfile::load(&metadata.lockfile_path)?;
        lockfile.update(metadata, true)?;
        lockfile.save(&metadata.lockfile_path)?;

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());

        Ok(true)
    }
}
