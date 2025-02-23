use crate::analyzer_error::AnalyzerError;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckModport {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
}

impl CheckModport {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckModport {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckModport {
    fn modport_item(&mut self, arg: &ModportItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                match &*arg.direction {
                    Direction::Ref(_) | Direction::Modport(_) => {}
                    Direction::Import(_) => {
                        if !matches!(symbol.found.kind, SymbolKind::Function(_)) {
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
