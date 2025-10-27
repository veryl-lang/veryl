use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::conv::{Context, Conv};
use crate::ir::{self, Op, TypedValue, Value, VarPath};
use crate::symbol_table;
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
    if let Some(x) = &range_item.range.range_opt {
        let exp0: ir::Expression = Conv::conv(context, x.expression.as_ref());
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

pub fn get_overridden_params(
    context: &mut Context,
    arg: &ComponentInstantiation,
) -> HashMap<VarPath, Value> {
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

        let target_type = target
            .kind
            .get_type()
            .and_then(|x| x.to_ir_type(context).ok());

        let value = if let Some(x) = &param.inst_parameter_item_opt {
            eval_expr(context, target_type, &x.expression)
        } else {
            let src: Expression = param.identifier.as_ref().into();
            eval_expr(context, target_type, &src)
        };

        if let Some(value) = value.0.get_value() {
            let path = VarPath::new(name);
            ret.insert(path, value);
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
