use miette::{self, Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;
use veryl_parser::resource_table;
use veryl_parser::veryl_token::VerylToken;

#[derive(Error, Diagnostic, Debug)]
pub enum AnalyzerError {
    #[diagnostic(severity(Error), code(duplicated_identifier), help(""))]
    #[error("{identifier} is duplicated")]
    DuplicatedIdentifier {
        identifier: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(invalid_allow), help(""))]
    #[error("{identifier} can't be allowed")]
    InvalidAllow {
        identifier: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_direction),
        help("remove {kind} direction")
    )]
    #[error("{kind} direction can't be placed at here")]
    InvalidDirection {
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(invalid_lsb), help("remove lsb"))]
    #[error("lsb can't be placed at here")]
    InvalidLsb {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(invalid_msb), help("remove msb"))]
    #[error("msb can't be placed at here")]
    InvalidMsb {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(invalid_number_character), help(""))]
    #[error("{kind} number can't contain {cause}")]
    InvalidNumberCharacter {
        cause: char,
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_statement),
        help("remove {kind} statement")
    )]
    #[error("{kind} statement can't be placed at here")]
    InvalidStatement {
        kind: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(
        severity(Error),
        code(invalid_system_function),
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

    #[diagnostic(severity(Error), code(mismatch_arity), help("fix function arguments"))]
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

    #[diagnostic(severity(Error), code(mismatch_attribute_args), help(""))]
    #[error("Arguments of \"{name}\" is expected to \"{expected}\"")]
    MismatchAttributeArgs {
        name: String,
        expected: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(mismatch_type), help(""))]
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

    #[diagnostic(
        severity(Error),
        code(missing_if_reset),
        help("add if_reset statement")
    )]
    #[error("if_reset statement is required for always_ff with reset signal")]
    MissingIfReset {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Warning), code(missing_port), help("add \"{port}\" port"))]
    #[error("module \"{name}\" has \"{port}\", but it is not connected")]
    MissingPort {
        name: String,
        port: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(missing_reset_signal), help("add reset port"))]
    #[error("reset signal is required for always_ff with if_reset statement")]
    MissingResetSignal {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(
        severity(Warning),
        code(missing_reset_statement),
        help("add reset statement")
    )]
    #[error("{name} is not reset in if_reset statement")]
    MissingResetStatement {
        name: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(too_large_enum_variant), help(""))]
    #[error("The value of enum variant {identifier} is {value}, it is can't be represented by {width} bits")]
    TooLargeEnumVariant {
        identifier: String,
        value: isize,
        width: usize,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(too_large_number), help("increase bit width"))]
    #[error("number is over the maximum size of {width} bits")]
    TooLargeNumber {
        width: usize,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(too_much_enum_variant), help(""))]
    #[error(
        "enum {identifier} has {number} variants, they are can't be represented by {width} bits"
    )]
    TooMuchEnumVariant {
        identifier: String,
        number: usize,
        width: usize,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(undefined_identifier), help(""))]
    #[error("{identifier} is undefined")]
    UndefinedIdentifier {
        identifier: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(unknown_attribute), help(""))]
    #[error("\"{name}\" is not valid attribute")]
    UnknownAttribute {
        name: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(unknown_member), help(""))]
    #[error("\"{name}\" doesn't have member \"{member}\"")]
    UnknownMember {
        name: String,
        member: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(unknown_msb), help(""))]
    #[error("resolving msb is failed")]
    UnknownMsb {
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(severity(Error), code(unknown_port), help("remove \"{port}\" port"))]
    #[error("module \"{name}\" doesn't have port \"{port}\", but it is connected")]
    UnknownPort {
        name: String,
        port: String,
        #[source_code]
        input: NamedSource,
        #[label("Error location")]
        error_location: SourceSpan,
    },

    #[diagnostic(
        severity(Warning),
        code(unused_variable),
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

impl AnalyzerError {
    fn named_source(source: &str, token: &VerylToken) -> NamedSource {
        NamedSource::new(
            resource_table::get_path_value(token.token.file_path)
                .unwrap()
                .to_string_lossy(),
            source.to_string(),
        )
    }

    pub fn duplicated_identifier(identifier: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::DuplicatedIdentifier {
            identifier: identifier.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_allow(identifier: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::InvalidAllow {
            identifier: identifier.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_direction(kind: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::InvalidDirection {
            kind: kind.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_lsb(source: &str, token: &VerylToken) -> Self {
        AnalyzerError::InvalidLsb {
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_msb(source: &str, token: &VerylToken) -> Self {
        AnalyzerError::InvalidMsb {
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_number_character(
        cause: char,
        kind: &str,
        source: &str,
        token: &VerylToken,
    ) -> Self {
        AnalyzerError::InvalidNumberCharacter {
            cause,
            kind: kind.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_statement(kind: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::InvalidStatement {
            kind: kind.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn invalid_system_function(name: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::InvalidSystemFunction {
            name: name.to_string(),
            input: AnalyzerError::named_source(source, token),
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
        AnalyzerError::MismatchArity {
            name: name.to_string(),
            arity,
            args,
            input: AnalyzerError::named_source(source, token),
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
        AnalyzerError::MismatchType {
            name: name.to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn missing_if_reset(source: &str, token: &VerylToken) -> Self {
        AnalyzerError::MissingIfReset {
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn missing_reset_signal(source: &str, token: &VerylToken) -> Self {
        AnalyzerError::MissingResetSignal {
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn missing_reset_statement(name: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::MissingResetStatement {
            name: name.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn mismatch_attribute_args(
        name: &str,
        expected: &str,
        source: &str,
        token: &VerylToken,
    ) -> Self {
        AnalyzerError::MismatchAttributeArgs {
            name: name.to_string(),
            expected: expected.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn missing_port(name: &str, port: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::MissingPort {
            name: name.to_string(),
            port: port.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn too_large_enum_variant(
        identifier: &str,
        value: isize,
        width: usize,
        source: &str,
        token: &VerylToken,
    ) -> Self {
        AnalyzerError::TooLargeEnumVariant {
            identifier: identifier.to_string(),
            value,
            width,
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn too_large_number(width: usize, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::TooLargeNumber {
            width,
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn too_much_enum_variant(
        identifier: &str,
        number: usize,
        width: usize,
        source: &str,
        token: &VerylToken,
    ) -> Self {
        AnalyzerError::TooMuchEnumVariant {
            identifier: identifier.to_string(),
            number,
            width,
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn undefined_identifier(identifier: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::UndefinedIdentifier {
            identifier: identifier.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn unknown_attribute(name: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::UnknownAttribute {
            name: name.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn unknown_member(name: &str, member: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::UnknownMember {
            name: name.to_string(),
            member: member.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn unknown_msb(source: &str, token: &VerylToken) -> Self {
        AnalyzerError::UnknownMsb {
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn unknown_port(name: &str, port: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::UnknownPort {
            name: name.to_string(),
            port: port.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }

    pub fn unused_variable(identifier: &str, source: &str, token: &VerylToken) -> Self {
        AnalyzerError::UnusedVariable {
            identifier: identifier.to_string(),
            input: AnalyzerError::named_source(source, token),
            error_location: token.token.into(),
        }
    }
}
