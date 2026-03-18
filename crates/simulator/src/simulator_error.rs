use miette::{Diagnostic, SourceSpan};
use thiserror::Error;
use veryl_analyzer::multi_sources::{MultiSources, Source};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::TokenSource;

#[derive(Error, Diagnostic, Debug)]
pub enum SimulatorError {
    #[diagnostic(severity(Error), code(top_module_not_found))]
    #[error("top module \"{module_name}\" not found")]
    TopModuleNotFound { module_name: String },

    #[diagnostic(severity(Error), code(no_initial_block))]
    #[error("no initial block found in module \"{module_name}\"")]
    NoInitialBlock {
        module_name: String,
        #[source_code]
        input: MultiSources,
        #[label("this module has no initial block")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(test_failed))]
    #[error("{message}")]
    TestFailed { message: String },

    #[diagnostic(severity(Error), code(io_error))]
    #[error("{message}")]
    IoError { message: String },

    #[diagnostic(severity(Error), code(unresolved_expression))]
    #[error("unresolved expression")]
    UnresolvedExpression {
        #[source_code]
        input: MultiSources,
        #[label("could not resolve this expression")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(recursive_function))]
    #[error("recursive function \"{function_name}\" cannot be inlined")]
    RecursiveFunction {
        function_name: String,
        #[source_code]
        input: MultiSources,
        #[label("recursive call here")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(unsupported_description))]
    #[error("unsupported description")]
    UnsupportedDescription {
        #[source_code]
        input: MultiSources,
        #[label("this description is not supported by the simulator")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(severity(Error), code(combinational_loop))]
    #[error("combinational loop detected")]
    CombinationalLoop {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        #[label(collection, "involved in loop")]
        loop_participants: Vec<SourceSpan>,
        token_source: TokenSource,
    },
}

fn source_with_context(
    token: &TokenRange,
    context: &[TokenRange],
) -> (MultiSources, Vec<SourceSpan>) {
    let path = token.beg.source.to_string();
    let text = token.beg.source.get_text();

    let mut base = text.len();
    let mut ranges = Vec::new();
    let mut sources = Vec::new();

    sources.push(Source { path, text });

    for x in context.iter().rev() {
        let path = x.beg.source.to_string();
        let text = x.beg.source.get_text();

        let mut range = *x;
        range.offset(base as u32);
        ranges.push(range.into());

        base += text.len();

        sources.push(Source { path, text });
    }

    let sources = MultiSources { sources };
    (sources, ranges)
}

impl SimulatorError {
    pub fn no_initial_block(module_name: &str, token: &TokenRange) -> Self {
        let path = token.beg.source.to_string();
        let text = token.beg.source.get_text();
        let input = MultiSources {
            sources: vec![Source { path, text }],
        };
        SimulatorError::NoInitialBlock {
            module_name: module_name.to_string(),
            input,
            error_location: (*token).into(),
            token_source: token.beg.source,
        }
    }

    pub fn unresolved_expression(token: &TokenRange) -> Self {
        let path = token.beg.source.to_string();
        let text = token.beg.source.get_text();
        let input = MultiSources {
            sources: vec![Source { path, text }],
        };
        SimulatorError::UnresolvedExpression {
            input,
            error_location: (*token).into(),
            token_source: token.beg.source,
        }
    }

    pub fn unsupported_description(token: &TokenRange) -> Self {
        let path = token.beg.source.to_string();
        let text = token.beg.source.get_text();
        let input = MultiSources {
            sources: vec![Source { path, text }],
        };
        SimulatorError::UnsupportedDescription {
            input,
            error_location: (*token).into(),
            token_source: token.beg.source,
        }
    }

    pub fn recursive_function(function_name: &str, token: &TokenRange) -> Self {
        let path = token.beg.source.to_string();
        let text = token.beg.source.get_text();
        let input = MultiSources {
            sources: vec![Source { path, text }],
        };
        SimulatorError::RecursiveFunction {
            function_name: function_name.to_string(),
            input,
            error_location: (*token).into(),
            token_source: token.beg.source,
        }
    }

    pub fn combinational_loop(token: &TokenRange, participants: &[TokenRange]) -> Self {
        let (input, loop_participants) = source_with_context(token, participants);
        SimulatorError::CombinationalLoop {
            input,
            error_location: (*token).into(),
            loop_participants,
            token_source: token.beg.source,
        }
    }
}
