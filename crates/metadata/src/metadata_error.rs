use crate::metadata::UrlPath;
use miette::{self, Diagnostic};
use semver::Version;
use std::path::PathBuf;
use thiserror::Error;
use url::Url;
use veryl_path::PathError;

#[derive(Error, Diagnostic, Debug)]
pub enum MetadataError {
    #[diagnostic(code(MetadataError::FileIO), help(""))]
    #[error("file I/O error")]
    FileIO(#[from] std::io::Error),

    #[diagnostic(code(MetadataError::FileNotFound), help(""))]
    #[error("Veryl.toml is not found")]
    FileNotFound,

    #[diagnostic(code(MetadataError::Deserialize), help(""))]
    #[error("toml load failed")]
    Deserialize(#[from] toml::de::Error),

    #[diagnostic(code(MetadataError::StripPrefix), help(""))]
    #[error("strip prefix error")]
    StripPrefix(#[from] std::path::StripPrefixError),

    #[diagnostic(code(MetadataError::Git), help(""))]
    #[error("git operation failure: {0}")]
    Git(Box<dyn std::error::Error + Sync + Send>),

    #[diagnostic(
        code(MetadataError::InvalidProjectName),
        help("\"[a-zA-Z_][0-9a-zA-Z_]*\" can be used as project name")
    )]
    #[error("project name \"{0}\" is invalid")]
    InvalidProjectName(String),

    #[diagnostic(
        code(MetadataError::InvalidLicense),
        help("license text should follow SPDX expression")
    )]
    #[error("license parse failed")]
    InvalidLicense(#[from] spdx::ParseError),

    #[diagnostic(code(MetadataError::PublishedVersion), help("bump up version"))]
    #[error("\"{0}\" is already published")]
    PublishedVersion(Version),

    #[diagnostic(code(MetadataError::ModifiedProject), help(""))]
    #[error("There are modified files in {0}")]
    ModifiedProject(PathBuf),

    #[diagnostic(code(MetadataError::Toml), help(""))]
    #[error("toml serialization error")]
    TomlSer(#[from] toml::ser::Error),

    #[diagnostic(code(MetadataError::VersionNotFound), help(""))]
    #[error("{version} @ {url} is not found")]
    VersionNotFound { url: UrlPath, version: String },

    #[diagnostic(code(MetadataError::GitSpec), help(""))]
    #[error("no version/rev/tag/branch specification of {0}")]
    GitSpec(Url),

    #[diagnostic(code(MetadataError::NameConflict), help(""))]
    #[error("project name \"{0}\" is used multiply in dependencies")]
    NameConflict(String),

    #[diagnostic(code(MetadataError::Path), help(""))]
    #[error("path error")]
    Path(#[from] PathError),
}
