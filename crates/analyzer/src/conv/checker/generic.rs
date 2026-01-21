use crate::conv::Context;
use crate::namespace::Namespace;
use crate::symbol::{
    GenericBoundKind, ProtoBound, Symbol, SymbolId, SymbolKind, Type as SymType, TypeKind,
};
use crate::symbol_path::{GenericSymbolPath, GenericSymbolPathKind};
use crate::{AnalyzerError, namespace_table, symbol_table};
use veryl_parser::veryl_grammar_trait::*;

fn collect_identifier(value: &ScopedIdentifier) -> Vec<ScopedIdentifier> {
    let mut ret = vec![value.clone()];

    if let ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) =
        value.scoped_identifier_group.as_ref()
        && let Some(x) = &x.scoped_identifier_opt
        && let Some(x) = &x.with_generic_argument.with_generic_argument_opt
    {
        let items: Vec<_> = x.with_generic_argument_list.as_ref().into();
        for item in items {
            if let WithGenericArgumentItem::GenericArgIdentifier(x) = item {
                ret.append(&mut collect_identifier(
                    x.generic_arg_identifier.scoped_identifier.as_ref(),
                ));
            }
        }
    }

    for x in &value.scoped_identifier_list {
        if let Some(x) = &x.scoped_identifier_opt0
            && let Some(x) = &x.with_generic_argument.with_generic_argument_opt
        {
            let items: Vec<_> = x.with_generic_argument_list.as_ref().into();
            for item in items {
                if let WithGenericArgumentItem::GenericArgIdentifier(x) = item {
                    ret.append(&mut collect_identifier(
                        x.generic_arg_identifier.scoped_identifier.as_ref(),
                    ));
                }
            }
        }
    }

    ret
}

pub fn check_generic_bound(context: &mut Context, value: &WithGenericParameter) {
    let items: Vec<_> = value.with_generic_parameter_list.as_ref().into();

    for item in items {
        match item.generic_bound.as_ref() {
            GenericBound::InstScopedIdentifier(x) => {
                let items = collect_identifier(&x.scoped_identifier);

                for item in &items {
                    if let Ok(symbol) = symbol_table::resolve(item)
                        && !symbol.found.is_proto_interface(false, true)
                    {
                        context.insert_error(AnalyzerError::mismatch_type(
                            &symbol.found.token.to_string(),
                            "proto module, proto interface or non generic interface",
                            &symbol.found.kind.to_kind_name(),
                            &item.identifier().token.into(),
                        ));
                    }
                }
            }
            GenericBound::GenericProtoBound(x) => {
                if let GenericProtoBound::ScopedIdentifier(x) = x.generic_proto_bound.as_ref() {
                    let items = collect_identifier(&x.scoped_identifier);

                    for item in &items {
                        if let Ok(symbol) = symbol_table::resolve(item) {
                            let is_valid = symbol.found.is_proto_module(false)
                                || symbol.found.is_proto_interface(false, false)
                                || symbol.found.is_proto_package(false)
                                || symbol.found.is_variable_type();
                            if !is_valid {
                                context.insert_error(AnalyzerError::mismatch_type(
                                    &symbol.found.token.to_string(),
                                    "proto module, proto interface, proto package or variable type",
                                    &symbol.found.kind.to_kind_name(),
                                    &item.identifier().token.into(),
                                ));
                            }
                        }
                    }
                }
            }
            _ => (),
        }
    }
}

fn is_referable_generic_arg_symbol(symbol: &Symbol, defined_namesapce: &Namespace) -> bool {
    match &symbol.kind {
        SymbolKind::Package(_)
        | SymbolKind::ProtoPackage(_)
        | SymbolKind::GenericParameter(_)
        | SymbolKind::SystemVerilog => {
            return true;
        }
        SymbolKind::GenericInstance(x) => {
            return symbol_table::get(x.base)
                .map(|x| is_referable_generic_arg_symbol(&x, defined_namesapce))
                .unwrap_or(false);
        }
        _ => {
            if symbol.is_variable_type()
                || matches!(
                    symbol.kind,
                    SymbolKind::Parameter(_) | SymbolKind::ProtoConst(_) | SymbolKind::Instance(_)
                )
            {
                if symbol.namespace.matched(defined_namesapce) {
                    return true;
                } else if let Some(parent) = symbol.get_parent() {
                    return is_referable_generic_arg_symbol(&parent, defined_namesapce);
                }
            }
        }
    }

    false
}

fn is_referable_generic_arg(full_path: &[SymbolId], defined_namesapce: &Namespace) -> bool {
    full_path
        .iter()
        .map(|x| symbol_table::get(*x).unwrap())
        .any(|x| is_referable_generic_arg_symbol(&x, defined_namesapce))
}

fn check_generic_type_arg(
    arg: &GenericSymbolPath,
    namespace: &Namespace,
    base: &Symbol,
) -> Option<AnalyzerError> {
    if arg.kind == GenericSymbolPathKind::TypeLiteral {
        None
    } else if arg.is_resolvable() {
        let Ok(symbol) = symbol_table::resolve((&arg.generic_path(), namespace)) else {
            // Undefiend identifier has been checked at 'analyze_post_pass1' phase
            return None;
        };

        if symbol.found.is_variable_type() {
            if is_referable_generic_arg_symbol(&symbol.found, &base.namespace) {
                return None;
            }

            Some(AnalyzerError::unresolvable_generic_argument(
                &arg.to_string(),
                &arg.range,
                &base.token.into(),
            ))
        } else {
            Some(AnalyzerError::mismatch_type(
                &arg.to_string(),
                "variable type",
                &symbol.found.kind.to_kind_name(),
                &arg.range,
            ))
        }
    } else {
        Some(AnalyzerError::mismatch_type(
            &arg.to_string(),
            "variable type",
            &arg.kind.to_string(),
            &arg.range,
        ))
    }
}

fn check_generic_inst_arg(
    arg: &GenericSymbolPath,
    namespace: &Namespace,
    bound: &GenericBoundKind,
    base: &Symbol,
) -> Option<AnalyzerError> {
    let required = bound.resolve_inst_bound(&base.namespace)?;
    let actual = if arg.is_resolvable() {
        'inst: {
            let arg_symbol = symbol_table::resolve((&arg.generic_path(), namespace)).ok()?;
            let SymbolKind::Instance(inst) = &arg_symbol.found.kind else {
                break 'inst arg_symbol.found.kind.to_kind_name();
            };
            if !is_referable_generic_arg_symbol(&arg_symbol.found, &base.namespace) {
                return Some(AnalyzerError::unresolvable_generic_argument(
                    &arg.to_string(),
                    &arg.range,
                    &base.token.into(),
                ));
            }

            let Ok(type_symbol) =
                symbol_table::resolve((&inst.type_name.mangled_path(), namespace))
            else {
                return None;
            };

            let Some(proto_symbol) = type_symbol.found.proto() else {
                break 'inst type_symbol.found.kind.to_kind_name();
            };

            if proto_symbol.id == required.id {
                return None;
            }

            proto_symbol.kind.to_kind_name()
        }
    } else {
        arg.kind.to_string()
    };

    Some(AnalyzerError::mismatch_type(
        &arg.to_string(),
        &format!("inst {}", required.token),
        &actual,
        &arg.range,
    ))
}

fn check_generic_proto_arg(
    arg: &GenericSymbolPath,
    namespace: &Namespace,
    bound: &GenericBoundKind,
    base: &Symbol,
) -> Option<AnalyzerError> {
    let required = bound.resolve_proto_bound(&base.namespace)?;
    let arg_symbol = if arg.is_resolvable() {
        let symbol = symbol_table::resolve((&arg.generic_path(), namespace)).ok();
        if symbol.is_some() {
            symbol
        } else {
            return None;
        }
    } else {
        None
    };
    let (arg_type, type_symbol) = if let Some(x) = &arg_symbol {
        let proto_symbol = x.found.proto();
        if proto_symbol.is_some() {
            (None, proto_symbol)
        } else if let Some(r#type) = x.found.kind.get_type() {
            r#type
                .trace_user_defined(Some(&x.found.namespace))
                .map(|(x, y)| (Some(x), y))
                .unwrap_or((None, None))
        } else if let Some(alias_target) = resolve_alias(&x.found) {
            (None, Some(alias_target))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let expected = match &required {
        ProtoBound::ProtoModule(r)
        | ProtoBound::ProtoInterface(r)
        | ProtoBound::ProtoPackage(r) => {
            if type_symbol.as_ref().map(|x| x.id == r.id).unwrap_or(false) {
                None
            } else {
                Some(format!("proto {}", r.token))
            }
        }
        ProtoBound::Enum((r, _)) => {
            let is_matched = if let Some(arg_symbol) = arg_symbol.as_ref() {
                if let SymbolKind::EnumMember(_) = arg_symbol.found.kind {
                    arg_symbol
                        .found
                        .get_parent()
                        .map(|x| x.id == r.id)
                        .unwrap_or(false)
                } else {
                    type_symbol.as_ref().map(|x| x.id == r.id).unwrap_or(false)
                }
            } else {
                false
            };
            if is_matched {
                None
            } else {
                Some(format!("{}", r.token))
            }
        }
        ProtoBound::Struct((r, _)) | ProtoBound::Union((r, _)) => {
            if type_symbol.as_ref().map(|x| x.id == r.id).unwrap_or(false) {
                None
            } else {
                Some(format!("{}", r.token))
            }
        }
        ProtoBound::FactorType(param_type) => {
            let is_matched = if let Some(arg_type) = arg_type.as_ref() {
                match_fixed_type(arg_type, param_type)
            } else if let Some(arg_symbol) = arg_symbol.as_ref() {
                match &arg_symbol.found.kind {
                    SymbolKind::Parameter(_) | SymbolKind::ProtoConst(_) => true,
                    SymbolKind::GenericParameter(x) => x
                        .bound
                        .resolve_proto_bound(&arg_symbol.found.namespace)
                        .map(|x| x.is_variable_type())
                        .unwrap_or(false),
                    _ => false,
                }
            } else {
                // if `arg_symbol` is none,
                // it means that the given generic arg is a literal
                true
            };
            if is_matched {
                None
            } else {
                Some(format!("{param_type}"))
            }
        }
    };

    if let Some(expected) = expected {
        let actual = if let Some(type_symbol) = type_symbol {
            type_symbol.kind.to_kind_name()
        } else {
            arg.kind.to_string()
        };
        return Some(AnalyzerError::mismatch_type(
            &arg.to_string(),
            &expected,
            &actual,
            &arg.range,
        ));
    }

    if required.is_variable_type()
        && !arg_symbol
            .map(|x| is_referable_generic_arg(&x.full_path, &base.namespace))
            .unwrap_or(true)
    {
        return Some(AnalyzerError::unresolvable_generic_argument(
            &arg.to_string(),
            &arg.range,
            &base.token.into(),
        ));
    }

    None
}

fn resolve_alias(symbol: &Symbol) -> Option<Symbol> {
    let target_path = symbol.alias_target(true)?;
    let target_symbol =
        symbol_table::resolve((&target_path.generic_path(), &symbol.namespace)).ok()?;
    if let Some(proto) = target_symbol.found.proto() {
        Some(proto)
    } else {
        Some(target_symbol.found)
    }
}

fn match_fixed_type(arg_type: &SymType, param_type: &SymType) -> bool {
    if arg_type.modifier.len() != param_type.modifier.len() {
        return false;
    }

    for i in 0..arg_type.modifier.len() {
        if arg_type.modifier[i].kind != param_type.modifier[i].kind {
            return false;
        }
    }

    if !arg_type.width.is_empty() || !arg_type.array.is_empty() {
        return false;
    }

    if matches!(param_type.kind, TypeKind::Bool | TypeKind::String) {
        arg_type.kind == param_type.kind
    } else {
        arg_type.kind.is_fixed()
    }
}

pub fn check_generic_args(context: &mut Context, path: &GenericSymbolPath) {
    let namespace = namespace_table::get(path.paths[0].base.id).unwrap();
    for i in 0..path.len() {
        let base_path = path.base_path(i);
        if let Ok(symbol) = symbol_table::resolve((&base_path, &namespace)) {
            let params = symbol.found.generic_parameters();
            let args = &path.paths[i].arguments;

            for (i, arg) in args.iter().enumerate() {
                if let Some(param) = params.get(i) {
                    let bound = &param.1.bound;
                    match bound {
                        GenericBoundKind::Type => {
                            if let Some(err) =
                                check_generic_type_arg(arg, &namespace, &symbol.found)
                            {
                                context.insert_error(err);
                            }
                        }
                        GenericBoundKind::Inst(_) => {
                            if let Some(err) =
                                check_generic_inst_arg(arg, &namespace, bound, &symbol.found)
                            {
                                context.insert_error(err);
                            }
                        }
                        GenericBoundKind::Proto(_) => {
                            if let Some(err) =
                                check_generic_proto_arg(arg, &namespace, bound, &symbol.found)
                            {
                                context.insert_error(err);
                            }
                        }
                    }
                }
            }
        }
    }
}
