use crate::analyzer_error::AnalyzerError;
use crate::conv::instance::InstanceHistoryError;
use crate::conv::{Context, Conv};
use crate::definition_table::{self, Definition};
use crate::ir::{
    self, Arguments, Comptime, IrResult, Op, Signature, ValueVariant, VarIndex, VarKind, VarPath,
    VarPathSelect, VarSelect,
};
use crate::symbol::{GenericMap, Symbol, SymbolKind};
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
) -> IrResult<(Comptime, ir::Expression)> {
    let expr: ir::Expression = Conv::conv(context, expr)?;

    let comptime = if let Some(dst_type) = dst_type {
        let mut comptime = expr.eval_comptime(context, Some(dst_type.total_width()));

        if !dst_type.compatible(&comptime.r#type) {
            let range = expr.token_range();
            let src_type = comptime.r#type.to_string();
            let dst_type = dst_type.to_string();
            context.insert_error(AnalyzerError::mismatch_assignment(
                &src_type,
                &dst_type,
                &range,
                &[],
            ));
        }

        comptime.r#type = dst_type;
        comptime
    } else {
        expr.eval_comptime(context, None)
    };

    Ok((comptime, expr))
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

                    let mut exprs = if let Some(x) = eval_array_literal(context, next_array, expr)?
                    {
                        x
                    } else {
                        vec![expr.clone()]
                    };

                    for _ in 0..repeat {
                        ret.append(&mut exprs);
                    }
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
        } else if target_len != x.len() {
            // TODO mismatch dimension error
            return Err(ir_error!(token));
        }
    } else {
        // TODO error, not array context
        return Err(ir_error!(token));
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
            &repeat.r#type.to_string(),
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
                let dst: Vec<_> = dst
                    .into_iter()
                    .filter_map(|x| x.to_assign_destination(context))
                    .collect();
                named.push((name, (expr, dst, token)));
            } else {
                // TODO error
            }
        } else {
            let token: TokenRange = arg.argument_expression.expression.as_ref().into();
            let expr = Conv::conv(context, arg.argument_expression.expression.as_ref())?;
            let dst: Vec<VarPathSelect> =
                Conv::conv(context, arg.argument_expression.expression.as_ref())?;
            let dst: Vec<_> = dst
                .into_iter()
                .filter_map(|x| x.to_assign_destination(context))
                .collect();
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

    if let Some(component) = context.instance_history.get(sig) {
        Ok(component)
    } else {
        let err = context.instance_history.push(sig.clone());

        if let Err(x) = err {
            match x {
                InstanceHistoryError::ExceedDepthLimit => {
                    context
                        .insert_error(AnalyzerError::exceed_limit("hierarchy depth limit", &token));
                }
                InstanceHistoryError::ExceedTotalLimit => {
                    context
                        .insert_error(AnalyzerError::exceed_limit("total instance limit", &token));
                }
                InstanceHistoryError::InfiniteRecursion => {
                    context.insert_error(AnalyzerError::infinite_recursion(&token));
                }
            }
            return Err(ir_error!(token));
        }

        let mut generic_map = GenericMap::default();
        for (key, val) in &sig.generic_parameters {
            generic_map.map.insert(*key, val.clone());
        }
        context.generic_maps.push(vec![generic_map]);

        let ret = match &symbol.kind {
            SymbolKind::Module(x) => {
                let definition = definition_table::get(x.definition).unwrap();
                let Definition::Module(x) = definition else {
                    unreachable!()
                };

                let component: IrResult<ir::Module> = Conv::conv(context, &x);
                match component {
                    Ok(component) => {
                        let component = ir::Component::Module(component);
                        context.instance_history.set(sig, component.clone());
                        context.instance_history.pop();
                        Ok(component)
                    }
                    Err(x) => {
                        context.instance_history.pop();
                        Err(x)
                    }
                }
            }
            SymbolKind::Interface(x) => {
                let definition = definition_table::get(x.definition).unwrap();
                let Definition::Interface(x) = definition else {
                    unreachable!()
                };

                let component: IrResult<ir::Interface> = Conv::conv(context, &x);
                match component {
                    Ok(component) => {
                        let component = ir::Component::Interface(component);
                        context.instance_history.set(sig, component.clone());
                        context.instance_history.pop();
                        Ok(component)
                    }
                    Err(x) => {
                        context.instance_history.pop();
                        Err(x)
                    }
                }
            }
            _ => Err(ir_error!(token)),
        };

        context.generic_maps.pop();

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
            let x = x.to_ir_type(context);
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
            eval_expr(context, target_type, &x.expression)?
        } else {
            let src: Expression = param.identifier.as_ref().into();
            eval_expr(context, target_type, &src)?
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
    port: &InstPortItem,
    port_path: &VarPath,
    port_type: &ir::Type,
    token: TokenRange,
) -> IrResult<Vec<(VarPath, Vec<VarPathSelect>, ir::Expression)>> {
    let mut ret = vec![];

    if let ir::TypeKind::Modport(_, _) = &port_type.kind {
        if let Some(x) = &port.inst_port_item_opt {
            // Check type compatibility
            let _ = eval_expr(context, Some(port_type.clone()), &x.expression);
        }

        let dst_path = if let Some(x) = &port.inst_port_item_opt {
            let dst: Vec<VarPathSelect> = Conv::conv(context, x.expression.as_ref())?;
            let dst = dst.first().ok_or_else(|| ir_error!(token))?;
            dst.0.clone()
        } else {
            port_path.clone()
        };

        let members = port_type.modport_members(&dst_path);
        for member in members.values() {
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
        // Check type compatibility
        let (_, expr) = eval_expr(context, Some(port_type.clone()), &x.expression)?;
        let dst: Vec<VarPathSelect> = Conv::conv(context, x.expression.as_ref())?;
        ret.push((port_path.clone(), dst, expr));
    } else if let Some((var_id, comptime)) = context.find_path(port_path) {
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
            let dst: Vec<_> = dst
                .into_iter()
                .filter_map(|x| x.to_assign_destination(context))
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
) -> Vec<ir::Statement> {
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

        let lhs_members = lhs_comptime.r#type.modport_members(&lhs_base);
        let mut rhs_members = rhs_comptime.r#type.modport_members(&rhs_base);

        for (name, lhs) in lhs_members {
            if let Some(rhs) = rhs_members.remove(&name) {
                let lhs_direction = lhs.1;
                let rhs_direction = rhs.1;
                let lhs = VarPathSelect(lhs.0, lhs_select.clone(), lhs_token);
                let rhs = VarPathSelect(rhs.0, rhs_select.clone(), rhs_token);

                let (dst, src) = if lhs_direction.is_output() && rhs_direction.is_input() {
                    (lhs, rhs)
                } else if rhs_direction.is_output() && lhs_direction.is_input() {
                    (rhs, lhs)
                } else {
                    // TODO direction error
                    return ret;
                };

                if let Some(src) = src.to_expression(context)
                    && let Some(dst) = dst.to_assign_destination(context)
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
            } else {
                // TODO member mismatch error
            }
        }

        if !rhs_members.is_empty() {
            // TODO member mismatch error
        }
    }

    ret
}

pub fn expand_connect_const(
    context: &mut Context,
    lhs: VarPathSelect,
    comptime: Comptime,
    token: TokenRange,
) -> Vec<ir::Statement> {
    let mut ret = vec![];

    let (lhs_path, lhs_select, lhs_token) = lhs.into();

    if let Some((_, lhs_comptime)) = context.find_path(&lhs_path) {
        // remove modport name from path
        let mut lhs_base = lhs_path.clone();
        if lhs_base.0.len() != 1 {
            lhs_base.pop();
        }

        let lhs_members = lhs_comptime.r#type.modport_members(&lhs_base);

        for (_, lhs) in lhs_members {
            if lhs.1.is_output() {
                let dst = VarPathSelect(lhs.0, lhs_select.clone(), lhs_token);
                let src = ir::Factor::Value(comptime.clone(), token);
                let src = ir::Expression::Term(Box::new(src));

                if let Some(dst) = dst.to_assign_destination(context)
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

    ret
}

pub fn function_call(
    context: &mut Context,
    path: &ExpressionIdentifier,
    symbol: &Symbol,
    args: Arguments,
    token: TokenRange,
) -> IrResult<ir::FunctionCall> {
    let generic_path: GenericSymbolPath = path.into();
    let sig = Signature::from_path(context, generic_path).ok_or_else(|| ir_error!(token))?;

    let path: VarPathSelect = Conv::conv(context, path)?;
    let (mut base_path, select, _) = path.into();
    let index = select.to_index();
    let index = index.eval_value(context);

    // remove function name
    base_path.pop();

    let path = ir::FuncPath {
        path: base_path.clone(),
        sig,
    };

    let (inputs, outputs) = args.to_function_args(context, symbol)?;

    if let Some((id, ret)) = context.func_paths.get(&path) {
        Ok(ir::FunctionCall {
            id: *id,
            index,
            ret: ret.clone(),
            inputs,
            outputs,
            token,
        })
    } else {
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

        let array = if let Some((_, comptime)) = context.find_path(&base_path) {
            comptime.r#type.array
        } else {
            vec![]
        };

        let mut local_context = Context {
            var_id: context.var_id,
            ..Default::default()
        };
        local_context.inherit(context);
        local_context.extract_var_paths(context, &base_path, &array);

        let _: () = Conv::conv(&mut local_context, &definition)?;

        context.extract_function(&mut local_context, &base_path, &array);

        context.inherit(&mut local_context);
        context.var_id = local_context.var_id;

        let (id, ret) = context
            .func_paths
            .get(&path)
            .ok_or_else(|| ir_error!(token))?;
        Ok(ir::FunctionCall {
            id: *id,
            index,
            ret: ret.clone(),
            inputs,
            outputs,
            token,
        })
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
