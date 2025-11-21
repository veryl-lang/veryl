use crate::analyzer_error::AnalyzerError;
use crate::conv::checker::function::check_function_call_factor;
use crate::conv::utils::{case_condition, eval_expr, range_list, switch_condition};
use crate::conv::{Context, Conv};
use crate::ir::bigint::gen_mask;
use crate::ir::{
    self, Op, Type, TypeKind, TypedValue, UserDefined, Value, ValueVariant, VarPathIndex,
};
use crate::symbol::{GenericBoundKind, ProtoBound, SymbolKind};
use crate::symbol_path::SymbolPathNamespace;
use crate::symbol_table;
use num_bigint::BigUint;
use num_traits::Num;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&Expression> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression) -> Self {
        Conv::conv(context, value.if_expression.as_ref())
    }
}

fn is_if_expression(value: &Expression) -> bool {
    !value.if_expression.if_expression_list.is_empty()
}

impl Conv<&IfExpression> for ir::Expression {
    fn conv(context: &mut Context, value: &IfExpression) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression01.as_ref());
        for x in value.if_expression_list.iter().rev() {
            let y: ir::Expression = Conv::conv(context, x.expression.as_ref());
            let z: ir::Expression = Conv::conv(context, x.expression0.as_ref());

            if is_if_expression(&x.expression) {
                let range: TokenRange = x.expression.as_ref().into();
                context.insert_error(AnalyzerError::unenclosed_inner_if_expression(&range));
            }
            if is_if_expression(&x.expression0) {
                let range: TokenRange = x.expression0.as_ref().into();
                context.insert_error(AnalyzerError::unenclosed_inner_if_expression(&range));
            }

            ret = ir::Expression::Ternary(Box::new(y), Box::new(z), Box::new(ret));
        }
        ret
    }
}

impl Conv<&Expression01> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression01) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression02.as_ref());
        for x in &value.expression01_list {
            let right: ir::Expression = Conv::conv(context, x.expression02.as_ref());
            ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression02> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression02) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression03.as_ref());
        for x in &value.expression02_list {
            let right: ir::Expression = Conv::conv(context, x.expression03.as_ref());
            ret = ir::Expression::Binary(Box::new(ret), Op::LogicAnd, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression03> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression03) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression04.as_ref());
        for x in &value.expression03_list {
            let right: ir::Expression = Conv::conv(context, x.expression04.as_ref());
            ret = ir::Expression::Binary(Box::new(ret), Op::BitOr, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression04> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression04) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression05.as_ref());
        for x in &value.expression04_list {
            let right: ir::Expression = Conv::conv(context, x.expression05.as_ref());
            let op = x.operator05.operator05_token.to_string();
            let op = match op.as_str() {
                "^" => Op::BitXor,
                "~^" => Op::BitXnor,
                _ => unreachable!(),
            };
            ret = ir::Expression::Binary(Box::new(ret), op, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression05> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression05) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression06.as_ref());
        for x in &value.expression05_list {
            let right: ir::Expression = Conv::conv(context, x.expression06.as_ref());
            ret = ir::Expression::Binary(Box::new(ret), Op::BitAnd, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression06> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression06) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression07.as_ref());
        for x in &value.expression06_list {
            let right: ir::Expression = Conv::conv(context, x.expression07.as_ref());
            let op = x.operator07.operator07_token.to_string();
            let op = match op.as_str() {
                "==" => Op::Eq,
                "!=" => Op::Ne,
                "==?" => Op::EqWildcard,
                "!=?" => Op::NeWildcard,
                _ => unreachable!(),
            };
            ret = ir::Expression::Binary(Box::new(ret), op, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression07> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression07) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression08.as_ref());
        for x in &value.expression07_list {
            let right: ir::Expression = Conv::conv(context, x.expression08.as_ref());
            let op = x.operator08.operator08_token.to_string();
            let op = match op.as_str() {
                "<=" => Op::LessEq,
                ">=" => Op::GreaterEq,
                "<:" => Op::Less,
                ">:" => Op::Greater,
                _ => unreachable!(),
            };
            ret = ir::Expression::Binary(Box::new(ret), op, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression08> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression08) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression09.as_ref());
        for x in &value.expression08_list {
            let right: ir::Expression = Conv::conv(context, x.expression09.as_ref());
            let op = x.operator09.operator09_token.to_string();
            let op = match op.as_str() {
                "<<<" => Op::ArithShiftL,
                ">>>" => Op::ArithShiftR,
                "<<" => Op::LogicShiftL,
                ">>" => Op::LogicShiftR,
                _ => unreachable!(),
            };
            ret = ir::Expression::Binary(Box::new(ret), op, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression09> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression09) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression10.as_ref());
        for x in &value.expression09_list {
            let right: ir::Expression = Conv::conv(context, x.expression10.as_ref());
            let op = x.operator10.operator10_token.to_string();
            ret = match op.as_str() {
                "+" => ir::Expression::Binary(Box::new(ret), Op::Add, Box::new(right)),
                "-" => {
                    let right = ir::Expression::Unary(Op::Sub, Box::new(right));
                    ir::Expression::Binary(Box::new(ret), Op::Add, Box::new(right))
                }
                _ => unreachable!(),
            };
        }
        ret
    }
}

impl Conv<&Expression10> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression10) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression11.as_ref());
        for x in &value.expression10_list {
            let right: ir::Expression = Conv::conv(context, x.expression11.as_ref());
            let op = match x.expression10_list_group.as_ref() {
                Expression10ListGroup::Operator11(x) => {
                    let op = x.operator11.operator11_token.to_string();
                    match op.as_str() {
                        "/" => Op::Div,
                        "%" => Op::Rem,
                        _ => unreachable!(),
                    }
                }
                Expression10ListGroup::Star(_) => Op::Mul,
            };
            ret = ir::Expression::Binary(Box::new(ret), op, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression11> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression11) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.expression12.as_ref());
        for x in &value.expression11_list {
            let right: ir::Expression = Conv::conv(context, x.expression12.as_ref());
            ret = ir::Expression::Binary(Box::new(ret), Op::Pow, Box::new(right));
        }
        ret
    }
}

impl Conv<&Expression12> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression12) -> Self {
        let ret = Conv::conv(context, value.expression13.as_ref());
        if let Some(x) = &value.expression12_opt {
            let right: ir::Factor = Conv::conv(context, x.casting_type.as_ref());
            ir::Expression::Binary(
                Box::new(ret),
                Op::As,
                Box::new(ir::Expression::Term(Box::new(right))),
            )
        } else {
            ret
        }
    }
}

impl Conv<&FactorType> for ir::Factor {
    fn conv(_context: &mut Context, value: &FactorType) -> Self {
        let range = value.into();
        let value = match value.factor_type_group.as_ref() {
            FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                // TODO width
                match x.variable_type.as_ref() {
                    VariableType::Clock(_) => Type::new(TypeKind::Clock, 1, false),
                    VariableType::ClockPosedge(_) => Type::new(TypeKind::ClockPosedge, 1, false),
                    VariableType::ClockNegedge(_) => Type::new(TypeKind::ClockNegedge, 1, false),
                    VariableType::Reset(_) => Type::new(TypeKind::Reset, 1, false),
                    VariableType::ResetAsyncHigh(_) => {
                        Type::new(TypeKind::ResetAsyncHigh, 1, false)
                    }
                    VariableType::ResetAsyncLow(_) => Type::new(TypeKind::ResetAsyncLow, 1, false),
                    VariableType::ResetSyncHigh(_) => Type::new(TypeKind::ResetSyncHigh, 1, false),
                    VariableType::ResetSyncLow(_) => Type::new(TypeKind::ResetSyncLow, 1, false),
                    VariableType::Logic(_) => Type::new(TypeKind::Logic, 1, false),
                    VariableType::Bit(_) => Type::new(TypeKind::Bit, 1, false),
                }
            }
            FactorTypeGroup::FixedType(x) => match x.fixed_type.as_ref() {
                FixedType::U8(_) => Type::new(TypeKind::Bit, 8, false),
                FixedType::U16(_) => Type::new(TypeKind::Bit, 16, false),
                FixedType::U32(_) => Type::new(TypeKind::Bit, 32, false),
                FixedType::U64(_) => Type::new(TypeKind::Bit, 64, false),
                FixedType::I8(_) => Type::new(TypeKind::Bit, 8, true),
                FixedType::I16(_) => Type::new(TypeKind::Bit, 16, true),
                FixedType::I32(_) => Type::new(TypeKind::Bit, 32, true),
                FixedType::I64(_) => Type::new(TypeKind::Bit, 64, true),
                FixedType::F32(_) => Type::new(TypeKind::Bit, 32, false),
                FixedType::F64(_) => Type::new(TypeKind::Bit, 64, false),
                FixedType::Bool(_) => Type::new(TypeKind::Bit, 1, false),
                FixedType::Strin(_) => Type::new(TypeKind::Unknown, 1, false),
            },
        };
        let r#type = Type {
            kind: TypeKind::Type,
            signed: false,
            width: vec![],
            array: vec![],
        };
        let ret = TypedValue {
            value: ValueVariant::Type(value),
            r#type,
            is_const: true,
        };
        ir::Factor::Value(ret, range)
    }
}

impl Conv<&CastingType> for ir::Factor {
    fn conv(context: &mut Context, value: &CastingType) -> Self {
        let range = value.into();
        let value = match value {
            CastingType::U8(_) => Type::new(TypeKind::Bit, 8, false),
            CastingType::U16(_) => Type::new(TypeKind::Bit, 16, false),
            CastingType::U32(_) => Type::new(TypeKind::Bit, 32, false),
            CastingType::U64(_) => Type::new(TypeKind::Bit, 64, false),
            CastingType::I8(_) => Type::new(TypeKind::Bit, 8, true),
            CastingType::I16(_) => Type::new(TypeKind::Bit, 16, true),
            CastingType::I32(_) => Type::new(TypeKind::Bit, 32, true),
            CastingType::I64(_) => Type::new(TypeKind::Bit, 64, true),
            CastingType::F32(_) => Type::new(TypeKind::Bit, 32, false),
            CastingType::F64(_) => Type::new(TypeKind::Bit, 64, false),
            CastingType::Bool(_) => Type::new(TypeKind::Bit, 1, false),
            CastingType::Clock(_) => Type::new(TypeKind::Clock, 1, false),
            CastingType::ClockPosedge(_) => Type::new(TypeKind::ClockPosedge, 1, false),
            CastingType::ClockNegedge(_) => Type::new(TypeKind::ClockNegedge, 1, false),
            CastingType::Reset(_) => Type::new(TypeKind::Reset, 1, false),
            CastingType::ResetAsyncHigh(_) => Type::new(TypeKind::ResetAsyncHigh, 1, false),
            CastingType::ResetAsyncLow(_) => Type::new(TypeKind::ResetAsyncLow, 1, false),
            CastingType::ResetSyncHigh(_) => Type::new(TypeKind::ResetSyncHigh, 1, false),
            CastingType::ResetSyncLow(_) => Type::new(TypeKind::ResetSyncLow, 1, false),
            CastingType::UserDefinedType(x) => Type {
                kind: TypeKind::UserDefined(UserDefined::Identifier(
                    x.user_defined_type.scoped_identifier.as_ref().clone(),
                )),
                signed: false,
                width: vec![],
                array: vec![],
            },
            CastingType::Based(x) => {
                let value: TypedValue = Conv::conv(context, x.based.as_ref());
                let value = value.get_value().unwrap().to_usize();
                Type::new(TypeKind::Logic, value, false)
            }
            CastingType::BaseLess(x) => {
                let value: TypedValue = Conv::conv(context, x.base_less.as_ref());
                let value = value.get_value().unwrap().to_usize();
                Type::new(TypeKind::Logic, value, false)
            }
        };
        let r#type = Type {
            kind: TypeKind::Type,
            signed: false,
            width: vec![],
            array: vec![],
        };
        let ret = TypedValue {
            value: ValueVariant::Type(value),
            r#type,
            is_const: true,
        };
        ir::Factor::Value(ret, range)
    }
}

impl Conv<&Expression13> for ir::Expression {
    fn conv(context: &mut Context, value: &Expression13) -> Self {
        let mut ret: ir::Expression = Conv::conv(context, value.factor.as_ref());
        for x in value.expression13_list.iter().rev() {
            let op = match x.expression13_list_group.as_ref() {
                Expression13ListGroup::UnaryOperator(x) => {
                    let token = x.unary_operator.unary_operator_token.to_string();
                    match token.as_str() {
                        "~&" => Op::BitNand,
                        "~|" => Op::BitNor,
                        "~" => Op::BitNot,
                        "!" => Op::LogicNot,
                        _ => unreachable!(),
                    }
                }
                Expression13ListGroup::Operator04(_) => Op::BitOr,
                Expression13ListGroup::Operator05(x) => {
                    let token = x.operator05.operator05_token.to_string();
                    match token.as_str() {
                        "^" => Op::BitXor,
                        "~^" => Op::BitXnor,
                        _ => unreachable!(),
                    }
                }
                Expression13ListGroup::Operator06(_) => Op::BitAnd,
                Expression13ListGroup::Operator10(x) => {
                    let token = x.operator10.operator10_token.to_string();
                    match token.as_str() {
                        "+" => Op::Add,
                        "-" => Op::Sub,
                        _ => unreachable!(),
                    }
                }
            };

            ret = ir::Expression::Unary(op, Box::new(ret));
        }
        ret
    }
}

impl Conv<&Factor> for ir::Expression {
    fn conv(context: &mut Context, value: &Factor) -> Self {
        let range = value.into();
        match value {
            Factor::Number(x) => {
                let x: TypedValue = Conv::conv(context, x.number.as_ref());
                ir::Expression::Term(Box::new(ir::Factor::Value(x, range)))
            }
            Factor::BooleanLiteral(x) => {
                let x = match x.boolean_literal.as_ref() {
                    BooleanLiteral::True(_) => 1u32,
                    BooleanLiteral::False(_) => 0u32,
                };
                let value = Value::new(BigUint::from(x), 1, false);
                let r#type = Type::new(TypeKind::Bit, 1, false);
                let ret = TypedValue {
                    value: ValueVariant::Numeric(value),
                    r#type,
                    is_const: true,
                };
                ir::Expression::Term(Box::new(ir::Factor::Value(ret, range)))
            }
            Factor::IdentifierFactor(x) => Conv::conv(context, x.identifier_factor.as_ref()),
            Factor::LParenExpressionRParen(x) => Conv::conv(context, x.expression.as_ref()),
            Factor::LBraceConcatenationListRBrace(x) => {
                let x = x.concatenation_list.as_ref();
                let exp: ir::Expression =
                    Conv::conv(context, x.concatenation_item.expression.as_ref());
                let rep: Option<ir::Expression> = x
                    .concatenation_item
                    .concatenation_item_opt
                    .as_ref()
                    .map(|x| Conv::conv(context, x.expression.as_ref()));
                let mut ret = vec![(exp, rep)];

                for x in &x.concatenation_list_list {
                    let exp: ir::Expression =
                        Conv::conv(context, x.concatenation_item.expression.as_ref());
                    let rep: Option<ir::Expression> = x
                        .concatenation_item
                        .concatenation_item_opt
                        .as_ref()
                        .map(|x| Conv::conv(context, x.expression.as_ref()));
                    ret.push((exp, rep));
                }

                ir::Expression::Concatenation(ret)
            }
            Factor::QuoteLBraceArrayLiteralListRBrace(_) => {
                ir::Expression::Term(Box::new(ir::Factor::Unknown(range)))
            }
            Factor::CaseExpression(x) => {
                let tgt: ir::Expression =
                    Conv::conv(context, x.case_expression.expression.as_ref());
                let exp: ir::Expression =
                    Conv::conv(context, x.case_expression.expression0.as_ref());
                let defaul: ir::Expression =
                    Conv::conv(context, x.case_expression.expression1.as_ref());
                let cond = case_condition(context, &tgt, x.case_expression.case_condition.as_ref());

                let mut ret =
                    ir::Expression::Ternary(Box::new(cond), Box::new(exp), Box::new(defaul));

                for x in &x.case_expression.case_expression_list {
                    let cond = case_condition(context, &tgt, x.case_condition.as_ref());
                    let exp: ir::Expression = Conv::conv(context, x.expression.as_ref());

                    if let ir::Expression::Ternary(x, y, z) = ret {
                        let arm = ir::Expression::Ternary(Box::new(cond), Box::new(exp), z);
                        ret = ir::Expression::Ternary(x, y, Box::new(arm));
                    } else {
                        unreachable!()
                    }
                }
                ret
            }
            Factor::SwitchExpression(x) => {
                let exp: ir::Expression =
                    Conv::conv(context, x.switch_expression.expression.as_ref());
                let defaul: ir::Expression =
                    Conv::conv(context, x.switch_expression.expression0.as_ref());
                let cond = switch_condition(context, x.switch_expression.switch_condition.as_ref());

                let mut ret =
                    ir::Expression::Ternary(Box::new(cond), Box::new(exp), Box::new(defaul));

                for x in &x.switch_expression.switch_expression_list {
                    let cond = switch_condition(context, x.switch_condition.as_ref());
                    let exp: ir::Expression = Conv::conv(context, x.expression.as_ref());

                    if let ir::Expression::Ternary(x, y, z) = ret {
                        let arm = ir::Expression::Ternary(Box::new(cond), Box::new(exp), z);
                        ret = ir::Expression::Ternary(x, y, Box::new(arm));
                    } else {
                        unreachable!()
                    }
                }
                ret
            }
            Factor::StringLiteral(_) => ir::Expression::Term(Box::new(ir::Factor::Unknown(range))),
            Factor::FactorGroup(_) => ir::Expression::Term(Box::new(ir::Factor::Unknown(range))),
            Factor::InsideExpression(x) => {
                let exp: ir::Expression =
                    Conv::conv(context, x.inside_expression.expression.as_ref());
                range_list(context, &exp, x.inside_expression.range_list.as_ref())
            }
            Factor::OutsideExpression(x) => {
                let exp: ir::Expression =
                    Conv::conv(context, x.outside_expression.expression.as_ref());
                let ret = range_list(context, &exp, x.outside_expression.range_list.as_ref());
                ir::Expression::Unary(Op::LogicNot, Box::new(ret))
            }
            Factor::TypeExpression(x) => {
                let (value, _) = eval_expr(context, None, &x.type_expression.expression);
                let r#type = Type {
                    kind: TypeKind::Type,
                    signed: false,
                    width: vec![],
                    array: vec![],
                };
                let ret = TypedValue {
                    value: ValueVariant::Type(value.r#type),
                    r#type,
                    is_const: true,
                };
                ir::Expression::Term(Box::new(ir::Factor::Value(ret, range)))
            }
            Factor::FactorTypeFactor(x) => {
                let ret = Conv::conv(context, x.factor_type_factor.factor_type.as_ref());
                ir::Expression::Term(Box::new(ret))
            }
        }
    }
}

impl Conv<&IdentifierFactor> for ir::Expression {
    fn conv(context: &mut Context, value: &IdentifierFactor) -> Self {
        check_function_call_factor(context, value);

        let range: TokenRange = value.into();
        if value.identifier_factor_opt.is_some() {
            // TODO function call
            ir::Expression::Term(Box::new(ir::Factor::Unknown(range)))
        } else {
            let x = value.expression_identifier.as_ref();
            let path: VarPathIndex = Conv::conv(context, x);
            let (path, index) = path.into();

            let factor = if let Some((var_id, typed_value)) = context.find_path(&path) {
                let (index, select) = index.split(typed_value.r#type.array.len());
                ir::Factor::Variable(var_id, index, typed_value.clone(), select, range)
            } else {
                let path: SymbolPathNamespace = x.into();
                if let Ok(symbol) = symbol_table::resolve(path) {
                    match &symbol.found.kind {
                        SymbolKind::Parameter(x) => {
                            if let Ok(r#type) = x.r#type.to_ir_type(context)
                                && let Some(expr) = &x.value
                            {
                                let (x, _) = eval_expr(context, Some(r#type), expr);
                                return ir::Expression::Term(Box::new(ir::Factor::Value(x, range)));
                            }
                        }
                        SymbolKind::GenericParameter(x) => {
                            if let Some(proto) =
                                x.bound.resolve_proto_bound(&symbol.found.namespace)
                            {
                                let r#type = match proto {
                                    ProtoBound::FactorType(x) => Some(x),
                                    ProtoBound::Enum((_, x)) => Some(x),
                                    ProtoBound::Struct((_, x)) => Some(x),
                                    ProtoBound::Union((_, x)) => Some(x),
                                    _ => None,
                                };
                                if let Some(r#type) = r#type
                                    && let Ok(r#type) = r#type.to_ir_type(context)
                                {
                                    let mut x = TypedValue::create_unknown();
                                    x.r#type = r#type;
                                    return ir::Expression::Term(Box::new(ir::Factor::Value(
                                        x, range,
                                    )));
                                }
                            } else if matches!(x.bound, GenericBoundKind::Type) {
                                let mut x = TypedValue::create_unknown();
                                x.r#type.kind = TypeKind::Type;
                                return ir::Expression::Term(Box::new(ir::Factor::Value(x, range)));
                            } else {
                                context.insert_error(AnalyzerError::invalid_factor(
                                    Some(&symbol.found.token.to_string()),
                                    &symbol.found.kind.to_kind_name(),
                                    &range,
                                    &[],
                                ));
                            }
                        }
                        SymbolKind::Function(_)
                        | SymbolKind::Module(_)
                        | SymbolKind::SystemFunction(_) => {
                            context.insert_error(AnalyzerError::invalid_factor(
                                Some(&symbol.found.token.to_string()),
                                &symbol.found.kind.to_kind_name(),
                                &range,
                                &[],
                            ));
                        }
                        _ => (),
                    }
                    ir::Factor::Unresolved(x.clone(), range)
                } else {
                    ir::Factor::Unresolved(x.clone(), range)
                }
            };
            ir::Expression::Term(Box::new(factor))
        }
    }
}

impl Conv<&Select> for ir::Select {
    fn conv(context: &mut Context, value: &Select) -> Self {
        let beg: ir::Expression = Conv::conv(context, value.expression.as_ref());
        let end = if let Some(x) = &value.select_opt {
            let end: ir::Expression = Conv::conv(context, x.expression.as_ref());
            Some(end)
        } else {
            None
        };
        ir::Select { beg, end }
    }
}

impl Conv<&Number> for TypedValue {
    fn conv(context: &mut Context, value: &Number) -> Self {
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

impl Conv<&Based> for TypedValue {
    fn conv(_context: &mut Context, value: &Based) -> Self {
        let x = value.based_token.to_string().replace('_', "");
        if let Some((width, rest)) = x.split_once('\'') {
            let signed = &rest[0..1] == "s";
            let rest = if signed { &rest[1..] } else { rest };
            let (base, value) = rest.split_at(1);
            let (radix, all1_char) = match base {
                "b" => (2, '1'),
                "o" => (8, '7'),
                "d" => (10, '0'),
                "h" => (16, 'f'),
                _ => unreachable!(),
            };

            let payload = value.replace(['x', 'X', 'z', 'Z'], "0");
            let mask_x: String = value
                .chars()
                .map(|x| if x == 'x' || x == 'X' { all1_char } else { '0' })
                .collect();
            let mask_z: String = value
                .chars()
                .map(|x| if x == 'z' || x == 'Z' { all1_char } else { '0' })
                .collect();

            let payload = BigUint::from_str_radix(&payload, radix).unwrap_or(BigUint::from(0u32));
            let mask_x = BigUint::from_str_radix(&mask_x, radix).unwrap_or(BigUint::from(0u32));
            let mask_z = BigUint::from_str_radix(&mask_z, radix).unwrap_or(BigUint::from(0u32));

            let kind = if mask_x.clone() != 0u32.into() || mask_z.clone() != 0u32.into() {
                TypeKind::Logic
            } else {
                TypeKind::Bit
            };

            let width = if let Ok(x) = str::parse::<usize>(width) {
                x
            } else {
                payload.bits().max(mask_x.bits()).max(mask_z.bits()) as usize
            };

            let value = Value {
                payload,
                mask_x,
                mask_z,
                width,
                signed,
            };

            let r#type = Type::new(kind, width, signed);

            TypedValue {
                value: ValueVariant::Numeric(value),
                r#type,
                is_const: true,
            }
        } else {
            unreachable!()
        }
    }
}

impl Conv<&BaseLess> for TypedValue {
    fn conv(_context: &mut Context, value: &BaseLess) -> Self {
        let x = value.base_less_token.to_string().replace('_', "");
        let x = str::parse::<BigUint>(&x).unwrap();
        let value = Value::new(x, 32, true);

        let r#type = Type::new(TypeKind::Bit, 32, true);

        TypedValue {
            value: ValueVariant::Numeric(value),
            r#type,
            is_const: true,
        }
    }
}

impl Conv<&AllBit> for TypedValue {
    fn conv(_context: &mut Context, value: &AllBit) -> Self {
        fn zero() -> BigUint {
            BigUint::from(0u32)
        }

        fn one() -> BigUint {
            BigUint::from(1u32)
        }

        let x = value.all_bit_token.to_string();
        let (payload, mask_x, mask_z, width) = if let Some((width, rest)) = x.split_once('\'') {
            if width.is_empty() {
                let width = 0;
                match rest {
                    "0" => (zero(), zero(), zero(), width),
                    "1" => (one(), zero(), zero(), width),
                    "x" | "X" => (zero(), one(), zero(), width),
                    "z" | "Z" => (zero(), zero(), one(), width),
                    _ => unreachable!(),
                }
            } else {
                let width = str::parse::<usize>(width).unwrap();
                let mask = gen_mask(width);
                match rest {
                    "0" => (zero(), zero(), zero(), width),
                    "1" => (mask, zero(), zero(), width),
                    "x" | "X" => (zero(), mask, zero(), width),
                    "z" | "Z" => (zero(), zero(), mask, width),
                    _ => unreachable!(),
                }
            }
        } else {
            unreachable!();
        };

        let kind = if mask_x.clone() != 0u32.into() || mask_z.clone() != 0u32.into() {
            TypeKind::Logic
        } else {
            TypeKind::Bit
        };

        let value = Value {
            payload,
            mask_x,
            mask_z,
            width,
            signed: false,
        };

        let r#type = Type::new(kind, 0, true);

        TypedValue {
            value: ValueVariant::Numeric(value),
            r#type,
            is_const: true,
        }
    }
}

impl Conv<&RealNumber> for TypedValue {
    fn conv(_context: &mut Context, value: &RealNumber) -> Self {
        match value {
            RealNumber::FixedPoint(x) => {
                let x = x.fixed_point.fixed_point_token.to_string();
                let (payload, mask_x) = if let Ok(value) = x.parse::<f64>() {
                    (BigUint::from(value.to_bits()), BigUint::from(0u32))
                } else {
                    (BigUint::from(0u32), gen_mask(64))
                };
                let mask_z = BigUint::from(0u32);
                let value = Value {
                    payload,
                    mask_x,
                    mask_z,
                    width: 64,
                    signed: false,
                };

                let r#type = Type::new(TypeKind::Bit, 64, false);

                TypedValue {
                    value: ValueVariant::Numeric(value),
                    r#type,
                    is_const: true,
                }
            }
            RealNumber::Exponent(x) => {
                let x = x.exponent.exponent_token.to_string();
                let (payload, mask_x) = if let Ok(value) = x.parse::<f64>() {
                    (BigUint::from(value.to_bits()), BigUint::from(0u32))
                } else {
                    (BigUint::from(0u32), gen_mask(64))
                };
                let mask_z = BigUint::from(0u32);
                let value = Value {
                    payload,
                    mask_x,
                    mask_z,
                    width: 64,
                    signed: false,
                };

                let r#type = Type::new(TypeKind::Bit, 64, false);

                TypedValue {
                    value: ValueVariant::Numeric(value),
                    r#type,
                    is_const: true,
                }
            }
        }
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

        let x0: TypedValue = Conv::conv(&mut context, &x0);
        let x1: TypedValue = Conv::conv(&mut context, &x1);
        let x2: TypedValue = Conv::conv(&mut context, &x2);
        let x3: TypedValue = Conv::conv(&mut context, &x3);

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "00000000");
        assert_eq!(format!("{x1:x}"), "00000001");
        assert_eq!(format!("{x2:x}"), "00000064");
        assert_eq!(format!("{x3:x}"), "00002710");
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

        let x0: TypedValue = Conv::conv(&mut context, &x0);
        let x1: TypedValue = Conv::conv(&mut context, &x1);
        let x2: TypedValue = Conv::conv(&mut context, &x2);
        let x3: TypedValue = Conv::conv(&mut context, &x3);
        let x4: TypedValue = Conv::conv(&mut context, &x4);
        let x5: TypedValue = Conv::conv(&mut context, &x5);
        let x6: TypedValue = Conv::conv(&mut context, &x6);
        let x7: TypedValue = Conv::conv(&mut context, &x7);

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();
        let x4 = x4.get_value().unwrap();
        let x5 = x5.get_value().unwrap();
        let x6 = x6.get_value().unwrap();
        let x7 = x7.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "00f5");
        assert_eq!(format!("{x1:x}"), "0xzz");
        assert_eq!(format!("{x2:x}"), "438299");
        assert_eq!(format!("{x3:x}"), "2zzexx");
        assert_eq!(format!("{x4:x}"), "075bcd15");
        assert_eq!(format!("{x5:x}"), "3ade68b1");
        assert_eq!(format!("{x6:x}"), "12a45f78");
        assert_eq!(format!("{x7:x}"), "fx7z5x32");
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

        let x0: TypedValue = Conv::conv(&mut context, &x0);
        let x1: TypedValue = Conv::conv(&mut context, &x1);
        let x2: TypedValue = Conv::conv(&mut context, &x2);
        let x3: TypedValue = Conv::conv(&mut context, &x3);
        let x4: TypedValue = Conv::conv(&mut context, &x4);
        let x5: TypedValue = Conv::conv(&mut context, &x5);
        let x6: TypedValue = Conv::conv(&mut context, &x6);
        let x7: TypedValue = Conv::conv(&mut context, &x7);

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();
        let x4 = x4.get_value().unwrap();
        let x5 = x5.get_value().unwrap();
        let x6 = x6.get_value().unwrap();
        let x7 = x7.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "f5");
        assert_eq!(format!("{x1:x}"), "xzz");
        assert_eq!(format!("{x2:x}"), "438299");
        assert_eq!(format!("{x3:x}"), "2zzexx");
        assert_eq!(format!("{x4:x}"), "75bcd15");
        assert_eq!(format!("{x5:x}"), "3ade68b1");
        assert_eq!(format!("{x6:x}"), "12a45f78");
        assert_eq!(format!("{x7:x}"), "fx7z5x32");
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

        let x0: TypedValue = Conv::conv(&mut context, &x0);
        let x1: TypedValue = Conv::conv(&mut context, &x1);
        let x2: TypedValue = Conv::conv(&mut context, &x2);
        let x3: TypedValue = Conv::conv(&mut context, &x3);
        let x4: TypedValue = Conv::conv(&mut context, &x4);
        let x5: TypedValue = Conv::conv(&mut context, &x5);

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();
        let x4 = x4.get_value().unwrap();
        let x5 = x5.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "all 0");
        assert_eq!(format!("{x1:x}"), "all 1");
        assert_eq!(format!("{x2:x}"), "all x");
        assert_eq!(format!("{x3:x}"), "all x");
        assert_eq!(format!("{x4:x}"), "all z");
        assert_eq!(format!("{x5:x}"), "all z");
    }

    #[test]
    fn float() {
        let mut context = Context::default();

        let x0 = parse_number("0123456789.0123456789");
        let x1 = parse_number("0123456789.0123456789e+012");
        let x2 = parse_number("0123456789.0123456789e-012");
        let x3 = parse_number("0123456789.0123456789E+012");
        let x4 = parse_number("0123456789.0123456789E-012");

        let x0: TypedValue = Conv::conv(&mut context, &x0);
        let x1: TypedValue = Conv::conv(&mut context, &x1);
        let x2: TypedValue = Conv::conv(&mut context, &x2);
        let x3: TypedValue = Conv::conv(&mut context, &x3);
        let x4: TypedValue = Conv::conv(&mut context, &x4);

        let x0 = x0.get_value().unwrap();
        let x1 = x1.get_value().unwrap();
        let x2 = x2.get_value().unwrap();
        let x3 = x3.get_value().unwrap();
        let x4 = x4.get_value().unwrap();

        assert_eq!(format!("{x0:x}"), "419d6f34540ca458");
        assert_eq!(format!("{x1:x}"), "441ac53a7e04bcda");
        assert_eq!(format!("{x2:x}"), "3f202e85be180b74");
        assert_eq!(format!("{x3:x}"), "441ac53a7e04bcda");
        assert_eq!(format!("{x4:x}"), "3f202e85be180b74");
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

        let x0: ir::Expression = Conv::conv(&mut context, &x0);
        let x1: ir::Expression = Conv::conv(&mut context, &x1);
        let x2: ir::Expression = Conv::conv(&mut context, &x2);
        let x3: ir::Expression = Conv::conv(&mut context, &x3);
        let x4: ir::Expression = Conv::conv(&mut context, &x4);
        let x5: ir::Expression = Conv::conv(&mut context, &x5);
        let x6: ir::Expression = Conv::conv(&mut context, &x6);
        let x7: ir::Expression = Conv::conv(&mut context, &x7);
        let x8: ir::Expression = Conv::conv(&mut context, &x8);
        let x9: ir::Expression = Conv::conv(&mut context, &x9);

        assert_eq!(format!("{x0}"), "(+ 00000001)");
        assert_eq!(format!("{x1}"), "(- 00000001)");
        assert_eq!(format!("{x2}"), "(! 00000001)");
        assert_eq!(format!("{x3}"), "(~ 00000001)");
        assert_eq!(format!("{x4}"), "(& 00000001)");
        assert_eq!(format!("{x5}"), "(| 00000001)");
        assert_eq!(format!("{x6}"), "(^ 00000001)");
        assert_eq!(format!("{x7}"), "(~& 00000001)");
        assert_eq!(format!("{x8}"), "(~| 00000001)");
        assert_eq!(format!("{x9}"), "(~^ 00000001)");
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

        let x00: ir::Expression = Conv::conv(&mut context, &x00);
        let x01: ir::Expression = Conv::conv(&mut context, &x01);
        let x02: ir::Expression = Conv::conv(&mut context, &x02);
        let x03: ir::Expression = Conv::conv(&mut context, &x03);
        let x04: ir::Expression = Conv::conv(&mut context, &x04);
        let x05: ir::Expression = Conv::conv(&mut context, &x05);
        let x06: ir::Expression = Conv::conv(&mut context, &x06);
        let x07: ir::Expression = Conv::conv(&mut context, &x07);
        let x08: ir::Expression = Conv::conv(&mut context, &x08);
        let x09: ir::Expression = Conv::conv(&mut context, &x09);
        let x10: ir::Expression = Conv::conv(&mut context, &x10);
        let x11: ir::Expression = Conv::conv(&mut context, &x11);
        let x12: ir::Expression = Conv::conv(&mut context, &x12);
        let x13: ir::Expression = Conv::conv(&mut context, &x13);
        let x14: ir::Expression = Conv::conv(&mut context, &x14);
        let x15: ir::Expression = Conv::conv(&mut context, &x15);
        let x16: ir::Expression = Conv::conv(&mut context, &x16);
        let x17: ir::Expression = Conv::conv(&mut context, &x17);
        let x18: ir::Expression = Conv::conv(&mut context, &x18);
        let x19: ir::Expression = Conv::conv(&mut context, &x19);
        let x20: ir::Expression = Conv::conv(&mut context, &x20);
        let x21: ir::Expression = Conv::conv(&mut context, &x21);
        let x22: ir::Expression = Conv::conv(&mut context, &x22);
        let x23: ir::Expression = Conv::conv(&mut context, &x23);
        let x24: ir::Expression = Conv::conv(&mut context, &x24);

        assert_eq!(format!("{x00}"), "(00000001 ** 00000001)");
        assert_eq!(format!("{x01}"), "(00000001 * 00000001)");
        assert_eq!(format!("{x02}"), "(00000001 / 00000001)");
        assert_eq!(format!("{x03}"), "(00000001 % 00000001)");
        assert_eq!(format!("{x04}"), "(00000001 + 00000001)");
        assert_eq!(format!("{x05}"), "(00000001 + (- 00000001))");
        assert_eq!(format!("{x06}"), "(00000001 << 00000001)");
        assert_eq!(format!("{x07}"), "(00000001 >> 00000001)");
        assert_eq!(format!("{x08}"), "(00000001 <<< 00000001)");
        assert_eq!(format!("{x09}"), "(00000001 >>> 00000001)");
        assert_eq!(format!("{x10}"), "(00000001 <: 00000001)");
        assert_eq!(format!("{x11}"), "(00000001 <= 00000001)");
        assert_eq!(format!("{x12}"), "(00000001 >: 00000001)");
        assert_eq!(format!("{x13}"), "(00000001 >= 00000001)");
        assert_eq!(format!("{x14}"), "(00000001 == 00000001)");
        assert_eq!(format!("{x15}"), "(00000001 != 00000001)");
        assert_eq!(format!("{x16}"), "(00000001 ==? 00000001)");
        assert_eq!(format!("{x17}"), "(00000001 !=? 00000001)");
        assert_eq!(format!("{x18}"), "(00000001 & 00000001)");
        assert_eq!(format!("{x19}"), "(00000001 ^ 00000001)");
        assert_eq!(format!("{x20}"), "(00000001 ~^ 00000001)");
        assert_eq!(format!("{x21}"), "(00000001 | 00000001)");
        assert_eq!(format!("{x22}"), "(00000001 && 00000001)");
        assert_eq!(format!("{x23}"), "(00000001 || 00000001)");
        assert_eq!(
            format!("{x24}"),
            "(((00000001 ** 00000001) + 00000001) + (- ((00000001 / 00000001) % 00000001)))"
        );
    }

    #[test]
    fn ternary() {
        let mut context = Context::default();

        let x0 = parse_expression("if 1 ? 2 : 3");
        let x1 = parse_expression("if 1 ? 2 : if 3 ? 4 : 5");

        let x0: ir::Expression = Conv::conv(&mut context, &x0);
        let x1: ir::Expression = Conv::conv(&mut context, &x1);

        assert_eq!(format!("{x0}"), "(00000001 ? 00000002 : 00000003)");
        assert_eq!(
            format!("{x1}"),
            "(00000001 ? 00000002 : (00000003 ? 00000004 : 00000005))"
        );
    }

    #[test]
    fn boolean() {
        let mut context = Context::default();

        let x0 = parse_expression("true");
        let x1 = parse_expression("false");

        let x0: ir::Expression = Conv::conv(&mut context, &x0);
        let x1: ir::Expression = Conv::conv(&mut context, &x1);

        assert_eq!(format!("{x0}"), "1");
        assert_eq!(format!("{x1}"), "0");
    }

    #[test]
    fn paren() {
        let mut context = Context::default();

        let x0 = parse_expression("(1 + 2) * 3");
        let x1 = parse_expression("1 + (2 * 3)");

        let x0: ir::Expression = Conv::conv(&mut context, &x0);
        let x1: ir::Expression = Conv::conv(&mut context, &x1);

        assert_eq!(format!("{x0}"), "((00000001 + 00000002) * 00000003)");
        assert_eq!(format!("{x1}"), "(00000001 + (00000002 * 00000003))");
    }

    #[test]
    fn concatenation() {
        let mut context = Context::default();

        let x0 = parse_expression("{1, 2, 3}");
        let x1 = parse_expression("{1 repeat 2, 2, 3 repeat 4}");

        let x0: ir::Expression = Conv::conv(&mut context, &x0);
        let x1: ir::Expression = Conv::conv(&mut context, &x1);

        assert_eq!(format!("{x0}"), "{00000001, 00000002, 00000003}");
        assert_eq!(
            format!("{x1}"),
            "{00000001 repeat 00000002, 00000002, 00000003 repeat 00000004}"
        );
    }

    #[test]
    fn case_expression() {
        let mut context = Context::default();

        let x0 = parse_expression("case 10 {0: 1, 1: 2, default: 3}");
        let x1 = parse_expression("case 10 {0..=2: 1, 4..5: 2, default: 3}");

        let x0: ir::Expression = Conv::conv(&mut context, &x0);
        let x1: ir::Expression = Conv::conv(&mut context, &x1);

        assert_eq!(
            format!("{x0}"),
            "((0000000a ==? 00000000) ? 00000001 : ((0000000a ==? 00000001) ? 00000002 : 00000003))"
        );
        assert_eq!(
            format!("{x1}"),
            "(((00000000 <= 0000000a) && (0000000a <= 00000002)) ? 00000001 : (((00000004 <= 0000000a) && (0000000a <: 00000005)) ? 00000002 : 00000003))"
        );
    }

    #[test]
    fn switch_expression() {
        let mut context = Context::default();

        let x0 = parse_expression("switch {0 == 1: 2, 1 <: 2: 2, default: 3}");

        let x0: ir::Expression = Conv::conv(&mut context, &x0);

        assert_eq!(
            format!("{x0}"),
            "((00000000 == 00000001) ? 00000002 : ((00000001 <: 00000002) ? 00000002 : 00000003))"
        );
    }
}
