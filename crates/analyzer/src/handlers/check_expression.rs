use crate::analyzer_error::AnalyzerError;
use crate::evaluator::{Evaluated, Evaluator};
use crate::symbol::{Direction, SymbolKind};
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
    in_inst_declaration: bool,
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
                    let error = AnalyzerError::invalid_factor(
                        &identifier,
                        &rr.found.kind.to_kind_name(),
                        self.text,
                        &token,
                    );
                    match rr.found.kind {
                        SymbolKind::Function(_) | SymbolKind::ModportFunctionMember(_) => {
                            if x.factor_opt.is_none() {
                                self.errors.push(error);
                            }
                        }
                        SymbolKind::SystemFunction => {
                            if x.factor_opt.is_none() {
                                self.errors.push(error);
                            }
                        }
                        // instance can be used as factor in inst_declaration
                        SymbolKind::Instance(_) if self.in_inst_declaration => (),
                        SymbolKind::Module(_)
                        | SymbolKind::ProtoModule(_)
                        | SymbolKind::Interface(_)
                        | SymbolKind::Instance(_)
                        | SymbolKind::Block
                        | SymbolKind::Package(_)
                        | SymbolKind::Modport(_)
                        | SymbolKind::Namespace
                        | SymbolKind::ClockDomain
                        | SymbolKind::Test(_) => {
                            self.errors.push(error);
                        }
                        SymbolKind::Port(x) => {
                            // modport and interface direction can be used as factor in inst_declaration
                            if !self.in_inst_declaration {
                                match x.direction {
                                    Direction::Interface | Direction::Modport => {
                                        self.errors.push(error);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        SymbolKind::TypeDef(_)
                        | SymbolKind::Struct(_)
                        | SymbolKind::Enum(_)
                        | SymbolKind::Union(_)
                        | SymbolKind::Parameter(_)
                        | SymbolKind::EnumMember(_)
                        | SymbolKind::EnumMemberMangled
                        | SymbolKind::Genvar
                        | SymbolKind::ModportVariableMember(_)
                        | SymbolKind::SystemVerilog
                        | SymbolKind::GenericParameter(_)
                        | SymbolKind::StructMember(_)
                        | SymbolKind::UnionMember(_)
                        | SymbolKind::GenericInstance(_)
                        | SymbolKind::Variable(_) => {}
                    }
                }

                if x.factor_opt.is_some() {
                    // Must be a function call
                    let expid = x.expression_identifier.as_ref();
                    if let Ok(rr) = symbol_table::resolve(expid) {
                        let is_function = match &rr.found.kind {
                            SymbolKind::Function(_)
                            | SymbolKind::SystemVerilog
                            | SymbolKind::ModportFunctionMember(..)
                            | SymbolKind::SystemFunction => true,
                            SymbolKind::GenericInstance(x) => {
                                let base = symbol_table::get(x.base).unwrap();
                                matches!(
                                    base.kind,
                                    SymbolKind::Function(_)
                                        | SymbolKind::SystemVerilog
                                        | SymbolKind::ModportFunctionMember(..)
                                        | SymbolKind::SystemFunction
                                )
                            }
                            _ => false,
                        };

                        if !is_function {
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
        Ok(())
    }

    fn inst_declaration(&mut self, _arg: &InstDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_inst_declaration = true,
            HandlerPoint::After => self.in_inst_declaration = false,
        }
        Ok(())
    }
}
