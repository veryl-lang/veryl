use miette::{self, Diagnostic, SourceSpan};
use std::fmt;
use thiserror::Error;
use veryl_analyzer::multi_sources::{MultiSources, Source};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::TokenSource;

#[derive(Error, Diagnostic, Debug)]
pub enum SynthesizerError {
    #[diagnostic(severity(Error), code(synth::top_module_not_found))]
    #[error("top module '{name}' not found")]
    TopModuleNotFound { name: String },

    #[diagnostic(severity(Error), code(synth::unsupported))]
    #[error("unsupported construct: {kind}")]
    Unsupported {
        kind: UnsupportedKind,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(synth::unknown_width))]
    #[error("unable to determine width for '{what}'")]
    UnknownWidth {
        what: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(synth::dynamic_select))]
    #[error("dynamic index or select is not supported ({what})")]
    DynamicSelect {
        what: String,
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(synth::internal))]
    #[error("internal error: {message}")]
    Internal { message: String },
}

/// Categorises every "can't synthesize this shape" situation. Stringified
/// into the `Unsupported` diagnostic's message. Keeping the reason as a typed
/// variant (rather than free-form text) keeps callers honest about what they
/// actually hit and lets tooling bucket errors without regex on messages.
#[derive(Clone, Debug)]
pub enum UnsupportedKind {
    NonNumericValueFactor,
    UnknownFactor,
    SystemFunctionCall,
    PowOperator,
    DynamicRangeSelect { what: String },
    DynamicRangeEnd { what: String },
    MultiDimDynamicSelect { what: String },
    DynamicMultiDimIndex { what: String },
    ForStatement,
    UnsupportedVariableType { path: String, type_kind: String },
    SystemVerilogBlackbox,
    BundledInstInput,
    BundledInstOutput,
}

impl fmt::Display for UnsupportedKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonNumericValueFactor => "non-numeric value factor".fmt(f),
            Self::UnknownFactor => write!(
                f,
                "unknown factor (analyzer could not resolve — often external dependency or failed generic inference)"
            ),
            Self::SystemFunctionCall => {
                "system function call ($-function is not synthesizable)".fmt(f)
            }
            Self::PowOperator => "power operator (only constant exponent would be synthesizable; not implemented yet)".fmt(f),
            Self::DynamicRangeSelect { what } => write!(f, "dynamic range select on {}", what),
            Self::DynamicRangeEnd { what } => write!(f, "dynamic range-end on {}", what),
            Self::MultiDimDynamicSelect { what } => {
                write!(f, "multi-dim dynamic select on {}", what)
            }
            Self::DynamicMultiDimIndex { what } => {
                write!(f, "dynamic multi-dim index on {}", what)
            }
            Self::ForStatement => "for loop".fmt(f),
            Self::UnsupportedVariableType { path, type_kind } => {
                write!(f, "variable '{}' has unsupported {} type", path, type_kind)
            }
            Self::SystemVerilogBlackbox => "SystemVerilog blackbox instantiation".fmt(f),
            Self::BundledInstInput => "bundled InstInput id".fmt(f),
            Self::BundledInstOutput => "bundled InstOutput id".fmt(f),
        }
    }
}

fn source(token: &TokenRange) -> MultiSources {
    let path = token.beg.source.to_string();
    let text = token.beg.source.get_text();
    MultiSources {
        sources: vec![Source { path, text }],
    }
}

impl SynthesizerError {
    pub fn top_module_not_found(name: impl Into<String>) -> Self {
        Self::TopModuleNotFound { name: name.into() }
    }

    pub fn unsupported(kind: UnsupportedKind, token: &TokenRange) -> Self {
        Self::Unsupported {
            kind,
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn unknown_width(what: impl Into<String>, token: &TokenRange) -> Self {
        Self::UnknownWidth {
            what: what.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn dynamic_select(what: impl Into<String>, token: &TokenRange) -> Self {
        Self::DynamicSelect {
            what: what.into(),
            input: source(token),
            error_location: token.into(),
            token_source: token.source(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }
}
