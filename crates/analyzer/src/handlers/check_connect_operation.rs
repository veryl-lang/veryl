use crate::analyzer_error::AnalyzerError;
use crate::connect_operation_table::{self, ConnectOperand};
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckConnectOperation {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
}

impl CheckConnectOperation {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckConnectOperation {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckConnectOperation {
    fn connect_declaration(&mut self, arg: &ConnectDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let (lhs, token) = match ConnectOperand::try_from(&*arg.hierarchical_identifier) {
                Ok(operand) => {
                    let token = arg
                        .hierarchical_identifier
                        .identifier
                        .identifier_token
                        .token;
                    (operand, token)
                }
                Err(error) => {
                    if let Some(error) = error {
                        self.errors.push(error);
                    }
                    return Ok(());
                }
            };
            let rhs = match ConnectOperand::try_from(&*arg.expression) {
                Ok(operand) => operand,
                Err(error) => {
                    if let Some(error) = error {
                        self.errors.push(error);
                    }
                    return Ok(());
                }
            };
            connect_operation_table::insert(&token, lhs, rhs);
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let IdentifierStatementGroup::Assignment(x) = &*arg.identifier_statement_group {
                if let AssignmentGroup::DiamondOperator(_) = &*x.assignment.assignment_group {
                    let (lhs, token) = match ConnectOperand::try_from(&*arg.expression_identifier) {
                        Ok(operand) => {
                            let token = arg.expression_identifier.identifier().token;
                            (operand, token)
                        }
                        Err(error) => {
                            if let Some(error) = error {
                                self.errors.push(error);
                            }
                            return Ok(());
                        }
                    };
                    let rhs = match ConnectOperand::try_from(&*x.assignment.expression) {
                        Ok(operand) => operand,
                        Err(error) => {
                            if let Some(error) = error {
                                self.errors.push(error);
                            }
                            return Ok(());
                        }
                    };
                    connect_operation_table::insert(&token, lhs, rhs);
                }
            }
        }
        Ok(())
    }
}
