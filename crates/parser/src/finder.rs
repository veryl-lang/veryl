use crate::veryl_grammar_trait::*;
use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;

#[derive(Default)]
pub struct Finder {
    pub line: u32,
    pub column: u32,
    pub token: Option<Token>,
    pub token_group: Vec<Token>,
    hit: bool,
    group_hit: bool,
    in_group: bool,
    lock_group: bool,
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
            self.group_hit = true;
        }
        if self.in_group && !self.lock_group {
            self.token_group.push(arg.token);
        }
    }

    /// Semantic action for non-terminal 'HierarchicalIdentifier'
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) {
        self.group_hit = false;
        self.in_group = true;
        self.identifier(&arg.identifier);
        self.in_group = false;
        for x in &arg.hierarchical_identifier_list {
            self.select(&x.select);
        }
        for x in &arg.hierarchical_identifier_list0 {
            self.dot(&x.dot);
            self.in_group = true;
            self.identifier(&x.identifier);
            self.in_group = false;
            for x in &x.hierarchical_identifier_list0_list {
                self.select(&x.select);
            }
        }
        if self.group_hit {
            self.lock_group = true;
        } else if !self.lock_group {
            self.token_group.clear();
        }
    }

    /// Semantic action for non-terminal 'ScopedIdentifier'
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) {
        self.group_hit = false;
        self.in_group = true;
        match &*arg.scoped_identifier_group {
            ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                self.identifier(&x.identifier);
                if let Some(ref x) = x.scoped_identifier_opt {
                    self.with_generic_argument(&x.with_generic_argument);
                }
            }
            ScopedIdentifierGroup::DollarIdentifier(x) => {
                self.dollar_identifier(&x.dollar_identifier)
            }
        }
        self.in_group = false;
        for x in &arg.scoped_identifier_list {
            self.colon_colon(&x.colon_colon);
            self.in_group = true;
            self.identifier(&x.identifier);
            self.in_group = false;
            if let Some(ref x) = x.scoped_identifier_opt0 {
                self.with_generic_argument(&x.with_generic_argument);
            }
        }
        if self.group_hit {
            self.lock_group = true;
        } else if !self.lock_group {
            self.token_group.clear();
        }
    }

    /// Semantic action for non-terminal 'ExpressionIdentifier'
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) {
        let x = &arg.scoped_identifier;
        self.group_hit = false;
        self.in_group = true;
        match &*x.scoped_identifier_group {
            ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                self.identifier(&x.identifier);
                if let Some(ref x) = x.scoped_identifier_opt {
                    self.with_generic_argument(&x.with_generic_argument);
                }
            }
            ScopedIdentifierGroup::DollarIdentifier(x) => {
                self.dollar_identifier(&x.dollar_identifier)
            }
        }
        self.in_group = false;
        for x in &x.scoped_identifier_list {
            self.colon_colon(&x.colon_colon);
            self.in_group = true;
            self.identifier(&x.identifier);
            self.in_group = false;
            if let Some(ref x) = x.scoped_identifier_opt0 {
                self.with_generic_argument(&x.with_generic_argument);
            }
        }
        if let Some(ref x) = arg.expression_identifier_opt {
            self.width(&x.width);
        }
        for x in &arg.expression_identifier_list {
            self.select(&x.select);
        }
        for x in &arg.expression_identifier_list0 {
            self.dot(&x.dot);
            self.in_group = true;
            self.identifier(&x.identifier);
            self.in_group = false;
            for x in &x.expression_identifier_list0_list {
                self.select(&x.select);
            }
        }
        if self.group_hit {
            self.lock_group = true;
        } else if !self.lock_group {
            self.token_group.clear();
        }
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        self.hit = false;
        self.lock_group = false;
        self.token_group.clear();

        self.start(&arg.start);
        for x in &arg.veryl_list {
            self.description_group(&x.description_group);
        }
    }
}
