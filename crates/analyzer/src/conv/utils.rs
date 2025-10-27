use crate::analyzer_error::AnalyzerError;
use crate::conv::checker::anonymous::check_anonymous;
use crate::conv::checker::clock_domain::check_clock_domain;
use crate::conv::checker::generic::check_generic_args;
use crate::conv::instance::InstanceHistoryError;
use crate::conv::{Context, Conv};
use crate::definition_table::{self, Definition};
use crate::ir::{
    self, Arguments, Comptime, FuncPath, FuncProto, IrResult, Op, Signature, ValueVariant, VarId,
    VarIndex, VarKind, VarPath, VarPathSelect, VarSelect,
};
use crate::symbol::{Affiliation, ClockDomain, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use crate::value::Value;
use crate::{HashMap, ir_error};
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

    if let Ok(expr) = expr {
        check_anonymous(context, &expr, allow_anonymous, token);

        let comptime = if let Some(dst_type) = dst_type {
            let mut comptime = expr.eval_comptime(context, Some(dst_type.total_width()));

            check_compatibility(context, &dst_type, &comptime, &token);

            comptime.r#type = dst_type;
            comptime
        } else {
            expr.eval_comptime(context, None)
        };

        Ok((comptime, expr))
    } else {
        let comptime = Comptime::create_unknown(ClockDomain::None, token);
        let expr = ir::Expression::Term(Box::new(ir::Factor::Unknown(token)));
        Ok((comptime, expr))
    }
}

pub fn eval_range(context: &mut Context, range: &Range) -> IrResult<(usize, usize)> {
    let beg: ir::Expression = Conv::conv(context, range.expression.as_ref())?;
    let beg = beg.eval_comptime(context, None);
    let beg = beg.get_value()?.to_usize();

    let end = if let Some(x) = &range.range_opt {
        let end: ir::Expression = Conv::conv(context, x.expression.as_ref())?;
        let end = end.eval_comptime(context, None);
        let end = end.get_value()?.to_usize();

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

pub fn eval_array_literal(
    context: &mut Context,
    context_array: Option<&[usize]>,
    expr: &ir::Expression,
) -> IrResult<Option<Vec<ir::Expression>>> {
    let token = expr.token_range();

    let ir::Expression::ArrayLiteral(x) = expr else {
        return Ok(None);
    };

    let mut ret = vec![];
    if let Some(array) = context_array {
        let mut default = None;
        let mut len = 0;

        for x in x {
            // context_array for inner item
            let next_array = if array.len() < 2 {
                None
            } else {
                Some(&array[1..])
            };

            match x {
                ir::ArrayLiteralItem::Value(expr, repeat) => {
                    let repeat = if let Some(repeat) = repeat {
                        let repeat =
                            eval_repeat(context, repeat).ok_or_else(|| ir_error!(token))?;
                        repeat.to_usize()
                    } else {
                        1
                    };

                    let exprs = if let Some(x) = eval_array_literal(context, next_array, expr)? {
                        x
                    } else {
                        vec![expr.clone()]
                    };

                    for _ in 0..repeat {
                        let mut exprs = exprs.clone();
                        ret.append(&mut exprs);
                    }

                    len += repeat;
                }
                ir::ArrayLiteralItem::Defaul(expr) => {
                    let exprs = if let Some(x) = eval_array_literal(context, next_array, expr)? {
                        x
                    } else {
                        vec![expr.clone()]
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

        let target_len = array[0];

        if let Some(x) = default {
            let remaining = target_len.checked_sub(x.len() - 1);
            if let Some(remaining) = remaining {
                for _ in 0..remaining {
                    ret.append(&mut x.clone());
                }
            } else {
                // TODO mismatch dimension error
                return Err(ir_error!(token));
            }
        } else if target_len != len {
            // TODO mismatch dimension error
            return Err(ir_error!(token));
        }
    } else {
        // TODO error, not array context
        return Err(ir_error!(token));
    }

    Ok(Some(ret))
}

pub fn eval_struct_constructor(
    context: &mut Context,
    dst_path: &VarPath,
    dst_type: &ir::Type,
    dsts: &[ir::AssignDestination],
    expr: &ir::Expression,
) -> IrResult<Option<Vec<(ir::AssignDestination, ir::Expression)>>> {
    let token = expr.token_range();

    let ir::Expression::StructConstructor(r#type, exprs) = expr else {
        return Ok(None);
    };

    let comptime = Comptime::from_type(r#type.clone(), ClockDomain::None, token);
    check_compatibility(context, dst_type, &comptime, &token);

    let mut dsts = dsts.to_vec();
    let mut ret = vec![];

    for (name, expr) in exprs {
        if dsts.is_empty() {
            return Err(ir_error!(token));
        }

        let dst = dsts.remove(0);
        let mut path = VarPath::new(*name);
        path.add_prelude(&dst_path.0);

        if dst.path == path {
            ret.push((dst, expr.clone()));
        } else if dst.path.starts_with(&path.0) {
            let name = *path.0.last().unwrap();

            let Some(sub_type) = dst_type.get_member_type(name) else {
                return Err(ir_error!(token));
            };
            let mut sub_path = dst_path.clone();
            sub_path.push(name);

            let mut sub_dsts = vec![dst];
            let cnt = dsts.iter().filter(|x| x.path.starts_with(&path.0)).count();
            for x in dsts.drain(0..cnt) {
                sub_dsts.push(x);
            }
            if let Some(mut x) =
                eval_struct_constructor(context, &sub_path, &sub_type, &sub_dsts, expr)?
            {
                ret.append(&mut x);
            }
        } else {
            return Err(ir_error!(token));
        }
    }

    Ok(Some(ret))
}

pub fn eval_repeat(context: &mut Context, expr: &ir::Expression) -> Option<Value> {
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
        ValueVariant::NumericArray(_) | ValueVariant::Type(_) | ValueVariant::String(_) => {
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

pub fn eval_assign_statement(
    context: &mut Context,
    dst: &ir::AssignDestination,
    expr: &(ir::Comptime, ir::Expression),
    token: TokenRange,
) -> IrResult<Vec<ir::Statement>> {
    let (comptime, expr) = expr;

    check_clock_domain(context, &dst.comptime, comptime, &token.beg);

    if context.is_affiliated(Affiliation::AlwaysFf)
        && let Some(clock) = context.current_clock.clone()
    {
        check_clock_domain(context, &dst.comptime, &clock, &token.beg);
    }

    let Some(width) = dst.total_width(context) else {
        return Err(ir_error!(token));
    };

    let expand_dst = dst.expand(context)?;
    let mut ret = vec![];

    let array_exprs = eval_array_literal(context, Some(&dst.comptime.r#type.array), expr)?;
    let struct_exprs =
        eval_struct_constructor(context, &dst.path, &dst.comptime.r#type, &expand_dst, expr)?;
    if let Some(exprs) = array_exprs {
        for (i, expr) in exprs.into_iter().enumerate() {
            check_reset_non_elaborative(context, &expr);

            let index = VarIndex::from_index(i, &dst.comptime.r#type.array);

            let mut dst = expand_dst.clone();
            for x in &mut dst {
                x.index = index.clone();
            }

            let statement = ir::Statement::Assign(ir::AssignStatement {
                dst,
                width,
                expr,
                token,
            });
            ret.push(statement);
        }
    } else if let Some(exprs) = struct_exprs {
        for (dst, expr) in exprs.into_iter() {
            check_reset_non_elaborative(context, &expr);

            let statement = ir::Statement::Assign(ir::AssignStatement {
                dst: vec![dst],
                width,
                expr,
                token,
            });
            ret.push(statement);
        }
    } else {
        check_reset_non_elaborative(context, expr);

        let statement = ir::Statement::Assign(ir::AssignStatement {
            dst: expand_dst,
            width,
            expr: expr.clone(),
            token,
        });
        ret.push(statement);
    }

    Ok(ret)
}

fn check_reset_non_elaborative(context: &mut Context, expr: &ir::Expression) {
    let comptime = expr.eval_comptime(context, None);
    if context.in_if_reset && !comptime.is_const {
        context.insert_error(AnalyzerError::invalid_reset_non_elaborative(
            &comptime.token,
        ));
    }
}

pub fn eval_struct_member(
    context: &mut Context,
    path: &GenericSymbolPath,
    mut member_path: VarPath,
    token: TokenRange,
) -> IrResult<ir::Expression> {
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
                    let (_, expr) = eval_expr(context, Some(r#type.clone()), expr, false)?;
                    let mut dsts = vec![];
                    for x in r#type.expand_struct(&path) {
                        let path = x.path;
                        let r#type = x.r#type;

                        let dst = ir::AssignDestination {
                            id: VarId::default(),
                            path: path.clone(),
                            index: VarIndex::default(),
                            select: VarSelect::default(),
                            comptime: Comptime::from_type(
                                r#type,
                                ClockDomain::None,
                                TokenRange::default(),
                            ),
                            token: TokenRange::default(),
                        };
                        dsts.push(dst);
                    }
                    let exprs = eval_struct_constructor(context, &path, &r#type, &dsts, &expr)?
                        .ok_or_else(|| ir_error!(token))?;

                    member_path.add_prelude(&path.0);
                    for (dst, expr) in exprs {
                        if dst.path == member_path {
                            let comptime = expr.eval_comptime(context, None);
                            return Ok(ir::Expression::Term(Box::new(ir::Factor::Value(
                                comptime, token,
                            ))));
                        }
                    }
                }
                Err(ir_error!(token))
            }
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
    let mut width = vec![];
    let mut array = vec![];
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
                width.push(x.to_usize());
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
                                let r#type = x.r#type.to_ir_type(c, TypePosition::Variable)?;
                                members.push(ir::TypeKindMember { name, r#type });
                            }
                        }
                        Ok(members)
                    });

                    context.pop_generic_map();

                    ir::TypeKind::Struct(ir::TypeKindStruct {
                        id: symbol.found.id,
                        members: members?,
                    })
                }
                SymbolKind::Enum(x) => {
                    let r#type = if let Some(x) = &x.r#type {
                        context.push_generic_map(map.clone());

                        let ret = context.block(|c| x.to_ir_type(c, TypePosition::Variable));

                        context.pop_generic_map();

                        ret?
                    } else {
                        ir::Type::new(ir::TypeKind::Logic, vec![], vec![x.width], false)
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
                    let mut r#type = x.r#type.to_ir_type(context, TypePosition::TypeDef)?;

                    width.append(&mut r#type.width);
                    array.append(&mut r#type.array);
                    signed = r#type.signed;

                    r#type.kind
                }
                SymbolKind::ProtoTypeDef(x) => {
                    if let Some(x) = &x.r#type {
                        let mut r#type = x.to_ir_type(context, TypePosition::TypeDef)?;
                        width.append(&mut r#type.width);
                        array.append(&mut r#type.array);
                        signed = r#type.signed;

                        r#type.kind
                    } else {
                        let token: TokenRange = symbol.found.token.into();
                        return Err(ir_error!(token));
                    }
                }
                SymbolKind::SystemVerilog => ir::TypeKind::SystemVerilog,
                SymbolKind::Interface(_) => {
                    let token: TokenRange = symbol.found.token.into();
                    let sig =
                        Signature::from_path(context, path).ok_or_else(|| ir_error!(token))?;
                    ir::TypeKind::Interface(sig)
                }
                _ => {
                    let token: TokenRange = symbol.found.token.into();
                    return Err(ir_error!(token));
                }
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
            let (index, select) = select.split(comptime.r#type.array.len());

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
            let (index, select) = select.split(comptime.r#type.array.len());

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
    let exp: ir::Expression = Conv::conv(context, range_item.range.expression.as_ref())?;

    let comptime = exp.eval_comptime(context, None);
    if !comptime.is_const {
        context.insert_error(AnalyzerError::invalid_case_condition_non_elaborative(
            &range_item.into(),
        ));
    }

    let ret = if let Some(x) = &range_item.range.range_opt {
        let exp0: ir::Expression = Conv::conv(context, x.expression.as_ref())?;

        let comptime = exp0.eval_comptime(context, None);
        if !comptime.is_const {
            context.insert_error(AnalyzerError::invalid_case_condition_non_elaborative(
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

    let ret = if !named.is_empty() {
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
                        "hierarchy depth limit",
                        x,
                        &token,
                    ));
                }
                InstanceHistoryError::ExceedTotalLimit(x) => {
                    context.insert_error(AnalyzerError::exceed_limit(
                        "total instance limit",
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
) -> IrResult<HashMap<VarPath, ValueVariant>> {
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

        let value = if let Some(x) = &param.inst_parameter_item_opt {
            eval_expr(context, target_type, &x.expression, false)?
        } else {
            let src: Expression = param.identifier.as_ref().into();
            eval_expr(context, target_type, &src, false)?
        };

        let path = VarPath::new(name);
        ret.insert(path, value.0.value);
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
            let (comptime, _) = eval_expr(context, Some(port_type.clone()), &x.expression, false)?;
            check_compatibility(context, port_type, &comptime, &token);
        }

        let dst_path = if let Some(x) = &port.inst_port_item_opt {
            let dst: Vec<VarPathSelect> = Conv::conv(context, x.expression.as_ref())?;
            let dst = dst.first().ok_or_else(|| ir_error!(token))?;
            dst.0.clone()
        } else {
            port_path.clone()
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
                    VarSelect::default(),
                    token,
                )];

                // append member name to port_path
                let mut port_path = port_path.clone();
                port_path.push(*member_path.0.last().unwrap());

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
    variable: &ir::Variable,
    dst: Vec<VarPathSelect>,
    expr: ir::Expression,
    inputs: &mut HashMap<ir::VarId, ir::Expression>,
    outputs: &mut HashMap<ir::VarId, Vec<ir::AssignDestination>>,
) {
    match variable.kind {
        VarKind::Input => {
            inputs.insert(variable.id, expr);
        }
        VarKind::Output => {
            if !expr.is_assignable() {
                context.insert_error(AnalyzerError::unassignable_output(&expr.token_range()));
            }
            let dst: Vec<_> = dst
                .into_iter()
                .filter_map(|x| x.to_assign_destination(context, false))
                .collect();
            outputs.insert(variable.id, dst);
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
                    && let Some(width) = dst.total_width(context)
                {
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

                if let Some(dst) = dst.to_assign_destination(context, false)
                    && let Some(width) = dst.total_width(context)
                {
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

fn get_function(context: &mut Context, path: &FuncPath, token: TokenRange) -> IrResult<FuncProto> {
    if let Some(x) = context.func_paths.get(path) {
        Ok(x.clone())
    } else {
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
            vec![]
        };

        let mut local_context = Context::default();
        local_context.var_id = context.var_id;
        local_context.inherit(context);
        local_context.extract_var_paths(context, &path.path, &array);

        let ret: IrResult<()> = Conv::conv(&mut local_context, &definition);

        context.extract_function(&mut local_context, &path.path, &array);
        context.inherit(&mut local_context);
        context.var_id = local_context.var_id;

        ret?;

        context
            .func_paths
            .get(&path.base())
            .cloned()
            .ok_or_else(|| ir_error!(token))
    }
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
        let proto = get_function(c, &path, token)?;
        let (inputs, outputs) = args.to_function_args(c, &proto)?;
        Ok(ir::FunctionCall {
            id: proto.id,
            index,
            ret: proto.ret,
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
