use crate::build::{Build, Target};
use crate::format::Format;
use crate::git::Git;
use crate::lint::Lint;
use crate::project::Project;
use crate::pubdata::Pubdata;
use crate::pubdata::Release;
use crate::publish::Publish;
use crate::MetadataError;
use directories::ProjectDirs;
use log::{debug, info};
use regex::Regex;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use spdx::Expression;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::Write;
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
    pub dependencies: HashMap<String, Dependency>,
    #[serde(skip)]
    pub metadata_path: PathBuf,
    #[serde(skip)]
    pub pubdata_path: PathBuf,
    #[serde(skip)]
    pub pubdata: Pubdata,
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
        metadata.metadata_path = path.clone();
        metadata.pubdata_path = path.with_file_name("Veryl.pub");
        metadata.check()?;

        if metadata.pubdata_path.exists() {
            let text = std::fs::read_to_string(&metadata.pubdata_path)?;
            metadata.pubdata = Pubdata::from_str(&text)?;
        }

        debug!(
            "Loaded metadata ({})",
            metadata.metadata_path.to_string_lossy()
        );
        Ok(metadata)
    }

    pub fn publish(&mut self) -> Result<(), MetadataError> {
        let prj_path = self.metadata_path.parent().unwrap();
        let git = Git::open(&prj_path)?;
        if !git.is_clean()? {
            return Err(MetadataError::ModifiedProject(prj_path.to_path_buf()));
        }

        for release in &self.pubdata.releases {
            if release.version == self.project.version {
                return Err(MetadataError::PublishedVersion(
                    self.project.version.clone(),
                ));
            }
        }

        let version = self.project.version.clone();
        let revision = git.get_revision()?;

        info!("Publishing release ({} @ {})", version, revision);

        let release = Release { version, revision };

        self.pubdata.releases.push(release);

        let text = toml::to_string(&self.pubdata)?;
        let mut file = File::create(&self.pubdata_path)?;
        write!(file, "{text}")?;
        file.flush()?;
        info!("Writing metadata ({})", self.pubdata_path.to_string_lossy());

        if self.publish.publish_commit {
            git.add(&self.pubdata_path)?;
            git.commit(&self.publish.publish_commit_message)?;
            info!(
                "Committing metadata ({})",
                self.pubdata_path.to_string_lossy()
            );
        }

        Ok(())
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

    pub fn bump_version(&mut self, kind: BumpKind) -> Result<(), MetadataError> {
        let prj_path = self.metadata_path.parent().unwrap();
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
        let re = Regex::new(r##"version\s+=\s+"([^"]*)""##).unwrap();
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
            let mut path = cache_dir.to_path_buf();
            path.push("repository");
            if let Some(host) = dep.git.host_str() {
                path.push(host);
            }
            path.push(dep.git.path().to_string().trim_start_matches('/'));

            debug!("Found dependency ({})", path.to_string_lossy());

            let release = Self::get_release(&dep.git, &path, &dep.version, update)?;
            let rev = Some(release.revision.clone());
            path.set_extension(format!("{}", release.version));

            let parent = path.parent().unwrap();
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }

            let git = Git::clone(&dep.git, &path)?;
            if update {
                git.fetch()?;
            }
            git.checkout(rev.as_deref())?;

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

        Ok(ret)
    }

    fn get_release(
        url: &Url,
        path: &Path,
        version_req: &VersionReq,
        update: bool,
    ) -> Result<Release, MetadataError> {
        let mut path = path.to_path_buf();
        path.set_extension("pub");

        let parent = path.parent().unwrap();
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }

        let git = Git::clone(url, &path)?;
        if update {
            git.fetch()?;
        }
        git.checkout(None)?;

        let toml = path.join("Veryl.pub");
        let mut pubdata = Pubdata::load(&toml)?;

        pubdata.releases.sort_by(|a, b| b.version.cmp(&a.version));

        for release in &pubdata.releases {
            if version_req.matches(&release.version) {
                return Ok(release.clone());
            }
        }

        Err(MetadataError::VersionNotFound {
            url: url.clone(),
            version: version_req.to_string(),
        })
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub git: Url,
    pub version: VersionReq,
}
