use crate::analyzer_error::AnalyzerError;
use crate::namespace_table;
use crate::symbol_table::{self, SymbolPath};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CreateReference<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CreateReference<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CreateReference<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateReference<'a> {
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let namespace = namespace_table::get(arg.identifier.identifier_token.token.id).unwrap();
            let path = SymbolPath::from(arg);
            let symbol = symbol_table::get(&path, &namespace);
            if let Some(symbol) = symbol {
                symbol_table::add_reference(
                    symbol.token.id,
                    &arg.identifier.identifier_token.token,
                );
            } else {
                let is_single_identifier = path.as_slice().len() == 1;
                if is_single_identifier {
                    let name =
                        resource_table::get_str_value(*path.as_slice().last().unwrap()).unwrap();
                    self.errors.push(AnalyzerError::undefined_identifier(
                        &name,
                        self.text,
                        &arg.identifier.identifier_token,
                    ));
                }
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // system function
            if arg.expression_identifier_opt.is_some() {
                return Ok(());
            }

            let namespace = namespace_table::get(arg.identifier.identifier_token.token.id).unwrap();
            let path = SymbolPath::from(arg);
            let symbol = symbol_table::get(&path, &namespace);
            if let Some(symbol) = symbol {
                symbol_table::add_reference(
                    symbol.token.id,
                    &arg.identifier.identifier_token.token,
                );
            } else {
                let is_single_identifier = path.as_slice().len() == 1;
                if is_single_identifier {
                    let name =
                        resource_table::get_str_value(*path.as_slice().last().unwrap()).unwrap();
                    self.errors.push(AnalyzerError::undefined_identifier(
                        &name,
                        self.text,
                        &arg.identifier.identifier_token,
                    ));
                }
            }
        }
        Ok(())
    }

    fn modport_item(&mut self, arg: &ModportItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let namespace = namespace_table::get(arg.identifier.identifier_token.token.id).unwrap();
            let path = SymbolPath::from(arg.identifier.as_ref());
            let symbol = symbol_table::get(&path, &namespace);
            if let Some(symbol) = symbol {
                symbol_table::add_reference(
                    symbol.token.id,
                    &arg.identifier.identifier_token.token,
                );
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let namespace =
                namespace_table::get(arg.identifier0.identifier_token.token.id).unwrap();
            let path = SymbolPath::from(arg.identifier0.as_ref());
            let symbol = symbol_table::get(&path, &namespace);
            if let Some(symbol) = symbol {
                symbol_table::add_reference(
                    symbol.token.id,
                    &arg.identifier0.identifier_token.token,
                );
            }
        }
        Ok(())
    }
}
