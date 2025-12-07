use crate::value::Value;
use std::fmt;
use veryl_parser::veryl_grammar_trait as syntax_tree;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Literal {
    Value(Value),
    Type(TypeLiteral),
    Boolean(bool),
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Value(x) => format!("{x:x}").fmt(f),
            Literal::Type(x) => x.fmt(f),
            Literal::Boolean(x) => x.fmt(f),
        }
    }
}

impl Literal {
    pub fn is_fixed_type(&self) -> bool {
        if let Literal::Type(x) = self {
            matches!(
                x,
                TypeLiteral::U8
                    | TypeLiteral::U16
                    | TypeLiteral::U32
                    | TypeLiteral::U64
                    | TypeLiteral::I8
                    | TypeLiteral::I16
                    | TypeLiteral::I32
                    | TypeLiteral::I64
                    | TypeLiteral::F32
                    | TypeLiteral::F64
                    | TypeLiteral::String
            )
        } else {
            false
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeLiteral {
    Bit,
    Bool,
    Clock,
    ClockPosedge,
    ClockNegedge,
    F32,
    F64,
    I8,
    I16,
    I32,
    I64,
    Logic,
    Reset,
    ResetAsyncHigh,
    ResetAsyncLow,
    ResetSyncHigh,
    ResetSyncLow,
    String,
    U8,
    U16,
    U32,
    U64,
}

impl TypeLiteral {
    pub fn to_sv_string(&self) -> String {
        let text = match self {
            TypeLiteral::Bit => "bit",
            TypeLiteral::Bool => "logic",
            TypeLiteral::Clock => "logic",
            TypeLiteral::ClockPosedge => "logic",
            TypeLiteral::ClockNegedge => "logic",
            TypeLiteral::F32 => "shortreal",
            TypeLiteral::F64 => "real",
            TypeLiteral::I8 => "byte signed",
            TypeLiteral::I16 => "shortint signed",
            TypeLiteral::I32 => "int signed",
            TypeLiteral::I64 => "longint signed",
            TypeLiteral::Logic => "logic",
            TypeLiteral::Reset => "logic",
            TypeLiteral::ResetAsyncHigh => "logic",
            TypeLiteral::ResetAsyncLow => "logic",
            TypeLiteral::ResetSyncHigh => "logic",
            TypeLiteral::ResetSyncLow => "logic",
            TypeLiteral::String => "string",
            TypeLiteral::U8 => "byte unsigned",
            TypeLiteral::U16 => "shortint unsigned",
            TypeLiteral::U32 => "int unsigned",
            TypeLiteral::U64 => "longint unsigned",
        };
        text.to_string()
    }
}

impl fmt::Display for TypeLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeLiteral::Bit => "bit".fmt(f),
            TypeLiteral::Bool => "bool".fmt(f),
            TypeLiteral::Clock => "clock".fmt(f),
            TypeLiteral::ClockPosedge => "clock_posedge".fmt(f),
            TypeLiteral::ClockNegedge => "clock_negedge".fmt(f),
            TypeLiteral::F32 => "f32".fmt(f),
            TypeLiteral::F64 => "f64".fmt(f),
            TypeLiteral::I8 => "i8".fmt(f),
            TypeLiteral::I16 => "i16".fmt(f),
            TypeLiteral::I32 => "i32".fmt(f),
            TypeLiteral::I64 => "i64".fmt(f),
            TypeLiteral::Logic => "logic".fmt(f),
            TypeLiteral::Reset => "reset".fmt(f),
            TypeLiteral::ResetAsyncHigh => "reset_async_high".fmt(f),
            TypeLiteral::ResetAsyncLow => "reset_async_low".fmt(f),
            TypeLiteral::ResetSyncHigh => "reset_sync_high".fmt(f),
            TypeLiteral::ResetSyncLow => "reset_sync_low".fmt(f),
            TypeLiteral::String => "string".fmt(f),
            TypeLiteral::U8 => "u8".fmt(f),
            TypeLiteral::U16 => "u16".fmt(f),
            TypeLiteral::U32 => "u32".fmt(f),
            TypeLiteral::U64 => "u64".fmt(f),
        }
    }
}

macro_rules! impl_to_literal {
    ($x:ident) => {
        impl From<&syntax_tree::$x> for Literal {
            fn from(_: &syntax_tree::$x) -> Self {
                Self::Type(TypeLiteral::$x)
            }
        }
    };
}

macro_rules! impl_to_type_literal {
    ($x:ident) => {
        impl From<&syntax_tree::$x> for TypeLiteral {
            fn from(_: &syntax_tree::$x) -> Self {
                Self::$x
            }
        }
    };
}

impl_to_literal!(Bit);
impl_to_literal!(Bool);
impl_to_literal!(Clock);
impl_to_literal!(ClockPosedge);
impl_to_literal!(ClockNegedge);
impl_to_literal!(F32);
impl_to_literal!(F64);
impl_to_literal!(I8);
impl_to_literal!(I16);
impl_to_literal!(I32);
impl_to_literal!(I64);
impl_to_literal!(Logic);
impl_to_literal!(Reset);
impl_to_literal!(ResetAsyncHigh);
impl_to_literal!(ResetAsyncLow);
impl_to_literal!(ResetSyncHigh);
impl_to_literal!(ResetSyncLow);
impl_to_literal!(U8);
impl_to_literal!(U16);
impl_to_literal!(U32);
impl_to_literal!(U64);

impl_to_type_literal!(Bit);
impl_to_type_literal!(Bool);
impl_to_type_literal!(Clock);
impl_to_type_literal!(ClockPosedge);
impl_to_type_literal!(ClockNegedge);
impl_to_type_literal!(F32);
impl_to_type_literal!(F64);
impl_to_type_literal!(I8);
impl_to_type_literal!(I16);
impl_to_type_literal!(I32);
impl_to_type_literal!(I64);
impl_to_type_literal!(Logic);
impl_to_type_literal!(Reset);
impl_to_type_literal!(ResetAsyncHigh);
impl_to_type_literal!(ResetAsyncLow);
impl_to_type_literal!(ResetSyncHigh);
impl_to_type_literal!(ResetSyncLow);
impl_to_type_literal!(U8);
impl_to_type_literal!(U16);
impl_to_type_literal!(U32);
impl_to_type_literal!(U64);

impl From<&syntax_tree::Strin> for Literal {
    fn from(_: &syntax_tree::Strin) -> Self {
        Self::Type(TypeLiteral::String)
    }
}

impl From<&syntax_tree::Strin> for TypeLiteral {
    fn from(_: &syntax_tree::Strin) -> Self {
        Self::String
    }
}
