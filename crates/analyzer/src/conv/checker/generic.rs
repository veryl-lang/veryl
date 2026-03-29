use crate::conv::utils::{TypePosition, eval_factor_path, eval_generic_expr};
use crate::conv::{Context, Conv};
use crate::ir::{self, Comptime, IrResult, Type, VarPathSelect};
use crate::namespace::Namespace;
use crate::symbol::{GenericBoundKind, ProtoBound, Symbol, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use crate::{AnalyzerError, namespace_table, symbol_table};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

pub fn check_generic_bound_item(
    context: &mut Context,
    bound: &GenericBoundKind,
    namespace: &Namespace,
) {
    match bound {
        GenericBoundKind::Inst(path) => {
            if let Err(Some(id)) = bound.resolve_inst_bound(namespace) {
                let symbol = symbol_table::get(id).unwrap();
                context.insert_error(AnalyzerError::mismatch_type(
                    &symbol.token.to_string(),
                    "proto module, proto interface or non generic interface",
                    &symbol.kind.to_kind_name(),
                    &path.range,
                ));
            }
        }
        GenericBoundKind::Proto(r#type) => {
            if let Err(Some(id)) = bound.resolve_proto_bound(namespace) {
                let symbol = symbol_table::get(id).unwrap();
                context.insert_error(AnalyzerError::mismatch_type(
                    &symbol.token.to_string(),
                    "proto module, proto interface, proto package or variable type",
                    &symbol.kind.to_kind_name(),
                    &r#type.token,
                ));
            }
        }
        _ => {}
    }
}

pub fn check_generic_bound(context: &mut Context, value: &WithGenericParameter) {
    let items: Vec<_> = value.with_generic_parameter_list.as_ref().into();

    for item in items {
        let _: IrResult<()> = Conv::conv(context, item);
    }
}

fn check_referable_generic_expression(
    context: &Context,
    expression: &Expression,
    base: Option<&Symbol>,
    token: TokenRange,
) -> Option<AnalyzerError> {
    let mut paths = collect_symbol_paths(expression);
    for path in paths.drain(..) {
        let mut path = context.resolve_path(path);
        path.unalias();
        let error = check_referable_path(&path, base, token);
        if error.is_some() {
            return error;
        }
    }

    None
}

fn collect_symbol_paths(expression: &Expression) -> Vec<GenericSymbolPath> {
    let factors = expression.collect_factors();
    factors
        .iter()
        .filter_map(|factor| match factor {
            Factor::Number(x) => Some(x.number.as_ref().into()),
            Factor::BooleanLiteral(x) => Some(x.boolean_literal.as_ref().into()),
            Factor::IdentifierFactor(x) => {
                Some(x.identifier_factor.expression_identifier.as_ref().into())
            }
            Factor::FactorTypeFactor(x) => {
                match x.factor_type_factor.factor_type.factor_type_group.as_ref() {
                    FactorTypeGroup::FixedType(x) => Some(x.fixed_type.as_ref().into()),
                    FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                        Some((x.variable_type.as_ref(), &vec![]).into())
                    }
                }
            }
            _ => None,
        })
        .collect()
}

fn check_referable_path(
    path: &GenericSymbolPath,
    base: Option<&Symbol>,
    token: TokenRange,
) -> Option<AnalyzerError> {
    if !path.is_resolvable() || base.map(|x| x.is_unbound_function()).unwrap_or(false) {
        return None;
    }

    let is_referable = if let Ok(symbol) = symbol_table::resolve(path) {
        if symbol.found.is_component(true) {
            true
        } else {
            symbol.full_path.iter().any(|x| {
                let symbol = symbol_table::get(*x).unwrap();
                is_referable_symbol(&symbol, base)
            })
        }
    } else {
        true
    };

    if !is_referable {
        Some(AnalyzerError::unresolvable_generic_expression(
            &path.to_string(),
            &path.range,
            &token,
        ))
    } else {
        None
    }
}

fn is_referable_symbol(symbol: &Symbol, base: Option<&Symbol>) -> bool {
    match &symbol.kind {
        SymbolKind::Package(_)
        | SymbolKind::ProtoPackage(_)
        | SymbolKind::GenericParameter(_)
        | SymbolKind::GenericConst(_)
        | SymbolKind::SystemVerilog => {
            return true;
        }
        SymbolKind::GenericInstance(x) => {
            return symbol_table::get(x.base)
                .map(|x| is_referable_symbol(&x, base))
                .unwrap_or(false);
        }
        _ => {
            if symbol.is_variable_type()
                || matches!(
                    symbol.kind,
                    SymbolKind::Parameter(_) | SymbolKind::ProtoConst(_) | SymbolKind::Instance(_)
                )
            {
                if let Some(base) = base
                    && symbol.namespace.matched(&base.namespace)
                {
                    return true;
                } else if let Some(parent) = symbol.get_parent() {
                    return is_referable_symbol(&parent, base);
                }
            }
        }
    }

    false
}

fn check_generic_type_arg(arg: &Comptime) -> Option<AnalyzerError> {
    let dst = Type {
        kind: crate::ir::TypeKind::Type,
        ..Default::default()
    };

    if dst.compatible(arg) {
        None
    } else {
        let src_type = arg.r#type.to_string();
        Some(AnalyzerError::mismatch_type(
            &arg.token.end.to_string(),
            "variable type",
            &src_type,
            &arg.token,
        ))
    }
}

fn check_generic_inst_arg(
    arg: &Comptime,
    bound: &GenericBoundKind,
    bound_namespace: &Namespace,
) -> Option<AnalyzerError> {
    let bound_symbol = bound.resolve_inst_bound(bound_namespace).ok()?;

    if !arg.r#type.is_interface_instance() {
        return Some(AnalyzerError::mismatch_type(
            &arg.token.end.to_string(),
            &format!("inst {}", bound_symbol.token),
            &arg.r#type.to_string(),
            &arg.token,
        ));
    }

    let proto_symbol = if let Some(sig) = arg.r#type.kind.signature() {
        let component = symbol_table::get(sig.symbol).unwrap();
        let Some(proto) = component.proto() else {
            return Some(AnalyzerError::mismatch_type(
                &arg.token.end.to_string(),
                &format!("inst {}", bound_symbol.token),
                &component.kind.to_kind_name(),
                &arg.token,
            ));
        };
        proto
    } else {
        unreachable!()
    };

    if proto_symbol.id != bound_symbol.id {
        return Some(AnalyzerError::mismatch_type(
            &arg.token.end.to_string(),
            &format!("inst {}", bound_symbol.token),
            &proto_symbol.kind.to_kind_name(),
            &arg.token,
        ));
    }

    None
}

fn check_generic_proto_arg(
    context: &mut Context,
    arg: &Comptime,
    bound: &GenericBoundKind,
    bound_namespace: &Namespace,
) -> Option<AnalyzerError> {
    let proto_bound = bound.resolve_proto_bound(bound_namespace).ok()?;
    let result = match &proto_bound {
        ProtoBound::ProtoModule(r)
        | ProtoBound::ProtoInterface(r)
        | ProtoBound::ProtoPackage(r) => {
            let proto = match &arg.r#type.kind {
                ir::TypeKind::Module(sig)
                | ir::TypeKind::Interface(sig)
                | ir::TypeKind::Package(sig) => {
                    let component = symbol_table::get(sig.symbol).unwrap();
                    component.proto()
                }
                _ => None,
            };
            if let Some(proto) = &proto
                && proto.id == r.id
            {
                None
            } else {
                let actual = proto.map(|x| x.kind.to_kind_name());
                Some((format!("proto {}", r.token), actual))
            }
        }
        ProtoBound::Enum((r, _)) | ProtoBound::Struct((r, _)) | ProtoBound::Union((r, _)) => {
            let is_matches = match &arg.r#type.kind {
                ir::TypeKind::Enum(x) => x.id == r.id,
                ir::TypeKind::Struct(x) => x.id == r.id,
                ir::TypeKind::Union(x) => x.id == r.id,
                _ => false,
            };
            if is_matches {
                None
            } else {
                Some((format!("{}", r.token), None))
            }
        }
        ProtoBound::FactorType(r) => {
            let param_type = r.to_ir_type(context, TypePosition::Generic).ok()?;
            if param_type.compatible(arg) {
                None
            } else {
                Some((format!("{}", r), None))
            }
        }
    };

    if let Some((expected, actual)) = result {
        let actual = actual.unwrap_or(arg.r#type.to_string());
        Some(AnalyzerError::mismatch_type(
            &arg.token.end.to_string(),
            &expected,
            &actual,
            &arg.token,
        ))
    } else {
        None
    }
}

fn eval_generic_arg(context: &mut Context, path: &GenericSymbolPath) -> Option<Comptime> {
    let allow_component_as_factor = context.allow_component_as_factor;

    context.allow_component_as_factor = true;
    let range = path.range;
    let ret = eval_factor_path(
        context,
        path.clone(),
        VarPathSelect::default(),
        false,
        range,
    );
    context.allow_component_as_factor = allow_component_as_factor;

    ret.map(|x| x.comptime().clone()).ok()
}

pub fn check_generic_refereence(context: &mut Context, path: &GenericSymbolPath) {
    let namespace = namespace_table::get(path.paths[0].base.id).unwrap_or_default();
    for i in 0..path.len() {
        let base_path = path.base_path(i);
        if let Ok(symbol) = symbol_table::resolve((&base_path, &namespace)) {
            let params = symbol.found.generic_parameters();
            let args = &path.paths[i].arguments;

            if context.in_unbound_func.is_some()
                && !params.is_empty()
                && !symbol.found.is_unbound_function()
            {
                let definition_token = context.in_unbound_func.unwrap();
                context.insert_error(AnalyzerError::unresolvable_generic_expression(
                    &path.to_string(),
                    &path.range,
                    &definition_token.into(),
                ));
            }

            for (i, arg) in args.iter().enumerate() {
                let Some(param) = params.get(i) else {
                    continue;
                };
                let bound = &param.1.bound;

                let mut arg = context.resolve_path(arg.clone());
                arg.unalias();

                if let Some(error) =
                    check_referable_path(&arg, Some(&symbol.found), symbol.found.token.into())
                {
                    context.insert_error(error);
                    continue;
                }

                if let Some(expr) = eval_generic_arg(context, &arg).as_ref() {
                    let error = match bound {
                        GenericBoundKind::Type => check_generic_type_arg(expr),
                        GenericBoundKind::Inst(_) => {
                            check_generic_inst_arg(expr, bound, &symbol.found.namespace)
                        }
                        GenericBoundKind::Proto(_) => {
                            check_generic_proto_arg(context, expr, bound, &symbol.found.namespace)
                        }
                    };
                    if let Some(error) = error {
                        context.insert_error(error);
                    }
                }
            }
        }
    }
}

pub fn check_generic_expression(
    context: &mut Context,
    expression: &Expression,
    bound: &GenericBoundKind,
    base: Option<&Symbol>,
    token: TokenRange,
) {
    if let Some(error) = check_referable_generic_expression(context, expression, base, token) {
        context.insert_error(error);
        return;
    }

    let Some((expression, _)) = eval_generic_expr(context, expression).ok() else {
        return;
    };

    let bound_namespace = if let Some(base) = base {
        &base.namespace
    } else if let Some(namespce) = context.currnet_namespace() {
        &namespce.clone()
    } else {
        &Namespace::default()
    };

    let error = match bound {
        GenericBoundKind::Type => check_generic_type_arg(&expression),
        GenericBoundKind::Inst(_) => check_generic_inst_arg(&expression, bound, bound_namespace),
        GenericBoundKind::Proto(_) => {
            check_generic_proto_arg(context, &expression, bound, bound_namespace)
        }
    };
    if let Some(error) = error {
        context.insert_error(error);
    }
}
