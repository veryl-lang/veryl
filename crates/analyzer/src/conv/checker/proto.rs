use crate::HashMap;
use crate::analyzer_error::{AnalyzerError, IncompatProtoKind};
use crate::conv::Context;
use crate::namespace::Namespace;
use crate::symbol::{
    Direction, EnumProperty, FunctionProperty, InterfaceProperty, ModportProperty, ModuleProperty,
    PackageProperty, Parameter, ParameterProperty, Port, ProtoConstProperty,
    ProtoInterfaceProperty, ProtoModuleProperty, ProtoPackageProperty, ProtoTypeDefProperty,
    StructProperty, SymbolId, SymbolKind, Type, TypeDefProperty, TypeKind, UnionProperty,
    VariableProperty,
};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

fn check_module_compat(
    actual: &ModuleProperty,
    proto: &ProtoModuleProperty,
) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    ret.append(&mut check_params_compat(
        &actual.parameters,
        &proto.parameters,
    ));
    ret.append(&mut check_ports_compat(&actual.ports, &proto.ports));
    ret
}

fn check_interface_compat(
    actual: &InterfaceProperty,
    proto: &ProtoInterfaceProperty,
) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();

    ret.append(&mut check_params_compat(
        &actual.parameters,
        &proto.parameters,
    ));

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
            let actual_symbol = actual;
            let proto_symbol = proto;
            match (&actual_symbol.kind, &proto_symbol.kind) {
                (SymbolKind::Parameter(actual), SymbolKind::ProtoConst(proto)) => {
                    if !check_const_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleParam(text));
                    }
                }
                (_, SymbolKind::ProtoConst(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleParam(text));
                }
                (SymbolKind::Variable(actual), SymbolKind::Variable(proto)) => {
                    if !check_variable_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleVar(text));
                    }
                }
                (_, SymbolKind::Variable(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleVar(text));
                }
                (SymbolKind::TypeDef(actual), SymbolKind::ProtoTypeDef(proto)) => {
                    if !check_typedef_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                    }
                }
                (_, SymbolKind::ProtoTypeDef(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                }
                (SymbolKind::Function(actual), SymbolKind::ProtoFunction(proto)) => {
                    if !check_function_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleFunction(text));
                    }
                }
                (_, SymbolKind::ProtoFunction(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleFunction(text));
                }
                (SymbolKind::AliasModule(actual), SymbolKind::ProtoAliasModule(proto)) => {
                    if !check_alias_compat(
                        &actual.target,
                        &actual_symbol.namespace,
                        &proto.target,
                        &proto_symbol.namespace,
                    )
                    .is_empty()
                    {
                        ret.push(IncompatProtoKind::IncompatibleAlias(text));
                    }
                }
                (_, SymbolKind::ProtoAliasModule(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleAlias(text));
                }
                (SymbolKind::AliasInterface(actual), SymbolKind::ProtoAliasInterface(proto)) => {
                    if !check_alias_compat(
                        &actual.target,
                        &actual_symbol.namespace,
                        &proto.target,
                        &proto_symbol.namespace,
                    )
                    .is_empty()
                    {
                        ret.push(IncompatProtoKind::IncompatibleAlias(text));
                    }
                }
                (_, SymbolKind::ProtoAliasInterface(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleAlias(text));
                }
                (SymbolKind::AliasPackage(actual), SymbolKind::ProtoAliasPackage(proto)) => {
                    if !check_alias_compat(
                        &actual.target,
                        &actual_symbol.namespace,
                        &proto.target,
                        &proto_symbol.namespace,
                    )
                    .is_empty()
                    {
                        ret.push(IncompatProtoKind::IncompatibleAlias(text));
                    }
                }
                (_, SymbolKind::ProtoAliasPackage(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleAlias(text));
                }
                (SymbolKind::Modport(actual), SymbolKind::Modport(proto)) => {
                    if !check_modport_comat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleModport(text));
                    }
                }
                (_, SymbolKind::Modport(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleModport(text));
                }
                _ => {}
            }
        } else {
            match proto.kind {
                SymbolKind::Parameter(_) | SymbolKind::ProtoConst(_) => {
                    ret.push(IncompatProtoKind::MissingParam(text))
                }
                SymbolKind::Variable(_) => ret.push(IncompatProtoKind::MissingVar(text)),
                SymbolKind::ProtoTypeDef(_) => ret.push(IncompatProtoKind::MissingTypedef(text)),
                SymbolKind::ProtoFunction(_) => ret.push(IncompatProtoKind::MissingFunction(text)),
                SymbolKind::ProtoAliasModule(_)
                | SymbolKind::ProtoAliasInterface(_)
                | SymbolKind::ProtoAliasPackage(_) => {
                    ret.push(IncompatProtoKind::MissingAlias(text))
                }
                SymbolKind::Modport(_) => ret.push(IncompatProtoKind::MissingModport(text)),
                _ => {}
            }
        }
    }

    ret
}

fn check_package_compat(
    actual: &PackageProperty,
    proto: &ProtoPackageProperty,
) -> Vec<IncompatProtoKind> {
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
            let actual_symbol = actual;
            let proto_symbol = proto;
            match (&actual_symbol.kind, &proto_symbol.kind) {
                (SymbolKind::Parameter(actual), SymbolKind::ProtoConst(proto)) => {
                    if !check_const_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleParam(text));
                    }
                }
                (_, SymbolKind::ProtoConst(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleParam(text));
                }
                (SymbolKind::TypeDef(actual), SymbolKind::ProtoTypeDef(proto)) => {
                    if !check_typedef_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                    }
                }
                (_, SymbolKind::ProtoTypeDef(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                }
                (SymbolKind::Enum(actual), SymbolKind::Enum(proto)) => {
                    if !check_enum_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                    }
                }
                (_, SymbolKind::Enum(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                }
                (SymbolKind::Struct(actual), SymbolKind::Struct(proto)) => {
                    if !check_struct_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                    }
                }
                (_, SymbolKind::Struct(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                }
                (SymbolKind::Union(actual), SymbolKind::Union(proto)) => {
                    if !check_union_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                    }
                }
                (_, SymbolKind::Union(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleTypedef(text));
                }
                (SymbolKind::Function(actual), SymbolKind::ProtoFunction(proto)) => {
                    if !check_function_compat(actual, proto).is_empty() {
                        ret.push(IncompatProtoKind::IncompatibleFunction(text));
                    }
                }
                (_, SymbolKind::ProtoFunction(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleFunction(text));
                }
                (SymbolKind::AliasModule(actual), SymbolKind::ProtoAliasModule(proto)) => {
                    if !check_alias_compat(
                        &actual.target,
                        &actual_symbol.namespace,
                        &proto.target,
                        &proto_symbol.namespace,
                    )
                    .is_empty()
                    {
                        ret.push(IncompatProtoKind::IncompatibleAlias(text));
                    }
                }
                (_, SymbolKind::ProtoAliasModule(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleAlias(text));
                }
                (SymbolKind::AliasInterface(actual), SymbolKind::ProtoAliasInterface(proto)) => {
                    if !check_alias_compat(
                        &actual.target,
                        &actual_symbol.namespace,
                        &proto.target,
                        &proto_symbol.namespace,
                    )
                    .is_empty()
                    {
                        ret.push(IncompatProtoKind::IncompatibleAlias(text));
                    }
                }
                (_, SymbolKind::ProtoAliasInterface(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleAlias(text));
                }
                (SymbolKind::AliasPackage(actual), SymbolKind::ProtoAliasPackage(proto)) => {
                    if !check_alias_compat(
                        &actual.target,
                        &actual_symbol.namespace,
                        &proto.target,
                        &proto_symbol.namespace,
                    )
                    .is_empty()
                    {
                        ret.push(IncompatProtoKind::IncompatibleAlias(text));
                    }
                }
                (_, SymbolKind::ProtoAliasPackage(_)) => {
                    ret.push(IncompatProtoKind::IncompatibleAlias(text));
                }
                _ => {}
            }
        } else {
            match proto.kind {
                SymbolKind::ProtoConst(_) => ret.push(IncompatProtoKind::MissingParam(text)),
                SymbolKind::ProtoTypeDef(_) => ret.push(IncompatProtoKind::MissingTypedef(text)),
                SymbolKind::Enum(_) => ret.push(IncompatProtoKind::MissingTypedef(text)),
                SymbolKind::Struct(_) => ret.push(IncompatProtoKind::MissingTypedef(text)),
                SymbolKind::Union(_) => ret.push(IncompatProtoKind::MissingTypedef(text)),
                SymbolKind::ProtoFunction(_) => ret.push(IncompatProtoKind::MissingFunction(text)),
                SymbolKind::ProtoAliasModule(_)
                | SymbolKind::ProtoAliasInterface(_)
                | SymbolKind::ProtoAliasPackage(_) => {
                    ret.push(IncompatProtoKind::MissingAlias(text))
                }
                _ => {}
            }
        }
    }

    ret
}

fn check_generic_params_compat(actual: &[SymbolId], proto: &[SymbolId]) -> Vec<IncompatProtoKind> {
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
                ret.push(IncompatProtoKind::IncompatibleGenericParam(name));
            }
        } else {
            ret.push(IncompatProtoKind::UnnecessaryGenericParam(name));
        }
    }

    for (name, _) in proto_params {
        ret.push(IncompatProtoKind::MissingGenericParam(name));
    }

    ret
}

fn check_params_compat(actual: &[Parameter], proto: &[Parameter]) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();

    let actual_params: HashMap<_, _> = actual.iter().map(|x| (x.name, x.property())).collect();
    let mut proto_params: HashMap<_, _> = proto.iter().map(|x| (x.name, x.property())).collect();

    for (name, actual) in actual_params {
        if let Some(proto) = proto_params.remove(&name) {
            if !check_param_compat(&actual, &proto).is_empty() {
                ret.push(IncompatProtoKind::IncompatibleParam(name));
            }
        } else {
            ret.push(IncompatProtoKind::UnnecessaryParam(name));
        }
    }

    for (name, _) in proto_params {
        ret.push(IncompatProtoKind::MissingParam(name));
    }

    ret
}

fn check_ports_compat(actual: &[Port], proto: &[Port]) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();

    let actual_ports: HashMap<_, _> = actual.iter().map(|x| (x.name(), x.property())).collect();
    let mut proto_ports: HashMap<_, _> = proto.iter().map(|x| (x.name(), x.property())).collect();

    for (name, actual) in actual_ports {
        if let Some(proto) = proto_ports.remove(&name) {
            if !actual.r#type.is_compatible(&proto.r#type) {
                ret.push(IncompatProtoKind::IncompatiblePort(name));
            }
        } else {
            ret.push(IncompatProtoKind::UnnecessaryPort(name));
        }
    }

    for (name, _) in proto_ports {
        ret.push(IncompatProtoKind::MissingPort(name));
    }

    ret
}

fn check_param_compat(
    actual: &ParameterProperty,
    proto: &ParameterProperty,
) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    if !(actual.kind == proto.kind && actual.r#type.is_compatible(&proto.r#type)) {
        ret.push(IncompatProtoKind::IncompatibleType)
    }
    ret
}

fn check_variable_compat(
    actual: &VariableProperty,
    proto: &VariableProperty,
) -> Vec<IncompatProtoKind> {
    check_type_compat(&Some(actual.r#type.clone()), &Some(proto.r#type.clone()))
}

fn check_const_compat(
    actual: &ParameterProperty,
    proto: &ProtoConstProperty,
) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    if !(actual.kind.is_const() && actual.r#type.is_compatible(&proto.r#type)) {
        ret.push(IncompatProtoKind::IncompatibleType)
    }
    ret
}

fn check_typedef_compat(
    actual: &TypeDefProperty,
    proto: &ProtoTypeDefProperty,
) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    if proto.r#type.is_some() {
        ret.append(&mut check_type_compat(
            &Some(actual.r#type.clone()),
            &proto.r#type,
        ));
    }
    ret
}

fn check_struct_compat(actual: &StructProperty, proto: &StructProperty) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    ret.append(&mut check_generic_params_compat(
        &actual.generic_parameters,
        &proto.generic_parameters,
    ));
    ret.append(&mut check_members_compat(&actual.members, &proto.members));
    ret
}

fn check_union_compat(actual: &UnionProperty, proto: &UnionProperty) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    ret.append(&mut check_generic_params_compat(
        &actual.generic_parameters,
        &proto.generic_parameters,
    ));
    ret.append(&mut check_members_compat(&actual.members, &proto.members));
    ret
}

fn check_enum_compat(actual: &EnumProperty, proto: &EnumProperty) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    ret.append(&mut check_type_compat(&actual.r#type, &proto.r#type));
    ret.append(&mut check_members_compat(&actual.members, &proto.members));
    ret
}

fn check_function_compat(
    actual: &FunctionProperty,
    proto: &FunctionProperty,
) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    ret.append(&mut check_generic_params_compat(
        &actual.generic_parameters,
        &proto.generic_parameters,
    ));
    ret.append(&mut check_ports_compat(&actual.ports, &proto.ports));
    ret.append(&mut check_type_compat(&actual.ret, &proto.ret));
    ret
}

fn check_alias_compat(
    actual_path: &GenericSymbolPath,
    actual_namespace: &Namespace,
    proto_path: &GenericSymbolPath,
    proto_namespace: &Namespace,
) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();

    let actual_proto = {
        let Ok(symbol) = symbol_table::resolve((&actual_path.generic_path(), actual_namespace))
        else {
            return ret;
        };
        symbol.found.proto()
    };
    let Ok(required_proto) = symbol_table::resolve((&proto_path.generic_path(), proto_namespace))
    else {
        return ret;
    };

    let proto_match = if let Some(actual_proto) = actual_proto {
        actual_proto.id == required_proto.found.id
    } else {
        false
    };

    if !proto_match {
        ret.push(IncompatProtoKind::IncompatibleType);
    }

    ret
}

fn check_modport_comat(
    actual: &ModportProperty,
    proto: &ModportProperty,
) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();

    let actual_members: HashMap<_, _> = actual
        .members
        .iter()
        .map(|x| {
            let symbol = symbol_table::get(*x).unwrap();
            let direction = match symbol.kind {
                SymbolKind::ModportVariableMember(x) => x.direction,
                SymbolKind::ModportFunctionMember(_) => Direction::Modport,
                _ => {
                    unreachable!()
                }
            };
            (symbol.token.text, direction)
        })
        .collect();
    let mut proto_members: HashMap<_, _> = proto
        .members
        .iter()
        .map(|x| {
            let symbol = symbol_table::get(*x).unwrap();
            let direction = match symbol.kind {
                SymbolKind::ModportVariableMember(x) => x.direction,
                SymbolKind::ModportFunctionMember(_) => Direction::Modport,
                _ => {
                    unreachable!()
                }
            };
            (symbol.token.text, direction)
        })
        .collect();

    for (name, actual) in actual_members {
        if let Some(proto) = proto_members.remove(&name) {
            if actual != proto {
                ret.push(IncompatProtoKind::IncompatibleMember(name));
            }
        } else {
            ret.push(IncompatProtoKind::UnnecessaryMember(name));
        }
    }

    for (name, _) in proto_members {
        ret.push(IncompatProtoKind::MissignMember(name));
    }

    ret
}

fn check_type_compat(actual: &Option<Type>, proto: &Option<Type>) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();

    if let (Some(actual), Some(proto)) = (actual, proto) {
        if !actual.is_compatible(proto) {
            ret.push(IncompatProtoKind::IncompatibleType);
        }
    } else if actual.is_none() && proto.is_some() {
        ret.push(IncompatProtoKind::MissingType);
    } else if actual.is_some() && proto.is_none() {
        ret.push(IncompatProtoKind::UnnecessaryType);
    }

    ret
}

fn check_members_compat(actual: &[SymbolId], proto: &[SymbolId]) -> Vec<IncompatProtoKind> {
    let mut ret = Vec::new();
    let default_type = Type {
        modifier: Vec::new(),
        kind: TypeKind::Bit,
        width: Vec::new(),
        array: Vec::new(),
        array_type: None,
        is_const: false,
        token: TokenRange::default(),
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
                ret.push(IncompatProtoKind::IncompatibleMember(name));
            }
        } else {
            ret.push(IncompatProtoKind::UnnecessaryMember(name));
        }
    }

    for (name, _) in proto_members {
        ret.push(IncompatProtoKind::MissignMember(name));
    }

    ret
}

pub fn check_proto(context: &mut Context, actual: &Identifier, proto: &ScopedIdentifier) {
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
            context.insert_error(AnalyzerError::mismatch_type(
                &proto_symbol.token.to_string(),
                "proto module",
                &proto_symbol.kind.to_kind_name(),
                &proto.identifier().token.into(),
            ));
        }
        (SymbolKind::Interface(actual), SymbolKind::ProtoInterface(proto)) => {
            errors.append(&mut check_interface_compat(actual, proto));
        }
        (SymbolKind::Interface(_), _) => {
            context.insert_error(AnalyzerError::mismatch_type(
                &proto_symbol.token.to_string(),
                "proto interface",
                &proto_symbol.kind.to_kind_name(),
                &proto.identifier().token.into(),
            ));
        }
        (SymbolKind::Package(actual), SymbolKind::ProtoPackage(proto)) => {
            errors.append(&mut check_package_compat(actual, proto));
        }
        (SymbolKind::Package(_), _) => {
            context.insert_error(AnalyzerError::mismatch_type(
                &proto_symbol.token.to_string(),
                "proto package",
                &proto_symbol.kind.to_kind_name(),
                &proto.identifier().token.into(),
            ));
        }
        _ => {}
    };

    for error in errors {
        context.insert_error(AnalyzerError::incompat_proto(
            &actual_symbol.token.to_string(),
            &proto_symbol.token.to_string(),
            error,
            &actual_symbol.token.into(),
        ));
    }
}
