use crate::ir::{Component, Statement, VarId};
use indent::indent_all_by;
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Default)]
pub struct DeclarationBlock(pub Vec<Declaration>);

impl DeclarationBlock {
    pub fn new(decl: Declaration) -> Self {
        Self(vec![decl])
    }
}

#[derive(Clone)]
pub enum Declaration {
    Comb(CombDeclaration),
    Ff(FfDeclaration),
    Inst(InstDeclaration),
    Null,
}

impl Declaration {
    pub fn new_comb(statements: Vec<Statement>) -> Self {
        Self::Comb(CombDeclaration { statements })
    }

    pub fn new_ff(clock: VarId, reset: VarId, statements: Vec<Statement>) -> Self {
        Self::Ff(FfDeclaration {
            clock,
            reset,
            statements,
        })
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Declaration::Null)
    }
}

impl fmt::Display for Declaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Declaration::Comb(x) => x.fmt(f),
            Declaration::Ff(x) => x.fmt(f),
            Declaration::Inst(x) => x.fmt(f),
            Declaration::Null => "".fmt(f),
        }
    }
}

#[derive(Clone)]
pub struct CombDeclaration {
    pub statements: Vec<Statement>,
}

impl fmt::Display for CombDeclaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = "comb {\n".to_string();

        for x in &self.statements {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct FfDeclaration {
    pub clock: VarId,
    pub reset: VarId,
    pub statements: Vec<Statement>,
}

impl fmt::Display for FfDeclaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("ff ({}, {}) {{\n", self.clock, self.reset);

        for x in &self.statements {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct InstDeclaration {
    pub name: StrId,
    pub component: Component,
}

impl fmt::Display for InstDeclaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("inst {} {{\n", self.name);

        let text = format!("{}\n", self.component);
        ret.push_str(&indent_all_by(2, text));

        ret.push('}');
        ret.fmt(f)
    }
}
