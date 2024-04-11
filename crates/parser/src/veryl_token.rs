use crate::doc_comment_table;
use crate::resource_table::{self, PathId, StrId, TokenId};
use crate::veryl_grammar_trait::*;
use once_cell::sync::Lazy;
use paste::paste;
use regex::Regex;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenSource {
    File(PathId),
    Builtin,
    External,
}

impl fmt::Display for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            TokenSource::File(x) => x.to_string(),
            TokenSource::Builtin => "builtin".to_string(),
            TokenSource::External => "external".to_string(),
        };
        text.fmt(f)
    }
}

impl PartialEq<PathId> for TokenSource {
    fn eq(&self, other: &PathId) -> bool {
        if let TokenSource::File(x) = self {
            x == other
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token {
    pub id: TokenId,
    pub text: StrId,
    pub line: u32,
    pub column: u32,
    pub length: u32,
    pub pos: u32,
    pub source: TokenSource,
}

impl Token {
    pub fn new(
        text: &str,
        line: u32,
        column: u32,
        length: u32,
        pos: u32,
        source: TokenSource,
    ) -> Self {
        let id = resource_table::new_token_id();
        let text = resource_table::insert_str(text);
        Token {
            id,
            text,
            line,
            column,
            length,
            pos,
            source,
        }
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{}", self.text);
        text.fmt(f)
    }
}

impl<'t> TryFrom<&parol_runtime::lexer::Token<'t>> for Token {
    type Error = anyhow::Error;
    fn try_from(x: &parol_runtime::lexer::Token<'t>) -> Result<Self, anyhow::Error> {
        let id = resource_table::new_token_id();
        let text = resource_table::insert_str(x.text());
        let pos = x.location.scanner_switch_pos + x.location.offset - x.location.length as usize;
        let source = TokenSource::File(resource_table::insert_path(&x.location.file_name));
        Ok(Token {
            id,
            text,
            line: x.location.start_line,
            column: x.location.start_column,
            length: x.location.length,
            pos: pos as u32,
            source,
        })
    }
}

impl From<&Token> for miette::SourceSpan {
    fn from(x: &Token) -> Self {
        (x.pos as usize, x.length as usize).into()
    }
}

impl From<Token> for miette::SourceSpan {
    fn from(x: Token) -> Self {
        (x.pos as usize, x.length as usize).into()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TokenRange {
    pub beg: Token,
    pub end: Token,
}

impl TokenRange {
    pub fn new(beg: &VerylToken, end: &VerylToken) -> Self {
        Self {
            beg: beg.token,
            end: end.token,
        }
    }

    pub fn include(&self, path: PathId, line: u32, column: u32) -> bool {
        if self.beg.source == path {
            if self.beg.line == line {
                if self.end.line == line {
                    self.beg.column <= column && column <= self.end.column
                } else {
                    self.beg.column <= column
                }
            } else if self.end.line == line {
                column <= self.end.column
            } else {
                self.beg.line < line && line < self.end.line
            }
        } else {
            false
        }
    }
}

impl From<&TokenRange> for miette::SourceSpan {
    fn from(x: &TokenRange) -> Self {
        let length = (x.end.pos - x.beg.pos + x.end.length) as usize;
        (x.beg.pos as usize, length).into()
    }
}

impl From<TokenRange> for miette::SourceSpan {
    fn from(x: TokenRange) -> Self {
        let length = (x.end.pos - x.beg.pos + x.end.length) as usize;
        (x.beg.pos as usize, length).into()
    }
}

impl From<Token> for TokenRange {
    fn from(value: Token) -> Self {
        let beg = value;
        let end = value;
        TokenRange { beg, end }
    }
}

impl From<&Token> for TokenRange {
    fn from(value: &Token) -> Self {
        let beg = *value;
        let end = *value;
        TokenRange { beg, end }
    }
}

impl From<&Identifier> for TokenRange {
    fn from(value: &Identifier) -> Self {
        let beg = value.identifier_token.token;
        let end = value.identifier_token.token;
        TokenRange { beg, end }
    }
}

impl From<&HierarchicalIdentifier> for TokenRange {
    fn from(value: &HierarchicalIdentifier) -> Self {
        let beg = value.identifier.identifier_token.token;
        let mut end = value.identifier.identifier_token.token;
        if let Some(x) = value.hierarchical_identifier_list.last() {
            end = x.select.r_bracket.r_bracket_token.token;
        }
        if let Some(x) = value.hierarchical_identifier_list0.last() {
            end = x.identifier.identifier_token.token;
            if let Some(x) = x.hierarchical_identifier_list0_list.last() {
                end = x.select.r_bracket.r_bracket_token.token;
            }
        }
        TokenRange { beg, end }
    }
}

impl From<&ScopedIdentifier> for TokenRange {
    fn from(value: &ScopedIdentifier) -> Self {
        let mut beg = value.identifier.identifier_token.token;
        if let Some(ref x) = value.scoped_identifier_opt {
            beg = x.dollar.dollar_token.token;
        }
        let mut end = value.identifier.identifier_token.token;
        if let Some(x) = value.scoped_identifier_list.last() {
            end = x.identifier.identifier_token.token;
        }
        TokenRange { beg, end }
    }
}

impl From<&ExpressionIdentifier> for TokenRange {
    fn from(value: &ExpressionIdentifier) -> Self {
        let mut beg = value.identifier.identifier_token.token;
        if let Some(ref x) = value.expression_identifier_opt {
            beg = x.dollar.dollar_token.token;
        }
        let mut end = value.identifier.identifier_token.token;
        match &*value.expression_identifier_group {
            ExpressionIdentifierGroup::ExpressionIdentifierScoped(x) => {
                let x = &x.expression_identifier_scoped;
                end = x.identifier.identifier_token.token;
                if let Some(x) = x.expression_identifier_scoped_list.last() {
                    end = x.identifier.identifier_token.token;
                }
                if let Some(x) = x.expression_identifier_scoped_list0.last() {
                    end = x.select.r_bracket.r_bracket_token.token;
                }
            }
            ExpressionIdentifierGroup::ExpressionIdentifierMember(x) => {
                let x = &x.expression_identifier_member;
                if let Some(x) = x.expression_identifier_member_list.last() {
                    end = x.select.r_bracket.r_bracket_token.token;
                }
                if let Some(x) = x.expression_identifier_member_list0.last() {
                    end = x.identifier.identifier_token.token;
                    if let Some(x) = x.expression_identifier_member_list0_list.last() {
                        end = x.select.r_bracket.r_bracket_token.token;
                    }
                }
            }
        }
        TokenRange { beg, end }
    }
}

impl From<&AlwaysFfDeclaration> for TokenRange {
    fn from(value: &AlwaysFfDeclaration) -> Self {
        let beg = value.always_ff.always_ff_token.token;
        let end = value.r_brace.r_brace_token.token;
        TokenRange { beg, end }
    }
}

#[derive(Debug, Clone)]
pub struct VerylToken {
    pub token: Token,
    pub comments: Vec<Token>,
}

impl VerylToken {
    pub fn new(token: Token) -> Self {
        Self {
            token,
            comments: vec![],
        }
    }

    pub fn replace(&self, text: &str) -> Self {
        let length = text.len();
        let text = resource_table::insert_str(text);
        let mut ret = self.clone();
        ret.token.text = text;
        ret.token.length = length as u32;
        ret
    }

    pub fn append(&self, prefix: &str, postfix: &str) -> Self {
        let text = format!("{}{}{}", prefix, self.token.text, postfix);
        let length = text.len();
        let text = resource_table::insert_str(&text);
        let mut ret = self.clone();
        ret.token.text = text;
        ret.token.length = length as u32;
        ret
    }
}

impl fmt::Display for VerylToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{}", self.token);
        text.fmt(f)
    }
}

static COMMENT_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"((?://.*(?:\r\n|\r|\n|$))|(?:(?ms)/\u{2a}.*?\u{2a}/))").unwrap());

fn split_comment_token(token: Token) -> Vec<Token> {
    let mut line = token.line;
    let text = resource_table::get_str_value(token.text).unwrap();

    let mut prev_pos = 0;
    let mut ret = Vec::new();
    for cap in COMMENT_REGEX.captures_iter(&text) {
        let cap = cap.get(0).unwrap();
        let pos = cap.start();
        let length = (cap.end() - pos) as u32;

        line += text[prev_pos..(pos)].matches('\n').count() as u32;
        prev_pos = pos;

        let id = resource_table::new_token_id();
        let text = &text[pos..pos + length as usize];
        let is_doc_comment = text.starts_with("///");
        let text = resource_table::insert_str(text);

        if is_doc_comment {
            if let TokenSource::File(file) = token.source {
                doc_comment_table::insert(file, line, text);
            }
        }

        let token = Token {
            id,
            text,
            line,
            column: 0,
            length,
            pos: pos as u32 + length,
            source: token.source,
        };
        ret.push(token);
    }
    ret
}

impl TryFrom<&StartToken> for VerylToken {
    type Error = anyhow::Error;

    fn try_from(x: &StartToken) -> Result<Self, anyhow::Error> {
        let mut comments = Vec::new();
        if let Some(ref x) = x.comments.comments_opt {
            let mut tokens = split_comment_token(x.comments_term.comments_term);
            comments.append(&mut tokens)
        }
        let id = resource_table::new_token_id();
        let text = resource_table::insert_str("");
        let source = TokenSource::Builtin;
        let token = Token {
            id,
            text,
            line: 1,
            column: 1,
            length: 0,
            pos: 0,
            source,
        };
        Ok(VerylToken { token, comments })
    }
}

macro_rules! token_with_comments {
    ($x:ident) => {
        paste! {
            impl TryFrom<&[<$x Token>]> for VerylToken {
                type Error = anyhow::Error;

                fn try_from(x: &[<$x Token>]) -> Result<Self, anyhow::Error> {
                    let mut comments = Vec::new();
                    if let Some(ref x) = x.comments.comments_opt {
                        let mut tokens = split_comment_token(x.comments_term.comments_term);
                        comments.append(&mut tokens)
                    }
                    Ok(VerylToken {
                        token: x.[<$x:snake _term>].clone(),
                        comments,
                    })
                }
            }
            impl TryFrom<&[<$x Term>]> for Token {
                type Error = anyhow::Error;

                fn try_from(x: &[<$x Term>]) -> Result<Self, anyhow::Error> {
                    Ok(Token {
                        id: x.[<$x:snake _term>].id,
                        text: x.[<$x:snake _term>].text,
                        line: x.[<$x:snake _term>].line,
                        column: x.[<$x:snake _term>].column,
                        length: x.[<$x:snake _term>].length,
                        pos: x.[<$x:snake _term>].pos,
                        source: x.[<$x:snake _term>].source,
                    })
                }
            }
        }
    };
}

token_with_comments!(StringLiteral);

token_with_comments!(FixedPoint);
token_with_comments!(Exponent);
token_with_comments!(Based);
token_with_comments!(BaseLess);
token_with_comments!(AllBit);

token_with_comments!(Colon);
token_with_comments!(ColonColon);
token_with_comments!(Comma);
token_with_comments!(Dollar);
token_with_comments!(DotDot);
token_with_comments!(DotDotEqu);
token_with_comments!(Dot);
token_with_comments!(Equ);
token_with_comments!(Hash);
token_with_comments!(LAngle);
token_with_comments!(LBrace);
token_with_comments!(LBracket);
token_with_comments!(LParen);
token_with_comments!(MinusColon);
token_with_comments!(MinusGT);
token_with_comments!(PlusColon);
token_with_comments!(RAngle);
token_with_comments!(RBrace);
token_with_comments!(RBracket);
token_with_comments!(RParen);
token_with_comments!(Semicolon);
token_with_comments!(Star);

token_with_comments!(AssignmentOperator);
token_with_comments!(Operator01);
token_with_comments!(Operator02);
token_with_comments!(Operator03);
token_with_comments!(Operator04);
token_with_comments!(Operator05);
token_with_comments!(Operator06);
token_with_comments!(Operator07);
token_with_comments!(Operator08);
token_with_comments!(Operator09);
token_with_comments!(Operator10);
token_with_comments!(Operator11);
token_with_comments!(UnaryOperator);

token_with_comments!(AlwaysComb);
token_with_comments!(AlwaysFf);
token_with_comments!(As);
token_with_comments!(Assign);
token_with_comments!(AsyncHigh);
token_with_comments!(AsyncLow);
token_with_comments!(Bit);
token_with_comments!(Break);
token_with_comments!(Case);
token_with_comments!(Default);
token_with_comments!(Else);
token_with_comments!(Embed);
token_with_comments!(Enum);
token_with_comments!(Export);
token_with_comments!(F32);
token_with_comments!(F64);
token_with_comments!(Final);
token_with_comments!(For);
token_with_comments!(Function);
token_with_comments!(I32);
token_with_comments!(I64);
token_with_comments!(If);
token_with_comments!(IfReset);
token_with_comments!(Import);
token_with_comments!(Initial);
token_with_comments!(Inout);
token_with_comments!(Input);
token_with_comments!(Inside);
token_with_comments!(Inst);
token_with_comments!(Interface);
token_with_comments!(In);
token_with_comments!(Let);
token_with_comments!(Local);
token_with_comments!(Logic);
token_with_comments!(Lsb);
token_with_comments!(Modport);
token_with_comments!(Module);
token_with_comments!(Msb);
token_with_comments!(Negedge);
token_with_comments!(Output);
token_with_comments!(Outside);
token_with_comments!(Package);
token_with_comments!(Param);
token_with_comments!(Posedge);
token_with_comments!(Pub);
token_with_comments!(Ref);
token_with_comments!(Repeat);
token_with_comments!(Return);
token_with_comments!(Signed);
token_with_comments!(Step);
token_with_comments!(String);
token_with_comments!(Struct);
token_with_comments!(SyncHigh);
token_with_comments!(SyncLow);
token_with_comments!(Tri);
token_with_comments!(Type);
token_with_comments!(U32);
token_with_comments!(U64);
token_with_comments!(Union);
token_with_comments!(Var);

token_with_comments!(Identifier);

fn embed_item_to_string(x: &EmbedItem) -> String {
    let mut ret = String::new();
    match x {
        EmbedItem::LBraceTermEmbedItemListRBraceTerm(x) => {
            ret.push_str(&x.l_brace_term.l_brace_term.to_string());
            for x in &x.embed_item_list {
                ret.push_str(&embed_item_to_string(&x.embed_item));
            }
            ret.push_str(&x.r_brace_term.r_brace_term.to_string());
        }
        EmbedItem::AnyTerm(x) => {
            ret.push_str(&x.any_term.any_term.to_string());
        }
    }
    ret
}

impl TryFrom<&EmbedContentToken> for VerylToken {
    type Error = anyhow::Error;

    fn try_from(x: &EmbedContentToken) -> Result<Self, anyhow::Error> {
        let head_token = &x.l_brace_term.l_brace_term;
        let line = head_token.line;
        let column = head_token.column;
        let length = head_token.length;
        let pos = head_token.pos;
        let source = head_token.source;

        let mut text = x.l_brace_term.l_brace_term.to_string();
        text.push_str(&x.l_brace_term0.l_brace_term.to_string());
        text.push_str(&x.l_brace_term1.l_brace_term.to_string());
        for x in &x.embed_content_token_list {
            text.push_str(&embed_item_to_string(&x.embed_item));
        }
        text.push_str(&x.r_brace_term.r_brace_term.to_string());
        text.push_str(&x.r_brace_term0.r_brace_term.to_string());
        text.push_str(&x.r_brace_term1.r_brace_term.to_string());

        let token = Token::new(&text, line, column, length, pos, source);
        Ok(VerylToken {
            token,
            comments: Vec::new(),
        })
    }
}
