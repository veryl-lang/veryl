use crate::analyzer_error::AnalyzerError;
use crate::connect_operation_table::{self, ConnectOperand, ConnectOperation};
use crate::symbol::{Direction, Symbol};
use crate::symbol_table;
use veryl_parser::ParolError;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckConnectOperation {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
}

fn includes_inout_ports(arg: &ConnectOperand) -> bool {
    if let ConnectOperand::Modport(x) = arg {
        x.get_ports()
            .iter()
            .any(|(_, direction)| matches!(direction, Direction::Inout))
    } else {
        false
    }
}

fn includes_unemittable_cast(target: &Symbol, driver: Option<&Symbol>) -> bool {
    fn get_type_symbol(symbol: &Symbol) -> Option<Symbol> {
        let r#type = symbol.kind.get_type();
        if r#type.is_none() || !r#type.unwrap().width.is_empty() {
            return None;
        }

        let user_defined = r#type.unwrap().get_user_defined()?;
        symbol_table::resolve((&user_defined.path.generic_path(), &symbol.namespace))
            .ok()
            .map(|x| x.found)
    }

    let Some(target_type) = get_type_symbol(target) else {
        return false;
    };

    if let Some(driver) = driver {
        if let Some(driver_type) = get_type_symbol(driver) {
            if target_type.id == driver_type.id {
                // no emit is required if both types are matched.
                return false;
            }
        }
    }

    target_type.namespace.matched(&target.namespace)
}

impl CheckConnectOperation {
    pub fn new() -> Self {
        Self::default()
    }

    fn is_valid_operation(
        &mut self,
        lhs_token: &Token,
        rhs_token: &Token,
        operation: &ConnectOperation,
        is_statement: bool,
    ) -> bool {
        if is_statement {
            if includes_inout_ports(&operation.lhs) {
                self.errors.push(AnalyzerError::invalid_connect_operand(
                    &lhs_token.to_string(),
                    "modport including inout ports can't be used at here",
                    &lhs_token.into(),
                ));
                return false;
            }
            if includes_inout_ports(&operation.rhs) {
                self.errors.push(AnalyzerError::invalid_connect_operand(
                    &rhs_token.to_string(),
                    "modport including inout ports can't be used at here",
                    &rhs_token.into(),
                ));
                return false;
            }
        }

        if let Some((ports, _)) = operation.get_ports_with_expression() {
            for (port, _) in ports {
                if includes_unemittable_cast(&port, None) {
                    self.errors.push(AnalyzerError::invalid_connect_operand(
                        &lhs_token.to_string(),
                        "modport including variables of which type is defined in the interface can't be used for a connect operand",
                        &lhs_token.into(),
                    ));
                    return false;
                }
            }
        } else {
            for (lhs_symbol, lhs_direction, rhs_symbol, _) in operation.get_connection_pairs() {
                let (target, driver, target_token) = if matches!(lhs_direction, Direction::Output) {
                    (&lhs_symbol, &rhs_symbol, lhs_token)
                } else {
                    (&rhs_symbol, &lhs_symbol, rhs_token)
                };

                if includes_unemittable_cast(target, Some(driver)) {
                    self.errors.push(AnalyzerError::invalid_connect_operand(
                        &target_token.to_string(),
                        "modport including variables of which type is defined in the interface can't be used for a connect operand",
                        &target_token.into(),
                    ));
                    return false;
                }
            }
        }

        true
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
            let (lhs, lhs_token) = match ConnectOperand::try_from(&*arg.hierarchical_identifier) {
                Ok(operand) => {
                    let token: TokenRange = arg.hierarchical_identifier.as_ref().into();
                    (operand, token)
                }
                Err(error) => {
                    if let Some(error) = error {
                        self.errors.push(error);
                    }
                    return Ok(());
                }
            };
            let (rhs, rhs_token) = match ConnectOperand::try_from(&*arg.expression) {
                Ok(operand) => {
                    let token: TokenRange = arg.expression.as_ref().into();
                    (operand, token)
                }
                Err(error) => {
                    if let Some(error) = error {
                        self.errors.push(error);
                    }
                    return Ok(());
                }
            };

            let operation = ConnectOperation { lhs, rhs };
            if self.is_valid_operation(&lhs_token.end, &rhs_token.end, &operation, false) {
                connect_operation_table::insert(&lhs_token.beg, operation);
            }
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let IdentifierStatementGroup::Assignment(assignment) = &*arg.identifier_statement_group
            else {
                return Ok(());
            };
            if !matches!(
                &*assignment.assignment.assignment_group,
                AssignmentGroup::DiamondOperator(_)
            ) {
                return Ok(());
            }

            let (lhs, lhs_token) = match ConnectOperand::try_from(&*arg.expression_identifier) {
                Ok(operand) => {
                    let token: TokenRange = arg.expression_identifier.as_ref().into();
                    (operand, token)
                }
                Err(error) => {
                    if let Some(error) = error {
                        self.errors.push(error);
                    }
                    return Ok(());
                }
            };
            let (rhs, rhs_token) =
                match ConnectOperand::try_from(&*assignment.assignment.expression) {
                    Ok(operand) => {
                        let token: TokenRange = assignment.assignment.expression.as_ref().into();
                        (operand, token)
                    }
                    Err(error) => {
                        if let Some(error) = error {
                            self.errors.push(error);
                        }
                        return Ok(());
                    }
                };

            let operation = ConnectOperation { lhs, rhs };
            if self.is_valid_operation(&lhs_token.end, &rhs_token.end, &operation, true) {
                connect_operation_table::insert(&lhs_token.beg, operation);
            }
        }
        Ok(())
    }
}
