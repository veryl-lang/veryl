use crate::{Condition, MetadataError, ProjectProperty};
use pathdiff::diff_paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SV {
    #[serde(default)]
    pub filesets: Vec<Fileset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fileset {
    #[serde(default)]
    pub root: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub include_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub enable_if: Option<Condition>,
}

/// Resolved SystemVerilog files and include directories.
/// Paths written as absolute paths in Veryl.toml (including paths under an
/// absolute `root`) are absolute, and the others are relative to the output
/// directory, which is the place of the filelist.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub files: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
}

impl Sources {
    /// Appends `other` while removing duplicated paths.
    pub fn append(&mut self, other: Sources) {
        for file in other.files {
            push_unique(&mut self.files, file);
        }
        for dir in other.include_dirs {
            push_unique(&mut self.include_dirs, dir);
        }
    }
}

fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

impl Fileset {
    pub fn is_enabled(
        &self,
        properties: &BTreeMap<String, ProjectProperty>,
        project: &str,
    ) -> Result<bool, MetadataError> {
        match &self.enable_if {
            Some(x) => x.eval(properties, project),
            None => Ok(true),
        }
    }

    /// Returns (path written in Veryl.toml with `root` applied, directory flag)
    /// of each file and include directory. An absolute path ignores `root`
    /// because `Path::join` replaces the base by an absolute path.
    fn entries(&self) -> Vec<(PathBuf, bool)> {
        let root = self.root.as_deref().unwrap_or(Path::new(""));
        let files = self.files.iter().map(|x| (root.join(x), false));
        let dirs = self.include_dirs.iter().map(|x| (root.join(x), true));
        files.chain(dirs).collect()
    }
}

impl SV {
    /// Resolves enabled filesets of the project placed at `base`.
    /// Relative paths are returned as paths relative to `out_base`.
    /// If `copy_to` is specified, files and include directories are copied
    /// into it keeping the structure relative to `base`, and the copied paths
    /// are returned. Copied files are appended to `copied`.
    pub fn resolve(
        &self,
        base: &Path,
        out_base: &Path,
        properties: &BTreeMap<String, ProjectProperty>,
        project: &str,
        copy_to: Option<&Path>,
        copied: &mut Vec<PathBuf>,
    ) -> Result<Sources, MetadataError> {
        let mut ret = Sources::default();

        for fileset in &self.filesets {
            if !fileset.is_enabled(properties, project)? {
                continue;
            }

            for (path, is_dir) in fileset.entries() {
                let src = base.join(&path);
                let exists = if is_dir { src.is_dir() } else { src.is_file() };
                if !exists {
                    return Err(MetadataError::SvPathNotFound(src));
                }

                let target = if let Some(copy_to) = copy_to {
                    if !is_publishable(&path) {
                        return Err(MetadataError::InvalidSvPath(path));
                    }
                    let dst = copy_to.join(&path);
                    if is_dir {
                        copy_dir(&src, &dst, copied)?;
                    } else {
                        copy_file(&src, &dst, copied)?;
                    }
                    dst
                } else {
                    src
                };

                let target = target
                    .canonicalize()
                    .map_err(|x| MetadataError::file_io(x, &target))?;
                let target = if path.is_absolute() {
                    target
                } else {
                    diff_paths(&target, out_base).unwrap_or(target)
                };

                if is_dir {
                    push_unique(&mut ret.include_dirs, target);
                } else {
                    push_unique(&mut ret.files, target);
                }
            }
        }

        Ok(ret)
    }

    /// Checks that all filesets (regardless of `enable_if`) can be published.
    pub fn check_publish(&self) -> Result<(), MetadataError> {
        for fileset in &self.filesets {
            if let Some(root) = &fileset.root
                && !is_publishable(root)
            {
                return Err(MetadataError::InvalidSvPath(root.clone()));
            }
            for path in fileset.files.iter().chain(fileset.include_dirs.iter()) {
                if !is_publishable(path) {
                    return Err(MetadataError::InvalidSvPath(path.clone()));
                }
            }
        }
        Ok(())
    }
}

/// A published path must be a relative path without `..`.
fn is_publishable(path: &Path) -> bool {
    path.components()
        .all(|x| matches!(x, Component::Normal(_) | Component::CurDir))
}

fn copy_file(src: &Path, dst: &Path, copied: &mut Vec<PathBuf>) -> Result<(), MetadataError> {
    if let Some(parent) = dst.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|x| MetadataError::file_io(x, parent))?;
    }

    let src_text = fs::read(src).map_err(|x| MetadataError::file_io(x, src))?;
    // Skip writing unchanged files to keep their timestamps
    if fs::read(dst).ok().as_ref() != Some(&src_text) {
        fs::write(dst, &src_text).map_err(|x| MetadataError::file_io(x, dst))?;
    }
    copied.push(dst.to_path_buf());
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path, copied: &mut Vec<PathBuf>) -> Result<(), MetadataError> {
    if !dst.exists() {
        fs::create_dir_all(dst).map_err(|x| MetadataError::file_io(x, dst))?;
    }

    let entries = fs::read_dir(src).map_err(|x| MetadataError::file_io(x, src))?;
    for entry in entries {
        let entry = entry.map_err(|x| MetadataError::file_io(x, src))?;
        let src = entry.path();
        let dst = dst.join(entry.file_name());
        if src.is_dir() {
            copy_dir(&src, &dst, copied)?;
        } else {
            copy_file(&src, &dst, copied)?;
        }
    }
    Ok(())
}
