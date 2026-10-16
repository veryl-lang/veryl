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

    #[diagnostic(
        severity(Error),
        code(undetermined_width),
        help(
            "this is usually a width derived from an external SystemVerilog item (`$sv::`), whose value the simulator can't see"
        )
    )]
    #[error("width of {subject} can't be determined by the simulator")]
    UndeterminedWidth {
        /// Carries its own quotes; a variable with no user-facing name
        /// substitutes a phrase instead.
        subject: String,
        #[source_code]
        input: MultiSources,
        #[label("this width is not a compile-time constant for the simulator")]
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

    #[diagnostic(
        severity(Error),
        code(unsupported_string),
        help(
            "the simulator supports `string` in a `const`, and in a `let` or `var` written only by assignments in its own module"
        )
    )]
    #[error("{subject} of type `string` is not supported by the simulator")]
    UnsupportedString {
        /// Carries its own quotes where it names a variable.
        subject: String,
        #[source_code]
        input: MultiSources,
        #[label("the text would not fit in the storage the simulator gives it")]
        error_location: SourceSpan,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unordered_combinational_logic),
        help(
            "the simulator orders combinational logic statement by statement and found no order in which each of these reads only values already computed; splitting the `always_comb` blocks involved usually resolves it, and `veryl test --allow-multi-pass` (or `VERYL_ALLOW_MULTI_PASS=1`) evaluates them repeatedly until they settle instead"
        )
    )]
    #[error("combinational logic has no one-pass evaluation order")]
    UnorderedCombinationalLogic {
        #[source_code]
        input: MultiSources,
        #[label("Error location")]
        error_location: SourceSpan,
        #[label(collection, "part of a dependency the simulator could not order")]
        participants: Vec<SourceSpan>,
        token_source: TokenSource,
    },

    #[diagnostic(
        severity(Error),
        code(unproven_pass_count),
        help(
            "this is a simulator bug, please report it; `veryl test --allow-multi-pass` (or `VERYL_ALLOW_MULTI_PASS=1`) works around it"
        )
    )]
    #[error(
        "internal error: the combinational logic of module \"{module_name}\" was ordered but needs {passes} settle passes"
    )]
    UnprovenPassCount { module_name: String, passes: usize },

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
    // Each file goes in once, however many labels point into it.
    let mut bases: Vec<(TokenSource, usize)> = Vec::new();
    let mut sources = Vec::new();
    let mut end = 0;
    let mut base_of = |source: TokenSource, sources: &mut Vec<Source>| -> usize {
        if let Some((_, base)) = bases.iter().find(|(s, _)| *s == source) {
            return *base;
        }
        let text = source.get_text();
        let base = end;
        end += text.len();
        bases.push((source, base));
        sources.push(Source::new(source.to_string(), text));
        base
    };
    base_of(token.beg.source, &mut sources);

    let mut ranges = Vec::new();
    for x in context.iter().rev() {
        let mut range = *x;
        range.offset(base_of(x.beg.source, &mut sources) as u32);
        ranges.push(range.into());
    }

    let sources = MultiSources { sources };
    (sources, ranges)
}

impl SimulatorError {
    pub fn no_initial_block(module_name: &str, token: &TokenRange) -> Self {
        let path = token.beg.source.to_string();
        let text = token.beg.source.get_text();
        let input = MultiSources {
            sources: vec![Source::new(path, text)],
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
            sources: vec![Source::new(path, text)],
        };
        SimulatorError::UnresolvedExpression {
            input,
            error_location: (*token).into(),
            token_source: token.beg.source,
        }
    }

    pub fn undetermined_width(subject: &str, token: &TokenRange) -> Self {
        let source = token.beg.source;
        let input = MultiSources {
            sources: vec![Source::new(source.to_string(), source.get_text())],
        };
        SimulatorError::UndeterminedWidth {
            subject: subject.to_string(),
            input,
            error_location: (*token).into(),
            token_source: source,
        }
    }

    pub fn unsupported_description(token: &TokenRange) -> Self {
        let path = token.beg.source.to_string();
        let text = token.beg.source.get_text();
        let input = MultiSources {
            sources: vec![Source::new(path, text)],
        };
        SimulatorError::UnsupportedDescription {
            input,
            error_location: (*token).into(),
            token_source: token.beg.source,
        }
    }

    pub fn unsupported_string(subject: &str, token: &TokenRange) -> Self {
        let source = token.beg.source;
        let input = MultiSources {
            sources: vec![Source::new(source.to_string(), source.get_text())],
        };
        SimulatorError::UnsupportedString {
            subject: subject.to_string(),
            input,
            error_location: (*token).into(),
            token_source: source,
        }
    }

    pub fn recursive_function(function_name: &str, token: &TokenRange) -> Self {
        let path = token.beg.source.to_string();
        let text = token.beg.source.get_text();
        let input = MultiSources {
            sources: vec![Source::new(path, text)],
        };
        SimulatorError::RecursiveFunction {
            function_name: function_name.to_string(),
            input,
            error_location: (*token).into(),
            token_source: token.beg.source,
        }
    }

    pub fn unordered_combinational_logic(token: &TokenRange, participants: &[TokenRange]) -> Self {
        let (input, participants) = source_with_context(token, participants);
        SimulatorError::UnorderedCombinationalLogic {
            input,
            error_location: (*token).into(),
            participants,
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
