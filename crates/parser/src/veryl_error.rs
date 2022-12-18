use parol_runtime::lexer::Token;
use parol_runtime::miette;
use parol_runtime::miette::{Diagnostic, SourceSpan};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum VerylError {
    #[error("{cause}")]
    SemanticError {
        cause: String,
        #[label("Error location")]
        error_location: SourceSpan,
    },
}

impl VerylError {
    pub fn semantic_error(cause: String, token: &Token) -> Self {
        VerylError::SemanticError {
            cause,
            error_location: token.into(),
        }
    }
}
