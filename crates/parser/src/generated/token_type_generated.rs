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
    LTMinus,
    PlusColon,
    AssignmentOperator,
    DiamondOperator,
    Operator08,
    Operator07,
    Operator06,
    Operator02,
    Operator01,
    Operator05,
    Operator04,
    Operator03,
    UnaryOperator,
    ColonColonLAngle,
    ColonColon,
    Colon,
    Comma,
    DotDotEqu,
    DotDot,
    Dot,
    Equ,
    HashLBracket,
    Hash,
    LAngle,
    Question,
    QuoteLBrace,
    Quote,
    EscapedLBrace,
    TripleLBrace,
    LBrace,
    LBracket,
    LParen,
    RAngle,
    EscapedRBrace,
    TripleRBrace,
    RBrace,
    RBracket,
    RParen,
    Semicolon,
    Star,
    Alias,
    AlwaysComb,
    AlwaysFf,
    Assign,
    As,
    Bind,
    Bit,
    Block,
    BBool,
    LBool,
    Case,
    Clock,
    ClockPosedge,
    ClockNegedge,
    Connect,
    Const,
    Converse,
    Default,
    Else,
    Embed,
    Enum,
    F32,
    F64,
    False,
    Final,
    For,
    Function,
    Gen,
    I8,
    I16,
    I32,
    I64,
    IfReset,
    If,
    Import,
    Include,
    Initial,
    Inout,
    Input,
    Inside,
    Inst,
    Interface,
    In,
    Let,
    Logic,
    Lsb,
    Modport,
    Module,
    Msb,
    Output,
    Outside,
    Package,
    Param,
    Proto,
    Pub,
    Repeat,
    Reset,
    ResetAsyncHigh,
    ResetAsyncLow,
    ResetSyncHigh,
    ResetSyncLow,
    Return,
    Rev,
    Break,
    Same,
    Signed,
    Step,
    String,
    Struct,
    Switch,
    Tri,
    True,
    Type,
    P8,
    P16,
    P32,
    P64,
    U8,
    U16,
    U32,
    U64,
    Union,
    Unsafe,
    Var,
    DollarIdentifier,
    Identifier,
    Any,
    Error,
}

impl TokenType {
    pub fn is_keyword(&self) -> bool {
        matches!(
            self,
            TokenType::Alias
                | TokenType::AlwaysComb
                | TokenType::AlwaysFf
                | TokenType::Assign
                | TokenType::As
                | TokenType::Bind
                | TokenType::Bit
                | TokenType::Block
                | TokenType::BBool
                | TokenType::LBool
                | TokenType::Case
                | TokenType::Clock
                | TokenType::ClockPosedge
                | TokenType::ClockNegedge
                | TokenType::Connect
                | TokenType::Const
                | TokenType::Converse
                | TokenType::Default
                | TokenType::Else
                | TokenType::Embed
                | TokenType::Enum
                | TokenType::F32
                | TokenType::F64
                | TokenType::False
                | TokenType::Final
                | TokenType::For
                | TokenType::Function
                | TokenType::Gen
                | TokenType::I8
                | TokenType::I16
                | TokenType::I32
                | TokenType::I64
                | TokenType::IfReset
                | TokenType::If
                | TokenType::Import
                | TokenType::Include
                | TokenType::Initial
                | TokenType::Inout
                | TokenType::Input
                | TokenType::Inside
                | TokenType::Inst
                | TokenType::Interface
                | TokenType::In
                | TokenType::Let
                | TokenType::Logic
                | TokenType::Lsb
                | TokenType::Modport
                | TokenType::Module
                | TokenType::Msb
                | TokenType::Output
                | TokenType::Outside
                | TokenType::Package
                | TokenType::Param
                | TokenType::Proto
                | TokenType::Pub
                | TokenType::Repeat
                | TokenType::Reset
                | TokenType::ResetAsyncHigh
                | TokenType::ResetAsyncLow
                | TokenType::ResetSyncHigh
                | TokenType::ResetSyncLow
                | TokenType::Return
                | TokenType::Rev
                | TokenType::Break
                | TokenType::Same
                | TokenType::Signed
                | TokenType::Step
                | TokenType::String
                | TokenType::Struct
                | TokenType::Switch
                | TokenType::Tri
                | TokenType::True
                | TokenType::Type
                | TokenType::P8
                | TokenType::P16
                | TokenType::P32
                | TokenType::P64
                | TokenType::U8
                | TokenType::U16
                | TokenType::U32
                | TokenType::U64
                | TokenType::Union
                | TokenType::Unsafe
                | TokenType::Var
        )
    }
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
            "LTMinusTerm" => TokenType::LTMinus,
            "PlusColonTerm" => TokenType::PlusColon,
            "AssignmentOperatorTerm" => TokenType::AssignmentOperator,
            "DiamondOperatorTerm" => TokenType::DiamondOperator,
            "Operator08Term" => TokenType::Operator08,
            "Operator07Term" => TokenType::Operator07,
            "Operator06Term" => TokenType::Operator06,
            "Operator02Term" => TokenType::Operator02,
            "Operator01Term" => TokenType::Operator01,
            "Operator05Term" => TokenType::Operator05,
            "Operator04Term" => TokenType::Operator04,
            "Operator03Term" => TokenType::Operator03,
            "UnaryOperatorTerm" => TokenType::UnaryOperator,
            "ColonColonLAngleTerm" => TokenType::ColonColonLAngle,
            "ColonColonTerm" => TokenType::ColonColon,
            "ColonTerm" => TokenType::Colon,
            "CommaTerm" => TokenType::Comma,
            "DotDotEquTerm" => TokenType::DotDotEqu,
            "DotDotTerm" => TokenType::DotDot,
            "DotTerm" => TokenType::Dot,
            "EquTerm" => TokenType::Equ,
            "HashLBracketTerm" => TokenType::HashLBracket,
            "HashTerm" => TokenType::Hash,
            "LAngleTerm" => TokenType::LAngle,
            "QuestionTerm" => TokenType::Question,
            "QuoteLBraceTerm" => TokenType::QuoteLBrace,
            "QuoteTerm" => TokenType::Quote,
            "EscapedLBraceTerm" => TokenType::EscapedLBrace,
            "TripleLBraceTerm" => TokenType::TripleLBrace,
            "LBraceTerm" => TokenType::LBrace,
            "LBracketTerm" => TokenType::LBracket,
            "LParenTerm" => TokenType::LParen,
            "RAngleTerm" => TokenType::RAngle,
            "EscapedRBraceTerm" => TokenType::EscapedRBrace,
            "TripleRBraceTerm" => TokenType::TripleRBrace,
            "RBraceTerm" => TokenType::RBrace,
            "RBracketTerm" => TokenType::RBracket,
            "RParenTerm" => TokenType::RParen,
            "SemicolonTerm" => TokenType::Semicolon,
            "StarTerm" => TokenType::Star,
            "AliasTerm" => TokenType::Alias,
            "AlwaysCombTerm" => TokenType::AlwaysComb,
            "AlwaysFfTerm" => TokenType::AlwaysFf,
            "AssignTerm" => TokenType::Assign,
            "AsTerm" => TokenType::As,
            "BindTerm" => TokenType::Bind,
            "BitTerm" => TokenType::Bit,
            "BlockTerm" => TokenType::Block,
            "BBoolTerm" => TokenType::BBool,
            "LBoolTerm" => TokenType::LBool,
            "CaseTerm" => TokenType::Case,
            "ClockTerm" => TokenType::Clock,
            "ClockPosedgeTerm" => TokenType::ClockPosedge,
            "ClockNegedgeTerm" => TokenType::ClockNegedge,
            "ConnectTerm" => TokenType::Connect,
            "ConstTerm" => TokenType::Const,
            "ConverseTerm" => TokenType::Converse,
            "DefaultTerm" => TokenType::Default,
            "ElseTerm" => TokenType::Else,
            "EmbedTerm" => TokenType::Embed,
            "EnumTerm" => TokenType::Enum,
            "F32Term" => TokenType::F32,
            "F64Term" => TokenType::F64,
            "FalseTerm" => TokenType::False,
            "FinalTerm" => TokenType::Final,
            "ForTerm" => TokenType::For,
            "FunctionTerm" => TokenType::Function,
            "GenTerm" => TokenType::Gen,
            "I8Term" => TokenType::I8,
            "I16Term" => TokenType::I16,
            "I32Term" => TokenType::I32,
            "I64Term" => TokenType::I64,
            "IfResetTerm" => TokenType::IfReset,
            "IfTerm" => TokenType::If,
            "ImportTerm" => TokenType::Import,
            "IncludeTerm" => TokenType::Include,
            "InitialTerm" => TokenType::Initial,
            "InoutTerm" => TokenType::Inout,
            "InputTerm" => TokenType::Input,
            "InsideTerm" => TokenType::Inside,
            "InstTerm" => TokenType::Inst,
            "InterfaceTerm" => TokenType::Interface,
            "InTerm" => TokenType::In,
            "LetTerm" => TokenType::Let,
            "LogicTerm" => TokenType::Logic,
            "LsbTerm" => TokenType::Lsb,
            "ModportTerm" => TokenType::Modport,
            "ModuleTerm" => TokenType::Module,
            "MsbTerm" => TokenType::Msb,
            "OutputTerm" => TokenType::Output,
            "OutsideTerm" => TokenType::Outside,
            "PackageTerm" => TokenType::Package,
            "ParamTerm" => TokenType::Param,
            "ProtoTerm" => TokenType::Proto,
            "PubTerm" => TokenType::Pub,
            "RepeatTerm" => TokenType::Repeat,
            "ResetTerm" => TokenType::Reset,
            "ResetAsyncHighTerm" => TokenType::ResetAsyncHigh,
            "ResetAsyncLowTerm" => TokenType::ResetAsyncLow,
            "ResetSyncHighTerm" => TokenType::ResetSyncHigh,
            "ResetSyncLowTerm" => TokenType::ResetSyncLow,
            "ReturnTerm" => TokenType::Return,
            "RevTerm" => TokenType::Rev,
            "BreakTerm" => TokenType::Break,
            "SameTerm" => TokenType::Same,
            "SignedTerm" => TokenType::Signed,
            "StepTerm" => TokenType::Step,
            "StringTerm" => TokenType::String,
            "StructTerm" => TokenType::Struct,
            "SwitchTerm" => TokenType::Switch,
            "TriTerm" => TokenType::Tri,
            "TrueTerm" => TokenType::True,
            "TypeTerm" => TokenType::Type,
            "P8Term" => TokenType::P8,
            "P16Term" => TokenType::P16,
            "P32Term" => TokenType::P32,
            "P64Term" => TokenType::P64,
            "U8Term" => TokenType::U8,
            "U16Term" => TokenType::U16,
            "U32Term" => TokenType::U32,
            "U64Term" => TokenType::U64,
            "UnionTerm" => TokenType::Union,
            "UnsafeTerm" => TokenType::Unsafe,
            "VarTerm" => TokenType::Var,
            "DollarIdentifierTerm" => TokenType::DollarIdentifier,
            "IdentifierTerm" => TokenType::Identifier,
            "AnyTerm" => TokenType::Any,
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
            TokenType::LTMinus => "<-",
            TokenType::PlusColon => "+:",
            TokenType::AssignmentOperator => "assignment operator",
            TokenType::DiamondOperator => "<>",
            TokenType::Operator08 => "operator",
            TokenType::Operator07 => "operator",
            TokenType::Operator06 => "operator",
            TokenType::Operator02 => "operator",
            TokenType::Operator01 => "operator",
            TokenType::Operator05 => "operator",
            TokenType::Operator04 => "operator",
            TokenType::Operator03 => "operator",
            TokenType::UnaryOperator => "operator",
            TokenType::ColonColonLAngle => "::<",
            TokenType::ColonColon => "::",
            TokenType::Colon => ":",
            TokenType::Comma => ",",
            TokenType::DotDotEqu => "..=",
            TokenType::DotDot => "..",
            TokenType::Dot => ".",
            TokenType::Equ => "=",
            TokenType::HashLBracket => "#[",
            TokenType::Hash => "#",
            TokenType::LAngle => "<",
            TokenType::Question => "?",
            TokenType::QuoteLBrace => "'{",
            TokenType::Quote => "'",
            TokenType::EscapedLBrace => "\\{",
            TokenType::TripleLBrace => "{{{",
            TokenType::LBrace => "{",
            TokenType::LBracket => "[",
            TokenType::LParen => "(",
            TokenType::RAngle => ">",
            TokenType::EscapedRBrace => "\\}",
            TokenType::TripleRBrace => "}}}",
            TokenType::RBrace => "}",
            TokenType::RBracket => "]",
            TokenType::RParen => ")",
            TokenType::Semicolon => ";",
            TokenType::Star => "*",
            TokenType::Alias => "alias",
            TokenType::AlwaysComb => "always_comb",
            TokenType::AlwaysFf => "always_ff",
            TokenType::Assign => "assign",
            TokenType::As => "as",
            TokenType::Bind => "bind",
            TokenType::Bit => "bit",
            TokenType::Block => "block",
            TokenType::BBool => "bbool",
            TokenType::LBool => "lbool",
            TokenType::Case => "case",
            TokenType::Clock => "clock",
            TokenType::ClockPosedge => "clock_posedge",
            TokenType::ClockNegedge => "clock_negedge",
            TokenType::Connect => "connect",
            TokenType::Const => "const",
            TokenType::Converse => "converse",
            TokenType::Default => "default",
            TokenType::Else => "else",
            TokenType::Embed => "embed",
            TokenType::Enum => "enum",
            TokenType::F32 => "f32",
            TokenType::F64 => "f64",
            TokenType::False => "false",
            TokenType::Final => "final",
            TokenType::For => "for",
            TokenType::Function => "function",
            TokenType::Gen => "gen",
            TokenType::I8 => "i8",
            TokenType::I16 => "i16",
            TokenType::I32 => "i32",
            TokenType::I64 => "i64",
            TokenType::IfReset => "if_reset",
            TokenType::If => "if",
            TokenType::Import => "import",
            TokenType::Include => "include",
            TokenType::Initial => "initial",
            TokenType::Inout => "inout",
            TokenType::Input => "input",
            TokenType::Inside => "inside",
            TokenType::Inst => "inst",
            TokenType::Interface => "interface",
            TokenType::In => "in",
            TokenType::Let => "let",
            TokenType::Logic => "logic",
            TokenType::Lsb => "lsb",
            TokenType::Modport => "modport",
            TokenType::Module => "module",
            TokenType::Msb => "msb",
            TokenType::Output => "output",
            TokenType::Outside => "outside",
            TokenType::Package => "package",
            TokenType::Param => "param",
            TokenType::Proto => "proto",
            TokenType::Pub => "pub",
            TokenType::Repeat => "repeat",
            TokenType::Reset => "reset",
            TokenType::ResetAsyncHigh => "reset_async_high",
            TokenType::ResetAsyncLow => "reset_async_low",
            TokenType::ResetSyncHigh => "reset_sync_high",
            TokenType::ResetSyncLow => "reset_sync_low",
            TokenType::Return => "return",
            TokenType::Rev => "rev",
            TokenType::Break => "break",
            TokenType::Same => "same",
            TokenType::Signed => "signed",
            TokenType::Step => "step",
            TokenType::String => "string",
            TokenType::Struct => "struct",
            TokenType::Switch => "switch",
            TokenType::Tri => "tri",
            TokenType::True => "true",
            TokenType::Type => "type",
            TokenType::P8 => "p8",
            TokenType::P16 => "p16",
            TokenType::P32 => "p32",
            TokenType::P64 => "p64",
            TokenType::U8 => "u8",
            TokenType::U16 => "u16",
            TokenType::U32 => "u32",
            TokenType::U64 => "u64",
            TokenType::Union => "union",
            TokenType::Unsafe => "unsafe",
            TokenType::Var => "var",
            TokenType::DollarIdentifier => "$identifier",
            TokenType::Identifier => "identifier",
            TokenType::Any => "embed content",
            TokenType::Error => "error",
        };
        text.fmt(f)
    }
}
