use crate::analyzer_error::AnalyzerError;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckStatement {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    in_always_ff: bool,
    in_always_comb: bool,
    in_non_void_function: bool,
    in_initial: bool,
    in_final: bool,
    statement_depth_in_always_ff: usize,
    statement_depth_in_loop: usize,
}

impl CheckStatement {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckStatement {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckStatement {
    fn statement(&mut self, _arg: &Statement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            self.statement_depth_in_always_ff += 1;
        }
        Ok(())
    }

    fn assignment(&mut self, arg: &Assignment) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && (self.in_initial || self.in_final)
        {
            let (kind, token) = match &*arg.assignment_group {
                AssignmentGroup::Equ(x) => ("assignment", &x.equ.equ_token.token),
                AssignmentGroup::AssignmentOperator(x) => (
                    "assignment",
                    &x.assignment_operator.assignment_operator_token.token,
                ),
                AssignmentGroup::DiamondOperator(x) => (
                    "connection",
                    &x.diamond_operator.diamond_operator_token.token,
                ),
            };
            self.errors
                .push(AnalyzerError::invalid_statement(kind, &token.into()));
        }
        Ok(())
    }

    fn if_reset_statement(&mut self, arg: &IfResetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if !self.in_always_ff {
                self.errors.push(AnalyzerError::invalid_statement(
                    "if_reset",
                    &arg.if_reset.if_reset_token.token.into(),
                ));
            }

            if self.in_always_ff && self.statement_depth_in_always_ff != 1 {
                self.errors.push(AnalyzerError::invalid_statement(
                    "if_reset",
                    &arg.if_reset.if_reset_token.token.into(),
                ));
            }
        }
        Ok(())
    }

    fn return_statement(&mut self, arg: &ReturnStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && !self.in_non_void_function
        {
            self.errors.push(AnalyzerError::invalid_statement(
                "return",
                &arg.r#return.return_token.token.into(),
            ));
        }
        Ok(())
    }

    fn break_statement(&mut self, arg: &BreakStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && self.statement_depth_in_loop == 0
        {
            self.errors.push(AnalyzerError::invalid_statement(
                "break",
                &arg.r#break.break_token.token.into(),
            ));
        }
        Ok(())
    }

    fn for_statement(&mut self, _arg: &ForStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.statement_depth_in_loop += 1,
            HandlerPoint::After => self.statement_depth_in_loop -= 1,
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

    fn initial_declaration(&mut self, _arg: &InitialDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_initial = true,
            HandlerPoint::After => self.in_initial = false,
        }
        Ok(())
    }

    fn final_declaration(&mut self, _arg: &FinalDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_final = true,
            HandlerPoint::After => self.in_final = false,
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_non_void_function = arg.function_declaration_opt1.is_some()
            }
            HandlerPoint::After => self.in_non_void_function = false,
        }
        Ok(())
    }
}
