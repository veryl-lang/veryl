//! Utilities for lowering the `string` type (its values are held in
//! `crate::string_pool`).
//!
//! A runtime `string` variable holds a 64-bit handle of its text. Where a
//! string goes into a string -- assigned to one, compared by `==`/`!=` with
//! one, connected to a `string` port or argument, printed with `%s`, taken
//! as a text by a testbench statement -- the expression lowers to an
//! ordinary 64-bit integer expression (`ProtoExpression::conv_for`): a
//! string constant becomes its handle, and the handles are then moved and
//! compared as integers.
//!
//! Where a string goes into bits -- a select, a concatenation, an assignment
//! to a non-string, an ordering comparison, a `%h` -- the handle is expanded
//! to the bytes of its text (`ProtoExpression::StringBytes`) at the width of
//! the variable's longest text, which is what a `string` of its bytes gave
//! (`cast`). Only the interpreter runs that. A `string` takes only a string
//! (the analyzer rejects anything else), so bits never turn back into a
//! text.

use crate::ir::context::{Context, Conv};
use crate::ir::expression::{
    Expression, ExpressionContext, ProtoExpression, conv_packed_select, storage_bounded_read,
};
use crate::ir::statement::{format_conv_takes_arg, parse_format_spec};
use crate::simulator_error::SimulatorError;
use crate::string_pool;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::{Shape, TypeKind, ValueVariant, VarId};
use veryl_analyzer::value::{MaskCache, Value};
use veryl_parser::token_range::TokenRange;

pub fn is_var(context: &mut Context, id: &VarId) -> bool {
    context
        .scope()
        .variable_meta
        .get(id)
        .is_some_and(|x| x.r#type.is_string())
}

/// Whether `expr` is a string (a constant, or the text of a runtime
/// `string`), as opposed to bits.
pub fn is_expr(expr: &air::Expression) -> bool {
    expr.comptime().r#type.is_string()
}

fn bit_type(width: usize) -> air::Type {
    let mut r#type = air::Type::new(TypeKind::Bit);
    r#type.set_concrete_width(Shape::new(vec![Some(width)]));
    r#type
}

/// `comptime` retyped as a 64-bit handle.
fn handle_comptime(comptime: &air::Comptime) -> air::Comptime {
    let mut comptime = comptime.clone();
    comptime.r#type = bit_type(string_pool::HANDLE_WIDTH);
    comptime.expr_context.width = string_pool::HANDLE_WIDTH;
    comptime.expr_context.signed = false;
    comptime
}

/// The text of a string constant (a literal or a folded `const`).
fn constant_bytes(factor: &air::Factor) -> Option<Value> {
    let comptime = match factor {
        air::Factor::Value(x) => x,
        air::Factor::Variable(_, _, _, x) if x.is_const && x.evaluated => x,
        _ => return None,
    };
    if comptime.r#type.kind != TypeKind::String {
        return None;
    }
    match &comptime.value {
        ValueVariant::Numeric(x) => Some(x.clone()),
        _ => None,
    }
}

fn unsupported(operation: &str, token: &TokenRange) -> SimulatorError {
    SimulatorError::unsupported_string_operation(operation, token)
}

/// Rewrites a string expression into its handle form: a string constant
/// becomes its handle, and a runtime `string` (a variable or an element of
/// one, a function's return value, a hierarchical reference) and `?:` of
/// those are retyped to read the handle itself, so the ordinary conversion
/// of a variable, a call or a reference reads it.
pub fn to_handle_expr(
    context: &mut Context,
    expr: &air::Expression,
) -> Result<air::Expression, SimulatorError> {
    match expr {
        air::Expression::Term(factor) => {
            if let Some(bytes) = constant_bytes(factor) {
                let comptime = factor.comptime();
                let mut comptime = handle_comptime(comptime);
                let handle = string_pool::from_value(&bytes, bytes.width());
                comptime.value =
                    ValueVariant::Numeric(Value::new(handle, string_pool::HANDLE_WIDTH, false));
                comptime.is_const = true;
                return Ok(air::Expression::Term(Box::new(air::Factor::Value(
                    comptime,
                ))));
            }
            match factor.as_ref() {
                air::Factor::Variable(id, index, select, comptime)
                    if select.is_empty() && is_var(context, id) =>
                {
                    Ok(air::Expression::Term(Box::new(air::Factor::Variable(
                        *id,
                        index.clone(),
                        select.clone(),
                        handle_comptime(comptime),
                    ))))
                }
                air::Factor::FunctionCall(call) if call.comptime.r#type.is_string() => {
                    let mut call = call.clone();
                    call.comptime = handle_comptime(&call.comptime);
                    Ok(air::Expression::Term(Box::new(air::Factor::FunctionCall(
                        call,
                    ))))
                }
                air::Factor::HierVariable(x)
                    if x.select.is_empty() && x.comptime.r#type.is_string() =>
                {
                    let mut x = x.clone();
                    x.comptime = handle_comptime(&x.comptime);
                    Ok(air::Expression::Term(Box::new(air::Factor::HierVariable(
                        x,
                    ))))
                }
                _ => Err(unsupported("this expression", &expr.token_range())),
            }
        }
        air::Expression::Ternary(c, x, y, comptime) => Ok(air::Expression::Ternary(
            c.clone(),
            Box::new(to_handle_expr(context, x)?),
            Box::new(to_handle_expr(context, y)?),
            Box::new(handle_comptime(comptime)),
        )),
        _ => Err(unsupported("this expression", &expr.token_range())),
    }
}

/// A concatenation of strings as the handle of its texts concatenated.
pub fn conv_concat(
    context: &mut Context,
    items: &[(air::Expression, Option<air::Expression>)],
) -> Result<ProtoExpression, SimulatorError> {
    let mut elements = Vec::with_capacity(items.len());
    for (x, repeat) in items {
        let handle = ProtoExpression::conv_for(context, x, &x.comptime().r#type)?;
        let repeat = match repeat {
            Some(repeat) => repeat
                .eval_value(&mut context.scope().analyzer_context)
                .and_then(|x| x.to_usize())
                .ok_or_else(|| SimulatorError::unsupported_description(&repeat.token_range()))?,
            None => 1,
        };
        elements.push((Box::new(handle), repeat));
    }
    Ok(ProtoExpression::StringConcat {
        elements,
        expr_context: ExpressionContext {
            width: string_pool::HANDLE_WIDTH,
            signed: false,
        },
    })
}

/// `handle`, a read of a `string`'s handle, moved into a destination: a
/// `string` takes the handle; anything else reads the bytes of its text at
/// `string_width`, an error naming `operation` when that is not known before
/// the simulation starts. The bytes take `expr_context`, widened to them.
pub fn cast(
    handle: ProtoExpression,
    string_width: Option<usize>,
    into_string: bool,
    operation: &str,
    token: &TokenRange,
    expr_context: ExpressionContext,
) -> Result<ProtoExpression, SimulatorError> {
    if into_string {
        return Ok(handle);
    }
    // An empty text still reads as one byte.
    let bytes = string_width
        .ok_or_else(|| unsupported(operation, token))?
        .max(8);
    Ok(ProtoExpression::StringBytes {
        x: Box::new(handle),
        bytes,
        select: None,
        dynamic_select: None,
        width: bytes,
        expr_context: ExpressionContext {
            width: expr_context.width.max(bytes),
            signed: expr_context.signed,
        },
    })
}

/// Checks a destination of type `dst_type`: a `string` is written only
/// whole (`s[7:0] = ...` is rejected), as a part would need its text turned
/// back from bits.
pub fn check_dst(
    dst_type: &air::Type,
    select: &air::VarSelect,
    operation: &str,
    token: &TokenRange,
) -> Result<(), SimulatorError> {
    if dst_type.is_string() && !select.is_empty() {
        Err(unsupported(operation, token))
    } else {
        Ok(())
    }
}

/// A runtime `string` variable read as bits: the bytes of its text at the
/// width of its longest text, with any select applied to those bytes as to
/// a `logic` of that width.
pub fn conv_bits(
    context: &mut Context,
    id: &VarId,
    index: &air::VarIndex,
    select: &air::VarSelect,
    comptime: &air::Comptime,
) -> Result<ProtoExpression, SimulatorError> {
    let token = comptime.token;
    let string_width = context
        .scope()
        .variable_meta
        .get(id)
        .and_then(|x| x.string_width);
    let handle = air::Expression::Term(Box::new(air::Factor::Variable(
        *id,
        index.clone(),
        air::VarSelect::default(),
        handle_comptime(comptime),
    )));
    let handle: ProtoExpression = Conv::conv(context, &handle)?;
    let expr_context: ExpressionContext = (&comptime.expr_context).into();
    let text = cast(
        handle,
        string_width,
        false,
        "a bit operation",
        &token,
        expr_context,
    )?;
    if select.is_empty() {
        return Ok(text);
    }
    let ProtoExpression::StringBytes { x, bytes, .. } = text else {
        unreachable!()
    };
    let (select, dynamic_select) = conv_packed_select(context, select, &bit_type(bytes))?;
    if let Some(dynamic_select) = dynamic_select {
        let width = dynamic_select.window;
        return Ok(ProtoExpression::StringBytes {
            x,
            bytes,
            select: None,
            dynamic_select: Some(dynamic_select),
            width,
            expr_context: ExpressionContext {
                width: expr_context.width.max(width),
                signed: false,
            },
        });
    }
    let Some((beg, end)) = select else {
        return Err(SimulatorError::unsupported_description(&token));
    };
    let span = beg - end + 1;
    let expr_context = ExpressionContext {
        width: expr_context.width.max(span),
        signed: false,
    };
    Ok(storage_bounded_read(
        select,
        bytes,
        bytes,
        expr_context,
        context.config.use_4state,
        |select, width, expr_context| ProtoExpression::StringBytes {
            x,
            bytes,
            select,
            dynamic_select: None,
            width,
            expr_context,
        },
    ))
}

/// A text a testbench statement takes (a file name, a component method's
/// argument): a string constant or a runtime `string`, as an expression of
/// its handle.
#[derive(Clone, Debug, Hash)]
pub struct ProtoText(pub ProtoExpression);

/// Pointer-bound form of [`ProtoText`].
#[derive(Clone)]
pub struct Text(pub Expression);

impl ProtoText {
    /// The text of a constant.
    #[cfg(test)]
    pub fn constant(text: &str) -> Self {
        let width = string_pool::HANDLE_WIDTH;
        ProtoText(ProtoExpression::Value {
            value: Value::new(string_pool::intern(text), width, false),
            width,
            expr_context: ExpressionContext {
                width,
                signed: false,
            },
        })
    }

    /// # Safety
    /// As [`ProtoExpression::apply_values_ptr`].
    pub unsafe fn apply_values_ptr(
        &self,
        ff_values_ptr: *mut u8,
        ff_len: usize,
        comb_values_ptr: *mut u8,
        comb_len: usize,
        use_4state: bool,
    ) -> Text {
        Text(unsafe {
            self.0
                .apply_values_ptr(ff_values_ptr, ff_len, comb_values_ptr, comb_len, use_4state)
        })
    }
}

impl Text {
    pub fn eval(&self, mask_cache: &mut MaskCache) -> String {
        string_pool::text(self.0.eval(mask_cache).payload_u64())
    }
}

/// The text `expr` gives; `None` when it is not a string.
pub fn conv_text(
    context: &mut Context,
    expr: &air::Expression,
) -> Result<Option<ProtoText>, SimulatorError> {
    if !is_expr(expr) {
        return Ok(None);
    }
    let handle = ProtoExpression::conv_for(context, expr, &expr.comptime().r#type)?;
    Ok(Some(ProtoText(handle)))
}

/// The conversion character consuming each of `nargs` arguments of
/// `format` (`None` past the last conversion).
pub fn format_arg_specs(format: &str, nargs: usize) -> Vec<Option<char>> {
    let chars: Vec<char> = format.chars().collect();
    let mut specs = Vec::new();
    let mut i = 0;
    while i < chars.len() && specs.len() < nargs {
        if chars[i] != '%' {
            i += 1;
            continue;
        }
        let Some(j) = parse_format_spec(&chars, i).conv else {
            break;
        };
        if format_conv_takes_arg(chars[j]) {
            specs.push(Some(chars[j]));
        }
        i = j + 1;
    }
    specs.resize(nargs, None);
    specs
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
