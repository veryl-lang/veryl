use crate::OptClean;
use log::info;
use miette::{IntoDiagnostic, Result};
use std::fs;
use veryl_metadata::Metadata;

pub struct CmdClean {
    _opt: OptClean,
}

impl CmdClean {
    pub fn new(opt: OptClean) -> Self {
        Self { _opt: opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths::<&str>(&[])?;
        for path in &paths {
            if path.dst.exists() {
                info!("Removing file ({})", path.dst.to_string_lossy());
                fs::remove_file(&path.dst).into_diagnostic()?;
            }
        }

        let project_dependencies_path = metadata.project_dependencies_path();
        if project_dependencies_path.exists() {
            info!(
                "Removing dir  ({})",
                project_dependencies_path.to_string_lossy()
            );
            fs::remove_dir_all(&project_dependencies_path).into_diagnostic()?;
        }

        let filelist_path = metadata.filelist_path();
        if filelist_path.exists() {
            info!("Removing file ({})", filelist_path.to_string_lossy());
            fs::remove_file(&filelist_path).into_diagnostic()?;
        }

        Ok(true)
    }
}
