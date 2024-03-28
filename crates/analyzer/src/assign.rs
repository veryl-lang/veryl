use crate::symbol::SymbolId;
use crate::symbol_table;
use std::fmt;
use veryl_parser::veryl_token::Token;

#[derive(Clone, Debug)]
pub struct Assign {
    pub path: AssignPath,
    pub position: AssignPosition,
    pub partial: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssignPath(pub Vec<SymbolId>);

impl AssignPath {
    pub fn new(x: SymbolId) -> Self {
        Self(vec![x])
    }

    pub fn push(&mut self, x: SymbolId) {
        self.0.push(x)
    }

    pub fn pop(&mut self) -> Option<SymbolId> {
        self.0.pop()
    }

    pub fn included(&self, x: &AssignPath) -> bool {
        for (i, x) in x.0.iter().enumerate() {
            if let Some(path) = self.0.get(i) {
                if path != x {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

impl fmt::Display for AssignPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut ret = "".to_string();
        for (i, id) in self.0.iter().enumerate() {
            if let Some(symbol) = symbol_table::get(*id) {
                if i != 0 {
                    ret.push('.');
                }
                ret.push_str(&symbol.token.to_string());
            }
        }
        ret.fmt(f)
    }
}

#[derive(Clone, Default, Debug)]
pub struct AssignPosition(pub Vec<AssignPositionType>);

impl AssignPosition {
    pub fn new(x: AssignPositionType) -> Self {
        Self(vec![x])
    }

    pub fn push(&mut self, x: AssignPositionType) {
        self.0.push(x)
    }

    pub fn pop(&mut self) -> Option<AssignPositionType> {
        self.0.pop()
    }
}

impl fmt::Display for AssignPosition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut ret = "".to_string();
        for (i, x) in self.0.iter().enumerate() {
            if i != 0 {
                ret.push('.');
            }
            ret.push_str(&x.token().to_string());
        }
        ret.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignPositionType {
    DeclarationBranch {
        token: Token,
        branches: usize,
    },
    DeclarationBranchItem {
        token: Token,
        index: usize,
    },
    Declaration {
        token: Token,
        r#type: AssignDeclarationType,
    },
    StatementBranch {
        token: Token,
        branches: usize,
        has_default: bool,
        allow_missing_reset_statement: bool,
        r#type: AssignStatementBranchType,
    },
    StatementBranchItem {
        token: Token,
        index: usize,
        r#type: AssignStatementBranchItemType,
    },
    Statement {
        token: Token,
        resettable: bool,
    },
}

impl AssignPositionType {
    pub fn token(&self) -> &Token {
        match self {
            AssignPositionType::DeclarationBranch { token: x, .. } => x,
            AssignPositionType::DeclarationBranchItem { token: x, .. } => x,
            AssignPositionType::Declaration { token: x, .. } => x,
            AssignPositionType::StatementBranch { token: x, .. } => x,
            AssignPositionType::StatementBranchItem { token: x, .. } => x,
            AssignPositionType::Statement { token: x, .. } => x,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignDeclarationType {
    Let,
    AlwaysFf,
    AlwaysComb,
    Assign,
    Inst,
    Function,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignStatementBranchType {
    If,
    IfReset,
    Case,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignStatementBranchItemType {
    If,
    IfReset,
    Else,
    Case,
}

#[derive(Clone, Default, Debug)]
pub struct AssignPositionTree {
    r#type: Option<AssignPositionType>,
    children: Vec<AssignPositionTree>,
}

impl AssignPositionTree {
    pub fn add(&mut self, mut pos: AssignPosition) {
        if pos.0.is_empty() {
            return;
        }

        let mut head: Vec<_> = pos.0.drain(0..1).collect();

        for child in &mut self.children {
            if child.r#type.as_ref() == head.first() {
                child.add(pos);
                return;
            }
        }

        let mut node = AssignPositionTree {
            r#type: Some(head.remove(0)),
            children: vec![],
        };
        node.add(pos);
        self.children.push(node);
    }

    pub fn check_always_comb_uncovered(&self) -> Option<Token> {
        if let Some(AssignPositionType::Declaration { ref r#type, .. }) = self.r#type {
            if *r#type == AssignDeclarationType::AlwaysComb {
                let children: Vec<_> = self
                    .children
                    .iter()
                    .map(|x| x.impl_always_comb_uncovered())
                    .collect();
                if children.iter().any(|x| x.is_none()) {
                    return None;
                } else {
                    return children.into_iter().find(|x| x.is_some()).flatten();
                }
            }
        }

        for child in &self.children {
            let ret = child.check_always_comb_uncovered();
            if ret.is_some() {
                return ret;
            }
        }

        None
    }

    fn impl_always_comb_uncovered(&self) -> Option<Token> {
        match self.r#type {
            Some(AssignPositionType::StatementBranch {
                token,
                branches,
                has_default,
                ..
            }) => {
                if !has_default || self.children.len() != branches {
                    Some(token)
                } else {
                    self.children
                        .iter()
                        .map(|x| x.impl_always_comb_uncovered())
                        .find(|x| x.is_some())
                        .flatten()
                }
            }
            Some(AssignPositionType::StatementBranchItem { .. }) => {
                let children: Vec<_> = self
                    .children
                    .iter()
                    .map(|x| x.impl_always_comb_uncovered())
                    .collect();
                if children.iter().any(|x| x.is_none()) {
                    None
                } else {
                    children.into_iter().find(|x| x.is_some()).flatten()
                }
            }
            Some(AssignPositionType::Statement { .. }) => None,
            _ => unreachable!(),
        }
    }

    pub fn check_always_ff_missing_reset(&self) -> Option<Token> {
        if let Some(AssignPositionType::StatementBranch {
            ref r#type,
            ref token,
            ref allow_missing_reset_statement,
            ..
        }) = self.r#type
        {
            if *r#type == AssignStatementBranchType::IfReset
                && !allow_missing_reset_statement
                && self.is_resettable()
            {
                if let Some(AssignPositionType::StatementBranchItem { ref r#type, .. }) =
                    self.children[0].r#type
                {
                    if *r#type != AssignStatementBranchItemType::IfReset {
                        return Some(*token);
                    }
                }
            }
        }

        for child in &self.children {
            let ret = child.check_always_ff_missing_reset();
            if ret.is_some() {
                return ret;
            }
        }

        None
    }

    fn is_resettable(&self) -> bool {
        if let Some(AssignPositionType::Statement { resettable, .. }) = self.r#type {
            resettable
        } else {
            self.children.iter().any(|x| x.is_resettable())
        }
    }
}
