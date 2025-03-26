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
        for path in metadata.build_info.generated_files.iter() {
            if path.exists() {
                info!("Removing file ({})", path.to_string_lossy());
                fs::remove_file(path).into_diagnostic()?;
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

        let doc_path = metadata.doc_path();
        if doc_path.exists() {
            info!("Removing dir  ({})", doc_path.to_string_lossy());
            fs::remove_dir_all(&doc_path).into_diagnostic()?;
        }

        metadata.build_info.generated_files.clear();
        metadata.save_build_info()?;

        Ok(true)
    }
}
