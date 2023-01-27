use crate::analyzer_error::AnalyzerError;
use crate::symbol::SymbolKind;
use crate::symbol_table::{self, SymbolPath};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckFunction<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckFunction<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
        }
    }
}

impl<'a> Handler for CheckFunction<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckFunction<'a> {
    fn factor(&mut self, arg: &Factor) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Factor::ExpressionIdentifierFactorOpt(x) = arg {
                // skip system function
                if x.factor_opt.is_some() {
                    return Ok(());
                }

                if let Ok(symbol) = symbol_table::resolve(x.expression_identifier.as_ref()) {
                    let arity = if let Some(symbol) = symbol.found {
                        if let SymbolKind::Function(x) = symbol.kind {
                            Some(x.ports.len())
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let mut args = 0;
                    if let Some(ref x) = x.factor_opt {
                        if let Some(ref x) = x.function_call.function_call_opt {
                            args += 1;
                            args += x.argument_list.argument_list_list.len();
                        }
                    }

                    if let Some(arity) = arity {
                        if arity != args {
                            let name = format!(
                                "{}",
                                SymbolPath::from(x.expression_identifier.as_ref())
                                    .as_slice()
                                    .last()
                                    .unwrap()
                            );
                            self.errors.push(AnalyzerError::mismatch_arity(
                                &name,
                                arity,
                                args,
                                self.text,
                                &x.expression_identifier.identifier.identifier_token,
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
