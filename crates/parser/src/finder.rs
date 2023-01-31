use crate::veryl_grammar_trait::*;
use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;

#[derive(Default)]
pub struct Finder {
    pub line: usize,
    pub column: usize,
    pub token: Option<Token>,
    pub token_group: Vec<Token>,
    hit: bool,
    in_group: bool,
}

impl Finder {
    pub fn new() -> Self {
        Default::default()
    }
}

impl VerylWalker for Finder {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        if arg.token.line == self.line
            && arg.token.column <= self.column
            && self.column < arg.token.column + arg.token.length
        {
            self.token = Some(arg.token);
            self.hit = true;
        }
        if self.in_group {
            self.token_group.push(arg.token);
        }
    }

    /// Semantic action for non-terminal 'HierarchicalIdentifier'
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) {
        self.hit = false;
        self.in_group = true;
        self.identifier(&arg.identifier);
        self.in_group = false;
        for x in &arg.hierarchical_identifier_list {
            self.range(&x.range);
        }
        for x in &arg.hierarchical_identifier_list0 {
            self.dot(&x.dot);
            self.in_group = true;
            self.identifier(&x.identifier);
            self.in_group = false;
            for x in &x.hierarchical_identifier_list0_list {
                self.range(&x.range);
            }
        }
        if !self.hit {
            self.token_group.clear();
        }
    }

    /// Semantic action for non-terminal 'ScopedIdentifier'
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) {
        self.hit = false;
        self.in_group = true;
        self.identifier(&arg.identifier);
        self.in_group = false;
        for x in &arg.scoped_identifier_list {
            self.colon_colon(&x.colon_colon);
            self.in_group = true;
            self.identifier(&x.identifier);
            self.in_group = false;
        }
        if !self.hit {
            self.token_group.clear();
        }
    }

    /// Semantic action for non-terminal 'ExpressionIdentifier'
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) {
        self.hit = false;
        if let Some(ref x) = arg.expression_identifier_opt {
            self.dollar(&x.dollar);
        }
        self.in_group = true;
        self.identifier(&arg.identifier);
        self.in_group = false;
        match &*arg.expression_identifier_group {
            ExpressionIdentifierGroup::ColonColonIdentifierExpressionIdentifierGroupListExpressionIdentifierGroupList0(x) => {
                self.colon_colon(&x.colon_colon);
                self.in_group = true;
                self.identifier(&x.identifier);
                self.in_group = false;
                for x in &x.expression_identifier_group_list {
                    self.colon_colon(&x.colon_colon);
                    self.in_group = true;
                    self.identifier(&x.identifier);
                    self.in_group = false;
                }
                for x in &x.expression_identifier_group_list0 {
                    self.range(&x.range);
                }
            }
            ExpressionIdentifierGroup::ExpressionIdentifierGroupList1ExpressionIdentifierGroupList2(x) => {
                for x in &x.expression_identifier_group_list1 {
                    self.range(&x.range);
                }
                for x in &x.expression_identifier_group_list2 {
                    self.dot(&x.dot);
                    self.in_group = true;
                    self.identifier(&x.identifier);
                    self.in_group = false;
                    for x in &x.expression_identifier_group_list2_list {
                        self.range(&x.range);
                    }
                }
            }
        }
        if !self.hit {
            self.token_group.clear();
        }
    }
}
