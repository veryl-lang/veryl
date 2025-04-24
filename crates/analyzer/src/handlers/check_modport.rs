use crate::analyzer_error::AnalyzerError;
use crate::attribute::ExpandItem;
use crate::attribute_table;
use crate::namespace::Namespace;
use crate::symbol::Direction as SymDirection;
use crate::symbol::{Symbol, SymbolKind, TypeKind};
use crate::symbol_path::SymbolPathNamespace;
use crate::symbol_table;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckModport {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    interface_namespace: Option<Namespace>,
}

impl CheckModport {
    pub fn new() -> Self {
        Self::default()
    }

    fn is_function_defined_in_interface(&self, symbol: &Symbol) -> bool {
        if let Some(namespace) = self.interface_namespace.as_ref() {
            matches!(symbol.kind, SymbolKind::Function(_)) && symbol.namespace.matched(namespace)
        } else {
            false
        }
    }
}

impl Handler for CheckModport {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn is_unexpandable_modport(symbol: &Symbol) -> bool {
    if let SymbolKind::Port(x) = &symbol.kind {
        if !matches!(x.direction, SymDirection::Modport) {
            return false;
        }

        match &x.r#type.kind {
            TypeKind::UserDefined(x) => {
                let Ok(symbol) = symbol_table::resolve((&x.path.generic_path(), &symbol.namespace))
                else {
                    return false;
                };

                if let SymbolKind::Modport(modport) = &symbol.found.kind {
                    if let Some(symbol) = symbol_table::get(modport.interface) {
                        let SymbolKind::Interface(x) = symbol.kind else {
                            unreachable!()
                        };
                        if !x.parameters.is_empty() {
                            return true;
                        }
                    }
                }
            }
            TypeKind::AbstractInterface(_) => return true,
            _ => {}
        }
    }

    false
}

impl VerylGrammarTrait for CheckModport {
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if matches!(self.point, HandlerPoint::Before)
            && attribute_table::is_expand(
                &arg.identifier.identifier_token.token,
                ExpandItem::Modport,
            )
        {
            if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                if is_unexpandable_modport(&symbol.found) {
                    self.errors.push(AnalyzerError::unexpandable_modport(
                        &arg.identifier.identifier_token.token.to_string(),
                        &arg.identifier.as_ref().into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.interface_namespace = symbol_table::resolve(arg.identifier.as_ref())
                .ok()
                .map(|x| x.found.inner_namespace());
        }
        Ok(())
    }

    fn modport_item(&mut self, arg: &ModportItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let mut path: SymbolPathNamespace = arg.identifier.as_ref().into();
            path.pop_namespace();

            if let Ok(symbol) = symbol_table::resolve(path) {
                match &*arg.direction {
                    Direction::Modport(_) => {}
                    Direction::Import(_) => {
                        if !self.is_function_defined_in_interface(&symbol.found) {
                            self.errors
                                .push(AnalyzerError::invalid_modport_function_item(
                                    &arg.identifier.identifier_token.token.to_string(),
                                    &arg.identifier.as_ref().into(),
                                ));
                        }
                    }
                    _ => {
                        if !matches!(symbol.found.kind, SymbolKind::Variable(_)) {
                            self.errors
                                .push(AnalyzerError::invalid_modport_variable_item(
                                    &arg.identifier.identifier_token.token.to_string(),
                                    &arg.identifier.as_ref().into(),
                                ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
