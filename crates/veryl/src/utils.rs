use miette::{IntoDiagnostic, Result};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn gather_files<T: AsRef<Path>>(base_dir: T) -> Result<Vec<PathBuf>> {
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
        r###"[package]
name = "{}"
version = "0.1.0""###,
        name
    )
}
