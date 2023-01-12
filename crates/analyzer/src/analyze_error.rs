use thiserror::Error;
use veryl_parser::miette::{self, Diagnostic, NamedSource, SourceSpan};
use veryl_parser::resource_table;
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

    #[diagnostic(code(AnalyzeError::IfResetRequired), help("add if_reset statement"))]
    #[error("if_reset statement is required for always_ff with reset signal")]
    IfResetRequired {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::ResetSignalMissing), help("add reset port"))]
    #[error("reset signal is required for always_ff with if_reset statement")]
    ResetSignalMissing {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::InvalidStatement), help("remove {kind} statement"))]
    #[error("{kind} statement can't be placed at here")]
    InvalidStatement {
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::InvalidDirection), help("remove {kind} direction"))]
    #[error("{kind} direction can't be placed at here")]
    InvalidDirection {
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(
        code(AnalyzeError::InvalidSystemFunction),
        help("fix system function name")
    )]
    #[error("system function \"{name}\" is not defined")]
    InvalidSystemFunction {
        name: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::MismatchArity), help("fix function arguments"))]
    #[error("function \"{name}\" has {arity} arguments, but {args} arguments are supplied")]
    MismatchArity {
        name: String,
        arity: usize,
        args: usize,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::MitmatchType), help(""))]
    #[error("\"{name}\" is expected to \"{expected}\", but it is \"{actual}\"")]
    MismatchType {
        name: String,
        expected: String,
        actual: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::MissingPort), help("add \"{port}\" port"))]
    #[error("module \"{name}\" has \"{port}\", but it is not connected")]
    MissingPort {
        name: String,
        port: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::UnknownPort), help("remove \"{port}\" port"))]
    #[error("module \"{name}\" doesn't has \"{port}\", but it is connected")]
    UnknownPort {
        name: String,
        port: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(code(AnalyzeError::DuplicatedIdentifier), help(""))]
    #[error("{identifier} is duplicated")]
    DuplicatedIdentifier {
        identifier: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(
        code(AnalyzeError::UnusedVariable),
        help("add prefix `_` to unused variable name")
    )]
    #[error("{identifier} is unused")]
    UnusedVariable {
        identifier: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },
}

impl AnalyzeError {
    fn named_source(source: &str, token: &VerylToken) -> NamedSource {
        NamedSource::new(
            resource_table::get_path_value(token.token.file_path)
                .unwrap()
                .to_string_lossy(),
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
            error_location: token.token.into(),
        }
    }

    pub fn number_overflow(width: usize, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::NumberOverflow {
            width,
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn if_reset_required(source: &str, token: &VerylToken) -> Self {
        AnalyzeError::IfResetRequired {
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn reset_signal_missing(source: &str, token: &VerylToken) -> Self {
        AnalyzeError::ResetSignalMissing {
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_statement(kind: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::InvalidStatement {
            kind: kind.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_direction(kind: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::InvalidDirection {
            kind: kind.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_system_function(name: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::InvalidSystemFunction {
            name: name.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn mismatch_arity(
        name: &str,
        arity: usize,
        args: usize,
        source: &str,
        token: &VerylToken,
    ) -> Self {
        AnalyzeError::MismatchArity {
            name: name.to_string(),
            arity,
            args,
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn mismatch_type(
        name: &str,
        expected: &str,
        actual: &str,
        source: &str,
        token: &VerylToken,
    ) -> Self {
        AnalyzeError::MismatchType {
            name: name.to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn missing_port(name: &str, port: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::MissingPort {
            name: name.to_string(),
            port: port.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn unknown_port(name: &str, port: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::UnknownPort {
            name: name.to_string(),
            port: port.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn duplicated_identifier(identifier: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::DuplicatedIdentifier {
            identifier: identifier.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn unused_variable(identifier: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzeError::UnusedVariable {
            identifier: identifier.to_string(),
            input: AnalyzeError::named_source(source, token),
            error_location: token.token.into(),
        }
    }
}
