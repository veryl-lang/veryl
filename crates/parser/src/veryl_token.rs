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
    Generated,
}

impl fmt::Display for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            TokenSource::File(x) => x.to_string(),
            TokenSource::Builtin => "builtin".to_string(),
            TokenSource::External => "external".to_string(),
            TokenSource::Generated => "generated".to_string(),
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

    pub fn generate(text: StrId) -> Self {
        let id = resource_table::new_token_id();
        Token {
            id,
            text,
            line: 0,
            column: 0,
            length: 0,
            pos: 0,
            source: TokenSource::Generated,
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
        let source = TokenSource::File(resource_table::insert_path(&x.location.file_name));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        let beg = value.identifier().token;
        let mut end = beg;
        if let Some(x) = value.scoped_identifier_list.last() {
            end = x.identifier.identifier_token.token;
        }
        TokenRange { beg, end }
    }
}

impl From<&ExpressionIdentifier> for TokenRange {
    fn from(value: &ExpressionIdentifier) -> Self {
        let mut range: TokenRange = value.scoped_identifier.as_ref().into();
        if let Some(ref x) = value.expression_identifier_opt {
            range.end = x.width.r_angle.r_angle_token.token;
        }
        for x in &value.expression_identifier_list {
            range.end = x.select.r_bracket.r_bracket_token.token;
        }
        for x in &value.expression_identifier_list0 {
            range.end = x.identifier.identifier_token.token;
            for x in &x.expression_identifier_list0_list {
                range.end = x.select.r_bracket.r_bracket_token.token;
            }
        }
        range
    }
}

impl From<&AlwaysFfDeclaration> for TokenRange {
    fn from(value: &AlwaysFfDeclaration) -> Self {
        let beg = value.always_ff.always_ff_token.token;
        let end = value.statement_block.r_brace.r_brace_token.token;
        TokenRange { beg, end }
    }
}

impl From<&Expression12ListGroup> for TokenRange {
    fn from(value: &Expression12ListGroup) -> Self {
        let beg = match value {
            Expression12ListGroup::UnaryOperator(x) => x.unary_operator.unary_operator_token.token,
            Expression12ListGroup::Operator09(x) => x.operator09.operator09_token.token,
            Expression12ListGroup::Operator05(x) => x.operator05.operator05_token.token,
            Expression12ListGroup::Operator04(x) => x.operator04.operator04_token.token,
            Expression12ListGroup::Operator03(x) => x.operator03.operator03_token.token,
        };
        let end = beg;
        TokenRange { beg, end }
    }
}

impl From<&IntegralNumber> for TokenRange {
    fn from(value: &IntegralNumber) -> Self {
        let beg = match value {
            IntegralNumber::Based(x) => x.based.based_token.token,
            IntegralNumber::BaseLess(x) => x.base_less.base_less_token.token,
            IntegralNumber::AllBit(x) => x.all_bit.all_bit_token.token,
        };
        let end = beg;
        TokenRange { beg, end }
    }
}

impl From<&RealNumber> for TokenRange {
    fn from(value: &RealNumber) -> Self {
        let beg = match value {
            RealNumber::FixedPoint(x) => x.fixed_point.fixed_point_token.token,
            RealNumber::Exponent(x) => x.exponent.exponent_token.token,
        };
        let end = beg;
        TokenRange { beg, end }
    }
}

impl From<&Number> for TokenRange {
    fn from(value: &Number) -> Self {
        match value {
            Number::IntegralNumber(x) => x.integral_number.as_ref().into(),
            Number::RealNumber(x) => x.real_number.as_ref().into(),
        }
    }
}

impl From<&TypeModifier> for TokenRange {
    fn from(value: &TypeModifier) -> Self {
        let beg = match value {
            TypeModifier::Tri(x) => x.tri.tri_token.token,
            TypeModifier::Signed(x) => x.signed.signed_token.token,
        };
        let end = beg;
        TokenRange { beg, end }
    }
}

macro_rules! impl_token_range {
    ($typename:ty, $first:ident, $firsttok:ident, $last:ident, $lasttok:ident) => {
        impl From<&$typename> for TokenRange {
            fn from(value: &$typename) -> Self {
                let beg = value.$first.$firsttok.token;
                let end = value.$last.$lasttok.token;
                TokenRange { beg, end }
            }
        }
    };
}

macro_rules! impl_token_range_singular {
    ($typename:ty, $first:ident) => {
        impl From<&$typename> for TokenRange {
            fn from(value: &$typename) -> Self {
                let beg = value.$first.token;
                let end = beg;
                TokenRange { beg, end }
            }
        }
    };
}

macro_rules! impl_token_range_dual {
    ($typename:ty, $first:ident, $second:ident) => {
        impl From<&$typename> for TokenRange {
            fn from(value: &$typename) -> Self {
                let beg = value.$first.$second.token;
                let end = beg;
                TokenRange { beg, end }
            }
        }
    };
}

impl_token_range!(IfExpression, r#if, if_token, r_brace0, r_brace_token);
impl_token_range!(CaseExpression, case, case_token, r_brace, r_brace_token);
impl_token_range!(
    FactorLParenExpressionRParen,
    l_paren,
    l_paren_token,
    r_paren,
    r_paren_token
);
impl_token_range!(
    FactorLBraceConcatenationListRBrace,
    l_brace,
    l_brace_token,
    r_brace,
    r_brace_token
);
impl_token_range!(
    FactorQuoteLBraceArrayLiteralListRBrace,
    quote_l_brace,
    quote_l_brace_token,
    r_brace,
    r_brace_token
);
impl_token_range_singular!(StringLiteral, string_literal_token);
impl_token_range_dual!(FactorGroupMsb, msb, msb_token);
impl_token_range_dual!(FactorGroupLsb, lsb, lsb_token);
impl_token_range_singular!(Inside, inside_token);
impl_token_range!(
    InsideExpression,
    inside,
    inside_token,
    r_brace,
    r_brace_token
);
impl_token_range!(
    OutsideExpression,
    outside,
    outside_token,
    r_brace,
    r_brace_token
);
impl_token_range!(
    SwitchExpression,
    switch,
    switch_token,
    r_brace,
    r_brace_token
);
impl_token_range!(TypeExpression, r#type, type_token, r_paren, r_paren_token);

impl From<&FactorGroup> for TokenRange {
    fn from(value: &FactorGroup) -> Self {
        match value {
            FactorGroup::Msb(x) => x.into(),
            FactorGroup::Lsb(x) => x.into(),
        }
    }
}

impl From<&FactorTypeFactor> for TokenRange {
    fn from(value: &FactorTypeFactor) -> Self {
        let beg: TokenRange = if let Some(x) = value.factor_type_factor_list.first() {
            x.type_modifier.as_ref().into()
        } else {
            value.factor_type.as_ref().into()
        };
        let end: TokenRange = value.factor_type.as_ref().into();
        TokenRange {
            beg: beg.beg,
            end: end.end,
        }
    }
}

impl From<&Factor> for TokenRange {
    fn from(value: &Factor) -> Self {
        match value {
            Factor::Number(x) => x.number.as_ref().into(),
            Factor::IdentifierFactor(x) => {
                x.identifier_factor.expression_identifier.as_ref().into()
            }
            Factor::LParenExpressionRParen(x) => x.into(),
            Factor::LBraceConcatenationListRBrace(x) => x.into(),
            Factor::QuoteLBraceArrayLiteralListRBrace(x) => x.into(),
            Factor::IfExpression(x) => x.if_expression.as_ref().into(),
            Factor::CaseExpression(x) => x.case_expression.as_ref().into(),
            Factor::SwitchExpression(x) => x.switch_expression.as_ref().into(),
            Factor::StringLiteral(x) => x.string_literal.as_ref().into(),
            Factor::FactorGroup(x) => x.factor_group.as_ref().into(),
            Factor::InsideExpression(x) => x.inside_expression.as_ref().into(),
            Factor::OutsideExpression(x) => x.outside_expression.as_ref().into(),
            Factor::TypeExpression(x) => x.type_expression.as_ref().into(),
            Factor::FactorTypeFactor(x) => x.factor_type_factor.as_ref().into(),
        }
    }
}

impl From<&Expression11> for TokenRange {
    fn from(value: &Expression11) -> Self {
        let beg: TokenRange = value.expression12.as_ref().into();
        let end = if let Some(ref x) = value.expression11_opt {
            let end: TokenRange = x.casting_type.as_ref().into();
            end.end
        } else {
            beg.end
        };
        let beg = beg.beg;
        TokenRange { beg, end }
    }
}

impl From<&Expression12> for TokenRange {
    fn from(value: &Expression12) -> Self {
        let end: TokenRange = value.factor.as_ref().into();
        let beg = if value.expression12_list.is_empty() {
            end.beg
        } else {
            let first = value.expression12_list.first().unwrap();
            let t: TokenRange = first.expression12_list_group.as_ref().into();
            t.beg
        };
        let end = end.end;
        TokenRange { beg, end }
    }
}

macro_rules! expression_token_range {
    ($typename:ty, $beg:ident, $list:ident, $prev:ident) => {
        impl From<&$typename> for TokenRange {
            fn from(value: &$typename) -> Self {
                let beg: TokenRange = value.$beg.as_ref().into();
                let end = if value.$list.is_empty() {
                    beg.end
                } else {
                    let last = value.$list.last().unwrap();
                    let end: TokenRange = last.$prev.as_ref().into();
                    end.end
                };
                let beg = beg.beg;
                TokenRange { beg, end }
            }
        }
    };
}

expression_token_range!(Expression10, expression11, expression10_list, expression11);
expression_token_range!(Expression09, expression10, expression09_list, expression10);
expression_token_range!(Expression08, expression09, expression08_list, expression09);
expression_token_range!(Expression07, expression08, expression07_list, expression08);
expression_token_range!(Expression06, expression07, expression06_list, expression07);
expression_token_range!(Expression05, expression06, expression05_list, expression06);
expression_token_range!(Expression04, expression05, expression04_list, expression05);
expression_token_range!(Expression03, expression04, expression03_list, expression04);
expression_token_range!(Expression02, expression03, expression02_list, expression03);
expression_token_range!(Expression01, expression02, expression01_list, expression02);
expression_token_range!(Expression, expression01, expression_list, expression01);

impl From<&FixedType> for TokenRange {
    fn from(value: &FixedType) -> Self {
        let beg = match value {
            FixedType::U32(x) => x.u32.u32_token.token,
            FixedType::U64(x) => x.u64.u64_token.token,
            FixedType::I32(x) => x.i32.i32_token.token,
            FixedType::I64(x) => x.i64.i64_token.token,
            FixedType::F32(x) => x.f32.f32_token.token,
            FixedType::F64(x) => x.f64.f64_token.token,
            FixedType::Strin(x) => x.strin.string_token.token,
        };
        let end = beg;
        TokenRange { beg, end }
    }
}

impl From<&VariableType> for TokenRange {
    fn from(value: &VariableType) -> Self {
        match value {
            VariableType::Clock(x) => {
                let beg = x.clock.clock_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::ClockPosedge(x) => {
                let beg = x.clock_posedge.clock_posedge_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::ClockNegedge(x) => {
                let beg = x.clock_negedge.clock_negedge_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::Reset(x) => {
                let beg = x.reset.reset_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::ResetAsyncHigh(x) => {
                let beg = x.reset_async_high.reset_async_high_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::ResetAsyncLow(x) => {
                let beg = x.reset_async_low.reset_async_low_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::ResetSyncHigh(x) => {
                let beg = x.reset_sync_high.reset_sync_high_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::ResetSyncLow(x) => {
                let beg = x.reset_sync_low.reset_sync_low_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::Logic(x) => {
                let beg = x.logic.logic_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            VariableType::Bit(x) => {
                let beg = x.bit.bit_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
        }
    }
}

impl From<&FactorType> for TokenRange {
    fn from(value: &FactorType) -> Self {
        match value.factor_type_group.as_ref() {
            FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                let mut range: TokenRange = x.variable_type.as_ref().into();
                if let Some(ref x) = x.factor_type_opt {
                    range.end = x.width.r_angle.r_angle_token.token;
                }
                range
            }
            FactorTypeGroup::FixedType(x) => x.fixed_type.as_ref().into(),
        }
    }
}

impl From<&ScalarType> for TokenRange {
    fn from(value: &ScalarType) -> Self {
        let mut range: TokenRange = match &*value.scalar_type_group {
            ScalarTypeGroup::UserDefinedTypeScalarTypeOpt(x) => {
                let mut range: TokenRange = x.user_defined_type.scoped_identifier.as_ref().into();
                if let Some(ref x) = x.scalar_type_opt {
                    range.end = x.width.r_angle.r_angle_token.token;
                }
                range
            }
            ScalarTypeGroup::FactorType(x) => x.factor_type.as_ref().into(),
        };

        if let Some(x) = value.scalar_type_list.first() {
            range.beg = match &*x.type_modifier {
                TypeModifier::Tri(x) => x.tri.tri_token.token,
                TypeModifier::Signed(x) => x.r#signed.signed_token.token,
            };
        }

        range
    }
}

impl From<&ArrayType> for TokenRange {
    fn from(value: &ArrayType) -> Self {
        let mut range: TokenRange = value.scalar_type.as_ref().into();

        if let Some(ref x) = value.array_type_opt {
            range.end = x.array.r_bracket.r_bracket_token.token;
        }

        range
    }
}

impl From<&CastingType> for TokenRange {
    fn from(value: &CastingType) -> Self {
        match value {
            CastingType::U32(x) => {
                let beg = x.u32.u32_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::U64(x) => {
                let beg = x.u64.u64_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::I32(x) => {
                let beg = x.i32.i32_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::I64(x) => {
                let beg = x.i64.i64_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::F32(x) => {
                let beg = x.f32.f32_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::F64(x) => {
                let beg = x.f64.f64_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::Clock(x) => {
                let beg = x.clock.clock_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::ClockPosedge(x) => {
                let beg = x.clock_posedge.clock_posedge_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::ClockNegedge(x) => {
                let beg = x.clock_negedge.clock_negedge_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::Reset(x) => {
                let beg = x.reset.reset_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::ResetAsyncHigh(x) => {
                let beg = x.reset_async_high.reset_async_high_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::ResetAsyncLow(x) => {
                let beg = x.reset_async_low.reset_async_low_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::ResetSyncHigh(x) => {
                let beg = x.reset_sync_high.reset_sync_high_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::ResetSyncLow(x) => {
                let beg = x.reset_sync_low.reset_sync_low_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::UserDefinedType(x) => {
                x.user_defined_type.scoped_identifier.as_ref().into()
            }
            CastingType::Based(x) => {
                let beg = x.based.based_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
            CastingType::BaseLess(x) => {
                let beg = x.base_less.base_less_token.token;
                let end = beg;
                TokenRange { beg, end }
            }
        }
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
        let prefix_str = if let Some(ref x) = prefix {
            x.as_str()
        } else {
            ""
        };
        let suffix_str = if let Some(ref x) = suffix {
            x.as_str()
        } else {
            ""
        };
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

token_with_comments!(BackQuote);
token_with_comments!(Colon);
token_with_comments!(ColonColon);
token_with_comments!(ColonColonLAngle);
token_with_comments!(Comma);
token_with_comments!(DotDot);
token_with_comments!(DotDotEqu);
token_with_comments!(Dot);
token_with_comments!(Equ);
token_with_comments!(Hash);
token_with_comments!(QuoteLBrace);
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
token_with_comments!(Bit);
token_with_comments!(Break);
token_with_comments!(Case);
token_with_comments!(Clock);
token_with_comments!(ClockPosedge);
token_with_comments!(ClockNegedge);
token_with_comments!(Const);
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
token_with_comments!(Ref);
token_with_comments!(Repeat);
token_with_comments!(Reset);
token_with_comments!(ResetAsyncHigh);
token_with_comments!(ResetAsyncLow);
token_with_comments!(ResetSyncHigh);
token_with_comments!(ResetSyncLow);
token_with_comments!(Return);
token_with_comments!(Signed);
token_with_comments!(Step);
token_with_comments!(String);
token_with_comments!(Struct);
token_with_comments!(Switch);
token_with_comments!(Tri);
token_with_comments!(Type);
token_with_comments!(U32);
token_with_comments!(U64);
token_with_comments!(Union);
token_with_comments!(Unsafe);
token_with_comments!(Var);

token_with_comments!(DollarIdentifier);
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

        let mut comments = Vec::new();
        if let Some(ref x) = x.comments.comments_opt {
            let mut tokens = split_comment_token(x.comments_term.comments_term);
            comments.append(&mut tokens)
        }

        let token = Token::new(&text, line, column, length, pos, source);
        Ok(VerylToken { token, comments })
    }
}
