use crate::HashMap;
use crate::conv::Context;
use crate::ir::assign_table::AssignTable;
use crate::ir::{AssignDestination, Component, Expression, Statement, VarId};
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
    Initial(InitialDeclaration),
    Final(FinalDeclaration),
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

    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        match self {
            Declaration::Comb(x) => x.eval_assign(context, assign_table),
            Declaration::Ff(x) => x.eval_assign(context, assign_table),
            Declaration::Inst(x) => x.eval_assign(context, assign_table),
            Declaration::Initial(x) => x.eval_assign(context, assign_table),
            Declaration::Final(x) => x.eval_assign(context, assign_table),
            Declaration::Null => (),
        }
    }
}

impl fmt::Display for Declaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Declaration::Comb(x) => x.fmt(f),
            Declaration::Ff(x) => x.fmt(f),
            Declaration::Inst(x) => x.fmt(f),
            Declaration::Initial(x) => x.fmt(f),
            Declaration::Final(x) => x.fmt(f),
            Declaration::Null => "".fmt(f),
        }
    }
}

#[derive(Clone)]
pub struct CombDeclaration {
    pub statements: Vec<Statement>,
}

impl CombDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, true);
        }
    }
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

impl FfDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, false);
        }
    }
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
    pub inputs: HashMap<VarId, Expression>,
    pub outputs: HashMap<VarId, Vec<AssignDestination>>,
    pub component: Component,
}

impl InstDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for dsts in self.outputs.values() {
            for dst in dsts {
                dst.eval_assign(context, assign_table, false);
            }
        }

        if let Component::SystemVerilog(x) = &self.component {
            for x in &x.connects {
                assign_table.maybe_assigned.push(x.clone());
            }
        }
    }
}

impl fmt::Display for InstDeclaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("inst {} (\n", self.name);

        for (id, expr) in &self.inputs {
            let text = format!("{} <- {};\n", id, expr);
            ret.push_str(&indent_all_by(2, text));
        }

        for (id, dst) in &self.outputs {
            let mut text = format!("{} -> ", id);
            if dst.len() == 1 {
                text.push_str(&format!("{}", dst[0]));
            } else {
                text.push_str(&format!("{{{}", dst[0]));
                for d in &dst[1..] {
                    text.push_str(&format!(", {}", d));
                }
                text.push_str("}}");
            }
            text.push_str(";\n");
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push_str(") {\n");

        let text = format!("{}\n", self.component);
        ret.push_str(&indent_all_by(2, text));

        ret.push('}');
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct InitialDeclaration {
    pub statements: Vec<Statement>,
}

impl InitialDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, true);
        }
    }
}

impl fmt::Display for InitialDeclaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = "initial {\n".to_string();

        for x in &self.statements {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct FinalDeclaration {
    pub statements: Vec<Statement>,
}

impl FinalDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, true);
        }
    }
}

impl fmt::Display for FinalDeclaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = "final {\n".to_string();

        for x in &self.statements {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret.fmt(f)
    }
}
