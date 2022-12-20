use thiserror::Error;
use veryl_parser::parol_runtime::miette;
use veryl_parser::parol_runtime::miette::{Diagnostic, NamedSource, SourceSpan};
use veryl_parser::veryl_token::VerylToken;

#[derive(Error, Diagnostic, Debug)]
pub enum AnalyzeError {
    #[diagnostic(code(AnalyzeError::InvalidNumberCharacter), help(""))]
    #[error("{kind} number can't contain {cause}")]
    InvalidNumberCharacter {
        cause: char,
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::NumberOverflow), help("increase bit width"))]
    #[error("number is over the maximum size of {width} bits")]
    NumberOverflow {
        width: usize,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::NumberOverflow), help("add if_reset statement"))]
    #[error("if_reset statement is required for always_ff with reset signal")]
    IfResetRequired {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::NumberOverflow), help("add reset port"))]
    #[error("reset signal is required for always_ff with if_reset statement")]
    ResetSignalMissing {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::NumberOverflow), help("remove {kind} statement"))]
    #[error("{kind} statement can't be placed at here")]
    InvalidStatement {
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::NumberOverflow), help("remove {kind} direction"))]
    #[error("{kind} direction can't be placed at here")]
    InvalidDirection {
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },
}

impl AnalyzeError {
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
        AnalyzeError::InvalidNumberCharacter {
            cause,
            kind: kind.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn number_overflow(width: usize, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::NumberOverflow {
            width,
            input: AnalyzeError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn if_reset_required(source: &str, token: &VerylToken) -> Self {
        AnalyzeError::IfResetRequired {
            input: AnalyzeError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn reset_signal_missing(source: &str, token: &VerylToken) -> Self {
        AnalyzeError::ResetSignalMissing {
            input: AnalyzeError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn invalid_statement(kind: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::InvalidStatement {
            kind: kind.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }

    pub fn invalid_direction(kind: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::InvalidDirection {
            kind: kind.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: (&token.token.token).into(),
        }
    }
}
