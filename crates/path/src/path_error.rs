use miette::{self, Diagnostic};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum PathError {
    #[diagnostic(code(PathError::FileIO), help(""))]
    #[error("file I/O error")]
    FileIO(#[from] std::io::Error),

    #[diagnostic(code(PathError::StripPrefix), help(""))]
    #[error("strip prefix error")]
    StripPrefix(#[from] std::path::StripPrefixError),
}
