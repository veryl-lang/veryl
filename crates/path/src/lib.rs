use directories::ProjectDirs;
#[cfg(not(target_family = "wasm"))]
use fs4::fs_std::FileExt;
use log::debug;
#[cfg(not(target_family = "wasm"))]
use std::fs::File;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

mod path_error;
pub use path_error::PathError;

#[derive(Clone, Debug)]
pub struct PathSet {
    pub prj: String,
    pub src: PathBuf,
    pub dst: PathBuf,
    pub map: PathBuf,
}

pub fn cache_path() -> PathBuf {
    let project_dir = ProjectDirs::from("org", "veryl-lang", "veryl").unwrap();
    project_dir.cache_dir().to_path_buf()
}

pub fn gather_files_with_extension<T: AsRef<Path>>(
    base_dir: T,
    ext: &str,
    symlink: bool,
) -> Result<Vec<PathBuf>, PathError> {
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
        .sort_by_file_name()
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

#[cfg(not(target_family = "wasm"))]
pub fn lock_dir<T: AsRef<Path>>(path: T) -> Result<File, PathError> {
    let base_dir = cache_path().join(path);
    let lock = base_dir.join("lock");
    let lock = File::create(lock)?;
    lock.lock_exclusive()?;
    Ok(lock)
}

#[cfg(not(target_family = "wasm"))]
pub fn unlock_dir(lock: File) -> Result<(), PathError> {
    lock.unlock()?;
    Ok(())
}

#[cfg(target_family = "wasm")]
pub fn lock_dir<T: AsRef<Path>>(_path: T) -> Result<(), PathError> {
    Ok(())
}

#[cfg(target_family = "wasm")]
pub fn unlock_dir(_lock: ()) -> Result<(), PathError> {
    Ok(())
}
