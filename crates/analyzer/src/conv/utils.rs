use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::conv::instance::{InstanceHistoryError, InstanceSignature};
use crate::conv::{Context, Conv};
use crate::definition_table::{self, Definition};
use crate::ir::{
    self, Arguments, Op, TypedValue, Value, ValueVariant, VarPath, VarPathSelect, VarSelect,
};
use crate::symbol::{Symbol, SymbolId, SymbolKind};
use crate::symbol_path::{GenericSymbolPath, GenericSymbolPathNamesapce, SymbolPathNamespace};
use crate::symbol_table;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

pub fn eval_expr(
    context: &mut Context,
    dst_type: Option<ir::Type>,
    expr: &Expression,
) -> (TypedValue, ir::Expression) {
    let expr: ir::Expression = Conv::conv(context, expr);

    let typed_value = if let Some(dst_type) = dst_type {
        let mut typed_value = expr.eval_type(context, Some(dst_type.total_width()));

        if !dst_type.compatible(&typed_value.r#type) {
            let range = expr.token_range();
            let src_type = typed_value.r#type.to_string();
            let dst_type = dst_type.to_string();
            context.insert_error(AnalyzerError::mismatch_assignment(
                &src_type,
                &dst_type,
                &range,
                &[],
            ));
        }

        typed_value.r#type = dst_type;
        typed_value
    } else {
        expr.eval_type(context, None)
    };

    (typed_value, expr)
}

pub fn eval_range(context: &mut Context, range: &Range) -> Option<(usize, usize)> {
    let beg: ir::Expression = Conv::conv(context, range.expression.as_ref());
    let beg = beg.eval_type(context, None);
    let beg = beg.get_value().map(|x| x.to_usize());

    let end = if let Some(x) = &range.range_opt {
        let end: ir::Expression = Conv::conv(context, x.expression.as_ref());
        let end = end.eval_type(context, None);
        let end = end.get_value().map(|x| x.to_usize());

        if matches!(x.range_operator.as_ref(), RangeOperator::DotDotEqu(_)) {
            end.map(|x| x + 1)
        } else {
            end
        }
    } else {
        beg
    };

    if let (Some(beg), Some(end)) = (beg, end) {
        Some((beg, end))
    } else {
        None
    }
}

pub fn eval_array_literal(
    context: &mut Context,
    context_array: Option<&[usize]>,
    expr: &ir::Expression,
) -> Option<Vec<ir::Expression>> {
    if let ir::Expression::ArrayLiteral(x) = expr {
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
                            if let Some(repeat) = eval_repeat(context, repeat) {
                                repeat.to_usize()
                            } else {
                                return None;
                            }
                        } else {
                            1
                        };

                        let mut exprs =
                            if let Some(x) = eval_array_literal(context, next_array, expr) {
                                x
                            } else {
                                vec![expr.clone()]
                            };

                        for _ in 0..repeat {
                            ret.append(&mut exprs);
                        }
                    }
                    ir::ArrayLiteralItem::Defaul(expr) => {
                        let exprs = if let Some(x) = eval_array_literal(context, next_array, expr) {
                            x
                        } else {
                            vec![expr.clone()]
                        };

                        if default.is_none() {
                            default = Some(exprs);
                        } else {
                            // TODO multiple default error
                            return None;
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
                    return None;
                }
            } else if target_len != x.len() {
                // TODO mismatch dimension error
                return None;
            }
        } else {
            // TODO error, not array context
            return None;
        }

        Some(ret)
    } else {
        None
    }
}

pub fn eval_repeat(context: &mut Context, expr: &ir::Expression) -> Option<Value> {
    let token = expr.token_range();
    let repeat = expr.eval_type(context, None);

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

pub fn case_condition(
    context: &mut Context,
    tgt: &ir::Expression,
    cond: &CaseCondition,
) -> ir::Expression {
    let mut ret = range_item(context, tgt, &cond.range_item);
    for x in &cond.case_condition_list {
        let item = range_item(context, tgt, &x.range_item);
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(item));
    }
    ret
}

pub fn range_list(context: &mut Context, tgt: &ir::Expression, list: &RangeList) -> ir::Expression {
    let mut ret = range_item(context, tgt, &list.range_item);
    for x in &list.range_list_list {
        let item = range_item(context, tgt, &x.range_item);
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(item));
    }
    ret
}

fn range_item(
    context: &mut Context,
    tgt: &ir::Expression,
    range_item: &RangeItem,
) -> ir::Expression {
    let exp: ir::Expression = Conv::conv(context, range_item.range.expression.as_ref());

    let typed_value = exp.eval_type(context, None);
    if !typed_value.is_const {
        context.insert_error(AnalyzerError::invalid_case_condition_non_elaborative(
            &range_item.into(),
        ));
    }

    if let Some(x) = &range_item.range.range_opt {
        let exp0: ir::Expression = Conv::conv(context, x.expression.as_ref());

        let typed_value = exp0.eval_type(context, None);
        if !typed_value.is_const {
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
    }
}

pub fn switch_condition(context: &mut Context, cond: &SwitchCondition) -> ir::Expression {
    let mut ret: ir::Expression = Conv::conv(context, cond.expression.as_ref());
    for x in &cond.switch_condition_list {
        let exp: ir::Expression = Conv::conv(context, x.expression.as_ref());
        ret = ir::Expression::Binary(Box::new(ret), Op::LogicOr, Box::new(exp));
    }
    ret
}

pub fn argument_list(context: &mut Context, value: &ArgumentList) -> Arguments {
    let mut positional = vec![];
    let mut named = vec![];
    let x: Vec<_> = value.into();
    for arg in x {
        if let Some(x) = &arg.argument_item_opt {
            if let Some(name) = arg.argument_expression.expression.unwrap_identifier() {
                let name = name.identifier().token.text;
                let expr = Conv::conv(context, x.expression.as_ref());
                let dst: Vec<VarPathSelect> = Conv::conv(context, x.expression.as_ref());
                let dst: Vec<_> = dst
                    .into_iter()
                    .filter_map(|x| x.to_assign_destination(context))
                    .collect();
                named.push((name, (expr, dst)));
            } else {
                // TODO error
            }
        } else {
            let expr = Conv::conv(context, arg.argument_expression.expression.as_ref());
            let dst: Vec<VarPathSelect> =
                Conv::conv(context, arg.argument_expression.expression.as_ref());
            let dst: Vec<_> = dst
                .into_iter()
                .filter_map(|x| x.to_assign_destination(context))
                .collect();
            positional.push((expr, dst));
        }
    }

    if !positional.is_empty() && !named.is_empty() {
        context.insert_error(AnalyzerError::mixed_function_argument(&value.into()));
    }

    if !named.is_empty() {
        Arguments::Named(named)
    } else if !positional.is_empty() {
        Arguments::Positional(positional)
    } else {
        Arguments::Null
    }
}

pub fn get_component(
    context: &mut Context,
    arg: &ComponentInstantiation,
) -> Option<(ir::Component, SymbolId)> {
    let generic_path: GenericSymbolPath = arg.scoped_identifier.as_ref().into();
    let path: SymbolPathNamespace = arg.scoped_identifier.as_ref().into();

    let symbol = symbol_table::resolve(&path).ok()?;

    let symbol = match &symbol.found.kind {
        SymbolKind::Module(_) | SymbolKind::Interface(_) => symbol.found,
        SymbolKind::GenericParameter(_) => {
            let name = symbol.found.token.text;
            let path = context.get_generic_argument(&name)?;
            let path: GenericSymbolPathNamesapce = (path, &symbol.found.namespace).into();
            let symbol = symbol_table::resolve(&path).ok()?;
            symbol.found
        }
        _ => {
            return None;
        }
    };

    let mut sig = InstanceSignature::new(symbol.id);

    let mut generic_arguments = HashMap::default();
    let mut generic_base = None;
    if generic_path.is_generic() {
        let mut path = path.clone();
        path.0 = generic_path.mangled_path();
        if let Ok(symbol) = symbol_table::resolve(path)
            && let SymbolKind::GenericInstance(x) = &symbol.found.kind
        {
            let base = symbol_table::get(x.base).unwrap();
            let params = base.kind.get_generic_parameters();

            if params.len() == x.arguments.len() {
                for (i, p) in params.iter().enumerate() {
                    let p = symbol_table::get(*p).unwrap();
                    let name = p.token.text;
                    sig.add_generic_parameter(name, x.arguments[i].clone());
                    generic_arguments.insert(name, x.arguments[i].clone());
                }
            }

            generic_base = Some(base);
        }
    }

    let parameters = symbol.kind.get_parameters();
    let overridden_params = get_overridden_params(context, arg);
    for x in parameters {
        let path = VarPath::new(x.name);
        if let Some(value) = overridden_params.get(&path) {
            sig.add_parameter(x.name, value.clone());
        }
    }

    if let Some(component) = context.instance_history.get(&sig) {
        Some((component, symbol.id))
    } else {
        let err = context.instance_history.push(sig.clone());

        if let Err(x) = err {
            let token: TokenRange = arg.identifier.as_ref().into();
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
            return None;
        }

        context.overrides.push(overridden_params);
        context.generic_arguments.push(generic_arguments);

        let ret = match &symbol.kind {
            SymbolKind::Module(x) => {
                let definition = definition_table::get(x.definition).unwrap();
                let Definition::Module(x) = definition else {
                    unreachable!()
                };

                let component: ir::Module = Conv::conv(context, &x);
                let component = ir::Component::Module(component);
                context.instance_history.set(&sig, component.clone());
                context.instance_history.pop();

                Some((component, symbol.id))
            }
            SymbolKind::Interface(x) => {
                let definition = definition_table::get(x.definition).unwrap();
                let Definition::Interface(x) = definition else {
                    unreachable!()
                };

                let component: ir::Interface = Conv::conv(context, &x);
                let component = ir::Component::Interface(component);
                context.instance_history.set(&sig, component.clone());
                context.instance_history.pop();

                Some((component, symbol.id))
            }
            SymbolKind::GenericInstance(_) => {
                let base = generic_base.unwrap();
                let component = match &base.kind {
                    SymbolKind::Module(x) => {
                        let definition = definition_table::get(x.definition).unwrap();
                        let Definition::Module(x) = definition else {
                            unreachable!()
                        };
                        let component: ir::Module = Conv::conv(context, &x);
                        ir::Component::Module(component)
                    }
                    SymbolKind::Interface(x) => {
                        let definition = definition_table::get(x.definition).unwrap();
                        let Definition::Interface(x) = definition else {
                            unreachable!()
                        };
                        let component: ir::Interface = Conv::conv(context, &x);
                        ir::Component::Interface(component)
                    }
                    _ => unreachable!(),
                };

                context.instance_history.set(&sig, component.clone());
                context.instance_history.pop();

                Some((component, symbol.id))
            }
            _ => None,
        };

        context.overrides.pop();
        context.generic_arguments.pop();

        ret
    }
}

fn get_overridden_params(
    context: &mut Context,
    arg: &ComponentInstantiation,
) -> HashMap<VarPath, ValueVariant> {
    let mut ret = HashMap::default();

    let Ok(component_namespace) =
        symbol_table::resolve(arg.scoped_identifier.as_ref()).map(|x| x.found.inner_namespace())
    else {
        return ret;
    };

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

        let target_type = target.kind.get_type().and_then(|x| x.to_ir_type(context));

        let value = if let Some(x) = &param.inst_parameter_item_opt {
            eval_expr(context, target_type, &x.expression)
        } else {
            let src: Expression = param.identifier.as_ref().into();
            eval_expr(context, target_type, &src)
        };

        let path = VarPath::new(name);
        ret.insert(path, value.0.value);
    }

    ret
}

pub fn match_interface(dst: &Symbol, src: &Symbol) -> bool {
    if matches!(dst.kind, SymbolKind::ProtoInterface(_)) {
        src.proto().map(|x| x.id == dst.id).unwrap_or(false)
    } else {
        src.id == dst.id
    }
}

pub fn get_return_str() -> StrId {
    resource_table::insert_str("return")
}

pub fn expand_connect(
    context: &mut Context,
    lhs: VarPathSelect,
    rhs: VarPathSelect,
    token: TokenRange,
) -> Vec<ir::Statement> {
    let mut ret = vec![];

    let (lhs_path, _, lhs_token) = lhs.into();
    let (rhs_path, _, rhs_token) = rhs.into();

    if let Some((_, lhs_typed_value)) = context.find_path(&lhs_path)
        && let Some((_, rhs_typed_value)) = context.find_path(&rhs_path)
    {
        // remove modport name from path
        let mut lhs_base = lhs_path.clone();
        let mut rhs_base = rhs_path.clone();
        lhs_base.pop();
        rhs_base.pop();

        let lhs_members = lhs_typed_value.r#type.modport_members(&lhs_base);
        let mut rhs_members = rhs_typed_value.r#type.modport_members(&rhs_base);

        for (name, lhs) in lhs_members {
            if let Some(rhs) = rhs_members.remove(&name) {
                let (dst, src) = if lhs.1.is_output() && rhs.1.is_input() {
                    (lhs.0, rhs.0)
                } else if rhs.1.is_output() && lhs.1.is_input() {
                    (rhs.0, lhs.0)
                } else {
                    // TODO direction error
                    return ret;
                };

                let dst = VarPathSelect(dst, VarSelect::default(), lhs_token);
                let dst = dst.to_assign_destination(context);

                let src = VarPathSelect(src, VarSelect::default(), rhs_token);
                let src = src.to_expression(context);

                if let Some(src) = src
                    && let Some(dst) = dst
                {
                    let statement = ir::Statement::Assign(ir::AssignStatement {
                        dst: vec![dst],
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
