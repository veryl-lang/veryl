use crate::analyzer_error::AnalyzerError;
use crate::connect_operation_table::{self, ConnectOperand, ConnectOperation};
use crate::conv::Context;
use crate::symbol::{Direction, Symbol};
use crate::symbol_table;
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

fn is_valid_operation(
    context: &mut Context,
    lhs_token: &Token,
    rhs_token: &Token,
    operation: &ConnectOperation,
    is_statement: bool,
) -> bool {
    if is_statement {
        if includes_inout_ports(&operation.lhs) {
            context.insert_error(AnalyzerError::invalid_connect_operand(
                &lhs_token.to_string(),
                "modport including inout ports can't be used at here",
                &lhs_token.into(),
            ));
            return false;
        }
        if includes_inout_ports(&operation.rhs) {
            context.insert_error(AnalyzerError::invalid_connect_operand(
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
                context.insert_error(AnalyzerError::invalid_connect_operand(
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
                context.insert_error(AnalyzerError::invalid_connect_operand(
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

pub fn check_connect<T, U>(context: &mut Context, lhs: &T, rhs: &U, is_statement: bool)
where
    ConnectOperand: for<'a> TryFrom<&'a T, Error = Option<AnalyzerError>>,
    ConnectOperand: for<'a> TryFrom<&'a U, Error = Option<AnalyzerError>>,
    TokenRange: for<'a> From<&'a T>,
    TokenRange: for<'a> From<&'a U>,
{
    let lhs_operand = match TryInto::<ConnectOperand>::try_into(lhs) {
        Ok(operand) => operand,
        Err(error) => {
            if let Some(error) = error {
                context.insert_error(error);
            }
            return;
        }
    };
    let rhs_operand = match TryInto::<ConnectOperand>::try_into(rhs) {
        Ok(operand) => operand,
        Err(error) => {
            if let Some(error) = error {
                context.insert_error(error);
            }
            return;
        }
    };

    let lhs_token: TokenRange = lhs.into();
    let rhs_token: TokenRange = rhs.into();

    let operation = ConnectOperation {
        lhs: lhs_operand,
        rhs: rhs_operand,
    };
    if is_valid_operation(
        context,
        &lhs_token.end,
        &rhs_token.end,
        &operation,
        is_statement,
    ) {
        connect_operation_table::insert(&lhs_token.beg, operation);
    }
}
