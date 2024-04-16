use crate::MetadataError;
use log::debug;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn gather_files_with_extension<T: AsRef<Path>>(
    base_dir: T,
    ext: &str,
    symlink: bool,
) -> Result<Vec<PathBuf>, MetadataError> {
    let mut ret = Vec::new();
    for entry in WalkDir::new(base_dir).follow_links(symlink) {
        let entry = entry?;
        if entry.file_type().is_file() {
            if let Some(x) = entry.path().extension() {
                if x == ext {
                    debug!("Found file ({})", entry.path().to_string_lossy());
                    ret.push(entry.path().to_path_buf());
                }
            }
        }
    }
    Ok(ret)
}
