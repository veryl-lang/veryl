use crate::resource_table::{self, PathId, StrId, TokenId};
use crate::veryl_grammar_trait::*;
use paste::paste;
use regex::Regex;

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub id: TokenId,
    pub text: StrId,
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub pos: usize,
    pub file_path: PathId,
}

impl<'t> TryFrom<&parol_runtime::lexer::Token<'t>> for Token {
    type Error = anyhow::Error;
    fn try_from(x: &parol_runtime::lexer::Token<'t>) -> Result<Self, anyhow::Error> {
        let id = resource_table::new_token_id();
        let text = resource_table::insert_str(x.text());
        let pos = x.location.scanner_switch_pos + x.location.offset - x.location.length;
        let file_path = resource_table::insert_path(&x.location.file_name);
        Ok(Token {
            id,
            text,
            line: x.location.start_line,
            column: x.location.start_column,
            length: x.location.length,
            pos,
            file_path,
        })
    }
}

impl From<&Token> for miette::SourceSpan {
    fn from(x: &Token) -> Self {
        (x.pos, x.length).into()
    }
}

impl From<Token> for miette::SourceSpan {
    fn from(x: Token) -> Self {
        (x.pos, x.length).into()
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
        ret.token.length = length;
        ret
    }

    pub fn text(&self) -> String {
        resource_table::get_str_value(self.token.text).unwrap()
    }
}

fn split_comment_token(token: Token) -> Vec<Token> {
    let mut line = token.line;
    let text = resource_table::get_str_value(token.text).unwrap();
    let re = Regex::new(r"((?://.*(?:\r\n|\r|\n|$))|(?:(?ms)/\u{2a}.*?\u{2a}/))").unwrap();

    let mut prev_pos = 0;
    let mut ret = Vec::new();
    for cap in re.captures_iter(&text) {
        let cap = cap.get(0).unwrap();
        let pos = cap.start();
        let length = cap.end() - pos;

        line += text[prev_pos..pos].matches('\n').count();
        prev_pos = pos;

        let id = resource_table::new_token_id();
        let text = resource_table::insert_str(&text[pos..pos + length]);
        let token = Token {
            id,
            text,
            line,
            column: 0,
            length,
            pos: pos + length,
            file_path: token.file_path,
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
        let file_path = resource_table::insert_path(std::path::Path::new(""));
        let token = Token {
            id,
            text,
            line: 1,
            column: 1,
            length: 0,
            pos: 0,
            file_path,
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
                        file_path: x.[<$x:snake _term>].file_path,
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
token_with_comments!(Function);
token_with_comments!(For);
token_with_comments!(I32);
token_with_comments!(I64);
token_with_comments!(If);
token_with_comments!(IfReset);
token_with_comments!(Import);
token_with_comments!(Inout);
token_with_comments!(Input);
token_with_comments!(Inst);
token_with_comments!(Interface);
token_with_comments!(In);
token_with_comments!(Localparam);
token_with_comments!(Logic);
token_with_comments!(Modport);
token_with_comments!(Module);
token_with_comments!(Negedge);
token_with_comments!(Output);
token_with_comments!(Package);
token_with_comments!(Parameter);
token_with_comments!(Posedge);
token_with_comments!(Ref);
token_with_comments!(Repeat);
token_with_comments!(Return);
token_with_comments!(Signed);
token_with_comments!(Step);
token_with_comments!(Struct);
token_with_comments!(SyncHigh);
token_with_comments!(SyncLow);
token_with_comments!(Tri);
token_with_comments!(Type);
token_with_comments!(U32);
token_with_comments!(U64);
token_with_comments!(Var);

token_with_comments!(Identifier);
