use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckStatement<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    in_always_ff: bool,
    in_always_comb: bool,
    in_function: bool,
    statement_depth_in_always_ff: usize,
}

impl<'a> CheckStatement<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckStatement<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckStatement<'a> {
    fn statement(&mut self, _arg: &Statement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.statement_depth_in_always_ff += 1;
        }
        Ok(())
    }

    fn if_reset_statement(&mut self, arg: &IfResetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_always_comb || self.in_function {
                self.errors.push(AnalyzerError::invalid_statement(
                    "if_reset",
                    self.text,
                    &arg.if_reset.if_reset_token,
                ));
            }

            if self.in_always_ff && self.statement_depth_in_always_ff != 1 {
                self.errors.push(AnalyzerError::invalid_statement(
                    "if_reset",
                    self.text,
                    &arg.if_reset.if_reset_token,
                ));
            }
        }
        Ok(())
    }

    fn return_statement(&mut self, arg: &ReturnStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_always_ff || self.in_always_comb {
                self.errors.push(AnalyzerError::invalid_statement(
                    "return",
                    self.text,
                    &arg.r#return.return_token,
                ));
            }
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, _arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_always_ff = true;
                self.statement_depth_in_always_ff = 0;
            }
            HandlerPoint::After => self.in_always_ff = false,
        }
        Ok(())
    }

    fn always_comb_declaration(&mut self, _arg: &AlwaysCombDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_always_comb = true,
            HandlerPoint::After => self.in_always_comb = false,
        }
        Ok(())
    }

    fn function_declaration(&mut self, _arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_function = true,
            HandlerPoint::After => self.in_function = false,
        }
        Ok(())
    }
}
