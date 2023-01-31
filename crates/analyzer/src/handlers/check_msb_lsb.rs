use crate::analyzer_error::AnalyzerError;
use crate::msb_table;
use crate::namespace_table;
use crate::symbol::SymbolKind;
use crate::symbol_table::{self, SymbolPath, SymbolPathNamespace};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckMsbLsb<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    identifier_path: Vec<SymbolPathNamespace>,
    range_dimension: Vec<usize>,
    in_expression_identifier: bool,
    in_range: bool,
}

impl<'a> CheckMsbLsb<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            identifier_path: Vec::new(),
            range_dimension: Vec::new(),
            in_expression_identifier: false,
            in_range: false,
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
            if !(self.in_expression_identifier && self.in_range) {
                self.errors
                    .push(AnalyzerError::invalid_lsb(self.text, &arg.lsb_token));
            }
        }
        Ok(())
    }

    fn msb(&mut self, arg: &Msb) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_expression_identifier && self.in_range {
                let resolved = if let Ok(x) =
                    symbol_table::resolve(self.identifier_path.last().unwrap().clone())
                {
                    if let Some(x) = x.found {
                        if let SymbolKind::Variable(x) = x.kind {
                            let range_dimension = *self.range_dimension.last().unwrap();
                            let expression = if range_dimension >= x.r#type.array.len() {
                                &x.r#type.width[range_dimension - x.r#type.array.len()]
                            } else {
                                &x.r#type.array[range_dimension]
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

    fn range(&mut self, _arg: &Range) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_range = true;
            }
            HandlerPoint::After => {
                self.in_range = false;
                if self.in_expression_identifier {
                    *self.range_dimension.last_mut().unwrap() += 1;
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
                self.range_dimension.push(0);
                self.in_expression_identifier = true;
            }
            HandlerPoint::After => {
                self.identifier_path.pop();
                self.range_dimension.pop();
                self.in_expression_identifier = false;
            }
        }
        Ok(())
    }
}
