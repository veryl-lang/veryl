use miette::{self, Diagnostic, NamedSource, SourceSpan};
use parol_runtime::{ParolError, TokenVec};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum ParserError {
    #[error("Syntax error(s)")]
    #[diagnostic(
        help("Syntax error(s) in input prevents prediction of next production"),
        code(ParserError::SyntaxError)
    )]
    SyntaxErrors { entries: Vec<SyntaxError> },

    #[error(transparent)]
    ParserError(#[from] parol_runtime::ParserError),

    #[error(transparent)]
    LexerError(#[from] parol_runtime::LexerError),

    #[error(transparent)]
    UserError(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
#[error("{cause}")]
pub struct SyntaxError {
    pub cause: String,
    pub input: Option<NamedSource>,
    pub error_location: SourceSpan,
    pub unexpected_tokens: Vec<UnexpectedToken>,
    pub expected_tokens: TokenVec,
    pub source: Option<anyhow::Error>,
}

#[derive(Error, Diagnostic, Debug)]
#[error("Unexpected token: {name} ({token_type})")]
#[diagnostic(help("Unexpected token"), code(parol_runtime::unexpected_token))]
pub struct UnexpectedToken {
    name: String,
    token_type: String,
    #[label("Unexpected token")]
    pub(crate) token: SourceSpan,
}

impl From<ParolError> for ParserError {
    fn from(x: ParolError) -> ParserError {
        match x {
            ParolError::ParserError(x) => match x {
                parol_runtime::ParserError::SyntaxErrors { entries } => ParserError::SyntaxErrors {
                    entries: entries
                        .into_iter()
                        .map(|e| SyntaxError {
                            cause: e.cause,
                            input: e.input.map(|e| FileSource(*e).into()),
                            error_location: Location(*e.error_location).into(),
                            unexpected_tokens: UnexpectedTokens(e.unexpected_tokens).into(),
                            expected_tokens: e.expected_tokens,
                            source: e.source.map(|x| x.into()),
                        })
                        .collect::<Vec<SyntaxError>>(),
                },
                _ => ParserError::ParserError(x),
            },
            ParolError::LexerError(x) => ParserError::LexerError(x),
            ParolError::UserError(x) => ParserError::UserError(x),
        }
    }
}

struct FileSource(parol_runtime::FileSource);

impl miette::SourceCode for FileSource {
    fn read_span<'a>(
        &'a self,
        span: &SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> Result<Box<dyn miette::SpanContents<'a> + 'a>, miette::MietteError> {
        <str as miette::SourceCode>::read_span(
            &self.0.input,
            span,
            context_lines_before,
            context_lines_after,
        )
    }
}

impl From<FileSource> for NamedSource {
    fn from(file_source: FileSource) -> Self {
        let file_name = file_source.0.file_name.clone();
        let file_name = file_name.to_str().unwrap_or("<Bad file name>");
        Self::new(file_name, file_source)
    }
}

struct Location(parol_runtime::Location);

impl From<Location> for SourceSpan {
    fn from(location: Location) -> Self {
        SourceSpan::new(
            (location.0.scanner_switch_pos + location.0.offset - location.0.length as usize).into(),
            (location.0.length as usize).into(),
        )
    }
}

struct UnexpectedTokens(Vec<parol_runtime::UnexpectedToken>);

impl From<UnexpectedTokens> for Vec<UnexpectedToken> {
    fn from(value: UnexpectedTokens) -> Self {
        value
            .0
            .into_iter()
            .map(|v| UnexpectedToken {
                name: v.name,
                token_type: v.token_type,
                token: Location(v.token).into(),
            })
            .collect::<Vec<UnexpectedToken>>()
    }
}
