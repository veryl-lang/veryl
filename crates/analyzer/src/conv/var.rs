use crate::conv::checker::separator::check_separator;
use crate::conv::{Context, Conv};
use crate::ir::{self, IrResult, VarPath, VarPathSelect, VarSelect, VarSelectOp};
use crate::symbol_path::{GenericSymbol, GenericSymbolPath};
use crate::{AnalyzerError, ir_error};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&Identifier> for VarPath {
    fn conv(_context: &mut Context, value: &Identifier) -> IrResult<Self> {
        Ok(VarPath(vec![value.text()]))
    }
}

impl Conv<&ScopedIdentifier> for VarPath {
    fn conv(_context: &mut Context, value: &ScopedIdentifier) -> IrResult<Self> {
        let mut path = Vec::new();
        match value.scoped_identifier_group.as_ref() {
            ScopedIdentifierGroup::DollarIdentifier(x) => {
                path.push(x.dollar_identifier.dollar_identifier_token.token.text);
            }
            ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                path.push(x.identifier.identifier_token.token.text);
            }
        }

        for x in &value.scoped_identifier_list {
            path.push(x.identifier.identifier_token.token.text);
        }

        Ok(VarPath(path))
    }
}

impl Conv<&SelectOperator> for VarSelectOp {
    fn conv(_context: &mut Context, value: &SelectOperator) -> IrResult<Self> {
        let ret = match value {
            SelectOperator::Colon(_) => VarSelectOp::Colon,
            SelectOperator::PlusColon(_) => VarSelectOp::PlusColon,
            SelectOperator::MinusColon(_) => VarSelectOp::MinusColon,
            SelectOperator::Step(_) => VarSelectOp::Step,
        };
        Ok(ret)
    }
}

fn check_select_type(context: &mut Context, expr: &mut ir::Expression, value: &Expression) {
    let token: TokenRange = value.into();
    let comptime = expr.eval_comptime(context, None, expr.eval_signed());
    if comptime.r#type.is_type() {
        context.insert_error(AnalyzerError::invalid_factor(
            None,
            &comptime.r#type.to_string(),
            &token,
            &[],
        ));
    }
}

impl Conv<&ScopedIdentifier> for VarPathSelect {
    fn conv(context: &mut Context, value: &ScopedIdentifier) -> IrResult<Self> {
        let var_path: VarPath = Conv::conv(context, value)?;
        let token: TokenRange = value.into();
        Ok(VarPathSelect(var_path, VarSelect::default(), token))
    }
}

impl Conv<&ExpressionIdentifier> for VarPathSelect {
    fn conv(context: &mut Context, value: &ExpressionIdentifier) -> IrResult<Self> {
        check_separator(context, value);

        let mut path: VarPath = Conv::conv(context, value.scoped_identifier.as_ref())?;
        let mut generic_path: GenericSymbolPath = value.scoped_identifier.as_ref().into();
        let mut select = VarSelect::default();
        let token: TokenRange = value.into();
        let mut end: Option<(VarSelectOp, ir::Expression)> = None;

        context.select_dims.push(0);

        for x in &value.expression_identifier_list {
            if end.is_some() {
                // TODO invalid_select error like "[1:0][0]"
                return Err(ir_error!(token));
            }
            context
                .select_paths
                .push((path.clone(), generic_path.clone()));
            let mut expr = Conv::conv(context, x.select.expression.as_ref())?;
            check_select_type(context, &mut expr, &x.select.expression);
            select.push(expr);
            if let Some(x) = &x.select.select_opt {
                let op = Conv::conv(context, x.select_operator.as_ref())?;
                let mut expr = Conv::conv(context, x.expression.as_ref())?;
                check_select_type(context, &mut expr, &x.expression);
                end = Some((op, expr));
            }
            context.select_paths.pop();
            context.inc_select_dim();
        }

        for x in &value.expression_identifier_list0 {
            path.push(x.identifier.identifier_token.token.text);
            generic_path.paths.push(GenericSymbol {
                base: x.identifier.identifier_token.token,
                arguments: vec![],
            });
            context
                .select_paths
                .push((path.clone(), generic_path.clone()));
            for x in &x.expression_identifier_list0_list {
                if end.is_some() {
                    // TODO invalid_select error like "[1:0][0]"
                    return Err(ir_error!(token));
                }
                let mut expr = Conv::conv(context, x.select.expression.as_ref())?;
                check_select_type(context, &mut expr, &x.select.expression);
                select.push(expr);
                if let Some(x) = &x.select.select_opt {
                    let op = Conv::conv(context, x.select_operator.as_ref())?;
                    let mut expr = Conv::conv(context, x.expression.as_ref())?;
                    check_select_type(context, &mut expr, &x.expression);
                    end = Some((op, expr));
                }
                context.inc_select_dim();
            }
            context.select_paths.pop();
        }

        context.select_dims.pop();

        select.1 = end;

        Ok(VarPathSelect(path, select, token))
    }
}

impl Conv<&HierarchicalIdentifier> for VarPathSelect {
    fn conv(context: &mut Context, value: &HierarchicalIdentifier) -> IrResult<Self> {
        let mut path: VarPath = Conv::conv(context, value.identifier.as_ref())?;
        let mut generic_path: GenericSymbolPath = value.identifier.as_ref().into();
        let mut select = VarSelect::default();
        let token: TokenRange = value.into();
        let mut end: Option<(VarSelectOp, ir::Expression)> = None;

        for x in &value.hierarchical_identifier_list {
            if end.is_some() {
                // TODO invalid_select error like "[1:0][0]"
                return Err(ir_error!(token));
            }
            context
                .select_paths
                .push((path.clone(), generic_path.clone()));
            let mut expr = Conv::conv(context, x.select.expression.as_ref())?;
            check_select_type(context, &mut expr, &x.select.expression);
            select.push(expr);
            if let Some(x) = &x.select.select_opt {
                let op = Conv::conv(context, x.select_operator.as_ref())?;
                let mut expr = Conv::conv(context, x.expression.as_ref())?;
                check_select_type(context, &mut expr, &x.expression);
                end = Some((op, expr));
            }
            context.select_paths.pop();
        }

        for x in &value.hierarchical_identifier_list0 {
            path.push(x.identifier.identifier_token.token.text);
            generic_path.paths.push(GenericSymbol {
                base: x.identifier.identifier_token.token,
                arguments: vec![],
            });
            context
                .select_paths
                .push((path.clone(), generic_path.clone()));
            for x in &x.hierarchical_identifier_list0_list {
                if end.is_some() {
                    // TODO invalid_select error like "[1:0][0]"
                    return Err(ir_error!(token));
                }
                let mut expr = Conv::conv(context, x.select.expression.as_ref())?;
                check_select_type(context, &mut expr, &x.select.expression);
                select.push(expr);
                if let Some(x) = &x.select.select_opt {
                    let op = Conv::conv(context, x.select_operator.as_ref())?;
                    let mut expr = Conv::conv(context, x.expression.as_ref())?;
                    check_select_type(context, &mut expr, &x.expression);
                    end = Some((op, expr));
                }
            }
            context.select_paths.pop();
        }

        select.1 = end;

        Ok(VarPathSelect(path, select, token))
    }
}

impl Conv<&Expression> for Vec<VarPathSelect> {
    fn conv(context: &mut Context, value: &Expression) -> IrResult<Self> {
        let mut ret = vec![];

        if let Some(x) = value.unwrap_factor() {
            match x {
                Factor::IdentifierFactor(x) => {
                    let x: VarPathSelect =
                        Conv::conv(context, x.identifier_factor.expression_identifier.as_ref())?;
                    ret.push(x);
                }
                Factor::LBraceConcatenationListRBrace(x) => {
                    let items: Vec<_> = x.concatenation_list.as_ref().into();
                    for item in items {
                        let mut x: Vec<VarPathSelect> =
                            Conv::conv(context, item.expression.as_ref())?;
                        ret.append(&mut x);
                    }
                }
                _ => (),
            }
        }

        Ok(ret)
    }
}
