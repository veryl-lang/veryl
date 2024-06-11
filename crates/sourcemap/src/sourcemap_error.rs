use miette::{self, Diagnostic};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum SourceMapError {
    #[diagnostic(code(MetadataError::Io), help(""))]
    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[diagnostic(code(MetadataError::SourceMap), help(""))]
    #[error("sourcemap error")]
    SourceMap(#[from] sourcemap::Error),

    #[diagnostic(code(MetadataError::NotFound), help(""))]
    #[error("map is not found")]
    NotFound,
}
