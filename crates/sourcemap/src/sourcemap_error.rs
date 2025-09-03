use miette::{self, Diagnostic};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum SourceMapError {
    #[diagnostic(code(MetadataError::Io), help(""))]
    #[error("IO error ({path})")]
    Io {
        #[source]
        source: std::io::Error,
        path: PathBuf,
    },

    #[diagnostic(code(MetadataError::SourceMap), help(""))]
    #[error("sourcemap error")]
    SourceMap(#[from] sourcemap::Error),

    #[diagnostic(code(MetadataError::NotFound), help(""))]
    #[error("map is not found")]
    NotFound,
}

impl SourceMapError {
    pub fn io(source: std::io::Error, path: &Path) -> SourceMapError {
        SourceMapError::Io {
            source,
            path: path.into(),
        }
    }
}
