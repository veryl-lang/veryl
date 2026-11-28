//! The description of the packages and interfaces marked `#[export(rust)]`
//! (see [`veryl_metadata::ExportDescription`]), built from the symbol table
//! after the analysis, where the rules on them are also checked.
//!
//! It reads the symbol table rather than the IR so that the packages of a
//! file restored from the incremental cache are described as well.

use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::conv::utils::{TypePosition, eval_expr};
use crate::ir::{self, ValueVariant};
use crate::post_pass1_scope;
use crate::symbol::SymbolId;
use crate::symbol::{Direction, EnumMemberValue, Symbol, SymbolKind, Type as SymType, TypeKind};
use crate::symbol_table;
use num_bigint::BigUint;
use veryl_metadata::export_description::{
    ExportConst, ExportDescription, ExportDirection, ExportEnum, ExportField, ExportFieldValue,
    ExportInterface, ExportMember, ExportModport, ExportPackage, ExportStruct, ExportType,
    ExportTypeAlias, ExportValue, ExportVariant,
};
use veryl_parser::token_range::TokenRange;

/// Describes the exported packages and interfaces of every project,
/// returning the breaches of the rules on them. The descriptions, by
/// project, are put into `output` when given.
pub fn describe(
    output: Option<&mut std::collections::HashMap<String, ExportDescription>>,
) -> Vec<AnalyzerError> {
    let mut errors = Vec::new();

    let mut symbols: Vec<_> = symbol_table::get_all()
        .into_iter()
        .filter(post_pass1_scope::is_exported_item)
        .collect();
    // The symbol table is a hash map: sort for a stable order of errors.
    symbols.sort_by_key(|x| x.id);

    let mut describers: Vec<Describer> = Vec::new();
    for symbol in &symbols {
        let project_name = symbol.namespace.to_string();
        let index = match describers
            .iter()
            .position(|x| x.project_name == project_name)
        {
            Some(x) => x,
            None => {
                describers.push(Describer::new(&project_name));
                describers.len() - 1
            }
        };
        let describer = &mut describers[index];
        match &symbol.kind {
            SymbolKind::Package(_) => describer.package(symbol),
            SymbolKind::Interface(_) => describer.interface(symbol),
            _ => unreachable!(),
        }
    }

    let mut output = output;
    for mut describer in describers {
        errors.append(&mut describer.errors);
        if let Some(output) = output.as_mut() {
            output.insert(describer.project_name, describer.description);
        }
    }
    errors
}

struct Describer {
    project_name: String,
    context: Context,
    description: ExportDescription,
    errors: Vec<AnalyzerError>,
}

impl Describer {
    fn new(project_name: &str) -> Self {
        let mut context = Context::default();
        context.set_project_name(project_name);
        Self {
            project_name: project_name.to_string(),
            context,
            description: ExportDescription::new(),
            errors: Vec::new(),
        }
    }

    fn error(&mut self, reason: impl AsRef<str>, token: &TokenRange) {
        self.errors
            .push(AnalyzerError::invalid_export(reason.as_ref(), token));
    }

    fn package(&mut self, symbol: &Symbol) {
        let SymbolKind::Package(property) = &symbol.kind else {
            unreachable!();
        };
        let token: TokenRange = symbol.token.into();
        if property.is_proto {
            self.error("cannot be attached to a proto package", &token);
            return;
        }
        if !property.generic_parameters.is_empty() {
            self.error(
                "package cannot have generic parameters: its types have to be fixed by itself",
                &token,
            );
            return;
        }

        self.context.push_namespace(symbol.inner_namespace());
        let mut package = ExportPackage::default();
        for member in &property.members {
            let Some(member) = symbol_table::get(*member) else {
                continue;
            };
            let name = member.token.to_string();
            let member_token: TokenRange = member.token.into();
            match &member.kind {
                SymbolKind::Parameter(x) => {
                    if x.r#type.kind.is_type() {
                        if let Some(r#type) = self.type_value(x.value.as_ref(), &member_token) {
                            package.types.push(ExportTypeAlias { name, r#type });
                        }
                    } else if let Some((r#type, value)) =
                        self.const_value(&x.r#type, x.value.as_ref(), &member_token)
                    {
                        package.consts.push(ExportConst {
                            name,
                            r#type,
                            value,
                        });
                    }
                }
                SymbolKind::TypeDef(x) => {
                    if let Some(r#type) = &x.r#type
                        && let Some(r#type) = self.data_type(r#type, false)
                    {
                        package.types.push(ExportTypeAlias { name, r#type });
                    }
                }
                SymbolKind::Enum(_) => {
                    if let Some(x) = self.enum_declaration(&member) {
                        package.enums.push(x);
                    }
                }
                SymbolKind::Struct(x) => {
                    if !x.generic_parameters.is_empty() {
                        self.error("struct cannot have generic parameters", &member_token);
                        continue;
                    }
                    let mut fields = Vec::new();
                    for field in &x.members {
                        let Some(field) = symbol_table::get(*field) else {
                            continue;
                        };
                        if let SymbolKind::StructMember(x) = &field.kind
                            && let Some(r#type) = self.data_type(&x.r#type, false)
                        {
                            fields.push(ExportField {
                                name: field.token.to_string(),
                                r#type,
                            });
                        }
                    }
                    package.structs.push(ExportStruct { name, fields });
                }
                SymbolKind::Union(_) => {
                    self.error("package cannot have unions", &member_token);
                }
                // Functions and imports are Veryl's own.
                _ => (),
            }
        }
        self.context.pop_namespace();

        self.description
            .packages
            .insert(symbol.token.to_string(), package);
    }

    fn interface(&mut self, symbol: &Symbol) {
        let SymbolKind::Interface(property) = &symbol.kind else {
            unreachable!();
        };
        let token: TokenRange = symbol.token.into();
        if property.is_proto {
            self.error("cannot be attached to a proto interface", &token);
            return;
        }
        if !property.generic_parameters.is_empty() || !property.parameters.is_empty() {
            self.error(
                "interface cannot have parameters or generic parameters: its signals have to be fixed by itself",
                &token,
            );
            return;
        }

        self.context.push_namespace(symbol.inner_namespace());
        let mut interface = ExportInterface::default();
        for member in &property.members {
            let Some(modport) = symbol_table::get(*member) else {
                continue;
            };
            let SymbolKind::Modport(x) = &modport.kind else {
                continue;
            };
            let mut members = Vec::new();
            for member in &x.members {
                let Some(member) = symbol_table::get(*member) else {
                    continue;
                };
                // Functions are Veryl's own.
                let SymbolKind::ModportVariableMember(x) = &member.kind else {
                    continue;
                };
                let member_token: TokenRange = member.token.into();
                let direction = match x.direction {
                    Direction::Input => ExportDirection::Input,
                    Direction::Output => ExportDirection::Output,
                    _ => {
                        self.error(
                            "modport member has to be an input or an output",
                            &member_token,
                        );
                        continue;
                    }
                };
                let Some(variable) = symbol_table::get(x.variable) else {
                    continue;
                };
                let SymbolKind::Variable(variable) = &variable.kind else {
                    continue;
                };
                if let Some(r#type) = self.data_type(&variable.r#type, true) {
                    members.push(ExportMember {
                        name: member.token.to_string(),
                        direction,
                        r#type,
                    });
                }
            }
            interface.modports.push(ExportModport {
                name: modport.token.to_string(),
                members,
            });
        }
        self.context.pop_namespace();

        self.description
            .interfaces
            .insert(symbol.token.to_string(), interface);
    }

    fn enum_declaration(&mut self, symbol: &Symbol) -> Option<ExportEnum> {
        let SymbolKind::Enum(x) = &symbol.kind else {
            unreachable!();
        };
        let token: TokenRange = symbol.token.into();
        if x.width > 128 {
            self.error(
                "enum wider than 128 bits is not supported: no enum discriminant of Rust holds its values",
                &token,
            );
            return None;
        }
        let mut variants = Vec::new();
        for member in &x.members {
            let member = symbol_table::get(*member)?;
            let SymbolKind::EnumMember(property) = &member.kind else {
                continue;
            };
            let Some(value) = property.value.value() else {
                // A value with X or Z is no discriminant of Rust; one that
                // cannot be evaluated is reported by the analysis of enums.
                if matches!(property.value, EnumMemberValue::ExplicitValue(_, None)) {
                    self.error(
                        format!(
                            "the value of enum variant \"{}\" has X or Z, which is not supported",
                            member.token
                        ),
                        &member.token.into(),
                    );
                }
                return None;
            };
            variants.push(ExportVariant {
                name: member.token.to_string(),
                value: value.to_string(),
            });
        }
        Some(ExportEnum {
            name: symbol.token.to_string(),
            width: x.width as u32,
            variants,
        })
    }

    /// The type and the value of a constant.
    fn const_value(
        &mut self,
        r#type: &SymType,
        value: Option<&veryl_parser::veryl_grammar_trait::Expression>,
        token: &TokenRange,
    ) -> Option<(ExportType, ExportValue)> {
        if matches!(r#type.kind, TypeKind::String) {
            return self.string_value(value?, token);
        }
        let export_type = self.data_type(r#type, false)?;
        let ir_type = self.ir_type(r#type)?;
        let (comptime, _) =
            eval_expr(&mut self.context, Some(ir_type.clone()), value?, false).ok()?;
        self.context.drain_errors();
        // One that cannot be evaluated is reported by the analysis.
        let value = comptime.get_value().ok()?;
        if value.is_xz() {
            self.error(
                "the value of constant has X or Z, which is not supported",
                token,
            );
            return None;
        }
        let bits = value.payload().into_owned();
        let value = self.decompose(&ir_type, &bits, 0, token)?;
        Some((export_type, value))
    }

    /// The value of a constant of the string type, whose evaluation packs
    /// its bytes, the first the most significant.
    fn string_value(
        &mut self,
        value: &veryl_parser::veryl_grammar_trait::Expression,
        token: &TokenRange,
    ) -> Option<(ExportType, ExportValue)> {
        let (comptime, _) = eval_expr(&mut self.context, None, value, false).ok()?;
        self.context.drain_errors();
        let value = comptime.get_value().ok()?;
        let len = comptime.r#type.total_width().unwrap_or(0) / 8;
        let mut bytes = value.payload().to_bytes_be();
        if bytes.len() < len {
            bytes.splice(0..0, std::iter::repeat_n(0, len - bytes.len()));
        }
        match String::from_utf8(bytes) {
            Ok(x) => Some((ExportType::String, ExportValue::String(x))),
            Err(_) => {
                self.error("the value of constant is no UTF-8 string", token);
                None
            }
        }
    }

    /// The value of the bits `[offset, offset + width)` of `bits` as of
    /// `type`.
    fn decompose(
        &mut self,
        r#type: &ir::Type,
        bits: &BigUint,
        offset: usize,
        token: &TokenRange,
    ) -> Option<ExportValue> {
        let mut dims: Vec<usize> = r#type.width().iter().map(|x| x.unwrap_or(0)).collect();
        let elem_width = match &r#type.kind {
            ir::TypeKind::Bit | ir::TypeKind::Logic => dims.pop().unwrap_or(1),
            ir::TypeKind::Struct(x) => x.members.iter().filter_map(|x| x.width()).sum(),
            ir::TypeKind::Enum(x) => x.r#type.total_width()?,
            _ => return None,
        };
        self.decompose_dims(r#type, &dims, elem_width, bits, offset, token)
    }

    /// An array over `dims`, the first element the most significant, of
    /// elements of `elem_width` bits.
    fn decompose_dims(
        &mut self,
        r#type: &ir::Type,
        dims: &[usize],
        elem_width: usize,
        bits: &BigUint,
        offset: usize,
        token: &TokenRange,
    ) -> Option<ExportValue> {
        if let Some((len, rest)) = dims.split_first() {
            let stride = rest.iter().product::<usize>() * elem_width;
            let mut elems = Vec::new();
            for i in 0..*len {
                let pos = offset + (len - 1 - i) * stride;
                elems.push(self.decompose_dims(r#type, rest, elem_width, bits, pos, token)?);
            }
            return Some(ExportValue::Array(elems));
        }

        let raw = (bits >> offset) & ((BigUint::from(1u8) << elem_width) - 1u8);
        match &r#type.kind {
            ir::TypeKind::Bit | ir::TypeKind::Logic => {
                if elem_width > 128 {
                    self.error("constant wider than 128 bits is not supported", token);
                    return None;
                }
                let value = if r#type.signed && raw.bit(elem_width as u64 - 1) {
                    format!("-{}", (BigUint::from(1u8) << elem_width) - &raw)
                } else {
                    raw.to_string()
                };
                Some(ExportValue::Bits {
                    width: elem_width as u32,
                    signed: r#type.signed,
                    value,
                })
            }
            ir::TypeKind::Enum(x) => {
                let symbol = symbol_table::get(x.id)?;
                let SymbolKind::Enum(property) = &symbol.kind else {
                    return None;
                };
                let variant = property.members.iter().find_map(|id| {
                    let member = symbol_table::get(*id)?;
                    let SymbolKind::EnumMember(x) = &member.kind else {
                        return None;
                    };
                    (x.value.value() == Some(&raw)).then(|| member.token.to_string())
                });
                let package = symbol.get_parent_package()?.token.to_string();
                match variant {
                    Some(variant) => Some(ExportValue::Variant {
                        package,
                        r#enum: symbol.token.to_string(),
                        variant,
                    }),
                    None => {
                        self.error(
                            format!(
                                "the value of constant {raw} is no variant of enum \"{}\"",
                                symbol.token
                            ),
                            token,
                        );
                        None
                    }
                }
            }
            ir::TypeKind::Struct(x) => {
                let symbol = symbol_table::get(x.id)?;
                let package = symbol.get_parent_package()?.token.to_string();
                // The first field is the most significant.
                let mut pos = offset + elem_width;
                let mut fields = Vec::new();
                for member in &x.members {
                    pos -= member.width()?;
                    fields.push(ExportFieldValue {
                        name: member.name.to_string(),
                        value: self.decompose(&member.r#type, bits, pos, token)?,
                    });
                }
                Some(ExportValue::Struct {
                    package,
                    name: symbol.token.to_string(),
                    fields,
                })
            }
            _ => None,
        }
    }

    /// The type a `const X: type = ..` gives.
    fn type_value(
        &mut self,
        value: Option<&veryl_parser::veryl_grammar_trait::Expression>,
        token: &TokenRange,
    ) -> Option<ExportType> {
        let (comptime, _) = eval_expr(&mut self.context, None, value?, false).ok()?;
        self.context.drain_errors();
        let ValueVariant::Type(r#type) = &comptime.value else {
            self.error("the type of constant cannot be evaluated", token);
            return None;
        };
        self.export_type(r#type, false, token)
    }

    fn ir_type(&mut self, r#type: &SymType) -> Option<ir::Type> {
        let ret = r#type
            .to_ir_type(&mut self.context, TypePosition::Variable)
            .ok();
        // Breaches of the type's own rules are reported by the analysis.
        self.context.drain_errors();
        ret
    }

    /// The type of a field, a member, or a constant; clocks and resets are
    /// allowed for a modport member only.
    fn data_type(&mut self, r#type: &SymType, allow_clock: bool) -> Option<ExportType> {
        let ir_type = self.ir_type(r#type)?;
        self.export_type(&ir_type, allow_clock, &r#type.token)
    }

    fn export_type(
        &mut self,
        r#type: &ir::Type,
        allow_clock: bool,
        token: &TokenRange,
    ) -> Option<ExportType> {
        if !r#type.array.is_empty() {
            self.error("unpacked array is not supported", token);
            return None;
        }

        let mut dims = Vec::new();
        for x in r#type.width().iter() {
            let Some(x) = x else {
                self.error("the width of type cannot be evaluated", token);
                return None;
            };
            dims.push(*x as u32);
        }

        let elem = match &r#type.kind {
            ir::TypeKind::Bit | ir::TypeKind::Logic => {
                let width = dims.pop().unwrap_or(1);
                ExportType::Bits {
                    width,
                    signed: r#type.signed,
                }
            }
            ir::TypeKind::Clock | ir::TypeKind::ClockPosedge | ir::TypeKind::ClockNegedge
                if allow_clock =>
            {
                // The width of a clock is one bit.
                dims.pop();
                ExportType::Clock
            }
            ir::TypeKind::Reset
            | ir::TypeKind::ResetAsyncHigh
            | ir::TypeKind::ResetAsyncLow
            | ir::TypeKind::ResetSyncHigh
            | ir::TypeKind::ResetSyncLow
                if allow_clock =>
            {
                dims.pop();
                ExportType::Reset
            }
            ir::TypeKind::Struct(x) => self.user_type(x.id)?,
            ir::TypeKind::Enum(x) => self.user_type(x.id)?,
            ir::TypeKind::Union(_) => {
                self.error("union is not supported", token);
                return None;
            }
            _ => {
                self.error(format!("type \"{}\" is not supported", r#type), token);
                return None;
            }
        };

        if matches!(elem, ExportType::Clock | ExportType::Reset) && !dims.is_empty() {
            self.error("array of clock or reset is not supported", token);
            return None;
        }

        Some(
            dims.into_iter()
                .rev()
                .fold(elem, |elem, len| ExportType::Array {
                    len,
                    elem: Box::new(elem),
                }),
        )
    }

    /// A reference to a struct or an enum, which has to be declared in an
    /// exported package.
    fn user_type(&mut self, id: SymbolId) -> Option<ExportType> {
        let symbol = symbol_table::get(id)?;
        // One in a package not exported is reported as a reference to it.
        let package = symbol
            .get_parent_package()
            .filter(|x| post_pass1_scope::is_exported_item(x))?;
        Some(ExportType::User {
            package: package.token.to_string(),
            name: symbol.token.to_string(),
        })
    }
}
