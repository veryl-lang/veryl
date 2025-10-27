use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::conv::utils::match_interface;
use crate::ir::{Op, Value, VarPath};
use crate::symbol::{Direction, Symbol, SymbolId, SymbolKind};
use crate::symbol_table;
use itertools::join;
use num_bigint::BigUint;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug)]
pub struct TypedValue {
    pub value: ValueVariant,
    pub r#type: Type,
    pub is_const: bool,
    pub is_global: bool,
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
            is_global: false,
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
            is_global: true,
        }
    }

    pub fn from_type(r#type: Type) -> TypedValue {
        TypedValue {
            value: ValueVariant::Unknown,
            r#type,
            is_const: false,
            is_global: false,
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

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueVariant {
    Numeric(Value),
    NumericArray(Vec<Value>),
    Type(Type),
    Unknown,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Type {
    pub kind: TypeKind,
    pub signed: bool,
    pub width: Vec<usize>,
    pub array: Vec<usize>,
}

impl Type {
    pub fn new(kind: TypeKind, width: Vec<usize>, signed: bool) -> Type {
        Type {
            kind,
            signed,
            width,
            array: vec![],
        }
    }

    pub fn is_4state(&self) -> bool {
        matches!(self.kind, TypeKind::Logic | TypeKind::UserDefined(_))
            | self.is_clock()
            | self.is_reset()
    }

    pub fn is_2state(&self) -> bool {
        matches!(self.kind, TypeKind::Bit)
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
            // TODO type compatible check
            let dst = self.kind.symbol();
            let src = src.kind.symbol();
            if let Some(dst) = dst
                && let SymbolKind::Modport(x) = dst.kind
            {
                if let Some(src) = src {
                    let dst = symbol_table::get(x.interface).unwrap();
                    match_interface(&dst, &src)
                } else {
                    false
                }
            } else {
                true
            }
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

    pub fn expand(&self, context: &mut Context, path: &VarPath) -> Vec<ExpandedType> {
        let mut ret = self.expand_inner(context, path, 0).0;
        ret.reverse();
        ret
    }

    fn expand_inner(
        &self,
        context: &mut Context,
        path: &VarPath,
        mut offset: usize,
    ) -> (Vec<ExpandedType>, usize) {
        let mut ret = vec![];

        let mut array = self.array.clone();
        array.append(&mut self.width.clone());

        let mut array = self.array.clone();
        array.append(&mut self.width.clone());
        if let TypeKind::UserDefined(x) = &self.kind {
            let symbol = symbol_table::get(*x).unwrap();
            match &symbol.kind {
                SymbolKind::Struct(x) => {
                    // Reverse order to iterate from LSB
                    for x in x.members.iter().rev() {
                        let symbol = symbol_table::get(*x).unwrap();
                        let mut path = path.clone();
                        path.push(symbol.token.text);
                        if let SymbolKind::StructMember(x) = &symbol.kind
                            && let Some(mut r#type) = x.r#type.to_ir_type(context)
                        {
                            r#type.array = array.clone();
                            let (mut types, off) = r#type.expand_inner(context, &path, offset);
                            offset += off;
                            ret.append(&mut types);
                        }
                    }
                }
                SymbolKind::Interface(x) => {
                    for x in x.members.iter().rev() {
                        let symbol = symbol_table::get(*x).unwrap();
                        let mut path = path.clone();
                        path.push(symbol.token.text);
                        if let SymbolKind::Variable(x) = &symbol.kind
                            && let Some(mut r#type) = x.r#type.to_ir_type(context)
                        {
                            r#type.array = array.clone();
                            let (mut types, off) = r#type.expand_inner(context, &path, offset);
                            offset += off;
                            ret.append(&mut types);
                        }
                    }
                }
                _ => {
                    return (
                        vec![ExpandedType::from_type(self, path, offset)],
                        self.total_width(),
                    );
                }
            }
        } else {
            return (
                vec![ExpandedType::from_type(self, path, offset)],
                self.total_width(),
            );
        }

        (ret, offset)
    }

    pub fn modport_members(&self, base: &VarPath) -> HashMap<StrId, (VarPath, Direction)> {
        let mut ret = HashMap::default();
        if let TypeKind::UserDefined(x) = &self.kind {
            let symbol = symbol_table::get(*x).unwrap();
            if let SymbolKind::Modport(x) = &symbol.kind {
                for x in &x.members {
                    let symbol = symbol_table::get(*x).unwrap();
                    if let SymbolKind::ModportVariableMember(x) = &symbol.kind {
                        let symbol = symbol_table::get(x.variable).unwrap();
                        let name = symbol.token.text;
                        let mut path = VarPath::new(name);
                        path.add_prelude(&base.0);
                        ret.insert(name, (path, x.direction));
                    }
                }
            }
        }
        ret
    }
}

#[derive(Clone, Debug)]
pub struct ExpandedType {
    pub path: VarPath,
    pub r#type: Type,
    pub beg: usize,
    pub end: usize,
}

impl ExpandedType {
    pub fn from_type(value: &Type, path: &VarPath, offset: usize) -> ExpandedType {
        let end = offset;
        let beg = end + value.total_width() - 1;
        ExpandedType {
            path: path.clone(),
            r#type: value.clone(),
            beg,
            end,
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

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
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
    UserDefined(SymbolId),
    Type,
    Unknown,
}

impl TypeKind {
    pub fn symbol(&self) -> Option<Symbol> {
        if let TypeKind::UserDefined(x) = self {
            symbol_table::get(*x)
        } else {
            None
        }
    }
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
            TypeKind::UserDefined(x) => {
                let symbol = symbol_table::get(*x).unwrap();
                symbol.token.text.to_string().fmt(f)
            }
            TypeKind::Type => "type".fmt(f),
            TypeKind::Unknown => "unknown".fmt(f),
        }
    }
}
