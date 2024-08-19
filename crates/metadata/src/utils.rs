use crate::MetadataError;
use log::debug;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn gather_files_with_extension<T: AsRef<Path>>(
    base_dir: T,
    ext: &str,
    symlink: bool,
) -> Result<Vec<PathBuf>, MetadataError> {
    let mut inner_prj = Vec::new();
    for entry in WalkDir::new(base_dir.as_ref())
        .follow_links(symlink)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file() {
            if let Some(x) = entry.path().file_name() {
                if x == "Veryl.toml" {
                    let prj_dir = entry.path().parent().unwrap();
                    if prj_dir != base_dir.as_ref() {
                        debug!("Found inner project ({})", prj_dir.to_string_lossy());
                        inner_prj.push(prj_dir.to_path_buf());
                    }
                }
            }
        }
    }

    let mut ret = Vec::new();
    for entry in WalkDir::new(base_dir.as_ref())
        .follow_links(symlink)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file() {
            if let Some(x) = entry.path().extension() {
                if x == ext {
                    let is_inner = inner_prj.iter().any(|x| entry.path().starts_with(x));

                    if !is_inner {
                        debug!("Found file ({})", entry.path().to_string_lossy());
                        ret.push(entry.path().to_path_buf());
                    }
                }
            }
        }
    }
    Ok(ret)
}
