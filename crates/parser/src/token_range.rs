use crate::resource_table::{PathId, TokenId};
use crate::veryl_grammar_trait::*;
use crate::veryl_token::{Token, VerylToken};
use paste::paste;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    pub fn offset(&mut self, value: u32) {
        self.beg.pos += value;
        self.end.pos += value;
    }

    pub fn set_beg(&mut self, value: TokenRange) {
        self.beg = value.beg;
    }

    pub fn set_end(&mut self, value: TokenRange) {
        self.end = value.end;
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

pub trait TokenExt {
    fn range(&self) -> TokenRange;
    fn first(&self) -> Token {
        self.range().beg
    }
    fn last(&self) -> Token {
        self.range().end
    }
    fn id(&self) -> TokenId {
        self.first().id
    }
    fn line(&self) -> u32 {
        self.first().line
    }
}

macro_rules! impl_token_ext {
    ($typename:ty) => {
        impl TokenExt for $typename {
            fn range(&self) -> TokenRange {
                self.into()
            }
        }
    };
}

macro_rules! impl_token_range {
    ($typename:ty, $first:ident, $last:ident) => {
        impl From<&$typename> for TokenRange {
            fn from(value: &$typename) -> Self {
                let beg: TokenRange = value.$first.as_ref().into();
                let end: TokenRange = value.$last.as_ref().into();
                TokenRange {
                    beg: beg.beg,
                    end: end.end,
                }
            }
        }
        impl_token_ext!($typename);
    };
    ($typename:ty, $first:ident) => {
        impl From<&$typename> for TokenRange {
            fn from(value: &$typename) -> Self {
                value.$first.as_ref().into()
            }
        }
        impl_token_ext!($typename);
    };
}

macro_rules! impl_token_range_singular {
    ($typename:ty) => {
        paste! {
            impl From<&$typename> for TokenRange {
                fn from(value: &$typename) -> Self {
                    let beg = value.[<$typename:snake _token>].token;
                    let end = beg;
                    TokenRange { beg, end }
                }
            }
            impl_token_ext!($typename);
        }
    };
}

macro_rules! impl_token_range_enum {
    ($typename:ty, $( $x:ident ),*) => {
        paste! {
            impl From<&$typename> for TokenRange {
                fn from(value: &$typename) -> Self {
                    match value {
                        $(
                            $typename::[<$x:camel>](x) => x.$x.as_ref().into()
                        ),*
                    }
                }
            }
            impl_token_ext!($typename);
        }
    };
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
        impl_token_ext!($typename);
    };
}

macro_rules! impl_token_range_list {
    ($typename:ty, $item:ty) => {
        paste! {
            impl From<&$typename> for TokenRange {
                fn from(value: &$typename) -> Self {
                    let mut ret: TokenRange = value.[<$item:snake>].as_ref().into();
                    if let Some(x) = value.[<$typename:snake _list>].last() {
                        let end: TokenRange = x.[<$item:snake>].as_ref().into();
                        ret.end = end.end;
                    }
                    if let Some(x) = &value.[<$typename:snake _opt>] {
                        let end: TokenRange = x.comma.as_ref().into();
                        ret.end = end.end;
                    }
                    ret
                }
            }
            impl_token_ext!($typename);
        }
    };
}

macro_rules! impl_token_range_group {
    ($typename:ty, $item:ty) => {
        paste! {
            impl From<&$typename> for TokenRange {
                fn from(value: &$typename) -> Self {
                    let mut ret: TokenRange = match value.[<$typename:snake _group>].as_ref() {
                        [<$typename Group>]::[<LBrace $typename GroupListRBrace>](x) => {
                            let beg = x.l_brace.l_brace_token.token;
                            let end = x.r_brace.r_brace_token.token;
                            TokenRange { beg, end }
                        }
                        [<$typename Group>]::$item(x) => x.[<$item:snake>].as_ref().into(),
                    };
                    if let Some(x) = value.[<$typename:snake _list>].first() {
                        let beg: TokenRange = x.attribute.as_ref().into();
                        ret.beg = beg.beg;
                    }
                    ret
                }
            }
            impl_token_ext!($typename);
        }
    };
    ($typename:ty, $list:ty, $item:ty) => {
        paste! {
            impl From<&$typename> for TokenRange {
                fn from(value: &$typename) -> Self {
                    let mut ret: TokenRange = match value.[<$typename:snake _group>].as_ref() {
                        [<$typename Group>]::[<LBrace $list RBrace>](x) => {
                            let beg = x.l_brace.l_brace_token.token;
                            let end = x.r_brace.r_brace_token.token;
                            TokenRange { beg, end }
                        }
                        [<$typename Group>]::$item(x) => x.[<$item:snake>].as_ref().into(),
                    };
                    if let Some(x) = value.[<$typename:snake _list>].first() {
                        let beg: TokenRange = x.attribute.as_ref().into();
                        ret.beg = beg.beg;
                    }
                    ret
                }
            }
            impl_token_ext!($typename);
        }
    };
}

// ----------------------------------------------------------------------------
// VerylToken
// ----------------------------------------------------------------------------

// Start
impl_token_range_singular!(Start);

// StringLiteral
impl_token_range_singular!(StringLiteral);

// Number
impl_token_range_singular!(Exponent);
impl_token_range_singular!(FixedPoint);
impl_token_range_singular!(Based);
impl_token_range_singular!(BaseLess);
impl_token_range_singular!(AllBit);

// Operator
impl_token_range_singular!(AssignmentOperator);
impl_token_range_singular!(DiamondOperator);
impl_token_range_singular!(Operator02);
impl_token_range_singular!(Operator03);
impl_token_range_singular!(Operator04);
impl_token_range_singular!(Operator05);
impl_token_range_singular!(Operator06);
impl_token_range_singular!(Operator07);
impl_token_range_singular!(Operator08);
impl_token_range_singular!(Operator09);
impl_token_range_singular!(Operator10);
impl_token_range_singular!(Operator11);
impl_token_range_singular!(Operator12);
impl_token_range_singular!(UnaryOperator);

// Symbol
impl_token_range_singular!(Colon);
impl_token_range_singular!(ColonColonLAngle);
impl_token_range_singular!(ColonColon);
impl_token_range_singular!(Comma);
impl_token_range_singular!(DotDot);
impl_token_range_singular!(DotDotEqu);
impl_token_range_singular!(Dot);
impl_token_range_singular!(Equ);
impl_token_range_singular!(Hash);
impl_token_range_singular!(Question);
impl_token_range_singular!(Quote);
impl_token_range_singular!(QuoteLBrace);
impl_token_range_singular!(LAngle);
impl_token_range_singular!(LBrace);
impl_token_range_singular!(LBracket);
impl_token_range_singular!(LParen);
impl_token_range_singular!(MinusColon);
impl_token_range_singular!(MinusGT);
impl_token_range_singular!(PlusColon);
impl_token_range_singular!(RAngle);
impl_token_range_singular!(RBrace);
impl_token_range_singular!(RBracket);
impl_token_range_singular!(RParen);
impl_token_range_singular!(Semicolon);
impl_token_range_singular!(Star);

// Keyword
impl_token_range_singular!(Alias);
impl_token_range_singular!(AlwaysComb);
impl_token_range_singular!(AlwaysFf);
impl_token_range_singular!(As);
impl_token_range_singular!(Assign);
impl_token_range_singular!(Bit);
impl_token_range_singular!(Bool);
impl_token_range_singular!(Break);
impl_token_range_singular!(Case);
impl_token_range_singular!(Clock);
impl_token_range_singular!(ClockPosedge);
impl_token_range_singular!(ClockNegedge);
impl_token_range_singular!(Connect);
impl_token_range_singular!(Const);
impl_token_range_singular!(Converse);

impl From<&Defaul> for TokenRange {
    fn from(value: &Defaul) -> Self {
        let beg = value.default_token.token;
        let end = beg;
        TokenRange { beg, end }
    }
}
impl_token_ext!(Defaul);

impl_token_range_singular!(Else);
impl_token_range_singular!(Embed);
impl_token_range_singular!(Enum);
impl_token_range_singular!(F32);
impl_token_range_singular!(F64);
impl_token_range_singular!(False);
impl_token_range_singular!(Final);
impl_token_range_singular!(For);
impl_token_range_singular!(Function);
impl_token_range_singular!(I8);
impl_token_range_singular!(I16);
impl_token_range_singular!(I32);
impl_token_range_singular!(I64);
impl_token_range_singular!(If);
impl_token_range_singular!(IfReset);
impl_token_range_singular!(Import);
impl_token_range_singular!(In);
impl_token_range_singular!(Include);
impl_token_range_singular!(Initial);
impl_token_range_singular!(Inout);
impl_token_range_singular!(Input);
impl_token_range_singular!(Inside);
impl_token_range_singular!(Inst);
impl_token_range_singular!(Interface);
impl_token_range_singular!(Let);
impl_token_range_singular!(Logic);
impl_token_range_singular!(Lsb);
impl_token_range_singular!(Modport);
impl_token_range_singular!(Module);
impl_token_range_singular!(Msb);
impl_token_range_singular!(Output);
impl_token_range_singular!(Outside);
impl_token_range_singular!(Package);
impl_token_range_singular!(Param);
impl_token_range_singular!(Proto);
impl_token_range_singular!(Pub);
impl_token_range_singular!(Repeat);
impl_token_range_singular!(Reset);
impl_token_range_singular!(ResetAsyncHigh);
impl_token_range_singular!(ResetAsyncLow);
impl_token_range_singular!(ResetSyncHigh);
impl_token_range_singular!(ResetSyncLow);
impl_token_range_singular!(Return);
impl_token_range_singular!(Same);
impl_token_range_singular!(Signed);
impl_token_range_singular!(Step);

impl From<&Strin> for TokenRange {
    fn from(value: &Strin) -> Self {
        let beg = value.string_token.token;
        let end = beg;
        TokenRange { beg, end }
    }
}
impl_token_ext!(Strin);

impl_token_range_singular!(Struct);
impl_token_range_singular!(Switch);
impl_token_range_singular!(Tri);
impl_token_range_singular!(True);
impl_token_range_singular!(Type);
impl_token_range_singular!(U8);
impl_token_range_singular!(U16);
impl_token_range_singular!(U32);
impl_token_range_singular!(U64);
impl_token_range_singular!(Union);
impl_token_range_singular!(Unsafe);
impl_token_range_singular!(Var);

// Identifier
impl_token_range_singular!(DollarIdentifier);
impl_token_range_singular!(Identifier);

// ----------------------------------------------------------------------------
// Number
// ----------------------------------------------------------------------------

impl_token_range_enum!(Number, integral_number, real_number);
impl_token_range_enum!(IntegralNumber, based, base_less, all_bit);
impl_token_range_enum!(RealNumber, fixed_point, exponent);

// ----------------------------------------------------------------------------
// Complex Identifier
// ----------------------------------------------------------------------------

impl From<&HierarchicalIdentifier> for TokenRange {
    fn from(value: &HierarchicalIdentifier) -> Self {
        let mut ret: TokenRange = value.identifier.as_ref().into();
        if let Some(x) = value.hierarchical_identifier_list.last() {
            ret.set_end(x.select.as_ref().into());
        }
        if let Some(x) = value.hierarchical_identifier_list0.last() {
            ret.set_end(x.identifier.as_ref().into());
            if let Some(x) = x.hierarchical_identifier_list0_list.last() {
                ret.set_end(x.select.as_ref().into());
            }
        }
        ret
    }
}
impl_token_ext!(HierarchicalIdentifier);

impl From<&ScopedIdentifier> for TokenRange {
    fn from(value: &ScopedIdentifier) -> Self {
        let mut ret: TokenRange = match value.scoped_identifier_group.as_ref() {
            ScopedIdentifierGroup::DollarIdentifier(x) => x.dollar_identifier.as_ref().into(),
            ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                let mut ret: TokenRange = x.identifier.as_ref().into();
                if let Some(x) = &x.scoped_identifier_opt {
                    ret.set_end(x.with_generic_argument.as_ref().into());
                }
                ret
            }
        };
        if let Some(x) = value.scoped_identifier_list.last() {
            ret.set_end(x.identifier.as_ref().into());
            if let Some(x) = &x.scoped_identifier_opt0 {
                ret.set_end(x.with_generic_argument.as_ref().into());
            }
        }
        ret
    }
}
impl_token_ext!(ScopedIdentifier);

impl From<&ExpressionIdentifier> for TokenRange {
    fn from(value: &ExpressionIdentifier) -> Self {
        let mut ret: TokenRange = value.scoped_identifier.as_ref().into();
        if let Some(x) = &value.expression_identifier_opt {
            ret.set_end(x.width.as_ref().into());
        }
        if let Some(x) = &value.expression_identifier_list.last() {
            ret.set_end(x.select.as_ref().into());
        }
        if let Some(x) = &value.expression_identifier_list0.last() {
            ret.set_end(x.identifier.as_ref().into());
            if let Some(x) = &x.expression_identifier_list0_list.last() {
                ret.set_end(x.select.as_ref().into());
            }
        }
        ret
    }
}
impl_token_ext!(ExpressionIdentifier);

// ----------------------------------------------------------------------------
// Expression
// ----------------------------------------------------------------------------

impl_token_range!(Expression, if_expression);

impl From<&IfExpression> for TokenRange {
    fn from(value: &IfExpression) -> Self {
        let mut ret: TokenRange = value.expression01.as_ref().into();
        if let Some(x) = value.if_expression_list.first() {
            ret.set_beg(x.r#if.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(IfExpression);

expression_token_range!(Expression01, expression02, expression01_list, expression02);
expression_token_range!(Expression02, expression03, expression02_list, expression03);
expression_token_range!(Expression03, expression04, expression03_list, expression04);
expression_token_range!(Expression04, expression05, expression04_list, expression05);
expression_token_range!(Expression05, expression06, expression05_list, expression06);
expression_token_range!(Expression06, expression07, expression06_list, expression07);
expression_token_range!(Expression07, expression08, expression07_list, expression08);
expression_token_range!(Expression08, expression09, expression08_list, expression09);
expression_token_range!(Expression09, expression10, expression09_list, expression10);
expression_token_range!(Expression10, expression11, expression10_list, expression11);
expression_token_range!(Expression11, expression12, expression11_list, expression12);

impl From<&Expression12> for TokenRange {
    fn from(value: &Expression12) -> Self {
        let mut ret: TokenRange = value.expression13.as_ref().into();
        if let Some(ref x) = value.expression12_opt {
            ret.set_end(x.casting_type.as_ref().into());
        };
        ret
    }
}
impl_token_ext!(Expression12);

impl From<&Expression13> for TokenRange {
    fn from(value: &Expression13) -> Self {
        let mut ret: TokenRange = value.factor.as_ref().into();
        if let Some(x) = value.expression13_list.first() {
            ret.set_beg(match x.expression13_list_group.as_ref() {
                Expression13ListGroup::UnaryOperator(x) => x.unary_operator.as_ref().into(),
                Expression13ListGroup::Operator10(x) => x.operator10.as_ref().into(),
                Expression13ListGroup::Operator06(x) => x.operator06.as_ref().into(),
                Expression13ListGroup::Operator05(x) => x.operator05.as_ref().into(),
                Expression13ListGroup::Operator04(x) => x.operator04.as_ref().into(),
            });
        }
        ret
    }
}
impl_token_ext!(Expression13);

impl From<&Factor> for TokenRange {
    fn from(value: &Factor) -> Self {
        match value {
            Factor::Number(x) => x.number.as_ref().into(),
            Factor::BooleanLiteral(x) => x.boolean_literal.as_ref().into(),
            Factor::IdentifierFactor(x) => {
                x.identifier_factor.expression_identifier.as_ref().into()
            }
            Factor::LParenExpressionRParen(x) => {
                let beg = x.l_paren.l_paren_token.token;
                let end = x.r_paren.r_paren_token.token;
                TokenRange { beg, end }
            }
            Factor::LBraceConcatenationListRBrace(x) => {
                let beg = x.l_brace.l_brace_token.token;
                let end = x.r_brace.r_brace_token.token;
                TokenRange { beg, end }
            }
            Factor::QuoteLBraceArrayLiteralListRBrace(x) => {
                let beg = x.quote_l_brace.quote_l_brace_token.token;
                let end = x.r_brace.r_brace_token.token;
                TokenRange { beg, end }
            }
            Factor::CaseExpression(x) => x.case_expression.as_ref().into(),
            Factor::SwitchExpression(x) => x.switch_expression.as_ref().into(),
            Factor::StringLiteral(x) => x.string_literal.as_ref().into(),
            Factor::FactorGroup(x) => match x.factor_group.as_ref() {
                FactorGroup::Msb(x) => x.msb.as_ref().into(),
                FactorGroup::Lsb(x) => x.lsb.as_ref().into(),
            },
            Factor::InsideExpression(x) => x.inside_expression.as_ref().into(),
            Factor::OutsideExpression(x) => x.outside_expression.as_ref().into(),
            Factor::TypeExpression(x) => x.type_expression.as_ref().into(),
            Factor::FactorTypeFactor(x) => x.factor_type_factor.as_ref().into(),
        }
    }
}
impl_token_ext!(Factor);

impl_token_range_enum!(BooleanLiteral, r#true, r#false);

impl From<&IdentifierFactor> for TokenRange {
    fn from(value: &IdentifierFactor) -> Self {
        let mut ret: TokenRange = value.expression_identifier.as_ref().into();
        if let Some(x) = &value.identifier_factor_opt {
            ret.set_end(match x.identifier_factor_opt_group.as_ref() {
                IdentifierFactorOptGroup::FunctionCall(x) => x.function_call.as_ref().into(),
                IdentifierFactorOptGroup::StructConstructor(x) => {
                    x.struct_constructor.as_ref().into()
                }
            });
        }
        ret
    }
}
impl_token_ext!(IdentifierFactor);

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
impl_token_ext!(FactorTypeFactor);

impl_token_range!(FunctionCall, l_paren, r_paren);
impl_token_range_list!(ArgumentList, ArgumentItem);

impl From<&ArgumentItem> for TokenRange {
    fn from(value: &ArgumentItem) -> Self {
        let mut ret: TokenRange = value.argument_expression.as_ref().into();
        if let Some(x) = &value.argument_item_opt {
            ret.set_end(x.expression.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(ArgumentItem);

impl_token_range!(ArgumentExpression, expression);
impl_token_range!(StructConstructor, quote_l_brace, r_brace);
impl_token_range_list!(StructConstructorList, StructConstructorItem);
impl_token_range!(StructConstructorItem, identifier, expression);
impl_token_range_list!(ConcatenationList, ConcatenationItem);

impl From<&ConcatenationItem> for TokenRange {
    fn from(value: &ConcatenationItem) -> Self {
        let mut ret: TokenRange = value.expression.as_ref().into();
        if let Some(x) = &value.concatenation_item_opt {
            ret.set_end(x.expression.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(ConcatenationItem);

impl_token_range_list!(ArrayLiteralList, ArrayLiteralItem);

impl From<&ArrayLiteralItem> for TokenRange {
    fn from(value: &ArrayLiteralItem) -> Self {
        match value.array_literal_item_group.as_ref() {
            ArrayLiteralItemGroup::ExpressionArrayLiteralItemOpt(x) => {
                let mut ret: TokenRange = x.expression.as_ref().into();
                if let Some(x) = &x.array_literal_item_opt {
                    ret.set_end(x.expression.as_ref().into());
                }
                ret
            }
            ArrayLiteralItemGroup::DefaulColonExpression(x) => {
                let beg: TokenRange = x.defaul.as_ref().into();
                let end: TokenRange = x.expression.as_ref().into();
                TokenRange {
                    beg: beg.beg,
                    end: end.end,
                }
            }
        }
    }
}
impl_token_ext!(ArrayLiteralItem);

impl_token_range!(CaseExpression, case, r_brace);
impl_token_range!(SwitchExpression, switch, r_brace);
impl_token_range!(TypeExpression, r#type, r_paren);
impl_token_range!(InsideExpression, inside, r_brace);
impl_token_range!(OutsideExpression, outside, r_brace);
impl_token_range_list!(RangeList, RangeItem);
impl_token_range!(RangeItem, range);

// ----------------------------------------------------------------------------
// Select / Width / Array / Range
// ----------------------------------------------------------------------------

impl_token_range!(Select, l_bracket, r_bracket);
impl_token_range_enum!(SelectOperator, colon, plus_colon, minus_colon, step);
impl_token_range!(Width, l_angle, r_angle);
impl_token_range!(Array, l_bracket, r_bracket);

impl From<&Range> for TokenRange {
    fn from(value: &Range) -> Self {
        let mut ret: TokenRange = value.expression.as_ref().into();
        if let Some(x) = &value.range_opt {
            ret.set_end(x.expression.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(Range);

impl_token_range_enum!(RangeOperator, dot_dot, dot_dot_equ);

// ----------------------------------------------------------------------------
// ScalarType / ArrayType / CastingType
// ----------------------------------------------------------------------------

impl_token_range_enum!(
    FixedType, u8, u16, u32, u64, i32, i8, i16, i64, f32, f64, bool, strin
);
impl_token_range_enum!(
    VariableType,
    clock,
    clock_posedge,
    clock_negedge,
    reset,
    reset_async_high,
    reset_async_low,
    reset_sync_high,
    reset_sync_low,
    logic,
    bit
);
impl_token_range!(UserDefinedType, scoped_identifier);
impl_token_range_enum!(TypeModifier, tri, signed, defaul);

impl From<&FactorType> for TokenRange {
    fn from(value: &FactorType) -> Self {
        match value.factor_type_group.as_ref() {
            FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                let mut ret: TokenRange = x.variable_type.as_ref().into();
                if let Some(ref x) = x.factor_type_opt {
                    ret.set_end(x.width.as_ref().into());
                }
                ret
            }
            FactorTypeGroup::FixedType(x) => x.fixed_type.as_ref().into(),
        }
    }
}
impl_token_ext!(FactorType);

impl From<&ScalarType> for TokenRange {
    fn from(value: &ScalarType) -> Self {
        let mut ret: TokenRange = match &*value.scalar_type_group {
            ScalarTypeGroup::UserDefinedTypeScalarTypeOpt(x) => {
                let mut ret: TokenRange = x.user_defined_type.as_ref().into();
                if let Some(x) = &x.scalar_type_opt {
                    ret.set_end(x.width.as_ref().into());
                }
                ret
            }
            ScalarTypeGroup::FactorType(x) => x.factor_type.as_ref().into(),
        };

        if let Some(x) = value.scalar_type_list.first() {
            ret.set_beg(x.type_modifier.as_ref().into());
        }

        ret
    }
}
impl_token_ext!(ScalarType);

impl From<&ArrayType> for TokenRange {
    fn from(value: &ArrayType) -> Self {
        let mut ret: TokenRange = value.scalar_type.as_ref().into();
        if let Some(x) = &value.array_type_opt {
            ret.set_end(x.array.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(ArrayType);

impl_token_range_enum!(
    CastingType,
    u8,
    u16,
    u32,
    u64,
    i8,
    i16,
    i32,
    i64,
    f32,
    f64,
    bool,
    clock,
    clock_posedge,
    clock_negedge,
    reset,
    reset_async_high,
    reset_async_low,
    reset_sync_high,
    reset_sync_low,
    user_defined_type,
    based,
    base_less
);

// ----------------------------------------------------------------------------
// ClockDomain
// ----------------------------------------------------------------------------

impl_token_range!(ClockDomain, quote, identifier);

// ----------------------------------------------------------------------------
// Statement
// ----------------------------------------------------------------------------

impl_token_range!(StatementBlock, l_brace, r_brace);
impl_token_range_group!(StatementBlockGroup, StatementBlockItem);
impl_token_range_enum!(
    StatementBlockItem,
    var_declaration,
    let_statement,
    statement
);
impl_token_range_enum!(
    Statement,
    identifier_statement,
    if_statement,
    if_reset_statement,
    return_statement,
    break_statement,
    for_statement,
    case_statement,
    switch_statement
);
impl_token_range!(LetStatement, r#let, semicolon);
impl_token_range!(IdentifierStatement, expression_identifier, semicolon);

impl From<&Assignment> for TokenRange {
    fn from(value: &Assignment) -> Self {
        let mut ret: TokenRange = match value.assignment_group.as_ref() {
            AssignmentGroup::Equ(x) => x.equ.as_ref().into(),
            AssignmentGroup::AssignmentOperator(x) => x.assignment_operator.as_ref().into(),
            AssignmentGroup::DiamondOperator(x) => x.diamond_operator.as_ref().into(),
        };
        ret.set_end(value.expression.as_ref().into());
        ret
    }
}
impl_token_ext!(Assignment);

impl From<&IfStatement> for TokenRange {
    fn from(value: &IfStatement) -> Self {
        let mut ret: TokenRange = value.r#if.as_ref().into();
        ret.set_end(value.statement_block.as_ref().into());
        if let Some(x) = value.if_statement_list.last() {
            ret.set_end(x.statement_block.as_ref().into());
        }
        if let Some(x) = &value.if_statement_opt {
            ret.set_end(x.statement_block.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(IfStatement);

impl From<&IfResetStatement> for TokenRange {
    fn from(value: &IfResetStatement) -> Self {
        let mut ret: TokenRange = value.if_reset.as_ref().into();
        ret.set_end(value.statement_block.as_ref().into());
        if let Some(x) = value.if_reset_statement_list.last() {
            ret.set_end(x.statement_block.as_ref().into());
        }
        if let Some(x) = &value.if_reset_statement_opt {
            ret.set_end(x.statement_block.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(IfResetStatement);

impl_token_range!(ReturnStatement, r#return, semicolon);
impl_token_range!(BreakStatement, r#break, semicolon);
impl_token_range!(ForStatement, r#for, statement_block);
impl_token_range!(CaseStatement, r#case, r_brace);

impl From<&CaseItem> for TokenRange {
    fn from(value: &CaseItem) -> Self {
        let mut ret: TokenRange = match value.case_item_group.as_ref() {
            CaseItemGroup::CaseCondition(x) => x.case_condition.as_ref().into(),
            CaseItemGroup::Defaul(x) => x.defaul.as_ref().into(),
        };
        match value.case_item_group0.as_ref() {
            CaseItemGroup0::Statement(x) => {
                ret.set_end(x.statement.as_ref().into());
            }
            CaseItemGroup0::StatementBlock(x) => {
                ret.set_end(x.statement_block.as_ref().into());
            }
        }
        ret
    }
}
impl_token_ext!(CaseItem);

impl From<&CaseCondition> for TokenRange {
    fn from(value: &CaseCondition) -> Self {
        let mut ret: TokenRange = value.range_item.as_ref().into();
        if let Some(x) = value.case_condition_list.last() {
            ret.set_end(x.range_item.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(CaseCondition);

impl_token_range!(SwitchStatement, switch, r_brace);

impl From<&SwitchItem> for TokenRange {
    fn from(value: &SwitchItem) -> Self {
        let mut ret: TokenRange = match value.switch_item_group.as_ref() {
            SwitchItemGroup::SwitchCondition(x) => x.switch_condition.as_ref().into(),
            SwitchItemGroup::Defaul(x) => x.defaul.as_ref().into(),
        };
        match value.switch_item_group0.as_ref() {
            SwitchItemGroup0::Statement(x) => {
                ret.set_end(x.statement.as_ref().into());
            }
            SwitchItemGroup0::StatementBlock(x) => {
                ret.set_end(x.statement_block.as_ref().into());
            }
        }
        ret
    }
}
impl_token_ext!(SwitchItem);

impl From<&SwitchCondition> for TokenRange {
    fn from(value: &SwitchCondition) -> Self {
        let mut ret: TokenRange = value.expression.as_ref().into();
        if let Some(x) = value.switch_condition_list.last() {
            ret.set_end(x.expression.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(SwitchCondition);

// ----------------------------------------------------------------------------
// Attribute
// ----------------------------------------------------------------------------

impl_token_range!(Attribute, hash, r_bracket);
impl_token_range_list!(AttributeList, AttributeItem);
impl_token_range_enum!(AttributeItem, identifier, string_literal);

// ----------------------------------------------------------------------------
// Declaration
// ----------------------------------------------------------------------------

impl_token_range!(LetDeclaration, r#let, semicolon);
impl_token_range!(VarDeclaration, var, semicolon);
impl_token_range!(ConstDeclaration, r#const, semicolon);
impl_token_range!(TypeDefDeclaration, r#type, semicolon);
impl_token_range!(AlwaysFfDeclaration, always_ff, statement_block);
impl_token_range!(AlwaysFfEventList, l_paren, r_paren);
impl_token_range!(AlwaysFfClock, hierarchical_identifier);
impl_token_range!(AlwaysFfReset, hierarchical_identifier);
impl_token_range!(AlwaysCombDeclaration, always_comb, statement_block);
impl_token_range!(AssignDeclaration, assign, semicolon);

impl From<&AssignDestination> for TokenRange {
    fn from(value: &AssignDestination) -> Self {
        match value {
            AssignDestination::HierarchicalIdentifier(x) => {
                x.hierarchical_identifier.as_ref().into()
            }
            AssignDestination::LBraceAssignConcatenationListRBrace(x) => {
                let beg = x.l_brace.l_brace_token.token;
                let end = x.r_brace.r_brace_token.token;
                TokenRange { beg, end }
            }
        }
    }
}
impl_token_ext!(AssignDestination);

impl_token_range_list!(AssignConcatenationList, AssignConcatenationItem);
impl_token_range!(AssignConcatenationItem, hierarchical_identifier);
impl_token_range!(ConnectDeclaration, connect, semicolon);
impl_token_range!(ModportDeclaration, modport, r_brace);
impl_token_range_list!(ModportList, ModportGroup);
impl_token_range_group!(ModportGroup, ModportList, ModportItem);
impl_token_range!(ModportItem, identifier, direction);

impl From<&ModportDefault> for TokenRange {
    fn from(value: &ModportDefault) -> Self {
        match value {
            ModportDefault::Input(x) => x.input.as_ref().into(),
            ModportDefault::Output(x) => x.output.as_ref().into(),
            ModportDefault::SameLParenIdentifierRParen(x) => {
                let beg = x.same.same_token.token;
                let end = x.r_paren.r_paren_token.token;
                TokenRange { beg, end }
            }
            ModportDefault::ConverseLParenIdentifierRParen(x) => {
                let beg = x.converse.converse_token.token;
                let end = x.r_paren.r_paren_token.token;
                TokenRange { beg, end }
            }
        }
    }
}
impl_token_ext!(ModportDefault);

impl_token_range!(EnumDeclaration, r#enum, r_brace);
impl_token_range_list!(EnumList, EnumGroup);
impl_token_range_group!(EnumGroup, EnumList, EnumItem);

impl From<&EnumItem> for TokenRange {
    fn from(value: &EnumItem) -> Self {
        let mut ret: TokenRange = value.identifier.as_ref().into();
        if let Some(x) = &value.enum_item_opt {
            ret.set_end(x.expression.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(EnumItem);

impl_token_range_enum!(StructUnion, r#struct, union);
impl_token_range!(StructUnionDeclaration, struct_union, r_brace);
impl_token_range_list!(StructUnionList, StructUnionGroup);
impl_token_range_group!(StructUnionGroup, StructUnionList, StructUnionItem);
impl_token_range!(StructUnionItem, identifier, scalar_type);
impl_token_range!(InitialDeclaration, initial, statement_block);
impl_token_range!(FinalDeclaration, r#final, statement_block);

// ----------------------------------------------------------------------------
// InstDeclaration
// ----------------------------------------------------------------------------

impl_token_range!(InstDeclaration, inst, semicolon);
impl_token_range!(InstParameter, hash, r_paren);
impl_token_range_list!(InstParameterList, InstParameterGroup);
impl_token_range_group!(InstParameterGroup, InstParameterList, InstParameterItem);

impl From<&InstParameterItem> for TokenRange {
    fn from(value: &InstParameterItem) -> Self {
        let mut ret: TokenRange = value.identifier.as_ref().into();
        if let Some(x) = &value.inst_parameter_item_opt {
            ret.set_end(x.expression.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(InstParameterItem);

impl_token_range_list!(InstPortList, InstPortGroup);
impl_token_range_group!(InstPortGroup, InstPortList, InstPortItem);

impl From<&InstPortItem> for TokenRange {
    fn from(value: &InstPortItem) -> Self {
        let mut ret: TokenRange = value.identifier.as_ref().into();
        if let Some(x) = &value.inst_port_item_opt {
            ret.set_end(x.expression.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(InstPortItem);

// ----------------------------------------------------------------------------
// WithParameter
// ----------------------------------------------------------------------------

impl_token_range!(WithParameter, hash, r_paren);
impl_token_range_list!(WithParameterList, WithParameterGroup);
impl_token_range_group!(WithParameterGroup, WithParameterList, WithParameterItem);

impl From<&WithParameterItem> for TokenRange {
    fn from(value: &WithParameterItem) -> Self {
        let mut ret: TokenRange = match value.with_parameter_item_group.as_ref() {
            WithParameterItemGroup::Param(x) => x.param.as_ref().into(),
            WithParameterItemGroup::Const(x) => x.r#const.as_ref().into(),
        };
        ret.set_end(value.expression.as_ref().into());
        ret
    }
}
impl_token_ext!(WithParameterItem);

// ----------------------------------------------------------------------------
// WithGenericParameter
// ----------------------------------------------------------------------------

impl From<&GenericBound> for TokenRange {
    fn from(value: &GenericBound) -> Self {
        match value {
            GenericBound::Type(x) => x.r#type.as_ref().into(),
            GenericBound::InstScopedIdentifier(x) => {
                let mut ret: TokenRange = x.inst.as_ref().into();
                ret.set_end(x.scoped_identifier.as_ref().into());
                ret
            }
            GenericBound::GenericProtoBound(x) => x.generic_proto_bound.as_ref().into(),
        }
    }
}
impl_token_ext!(GenericBound);

impl_token_range!(WithGenericParameter, colon_colon_l_angle, r_angle);
impl_token_range_list!(WithGenericParameterList, WithGenericParameterItem);

impl From<&WithGenericParameterItem> for TokenRange {
    fn from(value: &WithGenericParameterItem) -> Self {
        let mut ret: TokenRange = value.identifier.as_ref().into();
        ret.set_end(value.generic_bound.as_ref().into());
        if let Some(x) = &value.with_generic_parameter_item_opt {
            ret.set_end(x.with_generic_argument_item.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(WithGenericParameterItem);

impl_token_range_enum!(GenericProtoBound, scoped_identifier, fixed_type);

// ----------------------------------------------------------------------------
// WithGenericArgument
// ----------------------------------------------------------------------------

impl_token_range!(WithGenericArgument, colon_colon_l_angle, r_angle);
impl_token_range_list!(WithGenericArgumentList, WithGenericArgumentItem);
impl_token_range_enum!(
    WithGenericArgumentItem,
    scoped_identifier,
    fixed_type,
    number,
    boolean_literal
);

// ----------------------------------------------------------------------------
// PortDeclaration
// ----------------------------------------------------------------------------

impl_token_range!(PortDeclaration, l_paren, r_paren);
impl_token_range_list!(PortDeclarationList, PortDeclarationGroup);
impl_token_range_group!(
    PortDeclarationGroup,
    PortDeclarationList,
    PortDeclarationItem
);

impl From<&PortDeclarationItem> for TokenRange {
    fn from(value: &PortDeclarationItem) -> Self {
        let mut ret: TokenRange = value.identifier.as_ref().into();
        match value.port_declaration_item_group.as_ref() {
            PortDeclarationItemGroup::PortTypeConcrete(x) => {
                ret.set_end(x.port_type_concrete.as_ref().into());
            }
            PortDeclarationItemGroup::PortTypeAbstract(x) => {
                ret.set_end(x.port_type_abstract.as_ref().into());
            }
        }
        ret
    }
}
impl_token_ext!(PortDeclarationItem);

impl From<&PortTypeConcrete> for TokenRange {
    fn from(value: &PortTypeConcrete) -> Self {
        let mut ret: TokenRange = value.direction.as_ref().into();
        ret.set_end(value.array_type.as_ref().into());
        if let Some(x) = &value.port_type_concrete_opt0 {
            ret.set_end(x.port_default_value.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(PortTypeConcrete);

impl_token_range!(PortDefaultValue, expression);

impl From<&PortTypeAbstract> for TokenRange {
    fn from(value: &PortTypeAbstract) -> Self {
        let mut ret: TokenRange = value.interface.as_ref().into();
        if let Some(x) = &value.port_type_abstract_opt {
            ret.set_beg(x.clock_domain.as_ref().into());
        }
        if let Some(x) = &value.port_type_abstract_opt0 {
            ret.set_beg(x.identifier.as_ref().into());
        }
        if let Some(x) = &value.port_type_abstract_opt1 {
            ret.set_beg(x.array.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(PortTypeAbstract);

impl_token_range_enum!(Direction, input, output, inout, modport, import);

// ----------------------------------------------------------------------------
// Function
// ----------------------------------------------------------------------------

impl_token_range!(FunctionDeclaration, function, statement_block);

// ----------------------------------------------------------------------------
// Import
// ----------------------------------------------------------------------------

impl_token_range!(ImportDeclaration, import, semicolon);

// ----------------------------------------------------------------------------
// Unsafe
// ----------------------------------------------------------------------------

impl_token_range!(UnsafeBlock, r#unsafe, r_brace);

// ----------------------------------------------------------------------------
// Module/Interface
// ----------------------------------------------------------------------------

impl_token_range!(ModuleDeclaration, module, r_brace);
impl_token_range_group!(ModuleGroup, ModuleItem);
impl_token_range!(ModuleItem, generate_item);
impl_token_range!(InterfaceDeclaration, interface, r_brace);
impl_token_range_group!(InterfaceGroup, InterfaceItem);
impl_token_range_enum!(InterfaceItem, generate_item, modport_declaration);

impl From<&GenerateIfDeclaration> for TokenRange {
    fn from(value: &GenerateIfDeclaration) -> Self {
        let mut ret: TokenRange = value.r#if.as_ref().into();
        ret.set_end(value.generate_named_block.as_ref().into());
        if let Some(x) = value.generate_if_declaration_list.last() {
            ret.set_end(x.generate_optional_named_block.as_ref().into());
        }
        if let Some(x) = &value.generate_if_declaration_opt {
            ret.set_end(x.generate_optional_named_block.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(GenerateIfDeclaration);

impl_token_range!(GenerateForDeclaration, r#for, generate_named_block);
impl_token_range!(GenerateBlockDeclaration, generate_named_block);
impl_token_range!(GenerateNamedBlock, colon, r_brace);

impl From<&GenerateOptionalNamedBlock> for TokenRange {
    fn from(value: &GenerateOptionalNamedBlock) -> Self {
        let beg = value.l_brace.l_brace_token.token;
        let end = value.r_brace.r_brace_token.token;
        let mut ret = TokenRange { beg, end };
        if let Some(x) = &value.generate_optional_named_block_opt {
            ret.set_beg(x.colon.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(GenerateOptionalNamedBlock);

impl_token_range_group!(GenerateGroup, GenerateItem);
impl_token_range_enum!(
    GenerateItem,
    let_declaration,
    var_declaration,
    inst_declaration,
    const_declaration,
    always_ff_declaration,
    always_comb_declaration,
    assign_declaration,
    connect_declaration,
    function_declaration,
    generate_if_declaration,
    generate_for_declaration,
    generate_block_declaration,
    type_def_declaration,
    enum_declaration,
    struct_union_declaration,
    import_declaration,
    alias_declaration,
    initial_declaration,
    final_declaration,
    unsafe_block
);

// ----------------------------------------------------------------------------
// Package
// ----------------------------------------------------------------------------

impl_token_range!(PackageDeclaration, package, r_brace);
impl_token_range_group!(PackageGroup, PackageItem);
impl_token_range_enum!(
    PackageItem,
    const_declaration,
    type_def_declaration,
    enum_declaration,
    struct_union_declaration,
    function_declaration,
    import_declaration,
    alias_declaration
);

// ----------------------------------------------------------------------------
// Alias
// ----------------------------------------------------------------------------

impl_token_range!(AliasDeclaration, alias, semicolon);

// ----------------------------------------------------------------------------
// Proto
// ----------------------------------------------------------------------------

impl From<&ProtoDeclaration> for TokenRange {
    fn from(value: &ProtoDeclaration) -> Self {
        let beg: TokenRange = value.proto.as_ref().into();
        let end: TokenRange = match &*value.proto_declaration_group {
            ProtoDeclarationGroup::ProtoModuleDeclaration(x) => {
                x.proto_module_declaration.as_ref().into()
            }
            ProtoDeclarationGroup::ProtoInterfaceDeclaration(x) => {
                x.proto_interface_declaration.as_ref().into()
            }
            ProtoDeclarationGroup::ProtoPackageDeclaration(x) => {
                x.proto_package_declaration.as_ref().into()
            }
        };
        TokenRange {
            beg: beg.beg,
            end: end.end,
        }
    }
}
impl_token_range!(ProtoModuleDeclaration, module, semicolon);
impl_token_range!(ProtoInterfaceDeclaration, interface, r_brace);
impl_token_range_enum!(
    ProtoInterfaceItem,
    var_declaration,
    proto_const_declaration,
    proto_function_declaration,
    proto_type_def_declaration,
    proto_alias_declaration,
    modport_declaration,
    import_declaration
);
impl_token_range!(ProtoPackageDeclaration, package, r_brace);
impl_token_range_enum!(
    ProtoPacakgeItem,
    proto_const_declaration,
    proto_type_def_declaration,
    enum_declaration,
    struct_union_declaration,
    proto_function_declaration,
    proto_alias_declaration,
    import_declaration
);
impl_token_range!(ProtoConstDeclaration, r#const, semicolon);
impl_token_range!(ProtoTypeDefDeclaration, r#type, semicolon);
impl_token_range!(ProtoFunctionDeclaration, function, semicolon);
impl_token_range!(ProtoAliasDeclaration, alias, semicolon);

// ----------------------------------------------------------------------------
// Embed
// ----------------------------------------------------------------------------

impl_token_range!(EmbedDeclaration, embed, embed_content);
impl_token_range_singular!(EmbedContent);

// ----------------------------------------------------------------------------
// Include
// ----------------------------------------------------------------------------

impl_token_range!(IncludeDeclaration, include, semicolon);

// ----------------------------------------------------------------------------
// Description
// ----------------------------------------------------------------------------

impl From<&DescriptionItem> for TokenRange {
    fn from(value: &DescriptionItem) -> Self {
        match value {
            DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) => {
                let mut ret: TokenRange = x.public_description_item.as_ref().into();
                if let Some(x) = &x.description_item_opt {
                    ret.set_beg(x.r#pub.as_ref().into());
                }
                ret
            }
            DescriptionItem::ImportDeclaration(x) => x.import_declaration.as_ref().into(),
            DescriptionItem::EmbedDeclaration(x) => x.embed_declaration.as_ref().into(),
            DescriptionItem::IncludeDeclaration(x) => x.include_declaration.as_ref().into(),
        }
    }
}
impl_token_ext!(DescriptionItem);

impl_token_range_group!(DescriptionGroup, DescriptionItem);
impl_token_range_enum!(
    PublicDescriptionItem,
    module_declaration,
    interface_declaration,
    package_declaration,
    alias_declaration,
    proto_declaration
);

// ----------------------------------------------------------------------------
// SourceCode
// ----------------------------------------------------------------------------

impl From<&Veryl> for TokenRange {
    fn from(value: &Veryl) -> Self {
        let mut ret: TokenRange = value.start.as_ref().into();
        if let Some(x) = value.veryl_list.last() {
            ret.set_end(x.description_group.as_ref().into());
        }
        ret
    }
}
impl_token_ext!(Veryl);
