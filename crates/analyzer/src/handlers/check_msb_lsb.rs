use crate::analyzer_error::AnalyzerError;
use crate::msb_table;
use crate::namespace_table;
use crate::symbol::SymbolKind;
use crate::symbol_table::{self, ResolveSymbol, SymbolPath, SymbolPathNamespace};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckMsbLsb<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    identifier_path: Vec<SymbolPathNamespace>,
    select_dimension: Vec<usize>,
    in_expression_identifier: bool,
    in_select: bool,
}

impl<'a> CheckMsbLsb<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            identifier_path: Vec::new(),
            select_dimension: Vec::new(),
            in_expression_identifier: false,
            in_select: false,
        }
    }
}

impl<'a> Handler for CheckMsbLsb<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckMsbLsb<'a> {
    fn lsb(&mut self, arg: &Lsb) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if !(self.in_expression_identifier && self.in_select) {
                self.errors
                    .push(AnalyzerError::invalid_lsb(self.text, &arg.lsb_token));
            }
        }
        Ok(())
    }

    fn msb(&mut self, arg: &Msb) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_expression_identifier && self.in_select {
                let resolved = if let Ok(x) =
                    symbol_table::resolve(self.identifier_path.last().unwrap().clone())
                {
                    if let ResolveSymbol::Symbol(x) = x.found {
                        if let SymbolKind::Variable(x) = x.kind {
                            let select_dimension = *self.select_dimension.last().unwrap();
                            let expression = if select_dimension >= x.r#type.array.len() {
                                &x.r#type.width[select_dimension - x.r#type.array.len()]
                            } else {
                                &x.r#type.array[select_dimension]
                            };
                            msb_table::insert(arg.msb_token.token.id, expression);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !resolved {
                    self.errors
                        .push(AnalyzerError::unknown_msb(self.text, &arg.msb_token));
                }
            } else {
                self.errors
                    .push(AnalyzerError::invalid_msb(self.text, &arg.msb_token));
            }
        }
        Ok(())
    }

    fn identifier(&mut self, arg: &Identifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_expression_identifier {
                self.identifier_path
                    .last_mut()
                    .unwrap()
                    .0
                    .push(arg.identifier_token.token.text);
            }
        }
        Ok(())
    }

    fn select(&mut self, _arg: &Select) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_select = true;
            }
            HandlerPoint::After => {
                self.in_select = false;
                if self.in_expression_identifier {
                    *self.select_dimension.last_mut().unwrap() += 1;
                }
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                let namespace =
                    namespace_table::get(arg.identifier.identifier_token.token.id).unwrap();
                let symbol_path = SymbolPath::default();
                self.identifier_path
                    .push(SymbolPathNamespace(symbol_path, namespace));
                self.select_dimension.push(0);
                self.in_expression_identifier = true;
            }
            HandlerPoint::After => {
                self.identifier_path.pop();
                self.select_dimension.pop();
                self.in_expression_identifier = false;
            }
        }
        Ok(())
    }
}
