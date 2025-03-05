use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::symbol::{
    EnumProperty, FunctionProperty, ModuleProperty, PackageProperty, Parameter, ParameterProperty,
    Port, ProtoConstProperty, ProtoModuleProperty, ProtoPackageProperty, StructProperty, SymbolId,
    SymbolKind, Type, TypeKind, UnionProperty,
};
use crate::symbol_table;
use veryl_parser::ParolError;
use veryl_parser::resource_table::StrId;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

pub enum ProtoIncompatible {
    MissingParam(StrId),
    MissingPort(StrId),
    MissingGenericParam(StrId),
    MissingConst(StrId),
    MissingTypedef(StrId),
    MissignMember(StrId),
    MissingFunction(StrId),
    MissingType,
    UnnecessaryParam(StrId),
    UnnecessaryPort(StrId),
    UnnecessaryGenericParam(StrId),
    UnnecessaryMember(StrId),
    UnnecessaryType,
    IncompatibleParam(StrId),
    IncompatiblePort(StrId),
    IncompatibleGenericParam(StrId),
    IncompatibleTypedef(StrId),
    IncompatibleMember(StrId),
    IncompatibleFunction(StrId),
    IncompatibleType,
}

impl ProtoIncompatible {
    pub fn cause(&self) -> String {
        match self {
            ProtoIncompatible::MissingParam(x) => {
                format!("parameter {x} is missing")
            }
            ProtoIncompatible::MissingPort(x) => {
                format!("port {x} is missing")
            }
            ProtoIncompatible::MissingGenericParam(x) => {
                format!("generic parameter {x} is missing")
            }
            ProtoIncompatible::MissingConst(x) => {
                format!("const {x} is missing")
            }
            ProtoIncompatible::MissingTypedef(x) => {
                format!("type definition {x} is missing")
            }
            ProtoIncompatible::MissignMember(x) => {
                format!("member {x} is missing")
            }
            ProtoIncompatible::MissingFunction(x) => {
                format!("function {x} is missing")
            }
            ProtoIncompatible::MissingType => "type specification is missing".to_string(),
            ProtoIncompatible::UnnecessaryParam(x) => {
                format!("parameter {x} is unnecessary")
            }
            ProtoIncompatible::UnnecessaryPort(x) => {
                format!("port {x} is unnecessary")
            }
            ProtoIncompatible::UnnecessaryGenericParam(x) => {
                format!("generic parameter {x} is unnecessary")
            }
            ProtoIncompatible::UnnecessaryMember(x) => {
                format!("member {x} is unnecessary")
            }
            ProtoIncompatible::UnnecessaryType => "type specification is unnecessary".to_string(),
            ProtoIncompatible::IncompatibleParam(x) => {
                format!("parameter {x} has incompatible type")
            }
            ProtoIncompatible::IncompatiblePort(x) => {
                format!("port {x} has incompatible type")
            }
            ProtoIncompatible::IncompatibleGenericParam(x) => {
                format!("generic parameter {x} is incompatible")
            }
            ProtoIncompatible::IncompatibleTypedef(x) => {
                format!("type definition {x} is incompatible")
            }
            ProtoIncompatible::IncompatibleMember(x) => {
                format!("member {x} is incompatible")
            }
            ProtoIncompatible::IncompatibleFunction(x) => {
                format!("function {x} is incompatible")
            }
            ProtoIncompatible::IncompatibleType => "type specification is incompatible".to_string(),
        }
    }
}

fn check_module_compat(
    actual: &ModuleProperty,
    proto: &ProtoModuleProperty,
) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();
    ret.append(&mut check_params_compat(
        &actual.parameters,
        &proto.parameters,
    ));
    ret.append(&mut check_ports_compat(&actual.ports, &proto.ports));
    ret
}

fn check_package_compat(
    actual: &PackageProperty,
    proto: &ProtoPackageProperty,
) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();

    let actual_members: Vec<_> = actual
        .members
        .iter()
        .map(|x| symbol_table::get(*x).unwrap())
        .collect();
    let proto_members: Vec<_> = proto
        .members
        .iter()
        .map(|x| symbol_table::get(*x).unwrap())
        .collect();

    for proto in proto_members {
        let text = proto.token.text;
        if let Some(actual) = actual_members.iter().find(|x| x.token.text == text) {
            match (&actual.kind, &proto.kind) {
                (SymbolKind::Parameter(actual), SymbolKind::ProtoConst(proto)) => {
                    if !check_const_compat(actual, proto).is_empty() {
                        ret.push(ProtoIncompatible::IncompatibleParam(text));
                    }
                }
                (_, SymbolKind::ProtoConst(_)) => {
                    ret.push(ProtoIncompatible::IncompatibleParam(text));
                }
                (SymbolKind::TypeDef(_), SymbolKind::ProtoTypeDef) => {
                    // nothing to check
                }
                (_, SymbolKind::ProtoTypeDef) => {
                    ret.push(ProtoIncompatible::IncompatibleTypedef(text));
                }
                (SymbolKind::Enum(actual), SymbolKind::Enum(proto)) => {
                    if !check_enum_compat(actual, proto).is_empty() {
                        ret.push(ProtoIncompatible::IncompatibleTypedef(text));
                    }
                }
                (_, SymbolKind::Enum(_)) => {
                    ret.push(ProtoIncompatible::IncompatibleTypedef(text));
                }
                (SymbolKind::Struct(actual), SymbolKind::Struct(proto)) => {
                    if !check_struct_compat(actual, proto).is_empty() {
                        ret.push(ProtoIncompatible::IncompatibleTypedef(text));
                    }
                }
                (_, SymbolKind::Struct(_)) => {
                    ret.push(ProtoIncompatible::IncompatibleTypedef(text));
                }
                (SymbolKind::Union(actual), SymbolKind::Union(proto)) => {
                    if !check_union_compat(actual, proto).is_empty() {
                        ret.push(ProtoIncompatible::IncompatibleTypedef(text));
                    }
                }
                (_, SymbolKind::Union(_)) => {
                    ret.push(ProtoIncompatible::IncompatibleTypedef(text));
                }
                (SymbolKind::Function(actual), SymbolKind::ProtoFunction(proto)) => {
                    if !check_function_compat(actual, proto).is_empty() {
                        ret.push(ProtoIncompatible::IncompatibleFunction(text));
                    }
                }
                (_, SymbolKind::ProtoFunction(_)) => {
                    ret.push(ProtoIncompatible::IncompatibleFunction(text));
                }
                _ => {}
            }
        } else {
            match proto.kind {
                SymbolKind::ProtoConst(_) => ret.push(ProtoIncompatible::MissingParam(text)),
                SymbolKind::ProtoTypeDef => ret.push(ProtoIncompatible::MissingTypedef(text)),
                SymbolKind::Enum(_) => ret.push(ProtoIncompatible::MissingTypedef(text)),
                SymbolKind::Struct(_) => ret.push(ProtoIncompatible::MissingTypedef(text)),
                SymbolKind::Union(_) => ret.push(ProtoIncompatible::MissingTypedef(text)),
                SymbolKind::ProtoFunction(_) => ret.push(ProtoIncompatible::MissingFunction(text)),
                _ => {}
            }
        }
    }

    ret
}

fn check_generic_params_compat(actual: &[SymbolId], proto: &[SymbolId]) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();

    let actual_params: HashMap<_, _> = actual
        .iter()
        .map(|x| {
            let symbol = symbol_table::get(*x).unwrap();
            if let SymbolKind::GenericParameter(x) = symbol.kind {
                (symbol.token.text, x)
            } else {
                unreachable!()
            }
        })
        .collect();
    let mut proto_params: HashMap<_, _> = proto
        .iter()
        .map(|x| {
            let symbol = symbol_table::get(*x).unwrap();
            if let SymbolKind::GenericParameter(x) = symbol.kind {
                (symbol.token.text, x)
            } else {
                unreachable!()
            }
        })
        .collect();

    for (name, actual) in actual_params {
        if let Some(proto) = proto_params.remove(&name) {
            if !actual.bound.is_compatible(&proto.bound) {
                ret.push(ProtoIncompatible::IncompatibleGenericParam(name));
            }
        } else {
            ret.push(ProtoIncompatible::UnnecessaryGenericParam(name));
        }
    }

    for (name, _) in proto_params {
        ret.push(ProtoIncompatible::MissingGenericParam(name));
    }

    ret
}

fn check_params_compat(actual: &[Parameter], proto: &[Parameter]) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();

    let actual_params: HashMap<_, _> = actual.iter().map(|x| (x.name, x.property())).collect();
    let mut proto_params: HashMap<_, _> = proto.iter().map(|x| (x.name, x.property())).collect();

    for (name, actual) in actual_params {
        if let Some(proto) = proto_params.remove(&name) {
            if !actual.r#type.is_compatible(&proto.r#type) {
                ret.push(ProtoIncompatible::IncompatibleParam(name));
            }
        } else {
            ret.push(ProtoIncompatible::UnnecessaryParam(name));
        }
    }

    for (name, _) in proto_params {
        ret.push(ProtoIncompatible::MissingParam(name));
    }

    ret
}

fn check_ports_compat(actual: &[Port], proto: &[Port]) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();

    let actual_ports: HashMap<_, _> = actual.iter().map(|x| (x.name(), x.property())).collect();
    let mut proto_ports: HashMap<_, _> = proto.iter().map(|x| (x.name(), x.property())).collect();

    for (name, actual) in actual_ports {
        if let Some(proto) = proto_ports.remove(&name) {
            if !actual.r#type.is_compatible(&proto.r#type) {
                ret.push(ProtoIncompatible::IncompatiblePort(name));
            }
        } else {
            ret.push(ProtoIncompatible::UnnecessaryPort(name));
        }
    }

    for (name, _) in proto_ports {
        ret.push(ProtoIncompatible::MissingPort(name));
    }

    ret
}

fn check_const_compat(
    actual: &ParameterProperty,
    proto: &ProtoConstProperty,
) -> Vec<ProtoIncompatible> {
    check_type_compat(&Some(actual.r#type.clone()), &Some(proto.r#type.clone()))
}

fn check_struct_compat(actual: &StructProperty, proto: &StructProperty) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();
    ret.append(&mut check_generic_params_compat(
        &actual.generic_parameters,
        &proto.generic_parameters,
    ));
    ret.append(&mut check_members_compat(&actual.members, &proto.members));
    ret
}

fn check_union_compat(actual: &UnionProperty, proto: &UnionProperty) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();
    ret.append(&mut check_generic_params_compat(
        &actual.generic_parameters,
        &proto.generic_parameters,
    ));
    ret.append(&mut check_members_compat(&actual.members, &proto.members));
    ret
}

fn check_enum_compat(actual: &EnumProperty, proto: &EnumProperty) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();
    ret.append(&mut check_type_compat(&actual.r#type, &proto.r#type));
    ret.append(&mut check_members_compat(&actual.members, &proto.members));
    ret
}

fn check_function_compat(
    actual: &FunctionProperty,
    proto: &FunctionProperty,
) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();
    ret.append(&mut check_generic_params_compat(
        &actual.generic_parameters,
        &proto.generic_parameters,
    ));
    ret.append(&mut check_ports_compat(&actual.ports, &proto.ports));
    ret.append(&mut check_type_compat(&actual.ret, &proto.ret));
    ret
}

fn check_type_compat(actual: &Option<Type>, proto: &Option<Type>) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();

    if let (Some(actual), Some(proto)) = (actual, proto) {
        if !actual.is_compatible(proto) {
            ret.push(ProtoIncompatible::IncompatibleType);
        }
    } else if actual.is_none() && proto.is_some() {
        ret.push(ProtoIncompatible::MissingType);
    } else if actual.is_some() && proto.is_none() {
        ret.push(ProtoIncompatible::UnnecessaryType);
    }

    ret
}

fn check_members_compat(actual: &[SymbolId], proto: &[SymbolId]) -> Vec<ProtoIncompatible> {
    let mut ret = Vec::new();
    let default_type = Type {
        modifier: Vec::new(),
        kind: TypeKind::Bit,
        width: Vec::new(),
        array: Vec::new(),
        is_const: false,
    };

    let actual_members: HashMap<_, _> = actual
        .iter()
        .map(|x| {
            let symbol = symbol_table::get(*x).unwrap();
            match symbol.kind {
                SymbolKind::StructMember(x) => (symbol.token.text, x.r#type),
                SymbolKind::UnionMember(x) => (symbol.token.text, x.r#type),
                _ => (symbol.token.text, default_type.clone()), // enum member
            }
        })
        .collect();
    let mut proto_members: HashMap<_, _> = proto
        .iter()
        .map(|x| {
            let symbol = symbol_table::get(*x).unwrap();
            match symbol.kind {
                SymbolKind::StructMember(x) => (symbol.token.text, x.r#type),
                SymbolKind::UnionMember(x) => (symbol.token.text, x.r#type),
                _ => (symbol.token.text, default_type.clone()), // enum member
            }
        })
        .collect();

    for (name, actual) in actual_members {
        if let Some(proto) = proto_members.remove(&name) {
            if !actual.is_compatible(&proto) {
                ret.push(ProtoIncompatible::IncompatibleMember(name));
            }
        } else {
            ret.push(ProtoIncompatible::UnnecessaryMember(name));
        }
    }

    for (name, _) in proto_members {
        ret.push(ProtoIncompatible::MissignMember(name));
    }

    ret
}

#[derive(Default)]
pub struct CheckProto {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
}

impl CheckProto {
    pub fn new() -> Self {
        Self::default()
    }

    fn check_proto(&mut self, actual: &Identifier, proto: &ScopedIdentifier) {
        let actual_symbol = if let Ok(symbol) = symbol_table::resolve(actual) {
            symbol.found
        } else {
            return;
        };
        let proto_symbol = if let Ok(symbol) = symbol_table::resolve(proto) {
            symbol.found
        } else {
            return;
        };

        let mut errors = Vec::new();
        match (&actual_symbol.kind, &proto_symbol.kind) {
            (SymbolKind::Module(actual), SymbolKind::ProtoModule(proto)) => {
                errors.append(&mut check_module_compat(actual, proto));
            }
            (SymbolKind::Module(_), _) => {
                self.errors.push(AnalyzerError::mismatch_type(
                    &proto_symbol.token.to_string(),
                    "module prototype",
                    &proto_symbol.kind.to_kind_name(),
                    &proto.identifier().token.into(),
                ));
            }
            (SymbolKind::Package(actual), SymbolKind::ProtoPackage(proto)) => {
                errors.append(&mut check_package_compat(actual, proto));
            }
            (SymbolKind::Package(_), _) => {
                self.errors.push(AnalyzerError::mismatch_type(
                    &proto_symbol.token.to_string(),
                    "package prototype",
                    &proto_symbol.kind.to_kind_name(),
                    &proto.identifier().token.into(),
                ));
            }
            _ => {}
        };

        for error in errors {
            self.errors.push(AnalyzerError::incompat_proto(
                &actual_symbol.token.to_string(),
                &proto_symbol.token.to_string(),
                &error.cause(),
                &actual_symbol.token.into(),
            ));
        }
    }
}

impl Handler for CheckProto {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckProto {
    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Some(ref x) = arg.module_declaration_opt0 {
                self.check_proto(&arg.identifier, &x.scoped_identifier);
            }
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Some(ref x) = arg.package_declaration_opt0 {
                self.check_proto(&arg.identifier, &x.scoped_identifier);
            }
        }
        Ok(())
    }
}
