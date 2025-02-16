use miette::{self, Diagnostic, NamedSource, SourceSpan};
use parol_runtime::{ParolError, TokenVec};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum ParserError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SyntaxError(SyntaxError),

    #[error(transparent)]
    ParserError(#[from] parol_runtime::ParserError),

    #[error(transparent)]
    LexerError(#[from] parol_runtime::LexerError),

    #[error(transparent)]
    UserError(#[from] anyhow::Error),
}

#[derive(Error, Diagnostic, Debug)]
#[diagnostic(help(""), code(ParserError::SyntaxError))]
pub struct SyntaxError {
    pub cause: String,
    #[source_code]
    input: NamedSource<FileSource>,
    #[label("Error location")]
    pub error_location: SourceSpan,
    pub unexpected_tokens: Vec<UnexpectedToken>,
    pub expected_tokens: TokenVec,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.unexpected_tokens.is_empty() {
            let token = self.unexpected_tokens[0].token_type;
            if token == TokenType::LAngle {
                f.write_str(&format!(
                    "Unexpected token: '{}', do you mean \"less than operator '<:'\" ?",
                    token
                ))
            } else if token == TokenType::RAngle {
                f.write_str(&format!(
                    "Unexpected token: '{}', do you mean \"greater than operator '>:'\" ?",
                    token
                ))
            } else {
                f.write_str(&format!("Unexpected token: '{}'", token))
            }
        } else {
            f.write_str("Syntax Error")
        }
    }
}

impl From<parol_runtime::SyntaxError> for SyntaxError {
    fn from(value: parol_runtime::SyntaxError) -> Self {
        Self {
            cause: value.cause,
            input: value.input.map(|e| FileSource(*e).into()).unwrap(),
            error_location: Location(*value.error_location).into(),
            unexpected_tokens: UnexpectedTokens(value.unexpected_tokens).into(),
            expected_tokens: value.expected_tokens,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Comments,
    StringLiteral,
    Exponent,
    FixedPoint,
    Based,
    AllBit,
    BaseLess,
    MinusColon,
    MinusGT,
    PlusColon,
    AssignmentOperator,
    Operator11,
    Operator10,
    Operator09,
    Operator08,
    Operator07,
    Operator06,
    Operator02,
    Operator01,
    Operator05,
    Operator04,
    Operator03,
    UnaryOperator,
    ColonColon,
    Colon,
    Comma,
    Dollar,
    DotDotEqu,
    DotDot,
    Dot,
    Equ,
    Hash,
    LAngle,
    LBrace,
    LBracket,
    LParen,
    RAngle,
    RBrace,
    RBracket,
    RParen,
    Semicolon,
    Star,
    AlwaysComb,
    AlwaysFf,
    Assign,
    AsyncHigh,
    AsyncLow,
    As,
    Bit,
    Case,
    Default,
    Else,
    Enum,
    Export,
    F32,
    F64,
    Final,
    For,
    Function,
    I32,
    I64,
    IfReset,
    If,
    Import,
    Initial,
    Inout,
    Input,
    Inside,
    Inst,
    Interface,
    In,
    Let,
    Local,
    Logic,
    Lsb,
    Modport,
    Module,
    Msb,
    Negedge,
    Output,
    Outside,
    Package,
    Param,
    Posedge,
    Pub,
    Ref,
    Repeat,
    Return,
    Signed,
    Step,
    String,
    Struct,
    SyncHigh,
    SyncLow,
    Tri,
    Type,
    U32,
    U64,
    Union,
    Var,
    Identifier,
    Error,
}

impl From<&str> for TokenType {
    fn from(value: &str) -> Self {
        match value {
            "CommentsTerm" => TokenType::Comments,
            "StringLiteralTerm" => TokenType::StringLiteral,
            "ExponentTerm" => TokenType::Exponent,
            "FixedPointTerm" => TokenType::FixedPoint,
            "BasedTerm" => TokenType::Based,
            "AllBitTerm" => TokenType::AllBit,
            "BaseLessTerm" => TokenType::BaseLess,
            "MinusColonTerm" => TokenType::MinusColon,
            "MinusGTTerm" => TokenType::MinusGT,
            "PlusColonTerm" => TokenType::PlusColon,
            "AssignmentOperatorTerm" => TokenType::AssignmentOperator,
            "Operator11Term" => TokenType::Operator11,
            "Operator10Term" => TokenType::Operator10,
            "Operator09Term" => TokenType::Operator09,
            "Operator08Term" => TokenType::Operator08,
            "Operator07Term" => TokenType::Operator07,
            "Operator06Term" => TokenType::Operator06,
            "Operator02Term" => TokenType::Operator02,
            "Operator01Term" => TokenType::Operator01,
            "Operator05Term" => TokenType::Operator05,
            "Operator04Term" => TokenType::Operator04,
            "Operator03Term" => TokenType::Operator03,
            "UnaryOperatorTerm" => TokenType::UnaryOperator,
            "ColonColonTerm" => TokenType::ColonColon,
            "ColonTerm" => TokenType::Colon,
            "CommaTerm" => TokenType::Comma,
            "DollarTerm" => TokenType::Dollar,
            "DotDotEquTerm" => TokenType::DotDotEqu,
            "DotDotTerm" => TokenType::DotDot,
            "DotTerm" => TokenType::Dot,
            "EquTerm" => TokenType::Equ,
            "HashTerm" => TokenType::Hash,
            "LAngleTerm" => TokenType::LAngle,
            "LBraceTerm" => TokenType::LBrace,
            "LBracketTerm" => TokenType::LBracket,
            "LParenTerm" => TokenType::LParen,
            "RAngleTerm" => TokenType::RAngle,
            "RBraceTerm" => TokenType::RBrace,
            "RBracketTerm" => TokenType::RBracket,
            "RParenTerm" => TokenType::RParen,
            "SemicolonTerm" => TokenType::Semicolon,
            "StarTerm" => TokenType::Star,
            "AlwaysCombTerm" => TokenType::AlwaysComb,
            "AlwaysFfTerm" => TokenType::AlwaysFf,
            "AssignTerm" => TokenType::Assign,
            "AsyncHighTerm" => TokenType::AsyncHigh,
            "AsyncLowTerm" => TokenType::AsyncLow,
            "AsTerm" => TokenType::As,
            "BitTerm" => TokenType::Bit,
            "CaseTerm" => TokenType::Case,
            "DefaultTerm" => TokenType::Default,
            "ElseTerm" => TokenType::Else,
            "EnumTerm" => TokenType::Enum,
            "ExportTerm" => TokenType::Export,
            "F32Term" => TokenType::F32,
            "F64Term" => TokenType::F64,
            "FinalTerm" => TokenType::Final,
            "ForTerm" => TokenType::For,
            "FunctionTerm" => TokenType::Function,
            "I32Term" => TokenType::I32,
            "I64Term" => TokenType::I64,
            "IfResetTerm" => TokenType::IfReset,
            "IfTerm" => TokenType::If,
            "ImportTerm" => TokenType::Import,
            "InitialTerm" => TokenType::Initial,
            "InoutTerm" => TokenType::Inout,
            "InputTerm" => TokenType::Input,
            "InsideTerm" => TokenType::Inside,
            "InstTerm" => TokenType::Inst,
            "InterfaceTerm" => TokenType::Interface,
            "InTerm" => TokenType::In,
            "LetTerm" => TokenType::Let,
            "LocalTerm" => TokenType::Local,
            "LogicTerm" => TokenType::Logic,
            "LsbTerm" => TokenType::Lsb,
            "ModportTerm" => TokenType::Modport,
            "ModuleTerm" => TokenType::Module,
            "MsbTerm" => TokenType::Msb,
            "NegedgeTerm" => TokenType::Negedge,
            "OutputTerm" => TokenType::Output,
            "OutsideTerm" => TokenType::Outside,
            "PackageTerm" => TokenType::Package,
            "ParamTerm" => TokenType::Param,
            "PosedgeTerm" => TokenType::Posedge,
            "PubTerm" => TokenType::Pub,
            "RefTerm" => TokenType::Ref,
            "RepeatTerm" => TokenType::Repeat,
            "ReturnTerm" => TokenType::Return,
            "SignedTerm" => TokenType::Signed,
            "StepTerm" => TokenType::Step,
            "StringTerm" => TokenType::String,
            "StructTerm" => TokenType::Struct,
            "SyncHighTerm" => TokenType::SyncHigh,
            "SyncLowTerm" => TokenType::SyncLow,
            "TriTerm" => TokenType::Tri,
            "TypeTerm" => TokenType::Type,
            "U32Term" => TokenType::U32,
            "U64Term" => TokenType::U64,
            "UnionTerm" => TokenType::Union,
            "VarTerm" => TokenType::Var,
            "IdentifierTerm" => TokenType::Identifier,
            _ => TokenType::Error,
        }
    }
}

impl std::fmt::Display for TokenType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            TokenType::Comments => "comment",
            TokenType::StringLiteral => "string literal",
            TokenType::Exponent => "number",
            TokenType::FixedPoint => "number",
            TokenType::Based => "number",
            TokenType::AllBit => "number",
            TokenType::BaseLess => "number",
            TokenType::MinusColon => "-:",
            TokenType::MinusGT => "->",
            TokenType::PlusColon => "+:",
            TokenType::AssignmentOperator => "assignment operator",
            TokenType::Operator11 => "operator",
            TokenType::Operator10 => "operator",
            TokenType::Operator09 => "operator",
            TokenType::Operator08 => "operator",
            TokenType::Operator07 => "operator",
            TokenType::Operator06 => "operator",
            TokenType::Operator02 => "operator",
            TokenType::Operator01 => "operator",
            TokenType::Operator05 => "operator",
            TokenType::Operator04 => "operator",
            TokenType::Operator03 => "operator",
            TokenType::UnaryOperator => "operator",
            TokenType::ColonColon => "::",
            TokenType::Colon => ":",
            TokenType::Comma => ",",
            TokenType::Dollar => "$",
            TokenType::DotDotEqu => "..=",
            TokenType::DotDot => "..",
            TokenType::Dot => ".",
            TokenType::Equ => "=",
            TokenType::Hash => "#",
            TokenType::LAngle => "<",
            TokenType::LBrace => "{",
            TokenType::LBracket => "[",
            TokenType::LParen => "(",
            TokenType::RAngle => ">",
            TokenType::RBrace => "}",
            TokenType::RBracket => "]",
            TokenType::RParen => ")",
            TokenType::Semicolon => ";",
            TokenType::Star => "*",
            TokenType::AlwaysComb => "always_comb",
            TokenType::AlwaysFf => "always_ff",
            TokenType::Assign => "assign",
            TokenType::AsyncHigh => "async_high",
            TokenType::AsyncLow => "async_low",
            TokenType::As => "as",
            TokenType::Bit => "bit",
            TokenType::Case => "case",
            TokenType::Default => "default",
            TokenType::Else => "else",
            TokenType::Enum => "enum",
            TokenType::Export => "export",
            TokenType::F32 => "f32",
            TokenType::F64 => "f64",
            TokenType::Final => "final",
            TokenType::For => "for",
            TokenType::Function => "function",
            TokenType::I32 => "i32",
            TokenType::I64 => "i64",
            TokenType::IfReset => "if_reset",
            TokenType::If => "if",
            TokenType::Import => "import",
            TokenType::Initial => "initial",
            TokenType::Inout => "inout",
            TokenType::Input => "input",
            TokenType::Inside => "inside",
            TokenType::Inst => "inst",
            TokenType::Interface => "interface",
            TokenType::In => "in",
            TokenType::Let => "let",
            TokenType::Local => "local",
            TokenType::Logic => "logic",
            TokenType::Lsb => "lsb",
            TokenType::Modport => "modport",
            TokenType::Module => "module",
            TokenType::Msb => "msb",
            TokenType::Negedge => "negedge",
            TokenType::Output => "output",
            TokenType::Outside => "outside",
            TokenType::Package => "package",
            TokenType::Param => "param",
            TokenType::Posedge => "posedge",
            TokenType::Pub => "pub",
            TokenType::Ref => "ref",
            TokenType::Repeat => "repeat",
            TokenType::Return => "return",
            TokenType::Signed => "signed",
            TokenType::Step => "step",
            TokenType::String => "string",
            TokenType::Struct => "struct",
            TokenType::SyncHigh => "sync_high",
            TokenType::SyncLow => "sync_low",
            TokenType::Tri => "tri",
            TokenType::Type => "type",
            TokenType::U32 => "u32",
            TokenType::U64 => "u64",
            TokenType::Union => "union",
            TokenType::Var => "var",
            TokenType::Identifier => "identifier",
            TokenType::Error => "error",
        };
        text.fmt(f)
    }
}

impl From<ParolError> for ParserError {
    fn from(x: ParolError) -> ParserError {
        match x {
            ParolError::ParserError(x) => match x {
                parol_runtime::ParserError::SyntaxErrors { mut entries } if !entries.is_empty() => {
                    ParserError::SyntaxError(entries.remove(0).into())
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
