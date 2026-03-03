use crate::analyzer_error::{AnalyzerError, ExceedLimitKind, UnevaluableValueKind};
use crate::conv::checker::anonymous::check_anonymous;
use crate::conv::checker::clock_domain::check_clock_domain;
use crate::conv::checker::generic::check_generic_args;
use crate::conv::instance::InstanceHistoryError;
use crate::conv::{Context, Conv};
use crate::definition_table::{self, Definition};
use crate::ir::{
    self, Arguments, Comptime, FuncPath, FuncProto, IrResult, Op, PartSelectPath, Shape, ShapeRef,
    Signature, ValueVariant, VarIndex, VarKind, VarPath, VarPathSelect, VarSelect, Variable,
};
use crate::symbol::{
    self, Affiliation, ClockDomain, EnumMemberValue, GenericBoundKind, ProtoBound, Symbol,
    SymbolKind, TypeKind,
};
use crate::symbol_path::{GenericSymbolPath, GenericSymbolPathKind};
use crate::symbol_table::{self, ResolveResult};
use crate::value::Value;
use crate::{HashMap, ir_error, namespace_table};
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

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
            let mut comptime =
                expr.eval_comptime(context, dst_type.total_width(), expr.eval_signed());

            check_compatibility(context, &dst_type, &comptime, &token);

            comptime.r#type = dst_type;
            comptime
        } else {
            expr.eval_comptime(context, None, expr.eval_signed())
        };

        Ok((comptime, expr))
    } else {
        let comptime = Comptime::create_unknown(ClockDomain::None, token);
        let expr = ir::Expression::Term(Box::new(ir::Factor::Unknown(token)));
        Ok((comptime, expr))
    }
}

pub fn eval_range(context: &mut Context, range: &Range) -> IrResult<(usize, usize)> {
    let mut beg: ir::Expression = Conv::conv(context, range.expression.as_ref())?;
    let beg = beg.eval_comptime(context, None, beg.eval_signed());
    let beg = beg.get_value()?.to_usize().unwrap_or(0);

    let end = if let Some(x) = &range.range_opt {
        let mut end: ir::Expression = Conv::conv(context, x.expression.as_ref())?;
        let end = end.eval_comptime(context, None, end.eval_signed());
        let end = end.get_value()?.to_usize().unwrap_or(0);

        if matches!(x.range_operator.as_ref(), RangeOperator::DotDotEqu(_)) {
            end + 1
        } else {
            end
        }
    } else {
        beg
    };

    Ok((beg, end))
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

    let ir::Expression::ArrayLiteral(items) = expr else {
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
                            expr: expr.clone(),
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
                            expr: expr.clone(),
                        }]
                    };

                    if default.is_none() {
                        default = Some(exprs);
                    } else {
                        // TODO multiple default error
                        return Err(ir_error!(token));
                    }
                }
            }
        }

        if let Some(Some(target_len)) = array_width.get(0) {
            if let Some(x) = default {
                let remaining = target_len.checked_sub(x.len() - 1);
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
                    // TODO mismatch dimension error
                    return Err(ir_error!(token));
                }
            } else if *target_len != len {
                // TODO mismatch dimension error
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
    let repeat = expr.eval_comptime(context, None, expr.eval_signed());

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
    dst: &ir::AssignDestination,
    expr: &mut (ir::Comptime, ir::Expression),
    token: TokenRange,
) -> IrResult<Vec<ir::Statement>> {
    let (comptime, expr) = expr;

    check_clock_domain(context, &dst.comptime, comptime, &token.beg);

    if context.is_affiliated(Affiliation::AlwaysFf)
        && let Some(clock) = context.current_clock.clone()
    {
        check_clock_domain(context, &dst.comptime, &clock, &token.beg);
    }

    let width = dst.total_width(context);
    let mut ret = vec![];

    let array_exprs = eval_array_literal(
        context,
        Some(&dst.comptime.r#type.array),
        Some(&dst.comptime.r#type.width),
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
        part_type.width.drain(0..expr.select.len());
        let part_width = part_type.total_width().ok_or_else(|| ir_error!(token))?;

        let signed = expr.expr.eval_signed();
        let mut part_value = expr
            .expr
            .eval_value(context, Some(part_width), signed)
            .ok_or_else(|| ir_error!(token))?;
        part_value.trunc(part_width);

        value = if let Some(x) = value {
            Some(x.concat(&part_value))
        } else {
            Some(part_value)
        };

        prev = Some(expr.index);
    }

    if let Some(x) = value {
        ret.push(x);
    }

    Ok(ret)
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
        ir::Expression::ArrayLiteral(_) => {
            let exprs =
                eval_array_literal(context, Some(&r#type.array), Some(&r#type.width), expr)?;
            if let Some(exprs) = exprs {
                let values = eval_array_literal_expressions(context, r#type, exprs, token)?;
                let id = context.insert_var_path(path.clone(), comptime);
                let variable = Variable::new(
                    id,
                    path.clone(),
                    kind,
                    r#type.clone(),
                    values,
                    context.get_affiliation(),
                    &dst.token,
                );
                context.insert_variable(id, variable);
            } else {
                return Err(ir_error!(token));
            }
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

                    let variable = Variable::new(
                        id,
                        path.clone(),
                        kind,
                        r#type.clone(),
                        vec![value],
                        context.get_affiliation(),
                        &dst.token,
                    );
                    context.insert_variable(id, variable);
                }
                ValueVariant::NumericArray(_) => {
                    // TODO for param array
                    return Err(ir_error!(token));
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

    let values = if let Some(total_array) = r#type.total_array()
        && let Some(total_width) = r#type.total_width()
    {
        let mut values = vec![];
        for _ in 0..total_array {
            values.push(Value::new_x(total_width, signed));
        }
        values
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

    let variable = Variable::new(
        id,
        path.clone(),
        kind,
        r#type.clone(),
        values,
        context.get_affiliation(),
        &token,
    );
    context.insert_variable(id, variable);
}

fn check_reset_non_elaborative(context: &mut Context, expr: &mut ir::Expression) {
    let comptime = expr.eval_comptime(context, None, expr.eval_signed());
    if context.in_if_reset && !comptime.is_const {
        context.insert_error(AnalyzerError::unevaluable_value(
            UnevaluableValueKind::ResetValue,
            &comptime.token,
        ));
    }
}

pub fn eval_struct_member(
    context: &mut Context,
    path: &GenericSymbolPath,
    mut member_path: VarPath,
    token: TokenRange,
) -> IrResult<ir::Factor> {
    let mut parent_path = path.clone();
    if let Some(x) = parent_path.paths.pop() {
        member_path.add_prelude(&[x.base.text]);
    }

    if let Ok(symbol) = symbol_table::resolve(&parent_path) {
        match &symbol.found.kind {
            SymbolKind::Parameter(x) => {
                if let Some(expr) = &x.value {
                    let path = VarPath::new(symbol.found.token.text);
                    let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
                    let (_, mut expr) = eval_expr(context, Some(r#type.clone()), expr, false)?;

                    member_path.add_prelude(&path.0);
                    for x in r#type.expand_struct_union(&path, &[], None) {
                        if x.path == member_path {
                            let comptime = expr.eval_comptime(context, None, expr.eval_signed());
                            // TODO range select from PartSelect
                            return Ok(ir::Factor::Value(comptime, token));
                        }
                    }
                }
                Err(ir_error!(token))
            }
            SymbolKind::ProtoConst(x) => {
                let path = VarPath::new(symbol.found.token.text);
                let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
                member_path.add_prelude(&path.0);
                for x in r#type.expand_struct_union(&path, &[], None) {
                    if x.path == member_path {
                        let mut comptime = Comptime::from_type(r#type, ClockDomain::None, token);
                        comptime.is_const = true;
                        return Ok(ir::Factor::Value(comptime, token));
                    }
                }
                Err(ir_error!(token))
            }
            SymbolKind::GenericParameter(x) => match &x.bound {
                GenericBoundKind::Type => {
                    let mut comptime = Comptime::create_unknown(ClockDomain::None, token);
                    comptime.is_const = true;
                    Ok(ir::Factor::Value(comptime, token))
                }
                GenericBoundKind::Proto(x) => {
                    let r#type = x.to_ir_type(context, TypePosition::Variable)?;
                    let mut comptime = Comptime::from_type(r#type, ClockDomain::None, token);
                    comptime.is_const = true;
                    Ok(ir::Factor::Value(comptime, token))
                }
                _ => Err(ir_error!(token)),
            },
            SymbolKind::StructMember(_) => {
                eval_struct_member(context, &parent_path, member_path, token)
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
    let mut width = Shape::default();
    let mut array = Shape::default();
    let mut signed = false;

    let kind = if let Some(x) = path.to_var_path()
        && let Some(x) = context.var_paths.get(&x)
    {
        match &x.1.value {
            ValueVariant::Type(x) => {
                let mut x = x.clone();

                // append internal width/array
                width.append(&mut x.width);
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
                    &symbol.found.token.to_string(),
                    "enum or union or struct",
                    &symbol.found.kind.to_kind_name(),
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
                                r#type.width.replace(0, Some(x.width));
                            }
                        }

                        context.pop_generic_map();

                        ret?
                    } else {
                        ir::Type {
                            kind: ir::TypeKind::Logic,
                            width: Shape::new(vec![Some(x.width)]),
                            ..Default::default()
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
                SymbolKind::TypeDef(x) => {
                    context.push_generic_map(map.clone());

                    let r#type = context.block(|c| x.r#type.to_ir_type(c, TypePosition::TypeDef));

                    context.pop_generic_map();

                    let mut r#type = r#type?;

                    width.append(&mut r#type.width);
                    array.append(&mut r#type.array);
                    signed = r#type.signed;

                    r#type.kind
                }
                SymbolKind::ProtoConst(x) => {
                    if x.r#type.kind.is_type() {
                        ir::TypeKind::Unknown
                    } else {
                        let token: TokenRange = symbol.found.token.into();
                        return Err(ir_error!(token));
                    }
                }
                SymbolKind::ProtoTypeDef(x) => {
                    if let Some(x) = &x.r#type {
                        let mut r#type = x.to_ir_type(context, TypePosition::TypeDef)?;
                        width.append(&mut r#type.width);
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
                        width.append(&mut r#type.width);
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
                SymbolKind::Parameter(x) => {
                    if x.r#type.kind.is_type() {
                        if let Some(expr) = &x.value {
                            context.push_generic_map(map.clone());

                            let expr = context.block(|c| eval_expr(c, None, expr, false));

                            context.pop_generic_map();

                            let (comptime, _) = expr?;
                            if let ValueVariant::Type(x) = &comptime.value {
                                x.kind.clone()
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

    Ok(ir::Type {
        kind,
        signed,
        width,
        array,
    })
}

fn check_struct_union_members(
    context: &mut Context,
    members: &[ir::TypeKindMember],
    symbol: &Symbol,
) {
    if context.in_generic {
        return;
    }

    if !(members.iter().all(|x| x.r#type.is_4state())
        || members.iter().all(|x| x.r#type.is_2state()))
    {
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
    if r#type.is_struct_union() && !r#type.width.is_empty() && !width_select.is_empty() {
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

pub fn eval_for_range(
    context: &mut Context,
    range: &Range,
    rev: bool,
    step: Option<(&AssignmentOperator, &Expression)>,
    token: TokenRange,
) -> IrResult<Vec<usize>> {
    let (beg, end) = eval_range(context, range)?;

    if let Some((op, expr)) = step {
        let mut step: ir::Expression = Conv::conv(context, expr)?;
        let step = step.eval_comptime(context, None, step.eval_signed());
        let step = step.get_value()?.to_usize().unwrap_or(0);
        let op: ir::Op = Conv::conv(context, op)?;

        let mut ret = vec![];
        let mut tmp = beg;
        let mut count = 0;
        while tmp < end {
            ret.push(tmp);
            tmp = op.eval(tmp, step);
            count += 1;

            if tmp == op.eval(tmp, step) {
                break;
            }
            if context.check_size(count, token).is_none() {
                break;
            }
        }
        Ok(ret)
    } else if rev {
        Ok((beg..end).rev().collect())
    } else {
        Ok((beg..end).collect())
    }
}

pub fn eval_function_call(
    context: &mut Context,
    value: &IdentifierFactor,
    token: TokenRange,
) -> IrResult<ir::Expression> {
    if let Some(x) = &value.identifier_factor_opt
        && let IdentifierFactorOptGroup::FunctionCall(x) = x.identifier_factor_opt_group.as_ref()
    {
        let args = if let Some(x) = &x.function_call.function_call_opt {
            argument_list(context, x.argument_list.as_ref())?
        } else {
            ir::Arguments::Null
        };

        let symbol = symbol_table::resolve(value.expression_identifier.as_ref())
            .map_err(|_| ir_error!(token))?;

        match &symbol.found.kind {
            SymbolKind::SystemFunction(_) => {
                let name = symbol.found.token.text;
                let args = args.to_system_function_args(context, &symbol.found);
                let ret = ir::SystemFunctionCall::new(context, name, args, token)?;
                Ok(ir::Expression::Term(Box::new(
                    ir::Factor::SystemFunctionCall(ret, token),
                )))
            }
            SymbolKind::Function(_) | SymbolKind::ModportFunctionMember(_) => {
                let ret =
                    function_call(context, value.expression_identifier.as_ref(), args, token)?;

                Ok(ir::Expression::Term(Box::new(ir::Factor::FunctionCall(
                    ret, token,
                ))))
            }
            SymbolKind::SystemVerilog => {
                let mut x = Comptime::create_unknown(ClockDomain::None, token);
                x.is_const = true;
                context.insert_ir_error::<()>(&Err(ir_error!(token)));
                Ok(ir::Expression::Term(Box::new(ir::Factor::Value(x, token))))
            }
            SymbolKind::ProtoFunction(_) => Err(ir_error!(token)),
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
            _ => {
                // TODO error: non-struct/union type
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
                // TODO error: unknown member
                return Err(ir_error!(token));
            };

            ret.push((x.name, expr));
        }
        Ok(ir::Expression::StructConstructor(r#type, ret))
    } else {
        unreachable!();
    }
}

pub fn eval_factor_path(
    context: &mut Context,
    symbol_path: GenericSymbolPath,
    var_path: VarPathSelect,
    allow_unknown_value: bool,
    token: TokenRange,
) -> IrResult<ir::Factor> {
    let (path, select, _) = var_path.into();
    let generic_path = context.resolve_path(symbol_path);

    let found = if let Some(path) = generic_path.to_var_path()
        && let Some((var_id, comptime)) = context.find_path(&path)
    {
        Some((var_id, comptime))
    } else {
        context.find_path(&path)
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

            if comptime.r#type.is_type() {
                Ok(ir::Factor::Value(comptime, token))
            } else {
                Ok(ir::Factor::Variable(
                    var_id,
                    index,
                    width_select,
                    comptime,
                    token,
                ))
            }
        }
    } else if let Some(x) = generic_path.to_literal() {
        let x = x.eval_comptime(token);
        Ok(ir::Factor::Value(x, token))
    } else if generic_path.is_anonymous() {
        Ok(ir::Factor::Anonymous(token))
    } else if let Ok(symbol) = symbol_table::resolve(&generic_path) {
        // To resolve external symbol reference,
        // use an independent context to avoid name conflict
        let mut external_context = Context::default();
        external_context.inherit(context);

        external_context.push_generic_map(generic_path.to_generic_maps());
        let ret = external_context
            .block(|c| eval_external_symbol(c, generic_path, symbol, allow_unknown_value, token));

        external_context.pop_generic_map();
        context.inherit(&mut external_context);

        ret
    } else {
        Err(ir_error!(token))
    }
}

pub fn eval_external_symbol(
    context: &mut Context,
    path: GenericSymbolPath,
    symbol: ResolveResult,
    allow_unknown_value: bool,
    token: TokenRange,
) -> IrResult<ir::Factor> {
    match &symbol.found.kind {
        SymbolKind::Parameter(x) => {
            if let Some(expr) = &x.value {
                let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
                let (mut comptime, _) =
                    context.block(|c| eval_expr(c, Some(r#type), expr, false))?;

                if let Some(width) = comptime.r#type.total_width() {
                    comptime.value.expand_value(width);
                }

                return Ok(ir::Factor::Value(comptime, token));
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

            if let Some(proto) = x.bound.resolve_proto_bound(&symbol.found.namespace) {
                let r#type = match proto {
                    ProtoBound::FactorType(x) => Some(x),
                    ProtoBound::Enum((_, x)) => Some(x),
                    ProtoBound::Struct((_, x)) => Some(x),
                    ProtoBound::Union((_, x)) => Some(x),
                    _ => None,
                };
                if let Some(r#type) = r#type {
                    let r#type = r#type.to_ir_type(context, TypePosition::Generic)?;
                    let mut x = Comptime::create_unknown(ClockDomain::None, token);

                    if let Some(val) = default_value {
                        x.value = val.value;
                    }

                    // GenericParameter is const and global
                    x.is_const = true;
                    x.is_global = true;
                    x.r#type = r#type;

                    return Ok(ir::Factor::Value(x, token));
                }
            } else if matches!(x.bound, GenericBoundKind::Type) {
                let mut x = Comptime::create_unknown(ClockDomain::None, token);

                if let Some(val) = default_value {
                    x.value = val.value;
                }

                // GenericParameter is const and global
                x.is_const = true;
                x.is_global = true;
                x.r#type.kind = ir::TypeKind::Type;

                return Ok(ir::Factor::Value(x, token));
            } else {
                context.insert_error(AnalyzerError::invalid_factor(
                    Some(&symbol.found.token.to_string()),
                    &symbol.found.kind.to_kind_name(),
                    &token,
                    &[],
                ));
            }
        }
        SymbolKind::ProtoConst(x) if allow_unknown_value => {
            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
            let mut x = Comptime::from_type(r#type, ClockDomain::None, token);

            x.is_const = true;
            x.is_global = true;

            return Ok(ir::Factor::Value(x, token));
        }
        SymbolKind::ProtoTypeDef(_) if allow_unknown_value => {
            let mut x = Comptime::create_unknown(ClockDomain::None, token);

            x.is_const = true;
            x.is_global = true;

            return Ok(ir::Factor::Value(x, token));
        }
        SymbolKind::EnumMember(x) => {
            let enum_symbol = symbol.found.get_parent().unwrap();
            let SymbolKind::Enum(r#enum) = enum_symbol.kind else {
                unreachable!();
            };

            match &x.value {
                EnumMemberValue::ImplicitValue(x) => {
                    let value = Value::new(*x as u64, r#enum.width, false);
                    return Ok(ir::Factor::create_value(value, token));
                }
                EnumMemberValue::ExplicitValue(x, _) => {
                    let (x, _) = eval_expr(context, None, x, false)?;
                    return Ok(ir::Factor::Value(x, token));
                }
                EnumMemberValue::UnevaluableValue => (),
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
            return eval_struct_member(context, &path, VarPath::default(), token);
        }
        SymbolKind::SystemVerilog => {
            let r#type = ir::Type {
                kind: ir::TypeKind::SystemVerilog,
                ..Default::default()
            };
            let mut x = Comptime::from_type(r#type, ClockDomain::None, token);

            // $sv member is const / global
            x.is_const = true;
            x.is_global = true;

            return Ok(ir::Factor::Value(x, token));
        }
        SymbolKind::Function(_) | SymbolKind::Module(_) | SymbolKind::SystemFunction(_) => {
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

            return Ok(ir::Factor::Value(x, token));
        }
        SymbolKind::Enum(_) | SymbolKind::Struct(_) | SymbolKind::Union(_) => {
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
                r#type: ir::Type {
                    kind: ir::TypeKind::Type,
                    ..Default::default()
                },
                is_const: true,
                is_global: true,
                token,
                ..Default::default()
            };

            return Ok(ir::Factor::Value(x, token));
        }
        _ => (),
    }
    Err(ir_error!(token))
}

pub fn case_condition(
    context: &mut Context,
    tgt: &ir::Expression,
    cond: &CaseCondition,
) -> IrResult<ir::Expression> {
    let mut ret = range_item(context, tgt, &cond.range_item)?;
    for x in &cond.case_condition_list {
        let item = range_item(context, tgt, &x.range_item)?;
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(item));
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
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(item));
    }
    Ok(ret)
}

fn range_item(
    context: &mut Context,
    tgt: &ir::Expression,
    range_item: &RangeItem,
) -> IrResult<ir::Expression> {
    let mut exp: ir::Expression = Conv::conv(context, range_item.range.expression.as_ref())?;

    let comptime = exp.eval_comptime(context, None, exp.eval_signed());
    if !comptime.is_const {
        context.insert_error(AnalyzerError::unevaluable_value(
            UnevaluableValueKind::CaseCondition,
            &range_item.into(),
        ));
    }

    let ret = if let Some(x) = &range_item.range.range_opt {
        let mut exp0: ir::Expression = Conv::conv(context, x.expression.as_ref())?;

        let comptime = exp0.eval_comptime(context, None, exp0.eval_signed());
        if !comptime.is_const {
            context.insert_error(AnalyzerError::unevaluable_value(
                UnevaluableValueKind::CaseCondition,
                &range_item.into(),
            ));
        }

        match x.range_operator.as_ref() {
            RangeOperator::DotDot(_) => {
                let cond0 = ir::Expression::Binary(
                    Box::new(exp.clone()),
                    Op::LessEq,
                    Box::new(tgt.clone()),
                );
                let cond1 =
                    ir::Expression::Binary(Box::new(tgt.clone()), Op::Less, Box::new(exp0.clone()));
                ir::Expression::Binary(Box::new(cond0), Op::LogicAnd, Box::new(cond1))
            }
            RangeOperator::DotDotEqu(_) => {
                let cond0 = ir::Expression::Binary(
                    Box::new(exp.clone()),
                    Op::LessEq,
                    Box::new(tgt.clone()),
                );
                let cond1 = ir::Expression::Binary(
                    Box::new(tgt.clone()),
                    Op::LessEq,
                    Box::new(exp0.clone()),
                );
                ir::Expression::Binary(Box::new(cond0), Op::LogicAnd, Box::new(cond1))
            }
        }
    } else {
        ir::Expression::Binary(Box::new(tgt.clone()), Op::EqWildcard, Box::new(exp))
    };
    Ok(ret)
}

pub fn switch_condition(context: &mut Context, cond: &SwitchCondition) -> IrResult<ir::Expression> {
    let mut ret: ir::Expression = Conv::conv(context, cond.expression.as_ref())?;
    for x in &cond.switch_condition_list {
        let exp: ir::Expression = Conv::conv(context, x.expression.as_ref())?;
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(exp));
    }
    Ok(ret)
}

pub fn argument_list(context: &mut Context, value: &ArgumentList) -> IrResult<Arguments> {
    let mut positional = vec![];
    let mut named = vec![];
    let x: Vec<_> = value.into();
    for arg in x {
        if let Some(x) = &arg.argument_item_opt {
            if let Some(name) = arg.argument_expression.expression.unwrap_identifier() {
                let name = name.identifier().token.text;
                let token: TokenRange = x.expression.as_ref().into();
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
) -> IrResult<ir::Component> {
    let symbol = symbol_table::get(sig.symbol).unwrap();

    if let SymbolKind::SystemVerilog = symbol.kind {
        let component = ir::SystemVerilog {
            name: symbol.token.text,
            connects: vec![],
        };
        return Ok(ir::Component::SystemVerilog(component));
    }

    if let Some(component) = context.get_instance_history(sig) {
        Ok(component)
    } else {
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
            SymbolKind::Module(x) => {
                let definition =
                    definition_table::get(x.definition).ok_or_else(|| ir_error!(token))?;
                let Definition::Module(x) = definition else {
                    unreachable!()
                };

                let component: IrResult<ir::Module> = Conv::conv(c, &x);
                match component {
                    Ok(mut component) => {
                        // If IR is not used, component body is not necessary
                        if !c.config.use_ir {
                            component.functions.clear();
                            component.declarations.clear();
                        }

                        let component = ir::Component::Module(component);
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
            SymbolKind::Interface(x) => {
                let definition =
                    definition_table::get(x.definition).ok_or_else(|| ir_error!(token))?;
                let Definition::Interface(x) = definition else {
                    unreachable!()
                };

                let component: IrResult<ir::Interface> = Conv::conv(c, &x);
                match component {
                    Ok(component) => {
                        let component = ir::Component::Interface(component);
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
            SymbolKind::ProtoModule(x) => {
                let definition =
                    definition_table::get(x.definition).ok_or_else(|| ir_error!(token))?;
                let Definition::ProtoModule(x) = definition else {
                    unreachable!()
                };

                let component: IrResult<ir::Module> = Conv::conv(c, &x);
                match component {
                    Ok(component) => {
                        let component = ir::Component::Module(component);
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
            .map(|x| x.found)
        else {
            continue;
        };

        let target_type = if let Some(x) = target.kind.get_type() {
            let x = x.to_ir_type(context, TypePosition::Variable);
            context.insert_ir_error(&x);
            if let Ok(x) = x {
                Some(x)
            } else {
                continue;
            }
        } else {
            None
        };

        let expr = if let Some(x) = &param.inst_parameter_item_opt {
            eval_expr(context, target_type, &x.expression, false)?
        } else {
            let src: Expression = param.identifier.as_ref().into();
            eval_expr(context, target_type, &src, false)?
        };

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
                let expr = if let Some((var_id, comptime)) = context.find_path(&member_path) {
                    ir::Expression::Term(Box::new(ir::Factor::Variable(
                        var_id,
                        VarIndex::default(),
                        VarSelect::default(),
                        comptime,
                        token,
                    )))
                } else {
                    ir::Expression::Term(Box::new(ir::Factor::Unknown(token)))
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
    } else if let Some((var_id, comptime)) = context.find_path(port_path) {
        check_compatibility(context, port_type, &comptime, &token);

        let expr = ir::Expression::Term(Box::new(ir::Factor::Variable(
            var_id,
            VarIndex::default(),
            VarSelect::default(),
            comptime,
            token,
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
    variable: &[&ir::Variable],
    dst: Vec<VarPathSelect>,
    expr: ir::Expression,
    inputs: &mut Vec<ir::InstInput>,
    outputs: &mut Vec<ir::InstOutput>,
) {
    match variable[0].kind {
        VarKind::Input => {
            let id = variable.iter().map(|x| x.id).collect();
            inputs.push(ir::InstInput { id, expr });
        }
        VarKind::Output => {
            if !expr.is_assignable() {
                context.insert_error(AnalyzerError::unassignable_output(&expr.token_range()));
            }
            let id = variable.iter().map(|x| x.id).collect();
            let dst = var_path_to_assign_destination(context, dst, false);
            outputs.push(ir::InstOutput { id, dst });
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
                let src = ir::Factor::Value(comptime.clone(), token);
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
        .flat_map(|x| {
            let mut ret = vec![];
            let dst = x.to_assign_destination(context, ignore_error);
            if let Some(dst) = dst {
                ret.push(dst);
            }
            ret
        })
        .collect()
}

fn get_function(
    context: &mut Context,
    path: &FuncPath,
    is_local_function: bool,
    token: TokenRange,
) -> IrResult<FuncProto> {
    if !context.func_paths.contains_key(path) {
        if is_local_function
            && context.func_call_depth == 0
            && !context.func_paths.contains_key(&path.base())
        {
            context.insert_error(AnalyzerError::referring_before_definition(
                &token.beg.text.to_string(),
                &token,
            ));
            return Err(ir_error!(token));
        }

        let symbol = symbol_table::get(path.sig.symbol).unwrap();
        let definition = match &symbol.kind {
            SymbolKind::Function(x) => x.definition.unwrap(),
            SymbolKind::ModportFunctionMember(x) => {
                let symbol = symbol_table::get(x.function).unwrap();
                let SymbolKind::Function(x) = symbol.kind else {
                    unreachable!();
                };
                x.definition.unwrap()
            }
            _ => return Err(ir_error!(token)),
        };

        let definition = definition_table::get(definition).unwrap();
        let Definition::Function(definition) = definition else {
            unreachable!()
        };

        let array = if let Some((_, comptime)) = context.find_path(&path.path) {
            comptime.r#type.array
        } else {
            Shape::default()
        };

        let mut local_context = Context::default();
        local_context.var_id = context.var_id;
        local_context.inherit(context);
        local_context.extract_var_paths(context, &path.path, &array);

        local_context.func_call_depth += 1;
        let ret: IrResult<()> = Conv::conv(&mut local_context, (&definition, Some(path)));
        local_context.func_call_depth -= 1;

        context.extract_function(&mut local_context, &path.path, &array);
        context.inherit(&mut local_context);
        context.var_id = local_context.var_id;

        ret?;
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

    check_generic_args(context, &generic_path);

    let mut parent_path = generic_path.clone();
    parent_path.paths.pop();
    let sig = Signature::from_path(context, generic_path).ok_or_else(|| ir_error!(token))?;

    let is_local_function = {
        let namespace = namespace_table::get(path.identifier().token.id).unwrap();
        let symbol = symbol_table::get(sig.symbol).unwrap();
        namespace.included(&symbol.namespace)
    };

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

    let ret = context.block(|c| {
        let func = get_function(c, &path, is_local_function, token)?;
        let (inputs, outputs) = args.to_function_args(c, &func, token)?;
        Ok(ir::FunctionCall {
            id: func.id,
            index,
            ret: func.r#type,
            inputs,
            outputs,
            token,
        })
    });

    context.pop_generic_map();
    ret
}

pub fn check_compatibility(
    context: &mut Context,
    dst: &ir::Type,
    src: &ir::Comptime,
    token: &TokenRange,
) {
    if !dst.compatible(src) {
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

pub fn check_module_with_unevaluable_generic_parameters(ident: &Identifier) -> bool {
    if let Ok(symbol) = symbol_table::resolve(ident)
        && let SymbolKind::Module(x) = symbol.found.kind
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
                            && matches!(
                                symbol.found.kind,
                                SymbolKind::ProtoPackage(_) | SymbolKind::ProtoAliasPackage(_)
                            )
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
