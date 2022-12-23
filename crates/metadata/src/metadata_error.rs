use thiserror::Error;
use veryl_parser::miette::{self, Diagnostic};

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
}
