use crate::build::{Build, Target};
use crate::build_info::BuildInfo;
use crate::doc::Doc;
use crate::env_var::EnvVar;
use crate::format::Format;
use crate::git::Git;
use crate::lint::Lint;
use crate::lockfile::Lockfile;
use crate::project::Project;
use crate::pubfile::{Pubfile, Release};
use crate::publish::Publish;
use crate::test::Test;
use crate::{FilelistType, MetadataError, SourceMapTarget};
use log::{debug, info, warn};
use once_cell::sync::Lazy;
use regex::Regex;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use spdx::Expression;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use url::Url;
use veryl_path::{PathSet, ignore_already_exists};

#[derive(Clone, Copy, Debug)]
pub enum BumpKind {
    Major,
    Minor,
    Patch,
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
    pub publish: Publish,
    #[serde(default)]
    pub doc: Doc,
    #[serde(default)]
    pub test: Test,
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
    #[serde(skip)]
    pub metadata_path: PathBuf,
    #[serde(skip)]
    pub pubfile_path: PathBuf,
    #[serde(skip)]
    pub pubfile: Pubfile,
    #[serde(skip)]
    pub lockfile_path: PathBuf,
    #[serde(skip)]
    pub lockfile: Lockfile,
    #[serde(skip)]
    pub build_info: BuildInfo,
    #[serde(skip)]
    pub env_var: EnvVar,
}

#[derive(Clone, Debug, Serialize, Deserialize, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum UrlPath {
    Url(Url),
    Path(PathBuf),
}

impl fmt::Display for UrlPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UrlPath::Url(x) => x.fmt(f),
            UrlPath::Path(x) => {
                let text = x.to_string_lossy();
                text.fmt(f)
            }
        }
    }
}

static VALID_PROJECT_NAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-zA-Z_][0-9a-zA-Z_]*$").unwrap());

fn check_project_name(name: &str) -> Result<(), MetadataError> {
    if !VALID_PROJECT_NAME.is_match(name) {
        return Err(MetadataError::InvalidProjectName(name.to_string()));
    }
    if name.starts_with("__") {
        return Err(MetadataError::ReservedProjectName(name.to_string()));
    }
    Ok(())
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
        let text = fs::read_to_string(&path)?;
        let mut metadata: Metadata = Self::from_str(&text)?;
        metadata.metadata_path.clone_from(&path);
        metadata.pubfile_path = path.with_file_name("Veryl.pub");
        metadata.lockfile_path = path.with_file_name("Veryl.lock");
        metadata.check()?;

        if metadata.pubfile_path.exists() {
            metadata.pubfile = Pubfile::load(&metadata.pubfile_path)?;
        }

        let dot_build = metadata.project_dot_build_path();
        if !dot_build.exists() {
            ignore_already_exists(fs::create_dir(&dot_build))?;
        }

        let build_info = metadata.project_build_info_path();
        if build_info.exists() {
            metadata.build_info = BuildInfo::load(&build_info)?;
        }

        debug!(
            "Loaded metadata ({})",
            metadata.metadata_path.to_string_lossy()
        );
        Ok(metadata)
    }

    pub fn publish(&mut self) -> Result<(), MetadataError> {
        let prj_path = self.project_path();
        let git = Git::open(&prj_path)?;
        if !git.is_clean()? {
            return Err(MetadataError::ModifiedProject(prj_path.to_path_buf()));
        }

        for release in &self.pubfile.releases {
            if release.version == self.project.version {
                return Err(MetadataError::PublishedVersion(
                    self.project.version.clone(),
                ));
            }
        }

        let version = self.project.version.clone();
        let revision = git.get_revision()?;

        info!("Publishing release ({version} @ {revision})");

        let release = Release { version, revision };

        self.pubfile.releases.push(release);

        self.pubfile.save(&self.pubfile_path)?;
        info!("Writing metadata ({})", self.pubfile_path.to_string_lossy());

        if self.publish.publish_commit {
            git.add(&self.pubfile_path)?;
            git.commit(&self.publish.publish_commit_message)?;
            info!(
                "Committing metadata ({})",
                self.pubfile_path.to_string_lossy()
            );
        }

        Ok(())
    }

    pub fn check(&self) -> Result<(), MetadataError> {
        check_project_name(&self.project.name)?;

        if let Some(ref license) = self.project.license {
            let _ = Expression::parse(license)?;
        }

        Ok(())
    }

    pub fn bump_version(&mut self, kind: BumpKind) -> Result<(), MetadataError> {
        let prj_path = self.project_path();
        let git = Git::open(&prj_path)?;

        let mut bumped_version = self.project.version.clone();
        match kind {
            BumpKind::Major => {
                bumped_version.major += 1;
                bumped_version.minor = 0;
                bumped_version.patch = 0;
            }
            BumpKind::Minor => {
                bumped_version.minor += 1;
                bumped_version.patch = 0;
            }
            BumpKind::Patch => bumped_version.patch += 1,
        }
        info!(
            "Bumping version ({} -> {})",
            self.project.version, bumped_version
        );

        self.project.version = bumped_version.clone();

        let toml = fs::read_to_string(&self.metadata_path)?;
        let re = Regex::new(r#"version\s+=\s+"([^"]*)""#).unwrap();
        let caps = re
            .captures(&toml)
            .expect("safely unwrap because metadata is valid");
        let bumped_field = caps[0].replace(&caps[1], &bumped_version.to_string());
        let bumped_toml = re.replace(&toml, bumped_field);
        fs::write(&self.metadata_path, bumped_toml.as_bytes())?;
        info!(
            "Updating version field ({})",
            self.metadata_path.to_string_lossy()
        );

        if self.publish.bump_commit {
            git.add(&self.metadata_path)?;
            git.commit(&self.publish.bump_commit_message)?;
            info!(
                "Committing metadata ({})",
                self.metadata_path.to_string_lossy()
            );
        }

        Ok(())
    }

    pub fn update_lockfile(&mut self) -> Result<(), MetadataError> {
        let modified = if self.lockfile_path.exists() {
            let mut lockfile = Lockfile::load(self)?;
            let modified = lockfile.update(self, false)?;
            self.lockfile = lockfile;
            modified
        } else {
            self.lockfile = Lockfile::new(self)?;
            true
        };
        if modified {
            self.lockfile.save(&self.lockfile_path)?;
        }
        Ok(())
    }

    pub fn save_build_info(&mut self) -> Result<(), MetadataError> {
        let build_info = self.project_build_info_path();
        self.build_info.save(&build_info)
    }

    pub fn paths<T: AsRef<Path>>(
        &mut self,
        files: &[T],
        symlink: bool,
        include_dependencies: bool,
    ) -> Result<Vec<PathSet>, MetadataError> {
        let sources = if self.build.source.iter().count() > 0 {
            warn!(
                "[Veryl.toml] \"source\" field is deprecated. Replace it with \"sources\" field."
            );
            vec![self.build.source.clone()]
        } else {
            self.build.sources.clone()
        };

        let base = self.project_path();
        let mut ret = Vec::new();

        for source in &sources {
            let src_base = base.join(source);

            let src_files = if files.is_empty() {
                veryl_path::gather_files_with_extension(&src_base, "veryl", symlink)?
            } else {
                let mut ret = Vec::new();
                for file in files {
                    ret.push(fs::canonicalize(file.as_ref())?);
                }
                ret
            };

            for src in src_files {
                let Ok(src_relative) = src.strip_prefix(&src_base) else {
                    return Err(MetadataError::InvalidSourceLocation(src));
                };
                let dst = match self.build.target {
                    Target::Source => src.with_extension("sv"),
                    Target::Directory { ref path } => {
                        base.join(path.join(src_relative.with_extension("sv")))
                    }
                    Target::Bundle { .. } => base.join(
                        PathBuf::from("target").join(src.with_extension("sv").file_name().unwrap()),
                    ),
                };
                let map = match &self.build.sourcemap_target {
                    SourceMapTarget::Directory { path } => {
                        if let Target::Directory { .. } = self.build.target {
                            base.join(path.join(src_relative.with_extension("sv.map")))
                        } else {
                            let dst = dst.strip_prefix(&base).unwrap();
                            base.join(path.join(dst.with_extension("sv.map")))
                        }
                    }
                    _ => {
                        let mut map = dst.clone();
                        map.set_extension("sv.map");
                        map
                    }
                };
                ret.push(PathSet {
                    prj: self.project.name.clone(),
                    src: src.to_path_buf(),
                    dst,
                    map,
                });
            }
        }

        let base_dst = self.project_dependencies_path();
        if !base_dst.exists() {
            ignore_already_exists(fs::create_dir(&base_dst))?;
        }

        if include_dependencies {
            if !self.build.exclude_std {
                veryl_std::expand()?;
                ret.append(&mut veryl_std::paths(&base_dst)?);
            }

            self.update_lockfile()?;

            let mut deps = self.lockfile.paths(&base_dst)?;
            ret.append(&mut deps);
        }

        Ok(ret)
    }

    pub fn create_default_toml(name: &str) -> Result<String, MetadataError> {
        check_project_name(name)?;

        Ok(format!(
            r###"[project]
name = "{name}"
version = "0.1.0"
[build]
source = "src"
target = {{type = "directory", path = "target"}}"###
        ))
    }

    pub fn create_default_gitignore() -> &'static str {
        r#".build/
"#
    }

    pub fn project_path(&self) -> PathBuf {
        self.metadata_path.parent().unwrap().to_path_buf()
    }

    pub fn project_dependencies_path(&self) -> PathBuf {
        self.project_path().join("dependencies")
    }

    pub fn project_dot_build_path(&self) -> PathBuf {
        self.project_path().join(".build")
    }

    pub fn project_build_info_path(&self) -> PathBuf {
        self.project_dot_build_path().join("info.toml")
    }

    pub fn filelist_path(&self) -> PathBuf {
        let filelist_name = match self.build.filelist_type {
            FilelistType::Absolute => format!("{}.f", self.project.name),
            FilelistType::Relative => format!("{}.f", self.project.name),
            FilelistType::Flgen => format!("{}.list.rb", self.project.name),
        };

        self.metadata_path.with_file_name(filelist_name)
    }

    pub fn doc_path(&self) -> PathBuf {
        self.metadata_path.parent().unwrap().join(&self.doc.path)
    }
}

impl FromStr for Metadata {
    type Err = MetadataError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let metadata: Metadata = toml::from_str(s)?;
        Ok(metadata)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum Dependency {
    Version(VersionReq),
    Entry(DependencyEntry),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyEntry {
    pub version: Option<VersionReq>,
    pub git: Option<UrlPath>,
    pub github: Option<String>,
    pub project: Option<String>,
    pub path: Option<PathBuf>,
}
