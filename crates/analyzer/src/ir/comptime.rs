use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::conv::utils::get_component;
use crate::ir::{
    Component, Expression, IrResult, Op, Shape, ShapeRef, Signature, VarIndex, VarPath, VarSelect,
    VarSelectOp,
};
use crate::ir_error;
use crate::literal::TypeLiteral;
use crate::symbol::ClockDomain;
use crate::symbol::{Direction, SymbolId};
use crate::value::Value;
use num_bigint::BigUint;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Default, Debug)]
pub struct PartSelect {
    pub pos: usize,
    pub r#type: Type,
}

impl fmt::Display for PartSelect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format!("{}: {}", self.pos, self.r#type).fmt(f)
    }
}

#[derive(Clone, Debug)]
pub struct PartSelectPath {
    pub base: Type,
    pub path: VarPath,
    pub part_select: Vec<PartSelect>,
}

impl PartSelectPath {
    pub fn to_base_select(&self, mut select: &[usize]) -> Option<(usize, usize)> {
        let mut beg = 0;
        let mut end = 0;
        let len = self.part_select.len();
        for (i, x) in self.part_select.iter().enumerate() {
            let has_sub_part = i != (len - 1);
            let dims = x.r#type.width.dims();
            let is_select = !select.is_empty();

            if has_sub_part && dims > select.len() {
                return None;
            }

            let index = if is_select {
                let sel = &select[0..dims];
                select = &select[dims..];
                x.r#type.width.calc_index(sel)?
            } else {
                0
            };
            let width = if is_select {
                x.r#type.kind.width()?
            } else {
                x.r#type.total_width()?
            };
            end += x.pos + width * index;
            beg = end + width - 1;
        }
        Some((beg, end))
    }

    pub fn conv_width_select(
        &self,
        context: &mut Context,
        width_select: VarSelect,
    ) -> Option<VarSelect> {
        let (mut width_select, part_width_select) = width_select.split(self.base.width.dims());
        let part_width_select = self.to_base_select_expr(context, &part_width_select)?;
        width_select.append(part_width_select);
        Some(width_select)
    }

    pub fn to_base_select_expr(
        &self,
        context: &mut Context,
        select: &VarSelect,
    ) -> Option<VarSelect> {
        let mut ret: Option<(Expression, Expression)> = None;
        let len = self.part_select.len();
        let mut select = select.0.as_slice();
        let token = TokenRange::default();

        for (i, x) in self.part_select.iter().enumerate() {
            let has_sub_part = i != (len - 1);
            let dims = x.r#type.width.dims();
            let is_select = !select.is_empty();

            if has_sub_part && dims > select.len() {
                return None;
            }

            let index = if is_select {
                let sel = &select[0..dims];
                select = &select[dims..];
                x.r#type.width.calc_index_expr(sel)?
            } else {
                Expression::create_value(0u32.into(), 32, TokenRange::default())
            };
            let width = if is_select {
                x.r#type.kind.width()?
            } else {
                x.r#type.total_width()?
            };

            // end += x.pos + width * index;
            // beg = end + width - 1;
            let pos = Expression::create_value(x.pos.into(), 32, token);
            let one = Expression::create_value(1u32.into(), 32, token);
            let minus_one = Expression::Unary(Op::Sub, Box::new(one));
            let width = Expression::create_value(width.into(), 32, token);
            let end = Expression::Binary(Box::new(width.clone()), Op::Mul, Box::new(index));
            let mut end = Expression::Binary(Box::new(pos), Op::Add, Box::new(end));
            if let Some(y) = ret {
                let mut end = Expression::Binary(Box::new(y.1), Op::Add, Box::new(end));
                let beg = Expression::Binary(Box::new(end.clone()), Op::Add, Box::new(width));
                let mut beg = Expression::Binary(Box::new(beg), Op::Add, Box::new(minus_one));
                beg.eval_comptime(context, None);
                end.eval_comptime(context, None);
                ret = Some((beg, end));
            } else {
                let beg = Expression::Binary(Box::new(end.clone()), Op::Add, Box::new(width));
                let mut beg = Expression::Binary(Box::new(beg), Op::Add, Box::new(minus_one));
                beg.eval_comptime(context, None);
                end.eval_comptime(context, None);
                ret = Some((beg, end));
            }
        }

        if let Some((beg, end)) = ret {
            Some(VarSelect(vec![beg], Some((VarSelectOp::Colon, end))))
        } else {
            None
        }
    }
}

impl fmt::Display for PartSelectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("{}: ", self.path);

        for x in &self.part_select {
            ret.push_str(&format!("[{x}]"));
        }

        ret.fmt(f)
    }
}

#[derive(Clone, Default, Debug)]
pub struct Comptime {
    pub value: ValueVariant,
    pub r#type: Type,
    pub is_const: bool,
    pub is_global: bool,
    pub part_select: Option<PartSelectPath>,
    pub clock_domain: ClockDomain,
    pub token: TokenRange,
}

impl Comptime {
    pub fn create_unknown(clock_domain: ClockDomain, token: TokenRange) -> Self {
        Self {
            value: ValueVariant::Unknown,
            r#type: Type {
                kind: TypeKind::Unknown,
                ..Default::default()
            },
            clock_domain,
            token,
            ..Default::default()
        }
    }

    pub fn create_value(value: BigUint, width: usize, token: TokenRange) -> Self {
        let value = Value::new(value, width, false);
        Self {
            value: ValueVariant::Numeric(value),
            r#type: Type {
                kind: TypeKind::Bit,
                width: Shape::new(vec![Some(width)]),
                ..Default::default()
            },
            is_const: true,
            is_global: true,
            token,
            ..Default::default()
        }
    }

    pub fn from_type(r#type: Type, clock_domain: ClockDomain, token: TokenRange) -> Self {
        Self {
            r#type,
            clock_domain,
            token,
            ..Default::default()
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
            ..Default::default()
        };
        self.is_const = false;
    }

    pub fn invalid_logical_operand(&mut self, context: &mut Context, range: &TokenRange) {
        context.insert_error(AnalyzerError::invalid_logical_operand(true, range));
        self.value = ValueVariant::Unknown;
        self.r#type = Type {
            kind: TypeKind::Unknown,
            ..Default::default()
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
            ..Default::default()
        };
        self.is_const = false;
    }
}

#[derive(Clone, Default, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueVariant {
    Numeric(Value),
    NumericArray(Vec<Value>),
    Type(Type),
    #[default]
    Unknown,
}

#[derive(Clone, Default, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Type {
    pub kind: TypeKind,
    pub signed: bool,
    pub array: Shape,
    pub width: Shape,
}

impl Type {
    pub fn create_unknown() -> Type {
        Type {
            kind: TypeKind::Unknown,
            ..Default::default()
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
        self.total_width() == Some(1)
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

    pub fn is_string(&self) -> bool {
        matches!(&self.kind, TypeKind::String)
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

    pub fn total_width(&self) -> Option<usize> {
        Some(self.kind.width()? * self.width.total()?)
    }

    pub fn total_array(&self) -> Option<usize> {
        self.array.total()
    }

    pub fn compatible(&self, src: &Comptime) -> bool {
        // TODO type compatible check
        if self.is_unknown() | src.r#type.is_unknown() {
            true
        } else if let Some(mut dst_sig) = self.kind.signature() {
            dst_sig.parameters.clear();
            if let Some(mut src_sig) = src.r#type.kind.signature() {
                src_sig.parameters.clear();
                dst_sig.to_string() == src_sig.to_string()
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
            !src.r#type.is_clock() || src.is_const
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

    pub fn expand_struct_union(
        &self,
        path: &VarPath,
        part_select: &[PartSelect],
        base: Option<&Type>,
    ) -> Vec<PartSelectPath> {
        let base = if base.is_some() { base } else { Some(self) };
        match &self.kind {
            TypeKind::Struct(x) => x.expand_struct_union(path, part_select, base),
            TypeKind::Union(x) => x.expand_struct_union(path, part_select, base),
            _ => vec![],
        }
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

    pub fn prepend_array(&mut self, array: &ShapeRef) {
        if !array.is_empty() {
            let mut array = array.to_owned();
            array.append(&mut self.array);
            self.array = array;
        }
    }

    pub fn selected_dimension(&self, index: &VarIndex, select: &VarSelect) -> (usize, usize) {
        let array_dim = self.array.dims();
        let width_dim = self.width.dims();

        let array_dim = array_dim.saturating_sub(index.dimension());
        let width_dim = width_dim.saturating_sub(select.dimension());

        if self.total_width() == Some(1) {
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
            | TypeLiteral::String => Shape::new(vec![Some(1)]),
            TypeLiteral::F32 => Shape::new(vec![Some(32)]),
            TypeLiteral::F64 => Shape::new(vec![Some(64)]),
            TypeLiteral::I8 => Shape::new(vec![Some(8)]),
            TypeLiteral::I16 => Shape::new(vec![Some(16)]),
            TypeLiteral::I32 => Shape::new(vec![Some(32)]),
            TypeLiteral::I64 => Shape::new(vec![Some(64)]),
            TypeLiteral::U8 => Shape::new(vec![Some(8)]),
            TypeLiteral::U16 => Shape::new(vec![Some(16)]),
            TypeLiteral::U32 => Shape::new(vec![Some(32)]),
            TypeLiteral::U64 => Shape::new(vec![Some(64)]),
        };

        Type {
            kind,
            signed,
            width,
            ..Default::default()
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
        let width = value.total_width().unwrap_or(1);
        let end = offset;
        let beg = (end + width).saturating_sub(1);
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
            ret.push_str(&format!("<{}>", self.width));
        }

        if !self.array.is_empty() {
            ret.push_str(&format!("[{}]", self.array));
        }

        ret.fmt(f)
    }
}

#[derive(Clone, Default, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
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
    Union(TypeKindUnion),
    Enum(TypeKindEnum),
    Interface(Signature),
    Modport(Signature, StrId),
    AbstractInterface(Option<StrId>),
    Type,
    String,
    SystemVerilog,
    #[default]
    Unknown,
}

impl TypeKind {
    pub fn width(&self) -> Option<usize> {
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
            | TypeKind::AbstractInterface(_) => Some(1),
            TypeKind::Union(x) => x.width(),
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
            TypeKind::Union(x) => x.fmt(f),
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
    pub fn width(&self) -> Option<usize> {
        let mut ret = 0;
        for x in &self.members {
            ret += x.width()?;
        }
        Some(ret)
    }

    pub fn expand_struct(&self, path: &VarPath, array: &ShapeRef) -> Vec<ExpandedType> {
        let mut ret = vec![];
        let mut offset = 0;
        for x in self.members.iter().rev() {
            let width = x.width().unwrap_or(1);
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

    pub fn expand_struct_union(
        &self,
        path: &VarPath,
        part_select: &[PartSelect],
        base: Option<&Type>,
    ) -> Vec<PartSelectPath> {
        let mut ret = vec![];
        let mut offset = 0;
        for x in self.members.iter().rev() {
            let width = x.width().unwrap_or(1);
            let x = x.expand_struct_union(path, part_select, base);
            for mut x in x.into_iter().rev() {
                if let Some(x) = x.part_select.get_mut(part_select.len()) {
                    x.pos += offset;
                }
                ret.push(x);
            }
            offset += width;
        }
        ret.reverse();
        ret
    }

    pub fn expand_struct_middle(&self, path: &VarPath, array: &ShapeRef) -> Vec<(VarPath, Type)> {
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
pub struct TypeKindUnion {
    pub id: SymbolId,
    pub members: Vec<TypeKindMember>,
}

impl TypeKindUnion {
    pub fn width(&self) -> Option<usize> {
        self.members.first()?.width()
    }

    pub fn expand_struct_union(
        &self,
        path: &VarPath,
        part_select: &[PartSelect],
        base: Option<&Type>,
    ) -> Vec<PartSelectPath> {
        let mut ret = vec![];
        for x in &self.members {
            ret.append(&mut x.expand_struct_union(path, part_select, base));
        }
        ret
    }

    pub fn expand_union(&self, path: &VarPath, array: &ShapeRef) -> Vec<(VarPath, Type)> {
        let mut ret = vec![];
        for x in &self.members {
            ret.append(&mut x.expand_union(path, array));
        }
        ret
    }
}

impl fmt::Display for TypeKindUnion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = String::new();
        for x in &self.members {
            text.push_str(&format!(", {x}"));
        }
        let text = if text.is_empty() { &text } else { &text[2..] };

        format!("union {{{}}}", text).fmt(f)
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeKindMember {
    pub name: StrId,
    pub r#type: Type,
}

impl TypeKindMember {
    pub fn width(&self) -> Option<usize> {
        self.r#type.total_width()
    }

    pub fn expand_struct(&self, path: &VarPath, array: &ShapeRef) -> Vec<ExpandedType> {
        let mut path = path.clone();
        path.push(self.name);
        let mut ret = self.r#type.expand_struct(&path);
        for x in &mut ret {
            x.r#type.array = array.to_owned();
        }
        ret
    }

    pub fn expand_struct_union(
        &self,
        path: &VarPath,
        part_select: &[PartSelect],
        base: Option<&Type>,
    ) -> Vec<PartSelectPath> {
        let mut path = path.clone();
        path.push(self.name);

        let mut part_select = part_select.to_vec();
        part_select.push(PartSelect {
            pos: 0,
            r#type: self.r#type.clone(),
        });

        let mut ret = self.r#type.expand_struct_union(&path, &part_select, base);

        ret.push(PartSelectPath {
            base: base.unwrap().clone(),
            path: path.clone(),
            part_select,
        });

        ret
    }

    pub fn expand_struct_middle(&self, path: &VarPath, array: &ShapeRef) -> Vec<(VarPath, Type)> {
        let mut path = path.clone();
        path.push(self.name);
        let mut ret = self.r#type.expand_struct_middle(&path);
        for x in &mut ret {
            x.1.array = array.to_owned();
        }
        ret
    }

    pub fn expand_union(&self, path: &VarPath, array: &ShapeRef) -> Vec<(VarPath, Type)> {
        let mut path = path.clone();
        path.push(self.name);
        let mut r#type = self.r#type.clone();
        r#type.array = array.to_owned();
        vec![(path, r#type)]
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
    pub fn width(&self) -> Option<usize> {
        self.r#type.total_width()
    }
}

impl fmt::Display for TypeKindEnum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format!("enum {{{}}}", self.r#type).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veryl_parser::resource_table;

    pub fn create_logic(width: usize) -> Type {
        Type {
            kind: TypeKind::Logic,
            array: Shape::default(),
            width: Shape::new(vec![Some(width)]),
            signed: false,
        }
    }

    pub fn create_struct(members: &[(&'static str, Type)], width: usize) -> Type {
        let members = members
            .into_iter()
            .map(|(n, t)| TypeKindMember {
                name: resource_table::insert_str(n),
                r#type: t.clone(),
            })
            .collect();
        Type {
            kind: TypeKind::Struct(TypeKindStruct {
                id: SymbolId::default(),
                members,
            }),
            array: Shape::default(),
            width: Shape::new(vec![Some(width)]),
            signed: false,
        }
    }

    pub fn create_union(members: &[(&'static str, Type)], width: usize) -> Type {
        let members = members
            .into_iter()
            .map(|(n, t)| TypeKindMember {
                name: resource_table::insert_str(n),
                r#type: t.clone(),
            })
            .collect();
        Type {
            kind: TypeKind::Union(TypeKindUnion {
                id: SymbolId::default(),
                members,
            }),
            array: Shape::default(),
            width: Shape::new(vec![Some(width)]),
            signed: false,
        }
    }

    #[test]
    fn expand_struct() {
        // struct x0 {
        //   a: logic<2>,
        //   b: logic<3>,
        // }
        // struct x1 {
        //   c: logic<5>,
        //   d: logic<6>,
        // }
        // struct x2 {
        //   e: x0<4>,
        //   f: x1<7>,
        // }
        let x0 = create_struct(&[("a", create_logic(2)), ("b", create_logic(3))], 4);
        let x1 = create_struct(&[("c", create_logic(5)), ("d", create_logic(6))], 7);
        let x2 = create_struct(&[("e", x0.clone()), ("f", x1.clone())], 8);

        let path = VarPath::new(resource_table::insert_str("x"));

        let x0 = x0.expand_struct_union(&path, &[], None);
        let x1 = x1.expand_struct_union(&path, &[], None);
        let x2 = x2.expand_struct_union(&path, &[], None);

        assert_eq!(x0[0].to_string(), "x.a: [3: logic<2>]");
        assert_eq!(x0[1].to_string(), "x.b: [0: logic<3>]");
        assert_eq!(x1[0].to_string(), "x.c: [6: logic<5>]");
        assert_eq!(x1[1].to_string(), "x.d: [0: logic<6>]");
        assert_eq!(
            x2[0].to_string(),
            "x.e.a: [77: struct {a: logic<2>, b: logic<3>}<4>][3: logic<2>]"
        );
        assert_eq!(
            x2[1].to_string(),
            "x.e.b: [77: struct {a: logic<2>, b: logic<3>}<4>][0: logic<3>]"
        );
        assert_eq!(
            x2[2].to_string(),
            "x.e: [77: struct {a: logic<2>, b: logic<3>}<4>]"
        );
        assert_eq!(
            x2[3].to_string(),
            "x.f.c: [0: struct {c: logic<5>, d: logic<6>}<7>][6: logic<5>]"
        );
        assert_eq!(
            x2[4].to_string(),
            "x.f.d: [0: struct {c: logic<5>, d: logic<6>}<7>][0: logic<6>]"
        );
        assert_eq!(
            x2[5].to_string(),
            "x.f: [0: struct {c: logic<5>, d: logic<6>}<7>]"
        );

        assert_eq!(x0[0].to_base_select(&[]), Some((4, 3)));
        assert_eq!(x0[1].to_base_select(&[]), Some((2, 0)));
        assert_eq!(x1[0].to_base_select(&[]), Some((10, 6)));
        assert_eq!(x1[1].to_base_select(&[]), Some((5, 0)));
        assert_eq!(x2[0].to_base_select(&[]), None);
        assert_eq!(x2[0].to_base_select(&[0]), Some((81, 80)));
        assert_eq!(x2[0].to_base_select(&[1]), Some((86, 85)));
        assert_eq!(x2[0].to_base_select(&[2]), Some((91, 90)));
        assert_eq!(x2[0].to_base_select(&[3]), Some((96, 95)));
        assert_eq!(x2[1].to_base_select(&[]), None);
        assert_eq!(x2[1].to_base_select(&[0]), Some((79, 77)));
        assert_eq!(x2[1].to_base_select(&[1]), Some((84, 82)));
        assert_eq!(x2[1].to_base_select(&[2]), Some((89, 87)));
        assert_eq!(x2[1].to_base_select(&[3]), Some((94, 92)));
        assert_eq!(x2[2].to_base_select(&[]), Some((96, 77)));
        assert_eq!(x2[2].to_base_select(&[0]), Some((81, 77)));
        assert_eq!(x2[2].to_base_select(&[1]), Some((86, 82)));
        assert_eq!(x2[2].to_base_select(&[2]), Some((91, 87)));
        assert_eq!(x2[2].to_base_select(&[3]), Some((96, 92)));
        assert_eq!(x2[3].to_base_select(&[]), None);
        assert_eq!(x2[3].to_base_select(&[0]), Some((10, 6)));
        assert_eq!(x2[3].to_base_select(&[1]), Some((21, 17)));
        assert_eq!(x2[3].to_base_select(&[2]), Some((32, 28)));
        assert_eq!(x2[3].to_base_select(&[3]), Some((43, 39)));
        assert_eq!(x2[3].to_base_select(&[4]), Some((54, 50)));
        assert_eq!(x2[3].to_base_select(&[5]), Some((65, 61)));
        assert_eq!(x2[3].to_base_select(&[6]), Some((76, 72)));
        assert_eq!(x2[4].to_base_select(&[]), None);
        assert_eq!(x2[4].to_base_select(&[0]), Some((5, 0)));
        assert_eq!(x2[4].to_base_select(&[1]), Some((16, 11)));
        assert_eq!(x2[4].to_base_select(&[2]), Some((27, 22)));
        assert_eq!(x2[4].to_base_select(&[3]), Some((38, 33)));
        assert_eq!(x2[4].to_base_select(&[4]), Some((49, 44)));
        assert_eq!(x2[4].to_base_select(&[5]), Some((60, 55)));
        assert_eq!(x2[4].to_base_select(&[6]), Some((71, 66)));
        assert_eq!(x2[5].to_base_select(&[]), Some((76, 0)));
        assert_eq!(x2[5].to_base_select(&[0]), Some((10, 0)));
        assert_eq!(x2[5].to_base_select(&[1]), Some((21, 11)));
        assert_eq!(x2[5].to_base_select(&[2]), Some((32, 22)));
        assert_eq!(x2[5].to_base_select(&[3]), Some((43, 33)));
        assert_eq!(x2[5].to_base_select(&[4]), Some((54, 44)));
        assert_eq!(x2[5].to_base_select(&[5]), Some((65, 55)));
        assert_eq!(x2[5].to_base_select(&[6]), Some((76, 66)));
    }

    #[test]
    fn expand_union() {
        // struct x0 {
        //   a: logic<2>,
        //   b: logic<3>,
        // }
        // union x1 {
        //   e: x0<2>,
        //   f: logic<10>,
        // }
        // struct x2 {
        //   g: x1<2>,
        //   h: logic<5>,
        // }
        let x0 = create_struct(&[("a", create_logic(2)), ("b", create_logic(3))], 2);
        let x1 = create_union(&[("e", x0.clone()), ("f", create_logic(10))], 2);
        let x2 = create_struct(&[("g", x1.clone()), ("h", create_logic(5))], 2);

        let path = VarPath::new(resource_table::insert_str("x"));

        let x0 = x0.expand_struct_union(&path, &[], None);
        let x1 = x1.expand_struct_union(&path, &[], None);
        let x2 = x2.expand_struct_union(&path, &[], None);

        assert_eq!(x0[0].to_string(), "x.a: [3: logic<2>]");
        assert_eq!(x0[1].to_string(), "x.b: [0: logic<3>]");
        assert_eq!(
            x1[0].to_string(),
            "x.e.a: [0: struct {a: logic<2>, b: logic<3>}<2>][3: logic<2>]"
        );
        assert_eq!(
            x1[1].to_string(),
            "x.e.b: [0: struct {a: logic<2>, b: logic<3>}<2>][0: logic<3>]"
        );
        assert_eq!(
            x1[2].to_string(),
            "x.e: [0: struct {a: logic<2>, b: logic<3>}<2>]"
        );
        assert_eq!(x1[3].to_string(), "x.f: [0: logic<10>]");
        assert_eq!(
            x2[0].to_string(),
            "x.g.e.a: [5: union {e: struct {a: logic<2>, b: logic<3>}<2>, f: logic<10>}<2>][0: struct {a: logic<2>, b: logic<3>}<2>][3: logic<2>]"
        );
        assert_eq!(
            x2[1].to_string(),
            "x.g.e.b: [5: union {e: struct {a: logic<2>, b: logic<3>}<2>, f: logic<10>}<2>][0: struct {a: logic<2>, b: logic<3>}<2>][0: logic<3>]"
        );
        assert_eq!(
            x2[2].to_string(),
            "x.g.e: [5: union {e: struct {a: logic<2>, b: logic<3>}<2>, f: logic<10>}<2>][0: struct {a: logic<2>, b: logic<3>}<2>]"
        );
        assert_eq!(
            x2[3].to_string(),
            "x.g.f: [5: union {e: struct {a: logic<2>, b: logic<3>}<2>, f: logic<10>}<2>][0: logic<10>]"
        );
        assert_eq!(
            x2[4].to_string(),
            "x.g: [5: union {e: struct {a: logic<2>, b: logic<3>}<2>, f: logic<10>}<2>]"
        );
        assert_eq!(x2[5].to_string(), "x.h: [0: logic<5>]");

        assert_eq!(x0[0].to_base_select(&[]), Some((4, 3)));
        assert_eq!(x0[1].to_base_select(&[]), Some((2, 0)));
        assert_eq!(x1[0].to_base_select(&[]), None);
        assert_eq!(x1[0].to_base_select(&[0]), Some((4, 3)));
        assert_eq!(x1[0].to_base_select(&[1]), Some((9, 8)));
        assert_eq!(x1[1].to_base_select(&[]), None);
        assert_eq!(x1[1].to_base_select(&[0]), Some((2, 0)));
        assert_eq!(x1[1].to_base_select(&[1]), Some((7, 5)));
        assert_eq!(x1[2].to_base_select(&[]), Some((9, 0)));
        assert_eq!(x1[2].to_base_select(&[0]), Some((4, 0)));
        assert_eq!(x1[2].to_base_select(&[1]), Some((9, 5)));
        assert_eq!(x1[3].to_base_select(&[]), Some((9, 0)));
        assert_eq!(x2[0].to_base_select(&[]), None);
        assert_eq!(x2[0].to_base_select(&[0]), None);
        assert_eq!(x2[0].to_base_select(&[0, 0]), Some((9, 8)));
        assert_eq!(x2[0].to_base_select(&[0, 1]), Some((14, 13)));
        assert_eq!(x2[0].to_base_select(&[1]), None);
        assert_eq!(x2[0].to_base_select(&[1, 0]), Some((19, 18)));
        assert_eq!(x2[0].to_base_select(&[1, 1]), Some((24, 23)));
        assert_eq!(x2[1].to_base_select(&[]), None);
        assert_eq!(x2[1].to_base_select(&[0]), None);
        assert_eq!(x2[1].to_base_select(&[0, 0]), Some((7, 5)));
        assert_eq!(x2[1].to_base_select(&[0, 1]), Some((12, 10)));
        assert_eq!(x2[1].to_base_select(&[1]), None);
        assert_eq!(x2[1].to_base_select(&[1, 0]), Some((17, 15)));
        assert_eq!(x2[1].to_base_select(&[1, 1]), Some((22, 20)));
        assert_eq!(x2[2].to_base_select(&[]), None);
        assert_eq!(x2[2].to_base_select(&[0]), Some((14, 5)));
        assert_eq!(x2[2].to_base_select(&[0, 0]), Some((9, 5)));
        assert_eq!(x2[2].to_base_select(&[0, 1]), Some((14, 10)));
        assert_eq!(x2[2].to_base_select(&[1]), Some((24, 15)));
        assert_eq!(x2[2].to_base_select(&[1, 0]), Some((19, 15)));
        assert_eq!(x2[2].to_base_select(&[1, 1]), Some((24, 20)));
        assert_eq!(x2[3].to_base_select(&[]), None);
        assert_eq!(x2[3].to_base_select(&[0]), Some((14, 5)));
        assert_eq!(x2[3].to_base_select(&[1]), Some((24, 15)));
        assert_eq!(x2[4].to_base_select(&[]), Some((24, 5)));
        assert_eq!(x2[4].to_base_select(&[0]), Some((14, 5)));
        assert_eq!(x2[4].to_base_select(&[1]), Some((24, 15)));
        assert_eq!(x2[5].to_base_select(&[]), Some((4, 0)));
    }
}
