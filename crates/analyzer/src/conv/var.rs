use crate::conv::checker::separator::check_separator;
use crate::conv::{Context, Conv};
use crate::ir::{self, VarIndex, VarPath, VarPathIndex};
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&Identifier> for VarPath {
    fn conv(_context: &mut Context, value: &Identifier) -> Self {
        VarPath(vec![value.text()])
    }
}

impl Conv<&ScopedIdentifier> for VarPath {
    fn conv(_context: &mut Context, value: &ScopedIdentifier) -> Self {
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

        VarPath(path)
    }
}

impl Conv<&ExpressionIdentifier> for VarPathIndex {
    fn conv(context: &mut Context, value: &ExpressionIdentifier) -> Self {
        check_separator(context, value);

        let mut path: VarPath = Conv::conv(context, value.scoped_identifier.as_ref());
        let mut index = VarIndex::default();
        let mut end: Option<ir::Expression> = None;

        for x in &value.expression_identifier_list {
            if end.is_some() {
                // TODO invalid_select error like "[1:0][0]"
            }
            index.push(Conv::conv(context, x.select.expression.as_ref()));
            if let Some(x) = &x.select.select_opt {
                // TODO select operator
                end = Some(Conv::conv(context, x.expression.as_ref()));
            }
        }

        for x in &value.expression_identifier_list0 {
            path.push(x.identifier.identifier_token.token.text);
            for x in &x.expression_identifier_list0_list {
                if end.is_some() {
                    // TODO invalid_select error like "[1:0][0]"
                }
                index.push(Conv::conv(context, x.select.expression.as_ref()));
                if let Some(x) = &x.select.select_opt {
                    // TODO select operator
                    end = Some(Conv::conv(context, x.expression.as_ref()));
                }
            }
        }

        index.1 = end;

        VarPathIndex(path, index)
    }
}

impl Conv<&HierarchicalIdentifier> for VarPathIndex {
    fn conv(context: &mut Context, value: &HierarchicalIdentifier) -> Self {
        let mut path: VarPath = Conv::conv(context, value.identifier.as_ref());
        let mut index = VarIndex::default();
        let mut end: Option<ir::Expression> = None;

        for x in &value.hierarchical_identifier_list {
            if end.is_some() {
                // TODO invalid_select error like "[1:0][0]"
            }
            index.push(Conv::conv(context, x.select.expression.as_ref()));
            if let Some(x) = &x.select.select_opt {
                // TODO select operator
                end = Some(Conv::conv(context, x.expression.as_ref()));
            }
        }

        for x in &value.hierarchical_identifier_list0 {
            path.push(x.identifier.identifier_token.token.text);
            for x in &x.hierarchical_identifier_list0_list {
                if end.is_some() {
                    // TODO invalid_select error like "[1:0][0]"
                }
                index.push(Conv::conv(context, x.select.expression.as_ref()));
                if let Some(x) = &x.select.select_opt {
                    // TODO select operator
                    end = Some(Conv::conv(context, x.expression.as_ref()));
                }
            }
        }

        index.1 = end;

        VarPathIndex(path, index)
    }
}
