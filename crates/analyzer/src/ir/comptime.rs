use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::conv::utils::get_component;
use crate::ir::{Component, IrResult, Op, Signature, VarIndex, VarPath, VarSelect};
use crate::ir_error;
use crate::literal::TypeLiteral;
use crate::symbol::ClockDomain;
use crate::symbol::{Direction, SymbolId};
use crate::value::Value;
use itertools::join;
use num_bigint::BigUint;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug)]
pub struct Comptime {
    pub value: ValueVariant,
    pub r#type: Type,
    pub is_const: bool,
    pub is_global: bool,
    pub clock_domain: ClockDomain,
    pub token: TokenRange,
}

impl Comptime {
    pub fn create_unknown(clock_domain: ClockDomain, token: TokenRange) -> Self {
        Self {
            value: ValueVariant::Unknown,
            r#type: Type {
                kind: TypeKind::Unknown,
                signed: false,
                width: vec![],
                array: vec![],
            },
            is_const: false,
            is_global: false,
            clock_domain,
            token,
        }
    }

    pub fn create_value(
        value: BigUint,
        width: usize,
        clock_domain: ClockDomain,
        token: TokenRange,
    ) -> Self {
        let value = Value::new(value, width, false);
        Self {
            value: ValueVariant::Numeric(value),
            r#type: Type {
                kind: TypeKind::Bit,
                signed: false,
                width: vec![width],
                array: vec![],
            },
            is_const: true,
            is_global: true,
            clock_domain,
            token,
        }
    }

    pub fn from_type(r#type: Type, clock_domain: ClockDomain, token: TokenRange) -> Self {
        Self {
            value: ValueVariant::Unknown,
            r#type,
            is_const: false,
            is_global: false,
            clock_domain,
            token,
        }
    }

    pub fn get_value(&self) -> IrResult<Value> {
        if let ValueVariant::Numeric(x) = &self.value {
            Ok(x.clone())
        } else {
            Err(ir_error!(self.token))
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

    pub fn invalid_cast(
        &mut self,
        context: &mut Context,
        dst: &Type,
        src: &Type,
        range: &TokenRange,
    ) {
        context.insert_error(AnalyzerError::invalid_cast(
            &src.to_string(),
            &dst.to_string(),
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
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueVariant {
    Numeric(Value),
    NumericArray(Vec<Value>),
    Type(Type),
    String(String),
    Unknown,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Type {
    pub kind: TypeKind,
    pub signed: bool,
    pub array: Vec<usize>,
    pub width: Vec<usize>,
}

impl Type {
    pub fn new(kind: TypeKind, array: Vec<usize>, width: Vec<usize>, signed: bool) -> Type {
        Type {
            kind,
            signed,
            array,
            width,
        }
    }

    pub fn create_unknown() -> Type {
        Type {
            kind: TypeKind::Unknown,
            signed: false,
            array: vec![],
            width: vec![],
        }
    }

    pub fn is_4state(&self) -> bool {
        if self.is_struct() {
            self.expand_struct(&VarPath::default())
                .iter()
                .any(|x| x.r#type.is_4state())
        } else {
            matches!(self.kind, TypeKind::Logic) | self.is_clock() | self.is_reset()
        }
    }

    pub fn is_2state(&self) -> bool {
        if self.is_struct() {
            self.expand_struct(&VarPath::default())
                .iter()
                .all(|x| x.r#type.is_2state())
        } else {
            matches!(self.kind, TypeKind::Bit)
        }
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

    pub fn is_explicit_reset(&self) -> bool {
        matches!(
            self.kind,
            TypeKind::ResetAsyncHigh
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

    pub fn is_array(&self) -> bool {
        !self.array.is_empty()
    }

    pub fn is_binary(&self) -> bool {
        self.total_width() == 1
    }

    pub fn is_struct(&self) -> bool {
        matches!(&self.kind, TypeKind::Struct(_))
    }

    pub fn is_interface(&self) -> bool {
        matches!(
            &self.kind,
            TypeKind::Interface(_) | TypeKind::Modport(_, _) | TypeKind::AbstractInterface(_)
        )
    }

    pub fn is_systemverilog(&self) -> bool {
        matches!(&self.kind, TypeKind::SystemVerilog)
    }

    pub fn get_member_type(&self, name: StrId) -> Option<Type> {
        if let TypeKind::Struct(x) = &self.kind {
            for x in &x.members {
                if x.name == name {
                    return Some(x.r#type.clone());
                }
            }
        }
        None
    }

    pub fn total_width(&self) -> usize {
        self.kind.width() * self.width.iter().product::<usize>()
    }

    pub fn total_array(&self) -> usize {
        if self.array.is_empty() {
            1
        } else {
            self.array.iter().product::<usize>()
        }
    }

    pub fn compatible(&self, src: &Comptime) -> bool {
        // TODO type compatible check
        if self.is_unknown() | src.r#type.is_unknown() {
            true
        } else if let Some(mut dst_sig) = self.kind.signature() {
            dst_sig.parameters.clear();
            if let Some(mut src_sig) = src.r#type.kind.signature() {
                src_sig.parameters.clear();
                dst_sig == src_sig
            } else {
                false
            }
        } else if self.is_type() || src.r#type.is_type() {
            self.is_type() && src.r#type.is_type()
        } else if self.is_2state() {
            src.r#type.is_2state()
        } else if self.is_array() || src.r#type.is_array() {
            self.array == src.r#type.array
        } else if self.is_clock() {
            src.r#type.is_clock() || src.is_const
        } else if self.is_reset() {
            src.r#type.is_reset() || src.is_const
        } else {
            // TODO width array check
            true
        }
    }

    pub fn expand_struct(&self, path: &VarPath) -> Vec<ExpandedType> {
        if let TypeKind::Struct(x) = &self.kind {
            let mut array = self.array.clone();
            array.append(&mut self.width.clone());
            x.expand_struct(path, &array)
        } else {
            vec![ExpandedType::from_type(self, path, 0)]
        }
    }

    pub fn expand_struct_middle(&self, path: &VarPath) -> Vec<(VarPath, Type)> {
        let mut ret = vec![];
        if let TypeKind::Struct(x) = &self.kind {
            ret.push((path.clone(), self.clone()));
            let mut array = self.array.clone();
            array.append(&mut self.width.clone());
            ret.append(&mut x.expand_struct_middle(path, &array));
        }
        ret
    }

    pub fn expand_interface(
        &self,
        context: &mut Context,
        path: &VarPath,
        token: TokenRange,
    ) -> IrResult<Vec<(VarPath, Type)>> {
        let mut ret = vec![];
        match &self.kind {
            TypeKind::Struct(_) => {
                for x in self.expand_struct(path) {
                    ret.push((x.path, x.r#type));
                }
            }
            TypeKind::Modport(sig, name) => {
                let component = get_component(context, sig, token)?;
                let Component::Interface(component) = component else {
                    unreachable!();
                };

                let modport_members = component.get_modport(name);

                let mut temp = vec![];
                for (id, variable) in &component.variables {
                    if modport_members.contains_key(&variable.path.first()) {
                        let mut member_path = variable.path.clone();
                        member_path.add_prelude(&path.0);
                        temp.push((id, member_path, variable.r#type.clone()));
                    }
                }
                temp.sort_by_key(|x| x.0);
                ret = temp.into_iter().map(|x| (x.1, x.2)).collect();
            }
            TypeKind::Interface(sig) => {
                let component = get_component(context, sig, token)?;
                let Component::Interface(component) = component else {
                    unreachable!();
                };
                let mut temp = vec![];
                for (id, variable) in &component.variables {
                    let mut member_path = variable.path.clone();
                    member_path.add_prelude(&path.0);
                    temp.push((id, member_path, variable.r#type.clone()));
                }
                temp.sort_by_key(|x| x.0);
                ret = temp.into_iter().map(|x| (x.1, x.2)).collect();
            }
            _ => (),
        }
        Ok(ret)
    }

    pub fn expand_modport(
        &self,
        context: &mut Context,
        path: &VarPath,
        token: TokenRange,
    ) -> IrResult<Vec<(VarPath, Direction)>> {
        let mut ret = vec![];
        if let TypeKind::Modport(sig, name) = &self.kind {
            let component = get_component(context, sig, token)?;
            let Component::Interface(component) = component else {
                unreachable!();
            };

            let modport_members = component.get_modport(name);

            let mut temp = vec![];
            for (id, variable) in &component.variables {
                if let Some(x) = modport_members.get(&variable.path.first()) {
                    let mut member_path = variable.path.clone();
                    member_path.add_prelude(&path.0);
                    temp.push((id, member_path, *x));
                }
            }
            temp.sort_by_key(|x| x.0);
            ret = temp.into_iter().map(|x| (x.1, x.2)).collect();
        }
        Ok(ret)
    }

    pub fn flatten_enum(&mut self) {
        if let TypeKind::Enum(x) = self.kind.clone() {
            self.kind = x.r#type.kind;
            self.signed = x.r#type.signed;
            let mut array = x.r#type.array;
            let mut width = x.r#type.width;
            array.append(&mut self.array);
            width.append(&mut self.width);
            self.array = array;
            self.width = width;
        }
    }

    pub fn prepend_array(&mut self, array: &[usize]) {
        if !array.is_empty() {
            let mut array = array.to_vec();
            array.append(&mut self.array);
            self.array = array;
        }
    }

    pub fn selected_dimension(&self, index: &VarIndex, select: &VarSelect) -> (usize, usize) {
        let array_dim = self.array.len();
        let width_dim = self.width.len();

        let array_dim = array_dim.saturating_sub(index.dimension());
        let width_dim = width_dim.saturating_sub(select.dimension());

        if self.total_width() == 1 {
            (array_dim, 0)
        } else {
            (array_dim, width_dim)
        }
    }
}

impl From<&TypeLiteral> for Type {
    fn from(value: &TypeLiteral) -> Self {
        let kind = match value {
            TypeLiteral::Bit => TypeKind::Bit,
            TypeLiteral::Bool => TypeKind::Logic,
            TypeLiteral::Clock => TypeKind::Clock,
            TypeLiteral::ClockPosedge => TypeKind::ClockPosedge,
            TypeLiteral::ClockNegedge => TypeKind::ClockNegedge,
            TypeLiteral::F32 => TypeKind::Bit,
            TypeLiteral::F64 => TypeKind::Bit,
            TypeLiteral::I8 => TypeKind::Bit,
            TypeLiteral::I16 => TypeKind::Bit,
            TypeLiteral::I32 => TypeKind::Bit,
            TypeLiteral::I64 => TypeKind::Bit,
            TypeLiteral::Logic => TypeKind::Logic,
            TypeLiteral::Reset => TypeKind::Reset,
            TypeLiteral::ResetAsyncHigh => TypeKind::ResetAsyncHigh,
            TypeLiteral::ResetAsyncLow => TypeKind::ResetAsyncLow,
            TypeLiteral::ResetSyncHigh => TypeKind::ResetSyncHigh,
            TypeLiteral::ResetSyncLow => TypeKind::ResetSyncLow,
            TypeLiteral::String => TypeKind::Unknown,
            TypeLiteral::U8 => TypeKind::Bit,
            TypeLiteral::U16 => TypeKind::Bit,
            TypeLiteral::U32 => TypeKind::Bit,
            TypeLiteral::U64 => TypeKind::Bit,
        };

        let signed = matches!(
            value,
            TypeLiteral::F32
                | TypeLiteral::F64
                | TypeLiteral::I8
                | TypeLiteral::I16
                | TypeLiteral::I32
                | TypeLiteral::I64
        );

        let width = match value {
            TypeLiteral::Bit
            | TypeLiteral::Bool
            | TypeLiteral::Clock
            | TypeLiteral::ClockPosedge
            | TypeLiteral::ClockNegedge
            | TypeLiteral::Logic
            | TypeLiteral::Reset
            | TypeLiteral::ResetAsyncHigh
            | TypeLiteral::ResetAsyncLow
            | TypeLiteral::ResetSyncHigh
            | TypeLiteral::ResetSyncLow
            | TypeLiteral::String => vec![1],
            TypeLiteral::F32 => vec![32],
            TypeLiteral::F64 => vec![64],
            TypeLiteral::I8 => vec![8],
            TypeLiteral::I16 => vec![16],
            TypeLiteral::I32 => vec![32],
            TypeLiteral::I64 => vec![64],
            TypeLiteral::U8 => vec![8],
            TypeLiteral::U16 => vec![16],
            TypeLiteral::U32 => vec![32],
            TypeLiteral::U64 => vec![64],
        };

        Type {
            kind,
            signed,
            width,
            array: vec![],
        }
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
        let beg = (end + value.total_width()).saturating_sub(1);
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
    Struct(TypeKindStruct),
    Enum(TypeKindEnum),
    Interface(Signature),
    Modport(Signature, StrId),
    AbstractInterface(Option<StrId>),
    Type,
    String,
    SystemVerilog,
    Unknown,
}

impl TypeKind {
    pub fn width(&self) -> usize {
        match self {
            TypeKind::Clock
            | TypeKind::ClockPosedge
            | TypeKind::ClockNegedge
            | TypeKind::Reset
            | TypeKind::ResetAsyncHigh
            | TypeKind::ResetAsyncLow
            | TypeKind::ResetSyncHigh
            | TypeKind::ResetSyncLow
            | TypeKind::Bit
            | TypeKind::Logic
            | TypeKind::Type
            | TypeKind::String
            | TypeKind::Unknown
            | TypeKind::SystemVerilog
            | TypeKind::Interface(_)
            | TypeKind::Modport(_, _)
            | TypeKind::AbstractInterface(_) => 1,
            TypeKind::Struct(x) => x.width(),
            TypeKind::Enum(x) => x.width(),
        }
    }

    pub fn signature(&self) -> Option<Signature> {
        match self {
            TypeKind::Interface(x) => Some(x.clone()),
            TypeKind::Modport(x, _) => Some(x.clone()),
            _ => None,
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
            TypeKind::Struct(x) => x.fmt(f),
            TypeKind::Enum(x) => x.fmt(f),
            TypeKind::Interface(x) => format!("interface {x}").fmt(f),
            TypeKind::Modport(x, _) => format!("modport {x}").fmt(f),
            TypeKind::AbstractInterface(x) => {
                if let Some(x) = x {
                    format!("interface::{x}").fmt(f)
                } else {
                    "interface".fmt(f)
                }
            }
            TypeKind::Type => "type".fmt(f),
            TypeKind::String => "string".fmt(f),
            TypeKind::SystemVerilog => "systemverilog".fmt(f),
            TypeKind::Unknown => "unknown".fmt(f),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeKindStruct {
    pub id: SymbolId,
    pub members: Vec<TypeKindMember>,
}

impl TypeKindStruct {
    pub fn width(&self) -> usize {
        self.members.iter().map(|x| x.width()).sum()
    }

    pub fn expand_struct(&self, path: &VarPath, array: &[usize]) -> Vec<ExpandedType> {
        let mut ret = vec![];
        let mut offset = 0;
        for x in self.members.iter().rev() {
            let width = x.width();
            let x = x.expand_struct(path, array);
            for mut x in x.into_iter().rev() {
                x.beg += offset;
                x.end += offset;
                ret.push(x);
            }
            offset += width;
        }
        ret.reverse();
        ret
    }

    pub fn expand_struct_middle(&self, path: &VarPath, array: &[usize]) -> Vec<(VarPath, Type)> {
        let mut ret = vec![];
        for x in &self.members {
            ret.append(&mut x.expand_struct_middle(path, array));
        }
        ret
    }
}

impl fmt::Display for TypeKindStruct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = String::new();
        for x in &self.members {
            text.push_str(&format!(", {x}"));
        }
        let text = if text.is_empty() { &text } else { &text[2..] };

        format!("struct {{{}}}", text).fmt(f)
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeKindMember {
    pub name: StrId,
    pub r#type: Type,
}

impl TypeKindMember {
    pub fn width(&self) -> usize {
        self.r#type.total_width()
    }

    pub fn expand_struct(&self, path: &VarPath, array: &[usize]) -> Vec<ExpandedType> {
        let mut path = path.clone();
        path.push(self.name);
        let mut ret = self.r#type.expand_struct(&path);
        for x in &mut ret {
            x.r#type.array = array.to_vec();
        }
        ret
    }

    pub fn expand_struct_middle(&self, path: &VarPath, array: &[usize]) -> Vec<(VarPath, Type)> {
        let mut path = path.clone();
        path.push(self.name);
        let mut ret = self.r#type.expand_struct_middle(&path);
        for x in &mut ret {
            x.1.array = array.to_vec();
        }
        ret
    }
}

impl fmt::Display for TypeKindMember {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format!("{}: {}", self.name, self.r#type).fmt(f)
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeKindEnum {
    pub id: SymbolId,
    pub r#type: Box<Type>,
}

impl TypeKindEnum {
    pub fn width(&self) -> usize {
        self.r#type.total_width()
    }
}

impl fmt::Display for TypeKindEnum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format!("enum {{{}}}", self.r#type).fmt(f)
    }
}
