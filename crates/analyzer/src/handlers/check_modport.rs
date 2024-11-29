use crate::analyzer_error::AnalyzerError;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckModport<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckModport<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
        }
    }
}

impl Handler for CheckModport<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckModport<'_> {
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
                                    self.text,
                                    &arg.identifier.as_ref().into(),
                                ));
                        }
                    }
                    _ => {
                        if !matches!(symbol.found.kind, SymbolKind::Variable(_)) {
                            self.errors
                                .push(AnalyzerError::invalid_modport_variable_item(
                                    &arg.identifier.identifier_token.token.to_string(),
                                    self.text,
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
