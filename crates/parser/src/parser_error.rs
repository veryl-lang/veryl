use miette::{self, Diagnostic, NamedSource, SourceSpan};
use parol_runtime::{ParolError, TokenVec};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum ParserError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SyntaxError(Box<SyntaxError>),

    #[error(transparent)]
    ParserError(#[from] parol_runtime::ParserError),

    #[error(transparent)]
    LexerError(#[from] parol_runtime::LexerError),

    #[error(transparent)]
    UserError(#[from] anyhow::Error),
}

#[derive(Error, Diagnostic, Debug)]
#[diagnostic(code(ParserError::SyntaxError))]
pub struct SyntaxError {
    pub cause: String,
    #[source_code]
    input: NamedSource<FileSource>,
    #[label("Error location")]
    pub error_location: SourceSpan,
    pub unexpected_tokens: Vec<UnexpectedToken>,
    pub expected_tokens: ExpectedTokens,
    #[help]
    pub help: String,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The last lookahead token is the actual divergence; earlier ones form a valid prefix.
        if let Some(token) = self.unexpected_tokens.last() {
            // `TokenType::Error` (an unlexable character) displays as "error"; show its text.
            if token.token_type == TokenType::Error
                && let Some(text) = &token.text
            {
                write!(f, "Unexpected token: '{text}'")
            } else {
                write!(f, "Unexpected token: '{}'", token.token_type)
            }
        } else {
            f.write_str("Syntax Error")
        }
    }
}

fn l_angle(unexpected_token: TokenType, _expected_tokens: &ExpectedTokens) -> bool {
    unexpected_token == TokenType::LAngle
}

fn r_angle(unexpected_token: TokenType, _expected_tokens: &ExpectedTokens) -> bool {
    unexpected_token == TokenType::RAngle
}

fn colon_instead_of_in(unexpected_token: TokenType, expected_tokens: &ExpectedTokens) -> bool {
    unexpected_token == TokenType::Colon && expected_tokens.any(TokenType::In)
}

fn comma_instead_of_assignment_operator(
    unexpected_token: TokenType,
    expected_tokens: &ExpectedTokens,
) -> bool {
    unexpected_token == TokenType::Comma && expected_tokens.any(TokenType::AssignmentOperator)
}

fn l_brace_instead_of_colon(unexpected_token: TokenType, expected_tokens: &ExpectedTokens) -> bool {
    unexpected_token == TokenType::LBrace && expected_tokens.any(TokenType::Colon)
}

fn keyword_as_identifier(unexpected_token: TokenType, expected_tokens: &ExpectedTokens) -> bool {
    unexpected_token.is_keyword() && expected_tokens.any(TokenType::Identifier)
}

fn block_or_if_after_else(
    unexpected_tokens: &[UnexpectedToken],
    expected_tokens: &ExpectedTokens,
) -> bool {
    // `else` is valid after the preceding block, so the hint targets what must follow it.
    let after_else = unexpected_tokens.len() >= 2
        && unexpected_tokens[unexpected_tokens.len() - 2].token_type == TokenType::Else;
    after_else && expected_tokens.any(TokenType::LBrace) && expected_tokens.any(TokenType::If)
}

impl From<parol_runtime::SyntaxError> for SyntaxError {
    fn from(value: parol_runtime::SyntaxError) -> Self {
        let source = value.input.as_deref().map(|f| f.input.as_str());
        let unexpected_tokens: Vec<UnexpectedToken> = value
            .unexpected_tokens
            .into_iter()
            .map(|v| {
                let token: SourceSpan = Location(v.token).into();
                let token_type: TokenType = v.token_type.as_str().into();
                let text = (token_type == TokenType::Error)
                    .then(|| {
                        source.and_then(|s| s.get(token.offset()..token.offset() + token.len()))
                    })
                    .flatten()
                    .map(str::to_string);
                UnexpectedToken {
                    name: v.name,
                    token_type,
                    token,
                    text,
                }
            })
            .collect();
        let expected_tokens: ExpectedTokens = (&value.expected_tokens).into();

        let mut help = String::new();
        if let Some(token) = unexpected_tokens.last() {
            let token = token.token_type;
            if block_or_if_after_else(&unexpected_tokens, &expected_tokens) {
                help = "'else' must be followed by a block ('{ ... }') or 'if'".to_string();
            } else if l_angle(token, &expected_tokens) {
                help = "If you mean \"less than operator\", please use '<:'".to_string();
            } else if r_angle(token, &expected_tokens) {
                help = "If you mean \"greater than operator\", please use '>:'".to_string();
            } else if colon_instead_of_in(token, &expected_tokens) {
                help = "for declaration doesn't need type specifier (e.g. 'for i in 0..10 {')"
                    .to_string();
            } else if comma_instead_of_assignment_operator(token, &expected_tokens) {
                help = "single case statement with bit concatenation at the left-hand side is not allowed,\nplease surround it by '{}' (e.g. 'x: { {a, b} = 1; }')".to_string();
            } else if l_brace_instead_of_colon(token, &expected_tokens) {
                help =
                    "The first arm of generate-if declaration needs label (e.g. 'if x :label {')"
                        .to_string();
            } else if keyword_as_identifier(token, &expected_tokens) {
                help = format!(
                    "'{}' is a reserved keyword and cannot be used as an identifier",
                    token
                );
            }
        }

        Self {
            cause: value.cause,
            input: value.input.map(|e| FileSource(*e).into()).unwrap(),
            // parol points `error_location` at its LA(1), a valid prefix; use the divergence.
            error_location: unexpected_tokens
                .last()
                .map(|t| t.token)
                .unwrap_or_else(|| Location(*value.error_location).into()),
            unexpected_tokens,
            expected_tokens,
            help,
        }
    }
}

#[derive(Error, Diagnostic, Debug)]
#[error("Unexpected token: {name} ({token_type})")]
#[diagnostic(help("Unexpected token"), code(parol_runtime::unexpected_token))]
pub struct UnexpectedToken {
    name: String,
    token_type: TokenType,
    #[label("Unexpected token")]
    pub(crate) token: SourceSpan,
    // Source text, set only for `Error` tokens (see `Display`).
    text: Option<String>,
}

include!("generated/token_type_generated.rs");

impl From<ParolError> for ParserError {
    fn from(x: ParolError) -> ParserError {
        match x {
            ParolError::ParserError(x) => match x {
                parol_runtime::ParserError::SyntaxErrors { mut entries } if !entries.is_empty() => {
                    ParserError::SyntaxError(Box::new(entries.remove(0).into()))
                }
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

impl From<FileSource> for NamedSource<FileSource> {
    fn from(file_source: FileSource) -> Self {
        let file_name = file_source.0.file_name.clone();
        let file_name = file_name.to_str().unwrap_or("<Bad file name>");
        Self::new(file_name, file_source)
    }
}

struct Location(parol_runtime::Location);

impl From<Location> for SourceSpan {
    fn from(location: Location) -> Self {
        SourceSpan::new((location.0.start as usize).into(), location.0.len())
    }
}

#[derive(Debug)]
pub struct ExpectedTokens(Vec<TokenType>);

impl ExpectedTokens {
    pub fn any(&self, x: TokenType) -> bool {
        self.0.contains(&x)
    }
}

impl From<&TokenVec> for ExpectedTokens {
    fn from(value: &TokenVec) -> Self {
        ExpectedTokens(value.iter().map(|x| x.as_str().into()).collect())
    }
}
