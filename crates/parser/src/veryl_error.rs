use crate::veryl_token::VerylToken;
use parol_runtime::miette;
use parol_runtime::miette::{Diagnostic, SourceSpan};
use thiserror::Error;

#[derive(Clone, Error, Diagnostic, Debug)]
pub enum VerylError {
    #[error("{kind} number can't contain {cause}")]
    InvalidNumberCharacter {
        cause: char,
        kind: String,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[error("number is over the maximum size of {width} bits")]
    NumberOverflow {
        width: usize,
        #[label("Error location")]
        error_location: SourceSpan,
    },
}

impl VerylError {
    pub fn invalid_number_character(cause: char, kind: &str, token: &VerylToken) -> Self {
        VerylError::InvalidNumberCharacter {
            cause,
            kind: kind.to_string(),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn number_overflow(width: usize, token: &VerylToken) -> Self {
        VerylError::NumberOverflow {
            width,
            error_location: (&token.token.token).into(),
        }
    }
}
