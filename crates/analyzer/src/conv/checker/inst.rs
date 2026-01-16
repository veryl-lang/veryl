use crate::analyzer_error::AnalyzerError;
use crate::attribute::AllowItem;
use crate::attribute::Attribute as Attr;
use crate::attribute_table;
use crate::conv::Context;
use crate::symbol::{Symbol, SymbolId, SymbolKind};
use crate::symbol_path::SymbolPathNamespace;
use crate::symbol_table;
use veryl_parser::Stringifier;
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::VerylWalker;

enum InstTypeSource {
    Id(SymbolId),
    Path(SymbolPathNamespace),
}

fn resolve_inst_type(arg: &InstTypeSource) -> Option<Symbol> {
    let symbol = match arg {
        InstTypeSource::Id(x) => symbol_table::get(*x)?,
        InstTypeSource::Path(x) => symbol_table::resolve(x).ok()?.found,
    };

    match &symbol.kind {
        SymbolKind::AliasModule(x) | SymbolKind::ProtoAliasModule(x) => {
            let path: SymbolPathNamespace = (&x.target.generic_path(), &symbol.namespace).into();
            return resolve_inst_type(&InstTypeSource::Path(path));
        }
        SymbolKind::AliasInterface(x) | SymbolKind::ProtoAliasInterface(x) => {
            let path: SymbolPathNamespace = (&x.target.generic_path(), &symbol.namespace).into();
            return resolve_inst_type(&InstTypeSource::Path(path));
        }
        SymbolKind::GenericInstance(x) => {
            return resolve_inst_type(&InstTypeSource::Id(x.base));
        }
        SymbolKind::GenericParameter(x) => {
            let proto = x.bound.resolve_proto_bound(&symbol.namespace)?;
            if let Some(symbol) = proto.get_symbol() {
                return resolve_inst_type(&InstTypeSource::Id(symbol.id));
            }
        }
        _ => {}
    }

    Some(symbol)
}

fn get_inst_type_kind(inst_symbol: &Symbol) -> Option<SymbolKind> {
    if let SymbolKind::Instance(ref x) = inst_symbol.kind
        && let Ok(type_symbol) =
            symbol_table::resolve((&x.type_name.mangled_path(), &inst_symbol.namespace))
    {
        match type_symbol.found.kind {
            SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::SystemVerilog => {
                return Some(type_symbol.found.kind);
            }
            SymbolKind::GenericInstance(ref x) => {
                let base = symbol_table::get(x.base).unwrap();
                return Some(base.kind);
            }
            SymbolKind::GenericParameter(x) => {
                if let Some(proto) = x.bound.resolve_proto_bound(&inst_symbol.namespace) {
                    return proto.get_symbol().map(|x| x.kind);
                }
            }
            _ => {}
        }
    }

    None
}

pub fn check_inst(
    context: &mut Context,
    in_module: bool,
    header_token: &Token,
    arg: &ComponentInstantiation,
) {
    if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref())
        && let Some(x) = get_inst_type_kind(&symbol.found)
        && !matches!(x, SymbolKind::Interface(_) | SymbolKind::SystemVerilog)
        && let Some(x) = &arg.component_instantiation_opt
    {
        context.insert_error(AnalyzerError::invalid_clock_domain(
            &x.clock_domain.as_ref().into(),
        ));
    }

    let mut connected_params = Vec::new();
    if let Some(ref x) = arg.component_instantiation_opt1
        && let Some(ref x) = x.inst_parameter.inst_parameter_opt
    {
        let items: Vec<_> = x.inst_parameter_list.as_ref().into();
        for item in items {
            connected_params.push(item.identifier.text());
        }
    }

    let mut connected_ports = Vec::new();
    if let Some(ref x) = arg.component_instantiation_opt2
        && let Some(ref x) = x.inst_port.inst_port_opt
    {
        let items: Vec<_> = x.inst_port_list.as_ref().into();
        for item in items {
            connected_ports.push(item.identifier.text());
        }
    }

    let path: SymbolPathNamespace = arg.scoped_identifier.as_ref().into();
    if let Some(symbol) = resolve_inst_type(&InstTypeSource::Path(path)) {
        let mut stringifier = Stringifier::new();
        stringifier.scoped_identifier(&arg.scoped_identifier);
        let name = stringifier.as_str();

        let mut params = vec![];
        let mut ports = vec![];
        let mut check_port_connection = false;

        let type_expected = match symbol.kind {
            SymbolKind::Module(ref x) if in_module => {
                params.append(&mut x.parameters.clone());
                ports.append(&mut x.ports.clone());
                check_port_connection = true;
                None
            }
            SymbolKind::ProtoModule(ref x) if in_module => {
                params.append(&mut x.parameters.clone());
                ports.append(&mut x.ports.clone());
                check_port_connection = true;
                None
            }
            SymbolKind::Interface(ref x) => {
                params.append(&mut x.parameters.clone());
                check_port_connection = true;
                None
            }
            SymbolKind::ProtoInterface(ref x) => {
                params.append(&mut x.parameters.clone());
                check_port_connection = true;
                None
            }
            SymbolKind::SystemVerilog => None,
            _ => {
                if in_module {
                    Some("module or interface")
                } else {
                    Some("interface")
                }
            }
        };

        if let Some(expected) = type_expected {
            context.insert_error(AnalyzerError::mismatch_type(
                name,
                expected,
                &symbol.kind.to_kind_name(),
                &arg.identifier.as_ref().into(),
            ));
        }

        if check_port_connection {
            for port in &ports {
                if !connected_ports.contains(&port.name())
                    && port.property().default_value.is_none()
                    && !attribute_table::contains(header_token, Attr::Allow(AllowItem::MissingPort))
                {
                    let port = resource_table::get_str_value(port.name()).unwrap();
                    context.insert_error(AnalyzerError::missing_port(
                        name,
                        &port,
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
            for param in &connected_params {
                if !params.iter().any(|x| &x.name == param) {
                    let param = resource_table::get_str_value(*param).unwrap();
                    context.insert_error(AnalyzerError::unknown_param(
                        name,
                        &param,
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
            for port in &connected_ports {
                if !ports.iter().any(|x| &x.name() == port) {
                    let port = resource_table::get_str_value(*port).unwrap();
                    context.insert_error(AnalyzerError::unknown_port(
                        name,
                        &port,
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
        }
    }
}
