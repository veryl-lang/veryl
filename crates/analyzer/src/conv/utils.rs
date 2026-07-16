use crate::analyzer_error::{
    AnalyzerError, ComponentInterfaceMismatchKind, ExceedLimitKind, InvalidForStepKind,
    MismatchTypeKind, MultipleDefaultKind, UnevaluableValueKind,
};
use crate::conv::checker::anonymous::check_anonymous;
use crate::conv::checker::clock_domain::check_clock_domain;
use crate::conv::checker::generic::check_generic_refereence;
use crate::conv::instance::InstanceHistoryError;
use crate::conv::{Context, Conv};
use crate::definition_table::{self, Definition, DefinitionId};
use crate::ir::{
    self, Arguments, Comptime, FuncPath, FuncProto, IrResult, Op, PartSelectPath, Shape, ShapeRef,
    Signature, SystemFunctionInput, TbMethod, TbMethodCall, ValueVariant, VarIndex, VarKind,
    VarPath, VarPathSelect, VarSelect, Variable,
};
use crate::symbol::{
    self, Affiliation, ClockDomain, EnumMemberValue, GenericBoundKind, GenericMap, ProtoBound,
    Symbol, SymbolKind, TbComponentKind, TypeKind,
};
use crate::symbol_path::{
    GenericSymbolPath, GenericSymbolPathKind, SymbolPath, SymbolPathNamespace,
};
use crate::symbol_table::{self, ResolveResult};
use crate::value::Value;
use crate::{HashMap, HashSet, ir_error};
use std::rc::Rc;
use std::sync::Arc;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

/// True for "atomic" expressions whose result type is unambiguous:
/// variable references, sized / real / boolean literals, function
/// calls, and parenthesized recursions. Operators, casts, concatenation
/// and unsized literals are rejected.
pub fn is_inferable_expression(expr: &Expression) -> bool {
    let if_expr = &*expr.if_expression;
    if !if_expr.if_expression_list.is_empty() {
        return false;
    }
    let e01 = &*if_expr.expression01;
    if !e01.expression01_list.is_empty() {
        return false;
    }
    let e02 = &*e01.expression02;
    if !e02.expression02_list.is_empty() || e02.expression02_opt.is_some() {
        return false;
    }
    is_inferable_factor(&e02.factor)
}

fn is_inferable_factor(factor: &Factor) -> bool {
    match factor {
        Factor::IdentifierFactor(_) => true,
        Factor::BooleanLiteral(_) => true,
        Factor::LParenExpressionRParen(x) => is_inferable_expression(&x.expression),
        Factor::Number(x) => match x.number.as_ref() {
            Number::IntegralNumber(x) => match x.integral_number.as_ref() {
                IntegralNumber::Based(_) => true,
                IntegralNumber::BaseLess(_) | IntegralNumber::AllBit(_) => false,
            },
            Number::RealNumber(_) => true,
        },
        _ => false,
    }
}

/// Inference for `var X;` (no type) from a single assignment.
/// Returns the evaluation pair so the caller avoids re-walking the
/// expression. Subsequent conflicting assigns are rejected.
pub fn try_infer_var_assign(
    context: &mut Context,
    dst_symbol: &Symbol,
    expr: &Expression,
    token: TokenRange,
) -> IrResult<Option<(Comptime, ir::Expression)>> {
    let SymbolKind::Variable(var_prop) = &dst_symbol.kind else {
        return Ok(None);
    };
    if !var_prop.r#type.is_inferred() {
        return Ok(None);
    }

    if !is_inferable_expression(expr) {
        context.insert_error(AnalyzerError::type_inference_not_supported(&expr.into()));
        return Err(ir_error!(token));
    }
    let (comptime, ir_expr) = eval_expr(context, None, expr, false)?;
    let inferred = comptime.r#type.clone();
    check_inferred_type_emittable(context, &inferred, expr, token)?;

    if let Some(existing) = crate::resolved_type_table::get(&dst_symbol.token.id) {
        if existing != inferred {
            context.insert_error(AnalyzerError::type_inference_conflict(
                &dst_symbol.token.text.to_string(),
                &expr.into(),
            ));
            return Err(ir_error!(token));
        }
    } else {
        crate::resolved_type_table::insert(dst_symbol.token.id, inferred.clone());
        // Replace the placeholder Comptime so subsequent lookups see
        // the inferred type, not `Unknown`.
        let path = VarPath::new(dst_symbol.token.text);
        if let Some((id, mut ct)) = context.find_path(&path) {
            ct.r#type = inferred.clone();
            context.insert_var_path_with_id(path, id, ct);
        }
    }
    Ok(Some((comptime, ir_expr)))
}

/// Inference for `let`/`const` declarations. Returns `None` when the
/// type is not `Inferred`.
pub fn try_infer_decl_type(
    context: &mut Context,
    sym_type: &symbol::Type,
    expr: &Expression,
    decl_token_id: veryl_parser::resource_table::TokenId,
    token: TokenRange,
) -> IrResult<Option<(Comptime, ir::Expression)>> {
    if !sym_type.is_inferred() {
        return Ok(None);
    }
    if !is_inferable_expression(expr) {
        context.insert_error(AnalyzerError::type_inference_not_supported(&expr.into()));
        return Err(ir_error!(token));
    }
    let (comptime, ir_expr) = eval_expr(context, None, expr, false)?;
    check_inferred_type_emittable(context, &comptime.r#type, expr, token)?;
    crate::resolved_type_table::insert(decl_token_id, comptime.r#type.clone());
    Ok(Some((comptime, ir_expr)))
}

/// The emitter declares an inferred variable by its SV scalar type name;
/// a struct/union/enum has none and would silently emit a 1-bit `logic`,
/// so reject the inference instead of miscompiling.
fn check_inferred_type_emittable(
    context: &mut Context,
    r#type: &ir::Type,
    expr: &Expression,
    token: TokenRange,
) -> IrResult<()> {
    if matches!(
        r#type.kind,
        ir::TypeKind::Struct(_) | ir::TypeKind::Union(_) | ir::TypeKind::Enum(_)
    ) {
        context.insert_error(AnalyzerError::type_inference_not_supported(&expr.into()));
        return Err(ir_error!(token));
    }
    Ok(())
}

fn format_positive_type_name(r#type: &ir::Type) -> Option<String> {
    if !r#type.is_positive {
        return None;
    }

    if let Some(width) = r#type.width().first()
        && let Some(w) = width
    {
        return Some(match *w {
            8 => "p8".to_string(),
            16 => "p16".to_string(),
            32 => "p32".to_string(),
            64 => "p64".to_string(),
            _ => r#type.to_string(),
        });
    }

    Some(r#type.to_string())
}

pub fn eval_expr(
    context: &mut Context,
    dst_type: Option<ir::Type>,
    expr: &Expression,
    allow_anonymous: bool,
) -> IrResult<(Comptime, ir::Expression)> {
    let token: TokenRange = expr.into();
    let expr: IrResult<ir::Expression> = Conv::conv(context, expr);

    if let Ok(mut expr) = expr {
        check_anonymous(context, &expr, allow_anonymous, token);

        let comptime = if let Some(dst_type) = dst_type {
            let mut comptime = expr.eval_comptime(context, dst_type.total_width()).clone();

            check_compatibility(context, &dst_type, &comptime, &token);

            comptime.r#type = dst_type;
            comptime
        } else {
            expr.eval_comptime(context, None).clone()
        };

        Ok((comptime, expr))
    } else {
        let comptime = Comptime::create_unknown(token);
        let expr = ir::Expression::Term(Box::new(ir::Factor::Unknown(comptime.clone())));
        Ok((comptime, expr))
    }
}

pub fn eval_generic_expr(
    context: &mut Context,
    expr: &Expression,
) -> IrResult<(Comptime, ir::Expression)> {
    let allow_component_as_factor = context.allow_component_as_factor;
    context.allow_component_as_factor = true;
    let ret = eval_expr(context, None, expr, false);
    context.allow_component_as_factor = allow_component_as_factor;
    ret
}

pub fn eval_range(
    context: &mut Context,
    range: &Range,
) -> IrResult<(ir::ForBound, ir::ForBound, bool)> {
    eval_range_inner(context, range, false)
}

pub fn eval_range_const(
    context: &mut Context,
    range: &Range,
) -> IrResult<(ir::ForBound, ir::ForBound, bool)> {
    eval_range_inner(context, range, true)
}

fn eval_range_inner(
    context: &mut Context,
    range: &Range,
    require_const: bool,
) -> IrResult<(ir::ForBound, ir::ForBound, bool)> {
    let mut beg: ir::Expression = Conv::conv(context, range.expression.as_ref())?;
    let beg_comptime = beg.eval_comptime(context, None);
    if require_const && !beg_comptime.is_const {
        context.insert_error(AnalyzerError::unevaluable_value(
            UnevaluableValueKind::ForRange,
            &range.into(),
        ));
    }
    let beg = if beg_comptime.is_const {
        let val = beg_comptime.get_value()?.to_usize().unwrap_or(0);
        ir::ForBound::Const(val)
    } else {
        ir::ForBound::Expression(Box::new(beg))
    };

    let (end, inclusive) = if let Some(x) = &range.range_opt {
        let mut end: ir::Expression = Conv::conv(context, x.expression.as_ref())?;
        let end_comptime = end.eval_comptime(context, None);
        if require_const && !end_comptime.is_const {
            context.insert_error(AnalyzerError::unevaluable_value(
                UnevaluableValueKind::ForRange,
                &range.into(),
            ));
        }
        let end = if end_comptime.is_const {
            let val = end_comptime.get_value()?.to_usize().unwrap_or(0);
            ir::ForBound::Const(val)
        } else {
            ir::ForBound::Expression(Box::new(end))
        };

        if matches!(x.range_operator.as_ref(), RangeOperator::DotDotEqu(_)) {
            (end, true)
        } else {
            (end, false)
        }
    } else {
        (beg.clone(), false)
    };

    Ok((beg, end, inclusive))
}

#[derive(Clone)]
pub struct ArrayLiteralExpression {
    pub index: Vec<usize>,
    pub select: Vec<usize>,
    pub expr: ir::Expression,
}

impl ArrayLiteralExpression {
    pub fn to_var_index(&self) -> VarIndex {
        let mut ret = vec![];
        for i in &self.index {
            let expr = ir::Expression::create_value(
                Value::new(*i as u64, 32, false),
                TokenRange::default(),
            );
            ret.push(expr);
        }
        VarIndex(ret)
    }

    pub fn to_var_select(&self) -> VarSelect {
        let mut ret = vec![];
        for i in &self.select {
            let expr = ir::Expression::create_value(
                Value::new(*i as u64, 32, false),
                TokenRange::default(),
            );
            ret.push(expr);
        }
        VarSelect(ret, None)
    }
}

pub fn eval_array_literal(
    context: &mut Context,
    context_array: Option<&ShapeRef>,
    context_width: Option<&ShapeRef>,
    expr: &mut ir::Expression,
) -> IrResult<Option<Vec<ArrayLiteralExpression>>> {
    let token = expr.token_range();

    let ir::Expression::ArrayLiteral(items, _) = expr else {
        return Ok(None);
    };

    let (is_array, context_array_width) = if let Some(x) = context_array
        && !x.is_empty()
    {
        (true, context_array)
    } else {
        (false, context_width)
    };

    let mut ret = vec![];
    if let Some(array_width) = context_array_width {
        let mut default = None;
        let mut len = 0;
        let mut index = 0;

        for item in items {
            // context_array/context_width for inner item
            let next_array_width: Option<&ShapeRef> = if array_width.dims() < 2 {
                None
            } else {
                Some(array_width[1..].into())
            };
            let (next_array, next_width) = if is_array {
                (next_array_width, context_width)
            } else {
                (None, next_array_width)
            };

            match item {
                ir::ArrayLiteralItem::Value(expr, repeat) => {
                    let repeat = if let Some(repeat) = repeat {
                        let repeat =
                            eval_repeat(context, repeat).ok_or_else(|| ir_error!(token))?;
                        repeat.to_usize().unwrap_or(0)
                    } else {
                        1
                    };

                    let exprs = if let Some(x) =
                        eval_array_literal(context, next_array, next_width, expr)?
                    {
                        x
                    } else {
                        vec![ArrayLiteralExpression {
                            index: vec![],
                            select: vec![],
                            expr: expr.as_ref().clone(),
                        }]
                    };

                    for _ in 0..repeat {
                        let mut exprs = exprs.clone();
                        for expr in &mut exprs {
                            if is_array {
                                expr.index.insert(0, index);
                            } else {
                                expr.select.insert(0, index);
                            }
                        }
                        ret.append(&mut exprs);
                        index += 1;
                    }

                    len += repeat;
                }
                ir::ArrayLiteralItem::Defaul(expr) => {
                    let exprs = if let Some(x) =
                        eval_array_literal(context, next_array, next_width, expr)?
                    {
                        x
                    } else {
                        vec![ArrayLiteralExpression {
                            index: vec![],
                            select: vec![],
                            expr: expr.as_ref().clone(),
                        }]
                    };

                    if default.is_none() {
                        default = Some(exprs);
                    } else {
                        context.insert_error(AnalyzerError::multiple_default(
                            MultipleDefaultKind::ArrayLiteral,
                            "default",
                            &token,
                        ));
                        return Err(ir_error!(token));
                    }
                }
            }
        }

        if let Some(Some(target_len)) = array_width.get(0) {
            if let Some(x) = default {
                let remaining = target_len.checked_sub(len);
                if let Some(remaining) = remaining {
                    for _ in 0..remaining {
                        let mut exprs = x.clone();
                        for expr in &mut exprs {
                            if is_array {
                                expr.index.insert(0, index);
                            } else {
                                expr.select.insert(0, index);
                            }
                        }
                        ret.append(&mut exprs);
                        index += 1;
                    }
                } else {
                    context.insert_error(AnalyzerError::mismatch_type(
                        MismatchTypeKind::ArrayDimension {
                            expected: *target_len,
                            actual: len,
                        },
                        &token,
                    ));
                    return Err(ir_error!(token));
                }
            } else if *target_len != len {
                context.insert_error(AnalyzerError::mismatch_type(
                    MismatchTypeKind::ArrayDimension {
                        expected: *target_len,
                        actual: len,
                    },
                    &token,
                ));
                return Err(ir_error!(token));
            }
        } else {
            // TODO target_len is unknown
            return Err(ir_error!(token));
        }
    } else {
        // TODO error, not array context
        return Err(ir_error!(token));
    }

    Ok(Some(ret))
}

pub fn eval_repeat(context: &mut Context, expr: &mut ir::Expression) -> Option<Value> {
    let token = expr.token_range();
    let repeat = expr.eval_comptime(context, None);

    // array / type can't be operated
    if repeat.r#type.is_array() | repeat.r#type.is_type() {
        context.insert_error(AnalyzerError::invalid_operand(
            &repeat.r#type.to_string(),
            &Op::Repeat.to_string(),
            &token,
        ));
        return None;
    }

    if !repeat.is_const {
        context.insert_error(AnalyzerError::invalid_operand(
            "non const value",
            &Op::Repeat.to_string(),
            &token,
        ));
        return None;
    }

    match &repeat.value {
        ValueVariant::Numeric(repeat) => Some(repeat.clone()),
        ValueVariant::NumericArray(_) | ValueVariant::Type(_) => {
            context.insert_error(AnalyzerError::invalid_operand(
                &repeat.r#type.to_string(),
                &Op::Repeat.to_string(),
                &token,
            ));
            None
        }
        ValueVariant::Unknown => None,
    }
}

pub fn eval_size(
    context: &mut Context,
    expr: &Expression,
    allow_inferable_size: bool,
) -> IrResult<(Comptime, Option<usize>)> {
    let (comptime, expr) = eval_expr(context, None, expr, allow_inferable_size)?;
    if let Ok(x) = comptime.get_value() {
        let value = x.to_usize().unwrap_or(0);
        let value = context.check_size(value, expr.token_range());
        if value == Some(0) {
            Ok((comptime, None))
        } else {
            Ok((comptime, value))
        }
    } else {
        Ok((comptime, None))
    }
}

pub fn eval_assign_statement(
    context: &mut Context,
    dst: &mut ir::AssignDestination,
    expr: &mut (ir::Comptime, ir::Expression),
    token: TokenRange,
) -> IrResult<Vec<ir::Statement>> {
    let (comptime, expr) = expr;

    if (dst.comptime.r#type.is_clock() || dst.comptime.r#type.is_reset())
        && context.is_affiliated(Affiliation::AlwaysFf)
    {
        context.insert_error(AnalyzerError::invalid_clock_assignment(
            &dst.comptime.r#type.to_string(),
            &token,
        ));
    }

    if dst.comptime.clock_domain == ClockDomain::Implicit {
        let inferred = if context.is_affiliated(Affiliation::AlwaysFf) {
            context
                .current_clock
                .as_ref()
                .and_then(|c| c.clock_domain.domain_id())
        } else {
            comptime.clock_domain.domain_id()
        };
        if let Some(id) = inferred {
            dst.comptime.clock_domain = ClockDomain::Inferred(id);
            if let Some((_, path_comptime)) = context.var_paths.get_mut(&dst.path) {
                path_comptime.clock_domain = ClockDomain::Inferred(id);
            }
        }
    }

    let mut dst_comptime_for_cdc = dst.comptime.clone();
    dst_comptime_for_cdc.token = dst.token;
    check_clock_domain(context, &dst_comptime_for_cdc, comptime, &token.beg);

    if context.is_affiliated(Affiliation::AlwaysFf)
        && let Some(clock) = context.current_clock.clone()
    {
        check_clock_domain(context, &dst_comptime_for_cdc, &clock, &token.beg);
    }

    let width = dst.total_width(context);
    let mut ret = vec![];

    let array_exprs = eval_array_literal(
        context,
        Some(&dst.comptime.r#type.array),
        Some(dst.comptime.r#type.width()),
        expr,
    )?;
    if let Some(exprs) = array_exprs {
        for mut expr in exprs {
            check_reset_non_elaborative(context, &mut expr.expr);

            let index = expr.to_var_index();
            let select = expr.to_var_select();

            let mut dst = dst.clone();
            dst.index.append(&index);
            dst.select = select;

            let statement = ir::Statement::Assign(ir::AssignStatement {
                dst: vec![dst],
                width,
                expr: expr.expr,
                token,
            });
            ret.push(statement);
        }
    } else {
        check_reset_non_elaborative(context, expr);

        let statement = ir::Statement::Assign(ir::AssignStatement {
            dst: vec![dst.clone()],
            width,
            expr: expr.clone(),
            token,
        });
        ret.push(statement);
    }

    Ok(ret)
}

/// Expand an array-range assignment LHS (`o[0+:N] = '{...}`) into one assignment
/// per covered element (recursing inner dims for multi-dim arrays). Returns `None`
/// to defer to the scalar path when the LHS isn't a range or the RHS isn't an array
/// literal; an invalid slice is rejected.
pub fn eval_array_range_assign(
    context: &mut Context,
    lhs: &VarPathSelect,
    rhs: &Expression,
    token: TokenRange,
) -> IrResult<Option<Vec<ir::Statement>>> {
    let (_, select, _) = lhs.clone().into();
    let Some((_, mut comptime)) = context.find_path(&lhs.0) else {
        return Ok(None);
    };
    if let Some(part_select) = &comptime.part_select {
        comptime.r#type = part_select.base.clone();
    }
    let (array_select, _) = select.split(comptime.r#type.array.dims());
    if !array_select.is_range() {
        return Ok(None);
    }

    let dsts = lhs.clone().to_assign_destinations(context, false);
    if dsts.is_empty() {
        // Invalid range (to_assign_destinations validates and reports it). Type-check
        // the RHS, but with no `?`: a propagated error would roll back that diagnostic.
        let _ = eval_expr(context, None, rhs, false);
        return Ok(Some(vec![]));
    }

    // Type-check the RHS against the slice type ([n] prepended to the element dims).
    let n = dsts.len();
    let elem_type = dsts[0].comptime.r#type.clone();
    let mut sub_type = elem_type.clone();
    let mut dims = vec![Some(n)];
    dims.extend(elem_type.array.iter().copied());
    sub_type.array = Shape::new(dims);
    let (rhs_comptime, mut rhs_expr) = eval_expr(context, Some(sub_type), rhs, false)?;

    // Pair each outer literal item with one element dst; `eval_assign_statement`
    // recurses the element's inner dims. (A flat `eval_array_literal` would over-
    // recurse and mis-decompose a multi-dim element.) Non-literal RHS: defer.
    let ir::Expression::ArrayLiteral(items, _) = &mut rhs_expr else {
        return Ok(None);
    };
    let mut outer: Vec<ir::Expression> = Vec::new();
    let mut default: Option<ir::Expression> = None;
    let mut repeat_unresolved = false;
    for item in items.iter_mut() {
        match item {
            ir::ArrayLiteralItem::Value(expr, repeat) => {
                let count = if let Some(repeat) = repeat {
                    match eval_repeat(context, repeat).and_then(|v| v.to_usize()) {
                        Some(count) => count,
                        // Non-const repeat: already reported by eval_repeat. Skip the
                        // count below so we don't pile on a spurious dimension error.
                        None => {
                            repeat_unresolved = true;
                            0
                        }
                    }
                } else {
                    1
                };
                for _ in 0..count {
                    outer.push(expr.as_ref().clone());
                }
            }
            ir::ArrayLiteralItem::Defaul(expr) => {
                if default.is_some() {
                    context.insert_error(AnalyzerError::multiple_default(
                        MultipleDefaultKind::ArrayLiteral,
                        "default",
                        &token,
                    ));
                    return Ok(Some(vec![]));
                }
                default = Some(expr.as_ref().clone());
            }
        }
    }
    if let Some(def) = default {
        while outer.len() < n {
            outer.push(def.clone());
        }
    }
    // The one-level walk bypasses `eval_array_literal`'s count check, so verify the
    // item count here (else a mismatch is silently dropped and emits illegal SV).
    if outer.len() != n && !repeat_unresolved {
        context.insert_error(AnalyzerError::mismatch_type(
            MismatchTypeKind::ArrayDimension {
                expected: n,
                actual: outer.len(),
            },
            &token,
        ));
        return Ok(Some(vec![]));
    }

    let mut statements = vec![];
    for (mut dst, sub_expr) in dsts.into_iter().zip(outer) {
        let mut expr = (rhs_comptime.clone(), sub_expr);
        statements.extend(eval_assign_statement(context, &mut dst, &mut expr, token)?);
    }
    Ok(Some(statements))
}

fn eval_array_literal_expressions(
    context: &mut Context,
    r#type: &ir::Type,
    exprs: Vec<ArrayLiteralExpression>,
    token: TokenRange,
) -> IrResult<Vec<Value>> {
    let mut ret = vec![];

    let mut value: Option<Value> = None;
    let mut prev = None;
    for expr in exprs {
        if prev != Some(expr.index.clone())
            && let Some(x) = value
        {
            ret.push(x);
            value = None;
        }

        let mut part_type = r#type.clone();
        part_type.width_mut().drain(0..expr.select.len());

        if let Some(mut part_value) = expr.expr.eval_value(context) {
            let part_width = part_type.total_width().ok_or_else(|| ir_error!(token))?;
            part_value.trunc(part_width);

            value = if let Some(x) = value {
                Some(x.concat(&part_value))
            } else {
                Some(part_value)
            };
        } else {
            value = None;
        }

        prev = Some(expr.index);
    }

    if let Some(x) = value {
        ret.push(x);
    }

    Ok(ret)
}

/// Fold an array-literal expression into its per-element values, or `None`
/// when it isn't a resolvable array literal.
fn eval_array_literal_values(
    context: &mut Context,
    r#type: &ir::Type,
    expr: &mut ir::Expression,
) -> IrResult<Option<Vec<Value>>> {
    let token = expr.token_range();
    let Some(exprs) = eval_array_literal(context, Some(&r#type.array), Some(r#type.width()), expr)?
    else {
        return Ok(None);
    };
    Ok(Some(eval_array_literal_expressions(
        context, r#type, exprs, token,
    )?))
}

/// Register a const/param variable from its element `values`. The scalar arm
/// of `eval_const_assign` also expands struct/union members, so it can't use
/// this.
fn insert_const_variable(
    context: &mut Context,
    dst: &ir::AssignDestination,
    kind: VarKind,
    r#type: &ir::Type,
    comptime: Comptime,
    values: Vec<Value>,
) {
    let id = context.insert_var_path(dst.path.clone(), comptime);
    let array_limit = context.config.evaluate_array_limit;
    let variable = Variable::new(
        id,
        dst.path.clone(),
        kind,
        r#type.clone(),
        values,
        context.get_affiliation(),
        &dst.token,
        array_limit,
    );
    context.insert_variable(id, variable);
}

/// Fit each element to the destination element width. Values copied from a
/// source variable (inherited/sliced array params) keep the source width, so a
/// narrower signed source must be sign-extended -- the array-literal path can't
/// hit this because it evaluates each element in the destination type context.
/// A `None` width (e.g. `string`) is left untouched.
fn fit_array_elements(mut values: Vec<Value>, r#type: &ir::Type) -> Vec<Value> {
    if let Some(total_width) = r#type.total_width() {
        for value in &mut values {
            if value.width() > total_width {
                value.trunc(total_width);
            } else if value.width() < total_width {
                *value = value.expand(total_width, value.signed()).into_owned();
            }
        }
    }
    values
}

pub fn eval_const_assign(
    context: &mut Context,
    kind: VarKind,
    dst: &ir::AssignDestination,
    expr: &mut (ir::Comptime, ir::Expression),
) -> IrResult<()> {
    let (comptime, expr) = expr;
    let comptime = comptime.clone();
    let path = &dst.path;
    let r#type = &dst.comptime.r#type;
    let token = expr.token_range();

    match expr {
        // The guard skips an override literal, which already carries its values
        // as a NumericArray (below), so the literal isn't folded twice.
        ir::Expression::ArrayLiteral(_, _)
            if !matches!(comptime.value, ValueVariant::NumericArray(_)) =>
        {
            let Some(values) = eval_array_literal_values(context, r#type, expr)? else {
                return Err(ir_error!(token));
            };
            insert_const_variable(context, dst, kind, r#type, comptime, values);
        }
        _ => {
            match &comptime.value {
                ValueVariant::Numeric(value) => {
                    let id = context.insert_var_path(path.clone(), comptime.clone());

                    for x in r#type.expand_struct_union(path, &[], None) {
                        let r#type = x.part_select.last().unwrap().r#type.clone();
                        let mut comptime = Comptime::from_type(r#type, ClockDomain::None, token);
                        comptime.is_const = true;
                        let path = x.path.clone();
                        // Carry the field's bit offset so a member read selects
                        // that field instead of truncating the whole struct to
                        // its lowest field.
                        comptime.part_select = Some(x);
                        context.insert_var_path_with_id(path, id, comptime);
                    }

                    let mut value = value.clone();
                    if !comptime.r#type.is_string() {
                        let total_width = comptime
                            .r#type
                            .total_width()
                            .ok_or_else(|| ir_error!(token))?;
                        value.trunc(total_width);
                    }

                    let array_limit = context.config.evaluate_array_limit;
                    let variable = Variable::new(
                        id,
                        path.clone(),
                        kind,
                        r#type.clone(),
                        vec![value],
                        context.get_affiliation(),
                        &dst.token,
                        array_limit,
                    );
                    context.insert_variable(id, variable);
                }
                ValueVariant::NumericArray(values) => {
                    // An array override resolved by `get_overridden_params`
                    // (inheriting `#(XS)` or slicing `#(YS: XS[0])`). Not
                    // materializing it would leave later `XS[0]` reads dangling.
                    let values = fit_array_elements(values.clone(), r#type);
                    insert_const_variable(context, dst, kind, r#type, comptime, values);
                }
                ValueVariant::Type(x) => {
                    let mut comptime = comptime.clone();
                    comptime.value = ValueVariant::Type(x.clone());
                    context.insert_var_path(path.clone(), comptime);
                }
                ValueVariant::Unknown => {
                    context.insert_var_path(path.clone(), comptime);
                }
            }
        }
    }

    Ok(())
}

pub fn eval_variable(
    context: &mut Context,
    path: &VarPath,
    kind: VarKind,
    r#type: &ir::Type,
    clock_domain: ClockDomain,
    token: TokenRange,
) {
    let comptime = Comptime::from_type(r#type.clone(), clock_domain, token);
    let signed = comptime.r#type.signed;
    let id = context.insert_var_path(path.clone(), comptime);

    // Every element starts as the same x-state template, so store it once;
    // the simulator replicates it across the full `r#type.total_array()` at
    // fill time. Const/param arrays with per-element literals go through
    // `eval_array_literal` instead and keep their Vec<Value> intact.
    let values = if let Some(total_width) = r#type.total_width() {
        vec![Value::new_x(total_width, signed)]
    } else {
        vec![]
    };

    for x in r#type.expand_struct_union(path, &[], None) {
        let r#type = x.part_select.last().unwrap().r#type.clone();
        let mut comptime = Comptime::from_type(r#type, clock_domain, token);
        let path = x.path.clone();
        comptime.part_select = Some(x);
        context.insert_var_path_with_id(path, id, comptime);
    }

    let array_limit = context.config.evaluate_array_limit;
    let variable = Variable::new(
        id,
        path.clone(),
        kind,
        r#type.clone(),
        values,
        context.get_affiliation(),
        &token,
        array_limit,
    );
    context.insert_variable(id, variable);
}

fn check_reset_non_elaborative(context: &mut Context, expr: &mut ir::Expression) {
    let comptime = expr.eval_comptime(context, None);
    if context.in_if_reset && !comptime.is_const {
        context.insert_error(AnalyzerError::unevaluable_value(
            UnevaluableValueKind::ResetValue,
            &comptime.token,
        ));
    }
}

pub fn eval_struct_member(
    context: &mut Context,
    member_symbol: &Symbol,
    path: &GenericSymbolPath,
    mut member_path: VarPath,
    token: TokenRange,
) -> IrResult<ir::Factor> {
    fn get_member_type(context: &mut Context, member_symbol: &Symbol) -> IrResult<ir::Type> {
        let SymbolKind::StructMember(member) = &member_symbol.kind else {
            unreachable!();
        };

        member.r#type.to_ir_type(context, TypePosition::Variable)
    }

    let mut parent_path = path.clone();
    if let Some(x) = parent_path.paths.pop() {
        member_path.add_prelude(&[x.base.text]);
    }

    if let Ok(symbol) = symbol_table::resolve(&parent_path) {
        match &symbol.found.kind {
            SymbolKind::Parameter(x) if !x.is_proto => {
                if let Some(expr) = &x.value {
                    let path = VarPath::new(symbol.found.token.text);
                    let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
                    let (_, mut expr) = eval_expr(context, Some(r#type.clone()), expr, false)?;

                    member_path.add_prelude(&path.0);
                    for x in r#type.expand_struct_union(&path, &[], None) {
                        if x.path == member_path {
                            let mut comptime = expr.eval_comptime(context, None).clone();
                            comptime.token = token;
                            // Extract the field value from the full struct value.
                            // part_select encodes the bit position of the field: the sum of
                            // all pos values gives the LSB (end), and the last entry's type
                            // width gives the field width (beg = end + width - 1).
                            if let ValueVariant::Numeric(ref full_value) = comptime.value.clone() {
                                let end: usize = x.part_select.iter().map(|ps| ps.pos).sum();
                                if let Some(width) =
                                    x.part_select.last().and_then(|ps| ps.r#type.total_width())
                                {
                                    let beg = end + width - 1;
                                    comptime.value =
                                        ValueVariant::Numeric(full_value.select(beg, end));
                                }
                            }
                            comptime.r#type = get_member_type(context, member_symbol)?;
                            return Ok(ir::Factor::Value(comptime));
                        }
                    }
                }
                Err(ir_error!(token))
            }
            SymbolKind::Parameter(x) if x.is_proto => {
                let path = VarPath::new(symbol.found.token.text);
                let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
                member_path.add_prelude(&path.0);
                for x in r#type.expand_struct_union(&path, &[], None) {
                    if x.path == member_path {
                        let member_type = get_member_type(context, member_symbol)?;
                        let mut comptime =
                            Comptime::from_type(member_type, ClockDomain::None, token);
                        comptime.is_const = true;
                        return Ok(ir::Factor::Value(comptime));
                    }
                }
                Err(ir_error!(token))
            }
            SymbolKind::GenericParameter(x) => match &x.bound {
                GenericBoundKind::Type => {
                    let mut comptime = Comptime::create_unknown(token);
                    comptime.is_const = true;
                    Ok(ir::Factor::Value(comptime))
                }
                GenericBoundKind::Proto(x) => {
                    let r#type = x.to_ir_type(context, TypePosition::Variable)?;
                    let mut comptime = Comptime::from_type(r#type, ClockDomain::None, token);
                    comptime.is_const = true;
                    Ok(ir::Factor::Value(comptime))
                }
                _ => Err(ir_error!(token)),
            },
            SymbolKind::StructMember(_) => {
                eval_struct_member(context, member_symbol, &parent_path, member_path, token)
            }
            _ => Err(ir_error!(token)),
        }
    } else {
        Err(ir_error!(token))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TypePosition {
    Variable,
    Modport,
    Cast,
    Enum,
    Generic,
    TypeDef,
}

pub fn eval_type(
    context: &mut Context,
    path: &GenericSymbolPath,
    pos: TypePosition,
) -> IrResult<ir::Type> {
    // `_` is not a type; without this the decl emits an empty type keyword (invalid SV).
    if path.is_anonymous() {
        let token: TokenRange = path.range;
        context.insert_error(AnalyzerError::anonymous_identifier_usage(&token));
        return Err(ir_error!(token));
    }

    let mut width = Shape::default();
    let mut array = Shape::default();
    let mut signed = false;
    let mut is_positive = false;

    let kind = if let Some(x) = path.to_var_path()
        && let Some(x) = context.var_paths.get(&x)
    {
        match &x.1.value {
            ValueVariant::Type(x) => {
                let mut x = x.clone();

                // append internal width/array
                width.append(x.width_mut());
                array.append(&mut x.array);

                x.kind
            }
            ValueVariant::Numeric(x) => {
                let value = x.to_usize().unwrap_or(0);
                let value = context.check_size(value, path.paths[0].base.into());
                width.push(value);
                ir::TypeKind::Bit
            }
            _ => ir::TypeKind::Unknown,
        }
    } else {
        let mut path = context.resolve_path(path.clone());
        check_generic_refereence(context, &path);

        let map = path.to_generic_maps();
        if let Ok(symbol) = symbol_table::resolve(&path) {
            let type_error = match pos {
                TypePosition::Variable => !symbol.found.is_variable_type(),
                TypePosition::Cast => !symbol.found.is_casting_type(),
                _ => false,
            };

            if type_error {
                let token: TokenRange = symbol.found.token.into();
                context.insert_error(AnalyzerError::mismatch_type(
                    MismatchTypeKind::SymbolKind {
                        name: symbol.found.token.to_string(),
                        expected: "enum or union or struct".to_string(),
                        actual: symbol.found.kind.to_kind_name(),
                    },
                    &token,
                ));
            }

            match &symbol.found.kind {
                SymbolKind::Struct(x) => {
                    context.push_generic_map(map.clone());

                    let members = context.block(|c| {
                        let mut members = vec![];
                        for x in &x.members {
                            let member = symbol_table::get(*x).unwrap();
                            let name = member.token.text;

                            if let SymbolKind::StructMember(x) = member.kind {
                                if symbol.found.token.text == x.r#type.token.beg.text {
                                    // Prevent cyclic reference
                                    continue;
                                }

                                let r#type = x.r#type.to_ir_type(c, TypePosition::Variable)?;
                                members.push(ir::TypeKindMember { name, r#type });
                            }
                        }

                        check_struct_union_members(c, &members, &symbol.found);
                        Ok(members)
                    });

                    context.pop_generic_map();

                    ir::TypeKind::Struct(ir::TypeKindStruct {
                        id: symbol.found.id,
                        members: members?,
                    })
                }
                SymbolKind::Union(x) => {
                    context.push_generic_map(map.clone());

                    let members = context.block(|c| {
                        let mut members = vec![];
                        for x in &x.members {
                            let member = symbol_table::get(*x).unwrap();
                            let name = member.token.text;
                            if let SymbolKind::UnionMember(x) = member.kind {
                                if symbol.found.token.text == x.r#type.token.beg.text {
                                    // Prevent cyclic reference
                                    continue;
                                }

                                let r#type = x.r#type.to_ir_type(c, TypePosition::Variable)?;
                                members.push(ir::TypeKindMember { name, r#type });
                            }
                        }

                        check_struct_union_members(c, &members, &symbol.found);
                        Ok(members)
                    });

                    context.pop_generic_map();

                    ir::TypeKind::Union(ir::TypeKindUnion {
                        id: symbol.found.id,
                        members: members?,
                    })
                }
                SymbolKind::Enum(x) => {
                    let r#type = if let Some(enum_type) = &x.r#type {
                        context.push_generic_map(map.clone());

                        let mut ret =
                            context.block(|c| enum_type.to_ir_type(c, TypePosition::Enum));
                        if let Ok(r#type) = ret.as_mut() {
                            // Infer width from member variants
                            if r#type.is_inferable_width() {
                                r#type.width_mut().replace(0, Some(x.width));
                            }
                        }

                        context.pop_generic_map();

                        ret?
                    } else {
                        {
                            let mut t = ir::Type::new(ir::TypeKind::Logic);
                            t.set_concrete_width(Shape::new(vec![Some(x.width)]));
                            t
                        }
                    };
                    ir::TypeKind::Enum(ir::TypeKindEnum {
                        id: symbol.found.id,
                        r#type: Box::new(r#type),
                    })
                }
                SymbolKind::Modport(_) => {
                    // Remove modport name
                    path.paths.pop();
                    let token: TokenRange = symbol.found.token.into();
                    let sig =
                        Signature::from_path(context, path).ok_or_else(|| ir_error!(token))?;
                    ir::TypeKind::Modport(sig, symbol.found.token.text)
                }
                SymbolKind::TypeDef(x) if !x.is_proto => {
                    if let Some(ty) = &x.r#type
                        && context.push_typedef_visiting(symbol.found.id)
                    {
                        context.push_generic_map(map.clone());

                        let r#type = context.block(|c| ty.to_ir_type(c, TypePosition::TypeDef));

                        context.pop_generic_map();
                        context.pop_typedef_visiting();

                        let mut r#type = r#type?;

                        width.append(r#type.width_mut());
                        array.append(&mut r#type.array);
                        signed = r#type.signed;

                        r#type.kind
                    } else {
                        // Cyclic typedef chain (type_dag already reports it);
                        // stop expanding instead of recursing forever.
                        ir::TypeKind::Unknown
                    }
                }
                SymbolKind::Parameter(x) if x.is_proto => {
                    if x.r#type.kind.is_type() {
                        ir::TypeKind::Unknown
                    } else {
                        let token: TokenRange = symbol.found.token.into();
                        return Err(ir_error!(token));
                    }
                }
                SymbolKind::TypeDef(x) if x.is_proto => {
                    if let Some(x) = &x.r#type {
                        let mut r#type = x.to_ir_type(context, TypePosition::TypeDef)?;
                        width.append(r#type.width_mut());
                        array.append(&mut r#type.array);
                        signed = r#type.signed;

                        r#type.kind
                    } else {
                        ir::TypeKind::Unknown
                    }
                }
                SymbolKind::SystemVerilog => ir::TypeKind::SystemVerilog,
                SymbolKind::Interface(_) => {
                    let token: TokenRange = symbol.found.token.into();
                    let sig =
                        Signature::from_path(context, path).ok_or_else(|| ir_error!(token))?;
                    ir::TypeKind::Interface(sig)
                }
                SymbolKind::GenericParameter(x) => match &x.bound {
                    GenericBoundKind::Proto(x) => {
                        let mut r#type = x.to_ir_type(context, pos)?;
                        width.append(r#type.width_mut());
                        array.append(&mut r#type.array);
                        signed = r#type.signed;

                        r#type.kind
                    }
                    GenericBoundKind::Type => {
                        if let Some(x) = &x.default_value {
                            return eval_type(context, x, pos);
                        } else {
                            ir::TypeKind::Unknown
                        }
                    }
                    _ => ir::TypeKind::Unknown,
                },
                SymbolKind::GenericConst(x) => match &x.bound {
                    GenericBoundKind::Proto(x) => {
                        let mut r#type = x.to_ir_type(context, pos)?;
                        width.append(r#type.width_mut());
                        array.append(&mut r#type.array);
                        signed = r#type.signed;

                        r#type.kind
                    }
                    GenericBoundKind::Type => {
                        let (comptime, _) = eval_generic_expr(context, &x.value)?;
                        if let ValueVariant::Type(mut x) = comptime.value {
                            width.append(x.width_mut());
                            array.append(&mut x.array);
                            x.kind
                        } else {
                            ir::TypeKind::Unknown
                        }
                    }
                    _ => ir::TypeKind::Unknown,
                },
                SymbolKind::Parameter(x) if !x.is_proto => {
                    if x.r#type.kind.is_type() {
                        if let Some(expr) = &x.value {
                            context.push_generic_map(map.clone());

                            let expr = context.block(|c| eval_expr(c, None, expr, false));

                            context.pop_generic_map();

                            let (comptime, _) = expr?;
                            if let ValueVariant::Type(mut r#type) = comptime.value {
                                width.append(r#type.width_mut());
                                array.append(&mut r#type.array);
                                signed = r#type.signed;
                                r#type.kind
                            } else {
                                ir::TypeKind::Unknown
                            }
                        } else {
                            ir::TypeKind::Unknown
                        }
                    } else {
                        let token: TokenRange = symbol.found.token.into();
                        return Err(ir_error!(token));
                    }
                }
                SymbolKind::TbComponent(x)
                    if matches!(x.kind, TbComponentKind::File | TbComponentKind::External(_)) =>
                {
                    // The descriptor lives in the simulator (file table or
                    // component instance), so this slot is never read; back
                    // it with a throwaway 32-bit width.
                    if !context.in_test_module {
                        let token: TokenRange = path.paths[0].base.into();
                        context.insert_error(AnalyzerError::invalid_tb_usage(&token));
                    }
                    width.push(Some(32));
                    ir::TypeKind::Bit
                }
                _ => {
                    let token: TokenRange = symbol.found.token.into();
                    return Err(ir_error!(token));
                }
            }
        } else if matches!(path.kind, GenericSymbolPathKind::TypeLiteral)
            && path.paths.len() == 1
            && path.paths[0].arguments.is_empty()
        {
            // Fixed type given as generic arg
            match path.paths[0].base.to_string().as_str() {
                "p8" => {
                    width.push(Some(8));
                    is_positive = true;
                    ir::TypeKind::Bit
                }
                "p16" => {
                    width.push(Some(16));
                    is_positive = true;
                    ir::TypeKind::Bit
                }
                "p32" => {
                    width.push(Some(32));
                    is_positive = true;
                    ir::TypeKind::Bit
                }
                "p64" => {
                    width.push(Some(64));
                    is_positive = true;
                    ir::TypeKind::Bit
                }
                "u8" => {
                    width.push(Some(8));
                    ir::TypeKind::Bit
                }
                "u16" => {
                    width.push(Some(16));
                    ir::TypeKind::Bit
                }
                "u32" => {
                    width.push(Some(32));
                    ir::TypeKind::Bit
                }
                "u64" => {
                    width.push(Some(64));
                    ir::TypeKind::Bit
                }
                "i8" => {
                    width.push(Some(8));
                    signed = true;
                    ir::TypeKind::Bit
                }
                "i16" => {
                    width.push(Some(16));
                    signed = true;
                    ir::TypeKind::Bit
                }
                "i32" | "f32" => {
                    width.push(Some(32));
                    signed = true;
                    ir::TypeKind::Bit
                }
                "i64" | "f64" => {
                    width.push(Some(64));
                    signed = true;
                    ir::TypeKind::Bit
                }
                "bbool" => ir::TypeKind::Bit,
                "lbool" => ir::TypeKind::Logic,
                "string" => ir::TypeKind::String,
                _ => ir::TypeKind::Unknown,
            }
        } else {
            ir::TypeKind::Unknown
        }
    };

    let width_expr = ir::WidthExpr::from_shape(&width);
    let mut r#type = ir::Type::new(kind);
    r#type.signed = signed;
    r#type.is_positive = is_positive;
    r#type.array = array;
    if width_expr.len() == width.as_slice().len() {
        r#type.set_parametric_width(width, width_expr);
    } else {
        r#type.set_concrete_width(width);
    }
    Ok(r#type)
}

fn check_struct_union_members(
    context: &mut Context,
    members: &[ir::TypeKindMember],
    symbol: &Symbol,
) {
    if context.in_generic {
        return;
    }

    // SV-imported and unknown types have indeterminate 2/4-state, so accept them as either.
    let is_indeterminate = |t: &ir::Type| t.is_systemverilog() || t.is_unknown();
    let all_4state = members
        .iter()
        .all(|x| x.r#type.is_4state() || is_indeterminate(&x.r#type));
    let all_2state = members
        .iter()
        .all(|x| x.r#type.is_2state() || is_indeterminate(&x.r#type));
    if !(all_4state || all_2state) {
        let token: TokenRange = symbol.token.into();
        context.insert_error(AnalyzerError::mixed_struct_union_member(&token));
    }
}

pub fn eval_clock(context: &mut Context, value: &AlwaysFfDeclaration) -> IrResult<ir::FfClock> {
    let token: TokenRange = value.into();

    if let Some(x) = &value.always_ff_declaration_opt {
        let path = x
            .always_ff_event_list
            .always_ff_clock
            .hierarchical_identifier
            .as_ref();

        let path: VarPathSelect = Conv::conv(context, path)?;
        let (path, select, token) = path.into();

        if let Some((id, comptime)) = context.find_path(&path) {
            let (index, select) = select.split(comptime.r#type.array.dims());

            // Array select type check
            let _ = index.eval_comptime(context, &comptime.r#type, true);

            let index = index.to_index();

            let dim = comptime.r#type.selected_dimension(&index, &select);
            if !comptime.r#type.is_clock() || dim.0 != 0 || dim.1 != 0 || select.is_range() {
                context.insert_error(AnalyzerError::invalid_clock(
                    &path.to_string(),
                    &comptime.token,
                ));
            }

            Ok(ir::FfClock {
                id,
                index,
                select,
                comptime,
            })
        } else {
            Err(ir_error!(token))
        }
    } else if let Some((x, id)) = context.get_default_clock() {
        let token = value.always_ff.always_ff_token.token;
        symbol_table::add_reference(id, &token);
        Ok(x)
    } else {
        context.insert_error(AnalyzerError::missing_clock_signal(&token));
        Err(ir_error!(token))
    }
}

pub fn eval_reset(
    context: &mut Context,
    value: &AlwaysFfDeclaration,
) -> IrResult<Option<ir::FfReset>> {
    let token: TokenRange = value.into();

    if !value.has_if_reset() {
        if value.has_explicit_reset() {
            context.insert_error(AnalyzerError::missing_if_reset(&token));
        }
        return Ok(None);
    }

    if let Some(x) = &value.always_ff_declaration_opt
        && let Some(x) = &x.always_ff_event_list.always_ff_event_list_opt
    {
        let path = x.always_ff_reset.hierarchical_identifier.as_ref();
        let path: VarPathSelect = Conv::conv(context, path)?;
        let (path, select, token) = path.into();

        if let Some((id, comptime)) = context.find_path(&path) {
            let (index, select) = select.split(comptime.r#type.array.dims());

            // Array select type check
            let _ = index.eval_comptime(context, &comptime.r#type, true);

            let index = index.to_index();

            let dim = comptime.r#type.selected_dimension(&index, &select);
            if !comptime.r#type.is_reset() || dim.0 != 0 || dim.1 != 0 || select.is_range() {
                context.insert_error(AnalyzerError::invalid_reset(
                    &path.to_string(),
                    &comptime.token,
                ));
            }

            Ok(Some(ir::FfReset {
                id,
                index,
                select,
                comptime,
            }))
        } else {
            Err(ir_error!(token))
        }
    } else if let Some((x, id)) = context.get_default_reset() {
        let token = value.always_ff.always_ff_token.token;
        symbol_table::add_reference(id, &token);
        Ok(Some(x))
    } else {
        context.insert_error(AnalyzerError::missing_reset_signal(&token));
        Err(ir_error!(token))
    }
}

pub fn eval_width_select(
    context: &mut Context,
    path: &VarPath,
    r#type: &ir::Type,
    width_select: VarSelect,
) -> Option<VarSelect> {
    if r#type.is_struct_union() && !r#type.width().is_empty() && !width_select.is_empty() {
        let part_select = PartSelectPath {
            base: r#type.clone(),
            path: path.clone(),
            part_select: vec![],
        };
        part_select.to_base_select(context, &width_select)
    } else {
        Some(width_select)
    }
}

pub fn eval_generate_for_range(
    context: &mut Context,
    range: &Range,
    rev: bool,
    step: Option<(&AssignmentOperator, &Expression)>,
    token: TokenRange,
) -> IrResult<Vec<usize>> {
    let for_range = build_for_range_inner(context, range, rev, step, true)?;
    for_range.eval_iter(context).ok_or_else(|| ir_error!(token))
}

/// Build a `ForRange` from syntactic elements of a for-loop.
pub fn build_for_range(
    context: &mut Context,
    range: &Range,
    rev: bool,
    step: Option<(&AssignmentOperator, &Expression)>,
) -> IrResult<ir::ForRange> {
    build_for_range_inner(context, range, rev, step, false)
}

fn build_for_range_inner(
    context: &mut Context,
    range: &Range,
    rev: bool,
    step: Option<(&AssignmentOperator, &Expression)>,
    require_const: bool,
) -> IrResult<ir::ForRange> {
    let (beg, end, inclusive) = if require_const {
        eval_range_const(context, range)?
    } else {
        eval_range(context, range)?
    };

    if let Some((op, expr)) = step {
        let mut step_expr: ir::Expression = Conv::conv(context, expr)?;
        let step_comptime = step_expr.eval_comptime(context, None);
        let step_val = step_comptime.get_value()?.to_usize().unwrap_or(0);
        let op: ir::Op = Conv::conv(context, op)?;

        if matches!(op, ir::Op::Add) {
            // An additive step of 0 never advances the induction variable. The
            // unroll path bails in eval_iter, but a dynamic-range loop reaches
            // build_for_statement and would emit an infinite `for (; ; i += 0)`.
            if step_val == 0 {
                let token: TokenRange = expr.into();
                context.insert_error(AnalyzerError::invalid_for_step(
                    InvalidForStepKind::ZeroStep,
                    &token,
                ));
                return Err(ir_error!(token));
            }
            // Honor `rev` for additive steps so the unrolled iteration order
            // matches the emitted descending `for (i = hi; i >= lo; i -= step)`.
            if rev {
                Ok(ir::ForRange::Reverse {
                    start: beg,
                    end,
                    inclusive,
                    step: step_val,
                })
            } else {
                Ok(ir::ForRange::Forward {
                    start: beg,
                    end,
                    inclusive,
                    step: step_val,
                })
            }
        } else if rev {
            // A reverse loop is emitted as `for (i = hi; i >= lo; <step>)`, so a
            // non-additive step (`*=`, `<<=`, ...) cannot be inverted to descend
            // toward the lower bound and would never terminate. Reject it rather
            // than emit a broken loop.
            let token: TokenRange = range.into();
            context.insert_error(AnalyzerError::invalid_for_step(
                InvalidForStepKind::NonAdditiveReverse,
                &token,
            ));
            Err(ir_error!(token))
        } else {
            // A forward stepped loop runs while `i < end`, so the step must
            // strictly increase the induction variable. Ops that can only
            // hold or decrease it (and identity step values) would emit an
            // infinite SV loop while the unroller sees ≤1 iteration.
            let never_advances = match op {
                ir::Op::Sub
                | ir::Op::Div
                | ir::Op::Rem
                | ir::Op::BitAnd
                | ir::Op::ArithShiftR
                | ir::Op::LogicShiftR => true,
                ir::Op::Mul => step_val <= 1,
                ir::Op::BitOr | ir::Op::BitXor | ir::Op::ArithShiftL | ir::Op::LogicShiftL => {
                    step_val == 0
                }
                _ => false,
            };
            if never_advances {
                let token: TokenRange = expr.into();
                context.insert_error(AnalyzerError::invalid_for_step(
                    InvalidForStepKind::NeverAdvances,
                    &token,
                ));
                return Err(ir_error!(token));
            }
            // With const bounds, also catch value-dependent stalls (e.g.
            // `*= 2` starting at 0, or `|= k` once k's bits are set).
            if let (Some(start_val), Some(end_val)) =
                (beg.eval_value(context), end.eval_value(context))
            {
                let end_val = if inclusive {
                    end_val.saturating_add(1)
                } else {
                    end_val
                };
                let limit = context.config.evaluate_size_limit;
                let mut i = start_val;
                let mut n = 0usize;
                while i < end_val && n <= limit {
                    match op.eval(i, step_val) {
                        Some(next) if next > i => i = next,
                        _ => {
                            let token: TokenRange = expr.into();
                            context.insert_error(AnalyzerError::invalid_for_step(
                                InvalidForStepKind::StopsAdvancing,
                                &token,
                            ));
                            return Err(ir_error!(token));
                        }
                    }
                    n += 1;
                }
            }
            Ok(ir::ForRange::Stepped {
                start: beg,
                end,
                inclusive,
                step: step_val,
                op,
            })
        }
    } else if rev {
        Ok(ir::ForRange::Reverse {
            start: beg,
            end,
            inclusive,
            step: 1,
        })
    } else {
        Ok(ir::ForRange::Forward {
            start: beg,
            end,
            inclusive,
            step: 1,
        })
    }
}

/// Convert a `ForStatement` AST node into a runtime `ir::Statement::For`.
/// Used for dynamic-range for-loops that cannot be unrolled at compile time.
pub fn build_for_statement(
    context: &mut Context,
    value: &ForStatement,
    r#type: &ir::Type,
    clock_domain: ClockDomain,
    for_range: ir::ForRange,
    token: TokenRange,
) -> IrResult<ir::StatementBlock> {
    let var_name = value.identifier.text();

    let index = value.identifier.text();
    let path = VarPath::new(index);
    let kind = VarKind::Const;
    let comptime = Comptime::from_type(r#type.clone(), clock_domain, token);

    let loop_var_id = context.insert_var_path(path.clone(), comptime.clone());
    let values = if let Some(total_width) = r#type.total_width() {
        vec![Value::new_x(total_width, r#type.signed)]
    } else {
        vec![]
    };
    let array_limit = context.config.evaluate_array_limit;
    let variable = Variable::new(
        loop_var_id,
        path,
        kind,
        comptime.r#type.clone(),
        values,
        context.get_affiliation(),
        &token,
        array_limit,
    );
    context.insert_variable(loop_var_id, variable);

    let body: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref())?;

    Ok(ir::StatementBlock(vec![ir::Statement::For(Box::new(
        ir::ForStatement {
            var_id: loop_var_id,
            var_name,
            var_type: r#type.clone(),
            range: for_range,
            body: body.0,
            token,
        },
    ))]))
}

pub fn eval_function_call(
    context: &mut Context,
    value: &IdentifierFactor,
    token: TokenRange,
) -> IrResult<ir::Expression> {
    if let Some(x) = &value.identifier_factor_opt
        && let IdentifierFactorOptGroup::FunctionCall(x) = x.identifier_factor_opt_group.as_ref()
    {
        // A component method call in expression position hoists to its own
        // zero-time statement; builtin $tb methods return nothing. Testbench
        // components only exist inside #[test] modules, so the probe (one
        // extra resolve per multi-segment call) is skipped elsewhere.
        if context.in_test_module
            && let Some(tb_stmt) = tb_method_call(
                context,
                value.expression_identifier.as_ref(),
                &x.function_call,
                token,
                TbMethodCallPosition::Value,
            )?
        {
            let ir::Statement::TbMethodCall(method_call) = tb_stmt else {
                unreachable!()
            };
            if matches!(method_call.method, TbMethod::Component { .. }) {
                return hoist_component_method_call(context, method_call, token);
            }
            context.insert_error(AnalyzerError::invalid_factor(
                None,
                "testbench method without return value",
                &token,
                &[],
            ));
            return Err(ir_error!(token));
        }

        let args = if let Some(x) = &x.function_call.function_call_opt {
            argument_list(context, x.argument_list.as_ref())?
        } else {
            ir::Arguments::Null
        };

        let resolved_path = context.resolve_path(value.expression_identifier.as_ref().into());
        let symbol = symbol_table::resolve(&resolved_path).map_err(|_| ir_error!(token))?;

        match &symbol.found.kind {
            SymbolKind::SystemFunction(_) => {
                let name = symbol.found.token.text;
                let args = args.to_system_function_args(context, &symbol.found);
                let ret = ir::SystemFunctionCall::new(context, name, args, token)?;
                Ok(ir::Expression::Term(Box::new(
                    ir::Factor::SystemFunctionCall(ret),
                )))
            }
            SymbolKind::Function(x) if x.is_proto => {
                if context.in_generic {
                    let ret =
                        function_call(context, value.expression_identifier.as_ref(), args, token)?;

                    Ok(ir::Expression::Term(Box::new(ir::Factor::FunctionCall(
                        ret,
                    ))))
                } else {
                    Err(ir_error!(token))
                }
            }
            SymbolKind::Function(_) | SymbolKind::ModportFunctionMember(_) => {
                let ret =
                    function_call(context, value.expression_identifier.as_ref(), args, token)?;

                Ok(ir::Expression::Term(Box::new(ir::Factor::FunctionCall(
                    ret,
                ))))
            }
            SymbolKind::SystemVerilog => {
                let mut x = Comptime::create_unknown(token);
                x.is_const = true;
                Ok(ir::Expression::Term(Box::new(ir::Factor::Value(x))))
            }
            _ => {
                let name = symbol.found.token.text.to_string();
                let kind = symbol.found.kind.to_kind_name();
                context.insert_error(AnalyzerError::call_non_function(&name, &kind, &token));
                Err(ir_error!(token))
            }
        }
    } else {
        unreachable!();
    }
}

pub fn eval_struct_constructor(
    context: &mut Context,
    value: &IdentifierFactor,
    token: TokenRange,
) -> IrResult<ir::Expression> {
    if let Some(x) = &value.identifier_factor_opt
        && let IdentifierFactorOptGroup::StructConstructor(x) =
            x.identifier_factor_opt_group.as_ref()
    {
        let items: Vec<_> = x.struct_constructor.struct_constructor_list.as_ref().into();
        let default = &x.struct_constructor.struct_constructor_opt;

        let path: GenericSymbolPath = value.expression_identifier.as_ref().into();
        let r#type = symbol::Type {
            modifier: vec![],
            kind: symbol::TypeKind::UserDefined(symbol::UserDefinedType { path, symbol: None }),
            width: vec![],
            array: vec![],
            array_type: None,
            is_const: false,
            token,
        };
        let r#type = r#type.to_ir_type(context, TypePosition::Variable)?;

        let members = match &r#type.kind {
            ir::TypeKind::Struct(x) => &x.members,
            ir::TypeKind::Union(x) => &x.members,
            ir::TypeKind::SystemVerilog => &vec![],
            _ => {
                context.insert_error(AnalyzerError::mismatch_type(
                    MismatchTypeKind::NonStructUnionType {
                        actual: r#type.kind.to_string(),
                    },
                    &token,
                ));
                return Err(ir_error!(token));
            }
        };

        let mut exprs = HashMap::default();
        for item in items {
            let name = item.identifier.text();
            exprs.insert(name, item.expression.as_ref().clone());
        }

        let mut ret = vec![];
        for x in members {
            let (_, expr) = if let Some(expr) = exprs.get(&x.name) {
                eval_expr(context, Some(x.r#type.clone()), expr, false)?
            } else if let Some(expr) = default {
                eval_expr(context, Some(x.r#type.clone()), &expr.expression, false)?
            } else {
                context.insert_error(AnalyzerError::unknown_member(
                    "struct constructor",
                    &x.name.to_string(),
                    &token,
                ));
                return Err(ir_error!(token));
            };

            ret.push((x.name, expr));
        }

        let comptime = Box::new(Comptime::create_unknown(token));
        Ok(ir::Expression::StructConstructor(
            Box::new(r#type),
            ret,
            comptime,
        ))
    } else {
        unreachable!();
    }
}

/// A member path whose first segment is a module instance: a hierarchical
/// testbench reference (`dut.u_core.pc`) or one of its misuse forms.
enum HierReference {
    /// Instance-name path, variable path within the target module, and the
    /// variable's type.
    Resolved(Vec<StrId>, VarPath, Box<ir::Type>),
    UnknownMember {
        owner: String,
        member: StrId,
    },
    /// First segment is a scalar module instance that has not been
    /// converted yet (declared after the referencing statement).
    NotDeclaredYet(StrId),
    InstanceArray(StrId),
    NotHier,
}

fn classify_hier_reference(context: &Context, path: &VarPath) -> HierReference {
    let segs = &path.0;
    if segs.len() < 2 {
        return HierReference::NotHier;
    }

    if let Some(component) = context.inst_components.get(&segs[0]) {
        let mut component = component.as_ref();
        let mut inst_path = vec![segs[0]];
        let mut i = 1;

        loop {
            let ir::Component::Module(module) = component else {
                return HierReference::NotHier;
            };

            // The remaining segments may be a variable path in this module
            // (multi-segment for interface-flattened members) ...
            let var_path = VarPath::from_slice(&segs[i..]);
            if let Some(variable) = module.variables.values().find(|v| v.path == var_path) {
                return HierReference::Resolved(
                    inst_path,
                    var_path,
                    Box::new(variable.r#type.clone()),
                );
            }

            // ... or descend into the child instance whose qualified path
            // (generate prefix then name) begins the remaining segments.
            let child = module.declarations.iter().find_map(|x| {
                let ir::Declaration::Inst(inst) = x else {
                    return None;
                };
                let consumed = ir::qualified_prefix_len(&inst.hierarchy, inst.name, &segs[i..])?;
                Some((&inst.component, consumed))
            });
            let Some((child, consumed)) = child else {
                let owner = inst_path
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                return HierReference::UnknownMember {
                    owner,
                    member: segs[i],
                };
            };
            inst_path.extend_from_slice(&segs[i..i + consumed]);
            component = child.as_ref();
            i += consumed;
            if i >= segs.len() {
                let owner = inst_path
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                return HierReference::UnknownMember {
                    owner,
                    member: *segs.last().unwrap(),
                };
            }
        }
    }

    // Not converted (yet); classify through the symbol table so misuse gets
    // a diagnostic instead of a silent conversion failure.
    let Some(namespace) = context.current_namespace() else {
        return HierReference::NotHier;
    };
    let Ok(symbol) = symbol_table::resolve((segs[0], &namespace)) else {
        return HierReference::NotHier;
    };
    let SymbolKind::Instance(prop) = &symbol.found.kind else {
        return HierReference::NotHier;
    };
    let Ok(type_symbol) =
        symbol_table::resolve((&prop.type_name.generic_path(), &symbol.found.namespace))
    else {
        return HierReference::NotHier;
    };
    let is_module = match &type_symbol.found.kind {
        SymbolKind::Module(_) | SymbolKind::AliasModule(_) => true,
        SymbolKind::GenericInstance(x) => matches!(
            symbol_table::get(x.base).map(|x| x.kind),
            Some(SymbolKind::Module(_))
        ),
        _ => false,
    };
    if !is_module {
        return HierReference::NotHier;
    }

    if prop.array.is_empty() {
        HierReference::NotDeclaredYet(segs[0])
    } else {
        HierReference::InstanceArray(segs[0])
    }
}

pub fn eval_factor_path(
    context: &mut Context,
    symbol_path: GenericSymbolPath,
    var_path: VarPathSelect,
    allow_unknown_value: bool,
    token: TokenRange,
) -> IrResult<ir::Factor> {
    // Bounds recursion when a recursive generic function's self-call can't be
    // resolved to a base case here; otherwise it recurses until the native
    // stack overflows.
    context.function_eval_depth += 1;
    let ret = if context.function_eval_depth > context.config.function_instance_depth_limit {
        if context.function_eval_overflow.is_none() {
            context.function_eval_overflow = Some((token, context.function_eval_depth));
        }
        Err(ir_error!(token))
    } else {
        eval_factor_path_inner(context, symbol_path, var_path, allow_unknown_value, token)
    };
    context.function_eval_depth -= 1;
    ret
}

fn eval_factor_path_inner(
    context: &mut Context,
    symbol_path: GenericSymbolPath,
    var_path: VarPathSelect,
    allow_unknown_value: bool,
    token: TokenRange,
) -> IrResult<ir::Factor> {
    let (path, select, _) = var_path.into();

    let mut generic_path = context.resolve_path(symbol_path);
    generic_path.unalias(None);
    check_generic_refereence(context, &generic_path);

    let found = if let Some(path) = generic_path.to_var_path()
        && let Some((var_id, comptime)) = context.find_path(&path)
    {
        Some((var_id, comptime))
    } else {
        context.find_path(&path)
    };

    // Classified even outside initial/final blocks: reference_table
    // suppresses invisible_identifier for these paths, so misuse inside a
    // test module must be diagnosed here.
    let hier = if found.is_none() && context.in_test_module {
        classify_hier_reference(context, &path)
    } else {
        HierReference::NotHier
    };

    if let Some((var_id, mut comptime)) = found {
        if let Some(part_select) = &comptime.part_select {
            comptime.r#type = part_select.base.clone();
        }

        let (array_select, width_select) = select.split(comptime.r#type.array.dims());

        // Array select type check
        let _ = array_select.eval_comptime(context, &comptime.r#type, true);

        let width_select = if let Some(part_select) = &comptime.part_select {
            part_select.to_base_select(context, &width_select)
        } else {
            eval_width_select(context, &path, &comptime.r#type, width_select)
        };
        let width_select = width_select.ok_or_else(|| ir_error!(token))?;

        comptime.is_global = false;

        if array_select.is_range() {
            // TODO
            Err(ir_error!(token))
        } else {
            let index = array_select.to_index();
            comptime.r#type.array.drain(0..index.dimension());

            // A const symbol read through a dynamic index/select is not itself
            // a compile-time constant (e.g. `A[idx]` for a const array `A`).
            comptime.is_const &= index.is_const() && width_select.is_const();

            comptime.token = token;
            if comptime.r#type.is_type() {
                Ok(ir::Factor::Value(comptime))
            } else {
                // Params arrive with evaluated=true (set by eval_expr), which
                // would make gather_context skip applying the select width —
                // and skip the index/select clock-domain check, laundering a
                // foreign-domain index into a const lookup table.
                if !width_select.is_empty() || !index.is_const() {
                    comptime.evaluated = false;
                }
                Ok(ir::Factor::Variable(var_id, index, width_select, comptime))
            }
        }
    } else if let HierReference::Resolved(inst_path, var_path, r#type) = hier {
        if !context.in_tb_block {
            context.insert_error(AnalyzerError::invisible_identifier(
                &path.0[1].to_string(),
                &token,
            ));
            return Err(ir_error!(token));
        }

        let mut comptime = Comptime::from_type(*r#type, ClockDomain::None, token);

        let (array_select, width_select) = select.split(comptime.r#type.array.dims());
        let _ = array_select.eval_comptime(context, &comptime.r#type, true);
        let width_select = eval_width_select(context, &var_path, &comptime.r#type, width_select)
            .ok_or_else(|| ir_error!(token))?;

        if array_select.is_range() {
            Err(ir_error!(token))
        } else {
            let index = array_select.to_index();
            comptime.r#type.array.drain(0..index.dimension());

            if !width_select.is_empty() {
                comptime.r#type.flatten_struct_union_enum();
                if let Some(width) = width_select.eval_comptime(context, &comptime.r#type, false) {
                    comptime.r#type.set_concrete_width(width);
                }
            }
            comptime.token = token;

            Ok(ir::Factor::HierVariable(Box::new(ir::HierVarRef {
                inst_path,
                var_path,
                index,
                select: width_select,
                comptime,
            })))
        }
    } else if !matches!(hier, HierReference::NotHier) {
        match hier {
            HierReference::UnknownMember { owner, member } => {
                context.insert_error(AnalyzerError::unknown_member(
                    &owner,
                    &member.to_string(),
                    &token,
                ));
            }
            HierReference::NotDeclaredYet(name) => {
                if context.in_tb_block {
                    context.insert_error(AnalyzerError::referring_before_definition(
                        &name.to_string(),
                        &token,
                    ));
                } else {
                    context.insert_error(AnalyzerError::invisible_identifier(
                        &path.0[1].to_string(),
                        &token,
                    ));
                }
            }
            HierReference::InstanceArray(name) => {
                context.insert_error(AnalyzerError::invalid_factor(
                    Some(&name.to_string()),
                    "instance array",
                    &token,
                    &[],
                ));
            }
            HierReference::Resolved(..) | HierReference::NotHier => unreachable!(),
        }
        Err(ir_error!(token))
    } else if let Some(x) = generic_path.to_literal() {
        let x = x.eval_comptime(token);
        Ok(ir::Factor::Value(x))
    } else if generic_path.is_anonymous() {
        let mut comptime = Comptime::create_unknown(token);
        comptime.is_const = true;
        comptime.is_global = true;
        Ok(ir::Factor::Anonymous(comptime))
    } else if let Ok(symbol) = symbol_table::resolve(&generic_path) {
        let is_inernal = context
            .current_namespace()
            .map(|x| symbol.found.namespace.included(&x))
            .unwrap_or(false);
        if is_inernal {
            eval_factor_symbol(
                context,
                generic_path,
                (*symbol).clone(),
                allow_unknown_value,
                token,
            )
        } else {
            let maps = generic_path.to_generic_maps();
            eval_factor_symbol_external(
                context,
                generic_path,
                (*symbol).clone(),
                maps,
                allow_unknown_value,
                token,
            )
        }
    } else {
        Err(ir_error!(token))
    }
}

/// Evaluates a resolved symbol that lives outside the current module: an
/// independent context avoids name conflicts, with the given generic
/// arguments applied.
pub fn eval_factor_symbol_external(
    context: &mut Context,
    path: GenericSymbolPath,
    symbol: ResolveResult,
    maps: Vec<GenericMap>,
    allow_unknown_value: bool,
    token: TokenRange,
) -> IrResult<ir::Factor> {
    let mut external_context = Context::default();
    external_context.inherit(context);

    external_context.push_generic_map(maps);
    let ret =
        external_context.block(|c| eval_factor_symbol(c, path, symbol, allow_unknown_value, token));

    external_context.pop_generic_map();
    context.inherit(&mut external_context);

    ret
}

pub fn eval_factor_symbol(
    context: &mut Context,
    path: GenericSymbolPath,
    symbol: ResolveResult,
    allow_unknown_value: bool,
    token: TokenRange,
) -> IrResult<ir::Factor> {
    match &symbol.found.kind {
        SymbolKind::Parameter(x) if !x.is_proto => {
            // Parameter should be found through context.find_path from the defined namespace
            if let Some(namespace) = context.current_namespace()
                && symbol.found.namespace.included(&namespace)
            {
                context.insert_error(AnalyzerError::referring_before_definition(
                    &symbol.found.token.to_string(),
                    &token,
                ));
            }

            if let Some(expr) = &x.value {
                let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
                let (mut comptime, _) =
                    context.block(|c| eval_expr(c, Some(r#type), expr, false))?;

                if let Some(width) = comptime.r#type.total_width() {
                    comptime.value.expand_value(width);
                }

                comptime.token = token;
                return Ok(ir::Factor::Value(comptime));
            }
        }
        SymbolKind::GenericParameter(x) => {
            let default_value = if let Some(x) = &x.default_value
                && let Some(x) = x.to_literal()
            {
                let x = x.eval_comptime(token);
                Some(x)
            } else if allow_unknown_value {
                None
            } else {
                return Err(ir_error!(token));
            };

            if let Ok(proto) = x.bound.resolve_proto_bound(&symbol.found.namespace) {
                match proto {
                    ProtoBound::ProtoModule(_)
                    | ProtoBound::ProtoInterface(_)
                    | ProtoBound::ProtoPackage(_)
                        if context.allow_component_as_factor =>
                    {
                        context.allow_component_as_factor = false;
                        return Ok(ir::Factor::from_component_symbol(&symbol.found, token));
                    }
                    ProtoBound::FactorType(x)
                    | ProtoBound::Enum((_, x))
                    | ProtoBound::Struct((_, x))
                    | ProtoBound::Union((_, x)) => {
                        let r#type = x.to_ir_type(context, TypePosition::Generic)?;
                        let mut x = Comptime::from_type(r#type, ClockDomain::None, token);

                        if let Some(val) = default_value {
                            x.value = val.value;
                        }

                        // GenericParameter is const and global
                        x.is_const = true;
                        x.is_global = true;

                        return Ok(ir::Factor::Value(x));
                    }
                    _ => {}
                }
            } else if matches!(x.bound, GenericBoundKind::Type) {
                let mut x = Comptime::create_unknown(token);

                if let Some(val) = default_value {
                    x.value = val.value;
                }

                // GenericParameter is const and global
                x.is_const = true;
                x.is_global = true;
                x.r#type.kind = ir::TypeKind::Type;

                return Ok(ir::Factor::Value(x));
            } else {
                context.insert_error(AnalyzerError::invalid_factor(
                    Some(&symbol.found.token.to_string()),
                    &symbol.found.kind.to_kind_name(),
                    &token,
                    &[],
                ));
            }
        }
        SymbolKind::GenericConst(x) => {
            if let Ok(proto) = x.bound.resolve_proto_bound(&symbol.found.namespace) {
                match proto {
                    ProtoBound::ProtoModule(_)
                    | ProtoBound::ProtoInterface(_)
                    | ProtoBound::ProtoPackage(_)
                        if context.allow_component_as_factor =>
                    {
                        context.allow_component_as_factor = false;
                        return Ok(ir::Factor::from_component_symbol(&symbol.found, token));
                    }
                    ProtoBound::FactorType(t)
                    | ProtoBound::Enum((_, t))
                    | ProtoBound::Struct((_, t))
                    | ProtoBound::Union((_, t)) => {
                        let r#type = t.to_ir_type(context, TypePosition::Generic)?;

                        let mut comptime = if context.in_generic {
                            // RHS value of generic const may not be evaluated in generic component
                            Comptime::from_type(r#type.clone(), ClockDomain::None, token)
                        } else {
                            let (mut comptime, _) = eval_generic_expr(context, &x.value)?;
                            comptime.r#type = r#type;
                            comptime
                        };
                        comptime.is_const = true;
                        comptime.is_global = true;
                        comptime.token = token;
                        return Ok(ir::Factor::Value(comptime));
                    }
                    _ => {}
                }
            } else if matches!(x.bound, GenericBoundKind::Type) {
                let (mut comptime, _) = eval_generic_expr(context, &x.value)?;
                comptime.is_const = true;
                comptime.is_global = true;
                comptime.token = token;
                return Ok(ir::Factor::Value(comptime));
            }
        }
        SymbolKind::Parameter(x) if x.is_proto && allow_unknown_value => {
            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
            let mut x = Comptime::from_type(r#type, ClockDomain::None, token);

            x.is_const = true;
            x.is_global = true;

            return Ok(ir::Factor::Value(x));
        }
        SymbolKind::TypeDef(x) if x.is_proto => {
            if allow_unknown_value {
                let mut x = Comptime::create_unknown(token);

                x.is_const = true;
                x.is_global = true;

                return Ok(ir::Factor::Value(x));
            }
        }
        SymbolKind::EnumMember(x) => {
            let enum_symbol = symbol.found.get_parent().unwrap();
            let SymbolKind::Enum(r#enum) = enum_symbol.kind else {
                unreachable!();
            };

            let factor = match &x.value {
                EnumMemberValue::Unresolved => {
                    context.insert_error(AnalyzerError::referring_before_definition(
                        &symbol.found.token.to_string(),
                        &token,
                    ));
                    None
                }
                EnumMemberValue::ImplicitValue(x) => {
                    let value = Value::new_biguint(x.clone(), r#enum.width, false);
                    Some(ir::Factor::create_value(value, token))
                }
                EnumMemberValue::ExplicitValue(x, _) => {
                    let (mut x, _) = eval_expr(context, None, x, false)?;
                    x.token = token;
                    Some(ir::Factor::Value(x))
                }
                EnumMemberValue::UnevaluableValue => None,
            };

            if let Some(mut factor) = factor {
                let enum_type = factor.comptime().r#type.clone();
                let type_kind = ir::TypeKind::Enum(ir::TypeKindEnum {
                    id: enum_symbol.id,
                    r#type: Box::new(enum_type),
                });

                let factor_comptime = factor.comptime_mut();
                factor_comptime.r#type.kind = type_kind;
                // The member width lives in TypeKindEnum, so the outer shape must
                // be scalar (else total_width squares it), like to_ir_type's enums.
                factor_comptime.r#type.set_concrete_width(Shape::default());

                return Ok(factor);
            }
        }
        SymbolKind::StructMember(_) => {
            // Module local variable should be found through context.find_path
            let module_variable = symbol.full_path.iter().any(|x| {
                let symbol = symbol_table::get(*x).unwrap();
                if let SymbolKind::Variable(x) = symbol.kind {
                    x.affiliation == Affiliation::Module
                } else {
                    false
                }
            });
            if module_variable {
                context.insert_error(AnalyzerError::referring_before_definition(
                    &symbol.found.token.to_string(),
                    &token,
                ));
            }
            return eval_struct_member(context, &symbol.found, &path, VarPath::default(), token);
        }
        SymbolKind::SystemVerilog => {
            let r#type = ir::Type::new(ir::TypeKind::SystemVerilog);
            let mut x = Comptime::from_type(r#type, ClockDomain::None, token);

            // $sv member is const / global
            x.is_const = true;
            x.is_global = true;

            return Ok(ir::Factor::Value(x));
        }
        SymbolKind::Module(_)
        | SymbolKind::AliasModule(_)
        | SymbolKind::Interface(_)
        | SymbolKind::AliasInterface(_)
        | SymbolKind::Package(_)
        | SymbolKind::AliasPackage(_) => {
            if context.allow_component_as_factor {
                context.allow_component_as_factor = false;
                return Ok(ir::Factor::from_component_symbol(&symbol.found, token));
            } else {
                context.insert_error(AnalyzerError::invalid_factor(
                    Some(&symbol.found.token.to_string()),
                    &symbol.found.kind.to_kind_name(),
                    &token,
                    &[],
                ));
            }
        }
        SymbolKind::Instance(x) if context.allow_component_as_factor => {
            context.allow_component_as_factor = false;
            if let Ok(component) =
                symbol_table::resolve_generic_structural(&x.type_name, &symbol.found.namespace)
            {
                let sig = Signature::new(component.found.id);
                let kind = if symbol.found.is_module(true) {
                    ir::TypeKind::Instance(sig, ir::InstanceKind::Module)
                } else if symbol.found.is_interface(true) {
                    ir::TypeKind::Instance(sig, ir::InstanceKind::Interface)
                } else {
                    ir::TypeKind::Instance(sig, ir::InstanceKind::SystemVerilog)
                };
                let r#type = ir::Type::new(kind);

                let comptime = Comptime::from_type(r#type, ClockDomain::None, token);
                return Ok(ir::Factor::Value(comptime));
            }
        }
        SymbolKind::Function(x) if !x.is_proto => {
            context.insert_error(AnalyzerError::invalid_factor(
                Some(&symbol.found.token.to_string()),
                &symbol.found.kind.to_kind_name(),
                &token,
                &[],
            ));
        }
        SymbolKind::SystemFunction(_) => {
            context.insert_error(AnalyzerError::invalid_factor(
                Some(&symbol.found.token.to_string()),
                &symbol.found.kind.to_kind_name(),
                &token,
                &[],
            ));
        }
        // Mangled enum member can't be used directly
        SymbolKind::EnumMemberMangled => {
            context.insert_error(AnalyzerError::undefined_identifier(
                &symbol.found.token.to_string(),
                &symbol.found.token.into(),
            ));
        }
        SymbolKind::Variable(x) => {
            // Module local variable should be found through context.find_path
            if x.affiliation == Affiliation::Module {
                context.insert_error(AnalyzerError::referring_before_definition(
                    &symbol.found.token.to_string(),
                    &token,
                ));
            }

            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
            let x = Comptime::from_type(r#type, x.clock_domain, token);

            return Ok(ir::Factor::Value(x));
        }
        // Reached only when evaluated without IR context (e.g. cast emit in emitter)
        SymbolKind::Port(x) => {
            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
            let x = Comptime::from_type(r#type, x.clock_domain, token);

            return Ok(ir::Factor::Value(x));
        }
        SymbolKind::ModportVariableMember(x) => {
            let variable = symbol_table::get(x.variable).unwrap();
            if let SymbolKind::Variable(x) = &variable.kind {
                let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
                let x = Comptime::from_type(r#type, x.clock_domain, token);

                return Ok(ir::Factor::Value(x));
            }
        }
        SymbolKind::Enum(_)
        | SymbolKind::Struct(_)
        | SymbolKind::Union(_)
        | SymbolKind::TypeDef(_) => {
            let r#type = symbol::Type {
                modifier: vec![],
                kind: symbol::TypeKind::UserDefined(symbol::UserDefinedType { path, symbol: None }),
                width: vec![],
                array: vec![],
                array_type: None,
                is_const: true,
                token,
            };
            let r#type = r#type.to_ir_type(context, TypePosition::Variable)?;
            let x = Comptime {
                value: ValueVariant::Type(r#type),
                r#type: ir::Type::new(ir::TypeKind::Type),
                is_const: true,
                is_global: true,
                token,
                ..Default::default()
            };

            return Ok(ir::Factor::Value(x));
        }
        SymbolKind::ProjectProperty(x) => {
            let value = match &x.value {
                veryl_metadata::ProjectProperty::Int(x) => Value::new(*x as u64, 64, true),
                veryl_metadata::ProjectProperty::Bool(x) => {
                    if *x {
                        Value::new(1, 1, false)
                    } else {
                        Value::new(0, 1, false)
                    }
                }
            };
            let comptime = Comptime::create_value(value, symbol.found.token.into());
            return Ok(ir::Factor::Value(comptime));
        }
        _ => (),
    }
    Err(ir_error!(token))
}

/// Extract source-level `CasePattern`s from a `CaseCondition` (one per
/// `RangeItem`).
pub fn case_patterns(
    context: &mut Context,
    cond: &CaseCondition,
) -> IrResult<Vec<ir::CasePattern>> {
    let mut ret = Vec::with_capacity(1 + cond.case_condition_list.len());
    ret.push(range_item_pattern(context, &cond.range_item)?);
    for x in &cond.case_condition_list {
        ret.push(range_item_pattern(context, &x.range_item)?);
    }
    Ok(ret)
}

fn range_item_pattern(context: &mut Context, range_item: &RangeItem) -> IrResult<ir::CasePattern> {
    let mut lo: ir::Expression = Conv::conv(context, range_item.range.expression.as_ref())?;

    let comptime = lo.eval_comptime(context, None);
    if !comptime.is_const {
        context.insert_error(AnalyzerError::unevaluable_value(
            UnevaluableValueKind::CaseCondition,
            &range_item.into(),
        ));
    }

    let Some(opt) = &range_item.range.range_opt else {
        return Ok(ir::CasePattern::Eq(Box::new(lo)));
    };
    let mut hi: ir::Expression = Conv::conv(context, opt.expression.as_ref())?;
    let comptime = hi.eval_comptime(context, None);
    if !comptime.is_const {
        context.insert_error(AnalyzerError::unevaluable_value(
            UnevaluableValueKind::CaseCondition,
            &range_item.into(),
        ));
    }
    let inclusive = matches!(opt.range_operator.as_ref(), RangeOperator::DotDotEqu(_));
    Ok(ir::CasePattern::Range {
        lo: Box::new(lo),
        hi: Box::new(hi),
        inclusive,
    })
}

pub fn case_condition(
    context: &mut Context,
    tgt: &ir::Expression,
    cond: &CaseCondition,
) -> IrResult<ir::Expression> {
    let mut ret = range_item(context, tgt, &cond.range_item)?;
    for x in &cond.case_condition_list {
        let item = range_item(context, tgt, &x.range_item)?;
        let comptime = Box::new(Comptime::create_unknown(item.token_range()));
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(item), comptime);
    }
    Ok(ret)
}

pub fn range_list(
    context: &mut Context,
    tgt: &ir::Expression,
    list: &RangeList,
) -> IrResult<ir::Expression> {
    let mut ret = range_item(context, tgt, &list.range_item)?;
    for x in &list.range_list_list {
        let item = range_item(context, tgt, &x.range_item)?;
        let comptime = Box::new(Comptime::create_unknown(item.token_range()));
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(item), comptime);
    }
    Ok(ret)
}

fn range_item(
    context: &mut Context,
    tgt: &ir::Expression,
    range_item: &RangeItem,
) -> IrResult<ir::Expression> {
    let mut exp: ir::Expression = Conv::conv(context, range_item.range.expression.as_ref())?;

    let comptime = exp.eval_comptime(context, None);
    let lo_value = comptime.get_value().ok().and_then(|v| v.to_usize());
    if !comptime.is_const {
        context.insert_error(AnalyzerError::unevaluable_value(
            UnevaluableValueKind::CaseCondition,
            &range_item.into(),
        ));
    }

    let ret = if let Some(x) = &range_item.range.range_opt {
        let mut exp0: ir::Expression = Conv::conv(context, x.expression.as_ref())?;

        let token: TokenRange = range_item.into();
        let comptime = exp0.eval_comptime(context, None);
        if !comptime.is_const {
            context.insert_error(AnalyzerError::unevaluable_value(
                UnevaluableValueKind::CaseCondition,
                &token,
            ));
        }

        // An empty exclusive range (constant `lo >= hi`) miscompiles: the emitter's
        // `(hi)-1` underflows an unsigned `hi == 0` to a near-universal range.
        if matches!(x.range_operator.as_ref(), RangeOperator::DotDot(_))
            && let (Some(lo), Some(hi)) = (
                lo_value,
                comptime.get_value().ok().and_then(|v| v.to_usize()),
            )
            && lo >= hi
        {
            context.insert_error(AnalyzerError::invalid_range(
                "the lower bound of an exclusive range is not less than the upper bound",
                &token,
            ));
        }

        let comptime = Box::new(Comptime::create_unknown(token));

        match x.range_operator.as_ref() {
            RangeOperator::DotDot(_) => {
                let cond0 = ir::Expression::Binary(
                    Box::new(exp.clone()),
                    Op::LessEq,
                    Box::new(tgt.clone()),
                    comptime.clone(),
                );
                let cond1 = ir::Expression::Binary(
                    Box::new(tgt.clone()),
                    Op::Less,
                    Box::new(exp0.clone()),
                    comptime.clone(),
                );
                ir::Expression::Binary(Box::new(cond0), Op::LogicAnd, Box::new(cond1), comptime)
            }
            RangeOperator::DotDotEqu(_) => {
                let cond0 = ir::Expression::Binary(
                    Box::new(exp.clone()),
                    Op::LessEq,
                    Box::new(tgt.clone()),
                    comptime.clone(),
                );
                let cond1 = ir::Expression::Binary(
                    Box::new(tgt.clone()),
                    Op::LessEq,
                    Box::new(exp0.clone()),
                    comptime.clone(),
                );
                ir::Expression::Binary(Box::new(cond0), Op::LogicAnd, Box::new(cond1), comptime)
            }
        }
    } else {
        let token: TokenRange = range_item.into();
        let comptime = Box::new(Comptime::create_unknown(token));
        ir::Expression::Binary(
            Box::new(tgt.clone()),
            Op::EqWildcard,
            Box::new(exp),
            comptime,
        )
    };
    Ok(ret)
}

pub fn switch_condition(context: &mut Context, cond: &SwitchCondition) -> IrResult<ir::Expression> {
    let mut ret: ir::Expression = Conv::conv(context, cond.expression.as_ref())?;
    for x in &cond.switch_condition_list {
        let exp: ir::Expression = Conv::conv(context, x.expression.as_ref())?;
        let comptime = Box::new(Comptime::create_unknown(exp.token_range()));
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(exp), comptime);
    }
    // Resolve comptime widths like `if` conditions / `case` targets do;
    // otherwise a compound condition's sub-expression widths stay unevaluated.
    ret.eval_comptime(context, None);
    Ok(ret)
}

pub fn argument_list(context: &mut Context, value: &ArgumentList) -> IrResult<Arguments> {
    let mut positional = vec![];
    let mut named = vec![];
    let mut seen_named = HashSet::default();
    let x: Vec<_> = value.into();
    for arg in x {
        if let Some(x) = &arg.argument_item_opt {
            if let Some(name) = arg.argument_expression.expression.unwrap_identifier() {
                let name_token = name.identifier().token;
                let name = name_token.text;
                let token: TokenRange = x.expression.as_ref().into();
                if !seen_named.insert(name) {
                    context.insert_error(AnalyzerError::duplicate_argument(
                        &name_token.to_string(),
                        &token,
                    ));
                }
                let expr = Conv::conv(context, x.expression.as_ref())?;
                let dst: Vec<VarPathSelect> = Conv::conv(context, x.expression.as_ref())?;
                named.push((name, (expr, dst, token)));
            } else {
                // TODO error
            }
        } else {
            let token: TokenRange = arg.argument_expression.expression.as_ref().into();
            let expr = Conv::conv(context, arg.argument_expression.expression.as_ref())?;
            let dst: Vec<VarPathSelect> =
                Conv::conv(context, arg.argument_expression.expression.as_ref())?;
            positional.push((expr, dst, token));
        }
    }

    if !positional.is_empty() && !named.is_empty() {
        context.insert_error(AnalyzerError::mixed_function_argument(&value.into()));
    }

    let ret = if !positional.is_empty() && !named.is_empty() {
        Arguments::Mixed(positional, named)
    } else if !named.is_empty() {
        Arguments::Named(named)
    } else if !positional.is_empty() {
        Arguments::Positional(positional)
    } else {
        Arguments::Null
    };
    Ok(ret)
}

pub fn get_component(
    context: &mut Context,
    sig: &Signature,
    token: TokenRange,
) -> IrResult<Arc<ir::Component>> {
    // Normalize before any cache operations so keys always match what push() stores.
    let mut sig = sig.clone();
    sig.normalize();
    let sig = &sig;

    if let Some((component, in_generic)) = context.get_instance_history(sig) {
        if in_generic && !context.in_generic {
            // The IR result gotten from the cache may be incomplete
            // if the `in_generic` flag is set.
            // Such result should be removed from the cache and be created again.
            context.remove_instance_history(sig);
            get_component(context, sig, token)
        } else {
            Ok(component)
        }
    } else {
        let symbol = symbol_table::get(sig.symbol).unwrap();

        if let SymbolKind::SystemVerilog = symbol.kind {
            let component = ir::SystemVerilog {
                name: symbol.token.text,
                connects: vec![],
            };
            return Ok(Arc::new(ir::Component::SystemVerilog(component)));
        }

        let err = context.push_instance_history(sig.clone());

        if let Err(x) = err {
            match x {
                InstanceHistoryError::ExceedDepthLimit(x) => {
                    context.insert_error(AnalyzerError::exceed_limit(
                        ExceedLimitKind::HierarchyDepth,
                        x,
                        &token,
                    ));
                }
                InstanceHistoryError::ExceedTotalLimit(x) => {
                    context.insert_error(AnalyzerError::exceed_limit(
                        ExceedLimitKind::TotalInstance,
                        x,
                        &token,
                    ));
                }
                InstanceHistoryError::InfiniteRecursion => {
                    context.insert_error(AnalyzerError::infinite_recursion(&token));
                }
            }
            return Err(ir_error!(token));
        }

        context.push_generic_map(sig.to_generic_map());

        let ret = context.block(|c| match &symbol.kind {
            SymbolKind::Module(x) if !x.is_proto => {
                let definition =
                    definition_table::get(x.definition).ok_or_else(|| ir_error!(token))?;
                let Definition::Module(x) = definition.as_ref() else {
                    unreachable!()
                };

                let header_only = c.in_generic;
                let component: IrResult<ir::Module> = Conv::conv(c, (x, header_only));
                match component {
                    Ok(mut component) => {
                        if !c.config.retain_component_body {
                            component.functions.clear();
                            component.declarations.clear();
                        }

                        let component = Arc::new(ir::Component::Module(component));
                        c.set_instance_history(sig, component.clone());
                        c.pop_instance_history();
                        Ok(component)
                    }
                    Err(x) => {
                        c.pop_instance_history();
                        Err(x)
                    }
                }
            }
            SymbolKind::Interface(x) if !x.is_proto => {
                let definition = x
                    .definition
                    .and_then(definition_table::get)
                    .ok_or_else(|| ir_error!(token))?;
                let Definition::Interface(x) = definition.as_ref() else {
                    unreachable!()
                };

                let component: IrResult<ir::Interface> = Conv::conv(c, x);
                match component {
                    Ok(component) => {
                        let component = Arc::new(ir::Component::Interface(component));
                        c.set_instance_history(sig, component.clone());
                        c.pop_instance_history();
                        Ok(component)
                    }
                    Err(x) => {
                        c.pop_instance_history();
                        Err(x)
                    }
                }
            }
            SymbolKind::Module(x) if x.is_proto => {
                let definition =
                    definition_table::get(x.definition).ok_or_else(|| ir_error!(token))?;
                let Definition::ProtoModule(x) = definition.as_ref() else {
                    unreachable!()
                };

                let component: IrResult<ir::Module> = Conv::conv(c, x);
                match component {
                    Ok(mut component) => {
                        if !c.config.retain_component_body {
                            component.functions.clear();
                            component.declarations.clear();
                        }

                        let component = Arc::new(ir::Component::Module(component));
                        c.set_instance_history(sig, component.clone());
                        c.pop_instance_history();
                        Ok(component)
                    }
                    Err(x) => {
                        c.pop_instance_history();
                        Err(x)
                    }
                }
            }
            _ => Err(ir_error!(token)),
        });

        context.pop_generic_map();
        ret
    }
}

/// Resolve the per-element values of an array-typed parameter override.
///
/// `eval_expr` leaves an array value `Unknown` (only scalars fold to
/// `Numeric`), so this must run while the source variables are still in scope,
/// letting the caller carry the values in the override comptime for the
/// instance signature and `eval_const_assign`.
fn resolve_array_value(
    context: &mut Context,
    r#type: &ir::Type,
    expr: &ir::Expression,
) -> Option<Vec<Value>> {
    if r#type.array.is_empty() {
        return None;
    }

    match expr {
        ir::Expression::ArrayLiteral(_, _) => {
            // Clone so folding's constant-fold side effects don't mutate the
            // stored override expression.
            let mut expr = expr.clone();
            eval_array_literal_values(context, r#type, &mut expr)
                .ok()
                .flatten()
        }
        ir::Expression::Term(factor) => {
            let ir::Factor::Variable(id, index, select, _) = factor.as_ref() else {
                return None;
            };
            // A bit/part select can't be copied element-wise.
            if !select.is_empty() {
                return None;
            }
            if index.dimension() == 0 {
                // Whole-array reference; a single-entry template is preserved.
                return context.variables.get(id).map(|v| v.value.clone());
            }
            // Constant-indexed sub-array reference, e.g. `XS[0]` selecting a row
            // of a 2-D source into a 1-D parameter.
            if !index.is_const() {
                return None;
            }
            let idx = index.eval_value(context)?;
            let var = context.variables.get(id)?;
            let shape = &var.r#type.array;
            // calc_range/calc_index don't bounds-check, so an out-of-range index
            // would silently pick the wrong sub-array; reject it here.
            if idx
                .iter()
                .enumerate()
                .any(|(i, &v)| matches!(shape.get(i), Some(&Some(dim)) if v >= dim))
            {
                return None;
            }
            let (beg, end) = shape.calc_range(&idx)?;
            let inner_total = end - beg + 1;
            let total = var.r#type.total_array().unwrap_or(var.value.len());
            if var.value.len() == total {
                var.value.get(beg..=end).map(<[Value]>::to_vec)
            } else if var.value.len() == 1 {
                // All-same array stored as a single template value.
                Some(vec![var.value[0].clone(); inner_total])
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn get_overridden_params(
    context: &mut Context,
    arg: &ComponentInstantiation,
) -> IrResult<HashMap<VarPath, (Comptime, ir::Expression)>> {
    let mut ret = HashMap::default();

    let token: TokenRange = arg.scoped_identifier.as_ref().into();
    let symbol =
        symbol_table::resolve(arg.scoped_identifier.as_ref()).map_err(|_| ir_error!(token))?;
    let component_namespace = symbol.found.inner_namespace();

    let params = if let Some(ref x) = arg.component_instantiation_opt1 {
        if let Some(x) = &x.inst_parameter.inst_parameter_opt {
            x.inst_parameter_list.as_ref().into()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    for param in params {
        let name = param.identifier.text();

        let Ok(target) = symbol_table::resolve((param.identifier.as_ref(), &component_namespace))
            .map(|x| Rc::clone(&x.found))
        else {
            continue;
        };

        let target_type = if let Some(x) = target.kind.get_type() {
            let x = x.to_ir_type(context, TypePosition::Variable);
            if let Ok(x) = x {
                Some(x)
            } else {
                continue;
            }
        } else {
            None
        };

        let mut expr = if let Some(x) = &param.inst_parameter_item_opt {
            eval_expr(context, target_type.clone(), &x.expression, false)?
        } else {
            let src: Expression = param.identifier.as_ref().into();
            eval_expr(context, target_type.clone(), &src, false)?
        };

        // Carry an array override's element values as a NumericArray so the
        // signature distinguishes them and eval_const_assign can materialize it.
        if expr.0.value.is_unknown()
            && let Some(r#type) = &target_type
            && let Some(values) = resolve_array_value(context, r#type, &expr.1)
        {
            expr.0.value = ValueVariant::NumericArray(values);
        }

        let is_type_param = matches!(
            &target.kind,
            SymbolKind::Parameter(x) if !x.is_proto && matches!(x.r#type.kind, TypeKind::Type)
        );
        if !is_type_param && !expr.0.is_const {
            let token: TokenRange = param.identifier.as_ref().into();
            context.insert_error(AnalyzerError::unevaluable_value(
                UnevaluableValueKind::ParameterValue,
                &token,
            ));
        }

        let path = VarPath::new(name);
        ret.insert(path, expr);
    }

    Ok(ret)
}

pub fn get_return_str() -> StrId {
    resource_table::insert_str("return")
}

pub fn get_port_connects(
    context: &mut Context,
    component: &ir::Module,
    port: &InstPortItem,
    port_path: &VarPath,
    port_type: &ir::Type,
    token: TokenRange,
) -> IrResult<Vec<(VarPath, Vec<VarPathSelect>, ir::Expression)>> {
    let mut ret = vec![];

    if let ir::TypeKind::Modport(_, _) = &port_type.kind {
        if let Some(x) = &port.inst_port_item_opt {
            // for check_compatibility
            let _ = eval_expr(context, Some(port_type.clone()), &x.expression, false)?;
        }

        let (dst_path, dst_select) = if let Some(x) = &port.inst_port_item_opt {
            let dst: Vec<VarPathSelect> = Conv::conv(context, x.expression.as_ref())?;
            let dst = dst.first().ok_or_else(|| ir_error!(token))?;
            (dst.0.clone(), dst.1.clone())
        } else {
            (port_path.clone(), VarSelect::default())
        };

        let members = port_type.expand_modport(context, &dst_path, token)?;
        for member in members {
            if member.1.is_input() | member.1.is_output() {
                let member_path = member.0.clone();
                let expr = if let Some((var_id, mut comptime)) = context.find_path(&member_path) {
                    comptime.token = token;
                    // Apply dst_select (e.g. `[i]` in `port: s_port[i]`):
                    // array dims go to the variable's index, the rest to a bit select.
                    let (array_select, width_select) =
                        dst_select.clone().split(comptime.r#type.array.dims());
                    comptime.r#type.array.drain(0..array_select.dimension());
                    ir::Expression::Term(Box::new(ir::Factor::Variable(
                        var_id,
                        array_select.to_index(),
                        width_select,
                        comptime,
                    )))
                } else {
                    let comptime = Comptime::create_unknown(token);
                    ir::Expression::Term(Box::new(ir::Factor::Unknown(comptime)))
                };
                let dst = vec![VarPathSelect(
                    member_path.clone(),
                    dst_select.clone(),
                    token,
                )];

                // append member name to port_path
                let mut port_path = port_path.clone();
                let mut member_name = member_path.0.clone();
                member_name.remove(0);
                port_path.append(&member_name);

                ret.push((port_path.clone(), dst, expr));
            }
        }
    } else if let Some(x) = &port.inst_port_item_opt {
        let mut allow_anonymous = true;

        if let Some(id) = component.ports.get(port_path)
            && let Some(variable) = component.variables.get(id)
            && variable.kind == VarKind::Input
        {
            allow_anonymous = false;
        }

        let (comptime, expr) = eval_expr(
            context,
            Some(port_type.clone()),
            &x.expression,
            allow_anonymous,
        )?;

        check_compatibility(context, port_type, &comptime, &token);

        let dst: Vec<VarPathSelect> = Conv::conv(context, x.expression.as_ref())?;

        ret.push((port_path.clone(), dst, expr));
    } else if let Some((var_id, mut comptime)) = context.find_path(port_path) {
        check_compatibility(context, port_type, &comptime, &token);

        comptime.token = token;
        let expr = ir::Expression::Term(Box::new(ir::Factor::Variable(
            var_id,
            VarIndex::default(),
            VarSelect::default(),
            comptime,
        )));
        let dst = vec![VarPathSelect(
            port_path.clone(),
            VarSelect::default(),
            token,
        )];
        ret.push((port_path.clone(), dst, expr));
    }

    Ok(ret)
}

pub fn insert_port_connect(
    context: &mut Context,
    variable: &ir::Variable,
    dst: Vec<VarPathSelect>,
    expr: ir::Expression,
    inputs: &mut Vec<ir::InstInput>,
    outputs: &mut Vec<ir::InstOutput>,
) {
    match variable.kind {
        VarKind::Input => {
            inputs.push(ir::InstInput {
                id: variable.id,
                expr,
            });
        }
        VarKind::Output => {
            if !expr.is_assignable() {
                context.insert_error(AnalyzerError::unassignable_output(&expr.token_range()));
            }
            let dst = var_path_to_assign_destination(context, dst, false);
            outputs.push(ir::InstOutput {
                id: variable.id,
                dst,
            });
        }
        _ => (),
    }
}

pub fn expand_connect(
    context: &mut Context,
    lhs: VarPathSelect,
    rhs: VarPathSelect,
    token: TokenRange,
) -> IrResult<Vec<ir::Statement>> {
    let mut ret = vec![];

    let (lhs_path, lhs_select, lhs_token) = lhs.into();
    let (rhs_path, rhs_select, rhs_token) = rhs.into();

    if let Some((_, lhs_comptime)) = context.find_path(&lhs_path)
        && let Some((_, rhs_comptime)) = context.find_path(&rhs_path)
    {
        // remove modport name from path
        let mut lhs_base = lhs_path.clone();
        let mut rhs_base = rhs_path.clone();
        if lhs_base.0.len() != 1 {
            lhs_base.pop();
        }
        if rhs_base.0.len() != 1 {
            rhs_base.pop();
        }

        let lhs_members = lhs_comptime
            .r#type
            .expand_modport(context, &lhs_base, lhs_token)?;
        let rhs_members = rhs_comptime
            .r#type
            .expand_modport(context, &rhs_base, rhs_token)?;

        let rhs_members: HashMap<_, _> = rhs_members
            .into_iter()
            .map(|(path, dir)| {
                let mut key = path.clone();
                key.remove_prelude(&rhs_base.0);
                (key, (path, dir))
            })
            .collect();

        for lhs in &lhs_members {
            let mut key = lhs.0.clone();
            key.remove_prelude(&lhs_base.0);
            if let Some(rhs) = rhs_members.get(&key) {
                let lhs_path = lhs.0.clone();
                let rhs_path = rhs.0.clone();
                let lhs_direction = lhs.1;
                let rhs_direction = rhs.1;
                let lhs = VarPathSelect(lhs_path, lhs_select.clone(), lhs_token);
                let rhs = VarPathSelect(rhs_path, rhs_select.clone(), rhs_token);

                let (dst, src) = if lhs_direction.is_output() && rhs_direction.is_input() {
                    (lhs, rhs)
                } else if rhs_direction.is_output() && lhs_direction.is_input() {
                    (rhs, lhs)
                } else {
                    // TODO direction error
                    return Ok(ret);
                };

                if let Some((_, mut dst_comptime)) = context.find_path(&dst.0)
                    && let Some((_, mut src_comptime)) = context.find_path(&src.0)
                {
                    dst_comptime.token = dst.2;
                    src_comptime.token = src.2;
                    check_clock_domain(context, &dst_comptime, &src_comptime, &token.beg);
                }

                if let Some(src) = src.to_expression(context)
                    && let Some(dst) = dst.to_assign_destination(context, false)
                {
                    let width = dst.total_width(context);
                    let statement = ir::Statement::Assign(ir::AssignStatement {
                        dst: vec![dst],
                        width,
                        expr: src,
                        token,
                    });
                    ret.push(statement);
                } else {
                    // TODO unknown member error
                }
            }
        }
    }

    Ok(ret)
}

pub fn expand_connect_const(
    context: &mut Context,
    lhs: VarPathSelect,
    comptime: Comptime,
    token: TokenRange,
) -> IrResult<Vec<ir::Statement>> {
    let mut ret = vec![];

    let (lhs_path, lhs_select, lhs_token) = lhs.into();

    if let Some((_, lhs_comptime)) = context.find_path(&lhs_path) {
        // remove modport name from path
        let mut lhs_base = lhs_path.clone();
        if lhs_base.0.len() != 1 {
            lhs_base.pop();
        }

        let lhs_members = lhs_comptime
            .r#type
            .expand_modport(context, &lhs_base, lhs_token)?;

        for lhs in lhs_members {
            if lhs.1.is_output() {
                let dst = VarPathSelect(lhs.0, lhs_select.clone(), lhs_token);
                let mut comptime = comptime.clone();
                comptime.token = token;
                let src = ir::Factor::Value(comptime);
                let src = ir::Expression::Term(Box::new(src));

                if let Some(dst) = dst.to_assign_destination(context, false) {
                    let width = dst.total_width(context);
                    let statement = ir::Statement::Assign(ir::AssignStatement {
                        dst: vec![dst],
                        width,
                        expr: src,
                        token,
                    });
                    ret.push(statement);
                } else {
                    // TODO unknown member error
                }
            }
        }
    }

    Ok(ret)
}

pub fn var_path_to_assign_destination(
    context: &mut Context,
    path: Vec<VarPathSelect>,
    ignore_error: bool,
) -> Vec<ir::AssignDestination> {
    path.into_iter()
        .flat_map(|x| x.to_assign_destinations(context, ignore_error))
        .collect()
}

fn get_function(context: &mut Context, path: &FuncPath, token: TokenRange) -> IrResult<FuncProto> {
    fn conv_function(
        context: &mut Context,
        definition: DefinitionId,
        path: &FuncPath,
    ) -> IrResult<()> {
        let definition = definition_table::get(definition).unwrap();
        match definition.as_ref() {
            Definition::Function(x) => Conv::conv(context, (x, Some(path))),
            Definition::ProtoFunction(x) => Conv::conv(context, (x, Some(path))),
            _ => unreachable!(),
        }
    }

    if !context.func_paths.contains_key(path) {
        let symbol = symbol_table::get(path.sig.symbol).unwrap();
        let (definition, is_global) = match &symbol.kind {
            SymbolKind::Function(x) => (x.definition.unwrap(), x.is_global()),
            SymbolKind::ModportFunctionMember(x) => {
                let symbol = symbol_table::get(x.function).unwrap();
                let SymbolKind::Function(x) = symbol.kind else {
                    unreachable!();
                };
                (x.definition.unwrap(), false)
            }
            _ => return Err(ir_error!(token)),
        };

        let array = if let Some((_, comptime)) = context.find_path(&path.path) {
            comptime.r#type.array
        } else {
            Shape::default()
        };

        let is_local_func = context
            .current_namespace()
            .map(|namespace| symbol.namespace.included(&namespace))
            .unwrap_or(false);
        if is_local_func {
            let ret = conv_function(context, definition, path);
            ret?;
        } else {
            let generic_arg_paths = if is_global {
                path.sig
                    .generic_parameters
                    .iter()
                    .filter_map(|(_, x)| x.to_var_path())
                    .collect()
            } else {
                vec![]
            };

            let mut local_context = Context::default();
            local_context.var_id = context.var_id;
            local_context.inherit(context);
            local_context.extract_var_paths(context, &path.path, &array);

            for path in &generic_arg_paths {
                // Copy var path referenced as resolved generic arg from the given context
                if let Some((var_id, comptime)) = context.find_path(path)
                    && let Some(var) = context.variables.get(&var_id)
                {
                    local_context
                        .var_paths
                        .insert(path.clone(), (var_id, comptime));
                    local_context.variables.insert(var_id, var.clone());
                }
            }

            let ret = conv_function(&mut local_context, definition, path);

            for path in &generic_arg_paths {
                if let Some((var_id, _)) = local_context.var_paths.remove(path) {
                    local_context.variables.remove(&var_id);
                }
            }

            context.extract_function(&mut local_context, &path.path, &array);
            context.inherit(&mut local_context);
            context.var_id = local_context.var_id;

            ret?;
        }
    }

    let Some(id) = context.func_paths.get(path) else {
        return Err(ir_error!(token));
    };
    context
        .functions
        .get(id)
        .map(|func| func.to_proto())
        .ok_or_else(|| ir_error!(token))
}

pub fn function_call(
    context: &mut Context,
    path: &ExpressionIdentifier,
    args: Arguments,
    token: TokenRange,
) -> IrResult<ir::FunctionCall> {
    let generic_path: GenericSymbolPath = path.into();

    check_generic_refereence(context, &generic_path);

    let mut parent_path = generic_path.clone();
    parent_path.paths.pop();
    let mut sig = Signature::from_path(context, generic_path).ok_or_else(|| ir_error!(token))?;
    sig.normalize();

    // same signature re-entered => true infinite recursion
    if context.function_call_stack.contains(&sig) {
        context.insert_error(AnalyzerError::infinite_recursion(&token));
        return Err(ir_error!(token));
    }

    let path: VarPathSelect = Conv::conv(context, path)?;
    let (mut base_path, select, _) = path.into();
    let index = select.to_index();
    let index = index.eval_value(context);

    // remove function name
    base_path.pop();

    let path = ir::FuncPath {
        path: base_path.clone(),
        sig: sig.clone(),
    };

    let mut map = sig.to_generic_map();

    if !parent_path.is_empty()
        && let Ok(symbol) = symbol_table::resolve(&parent_path)
    {
        match &symbol.found.kind {
            SymbolKind::Instance(x) => {
                let mut parent_map = x.type_name.to_generic_maps();
                parent_map.append(&mut map);
                map = parent_map;
            }
            SymbolKind::Port(x) => {
                if let Some(x) = x.r#type.get_user_defined() {
                    let mut parent_map = x.path.to_generic_maps();
                    parent_map.append(&mut map);
                    map = parent_map;
                }
            }
            _ => (),
        }
    }

    context.push_generic_map(map);
    context.function_call_stack.push(sig.clone());

    let ret = context.block(|c| {
        let func = get_function(c, &path, token)?;
        let (mut inputs, outputs) = args.to_function_args(c, &func, token)?;

        let mut comptime = func.r#type.clone();
        comptime.token = token;

        // Merge the inputs' clock domains into the result and cross-check, so a
        // call can't launder a domain crossing (mirrors Op::eval_type_binary);
        // also check output-arg destinations against the merged domain.
        for expr in inputs.values_mut() {
            let x = expr.eval_comptime(c, None);
            check_clock_domain(c, &comptime, x, &token.beg);
            comptime.clock_domain = comptime.clock_domain.merge(&x.clock_domain);
        }
        for dsts in outputs.values() {
            for dst in dsts {
                let mut dst_comptime = dst.comptime.clone();
                dst_comptime.token = dst.token;
                check_clock_domain(c, &dst_comptime, &comptime, &token.beg);
            }
        }

        Ok(ir::FunctionCall {
            id: func.id,
            index,
            comptime,
            inputs,
            outputs,
        })
    });

    context.pop_generic_map();
    context.function_call_stack.pop();
    ret
}

pub fn check_compatibility(
    context: &mut Context,
    dst: &ir::Type,
    src: &ir::Comptime,
    token: &TokenRange,
) {
    if dst.is_positive
        && let Ok(value) = src.get_value()
        && !(src.r#type.is_positive && value.is_xz())
        && (!value.is_positive() || value.is_semantically_not_positive())
    {
        context.insert_error(AnalyzerError::non_positive_value(
            "non-positive",
            &format_positive_type_name(dst).unwrap(),
            token,
        ));
    }
    // This branch is reached via `eval_expr` BEFORE its dst_type
    // overwrite, so `src` is the connected value's own type; the later
    // call on the same connection sees the overwritten (matching) type
    // and stays silent.
    if (dst.is_clock() || dst.is_reset()) && context.in_inst_port {
        check_implicit_clock_conversion(context, dst, src, token);
        return;
    }
    if !dst.compatible(src, context.in_generic) {
        let src_type = src.r#type.to_string();
        let dst_type = dst.to_string();
        context.insert_error(AnalyzerError::mismatch_assignment(
            &src_type,
            &dst_type,
            token,
            &[],
        ));
    }
}

/// Reject implicit conversion to a clock/reset-typed input port or
/// function argument.  `$sv` ports are SystemVerilog-typed and never
/// reach this check, which keeps the SystemVerilog boundary unchecked
/// by design.
pub fn check_implicit_clock_conversion(
    context: &mut Context,
    dst: &ir::Type,
    src: &Comptime,
    token: &TokenRange,
) {
    let dst_is_clock = dst.is_clock();
    let dst_is_reset = dst.is_reset();
    if !dst_is_clock && !dst_is_reset {
        return;
    }
    if src.r#type.is_unknown() || src.r#type.is_systemverilog() || src.is_const {
        return;
    }
    let ok = if dst_is_clock {
        src.r#type.is_clock()
    } else {
        src.r#type.is_reset()
    };
    if !ok {
        context.insert_error(AnalyzerError::implicit_clock_conversion(
            &src.r#type.to_string(),
            &dst.to_string(),
            token,
            &[],
        ));
    }
}

pub fn check_module_with_unevaluable_generic_parameters(ident: &Identifier) -> bool {
    if let Ok(symbol) = symbol_table::resolve(ident)
        && let SymbolKind::Module(x) = &symbol.found.kind
    {
        let mut ret = false;

        for x in &x.generic_parameters {
            let param = symbol_table::get(*x).unwrap();
            if let SymbolKind::GenericParameter(x) = param.kind {
                let has_default = x.default_value.is_some();
                ret |= match &x.bound {
                    GenericBoundKind::Type => false,
                    GenericBoundKind::Inst(_) => false,
                    GenericBoundKind::Proto(x) => {
                        // Fixed type or proto package with non-default may be unevaluable
                        if x.kind.is_fixed() && !has_default {
                            true
                        } else if let TypeKind::UserDefined(x) = &x.kind
                            && let Ok(symbol) = symbol_table::resolve(&x.path)
                            && (matches!(symbol.found.kind, SymbolKind::Package(ref x) if x.is_proto)
                                || matches!(&symbol.found.kind, SymbolKind::AliasPackage(x) if x.is_proto))
                            && !has_default
                        {
                            true
                        } else {
                            false
                        }
                    }
                };
            } else {
                unreachable!();
            }
        }

        ret
    } else {
        false
    }
}

#[cfg(test)]
pub fn parse_expression(s: &str) -> Expression {
    use veryl_parser::parser::Parser;
    use veryl_parser::veryl_walker::VerylWalker;

    let src = format!(
        r#"
        module A {{
            let a: bit = {s};
        }}
        "#
    );
    let parser = Parser::parse(&src, &"").unwrap();

    struct Extractor(Option<Expression>);
    impl VerylWalker for Extractor {
        fn expression(&mut self, arg: &Expression) {
            self.0 = Some(arg.clone());
        }
    }

    let mut extractor = Extractor(None);
    extractor.veryl(&parser.veryl);
    extractor.0.unwrap()
}

/// Where a testbench method call appears, for return-value diagnostics.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TbMethodCallPosition {
    /// Bare statement: a declared return value would be silently dropped.
    Statement,
    /// Assignment or expression position: the value is consumed.
    Value,
}

/// Converts a method call on a user-defined `$comp` component into a
/// [`TbMethod::Component`], diagnosing the call against the interface
/// manifest when one is present. Returns the method and the resolved return
/// width (`None` when undeclared).
fn conv_external_method_call(
    context: &mut Context,
    component: resource_table::StrId,
    path: &GenericSymbolPath,
    args: Option<ir::Arguments>,
    inst_name: resource_table::StrId,
    position: TbMethodCallPosition,
    token: TokenRange,
) -> IrResult<(TbMethod, Option<u32>)> {
    // Only the exact `instance.method(...)` shape reaches the component;
    // extra path segments or generic arguments would otherwise be silently
    // dropped.
    if path.paths.len() != 2 {
        context.insert_error(AnalyzerError::component_interface_mismatch(
            ComponentInterfaceMismatchKind::MethodCallForm,
            Some(component),
            &token,
        ));
        return Err(ir_error!(token));
    }
    if path.paths.iter().any(|x| !x.arguments.is_empty()) {
        context.insert_error(AnalyzerError::component_interface_mismatch(
            ComponentInterfaceMismatchKind::MethodNoGenericArgs,
            Some(component),
            &token,
        ));
        return Err(ir_error!(token));
    }
    let args: Vec<SystemFunctionInput> = match args {
        Some(ir::Arguments::Positional(ref positional)) => positional
            .iter()
            .map(|arg| SystemFunctionInput(arg.0.clone()))
            .collect(),
        None | Some(ir::Arguments::Null) => Vec::new(),
        // The ABI carries values only, no argument names.
        Some(ir::Arguments::Named(_)) | Some(ir::Arguments::Mixed(..)) => {
            context.insert_error(AnalyzerError::invalid_factor(
                None,
                "component method with named arguments",
                &token,
                &[],
            ));
            return Err(ir_error!(token));
        }
    };
    // The manifest keys methods by their unrawed name.
    let method = resource_table::canonical_str_id(path.paths[path.paths.len() - 1].base.text);
    let mut ret_width = None;
    if let Some(manifest) = crate::component_manifest_table::get(component)
        && !manifest.methods.is_empty()
    {
        let name = method.to_string();
        match manifest.method(&name) {
            None => {
                context.insert_error(AnalyzerError::component_interface_mismatch(
                    ComponentInterfaceMismatchKind::UnknownMethod {
                        method: name.clone(),
                    },
                    Some(component),
                    &token,
                ));
            }
            Some(m) if m.args.len() != args.len() => {
                context.insert_error(AnalyzerError::mismatch_function_arity(
                    &name,
                    m.args.len(),
                    args.len(),
                    &token,
                ));
            }
            Some(m) => {
                match position {
                    TbMethodCallPosition::Statement if m.ret.is_some() => {
                        context.insert_error(AnalyzerError::unused_return(&name, &token));
                    }
                    TbMethodCallPosition::Value if m.ret.is_none() => {
                        context.insert_error(AnalyzerError::component_interface_mismatch(
                            ComponentInterfaceMismatchKind::MethodReturnsNoValue {
                                method: name.clone(),
                            },
                            Some(component),
                            &token,
                        ));
                    }
                    _ => {}
                }

                let inst_params = context
                    .tb_component_params
                    .get(&inst_name)
                    .cloned()
                    .unwrap_or_default();
                let resolve_width =
                    |context: &mut Context, expr: &veryl_metadata::WidthExpr, what: &str| {
                        match veryl_metadata::eval_width_expr(expr, &inst_params) {
                            Some(w) if w > 0 => Some(w as u32),
                            _ => {
                                context.insert_error(AnalyzerError::component_interface_mismatch(
                                    ComponentInterfaceMismatchKind::UnresolvableWidth {
                                        expr: expr.to_string(),
                                        what: what.to_string(),
                                    },
                                    Some(component),
                                    &token,
                                ));
                                None
                            }
                        }
                    };
                if let Some(expr) = &m.ret_width.clone() {
                    ret_width = resolve_width(context, expr, &format!("method `{name}`'s return"));
                    // Method return values cross the host ABI in a fixed
                    // 8 x 64-bit word buffer; wider declared returns can never
                    // be delivered.
                    const METHOD_RET_MAX_WIDTH: u32 = 512;
                    if let Some(w) = ret_width
                        && w > METHOD_RET_MAX_WIDTH
                    {
                        context.insert_error(AnalyzerError::component_interface_mismatch(
                            ComponentInterfaceMismatchKind::MethodReturnTooWide {
                                method: name.clone(),
                                declared: w,
                                max: METHOD_RET_MAX_WIDTH,
                            },
                            Some(component),
                            &token,
                        ));
                    }
                }
                // A `Value` argument's width is inferred from the call-site
                // expression, so there is nothing to check here.
            }
        }
    }
    Ok((TbMethod::Component { method, args }, ret_width))
}

pub fn tb_method_call(
    context: &mut Context,
    expr_id: &ExpressionIdentifier,
    func_call: &FunctionCall,
    token: TokenRange,
    position: TbMethodCallPosition,
) -> IrResult<Option<ir::Statement>> {
    let path: GenericSymbolPath = expr_id.into();

    // Need at least 2 path elements: instance.method
    if path.paths.len() < 2 {
        return Ok(None);
    }

    // Resolve just the first element (the instance name)
    let inst_name = path.paths[0].base.text;
    let inst_path = SymbolPath::new(&[inst_name]);
    let Some((inst_scope, inst_define_context)) = crate::scope::token_scope(path.paths[0].base.id)
    else {
        return Ok(None);
    };

    let inst_symbol = match symbol_table::resolve(SymbolPathNamespace::from_scope(
        inst_path,
        inst_scope,
        inst_define_context,
    )) {
        Ok(s) => s,
        Err(_) => {
            return Ok(None);
        }
    };

    // `$tb` methods are called on either an `inst` (clock_gen/reset_gen) or a
    // `var` (file). Pull the component type path from whichever form it is.
    let type_path = match &inst_symbol.found.kind {
        SymbolKind::Instance(x) => &x.type_name,
        SymbolKind::Variable(x) => {
            if let TypeKind::UserDefined(ref ud) = x.r#type.kind {
                &ud.path
            } else {
                return Ok(None);
            }
        }
        _ => {
            return Ok(None);
        }
    };

    let type_symbol =
        match symbol_table::resolve_generic_structural(type_path, &inst_symbol.found.namespace) {
            Ok(s) => s,
            Err(_) => {
                return Ok(None);
            }
        };

    let tb_kind = match &type_symbol.found.kind {
        SymbolKind::TbComponent(x) => &x.kind,
        _ => {
            return Ok(None);
        }
    };

    let method_name = resource_table::get_str_value(path.paths[path.paths.len() - 1].base.text);
    let method_name = method_name.as_deref().unwrap_or("");

    // Parse arguments for the method call
    let args = if let Some(ref arg_list) = func_call.function_call_opt {
        Some(argument_list(context, arg_list.argument_list.as_ref())?)
    } else {
        None
    };

    let mut ret_width = None;
    let method = match (tb_kind, method_name) {
        (TbComponentKind::ClockGen, "next") => {
            let count = if let Some(ir::Arguments::Positional(ref positional)) = args
                && !positional.is_empty()
            {
                Some(positional[0].0.clone())
            } else {
                None
            };
            let period = context
                .tb_clock_period
                .get(&inst_name)
                .cloned()
                .map(Box::new);
            TbMethod::ClockNext { count, period }
        }
        (TbComponentKind::ResetGen, "assert") => {
            let clock = context
                .tb_reset_clock
                .get(&inst_name)
                .copied()
                .ok_or_else(|| ir_error!(token))?;
            let duration = if let Some(ir::Arguments::Positional(ref positional)) = args
                && let Some(arg) = positional.first()
            {
                Some(arg.0.clone())
            } else {
                context.tb_reset_cycles.get(&inst_name).cloned()
            };
            TbMethod::ResetAssert { clock, duration }
        }
        (TbComponentKind::File, "open") | (TbComponentKind::File, "append") => {
            let name = if let Some(ir::Arguments::Positional(ref positional)) = args
                && let Some(arg) = positional.first()
            {
                SystemFunctionInput(arg.0.clone())
            } else {
                let actual = if let Some(ir::Arguments::Positional(ref positional)) = args {
                    positional.len()
                } else {
                    0
                };
                context.insert_error(AnalyzerError::mismatch_function_arity(
                    method_name,
                    1,
                    actual,
                    &token,
                ));
                return Err(ir_error!(token));
            };
            TbMethod::FileOpen {
                name,
                append: method_name == "append",
            }
        }
        (TbComponentKind::File, "write") => {
            let args = if let Some(ir::Arguments::Positional(ref positional)) = args {
                positional
                    .iter()
                    .map(|arg| SystemFunctionInput(arg.0.clone()))
                    .collect()
            } else {
                Vec::new()
            };
            TbMethod::FileWrite { args }
        }
        (TbComponentKind::File, "close") => TbMethod::FileClose,
        (TbComponentKind::File, "flush") => TbMethod::FileFlush,
        // Any method name is accepted on a user-defined component; the
        // component validates it at run time. With a manifest present the
        // name and arity are also diagnosed here.
        (TbComponentKind::External(component), _) => {
            let (method, rw) = conv_external_method_call(
                context, *component, &path, args, inst_name, position, token,
            )?;
            ret_width = rw;
            method
        }
        _ => return Err(ir_error!(token)),
    };

    Ok(Some(ir::Statement::TbMethodCall(TbMethodCall {
        inst: inst_name,
        method,
        ret: None,
        ret_strict: false,
        ret_width,
    })))
}

/// Hoists a component method call out of an expression: the call runs as
/// its own zero-time statement immediately before the enclosing testbench
/// statement, its return value lands in a synthetic temporary, and the
/// expression reads the temporary. The temporary carries the declared
/// return width when the manifest provides one; otherwise it is 64 bits
/// and a wider return fails at run time (the direct assignment form
/// `x = inst.method(...)` then carries the declared width of `x`).
pub fn hoist_component_method_call(
    context: &mut Context,
    mut method_call: TbMethodCall,
    token: TokenRange,
) -> IrResult<ir::Expression> {
    if context.tb_hoist.is_none() {
        context.insert_error(AnalyzerError::invalid_factor(
            None,
            "component method call outside testbench statements",
            &token,
            &[],
        ));
        return Err(ir_error!(token));
    }

    let width = method_call.ret_width.unwrap_or(64) as usize;
    let name = format!("__tb_method_ret_{}", context.tb_hoist_count);
    context.tb_hoist_count += 1;
    let path = VarPath::new(resource_table::insert_str(&name));
    let mut r#type = ir::Type::new(ir::TypeKind::Bit);
    r#type.set_concrete_width(Shape::new(vec![Some(width)]));

    let comptime = Comptime::from_type(r#type.clone(), ClockDomain::None, token);
    let id = context.insert_var_path(path.clone(), comptime.clone());
    let array_limit = context.config.evaluate_array_limit;
    let variable = Variable::new(
        id,
        path.clone(),
        VarKind::Variable,
        r#type,
        vec![Value::new_x(width, false)],
        context.get_affiliation(),
        &token,
        array_limit,
    );
    context.insert_variable(id, variable);

    method_call.ret = Some(Box::new(ir::AssignDestination {
        id,
        path,
        index: VarIndex::default(),
        select: VarSelect::default(),
        comptime: comptime.clone(),
        token,
    }));
    method_call.ret_strict = true;
    context
        .tb_hoist
        .as_mut()
        .unwrap()
        .push(ir::Statement::TbMethodCall(method_call));

    Ok(ir::Expression::Term(Box::new(ir::Factor::Variable(
        id,
        VarIndex::default(),
        VarSelect::default(),
        comptime,
    ))))
}

/// Extracts the sole `receiver.method(...)` call of an expression, when the
/// expression is exactly one identifier factor with a function call. Used to
/// intercept component method calls in assignment position.
pub fn single_function_call_factor(
    expr: &Expression,
) -> Option<(&ExpressionIdentifier, &FunctionCall)> {
    let if_expr = &*expr.if_expression;
    if !if_expr.if_expression_list.is_empty() {
        return None;
    }
    let e01 = &*if_expr.expression01;
    if !e01.expression01_list.is_empty() {
        return None;
    }
    let e02 = &*e01.expression02;
    if !e02.expression02_list.is_empty() || e02.expression02_opt.is_some() {
        return None;
    }
    if let Factor::IdentifierFactor(x) = e02.factor.as_ref()
        && let Some(opt) = &x.identifier_factor.identifier_factor_opt
        && let IdentifierFactorOptGroup::FunctionCall(call) =
            opt.identifier_factor_opt_group.as_ref()
    {
        return Some((
            x.identifier_factor.expression_identifier.as_ref(),
            call.function_call.as_ref(),
        ));
    }
    None
}

#[cfg(test)]
pub fn parse_number(s: &str) -> Number {
    use veryl_parser::parser::Parser;
    use veryl_parser::veryl_walker::VerylWalker;

    let src = format!(
        r#"
        module A {{
            let a: bit = {s};
        }}
        "#
    );
    let parser = Parser::parse(&src, &"").unwrap();

    struct Extractor(Option<Number>);
    impl VerylWalker for Extractor {
        fn number(&mut self, arg: &Number) {
            self.0 = Some(arg.clone());
        }
    }

    let mut extractor = Extractor(None);
    extractor.veryl(&parser.veryl);
    extractor.0.unwrap()
}
