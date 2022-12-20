use crate::veryl_token::VerylToken;
use parol_runtime::miette;
use parol_runtime::miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum VerylError {
    #[diagnostic(code(VerylError::InvalidNumberCharacter), help(""))]
    #[error("{kind} number can't contain {cause}")]
    InvalidNumberCharacter {
        cause: char,
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(VerylError::NumberOverflow), help("increase bit width"))]
    #[error("number is over the maximum size of {width} bits")]
    NumberOverflow {
        width: usize,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(VerylError::NumberOverflow), help("add if_reset statement"))]
    #[error("if_reset statement is required for always_ff with reset signal")]
    IfResetRequired {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(VerylError::NumberOverflow), help("add reset port"))]
    #[error("reset signal is required for always_ff with if_reset statement")]
    ResetSignalMissing {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },
}

impl VerylError {
    fn named_source(source: &str, token: &VerylToken) -> NamedSource {
        NamedSource::new(
            token.token.token.location.file_name.to_string_lossy(),
            source.to_string(),
        )
    }

    pub fn invalid_number_character(
        cause: char,
        kind: &str,
        source: &str,
        token: &VerylToken,
    ) -> Self {
        VerylError::InvalidNumberCharacter {
            cause,
            kind: kind.to_string(),
            input: VerylError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn number_overflow(width: usize, source: &str, token: &VerylToken) -> Self {
        VerylError::NumberOverflow {
            width,
            input: VerylError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn if_reset_required(source: &str, token: &VerylToken) -> Self {
        VerylError::IfResetRequired {
            input: VerylError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn reset_signal_missing(source: &str, token: &VerylToken) -> Self {
        VerylError::ResetSignalMissing {
            input: VerylError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }
}
