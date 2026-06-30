use crate::lockfile::{Lock, LockSource};
use crate::metadata::Metadata;
use crate::metadata_error::MetadataError;
use semver::Version;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MetadataOutputV2 {
    pub format_version: usize,
    pub root: MetadataProjectV2,
    pub dependencies: Vec<MetadataDependencyV2>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MetadataProjectV2 {
    pub name: String,
    pub version: Option<Version>,
    pub local_path: PathBuf,
    pub metadata: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MetadataDependencyV2 {
    pub id: String,
    pub name: String,
    pub project: String,
    pub source: MetadataSourceV2,
    pub local_path: PathBuf,
    pub metadata: BTreeMap<String, toml::Value>,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetadataSourceV2 {
    Path {
        path: PathBuf,
    },
    Repository {
        url: String,
        project: String,
        version: Version,
        revision: String,
        path: PathBuf,
    },
}

impl MetadataOutputV2 {
    pub fn from_metadata(metadata: &Metadata) -> Result<Self, MetadataError> {
        let project_path = metadata.project_path();
        let locks = metadata.lockfile.projects();
        let dependency_ids = locks
            .iter()
            .map(|lock| (lock.source.clone(), format!("dep:{}", lock.name)))
            .collect::<HashMap<_, _>>();
        let mut dependencies = locks
            .iter()
            .map(|lock| MetadataDependencyV2::from_lock(lock, metadata, &dependency_ids))
            .collect::<Result<Vec<_>, _>>()?;
        dependencies.sort_by(|x, y| x.id.cmp(&y.id));

        Ok(Self {
            format_version: 2,
            root: MetadataProjectV2 {
                name: metadata.project.name.clone(),
                version: metadata.project.version.clone(),
                local_path: project_path,
                metadata: metadata.metadata.clone().into_iter().collect(),
            },
            dependencies,
        })
    }
}

impl MetadataDependencyV2 {
    fn from_lock(
        lock: &Lock,
        root_metadata: &Metadata,
        dependency_ids: &HashMap<LockSource, String>,
    ) -> Result<Self, MetadataError> {
        let source = MetadataSourceV2::from_lock_source(&lock.source);
        let metadata = root_metadata.lockfile.get_metadata(&lock.source)?;
        let mut dependencies = lock
            .dependencies
            .iter()
            .map(|dependency| {
                dependency_ids
                    .get(&dependency.source)
                    .cloned()
                    .unwrap_or_else(|| format!("dep:{}", dependency.name))
            })
            .collect::<Vec<_>>();
        dependencies.sort();

        Ok(Self {
            // Lock names are conflict-disambiguated during lock generation, so they are stable ids.
            id: format!("dep:{}", lock.name),
            name: lock.name.clone(),
            project: metadata.project.name.clone(),
            local_path: metadata.project_path(),
            metadata: metadata.metadata.into_iter().collect(),
            source,
            dependencies,
        })
    }
}

impl MetadataSourceV2 {
    fn from_lock_source(source: &LockSource) -> Self {
        match source {
            LockSource::Path(path) => Self::Path { path: path.clone() },
            LockSource::Repository(repository) => Self::Repository {
                url: repository.url.to_string(),
                project: repository.project.clone(),
                version: repository.version.clone(),
                revision: repository.revision.clone(),
                path: repository.path.clone(),
            },
        }
    }
}
