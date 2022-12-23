use thiserror::Error;
use veryl_parser::miette::{self, Diagnostic};

#[derive(Error, Diagnostic, Debug)]
pub enum ConfigError {
    #[diagnostic(code(ConfigError::FileIO), help(""))]
    #[error("file I/O error")]
    FileIO(#[from] std::io::Error),

    #[diagnostic(code(ConfigError::FileNotFound), help(""))]
    #[error("Veryl.toml is not found")]
    FileNotFound,

    #[diagnostic(code(ConfigError::Deserialize), help(""))]
    #[error("toml load failed")]
    Deserialize(#[from] toml::de::Error),
}
