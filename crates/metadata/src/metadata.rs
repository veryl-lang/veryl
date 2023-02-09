use crate::build::{Build, Target};
use crate::format::Format;
use crate::git::Git;
use crate::lint::Lint;
use crate::project::Project;
use crate::MetadataError;
use directories::ProjectDirs;
use log::debug;
use regex::Regex;
use serde::{Deserialize, Serialize};
use spdx::Expression;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use url::Url;
use walkdir::WalkDir;

#[derive(Clone, Debug)]
pub struct PathPair {
    pub prj: Vec<String>,
    pub src: PathBuf,
    pub dst: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub project: Project,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub lint: Lint,
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
    #[serde(skip)]
    pub metadata_path: PathBuf,
}

impl Metadata {
    pub fn search_from_current() -> Result<PathBuf, MetadataError> {
        Metadata::search_from(env::current_dir()?)
    }

    pub fn search_from<T: AsRef<Path>>(from: T) -> Result<PathBuf, MetadataError> {
        for path in from.as_ref().ancestors() {
            let path = path.join("Veryl.toml");
            if path.is_file() {
                return Ok(path);
            }
        }

        Err(MetadataError::FileNotFound)
    }

    pub fn load<T: AsRef<Path>>(path: T) -> Result<Self, MetadataError> {
        let path = path.as_ref().canonicalize()?;
        let text = std::fs::read_to_string(&path)?;
        let mut metadata: Metadata = Self::from_str(&text)?;
        metadata.metadata_path = path;
        metadata.check()?;
        debug!(
            "Loaded metadata ({})",
            metadata.metadata_path.to_string_lossy()
        );
        Ok(metadata)
    }

    pub fn check(&self) -> Result<(), MetadataError> {
        let valid_project_name = Regex::new(r"^[a-zA-Z_][0-9a-zA-Z_]*$").unwrap();
        if !valid_project_name.is_match(&self.project.name) {
            return Err(MetadataError::InvalidProjectName(self.project.name.clone()));
        }

        if let Some(ref license) = self.project.license {
            let _ = Expression::parse(license)?;
        }

        Ok(())
    }

    fn gather_files_with_extension<T: AsRef<Path>>(
        base_dir: T,
        ext: &str,
    ) -> Result<Vec<PathBuf>, MetadataError> {
        let mut ret = Vec::new();
        for entry in WalkDir::new(base_dir) {
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

    fn gather_dependencies<T: AsRef<str>>(
        &self,
        update: bool,
        base_prj: &[T],
        base_dst: &Path,
        tomls: &mut HashSet<PathBuf>,
    ) -> Result<Vec<PathPair>, MetadataError> {
        let cache_dir = Self::cache_dir();

        let mut ret = Vec::new();
        for (name, dep) in &self.dependencies {
            if let Some(ref git) = dep.git {
                let mut path = cache_dir.to_path_buf();
                path.push("repository");
                if let Some(host) = git.host_str() {
                    path.push(host);
                }
                path.push(git.path().to_string().trim_start_matches('/'));

                debug!("Found dependency ({})", path.to_string_lossy());

                if let Some(ref rev) = dep.rev {
                    path.set_extension(rev);
                } else if let Some(ref tag) = dep.tag {
                    path.set_extension(tag);
                } else if let Some(ref branch) = dep.branch {
                    path.set_extension(branch);
                }

                let parent = path.parent().unwrap();
                if !parent.exists() {
                    std::fs::create_dir_all(parent)?;
                }

                let git = Git::clone(
                    git,
                    &path,
                    dep.rev.as_deref(),
                    dep.tag.as_deref(),
                    dep.branch.as_deref(),
                )?;
                if update {
                    git.fetch()?;
                }
                git.checkout()?;

                let mut prj: Vec<_> = base_prj.iter().map(|x| x.as_ref().to_string()).collect();
                prj.push(name.clone());

                for src in &Self::gather_files_with_extension(&path, "vl")? {
                    let rel = src.strip_prefix(&path)?;
                    let mut dst = base_dst.join(name);
                    dst.push(rel);
                    dst.set_extension("sv");
                    ret.push(PathPair {
                        prj: prj.clone(),
                        src: src.to_path_buf(),
                        dst,
                    });
                }

                let toml = path.join("Veryl.toml");
                if !tomls.contains(&toml) {
                    let metadata = Metadata::load(&toml)?;
                    tomls.insert(toml);
                    let base_dst = base_dst.join(name);
                    let deps = metadata.gather_dependencies(update, &prj, &base_dst, tomls)?;
                    for dep in deps {
                        ret.push(dep);
                    }
                }
            }
        }

        Ok(ret)
    }

    pub fn paths<T: AsRef<Path>>(
        &self,
        files: &[T],
        update: bool,
    ) -> Result<Vec<PathPair>, MetadataError> {
        let base = self.metadata_path.parent().unwrap();

        let src_files = if files.is_empty() {
            Self::gather_files_with_extension(base, "vl")?
        } else {
            files.iter().map(|x| x.as_ref().to_path_buf()).collect()
        };

        let mut ret = Vec::new();
        for src in src_files {
            let dst = match self.build.target {
                Target::Source => src.with_extension("sv"),
                Target::Directory { ref path } => {
                    base.join(path.join(src.with_extension("sv").file_name().unwrap()))
                }
            };
            ret.push(PathPair {
                prj: vec![self.project.name.clone()],
                src: src.to_path_buf(),
                dst,
            });
        }

        let base_dst = self.metadata_path.parent().unwrap().join("dependencies");
        if !base_dst.exists() {
            std::fs::create_dir(&base_dst)?;
        }

        let mut tomls = HashSet::new();
        let deps = self.gather_dependencies::<&str>(update, &[], &base_dst, &mut tomls)?;
        for dep in deps {
            ret.push(dep);
        }

        Ok(ret)
    }

    pub fn create_default_toml(name: &str) -> String {
        format!(
            r###"[project]
name = "{name}"
version = "0.1.0""###
        )
    }

    pub fn cache_dir() -> PathBuf {
        let project_dir = ProjectDirs::from("", "dalance", "veryl").unwrap();
        project_dir.cache_dir().to_path_buf()
    }
}

impl FromStr for Metadata {
    type Err = MetadataError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let metadata: Metadata = toml::from_str(s)?;
        Ok(metadata)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub git: Option<Url>,
    pub rev: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{ClockType, ResetType};
    use semver::{BuildMetadata, Prerelease, Version};

    const TEST_TOML: &'static str = r#"
[project]
name = "test"
version = "0.1.0"

[build]
clock_type = "posedge"
reset_type = "async_low"
target = {type = "source"}
#target = {type = "directory", path = "aaa"}

[format]
indent_width = 4
    "#;

    #[test]
    fn load_toml() {
        let metadata: Metadata = toml::from_str(TEST_TOML).unwrap();
        assert_eq!(metadata.project.name, "test");
        assert_eq!(
            metadata.project.version,
            Version {
                major: 0,
                minor: 1,
                patch: 0,
                pre: Prerelease::EMPTY,
                build: BuildMetadata::EMPTY,
            }
        );
        assert_eq!(metadata.build.clock_type, ClockType::PosEdge);
        assert_eq!(metadata.build.reset_type, ResetType::AsyncLow);
        assert_eq!(metadata.format.indent_width, 4);
    }

    #[test]
    fn search_config() {
        let path = Metadata::search_from_current();
        assert!(path.is_ok());
    }
}
