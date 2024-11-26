use crate::OptUpdate;
use miette::Result;
use veryl_metadata::{Lockfile, Metadata};

pub struct CmdUpdate {
    _opt: OptUpdate,
}

impl CmdUpdate {
    pub fn new(opt: OptUpdate) -> Self {
        Self { _opt: opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        if metadata.lockfile_path.exists() {
            let mut lockfile = Lockfile::load(metadata)?;
            let modified = lockfile.update(metadata, true)?;
            if modified {
                lockfile.save(&metadata.lockfile_path)?;
            }
        } else {
            let mut lockfile = Lockfile::new(metadata)?;
            lockfile.save(&metadata.lockfile_path)?;
        }

        Ok(true)
    }
}
