use crate::analyzer_error::{AnalyzerError, InvalidConnectOperandKind};
use crate::connect_operation_table::{self, ConnectOperand, ConnectOperation};
use crate::symbol::{Direction, Symbol};
use crate::symbol_table::{self, Connect};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::Token;

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

    if let Some(driver) = driver
        && let Some(driver_type) = get_type_symbol(driver)
        && target_type.id == driver_type.id
    {
        // no emit is required if both types are matched.
        return false;
    }

    target_type.namespace.matched(&target.namespace)
}

#[allow(clippy::result_large_err)]
fn is_valid_operation(
    lhs_token: &Token,
    rhs_token: &Token,
    operation: &ConnectOperation,
    is_statement: bool,
) -> Result<(), AnalyzerError> {
    if is_statement {
        if includes_inout_ports(&operation.lhs) {
            return Err(AnalyzerError::invalid_connect_operand(
                &lhs_token.to_string(),
                InvalidConnectOperandKind::IncludeInout,
                &lhs_token.into(),
            ));
        }
        if includes_inout_ports(&operation.rhs) {
            return Err(AnalyzerError::invalid_connect_operand(
                &rhs_token.to_string(),
                InvalidConnectOperandKind::IncludeInout,
                &rhs_token.into(),
            ));
        }
    }

    if let Some((ports, _)) = operation.get_ports_with_expression() {
        for (port, _) in ports {
            if includes_unemittable_cast(&port, None) {
                return Err(AnalyzerError::invalid_connect_operand(
                    &lhs_token.to_string(),
                    InvalidConnectOperandKind::UnemittableCast,
                    &lhs_token.into(),
                ));
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
                return Err(AnalyzerError::invalid_connect_operand(
                    &target_token.to_string(),
                    InvalidConnectOperandKind::UnemittableCast,
                    &target_token.into(),
                ));
            }
        }
    }

    Ok(())
}

fn create_connect_operand<T>(
    value: &T,
    errors: &mut Vec<AnalyzerError>,
) -> Option<(ConnectOperand, TokenRange)>
where
    ConnectOperand: for<'a> TryFrom<&'a T, Error = Option<AnalyzerError>>,
    TokenRange: for<'a> From<&'a T>,
{
    match TryInto::<ConnectOperand>::try_into(value) {
        Ok(x) => {
            let token: TokenRange = value.into();
            Some((x, token))
        }
        Err(error) => {
            if let Some(error) = error {
                errors.push(error);
            }
            None
        }
    }
}

pub fn check_connect(list: Vec<Connect>) -> Vec<AnalyzerError> {
    let mut ret = vec![];

    for x in &list {
        let (lhs_operand, lhs_token, rhs_operand, rhs_token) = match x {
            Connect::Statement(lhs, rhs) => {
                let Some((lhs, lhs_token)) = create_connect_operand(lhs, &mut ret) else {
                    continue;
                };
                let Some((rhs, rhs_token)) = create_connect_operand(rhs, &mut ret) else {
                    continue;
                };
                (lhs, lhs_token, rhs, rhs_token)
            }
            Connect::Declaration(lhs, rhs) => {
                let Some((lhs, lhs_token)) = create_connect_operand(lhs, &mut ret) else {
                    continue;
                };
                let Some((rhs, rhs_token)) = create_connect_operand(rhs, &mut ret) else {
                    continue;
                };
                (lhs, lhs_token, rhs, rhs_token)
            }
        };

        let is_statement = matches!(x, Connect::Statement(_, _));

        let operation = ConnectOperation {
            lhs: lhs_operand,
            rhs: rhs_operand,
        };

        match is_valid_operation(&lhs_token.end, &rhs_token.end, &operation, is_statement) {
            Ok(_) => {
                connect_operation_table::insert(&lhs_token.beg, operation);
            }
            Err(x) => ret.push(x),
        }
    }

    ret
}
