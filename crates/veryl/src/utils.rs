use miette::{IntoDiagnostic, Result};
use std::path::{Path, PathBuf};
use veryl_metadata::{Metadata, Target};
use walkdir::WalkDir;

#[derive(Clone, Debug)]
pub struct PathPair {
    pub prj: String,
    pub src: PathBuf,
    pub dst: PathBuf,
}

pub fn gather_files(files: &[PathBuf], metadata: &Metadata) -> Result<Vec<PathPair>> {
    let src_files = if files.is_empty() {
        gather_vl_files(metadata.metadata_path.parent().unwrap())?
    } else {
        files.to_vec()
    };

    let mut ret = Vec::new();
    for src in src_files {
        let dst = match metadata.build.target {
            Target::Source => src.with_extension("sv"),
            Target::Directory { ref path } => {
                let base = metadata.metadata_path.parent().unwrap().to_owned();
                base.join(path.join(src.with_extension("sv").file_name().unwrap()))
            }
        };
        ret.push(PathPair {
            prj: metadata.project.name.clone(),
            src: src.to_path_buf(),
            dst,
        });
    }
    Ok(ret)
}

pub fn gather_vl_files<T: AsRef<Path>>(base_dir: T) -> Result<Vec<PathBuf>> {
    let mut ret = Vec::new();
    for entry in WalkDir::new(base_dir) {
        let entry = entry.into_diagnostic()?;
        if entry.file_type().is_file() {
            if let Some(x) = entry.path().extension() {
                if x == "vl" {
                    ret.push(entry.path().to_path_buf());
                }
            }
        }
    }
    Ok(ret)
}

pub fn create_default_toml(name: &str) -> String {
    format!(
        r###"[project]
name = "{}"
version = "0.1.0""###,
        name
    )
}
