use directories::ProjectDirs;
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
        if entry.file_type().is_file()
            && let Some(x) = entry.path().file_name()
            && x == "Veryl.toml"
        {
            let prj_dir = entry.path().parent().unwrap();
            if prj_dir != base_dir.as_ref() {
                debug!("Found inner project ({})", prj_dir.to_string_lossy());
                inner_prj.push(prj_dir.to_path_buf());
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
        if entry.file_type().is_file()
            && let Some(x) = entry.path().extension()
            && x == ext
        {
            let is_inner = inner_prj.iter().any(|x| entry.path().starts_with(x));

            if !is_inner {
                debug!("Found file ({})", entry.path().to_string_lossy());
                ret.push(entry.path().to_path_buf());
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
    fs4::FileExt::lock(&lock)?;
    Ok(lock)
}

#[cfg(not(target_family = "wasm"))]
pub fn unlock_dir(lock: File) -> Result<(), PathError> {
    fs4::FileExt::unlock(&lock)?;
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

/// Write `contents` to `path` atomically (temp file + rename) so a concurrent
/// reader never observes a truncated/empty file, only the old or new contents.
#[cfg(not(target_family = "wasm"))]
pub fn atomic_write<P: AsRef<Path>>(path: P, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let path = path.as_ref();
    // The temp must share the target's dir so the rename stays on one filesystem.
    let dir = path
        .parent()
        .filter(|x| !x.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(contents)?;
    // tempfile creates with 0600; widen to 0644 to match a plain write.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o644))?;
    }
    // On Windows, replacing the target while a reader holds it open transiently
    // fails with a sharing violation (PermissionDenied); retry a few times.
    let mut attempts = 0;
    loop {
        match file.persist(path) {
            Ok(_) => return Ok(()),
            Err(e) => {
                attempts += 1;
                if attempts >= 50 || e.error.kind() != std::io::ErrorKind::PermissionDenied {
                    return Err(e.error);
                }
                file = e.file;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

// wasm has no real filesystem concurrency; fall back to a plain write.
#[cfg(target_family = "wasm")]
pub fn atomic_write<P: AsRef<Path>>(path: P, contents: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

pub fn ignore_already_exists(x: Result<(), std::io::Error>) -> Result<(), std::io::Error> {
    if let Err(x) = x
        && x.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(x);
    }
    Ok(())
}

pub fn ignore_directory_not_empty(x: Result<(), std::io::Error>) -> Result<(), std::io::Error> {
    if let Err(x) = x
        && x.kind() != std::io::ErrorKind::DirectoryNotEmpty
    {
        return Err(x);
    }
    Ok(())
}
