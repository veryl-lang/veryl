use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::ir::{Op, Value};
use crate::symbol::SymbolId;
use crate::symbol_table;
use itertools::join;
use num_bigint::BigUint;
use std::fmt;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::ScopedIdentifier;

#[derive(Clone, Debug)]
pub struct TypedValue {
    pub value: ValueVariant,
    pub r#type: Type,
    pub is_const: bool,
}

impl TypedValue {
    pub fn create_unknown() -> TypedValue {
        TypedValue {
            value: ValueVariant::Unknown,
            r#type: Type {
                kind: TypeKind::Unknown,
                signed: false,
                width: vec![],
                array: vec![],
            },
            is_const: false,
        }
    }

    pub fn create_value(value: BigUint, width: usize) -> TypedValue {
        let value = Value::new(value, width, false);
        TypedValue {
            value: ValueVariant::Numeric(value),
            r#type: Type {
                kind: TypeKind::Bit,
                signed: false,
                width: vec![width],
                array: vec![],
            },
            is_const: true,
        }
    }

    pub fn from_type(r#type: Type) -> TypedValue {
        TypedValue {
            value: ValueVariant::Unknown,
            r#type,
            is_const: false,
        }
    }

    pub fn get_value(&self) -> Option<Value> {
        if let ValueVariant::Numeric(x) = &self.value {
            Some(x.clone())
        } else {
            None
        }
    }

    pub fn invalid_operand(&mut self, context: &mut Context, op: Op, x: &Type, range: &TokenRange) {
        context.insert_error(AnalyzerError::invalid_operand(
            &x.to_string(),
            &op.to_string(),
            range,
        ));
        self.value = ValueVariant::Unknown;
        self.r#type = Type {
            kind: TypeKind::Unknown,
            signed: false,
            width: vec![],
            array: vec![],
        };
        self.is_const = false;
    }

    pub fn invalid_logical_operand(&mut self, context: &mut Context, range: &TokenRange) {
        context.insert_error(AnalyzerError::invalid_logical_operand(true, range));
        self.value = ValueVariant::Unknown;
        self.r#type = Type {
            kind: TypeKind::Unknown,
            signed: false,
            width: vec![],
            array: vec![],
        };
        self.is_const = false;
    }
}

#[derive(Clone, Debug)]
pub enum ValueVariant {
    Numeric(Value),
    NumericArray(Vec<Value>),
    Type(Type),
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Type {
    pub kind: TypeKind,
    pub signed: bool,
    pub width: Vec<usize>,
    pub array: Vec<usize>,
}

impl Type {
    pub fn new(kind: TypeKind, width: usize, signed: bool) -> Type {
        Type {
            kind,
            signed,
            width: vec![width],
            array: vec![],
        }
    }

    pub fn is_4state(&self) -> bool {
        matches!(self.kind, TypeKind::Logic | TypeKind::UserDefined(_))
            | self.is_clock()
            | self.is_reset()
    }

    pub fn is_2state(&self) -> bool {
        // Unknown may be 2state value
        matches!(self.kind, TypeKind::Bit | TypeKind::Unknown)
    }

    pub fn is_clock(&self) -> bool {
        matches!(
            self.kind,
            TypeKind::Clock | TypeKind::ClockPosedge | TypeKind::ClockNegedge
        )
    }

    pub fn is_reset(&self) -> bool {
        matches!(
            self.kind,
            TypeKind::Reset
                | TypeKind::ResetAsyncHigh
                | TypeKind::ResetAsyncLow
                | TypeKind::ResetSyncHigh
                | TypeKind::ResetSyncLow
        )
    }

    pub fn is_type(&self) -> bool {
        matches!(self.kind, TypeKind::Type)
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self.kind, TypeKind::Unknown)
    }

    pub fn is_user_defined(&self) -> bool {
        matches!(self.kind, TypeKind::UserDefined(_))
    }

    pub fn is_array(&self) -> bool {
        !self.array.is_empty()
    }

    pub fn is_binary(&self) -> bool {
        self.total_width() == 1
    }

    pub fn total_width(&self) -> usize {
        self.width.iter().product::<usize>()
    }

    pub fn total_array(&self) -> usize {
        if self.array.is_empty() {
            1
        } else {
            self.array.iter().product::<usize>()
        }
    }

    pub fn compatible(&self, src: &Type) -> bool {
        if self.is_unknown() | src.is_unknown() {
            true
        } else if self.is_user_defined() | src.is_user_defined() {
            // TODO
            // refer evaluate_connection in check_expression.rs for modport/interface compatibility
            true
        } else if self.is_type() {
            src.is_type()
        } else if self.is_2state() {
            src.is_2state()
        } else if self.is_array() || src.is_array() {
            self.array == src.array
        } else {
            // TODO width array check
            true
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = if self.signed {
            "signed ".to_string()
        } else {
            String::new()
        };
        ret.push_str(&self.kind.to_string());

        if !self.width.is_empty() {
            ret.push('<');
            ret.push_str(&join(&self.width, ", "));
            ret.push('>');
        }

        if !self.array.is_empty() {
            ret.push('[');
            ret.push_str(&join(&self.array, ", "));
            ret.push(']');
        }

        ret.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub enum TypeKind {
    Clock,
    ClockPosedge,
    ClockNegedge,
    Reset,
    ResetAsyncHigh,
    ResetAsyncLow,
    ResetSyncHigh,
    ResetSyncLow,
    Bit,
    Logic,
    UserDefined(UserDefined),
    Type,
    Unknown,
}

#[derive(Clone, Debug)]
pub enum UserDefined {
    Identifier(ScopedIdentifier),
    Symbol(SymbolId),
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeKind::Clock => "clock".fmt(f),
            TypeKind::ClockPosedge => "clock_posedge".fmt(f),
            TypeKind::ClockNegedge => "clock_negedge".fmt(f),
            TypeKind::Reset => "reset".fmt(f),
            TypeKind::ResetAsyncHigh => "reset_async_high".fmt(f),
            TypeKind::ResetAsyncLow => "reset_async_low".fmt(f),
            TypeKind::ResetSyncHigh => "reset_sync_high".fmt(f),
            TypeKind::ResetSyncLow => "reset_sync_low".fmt(f),
            TypeKind::Bit => "bit".fmt(f),
            TypeKind::Logic => "logic".fmt(f),
            TypeKind::UserDefined(x) => match x {
                UserDefined::Identifier(x) => x.identifier().fmt(f),
                UserDefined::Symbol(x) => {
                    let symbol = symbol_table::get(*x).unwrap();
                    symbol.token.text.to_string().fmt(f)
                }
            },
            TypeKind::Type => "type".fmt(f),
            TypeKind::Unknown => "unknown".fmt(f),
        }
    }
}
