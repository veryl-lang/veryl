use crate::analyzer_error::{AnalyzerError, InvalidModportItemKind};
use crate::attribute::ExpandItem;
use crate::attribute_table;
use crate::conv::{Affiliation, Context};
use crate::symbol::Direction as SymDirection;
use crate::symbol::{Symbol, SymbolKind, TypeKind};
use crate::symbol_path::SymbolPathNamespace;
use crate::symbol_table;
use veryl_parser::resource_table::StrId;
use veryl_parser::veryl_grammar_trait::*;

fn is_target_modport(context: &mut Context, identifier: &Identifier) -> bool {
    context.is_affiliated(Affiliation::Function)
        || attribute_table::is_expand(
            &identifier.identifier_token.token.into(),
            ExpandItem::Modport,
        )
}

fn is_unexpandable_modport(context: &mut Context, symbol: &Symbol) -> bool {
    if let SymbolKind::Port(x) = &symbol.kind {
        if !matches!(x.direction, SymDirection::Modport) {
            return false;
        }

        let port_type = &x.r#type;
        match &x.r#type.kind {
            TypeKind::UserDefined(x) => {
                let Ok(symbol) = symbol_table::resolve((&x.path.generic_path(), &symbol.namespace))
                else {
                    return false;
                };

                if let SymbolKind::Modport(modport) = &symbol.found.kind
                    && let Some(symbol) = symbol_table::get(modport.interface)
                {
                    let SymbolKind::Interface(x) = symbol.kind else {
                        unreachable!()
                    };

                    let is_expandable = x.parameters.is_empty()
                        && (!context.is_affiliated(Affiliation::Function)
                            || port_type.array.is_empty());
                    if !is_expandable {
                        return true;
                    }
                }
            }
            TypeKind::AbstractInterface(_) => return true,
            _ => {}
        }
    }

    false
}

fn is_function_defined_in_interface(symbol: &Symbol) -> bool {
    match &symbol.kind {
        SymbolKind::Function(x) => x.affiliation == Affiliation::Interface,
        SymbolKind::ProtoFunction(x) => x.affiliation == Affiliation::Interface,
        _ => false,
    }
}

pub fn check_modport_in_port(context: &mut Context, arg: &PortDeclarationItem) {
    if is_target_modport(context, &arg.identifier)
        && let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref())
        && is_unexpandable_modport(context, &symbol.found)
    {
        context.insert_error(AnalyzerError::unexpandable_modport(
            &arg.identifier.identifier_token.token.to_string(),
            &arg.identifier.as_ref().into(),
        ));
    }
}

pub fn check_modport(context: &mut Context, arg: &ModportItem) {
    let mut path: SymbolPathNamespace = arg.identifier.as_ref().into();
    path.pop_namespace();

    if let Ok(symbol) = symbol_table::resolve(path) {
        match &*arg.direction {
            Direction::Modport(_) => {}
            Direction::Import(_) => {
                if !is_function_defined_in_interface(&symbol.found) {
                    context.insert_error(AnalyzerError::invalid_modport_item(
                        InvalidModportItemKind::Function,
                        &arg.identifier.identifier_token.token.to_string(),
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
            _ => {
                if !matches!(symbol.found.kind, SymbolKind::Variable(_)) {
                    context.insert_error(AnalyzerError::invalid_modport_item(
                        InvalidModportItemKind::Variable,
                        &arg.identifier.identifier_token.token.to_string(),
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
        }
    }
}

pub fn check_modport_default(context: &mut Context, arg: &ModportDefault, name: StrId) {
    let default_member_identifier = match arg {
        ModportDefault::ConverseLParenIdentifierRParen(x) => x.identifier.as_ref(),
        ModportDefault::SameLParenIdentifierRParen(x) => x.identifier.as_ref(),
        _ => return,
    };
    let Ok(symbol) = symbol_table::resolve(default_member_identifier) else {
        return;
    };

    if !matches!(symbol.found.kind, SymbolKind::Modport(_)) {
        // Check modport default member type
        context.insert_error(AnalyzerError::mismatch_type(
            &symbol.found.token.to_string(),
            "modport",
            &symbol.found.kind.to_kind_name(),
            &default_member_identifier.identifier_token.token.into(),
        ));
    } else if symbol.found.token.text == name {
        // Check self reference
        context.insert_error(AnalyzerError::mismatch_type(
            &symbol.found.token.to_string(),
            "other modport",
            "ownself",
            &default_member_identifier.identifier_token.token.into(),
        ));
    }
}
