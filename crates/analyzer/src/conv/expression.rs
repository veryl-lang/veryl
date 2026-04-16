use crate::analyzer_error::AnalyzerError;
use crate::conv::utils::{
    TypePosition, case_condition, eval_expr, eval_factor_path, eval_function_call, eval_size,
    eval_struct_constructor, eval_type, range_list, switch_condition,
};
use crate::conv::{Context, Conv};
use crate::ir::{
    self, Comptime, IrResult, Op, Shape, Type, TypeKind, ValueVariant, VarPath, VarPathSelect,
};
use crate::symbol::SymbolKind;
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use crate::value::{Value, string_to_byte_value};
use crate::{ir_error, msb_table};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&Expression> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression) -> IrResult<Self> {
        Conv::conv(context, value.if_expression.as_ref())
    }
}

fn is_if_expression(value: &Expression) -> bool {
    !value.if_expression.if_expression_list.is_empty()
}

impl Conv<&IfExpression> for ir::Expression {
    fn conv(context: &mut Context, value: &IfExpression) -> IrResult<Self> {
        let mut ret: ir::Expression = Conv::conv(context, value.expression01.as_ref())?;
        for x in value.if_expression_list.iter().rev() {
            let y: ir::Expression = Conv::conv(context, x.expression.as_ref())?;
            let z: ir::Expression = Conv::conv(context, x.expression0.as_ref())?;

            if is_if_expression(&x.expression) {
                let token: TokenRange = x.expression.as_ref().into();
                context.insert_error(AnalyzerError::unenclosed_inner_if_expression(&token));
            }
            if is_if_expression(&x.expression0) {
                let token: TokenRange = x.expression0.as_ref().into();
                context.insert_error(AnalyzerError::unenclosed_inner_if_expression(&token));
            }

            let token = TokenRange::from_range(&y.token_range(), &ret.token_range());
            let comptime = Box::new(Comptime::create_unknown(token));

            ret = ir::Expression::Ternary(Box::new(y), Box::new(z), Box::new(ret), comptime);
        }
        Ok(ret)
    }
}

fn resolve_op(op: &Expression01Op) -> (Op, u32) {
    match op {
        Expression01Op::Operator01(x) => {
            let tok = x.operator01.operator01_token.to_string();
            match tok.as_str() {
                "||" => (Op::LogicOr, 1),
                "&&" => (Op::LogicAnd, 2),
                _ => unreachable!(),
            }
        }
        Expression01Op::Operator03(_) => (Op::BitOr, 3),
        Expression01Op::Operator04(x) => {
            let tok = x.operator04.operator04_token.to_string();
            match tok.as_str() {
                "^" => (Op::BitXor, 4),
                "~^" => (Op::BitXnor, 4),
                _ => unreachable!(),
            }
        }
        Expression01Op::Operator05(_) => (Op::BitAnd, 5),
        Expression01Op::Operator02(x) => {
            let tok = x.operator02.operator02_token.to_string();
            match tok.as_str() {
                "==" => (Op::Eq, 6),
                "!=" => (Op::Ne, 6),
                "==?" => (Op::EqWildcard, 6),
                "!=?" => (Op::NeWildcard, 6),
                "<=" => (Op::LessEq, 7),
                ">=" => (Op::GreaterEq, 7),
                "<:" => (Op::Less, 7),
                ">:" => (Op::Greater, 7),
                "<<<" => (Op::ArithShiftL, 8),
                ">>>" => (Op::ArithShiftR, 8),
                "<<" => (Op::LogicShiftL, 8),
                ">>" => (Op::LogicShiftR, 8),
                _ => unreachable!(),
            }
        }
        Expression01Op::Operator06(x) => {
            let tok = x.operator06.operator06_token.to_string();
            match tok.as_str() {
                "+" => (Op::Add, 9),
                "-" => (Op::Sub, 9),
                _ => unreachable!(),
            }
        }
        Expression01Op::Operator07(x) => {
            let tok = x.operator07.operator07_token.to_string();
            match tok.as_str() {
                "/" => (Op::Div, 10),
                "%" => (Op::Rem, 10),
                _ => unreachable!(),
            }
        }
        Expression01Op::Star(_) => (Op::Mul, 10),
        Expression01Op::Operator08(_) => (Op::Pow, 11),
    }
}

fn prec_climb(
    context: &mut Context,
    exprs: &[&Expression02],
    ops: &[(Op, u32)],
    lo: usize,
    hi: usize,
) -> IrResult<ir::Expression> {
    if lo == hi {
        return Conv::conv(context, exprs[lo]);
    }
    // find the lowest-precedence (leftmost if tied) operator in [lo..hi)
    let mut min_idx = lo;
    let mut min_prec = ops[lo].1;
    for (i, op) in ops.iter().enumerate().take(hi).skip(lo + 1) {
        if op.1 <= min_prec {
            min_idx = i;
            min_prec = op.1;
        }
    }
    let left = prec_climb(context, exprs, ops, lo, min_idx)?;
    let right = prec_climb(context, exprs, ops, min_idx + 1, hi)?;
    let token = TokenRange::from_range(&left.token_range(), &right.token_range());
    let comptime = Box::new(Comptime::create_unknown(token));
    Ok(ir::Expression::Binary(
        Box::new(left),
        ops[min_idx].0,
        Box::new(right),
        comptime,
    ))
}

impl Conv<&Expression01> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression01) -> IrResult<Self> {
        if value.expression01_list.is_empty() {
            return Conv::conv(context, value.expression02.as_ref());
        }

        let mut exprs: Vec<&Expression02> = vec![value.expression02.as_ref()];
        let mut ops: Vec<(Op, u32)> = Vec::new();

        for x in &value.expression01_list {
            let (op, prec) = resolve_op(x.expression01_op.as_ref());
            ops.push((op, prec));
            exprs.push(x.expression02.as_ref());
        }

        prec_climb(context, &exprs, &ops, 0, exprs.len() - 1)
    }
}

impl Conv<&Expression02> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression02) -> IrResult<Self> {
        let mut ret: ir::Expression = Conv::conv(context, value.factor.as_ref())?;

        // optional `as` cast
        if let Some(x) = &value.expression02_opt {
            let right: ir::Factor = Conv::conv(context, x.casting_type.as_ref())?;

            let token = TokenRange::from_range(&ret.token_range(), &right.token_range());
            let comptime = Box::new(Comptime::create_unknown(token));

            ret = ir::Expression::Binary(
                Box::new(ret),
                Op::As,
                Box::new(ir::Expression::Term(Box::new(right))),
                comptime,
            );
        }

        // unary prefix operators (reverse iteration for right-associativity)
        for x in value.expression02_list.iter().rev() {
            let op = match x.expression02_op.as_ref() {
                Expression02Op::UnaryOperator(x) => {
                    let token = x.unary_operator.unary_operator_token.to_string();
                    match token.as_str() {
                        "~&" => Op::BitNand,
                        "~|" => Op::BitNor,
                        "~" => Op::BitNot,
                        "!" => Op::LogicNot,
                        _ => unreachable!(),
                    }
                }
                Expression02Op::Operator03(_) => Op::BitOr,
                Expression02Op::Operator04(x) => {
                    let token = x.operator04.operator04_token.to_string();
                    match token.as_str() {
                        "^" => Op::BitXor,
                        "~^" => Op::BitXnor,
                        _ => unreachable!(),
                    }
                }
                Expression02Op::Operator05(_) => Op::BitAnd,
                Expression02Op::Operator06(x) => {
                    let token = x.operator06.operator06_token.to_string();
                    match token.as_str() {
                        "+" => Op::Add,
                        "-" => Op::Sub,
                        _ => unreachable!(),
                    }
                }
            };

            let token: TokenRange = value.into();
            let comptime = Box::new(Comptime::create_unknown(token));
            ret = ir::Expression::Unary(op, Box::new(ret), comptime);
        }

        Ok(ret)
    }
}

impl Conv<&FactorType> for ir::Factor {
    fn conv(context: &mut Context, value: &FactorType) -> IrResult<Self> {
        let token = value.into();
        let mut is_global = true;
        let value = match value.factor_type_group.as_ref() {
            FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                let width = if let Some(x) = &x.factor_type_opt {
                    let exprs: Vec<_> = x.width.as_ref().into();
                    let mut ret = Shape::default();
                    for expr in exprs {
                        let (comptime, value) = eval_size(context, expr, false)?;

                        is_global &= comptime.is_global;

                        ret.push(value);
                    }
                    ret
                } else {
                    Shape::new(vec![Some(1)])
                };
                let kind = match x.variable_type.as_ref() {
                    VariableType::Clock(_) => TypeKind::Clock,
                    VariableType::ClockPosedge(_) => TypeKind::ClockPosedge,
                    VariableType::ClockNegedge(_) => TypeKind::ClockNegedge,
                    VariableType::Reset(_) => TypeKind::Reset,
                    VariableType::ResetAsyncHigh(_) => TypeKind::ResetAsyncHigh,
                    VariableType::ResetAsyncLow(_) => TypeKind::ResetAsyncLow,
                    VariableType::ResetSyncHigh(_) => TypeKind::ResetSyncHigh,
                    VariableType::ResetSyncLow(_) => TypeKind::ResetSyncLow,
                    VariableType::Logic(_) => TypeKind::Logic,
                    VariableType::Bit(_) => TypeKind::Bit,
                };
                {
                    let mut t = Type::new(kind);
                    t.set_concrete_width(width);
                    t
                }
            }
            FactorTypeGroup::FixedType(x) => {
                let (kind, width, signed, is_positive) = match x.fixed_type.as_ref() {
                    FixedType::P8(_) => (TypeKind::Bit, Shape::new(vec![Some(8)]), false, true),
                    FixedType::P16(_) => (TypeKind::Bit, Shape::new(vec![Some(16)]), false, true),
                    FixedType::P32(_) => (TypeKind::Bit, Shape::new(vec![Some(32)]), false, true),
                    FixedType::P64(_) => (TypeKind::Bit, Shape::new(vec![Some(64)]), false, true),
                    FixedType::U8(_) => (TypeKind::Bit, Shape::new(vec![Some(8)]), false, false),
                    FixedType::U16(_) => (TypeKind::Bit, Shape::new(vec![Some(16)]), false, false),
                    FixedType::U32(_) => (TypeKind::Bit, Shape::new(vec![Some(32)]), false, false),
                    FixedType::U64(_) => (TypeKind::Bit, Shape::new(vec![Some(64)]), false, false),
                    FixedType::I8(_) => (TypeKind::Bit, Shape::new(vec![Some(8)]), true, false),
                    FixedType::I16(_) => (TypeKind::Bit, Shape::new(vec![Some(16)]), true, false),
                    FixedType::I32(_) => (TypeKind::Bit, Shape::new(vec![Some(32)]), true, false),
                    FixedType::I64(_) => (TypeKind::Bit, Shape::new(vec![Some(64)]), true, false),
                    FixedType::F32(_) => (TypeKind::F32, Shape::new(vec![Some(32)]), false, false),
                    FixedType::F64(_) => (TypeKind::F64, Shape::new(vec![Some(64)]), false, false),
                    FixedType::BBool(_) => (TypeKind::Bit, Shape::new(vec![Some(1)]), false, false),
                    FixedType::LBool(_) => (TypeKind::Bit, Shape::new(vec![Some(1)]), false, false),
                    FixedType::Strin(_) => {
                        (TypeKind::Unknown, Shape::new(vec![Some(1)]), false, false)
                    }
                };
                {
                    let mut t = Type::new(kind);
                    t.set_concrete_width(width);
                    t.signed = signed;
                    t.is_positive = is_positive;
                    t
                }
            }
        };
        let r#type = Type::new(TypeKind::Type);
        let ret = Comptime {
            value: ValueVariant::Type(value),
            r#type,
            is_const: true,
            is_global,
            token,
            ..Default::default()
        };
        Ok(ir::Factor::Value(ret))
    }
}

impl Conv<&CastingType> for ir::Factor {
    fn conv(context: &mut Context, value: &CastingType) -> IrResult<Self> {
        let token = value.into();
        let value = if let CastingType::UserDefinedType(x) = value {
            let identifier = x.user_defined_type.scoped_identifier.as_ref();
            let symbol_path: GenericSymbolPath = identifier.into();

            let r#type = eval_type(context, &symbol_path, TypePosition::Cast);
            if r#type.is_ok() {
                r#type?
            } else {
                let var_path: VarPathSelect = Conv::conv(context, identifier)?;
                return eval_factor_path(context, symbol_path, var_path, false, token);
            }
        } else {
            let (kind, width, signed, is_positive) = match value {
                CastingType::P8(_) => (TypeKind::Bit, Shape::new(vec![Some(8)]), false, true),
                CastingType::P16(_) => (TypeKind::Bit, Shape::new(vec![Some(16)]), false, true),
                CastingType::P32(_) => (TypeKind::Bit, Shape::new(vec![Some(32)]), false, true),
                CastingType::P64(_) => (TypeKind::Bit, Shape::new(vec![Some(64)]), false, true),
                CastingType::U8(_) => (TypeKind::Bit, Shape::new(vec![Some(8)]), false, false),
                CastingType::U16(_) => (TypeKind::Bit, Shape::new(vec![Some(16)]), false, false),
                CastingType::U32(_) => (TypeKind::Bit, Shape::new(vec![Some(32)]), false, false),
                CastingType::U64(_) => (TypeKind::Bit, Shape::new(vec![Some(64)]), false, false),
                CastingType::I8(_) => (TypeKind::Bit, Shape::new(vec![Some(8)]), true, false),
                CastingType::I16(_) => (TypeKind::Bit, Shape::new(vec![Some(16)]), true, false),
                CastingType::I32(_) => (TypeKind::Bit, Shape::new(vec![Some(32)]), true, false),
                CastingType::I64(_) => (TypeKind::Bit, Shape::new(vec![Some(64)]), true, false),
                CastingType::F32(_) => (TypeKind::F32, Shape::new(vec![Some(32)]), false, false),
                CastingType::F64(_) => (TypeKind::F64, Shape::new(vec![Some(64)]), false, false),
                CastingType::BBool(_) => (TypeKind::Bit, Shape::new(vec![Some(1)]), false, false),
                CastingType::LBool(_) => (TypeKind::Bit, Shape::new(vec![Some(1)]), false, false),
                CastingType::Clock(_) => (TypeKind::Clock, Shape::new(vec![Some(1)]), false, false),
                CastingType::ClockPosedge(_) => (
                    TypeKind::ClockPosedge,
                    Shape::new(vec![Some(1)]),
                    false,
                    false,
                ),
                CastingType::ClockNegedge(_) => (
                    TypeKind::ClockNegedge,
                    Shape::new(vec![Some(1)]),
                    false,
                    false,
                ),
                CastingType::Reset(_) => (TypeKind::Reset, Shape::new(vec![Some(1)]), false, false),
                CastingType::ResetAsyncHigh(_) => (
                    TypeKind::ResetAsyncHigh,
                    Shape::new(vec![Some(1)]),
                    false,
                    false,
                ),
                CastingType::ResetAsyncLow(_) => (
                    TypeKind::ResetAsyncLow,
                    Shape::new(vec![Some(1)]),
                    false,
                    false,
                ),
                CastingType::ResetSyncHigh(_) => (
                    TypeKind::ResetSyncHigh,
                    Shape::new(vec![Some(1)]),
                    false,
                    false,
                ),
                CastingType::ResetSyncLow(_) => (
                    TypeKind::ResetSyncLow,
                    Shape::new(vec![Some(1)]),
                    false,
                    false,
                ),
                CastingType::Based(x) => {
                    let token: TokenRange = x.based.based_token.token.into();
                    let comptime: Comptime = Conv::conv(context, x.based.as_ref())?;

                    if let Ok(value) = comptime.get_value()
                        && let Some(value) = value.to_usize()
                    {
                        let _ = context.check_size(value, token);
                    }

                    return Ok(ir::Factor::Value(comptime));
                }
                CastingType::BaseLess(x) => {
                    let token: TokenRange = x.base_less.base_less_token.token.into();
                    let comptime: Comptime = Conv::conv(context, x.base_less.as_ref())?;

                    if let Ok(value) = comptime.get_value()
                        && let Some(value) = value.to_usize()
                    {
                        let _ = context.check_size(value, token);
                    }

                    return Ok(ir::Factor::Value(comptime));
                }
                CastingType::UserDefinedType(_) => unreachable!(),
            };
            {
                let mut t = Type::new(kind);
                t.set_concrete_width(width);
                t.signed = signed;
                t.is_positive = is_positive;
                t
            }
        };

        let r#type = Type::new(TypeKind::Type);
        let ret = Comptime {
            value: ValueVariant::Type(value),
            r#type,
            is_const: true,
            is_global: true,
            token,
            ..Default::default()
        };
        Ok(ir::Factor::Value(ret))
    }
}

impl Conv<&Factor> for ir::Expression {
    fn conv(context: &mut Context, value: &Factor) -> IrResult<Self> {
        let token = value.into();
        match value {
            Factor::Number(x) => {
                let x: Comptime = Conv::conv(context, x.number.as_ref())?;
                Ok(ir::Expression::Term(Box::new(ir::Factor::Value(x))))
            }
            Factor::BooleanLiteral(x) => {
                let x = match x.boolean_literal.as_ref() {
                    BooleanLiteral::True(_) => 1,
                    BooleanLiteral::False(_) => 0,
                };
                let value = Value::new(x, 1, false);
                let r#type = {
                    let mut t = Type::new(TypeKind::Bit);
                    t.set_concrete_width(Shape::new(vec![Some(1)]));
                    t
                };
                let ret = Comptime {
                    value: ValueVariant::Numeric(value),
                    r#type,
                    is_const: true,
                    is_global: true,
                    token,
                    ..Default::default()
                };
                Ok(ir::Expression::Term(Box::new(ir::Factor::Value(ret))))
            }
            Factor::IdentifierFactor(x) => Conv::conv(context, x.identifier_factor.as_ref()),
            Factor::LParenExpressionRParen(x) => Conv::conv(context, x.expression.as_ref()),
            Factor::LBraceConcatenationListRBrace(x) => {
                let x = x.concatenation_list.as_ref();
                let exp: ir::Expression =
                    Conv::conv(context, x.concatenation_item.expression.as_ref())?;
                let rep: Option<ir::Expression> =
                    if let Some(x) = x.concatenation_item.concatenation_item_opt.as_ref() {
                        Some(Conv::conv(context, x.expression.as_ref())?)
                    } else {
                        None
                    };
                let mut ret = vec![(exp, rep)];

                for x in &x.concatenation_list_list {
                    let exp: ir::Expression =
                        Conv::conv(context, x.concatenation_item.expression.as_ref())?;
                    let rep: Option<ir::Expression> =
                        if let Some(x) = x.concatenation_item.concatenation_item_opt.as_ref() {
                            Some(Conv::conv(context, x.expression.as_ref())?)
                        } else {
                            None
                        };
                    ret.push((exp, rep));
                }

                let comptime = Box::new(Comptime::create_unknown(token));

                Ok(ir::Expression::Concatenation(ret, comptime))
            }
            Factor::QuoteLBraceArrayLiteralListRBrace(x) => {
                let items: Vec<_> = x.array_literal_list.as_ref().into();
                let mut ret = vec![];

                for item in items {
                    let item = match item.array_literal_item_group.as_ref() {
                        ArrayLiteralItemGroup::ExpressionArrayLiteralItemOpt(x) => {
                            let rep: Option<ir::Expression> =
                                if let Some(x) = x.array_literal_item_opt.as_ref() {
                                    Some(Conv::conv(context, x.expression.as_ref())?)
                                } else {
                                    None
                                };
                            let exp: ir::Expression = Conv::conv(context, x.expression.as_ref())?;
                            ir::ArrayLiteralItem::Value(exp, rep)
                        }
                        ArrayLiteralItemGroup::DefaulColonExpression(x) => {
                            let exp: ir::Expression = Conv::conv(context, x.expression.as_ref())?;
                            ir::ArrayLiteralItem::Defaul(exp)
                        }
                    };
                    ret.push(item);
                }

                let comptime = Box::new(Comptime::create_unknown(token));

                Ok(ir::Expression::ArrayLiteral(ret, comptime))
            }
            Factor::CaseExpression(x) => {
                let mut tgt: ir::Expression =
                    Conv::conv(context, x.case_expression.expression.as_ref())?;
                tgt.eval_comptime(context, None);
                let exp: ir::Expression =
                    Conv::conv(context, x.case_expression.expression0.as_ref())?;
                let defaul: ir::Expression =
                    Conv::conv(context, x.case_expression.expression1.as_ref())?;
                let cond =
                    case_condition(context, &tgt, x.case_expression.case_condition.as_ref())?;

                let comptime = Box::new(Comptime::create_unknown(token));

                let mut ret = ir::Expression::Ternary(
                    Box::new(cond),
                    Box::new(exp),
                    Box::new(defaul),
                    comptime,
                );

                for x in &x.case_expression.case_expression_list {
                    let cond = case_condition(context, &tgt, x.case_condition.as_ref())?;
                    let exp: ir::Expression = Conv::conv(context, x.expression.as_ref())?;

                    if let ir::Expression::Ternary(x, y, z, comptime) = ret {
                        let arm = ir::Expression::Ternary(
                            Box::new(cond),
                            Box::new(exp),
                            z,
                            comptime.clone(),
                        );
                        ret = ir::Expression::Ternary(x, y, Box::new(arm), comptime);
                    } else {
                        unreachable!()
                    }
                }
                Ok(ret)
            }
            Factor::SwitchExpression(x) => {
                let exp: ir::Expression =
                    Conv::conv(context, x.switch_expression.expression.as_ref())?;
                let defaul: ir::Expression =
                    Conv::conv(context, x.switch_expression.expression0.as_ref())?;
                let cond =
                    switch_condition(context, x.switch_expression.switch_condition.as_ref())?;

                let comptime = Box::new(Comptime::create_unknown(token));

                let mut ret = ir::Expression::Ternary(
                    Box::new(cond),
                    Box::new(exp),
                    Box::new(defaul),
                    comptime,
                );

                for x in &x.switch_expression.switch_expression_list {
                    let cond = switch_condition(context, x.switch_condition.as_ref())?;
                    let exp: ir::Expression = Conv::conv(context, x.expression.as_ref())?;

                    if let ir::Expression::Ternary(x, y, z, comptime) = ret {
                        let arm = ir::Expression::Ternary(
                            Box::new(cond),
                            Box::new(exp),
                            z,
                            comptime.clone(),
                        );
                        ret = ir::Expression::Ternary(x, y, Box::new(arm), comptime);
                    } else {
                        unreachable!()
                    }
                }
                Ok(ret)
            }
            Factor::StringLiteral(x) => {
                let text = x.string_literal.string_literal_token.token.text;
                let text_str =
                    veryl_parser::resource_table::get_str_value(text).unwrap_or_default();
                let value = string_to_byte_value(&text_str);
                let width = value.width() as usize;
                let r#type = {
                    let mut t = Type::new(TypeKind::String);
                    t.set_concrete_width(Shape::new(vec![Some(width)]));
                    t
                };
                let ret = Comptime {
                    value: ValueVariant::Numeric(value),
                    r#type,
                    is_const: true,
                    is_global: true,
                    token,
                    ..Default::default()
                };
                Ok(ir::Expression::Term(Box::new(ir::Factor::Value(ret))))
            }
            Factor::FactorGroup(x) => {
                let Some((path, generic_path)) = context.select_paths.last().cloned() else {
                    return Err(ir_error!(token));
                };

                match x.factor_group.as_ref() {
                    FactorGroup::Msb(msb) => {
                        if let Some((_, comptime)) = context.find_path(&path) {
                            // msb through interface is forbidden
                            // https://github.com/veryl-lang/veryl/pull/1154
                            if let Some((_, comptime)) =
                                context.find_path(&VarPath::new(path.first()))
                                && comptime.r#type.is_interface()
                            {
                                return Err(ir_error!(token));
                            }

                            if comptime.r#type.is_systemverilog() {
                                return Err(ir_error!(token));
                            }

                            let dim = context.get_select_dim().unwrap();

                            let width =
                                if comptime.r#type.is_struct() || comptime.r#type.is_unknown() {
                                    comptime.r#type.total_width()
                                } else {
                                    comptime.r#type.width()[dim]
                                };
                            let comptime = if let Some(width) = width {
                                let msb = width.saturating_sub(1);
                                Comptime::create_value(Value::new(msb as u64, 32, false), token)
                            } else {
                                let mut ret =
                                    Comptime::create_value(Value::new(0, 32, false), token);
                                ret.value = ValueVariant::Unknown;
                                ret
                            };
                            Ok(ir::Expression::Term(Box::new(ir::Factor::Value(comptime))))
                        } else if let Ok(symbol) = symbol_table::resolve(&generic_path)
                            && let SymbolKind::Parameter(x) = &symbol.found.kind
                        {
                            let r#type = x.r#type.to_ir_type(context, TypePosition::Variable)?;
                            let dim = context.get_select_dim().unwrap();

                            msb_table::insert(msb.msb.msb_token.token.id, dim + 1);

                            let width = if r#type.is_struct() {
                                r#type.total_width()
                            } else {
                                r#type.width()[dim]
                            };
                            let msb = if let Some(width) = width {
                                width - 1
                            } else {
                                0
                            };
                            Ok(ir::Expression::create_value(
                                Value::new(msb as u64, 32, false),
                                token,
                            ))
                        } else {
                            context.insert_error(AnalyzerError::unknown_msb(&token));
                            Err(ir_error!(token))
                        }
                    }
                    FactorGroup::Lsb(_) => Ok(ir::Expression::create_value(
                        Value::new(0, 32, false),
                        token,
                    )),
                }
            }
            Factor::InsideExpression(x) => {
                let mut exp: ir::Expression =
                    Conv::conv(context, x.inside_expression.expression.as_ref())?;
                exp.eval_comptime(context, None);
                let ret = range_list(context, &exp, x.inside_expression.range_list.as_ref())?;
                Ok(ret)
            }
            Factor::OutsideExpression(x) => {
                let mut exp: ir::Expression =
                    Conv::conv(context, x.outside_expression.expression.as_ref())?;
                exp.eval_comptime(context, None);
                let ret = range_list(context, &exp, x.outside_expression.range_list.as_ref())?;
                let comptime = Box::new(Comptime::create_unknown(token));
                Ok(ir::Expression::Unary(Op::LogicNot, Box::new(ret), comptime))
            }
            Factor::TypeExpression(x) => {
                let (comptime, _) = eval_expr(context, None, &x.type_expression.expression, false)?;
                let r#type = Type::new(TypeKind::Type);
                let ret = Comptime {
                    value: ValueVariant::Type(comptime.r#type),
                    r#type,
                    is_const: true,
                    is_global: comptime.is_global,
                    token,
                    ..Default::default()
                };
                Ok(ir::Expression::Term(Box::new(ir::Factor::Value(ret))))
            }
            Factor::FactorTypeFactor(x) => {
                let ret = Conv::conv(context, x.factor_type_factor.factor_type.as_ref())?;
                Ok(ir::Expression::Term(Box::new(ret)))
            }
        }
    }
}

impl Conv<&IdentifierFactor> for ir::Expression {
    fn conv(context: &mut Context, value: &IdentifierFactor) -> IrResult<Self> {
        let token: TokenRange = value.into();
        if let Some(x) = &value.identifier_factor_opt {
            let x = x.identifier_factor_opt_group.as_ref();
            match x {
                IdentifierFactorOptGroup::FunctionCall(_) => {
                    eval_function_call(context, value, token)
                }
                IdentifierFactorOptGroup::StructConstructor(_) => {
                    eval_struct_constructor(context, value, token)
                }
            }
        } else {
            let var_path: VarPathSelect =
                Conv::conv(context, value.expression_identifier.as_ref())?;
            let symbol_path: GenericSymbolPath = value.expression_identifier.as_ref().into();

            let factor = eval_factor_path(context, symbol_path, var_path, true, token)?;
            Ok(ir::Expression::Term(Box::new(factor)))
        }
    }
}

impl Conv<&Number> for Comptime {
    fn conv(context: &mut Context, value: &Number) -> IrResult<Self> {
        match value {
            Number::IntegralNumber(x) => match x.integral_number.as_ref() {
                IntegralNumber::Based(x) => Conv::conv(context, x.based.as_ref()),
                IntegralNumber::BaseLess(x) => Conv::conv(context, x.base_less.as_ref()),
                IntegralNumber::AllBit(x) => Conv::conv(context, x.all_bit.as_ref()),
            },
            Number::RealNumber(x) => Conv::conv(context, x.real_number.as_ref()),
        }
    }
}

impl Conv<&Based> for Comptime {
    fn conv(context: &mut Context, value: &Based) -> IrResult<Self> {
        let token: TokenRange = value.into();
        let value: Value = value.into();

        let kind = if value.is_xz() {
            TypeKind::Logic
        } else {
            TypeKind::Bit
        };

        let width = context.check_size(value.width(), token);

        let mut r#type = Type::new(kind);
        r#type.signed = value.signed();
        r#type.set_concrete_width(Shape::new(vec![width]));

        Ok(Comptime {
            value: ValueVariant::Numeric(value),
            r#type,
            is_const: true,
            is_global: true,
            token,
            ..Default::default()
        })
    }
}

impl Conv<&BaseLess> for Comptime {
    fn conv(_context: &mut Context, value: &BaseLess) -> IrResult<Self> {
        let token: TokenRange = value.into();
        let value: Value = value.into();
        let mut r#type = Type::new(TypeKind::Bit);
        r#type.signed = true;
        r#type.set_concrete_width(Shape::new(vec![Some(32)]));

        Ok(Comptime {
            value: ValueVariant::Numeric(value),
            r#type,
            is_const: true,
            is_global: true,
            token,
            ..Default::default()
        })
    }
}

impl Conv<&AllBit> for Comptime {
    fn conv(_context: &mut Context, value: &AllBit) -> IrResult<Self> {
        let token: TokenRange = value.into();
        let value: Value = value.into();

        let kind = if value.is_xz() {
            TypeKind::Logic
        } else {
            TypeKind::Bit
        };

        let mut r#type = Type::new(kind);
        r#type.signed = true;
        r#type.set_concrete_width(Shape::new(vec![Some(0)]));

        Ok(Comptime {
            value: ValueVariant::Numeric(value),
            r#type,
            is_const: true,
            is_global: true,
            token,
            ..Default::default()
        })
    }
}

impl Conv<&RealNumber> for Comptime {
    fn conv(_context: &mut Context, value: &RealNumber) -> IrResult<Self> {
        let token: TokenRange = value.into();
        let ret = match value {
            RealNumber::FixedPoint(x) => {
                let value: Value = x.fixed_point.as_ref().into();
                let mut r#type = Type::new(TypeKind::F64);
                r#type.set_concrete_width(Shape::new(vec![Some(64)]));

                Comptime {
                    value: ValueVariant::Numeric(value),
                    r#type,
                    is_const: true,
                    is_global: true,
                    token,
                    ..Default::default()
                }
            }
            RealNumber::Exponent(x) => {
                let value: Value = x.exponent.as_ref().into();
                let mut r#type = Type::new(TypeKind::F64);
                r#type.set_concrete_width(Shape::new(vec![Some(64)]));

                Comptime {
                    value: ValueVariant::Numeric(value),
                    r#type,
                    is_const: true,
                    is_global: true,
                    token,
                    ..Default::default()
                }
            }
        };
        Ok(ret)
    }
}

impl Conv<&AssignmentOperator> for Op {
    fn conv(_context: &mut Context, value: &AssignmentOperator) -> IrResult<Self> {
        let text = value.assignment_operator_token.token.text.to_string();
        let ret = match text.as_str() {
            "+=" => Op::Add,
            "-=" => Op::Sub,
            "*=" => Op::Mul,
            "/=" => Op::Div,
            "%=" => Op::Rem,
            "&=" => Op::BitAnd,
            "|=" => Op::BitOr,
            "^=" => Op::BitXor,
            "<<=" => Op::LogicShiftL,
            ">>=" => Op::LogicShiftR,
            "<<<=" => Op::ArithShiftL,
            ">>>=" => Op::ArithShiftR,
            _ => unreachable!(),
        };
        Ok(ret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conv::utils::{parse_expression, parse_number};

    #[test]
    fn baseless() {
        let mut context = Context::default();

        let x0 = parse_number("0");
        let x1 = parse_number("1");
        let x2 = parse_number("1_00");
        let x3 = parse_number("10_000");

        let x0: Comptime = Conv::conv(&mut context, &x0).unwrap();
        let x1: Comptime = Conv::conv(&mut context, &x1).unwrap();
        let x2: Comptime = Conv::conv(&mut context, &x2).unwrap();
        let x3: Comptime = Conv::conv(&mut context, &x3).unwrap();

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "32'sh00000000");
        assert_eq!(format!("{x1:x}"), "32'sh00000001");
        assert_eq!(format!("{x2:x}"), "32'sh00000064");
        assert_eq!(format!("{x3:x}"), "32'sh00002710");
    }

    #[test]
    fn based() {
        let mut context = Context::default();

        let x0 = parse_number("16'b000011110101");
        let x1 = parse_number("16'b0x0X11z10Z01");
        let x2 = parse_number("24'o20701231");
        let x3 = parse_number("24'o11z173x1");
        let x4 = parse_number("32'd123456789");
        let x5 = parse_number("32'd987654321");
        let x6 = parse_number("32'h12a45f78");
        let x7 = parse_number("32'hfx7Z5X32");

        let x0: Comptime = Conv::conv(&mut context, &x0).unwrap();
        let x1: Comptime = Conv::conv(&mut context, &x1).unwrap();
        let x2: Comptime = Conv::conv(&mut context, &x2).unwrap();
        let x3: Comptime = Conv::conv(&mut context, &x3).unwrap();
        let x4: Comptime = Conv::conv(&mut context, &x4).unwrap();
        let x5: Comptime = Conv::conv(&mut context, &x5).unwrap();
        let x6: Comptime = Conv::conv(&mut context, &x6).unwrap();
        let x7: Comptime = Conv::conv(&mut context, &x7).unwrap();

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();
        let x4 = x4.get_value().unwrap();
        let x5 = x5.get_value().unwrap();
        let x6 = x6.get_value().unwrap();
        let x7 = x7.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "16'h00f5");
        assert_eq!(format!("{x1:x}"), "16'h0XZZ");
        assert_eq!(format!("{x2:x}"), "24'h438299");
        assert_eq!(format!("{x3:x}"), "24'h2ZZeXX");
        assert_eq!(format!("{x4:x}"), "32'h075bcd15");
        assert_eq!(format!("{x5:x}"), "32'h3ade68b1");
        assert_eq!(format!("{x6:x}"), "32'h12a45f78");
        assert_eq!(format!("{x7:x}"), "32'hfx7z5x32");
    }

    #[test]
    fn widthless_based() {
        let mut context = Context::default();

        let x0 = parse_number("'b000011110101");
        let x1 = parse_number("'b0x0X11z10Z01");
        let x2 = parse_number("'o20701231");
        let x3 = parse_number("'o11z173x1");
        let x4 = parse_number("'d123456789");
        let x5 = parse_number("'d987654321");
        let x6 = parse_number("'h12a45f78");
        let x7 = parse_number("'hfx7Z5X32");

        let x0: Comptime = Conv::conv(&mut context, &x0).unwrap();
        let x1: Comptime = Conv::conv(&mut context, &x1).unwrap();
        let x2: Comptime = Conv::conv(&mut context, &x2).unwrap();
        let x3: Comptime = Conv::conv(&mut context, &x3).unwrap();
        let x4: Comptime = Conv::conv(&mut context, &x4).unwrap();
        let x5: Comptime = Conv::conv(&mut context, &x5).unwrap();
        let x6: Comptime = Conv::conv(&mut context, &x6).unwrap();
        let x7: Comptime = Conv::conv(&mut context, &x7).unwrap();

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();
        let x4 = x4.get_value().unwrap();
        let x5 = x5.get_value().unwrap();
        let x6 = x6.get_value().unwrap();
        let x7 = x7.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "8'hf5");
        assert_eq!(format!("{x1:x}"), "11'hXZZ");
        assert_eq!(format!("{x2:x}"), "23'h438299");
        assert_eq!(format!("{x3:x}"), "22'h2ZZeXX");
        assert_eq!(format!("{x4:x}"), "27'h75bcd15");
        assert_eq!(format!("{x5:x}"), "30'h3ade68b1");
        assert_eq!(format!("{x6:x}"), "29'h12a45f78");
        assert_eq!(format!("{x7:x}"), "32'hfx7z5x32");
    }

    #[test]
    fn all_bit() {
        let mut context = Context::default();

        let x0 = parse_number("'0");
        let x1 = parse_number("'1");
        let x2 = parse_number("'x");
        let x3 = parse_number("'X");
        let x4 = parse_number("'z");
        let x5 = parse_number("'Z");

        let x0: Comptime = Conv::conv(&mut context, &x0).unwrap();
        let x1: Comptime = Conv::conv(&mut context, &x1).unwrap();
        let x2: Comptime = Conv::conv(&mut context, &x2).unwrap();
        let x3: Comptime = Conv::conv(&mut context, &x3).unwrap();
        let x4: Comptime = Conv::conv(&mut context, &x4).unwrap();
        let x5: Comptime = Conv::conv(&mut context, &x5).unwrap();

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();
        let x4 = x4.get_value().unwrap();
        let x5 = x5.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "'0");
        assert_eq!(format!("{x1:x}"), "'1");
        assert_eq!(format!("{x2:x}"), "'x");
        assert_eq!(format!("{x3:x}"), "'x");
        assert_eq!(format!("{x4:x}"), "'z");
        assert_eq!(format!("{x5:x}"), "'z");
    }

    #[test]
    fn float() {
        let mut context = Context::default();

        let x0 = parse_number("0123456789.0123456789");
        let x1 = parse_number("0123456789.0123456789e+012");
        let x2 = parse_number("0123456789.0123456789e-012");
        let x3 = parse_number("0123456789.0123456789E+012");
        let x4 = parse_number("0123456789.0123456789E-012");

        let x0: Comptime = Conv::conv(&mut context, &x0).unwrap();
        let x1: Comptime = Conv::conv(&mut context, &x1).unwrap();
        let x2: Comptime = Conv::conv(&mut context, &x2).unwrap();
        let x3: Comptime = Conv::conv(&mut context, &x3).unwrap();
        let x4: Comptime = Conv::conv(&mut context, &x4).unwrap();

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();
        let x4 = x4.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "64'h419d6f34540ca458");
        assert_eq!(format!("{x1:x}"), "64'h441ac53a7e04bcda");
        assert_eq!(format!("{x2:x}"), "64'h3f202e85be180b74");
        assert_eq!(format!("{x3:x}"), "64'h441ac53a7e04bcda");
        assert_eq!(format!("{x4:x}"), "64'h3f202e85be180b74");
    }

    #[test]
    fn unary() {
        let mut context = Context::default();

        let x0 = parse_expression("+1");
        let x1 = parse_expression("-1");
        let x2 = parse_expression("!1");
        let x3 = parse_expression("~1");
        let x4 = parse_expression("&1");
        let x5 = parse_expression("|1");
        let x6 = parse_expression("^1");
        let x7 = parse_expression("~&1");
        let x8 = parse_expression("~|1");
        let x9 = parse_expression("~^1");

        let x0: ir::Expression = Conv::conv(&mut context, &x0).unwrap();
        let x1: ir::Expression = Conv::conv(&mut context, &x1).unwrap();
        let x2: ir::Expression = Conv::conv(&mut context, &x2).unwrap();
        let x3: ir::Expression = Conv::conv(&mut context, &x3).unwrap();
        let x4: ir::Expression = Conv::conv(&mut context, &x4).unwrap();
        let x5: ir::Expression = Conv::conv(&mut context, &x5).unwrap();
        let x6: ir::Expression = Conv::conv(&mut context, &x6).unwrap();
        let x7: ir::Expression = Conv::conv(&mut context, &x7).unwrap();
        let x8: ir::Expression = Conv::conv(&mut context, &x8).unwrap();
        let x9: ir::Expression = Conv::conv(&mut context, &x9).unwrap();

        assert_eq!(format!("{x0}"), "(+ 32'sh00000001)");
        assert_eq!(format!("{x1}"), "(- 32'sh00000001)");
        assert_eq!(format!("{x2}"), "(! 32'sh00000001)");
        assert_eq!(format!("{x3}"), "(~ 32'sh00000001)");
        assert_eq!(format!("{x4}"), "(& 32'sh00000001)");
        assert_eq!(format!("{x5}"), "(| 32'sh00000001)");
        assert_eq!(format!("{x6}"), "(^ 32'sh00000001)");
        assert_eq!(format!("{x7}"), "(~& 32'sh00000001)");
        assert_eq!(format!("{x8}"), "(~| 32'sh00000001)");
        assert_eq!(format!("{x9}"), "(~^ 32'sh00000001)");
    }

    #[test]
    fn binary() {
        let mut context = Context::default();

        let x00 = parse_expression("1 ** 1");
        let x01 = parse_expression("1 * 1");
        let x02 = parse_expression("1 / 1");
        let x03 = parse_expression("1 % 1");
        let x04 = parse_expression("1 + 1");
        let x05 = parse_expression("1 - 1");
        let x06 = parse_expression("1 << 1");
        let x07 = parse_expression("1 >> 1");
        let x08 = parse_expression("1 <<< 1");
        let x09 = parse_expression("1 >>> 1");
        let x10 = parse_expression("1 <: 1");
        let x11 = parse_expression("1 <= 1");
        let x12 = parse_expression("1 >: 1");
        let x13 = parse_expression("1 >= 1");
        let x14 = parse_expression("1 == 1");
        let x15 = parse_expression("1 != 1");
        let x16 = parse_expression("1 ==? 1");
        let x17 = parse_expression("1 !=? 1");
        let x18 = parse_expression("1 & 1");
        let x19 = parse_expression("1 ^ 1");
        let x20 = parse_expression("1 ~^ 1");
        let x21 = parse_expression("1 | 1");
        let x22 = parse_expression("1 && 1");
        let x23 = parse_expression("1 || 1");
        let x24 = parse_expression("1 ** 1 + 1 - 1 / 1 % 1");

        let x00: ir::Expression = Conv::conv(&mut context, &x00).unwrap();
        let x01: ir::Expression = Conv::conv(&mut context, &x01).unwrap();
        let x02: ir::Expression = Conv::conv(&mut context, &x02).unwrap();
        let x03: ir::Expression = Conv::conv(&mut context, &x03).unwrap();
        let x04: ir::Expression = Conv::conv(&mut context, &x04).unwrap();
        let x05: ir::Expression = Conv::conv(&mut context, &x05).unwrap();
        let x06: ir::Expression = Conv::conv(&mut context, &x06).unwrap();
        let x07: ir::Expression = Conv::conv(&mut context, &x07).unwrap();
        let x08: ir::Expression = Conv::conv(&mut context, &x08).unwrap();
        let x09: ir::Expression = Conv::conv(&mut context, &x09).unwrap();
        let x10: ir::Expression = Conv::conv(&mut context, &x10).unwrap();
        let x11: ir::Expression = Conv::conv(&mut context, &x11).unwrap();
        let x12: ir::Expression = Conv::conv(&mut context, &x12).unwrap();
        let x13: ir::Expression = Conv::conv(&mut context, &x13).unwrap();
        let x14: ir::Expression = Conv::conv(&mut context, &x14).unwrap();
        let x15: ir::Expression = Conv::conv(&mut context, &x15).unwrap();
        let x16: ir::Expression = Conv::conv(&mut context, &x16).unwrap();
        let x17: ir::Expression = Conv::conv(&mut context, &x17).unwrap();
        let x18: ir::Expression = Conv::conv(&mut context, &x18).unwrap();
        let x19: ir::Expression = Conv::conv(&mut context, &x19).unwrap();
        let x20: ir::Expression = Conv::conv(&mut context, &x20).unwrap();
        let x21: ir::Expression = Conv::conv(&mut context, &x21).unwrap();
        let x22: ir::Expression = Conv::conv(&mut context, &x22).unwrap();
        let x23: ir::Expression = Conv::conv(&mut context, &x23).unwrap();
        let x24: ir::Expression = Conv::conv(&mut context, &x24).unwrap();

        assert_eq!(format!("{x00}"), "(32'sh00000001 ** 32'sh00000001)");
        assert_eq!(format!("{x01}"), "(32'sh00000001 * 32'sh00000001)");
        assert_eq!(format!("{x02}"), "(32'sh00000001 / 32'sh00000001)");
        assert_eq!(format!("{x03}"), "(32'sh00000001 % 32'sh00000001)");
        assert_eq!(format!("{x04}"), "(32'sh00000001 + 32'sh00000001)");
        assert_eq!(format!("{x05}"), "(32'sh00000001 - 32'sh00000001)");
        assert_eq!(format!("{x06}"), "(32'sh00000001 << 32'sh00000001)");
        assert_eq!(format!("{x07}"), "(32'sh00000001 >> 32'sh00000001)");
        assert_eq!(format!("{x08}"), "(32'sh00000001 <<< 32'sh00000001)");
        assert_eq!(format!("{x09}"), "(32'sh00000001 >>> 32'sh00000001)");
        assert_eq!(format!("{x10}"), "(32'sh00000001 <: 32'sh00000001)");
        assert_eq!(format!("{x11}"), "(32'sh00000001 <= 32'sh00000001)");
        assert_eq!(format!("{x12}"), "(32'sh00000001 >: 32'sh00000001)");
        assert_eq!(format!("{x13}"), "(32'sh00000001 >= 32'sh00000001)");
        assert_eq!(format!("{x14}"), "(32'sh00000001 == 32'sh00000001)");
        assert_eq!(format!("{x15}"), "(32'sh00000001 != 32'sh00000001)");
        assert_eq!(format!("{x16}"), "(32'sh00000001 ==? 32'sh00000001)");
        assert_eq!(format!("{x17}"), "(32'sh00000001 !=? 32'sh00000001)");
        assert_eq!(format!("{x18}"), "(32'sh00000001 & 32'sh00000001)");
        assert_eq!(format!("{x19}"), "(32'sh00000001 ^ 32'sh00000001)");
        assert_eq!(format!("{x20}"), "(32'sh00000001 ~^ 32'sh00000001)");
        assert_eq!(format!("{x21}"), "(32'sh00000001 | 32'sh00000001)");
        assert_eq!(format!("{x22}"), "(32'sh00000001 && 32'sh00000001)");
        assert_eq!(format!("{x23}"), "(32'sh00000001 || 32'sh00000001)");
        assert_eq!(
            format!("{x24}"),
            "(((32'sh00000001 ** 32'sh00000001) + 32'sh00000001) - ((32'sh00000001 / 32'sh00000001) % 32'sh00000001))"
        );
    }

    #[test]
    fn ternary() {
        let mut context = Context::default();

        let x0 = parse_expression("if 1 ? 2 : 3");
        let x1 = parse_expression("if 1 ? 2 : if 3 ? 4 : 5");

        let x0: ir::Expression = Conv::conv(&mut context, &x0).unwrap();
        let x1: ir::Expression = Conv::conv(&mut context, &x1).unwrap();

        assert_eq!(
            format!("{x0}"),
            "(32'sh00000001 ? 32'sh00000002 : 32'sh00000003)"
        );
        assert_eq!(
            format!("{x1}"),
            "(32'sh00000001 ? 32'sh00000002 : (32'sh00000003 ? 32'sh00000004 : 32'sh00000005))"
        );
    }

    #[test]
    fn boolean() {
        let mut context = Context::default();

        let x0 = parse_expression("true");
        let x1 = parse_expression("false");

        let x0: ir::Expression = Conv::conv(&mut context, &x0).unwrap();
        let x1: ir::Expression = Conv::conv(&mut context, &x1).unwrap();

        assert_eq!(format!("{x0}"), "1'h1");
        assert_eq!(format!("{x1}"), "1'h0");
    }

    #[test]
    fn paren() {
        let mut context = Context::default();

        let x0 = parse_expression("(1 + 2) * 3");
        let x1 = parse_expression("1 + (2 * 3)");

        let x0: ir::Expression = Conv::conv(&mut context, &x0).unwrap();
        let x1: ir::Expression = Conv::conv(&mut context, &x1).unwrap();

        assert_eq!(
            format!("{x0}"),
            "((32'sh00000001 + 32'sh00000002) * 32'sh00000003)"
        );
        assert_eq!(
            format!("{x1}"),
            "(32'sh00000001 + (32'sh00000002 * 32'sh00000003))"
        );
    }

    #[test]
    fn concatenation() {
        let mut context = Context::default();

        let x0 = parse_expression("{1, 2, 3}");
        let x1 = parse_expression("{1 repeat 2, 2, 3 repeat 4}");

        let x0: ir::Expression = Conv::conv(&mut context, &x0).unwrap();
        let x1: ir::Expression = Conv::conv(&mut context, &x1).unwrap();

        assert_eq!(
            format!("{x0}"),
            "{32'sh00000001, 32'sh00000002, 32'sh00000003}"
        );
        assert_eq!(
            format!("{x1}"),
            "{32'sh00000001 repeat 32'sh00000002, 32'sh00000002, 32'sh00000003 repeat 32'sh00000004}"
        );
    }

    #[test]
    fn case_expression() {
        let mut context = Context::default();

        let x0 = parse_expression("case 10 {0: 1, 1: 2, default: 3}");
        let x1 = parse_expression("case 10 {0..=2: 1, 4..5: 2, default: 3}");

        let x0: ir::Expression = Conv::conv(&mut context, &x0).unwrap();
        let x1: ir::Expression = Conv::conv(&mut context, &x1).unwrap();

        assert_eq!(
            format!("{x0}"),
            "((32'sh0000000a ==? 32'sh00000000) ? 32'sh00000001 : ((32'sh0000000a ==? 32'sh00000001) ? 32'sh00000002 : 32'sh00000003))"
        );
        assert_eq!(
            format!("{x1}"),
            "(((32'sh00000000 <= 32'sh0000000a) && (32'sh0000000a <= 32'sh00000002)) ? 32'sh00000001 : (((32'sh00000004 <= 32'sh0000000a) && (32'sh0000000a <: 32'sh00000005)) ? 32'sh00000002 : 32'sh00000003))"
        );
    }

    #[test]
    fn switch_expression() {
        let mut context = Context::default();

        let x0 = parse_expression("switch {0 == 1: 2, 1 <: 2: 2, default: 3}");

        let x0: ir::Expression = Conv::conv(&mut context, &x0).unwrap();

        assert_eq!(
            format!("{x0}"),
            "((32'sh00000000 == 32'sh00000001) ? 32'sh00000002 : ((32'sh00000001 <: 32'sh00000002) ? 32'sh00000002 : 32'sh00000003))"
        );
    }
}
