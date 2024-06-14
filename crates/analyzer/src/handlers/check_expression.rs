use crate::analyzer_error::AnalyzerError;
use crate::evaluator::{Evaluated, Evaluator};
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::TokenRange;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckExpression<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    case_condition_depth: usize,
    evaluator: Evaluator,
}

impl<'a> CheckExpression<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckExpression<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckExpression<'a> {
    fn case_condition(&mut self, _arg: &CaseCondition) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.case_condition_depth += 1,
            HandlerPoint::After => self.case_condition_depth -= 1,
        }
        Ok(())
    }

    fn expression(&mut self, arg: &Expression) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.case_condition_depth >= 1 {
                let result = matches!(self.evaluator.expression(arg), Evaluated::Variable { .. });
                if result {
                    self.errors
                        .push(AnalyzerError::invalid_case_condition_non_elaborative(
                            self.text,
                            &arg.into(),
                        ));
                }
            }
        }
        Ok(())
    }

    fn factor(&mut self, arg: &Factor) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Factor::ExpressionIdentifierFactorOpt(x) = arg {
                let expid = x.expression_identifier.as_ref();
                if let Ok(rr) = symbol_table::resolve(expid) {
                    let identifier = rr.found.token.to_string();
                    let token: TokenRange = x.expression_identifier.as_ref().into();
                    match rr.found.kind {
                        SymbolKind::Function(_)
                        | SymbolKind::ModportFunctionMember(_)
                        | SymbolKind::SystemFunction => {
                            if x.factor_opt.is_none() {
                                self.errors.push(AnalyzerError::invalid_factor(
                                    &identifier,
                                    &rr.found.kind.to_kind_name(),
                                    self.text,
                                    &token,
                                ));
                            }
                        }
                        SymbolKind::Module(_)
                        | SymbolKind::Interface(_)
                        | SymbolKind::Instance(_)
                        | SymbolKind::Block
                        | SymbolKind::Package(_)
                        | SymbolKind::TypeDef(_)
                        | SymbolKind::Enum(_)
                        | SymbolKind::Modport(_)
                        | SymbolKind::Namespace
                        | SymbolKind::GenericInstance(_) => {
                            self.errors.push(AnalyzerError::invalid_factor(
                                &identifier,
                                &rr.found.kind.to_kind_name(),
                                self.text,
                                &token,
                            ));
                        }
                        _ => {}
                    }
                }

                if x.factor_opt.is_some() {
                    // Must be a function call
                    let expid = x.expression_identifier.as_ref();
                    if let Ok(rr) = symbol_table::resolve(expid) {
                        match rr.found.kind {
                            SymbolKind::Function(_)
                            | SymbolKind::SystemVerilog
                            | SymbolKind::ModportFunctionMember(..)
                            | SymbolKind::SystemFunction => {}
                            _ => {
                                let identifier = rr.found.token.to_string();
                                let token: TokenRange = x.expression_identifier.as_ref().into();
                                self.errors.push(AnalyzerError::call_non_function(
                                    &identifier,
                                    &rr.found.kind.to_kind_name(),
                                    self.text,
                                    &token,
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
