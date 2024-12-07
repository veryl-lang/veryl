use crate::analyzer_error::AnalyzerError;
use crate::symbol::{SymbolId, SymbolKind};
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckSeparator<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckSeparator<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    fn check_separator(
        &mut self,
        full_path: &mut Vec<SymbolId>,
        check_dot_separator: bool,
        separator_token: &Token,
    ) {
        let preceed_symbol = if let Some(symbol_id) = full_path.pop() {
            symbol_table::get(symbol_id).unwrap()
        } else {
            return;
        };
        let this_symbol = if let Some(symbol_id) = full_path.last() {
            symbol_table::get(*symbol_id).unwrap()
        } else {
            // length of `full_path` may be shorter than length of actual path
            // if the type of the preceed symbol is defined in the SV namespace.
            return;
        };
        let expect_dot_separator = if let SymbolKind::Function(_) = this_symbol.kind {
            matches!(
                preceed_symbol.kind,
                SymbolKind::Instance(_) // member function of interface
            )
        } else {
            matches!(
                this_symbol.kind,
                SymbolKind::Variable(_) // member variable of instance
                | SymbolKind::StructMember(_)
                | SymbolKind::UnionMember(_)
                | SymbolKind::ModportVariableMember(_)
                | SymbolKind::ModportFunctionMember(_)
            )
        };

        if expect_dot_separator != check_dot_separator {
            self.errors.push(AnalyzerError::wrong_seperator(
                &separator_token.to_string(),
                self.text,
                &separator_token.into(),
            ));
        }
    }
}

impl Handler for CheckSeparator<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckSeparator<'_> {
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(symbol) = symbol_table::resolve(arg) {
                let mut full_path: Vec<_> = symbol.full_path.into_iter().rev().collect();

                for x in &arg.scoped_identifier.scoped_identifier_list {
                    self.check_separator(
                        &mut full_path,
                        false,
                        &x.colon_colon.colon_colon_token.token,
                    );
                }

                for x in &arg.expression_identifier_list0 {
                    self.check_separator(&mut full_path, true, &x.dot.dot_token.token);
                }
            }
        }
        Ok(())
    }
}
