use crate::HashSet;
use crate::analyzer_error::AnalyzerError;
use crate::ir::Op;
use crate::reset_value_table::{
    self, ResetValueCandidate, ResetValueTarget, invalid_reset_value_literal,
};
use crate::symbol_path::GenericSymbolPath;
use crate::utils::{resolve_op, split_index};
use veryl_parser::ParolError;
use veryl_parser::resource_table::TokenId;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

/// Collects reset value literals (`reset_active` / `reset_inactive`) placed where
/// the counterpart reset type can be determined. The counterpart is resolved
/// at `reset_value_table::apply`, and literals placed elsewhere are reported
/// as errors here.
#[derive(Default)]
pub struct CheckResetValueLiteral {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    accepted: HashSet<TokenId>,
    inst_type: Option<GenericSymbolPath>,
}

impl CheckResetValueLiteral {
    pub fn new() -> Self {
        Self::default()
    }

    fn add(&mut self, token: Token, target: ResetValueTarget) {
        self.accepted.insert(token.id);
        reset_value_table::add(ResetValueCandidate { token, target });
    }

    fn add_variable(&mut self, expression: &Expression, path: Option<(GenericSymbolPath, usize)>) {
        if let Some(token) = literal_token(expression)
            && let Some((path, select)) = path
        {
            self.add(token, ResetValueTarget::Variable { path, select });
        }
    }

    /// Checks `==`/`!=` whose operands are a reset value literal and an
    /// identifier reference in `exprs[lo..=hi]`.
    fn check_equality(&mut self, exprs: &[&Expression02], ops: &[(Op, u32)], lo: usize, hi: usize) {
        if lo == hi {
            return;
        }

        let idx = split_index(ops, lo, hi);
        if matches!(ops[idx].0, Op::Eq | Op::Ne) && lo == idx && idx + 1 == hi {
            let (lhs, rhs) = (exprs[lo], exprs[hi]);
            for (literal, counterpart) in [(lhs, rhs), (rhs, lhs)] {
                if let Some(token) = expression02_literal_token(literal)
                    && let Some((path, select)) = expression02_identifier(counterpart)
                {
                    self.add(token, ResetValueTarget::Variable { path, select });
                }
            }
        }

        self.check_equality(exprs, ops, lo, idx);
        self.check_equality(exprs, ops, idx + 1, hi);
    }
}

impl Handler for CheckResetValueLiteral {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn reset_value_literal_token(arg: &ResetValueLiteral) -> Token {
    match arg {
        ResetValueLiteral::ResetActive(x) => x.reset_active.reset_active_token.token,
        ResetValueLiteral::ResetInactive(x) => x.reset_inactive.reset_inactive_token.token,
    }
}

fn factor_literal_token(arg: &Factor) -> Option<Token> {
    match arg {
        Factor::ResetValueLiteral(x) => Some(reset_value_literal_token(&x.reset_value_literal)),
        Factor::LParenExpressionRParen(x) => literal_token(&x.expression),
        _ => None,
    }
}

/// Token of a standalone reset value literal (parentheses allowed).
fn literal_token(arg: &Expression) -> Option<Token> {
    factor_literal_token(arg.unwrap_factor()?)
}

fn expression02_literal_token(arg: &Expression02) -> Option<Token> {
    if !arg.expression02_list.is_empty() || arg.expression02_opt.is_some() {
        return None;
    }
    factor_literal_token(&arg.factor)
}

fn expression02_identifier(arg: &Expression02) -> Option<(GenericSymbolPath, usize)> {
    if !arg.expression02_list.is_empty() || arg.expression02_opt.is_some() {
        return None;
    }
    if let Factor::IdentifierFactor(x) = arg.factor.as_ref()
        && x.identifier_factor.identifier_factor_opt.is_none()
    {
        expression_identifier_path(&x.identifier_factor.expression_identifier)
    } else {
        None
    }
}

/// Number of selects. Range select is not allowed because the result is
/// not a single reset signal.
fn select_count<'a>(selects: impl Iterator<Item = &'a Select>) -> Option<usize> {
    let mut count = 0;
    for x in selects {
        if x.select_opt.is_some() {
            return None;
        }
        count += 1;
    }
    Some(count)
}

fn expression_identifier_path(arg: &ExpressionIdentifier) -> Option<(GenericSymbolPath, usize)> {
    if arg.expression_identifier_opt.is_some() {
        return None;
    }
    let select = if let Some(x) = arg.expression_identifier_list0.last() {
        select_count(
            x.expression_identifier_list0_list
                .iter()
                .map(|x| x.select.as_ref()),
        )?
    } else {
        select_count(
            arg.expression_identifier_list
                .iter()
                .map(|x| x.select.as_ref()),
        )?
    };
    Some((arg.into(), select))
}

fn hierarchical_identifier_path(
    arg: &HierarchicalIdentifier,
) -> Option<(GenericSymbolPath, usize)> {
    let select = if let Some(x) = arg.hierarchical_identifier_list0.last() {
        select_count(
            x.hierarchical_identifier_list0_list
                .iter()
                .map(|x| x.select.as_ref()),
        )?
    } else {
        select_count(
            arg.hierarchical_identifier_list
                .iter()
                .map(|x| x.select.as_ref()),
        )?
    };
    Some((arg.into(), select))
}

impl VerylGrammarTrait for CheckResetValueLiteral {
    fn reset_value_literal(&mut self, arg: &ResetValueLiteral) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = reset_value_literal_token(arg);
            if !self.accepted.remove(&token.id) {
                self.errors.push(invalid_reset_value_literal(&token));
            }
        }
        Ok(())
    }

    fn expression01(&mut self, arg: &Expression01) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && !arg.expression01_list.is_empty()
        {
            let mut exprs = vec![arg.expression02.as_ref()];
            let mut ops = Vec::new();
            for x in &arg.expression01_list {
                exprs.push(x.expression02.as_ref());
                ops.push(resolve_op(&x.expression01_op));
            }
            self.check_equality(&exprs, &ops, 0, exprs.len() - 1);
        }
        Ok(())
    }

    fn assign_declaration(&mut self, arg: &AssignDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && let AssignDestination::HierarchicalIdentifier(x) = arg.assign_destination.as_ref()
        {
            let path = hierarchical_identifier_path(&x.hierarchical_identifier);
            self.add_variable(&arg.expression, path);
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && let IdentifierStatementGroup::Assignment(x) = arg.identifier_statement_group.as_ref()
            && let AssignmentGroup::Equ(_) = x.assignment.assignment_group.as_ref()
        {
            let path = expression_identifier_path(&arg.expression_identifier);
            self.add_variable(&x.assignment.expression, path);
        }
        Ok(())
    }

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if arg.let_declaration_opt.is_some() {
                let path = Some((arg.identifier.as_ref().into(), 0));
                self.add_variable(&arg.expression, path);
            } else if let Some(token) = literal_token(&arg.expression) {
                // Type inference of `let` is reported by another check.
                self.accepted.insert(token.id);
            }
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if arg.let_statement_opt.is_some() {
                let path = Some((arg.identifier.as_ref().into(), 0));
                self.add_variable(&arg.expression, path);
            } else if let Some(token) = literal_token(&arg.expression) {
                // Type inference of `let` is reported by another check.
                self.accepted.insert(token.id);
            }
        }
        Ok(())
    }

    fn component_instantiation(&mut self, arg: &ComponentInstantiation) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.inst_type = Some(arg.scoped_identifier.as_ref().into());
            }
            HandlerPoint::After => {
                self.inst_type = None;
            }
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && let Some(x) = &arg.inst_port_item_opt
            && let Some(token) = literal_token(&x.expression)
            && let Some(inst_type) = self.inst_type.clone()
        {
            let port = arg.identifier.identifier_token.token.text;
            self.add(token, ResetValueTarget::InstPort { inst_type, port });
        }
        Ok(())
    }
}
