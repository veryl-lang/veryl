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
        if !self.unexpected_tokens.is_empty() {
            let token = self.unexpected_tokens[0].token_type;
            f.write_str(&format!("Unexpected token: '{token}'"))
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

impl From<parol_runtime::SyntaxError> for SyntaxError {
    fn from(value: parol_runtime::SyntaxError) -> Self {
        let unexpected_tokens: Vec<_> = UnexpectedTokens(value.unexpected_tokens).into();
        let expected_tokens: ExpectedTokens = (&value.expected_tokens).into();

        let mut help = String::new();
        if !unexpected_tokens.is_empty() {
            let token = unexpected_tokens[0].token_type;
            if l_angle(token, &expected_tokens) {
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
            error_location: Location(*value.error_location).into(),
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

struct UnexpectedTokens(Vec<parol_runtime::UnexpectedToken>);

impl From<UnexpectedTokens> for Vec<UnexpectedToken> {
    fn from(value: UnexpectedTokens) -> Self {
        value
            .0
            .into_iter()
            .map(|v| UnexpectedToken {
                name: v.name,
                token_type: v.token_type.as_str().into(),
                token: Location(v.token).into(),
            })
            .collect::<Vec<UnexpectedToken>>()
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
