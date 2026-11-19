//! Lowering of handle `string` variables (see `crate::string_pool`).
//!
//! A handle variable holds a 64-bit handle, so an expression of it lowers to
//! an ordinary 64-bit integer expression: a string constant becomes its
//! handle, and assignment, `?:`, `==` and `!=` then work on handles as on
//! any integer. Only these behave as on a string, so any other operation on
//! a handle variable is rejected.

use crate::ir::context::Context;
use crate::ir::statement::{format_conv_takes_arg, parse_format_spec};
use crate::simulator_error::SimulatorError;
use crate::string_pool;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::{Shape, TypeKind, ValueVariant, VarId};
use veryl_analyzer::value::Value;
use veryl_parser::token_range::TokenRange;

pub fn is_handle_var(context: &mut Context, id: &VarId) -> bool {
    context
        .scope()
        .variable_meta
        .get(id)
        .is_some_and(|x| x.string_handle)
}

/// Whether `expr` reads a handle variable as a whole string: the variable
/// itself, or a `?:` with such a branch.
pub fn is_handle_expr(context: &mut Context, expr: &air::Expression) -> bool {
    match expr {
        air::Expression::Term(factor) => {
            matches!(factor.as_ref(), air::Factor::Variable(id, _, _, _) if is_handle_var(context, id))
        }
        air::Expression::Ternary(_, x, y, _) => {
            is_handle_expr(context, x) || is_handle_expr(context, y)
        }
        _ => false,
    }
}

fn handle_type() -> air::Type {
    let mut r#type = air::Type::new(TypeKind::Bit);
    r#type.set_concrete_width(Shape::new(vec![Some(string_pool::HANDLE_WIDTH)]));
    r#type
}

/// `comptime` retyped as a 64-bit handle.
fn handle_comptime(comptime: &air::Comptime) -> air::Comptime {
    let mut comptime = comptime.clone();
    comptime.r#type = handle_type();
    comptime.expr_context.width = string_pool::HANDLE_WIDTH;
    comptime.expr_context.signed = false;
    comptime
}

/// The text of a string constant (a literal or a folded `const`).
fn constant_text(factor: &air::Factor) -> Option<String> {
    let comptime = match factor {
        air::Factor::Value(x) => x,
        air::Factor::Variable(_, _, _, x) if x.is_const && x.evaluated => x,
        _ => return None,
    };
    if comptime.r#type.kind != TypeKind::String {
        return None;
    }
    match &comptime.value {
        ValueVariant::Numeric(x) => Some(veryl_analyzer::value::byte_value_to_string_lossy(x)),
        _ => None,
    }
}

fn unsupported(token: &TokenRange) -> SimulatorError {
    SimulatorError::unsupported_string_operation("this expression", token)
}

/// Rewrites a string expression stored into or compared with a handle
/// variable into its handle form. Accepts a handle variable, a string
/// constant, and `?:` of those; anything else (in particular a variable
/// holding the bytes of its text) is rejected.
pub fn to_handle_expr(
    context: &mut Context,
    expr: &air::Expression,
) -> Result<air::Expression, SimulatorError> {
    match expr {
        air::Expression::Term(factor) => {
            if let Some(text) = constant_text(factor) {
                let comptime = factor.comptime();
                let mut comptime = handle_comptime(comptime);
                let handle = string_pool::intern(&text);
                comptime.value =
                    ValueVariant::Numeric(Value::new(handle, string_pool::HANDLE_WIDTH, false));
                comptime.is_const = true;
                return Ok(air::Expression::Term(Box::new(air::Factor::Value(
                    comptime,
                ))));
            }
            if let air::Factor::Variable(id, index, select, comptime) = factor.as_ref()
                && is_handle_var(context, id)
            {
                if !index.indices.is_empty() || index.range.is_some() || !select.is_empty() {
                    return Err(unsupported(&comptime.token));
                }
                return Ok(air::Expression::Term(Box::new(air::Factor::Variable(
                    *id,
                    index.clone(),
                    select.clone(),
                    handle_comptime(comptime),
                ))));
            }
            Err(unsupported(&expr.token_range()))
        }
        air::Expression::Ternary(c, x, y, comptime) => Ok(air::Expression::Ternary(
            c.clone(),
            Box::new(to_handle_expr(context, x)?),
            Box::new(to_handle_expr(context, y)?),
            Box::new(handle_comptime(comptime)),
        )),
        _ => Err(unsupported(&expr.token_range())),
    }
}

/// Rewrites the `%s` conversions whose argument is a handle into
/// `HANDLE_SPEC`, so the formatter prints the handle's text. Conversions are
/// counted as `format_display_string` consumes arguments.
pub fn mark_handle_specs(format: &str, handle_args: &[bool]) -> String {
    let chars: Vec<char> = format.chars().collect();
    let mut result = String::with_capacity(format.len());
    let mut arg_idx = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            result.push(chars[i]);
            i += 1;
            continue;
        }
        let Some(j) = parse_format_spec(&chars, i).conv else {
            result.extend(&chars[i..]);
            break;
        };
        result.extend(&chars[i..j]);
        let conv = chars[j];
        if format_conv_takes_arg(conv) {
            let is_handle = handle_args.get(arg_idx).copied().unwrap_or(false);
            if is_handle && matches!(conv, 's' | 'S') {
                result.push(string_pool::HANDLE_SPEC);
            } else {
                result.push(conv);
            }
            arg_idx += 1;
        } else {
            result.push(conv);
        }
        i = j + 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_specs() {
        let h = string_pool::HANDLE_SPEC;
        assert_eq!(
            mark_handle_specs("%d %s %5s %-3s %% %m %s", &[false, true, false, true, true]),
            format!("%d %{h} %5s %-3{h} %% %m %{h}")
        );
        assert_eq!(mark_handle_specs("ends %5", &[true]), "ends %5");
    }
}
