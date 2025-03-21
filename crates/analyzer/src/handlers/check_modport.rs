use crate::analyzer_error::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{Symbol, SymbolKind};
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
        let namespace = self.interface_namespace.as_ref().unwrap();
        matches!(symbol.kind, SymbolKind::Function(_)) && symbol.namespace.matched(namespace)
    }
}

impl Handler for CheckModport {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckModport {
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.interface_namespace =
                namespace_table::get(arg.identifier.identifier_token.token.id);
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
