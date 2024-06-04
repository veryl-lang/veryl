use crate::analyzer_error::AnalyzerError;
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
                    match rr.found.kind {
                        crate::symbol::SymbolKind::Function(_)
                        | crate::symbol::SymbolKind::ModportFunctionMember(_)
                        | crate::symbol::SymbolKind::SystemFunction => {
                            if x.factor_opt.is_none() {
                                self.errors.push(AnalyzerError::invalid_factor(
                                    &identifier,
                                    &rr.found.kind.to_kind_name(),
                                    self.text,
                                    &token,
                                ));
                            }
                        }
                        crate::symbol::SymbolKind::Module(_)
                        | crate::symbol::SymbolKind::Interface(_)
                        | crate::symbol::SymbolKind::Instance(_)
                        | crate::symbol::SymbolKind::Block
                        | crate::symbol::SymbolKind::Package(_)
                        | crate::symbol::SymbolKind::TypeDef(_)
                        | crate::symbol::SymbolKind::Enum(_)
                        | crate::symbol::SymbolKind::Modport(_)
                        | crate::symbol::SymbolKind::ModportVariableMember(_)
                        | crate::symbol::SymbolKind::Namespace
                        | crate::symbol::SymbolKind::GenericInstance(_) => {
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
            }
        }
        Ok(())
    }
}
