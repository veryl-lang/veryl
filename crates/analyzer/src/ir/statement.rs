use crate::ir::{Expression, Select, VarId, VarIndex};
use indent::indent_all_by;
use std::fmt;

#[derive(Clone)]
pub struct StatementBlock(pub Vec<Statement>);

#[derive(Clone)]
pub enum Statement {
    Assign(AssignStatement),
    If(IfStatement),
    IfReset(IfResetStatement),
    For(ForStatement),
    Null,
}

impl Statement {
    pub fn is_null(&self) -> bool {
        matches!(self, Statement::Null)
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Assign(x) => x.fmt(f),
            Statement::If(x) => x.fmt(f),
            Statement::IfReset(x) => x.fmt(f),
            Statement::For(x) => x.fmt(f),
            Statement::Null => "".fmt(f),
        }
    }
}

#[derive(Clone)]
pub struct AssignStatement {
    pub dst: VarId,
    pub index: VarIndex,
    pub select: Vec<Select>,
    pub expr: Expression,
}

impl fmt::Display for AssignStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut select = String::new();
        for s in &self.select {
            select.push_str(&s.to_string());
        }
        let ret = format!("{}{}{} = {};", self.dst, self.index, select, self.expr);
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct IfStatement {
    pub cond: Expression,
    pub true_side: Vec<Statement>,
    pub false_side: Vec<Statement>,
}

impl IfStatement {
    pub fn insert_leaf_false(&mut self, false_side: Vec<Statement>) {
        if self.false_side.is_empty() {
            self.false_side = false_side;
        } else if let Statement::If(x) = &mut self.false_side[0] {
            x.insert_leaf_false(false_side);
        }
    }
}

impl fmt::Display for IfStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("if {} {{\n", self.cond);
        for x in &self.true_side {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }
        ret.push('}');
        if !self.false_side.is_empty() {
            ret.push_str(" else {\n");
            for x in &self.false_side {
                let text = format!("{}\n", x);
                ret.push_str(&indent_all_by(2, text));
            }
            ret.push('}');
        }
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct IfResetStatement {
    pub true_side: Vec<Statement>,
    pub false_side: Vec<Statement>,
}

impl fmt::Display for IfResetStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = "if_reset {\n".to_string();
        for x in &self.true_side {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }
        ret.push('}');
        if !self.false_side.is_empty() {
            ret.push_str(" else {\n");
            for x in &self.false_side {
                let text = format!("{}\n", x);
                ret.push_str(&indent_all_by(2, text));
            }
            ret.push('}');
        }
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct ForStatement {}

impl fmt::Display for ForStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = "for".to_string();
        ret.fmt(f)
    }
}
