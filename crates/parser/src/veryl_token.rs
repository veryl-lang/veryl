use crate::doc_comment_table;
use crate::resource_table::{self, PathId, StrId, TokenId};
use crate::veryl_grammar_trait::*;
use once_cell::sync::Lazy;
use paste::paste;
use regex::Regex;

#[derive(Debug, Clone, Copy)]
pub enum TokenSource {
    File(PathId),
    Builtin,
}

impl ToString for TokenSource {
    fn to_string(&self) -> String {
        if let TokenSource::File(x) = self {
            x.to_string()
        } else {
            String::from("builtin")
        }
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

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug, Clone)]
pub struct VerylToken {
    pub token: Token,
    pub comments: Vec<Token>,
}

impl VerylToken {
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

    pub fn text(&self) -> String {
        resource_table::get_str_value(self.token.text).unwrap()
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
token_with_comments!(Case);
token_with_comments!(Default);
token_with_comments!(Else);
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
token_with_comments!(Localparam);
token_with_comments!(Logic);
token_with_comments!(Lsb);
token_with_comments!(Modport);
token_with_comments!(Module);
token_with_comments!(Msb);
token_with_comments!(Negedge);
token_with_comments!(Output);
token_with_comments!(Outside);
token_with_comments!(Package);
token_with_comments!(Parameter);
token_with_comments!(Posedge);
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
token_with_comments!(Void);

token_with_comments!(Identifier);
