use crate::analyzer_error::InvalidSelectKind;
use crate::conv::checker::separator::check_separator;
use crate::conv::{Context, Conv, generate_block_label};
use crate::ir::{self, IrResult, VarPath, VarPathSelect, VarSelect, VarSelectOp};
use crate::namespace::Namespace;
use crate::symbol::{Symbol, SymbolKind};
use crate::symbol_path::{GenericSymbol, GenericSymbolPath};
use crate::symbol_table;
use crate::{AnalyzerError, ir_error};
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;

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
    let comptime = expr.eval_comptime(context, None);
    if comptime.r#type.is_type() {
        context.insert_error(AnalyzerError::invalid_factor(
            None,
            &comptime.r#type.to_string(),
            &token,
            &[],
        ));
    }
}

// SystemVerilog requires constant bounds for part-selects: the width for
// indexed part-selects (`+:`/`-:`/`step`), and BOTH bounds for a `[msb:lsb]`
// range (`Colon`). A runtime bound is unsynthesizable, so reject it.
// The comptimes must already be evaluated (via check_select_type).
fn check_part_select_width(
    context: &mut Context,
    op: &VarSelectOp,
    base: &ir::Expression,
    bound: &ir::Expression,
) {
    if !bound.comptime().is_const
        && matches!(
            op,
            VarSelectOp::PlusColon
                | VarSelectOp::MinusColon
                | VarSelectOp::Step
                | VarSelectOp::Colon
        )
    {
        context.insert_error(AnalyzerError::non_constant_select_width(
            &bound.token_range(),
        ));
    }
    // A `[msb:lsb]` range also needs a constant base index (the msb).
    if matches!(op, VarSelectOp::Colon) && !base.comptime().is_const {
        context.insert_error(AnalyzerError::non_constant_select_width(
            &base.token_range(),
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

/// Folds a `g_leaf[0]` generate-block hop into one combined `label[index]`
/// segment, the form the IR stores the block under (and the block's genvar
/// variable path uses). Returns the folded name and the block's inner namespace,
/// or `None` if the hop is not a constant-index generate block.
///
/// `block_scope`, the previous hop's block namespace, distinguishes the first
/// block of a chain -- reached across an instance boundary, so the symbol table
/// reports it invisible and it must be recovered from the reference path -- from
/// a nested block, which lives in `block_scope` past that boundary.
fn fold_generate_block_index(
    context: &mut Context,
    generic_path: &GenericSymbolPath,
    block_scope: Option<&Namespace>,
    base_token: &Token,
    selects: &[ExpressionIdentifierList0List],
) -> Option<(StrId, Namespace)> {
    // A generate block has one index dimension and no range.
    let [select] = selects else {
        return None;
    };
    if select.select.select_opt.is_some() {
        return None;
    }

    let block: Symbol = if let Some(scope) = block_scope {
        (*symbol_table::resolve((base_token.text, scope)).ok()?.found).clone()
    } else {
        let i = generic_path.paths.len() - 1;
        let symbol = match symbol_table::resolve_base_path(generic_path, i, base_token.id) {
            Ok(symbol) => (*symbol.found).clone(),
            Err(err) => *err.last_found?,
        };
        // `last_found` is wherever the invisible walk stopped; require it to be
        // this segment so a deeper hop is not read as the block it stopped at.
        if symbol.token.text != base_token.text {
            return None;
        }
        symbol
    };
    if !matches!(block.kind, SymbolKind::Block) {
        return None;
    }

    // Generate elaboration is static, so the subscript must be constant.
    let mut index: ir::Expression = Conv::conv(context, select.select.expression.as_ref()).ok()?;
    let comptime = index.eval_comptime(context, None);
    if !comptime.is_const {
        return None;
    }
    let index = comptime.get_value().ok()?.to_usize()?;
    Some((
        generate_block_label(base_token.text, index),
        block.inner_namespace(),
    ))
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
                context.insert_error(AnalyzerError::invalid_select(
                    &InvalidSelectKind::SelectAfterRange,
                    &token,
                    &[],
                ));
                return Err(ir_error!(token));
            }
            context
                .select_paths
                .push((path.clone(), generic_path.clone()));
            let base_value = x.select.expression.as_ref();
            let mut base = Conv::conv(context, base_value)?;
            check_select_type(context, &mut base, base_value);
            if let Some(x) = &x.select.select_opt {
                let op = Conv::conv(context, x.select_operator.as_ref())?;
                let mut bound = Conv::conv(context, x.expression.as_ref())?;
                check_select_type(context, &mut bound, &x.expression);
                check_part_select_width(context, &op, &base, &bound);
                end = Some((op, bound));
            }
            select.push(base);
            context.select_paths.pop();
            context.inc_select_dim();
        }

        let mut block_scope: Option<Namespace> = None;
        for x in &value.expression_identifier_list0 {
            let base_token = x.identifier.identifier_token.token;
            path.push(base_token.text);
            generic_path.paths.push(GenericSymbol {
                base: base_token,
                arguments: vec![],
            });

            // Generate-block hops only occur in cross-instance references, valid
            // only in a test module; gating there keeps the lookup off the hot
            // path for ordinary RTL member accesses.
            if context.in_test_module
                && !x.expression_identifier_list0_list.is_empty()
                && let Some((folded, inner)) = fold_generate_block_index(
                    context,
                    &generic_path,
                    block_scope.as_ref(),
                    &base_token,
                    &x.expression_identifier_list0_list,
                )
            {
                *path.0.last_mut().unwrap() = folded;
                block_scope = Some(inner);
                continue;
            }
            block_scope = None;

            context
                .select_paths
                .push((path.clone(), generic_path.clone()));
            for x in &x.expression_identifier_list0_list {
                if end.is_some() {
                    context.insert_error(AnalyzerError::invalid_select(
                        &InvalidSelectKind::SelectAfterRange,
                        &token,
                        &[],
                    ));
                    return Err(ir_error!(token));
                }
                let base_value = x.select.expression.as_ref();
                let mut base = Conv::conv(context, base_value)?;
                check_select_type(context, &mut base, base_value);
                if let Some(x) = &x.select.select_opt {
                    let op = Conv::conv(context, x.select_operator.as_ref())?;
                    let mut bound = Conv::conv(context, x.expression.as_ref())?;
                    check_select_type(context, &mut bound, &x.expression);
                    check_part_select_width(context, &op, &base, &bound);
                    end = Some((op, bound));
                }
                select.push(base);
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
                context.insert_error(AnalyzerError::invalid_select(
                    &InvalidSelectKind::SelectAfterRange,
                    &token,
                    &[],
                ));
                return Err(ir_error!(token));
            }
            context
                .select_paths
                .push((path.clone(), generic_path.clone()));
            let base_value = x.select.expression.as_ref();
            let mut base = Conv::conv(context, base_value)?;
            check_select_type(context, &mut base, base_value);
            if let Some(x) = &x.select.select_opt {
                let op = Conv::conv(context, x.select_operator.as_ref())?;
                let mut bound = Conv::conv(context, x.expression.as_ref())?;
                check_select_type(context, &mut bound, &x.expression);
                check_part_select_width(context, &op, &base, &bound);
                end = Some((op, bound));
            }
            select.push(base);
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
                    context.insert_error(AnalyzerError::invalid_select(
                        &InvalidSelectKind::SelectAfterRange,
                        &token,
                        &[],
                    ));
                    return Err(ir_error!(token));
                }
                let base_value = x.select.expression.as_ref();
                let mut base = Conv::conv(context, base_value)?;
                check_select_type(context, &mut base, base_value);
                if let Some(x) = &x.select.select_opt {
                    let op = Conv::conv(context, x.select_operator.as_ref())?;
                    let mut bound = Conv::conv(context, x.expression.as_ref())?;
                    check_select_type(context, &mut bound, &x.expression);
                    check_part_select_width(context, &op, &base, &bound);
                    end = Some((op, bound));
                }
                select.push(base);
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
