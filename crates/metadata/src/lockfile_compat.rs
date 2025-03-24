pub mod v0 {
    use crate::metadata::UrlPath;
    use semver::Version;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use uuid::Uuid;

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Lockfile {
        pub projects: Vec<Lock>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Lock {
        pub name: String,
        pub uuid: Uuid,
        pub version: Version,
        pub url: UrlPath,
        pub revision: String,
        pub path: Option<PathBuf>,
        pub dependencies: Vec<LockDependency>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct LockDependency {
        pub name: String,
        pub version: Version,
        pub url: UrlPath,
        pub revision: String,
    }
}
