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

        if metadata.lockfile_path.exists() {
            let mut lockfile = Lockfile::load(&metadata.lockfile_path)?;
            let modified = lockfile.update(metadata, true)?;
            if modified {
                lockfile.save(&metadata.lockfile_path)?;
            }
        } else {
            let mut lockfile = Lockfile::new(metadata)?;
            lockfile.save(&metadata.lockfile_path)?;
        }

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());

        Ok(true)
    }
}
