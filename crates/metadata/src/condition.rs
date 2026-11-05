use crate::{MetadataError, ProjectProperty};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// Condition expression on project properties (e.g. `!for_asic && width == 32`).
///
/// Operators (from the highest precedence):
/// * `!`
/// * `<:` `>:` `<=` `>=`
/// * `==` `!=`
/// * `&&`
/// * `||`
///
/// Operands are property names, unsized numbers in Veryl syntax
/// (`32`, `'b1010`, `'hff`) and parenthesized expressions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Condition {
    text: String,
    expr: Expr,
}

impl Condition {
    pub fn eval(
        &self,
        properties: &BTreeMap<String, ProjectProperty>,
        project: &str,
    ) -> Result<bool, MetadataError> {
        let evaluator = Evaluator {
            text: &self.text,
            properties,
            project,
        };
        evaluator.eval_bool(&self.expr)
    }
}

impl FromStr for Condition {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let tokens = tokenize(s)?;
        let mut parser = Parser {
            text: s,
            tokens: &tokens,
            pos: 0,
        };
        let expr = parser.parse()?;
        Ok(Condition {
            text: s.to_string(),
            expr,
        })
    }
}

impl TryFrom<String> for Condition {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Condition> for String {
    fn from(value: Condition) -> Self {
        value.text
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.text.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinaryOp {
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExprKind {
    Property(String),
    Number(i64),
    Not(Box<Expr>),
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
}

/// `start` and `end` are byte offsets of the expression in the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Expr {
    kind: ExprKind,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    Identifier(String),
    Number(i64),
    Not,
    Binary(BinaryOp),
    LParen,
    RParen,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
}

fn error_at(text: &str, pos: usize, message: &str) -> String {
    let column = text[..pos].chars().count() + 1;
    format!("{message} at column {column} of \"{text}\"")
}

fn is_identifier_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Scans digits of `radix` separated by single `_` (e.g. `1_000`) from `pos`,
/// and returns the end position.
fn scan_digits(bytes: &[u8], pos: usize, radix: u32) -> usize {
    let is_digit = |c: u8| (c as char).is_digit(radix);
    let mut end = pos;
    while end < bytes.len() && is_digit(bytes[end]) {
        end += 1;
        if end + 1 < bytes.len() && bytes[end] == b'_' && is_digit(bytes[end + 1]) {
            end += 1;
        }
    }
    end
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let bytes = text.as_bytes();
    let mut ret = Vec::new();
    let mut pos = 0;

    while pos < bytes.len() {
        let c = bytes[pos];
        if c.is_ascii_whitespace() {
            pos += 1;
            continue;
        }

        let start = pos;
        let next = bytes.get(pos + 1).copied();
        let (kind, len) = match (c, next) {
            (b'!', Some(b'=')) => (TokenKind::Binary(BinaryOp::Ne), 2),
            (b'!', _) => (TokenKind::Not, 1),
            (b'=', Some(b'=')) => (TokenKind::Binary(BinaryOp::Eq), 2),
            (b'<', Some(b':')) => (TokenKind::Binary(BinaryOp::Lt), 2),
            (b'<', Some(b'=')) => (TokenKind::Binary(BinaryOp::Le), 2),
            (b'>', Some(b':')) => (TokenKind::Binary(BinaryOp::Gt), 2),
            (b'>', Some(b'=')) => (TokenKind::Binary(BinaryOp::Ge), 2),
            (b'&', Some(b'&')) => (TokenKind::Binary(BinaryOp::And), 2),
            (b'|', Some(b'|')) => (TokenKind::Binary(BinaryOp::Or), 2),
            (b'(', _) => (TokenKind::LParen, 1),
            (b')', _) => (TokenKind::RParen, 1),
            (b'a'..=b'z' | b'A'..=b'Z' | b'_', _) => {
                let mut end = pos + 1;
                while end < bytes.len() && is_identifier_char(bytes[end]) {
                    end += 1;
                }
                (TokenKind::Identifier(text[pos..end].to_string()), end - pos)
            }
            (b'0'..=b'9' | b'\'', _) => {
                let (digits_start, radix) = if c == b'\'' {
                    match next {
                        Some(b'b') => (pos + 2, 2),
                        Some(b'h') => (pos + 2, 16),
                        _ => return Err(error_at(text, pos, "invalid number")),
                    }
                } else {
                    (pos, 10)
                };

                let end = scan_digits(bytes, digits_start, radix);
                if end < bytes.len() && bytes[end] == b'\'' {
                    return Err(error_at(text, pos, "sized number is not supported"));
                }
                if end == digits_start || (end < bytes.len() && is_identifier_char(bytes[end])) {
                    return Err(error_at(text, pos, "invalid number"));
                }

                let digits = text[digits_start..end].replace('_', "");
                let Ok(value) = i64::from_str_radix(&digits, radix) else {
                    return Err(error_at(text, pos, "number is too large"));
                };
                (TokenKind::Number(value), end - pos)
            }
            _ => {
                let c = text[pos..].chars().next().unwrap();
                return Err(error_at(
                    text,
                    pos,
                    &format!("unexpected character \"{c}\""),
                ));
            }
        };

        pos += len;
        ret.push(Token {
            kind,
            start,
            end: pos,
        });
    }

    Ok(ret)
}

struct Parser<'a> {
    text: &'a str,
    tokens: &'a [Token],
    pos: usize,
}

impl Parser<'_> {
    fn parse(&mut self) -> Result<Expr, String> {
        let expr = self.parse_or()?;
        if let Some(token) = self.tokens.get(self.pos) {
            return Err(error_at(self.text, token.start, "unexpected token"));
        }
        Ok(expr)
    }

    fn peek_binary(&self, ops: &[BinaryOp]) -> Option<BinaryOp> {
        match self.tokens.get(self.pos) {
            Some(Token {
                kind: TokenKind::Binary(op),
                ..
            }) if ops.contains(op) => Some(*op),
            _ => None,
        }
    }

    /// Parses left-associative binary operators in `ops` whose operands are parsed by `operand`.
    fn parse_binary(
        &mut self,
        ops: &[BinaryOp],
        operand: fn(&mut Self) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let mut lhs = operand(self)?;
        while let Some(op) = self.peek_binary(ops) {
            self.pos += 1;
            let rhs = operand(self)?;
            lhs = Expr {
                start: lhs.start,
                end: rhs.end,
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            };
        }
        Ok(lhs)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        self.parse_binary(&[BinaryOp::Or], Self::parse_and)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        self.parse_binary(&[BinaryOp::And], Self::parse_equality)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        self.parse_binary(&[BinaryOp::Eq, BinaryOp::Ne], Self::parse_relational)
    }

    fn parse_relational(&mut self) -> Result<Expr, String> {
        self.parse_binary(
            &[BinaryOp::Lt, BinaryOp::Gt, BinaryOp::Le, BinaryOp::Ge],
            Self::parse_unary,
        )
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if let Some(Token {
            kind: TokenKind::Not,
            start,
            ..
        }) = self.tokens.get(self.pos)
        {
            let start = *start;
            self.pos += 1;
            let operand = self.parse_unary()?;
            return Ok(Expr {
                start,
                end: operand.end,
                kind: ExprKind::Not(Box::new(operand)),
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let Some(token) = self.tokens.get(self.pos) else {
            return Err(error_at(self.text, self.text.len(), "expected expression"));
        };
        self.pos += 1;

        let kind = match &token.kind {
            TokenKind::Identifier(x) => ExprKind::Property(x.clone()),
            TokenKind::Number(x) => ExprKind::Number(*x),
            TokenKind::LParen => {
                let expr = self.parse_or()?;
                match self.tokens.get(self.pos) {
                    Some(Token {
                        kind: TokenKind::RParen,
                        end,
                        ..
                    }) => {
                        self.pos += 1;
                        return Ok(Expr {
                            start: token.start,
                            end: *end,
                            kind: expr.kind,
                        });
                    }
                    Some(x) => return Err(error_at(self.text, x.start, "expected \")\"")),
                    None => return Err(error_at(self.text, self.text.len(), "expected \")\"")),
                }
            }
            _ => return Err(error_at(self.text, token.start, "expected expression")),
        };

        Ok(Expr {
            kind,
            start: token.start,
            end: token.end,
        })
    }
}

struct Evaluator<'a> {
    text: &'a str,
    properties: &'a BTreeMap<String, ProjectProperty>,
    project: &'a str,
}

impl Evaluator<'_> {
    fn mismatch(&self, expr: &Expr, expected: &str, actual: &ProjectProperty) -> MetadataError {
        MetadataError::MismatchType {
            name: self.text[expr.start..expr.end].to_string(),
            expected: expected.to_string(),
            actual: actual.type_name(),
        }
    }

    fn eval_bool(&self, expr: &Expr) -> Result<bool, MetadataError> {
        match self.eval(expr)? {
            ProjectProperty::Bool(x) => Ok(x),
            x => Err(self.mismatch(expr, "bool", &x)),
        }
    }

    fn eval_int(&self, expr: &Expr) -> Result<i64, MetadataError> {
        match self.eval(expr)? {
            ProjectProperty::Int(x) => Ok(x),
            x => Err(self.mismatch(expr, "int", &x)),
        }
    }

    // `&&` and `||` evaluate both operands to report errors regardless of the value of `lhs`.
    fn eval(&self, expr: &Expr) -> Result<ProjectProperty, MetadataError> {
        let ret = match &expr.kind {
            ExprKind::Property(name) => {
                let Some(value) = self.properties.get(name) else {
                    return Err(MetadataError::UnknownProperty {
                        property: name.clone(),
                        project: self.project.to_string(),
                    });
                };
                value.clone()
            }
            ExprKind::Number(x) => ProjectProperty::Int(*x),
            ExprKind::Not(x) => ProjectProperty::Bool(!self.eval_bool(x)?),
            ExprKind::Binary { op, lhs, rhs } => match op {
                BinaryOp::And => {
                    let lhs = self.eval_bool(lhs)?;
                    let rhs = self.eval_bool(rhs)?;
                    ProjectProperty::Bool(lhs && rhs)
                }
                BinaryOp::Or => {
                    let lhs = self.eval_bool(lhs)?;
                    let rhs = self.eval_bool(rhs)?;
                    ProjectProperty::Bool(lhs || rhs)
                }
                BinaryOp::Eq | BinaryOp::Ne => {
                    let lhs_value = self.eval(lhs)?;
                    let rhs_value = self.eval(rhs)?;
                    if !lhs_value.is_compatible(&rhs_value) {
                        return Err(self.mismatch(rhs, &lhs_value.type_name(), &rhs_value));
                    }
                    ProjectProperty::Bool((lhs_value == rhs_value) == (*op == BinaryOp::Eq))
                }
                BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => {
                    let lhs = self.eval_int(lhs)?;
                    let rhs = self.eval_int(rhs)?;
                    let ret = match op {
                        BinaryOp::Lt => lhs < rhs,
                        BinaryOp::Gt => lhs > rhs,
                        BinaryOp::Le => lhs <= rhs,
                        _ => lhs >= rhs,
                    };
                    ProjectProperty::Bool(ret)
                }
            },
        };
        Ok(ret)
    }
}
