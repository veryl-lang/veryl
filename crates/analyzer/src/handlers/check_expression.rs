use crate::analyzer_error::AnalyzerError;
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
    call_stack_kind: Vec<FunctionKind>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FunctionKind {
    System,
    NonSystem,
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
                            } else {
                                self.call_stack_kind.push(FunctionKind::NonSystem);
                            }
                        }
                        SymbolKind::SystemFunction => {
                            if x.factor_opt.is_none() {
                                self.errors.push(error);
                            } else {
                                self.call_stack_kind.push(FunctionKind::System);
                            }
                        }
                        SymbolKind::Module(_)
                        | SymbolKind::Interface(_)
                        | SymbolKind::Instance(_)
                        | SymbolKind::Block
                        | SymbolKind::Package(_)
                        | SymbolKind::TypeDef(_)
                        | SymbolKind::Modport(_)
                        | SymbolKind::Namespace
                        | SymbolKind::GenericInstance(_) => {
                            self.errors.push(error);
                        }
                        SymbolKind::Port(x) => match x.direction {
                            Direction::Interface | Direction::Modport => {
                                self.errors.push(error);
                            }
                            _ => {}
                        },
                        SymbolKind::Parameter(_)
                        | SymbolKind::EnumMember(_)
                        | SymbolKind::Genvar
                        | SymbolKind::ModportVariableMember(_)
                        | SymbolKind::SystemVerilog
                        | SymbolKind::GenericParameter(_)
                        | SymbolKind::Variable(_) => {}

                        SymbolKind::Enum(_)
                        | SymbolKind::Union(_)
                        | SymbolKind::Struct(_)
                        | SymbolKind::StructMember(_)
                        | SymbolKind::UnionMember(_) => {
                            if let Some(FunctionKind::System) = self.call_stack_kind.last() {
                            } else {
                                self.errors.push(error);
                            }
                        }
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
        } else if let Factor::ExpressionIdentifierFactorOpt(x) = arg {
            let expid = x.expression_identifier.as_ref();
            if let Ok(rr) = symbol_table::resolve(expid) {
                match rr.found.kind {
                    SymbolKind::Function(_) | SymbolKind::ModportFunctionMember(_) => {
                        self.call_stack_kind.pop();
                    }
                    SymbolKind::SystemFunction => {
                        self.call_stack_kind.push(FunctionKind::System);
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
