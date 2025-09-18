use crate::doc_comment_table;
use crate::resource_table::{self, PathId, StrId, TokenId};
use crate::text_table::{self, TextId};
use crate::veryl_grammar_trait::*;
use once_cell::sync::Lazy;
use paste::paste;
use regex::Regex;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenSource {
    File { path: PathId, text: TextId },
    Builtin,
    External,
    Generated(PathId),
}

impl fmt::Display for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            TokenSource::File { path, .. } => path.to_string(),
            TokenSource::Builtin => "builtin".to_string(),
            TokenSource::External => "external".to_string(),
            TokenSource::Generated(_) => "generated".to_string(),
        };
        text.fmt(f)
    }
}

impl PartialEq<PathId> for TokenSource {
    fn eq(&self, other: &PathId) -> bool {
        match self {
            TokenSource::File { path, .. } => path == other,
            TokenSource::Generated(x) => x == other,
            _ => false,
        }
    }
}

impl TokenSource {
    pub fn get_text(&self) -> String {
        if let TokenSource::File { text, .. } = self {
            if let Some(x) = text_table::get(*text) {
                x.text
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    }

    pub fn get_path(&self) -> Option<PathId> {
        match self {
            TokenSource::File { path, .. } => Some(*path),
            TokenSource::Generated(x) => Some(*x),
            _ => None,
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

    pub fn generate(text: StrId, path: PathId) -> Self {
        let id = resource_table::new_token_id();
        Token {
            id,
            text,
            line: 0,
            column: 0,
            length: 0,
            pos: 0,
            source: TokenSource::Generated(path),
        }
    }

    pub fn end_line(&self) -> u32 {
        let text = self.to_string();
        self.line + text.matches('\n').count() as u32
    }

    pub fn end_column(&self) -> u32 {
        let text = self.to_string();
        if text.matches('\n').count() > 0 {
            text.split('\n')
                .next_back()
                .map(|x| x.len() as u32)
                .unwrap()
        } else {
            self.column + self.length - 1
        }
    }
}

pub fn is_anonymous_text(text: StrId) -> bool {
    let anonymous_id = resource_table::insert_str("_");
    text == anonymous_id
}

pub fn is_anonymous_token(token: &Token) -> bool {
    is_anonymous_text(token.text)
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
        let pos = x.location.start;
        let source = TokenSource::File {
            path: resource_table::insert_path(&x.location.file_name),
            text: text_table::get_current_text(),
        };
        Ok(Token {
            id,
            text,
            line: x.location.start_line,
            column: x.location.start_column,
            length: x.location.len() as u32,
            pos,
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

    pub fn append(&self, prefix: &Option<String>, suffix: &Option<String>) -> Self {
        let prefix_str = if let Some(x) = prefix { x.as_str() } else { "" };
        let suffix_str = if let Some(x) = suffix { x.as_str() } else { "" };
        let text = format!("{}{}{}", prefix_str, self.token.text, suffix_str);
        let length = text.len();
        let text = resource_table::insert_str(&text);
        let mut ret = self.clone();
        ret.token.text = text;
        ret.token.length = length as u32;
        ret
    }

    pub fn strip_prefix(&self, prefix: &str) -> Self {
        let text = self.token.text.to_string();
        if let Some(text) = text.strip_prefix(prefix) {
            let length = text.len();
            let text = resource_table::insert_str(text);
            let mut ret = self.clone();
            ret.token.text = text;
            ret.token.length = length as u32;
            ret
        } else {
            self.clone()
        }
    }
}

impl fmt::Display for VerylToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = format!("{}", self.token);
        text.fmt(f)
    }
}

impl ScopedIdentifier {
    pub fn identifier(&self) -> &VerylToken {
        match &*self.scoped_identifier_group {
            ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                &x.identifier.identifier_token
            }
            ScopedIdentifierGroup::DollarIdentifier(x) => {
                &x.dollar_identifier.dollar_identifier_token
            }
        }
    }
}

impl ExpressionIdentifier {
    pub fn identifier(&self) -> &VerylToken {
        self.scoped_identifier.identifier()
    }
}

static COMMENT_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"((?://.*(?:\r\n|\r|\n|$))|(?:(?ms)/\u{2a}.*?\u{2a}/))").unwrap());

fn split_comment_token(token: Token) -> Vec<Token> {
    let mut line = token.line;
    let mut column = token.column;
    let text = resource_table::get_str_value(token.text).unwrap();

    let mut prev_pos = 0;
    let mut ret = Vec::new();
    for cap in COMMENT_REGEX.captures_iter(&text) {
        let cap = cap.get(0).unwrap();
        let pos = cap.start();
        let length = (cap.end() - pos) as u32;

        let prev_text = &text[prev_pos..(pos)];
        let n_lines = prev_text.matches('\n').count() as u32;
        line += n_lines;

        column = if n_lines == 0 {
            column + prev_text.len() as u32
        } else {
            (prev_text.len() - prev_text.rfind('\n').unwrap_or(0)) as u32
        };

        prev_pos = pos;

        let id = resource_table::new_token_id();
        let text = &text[pos..pos + length as usize];
        let is_doc_comment = text.starts_with("///");
        let text = resource_table::insert_str(text);

        if is_doc_comment && let TokenSource::File { path, .. } = token.source {
            doc_comment_table::insert(path, line, text);
        }

        let token = Token {
            id,
            text,
            line,
            column,
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

macro_rules! token_without_comments {
    ($x:ident, $y:ident) => {
        paste! {
            impl TryFrom<&[<$x Token>]> for VerylToken {
                type Error = anyhow::Error;

                fn try_from(x: &[<$x Token>]) -> Result<Self, anyhow::Error> {
                    Ok(VerylToken {
                        token: x.[<$y:snake _term>].clone(),
                        comments: Vec::new(),
                    })
                }
            }
        }
    };
    ($x:ident) => {
        paste! {
            impl TryFrom<&[<$x Token>]> for VerylToken {
                type Error = anyhow::Error;

                fn try_from(x: &[<$x Token>]) -> Result<Self, anyhow::Error> {
                    Ok(VerylToken {
                        token: x.[<$x:snake _term>].clone(),
                        comments: Vec::new(),
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
token_with_comments!(ColonColonLAngle);
token_with_comments!(Comma);
token_with_comments!(DotDot);
token_with_comments!(DotDotEqu);
token_with_comments!(Dot);
token_with_comments!(Equ);
token_with_comments!(HashLBracket);
token_with_comments!(Hash);
token_with_comments!(Question);
token_with_comments!(Quote);
token_with_comments!(QuoteLBrace);
token_with_comments!(LAngle);
token_without_comments!(EmbedLBrace, LBrace);
token_without_comments!(EscapedLBrace);
token_without_comments!(TripleLBrace);
token_with_comments!(LBrace);
token_with_comments!(LBracket);
token_with_comments!(LParen);
token_with_comments!(LTMinus);
token_with_comments!(MinusColon);
token_with_comments!(MinusGT);
token_with_comments!(PlusColon);
token_with_comments!(RAngle);
token_without_comments!(EmbedRBrace, RBrace);
token_without_comments!(EscapedRBrace);
token_with_comments!(TripleRBrace);
token_with_comments!(RBrace);
token_with_comments!(RBracket);
token_with_comments!(RParen);
token_with_comments!(Semicolon);
token_with_comments!(Star);

token_with_comments!(AssignmentOperator);
token_with_comments!(DiamondOperator);
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
token_with_comments!(Operator12);
token_with_comments!(UnaryOperator);

token_with_comments!(Alias);
token_with_comments!(AlwaysComb);
token_with_comments!(AlwaysFf);
token_with_comments!(As);
token_with_comments!(Assign);
token_with_comments!(Bind);
token_with_comments!(Bit);
token_with_comments!(Bool);
token_with_comments!(Break);
token_with_comments!(Case);
token_with_comments!(Clock);
token_with_comments!(ClockPosedge);
token_with_comments!(ClockNegedge);
token_with_comments!(Connect);
token_with_comments!(Const);
token_with_comments!(Converse);
token_with_comments!(Default);
token_with_comments!(Else);
token_with_comments!(Embed);
token_with_comments!(Enum);
token_with_comments!(F32);
token_with_comments!(F64);
token_with_comments!(False);
token_with_comments!(Final);
token_with_comments!(For);
token_with_comments!(Function);
token_with_comments!(I8);
token_with_comments!(I16);
token_with_comments!(I32);
token_with_comments!(I64);
token_with_comments!(If);
token_with_comments!(IfReset);
token_with_comments!(Import);
token_with_comments!(Include);
token_with_comments!(Initial);
token_with_comments!(Inout);
token_with_comments!(Input);
token_with_comments!(Inside);
token_with_comments!(Inst);
token_with_comments!(Interface);
token_with_comments!(In);
token_with_comments!(Let);
token_with_comments!(Logic);
token_with_comments!(Lsb);
token_with_comments!(Modport);
token_with_comments!(Module);
token_with_comments!(Msb);
token_with_comments!(Output);
token_with_comments!(Outside);
token_with_comments!(Package);
token_with_comments!(Param);
token_with_comments!(Proto);
token_with_comments!(Pub);
token_with_comments!(Repeat);
token_with_comments!(Reset);
token_with_comments!(ResetAsyncHigh);
token_with_comments!(ResetAsyncLow);
token_with_comments!(ResetSyncHigh);
token_with_comments!(ResetSyncLow);
token_with_comments!(Return);
token_with_comments!(Rev);
token_with_comments!(Same);
token_with_comments!(Signed);
token_with_comments!(Step);
token_with_comments!(String);
token_with_comments!(Struct);
token_with_comments!(Switch);
token_with_comments!(Tri);
token_with_comments!(True);
token_with_comments!(Type);
token_with_comments!(U8);
token_with_comments!(U16);
token_with_comments!(U32);
token_with_comments!(U64);
token_with_comments!(Union);
token_with_comments!(Unsafe);
token_with_comments!(Var);

token_with_comments!(DollarIdentifier);
token_with_comments!(Identifier);

token_without_comments!(Any);
