use crate::analyzer_error::AnalyzerError;
use crate::symbol_table::{self, SymbolPath};
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
            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    if symbol.found.is_some() {
                        for symbol in symbol.full_path {
                            symbol_table::add_reference(
                                symbol.token.id,
                                &arg.identifier.identifier_token.token,
                            );
                        }
                    } else {
                        let is_single_identifier = SymbolPath::from(arg).as_slice().len() == 1;
                        if is_single_identifier {
                            let name = arg.identifier.identifier_token.text();
                            self.errors.push(AnalyzerError::undefined_identifier(
                                &name,
                                self.text,
                                &arg.identifier.identifier_token,
                            ));
                        }
                    }
                }
                Err(err) => {
                    let name = format!("{}", err.last_found.token.text);
                    let member = format!("{}", err.not_found);
                    self.errors.push(AnalyzerError::unknown_member(
                        &name,
                        &member,
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

            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    if symbol.found.is_some() {
                        for symbol in symbol.full_path {
                            symbol_table::add_reference(
                                symbol.token.id,
                                &arg.identifier.identifier_token.token,
                            );
                        }
                    } else {
                        let is_single_identifier = SymbolPath::from(arg).as_slice().len() == 1;
                        if is_single_identifier {
                            let name = arg.identifier.identifier_token.text();
                            self.errors.push(AnalyzerError::undefined_identifier(
                                &name,
                                self.text,
                                &arg.identifier.identifier_token,
                            ));
                        }
                    }
                }
                Err(err) => {
                    let name = format!("{}", err.last_found.token.text);
                    let member = format!("{}", err.not_found);
                    self.errors.push(AnalyzerError::unknown_member(
                        &name,
                        &member,
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
            match symbol_table::resolve(arg.identifier.as_ref()) {
                Ok(symbol) => {
                    if symbol.found.is_some() {
                        for symbol in symbol.full_path {
                            symbol_table::add_reference(
                                symbol.token.id,
                                &arg.identifier.identifier_token.token,
                            );
                        }
                    }
                }
                Err(err) => {
                    let name = format!("{}", err.last_found.token.text);
                    let member = format!("{}", err.not_found);
                    self.errors.push(AnalyzerError::unknown_member(
                        &name,
                        &member,
                        self.text,
                        &arg.identifier.identifier_token,
                    ));
                }
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                Ok(symbol) => {
                    if symbol.found.is_some() {
                        for symbol in symbol.full_path {
                            symbol_table::add_reference(
                                symbol.token.id,
                                &arg.scoped_identifier.identifier.identifier_token.token,
                            );
                        }
                    }
                }
                Err(err) => {
                    let name = format!("{}", err.last_found.token.text);
                    let member = format!("{}", err.not_found);
                    self.errors.push(AnalyzerError::unknown_member(
                        &name,
                        &member,
                        self.text,
                        &arg.scoped_identifier.identifier.identifier_token,
                    ));
                }
            }
        }
        Ok(())
    }
}
