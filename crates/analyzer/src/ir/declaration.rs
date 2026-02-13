use crate::conv::Context;
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::{
    AssignDestination, Component, Comptime, Expression, Statement, VarId, VarIndex, VarSelect,
};
use indent::indent_all_by;
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
    Ff(Box<FfDeclaration>),
    Inst(Box<InstDeclaration>),
    Initial(InitialDeclaration),
    Final(FinalDeclaration),
    Null,
}

impl Declaration {
    pub fn new_comb(statements: Vec<Statement>) -> Self {
        Self::Comb(CombDeclaration { statements })
    }

    pub fn new_ff(clock: FfClock, reset: Option<FfReset>, statements: Vec<Statement>) -> Self {
        Self::Ff(Box::new(FfDeclaration {
            clock,
            reset,
            statements,
        }))
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
        assign_table.refernced.clear();
    }

    pub fn to_string(&self, context: &Context) -> String {
        match self {
            Declaration::Comb(x) => x.to_string(context),
            Declaration::Ff(x) => x.to_string(context),
            Declaration::Inst(x) => x.to_string(context),
            Declaration::Initial(x) => x.to_string(context),
            Declaration::Final(x) => x.to_string(context),
            Declaration::Null => "".to_string(),
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
            x.eval_assign(context, assign_table, AssignContext::Comb, &[]);
        }
    }

    pub fn to_string(&self, context: &Context) -> String {
        let mut ret = "comb {\n".to_string();

        for x in &self.statements {
            let text = format!("{}\n", x.to_string(context));
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret
    }
}

#[derive(Clone)]
pub struct FfClock {
    pub id: VarId,
    pub index: VarIndex,
    pub select: VarSelect,
    pub comptime: Comptime,
}

impl FfClock {
    pub fn to_string(&self, context: &Context) -> String {
        format!(
            "{}{}{}",
            self.id,
            self.index.to_string(context),
            self.select.to_string(context)
        )
    }
}

#[derive(Clone)]
pub struct FfReset {
    pub id: VarId,
    pub index: VarIndex,
    pub select: VarSelect,
    pub comptime: Comptime,
}

impl FfReset {
    pub fn to_string(&self, context: &Context) -> String {
        format!(
            "{}{}{}",
            self.id,
            self.index.to_string(context),
            self.select.to_string(context)
        )
    }
}

#[derive(Clone)]
pub struct FfDeclaration {
    pub clock: FfClock,
    pub reset: Option<FfReset>,
    pub statements: Vec<Statement>,
}

impl FfDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, AssignContext::Ff, &[]);
        }
    }

    pub fn to_string(&self, context: &Context) -> String {
        let mut ret = if let Some(x) = &self.reset {
            format!(
                "ff ({}, {}) {{\n",
                self.clock.to_string(context),
                x.to_string(context)
            )
        } else {
            format!("ff ({}) {{\n", self.clock.to_string(context))
        };

        for x in &self.statements {
            let text = format!("{}\n", x.to_string(context));
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret
    }
}

#[derive(Clone)]
pub struct InstInput {
    pub id: Vec<VarId>,
    pub expr: Expression,
}

impl InstInput {
    pub fn to_string(&self, context: &Context) -> String {
        let mut ret = String::new();

        if self.id.len() == 1 {
            ret.push_str(&format!("{}", self.id[0]));
        } else if !self.id.is_empty() {
            ret.push_str(&format!("{{{}", self.id[0]));
            for x in &self.id[1..] {
                ret.push_str(&format!(", {}", x));
            }
            ret.push('}');
        }

        ret.push_str(&format!(" <- {}", self.expr.to_string(context)));

        ret
    }
}

#[derive(Clone)]
pub struct InstOutput {
    pub id: Vec<VarId>,
    pub dst: Vec<AssignDestination>,
}

impl InstOutput {
    pub fn to_string(&self, context: &Context) -> String {
        let mut ret = String::new();

        if self.id.len() == 1 {
            ret.push_str(&format!("{}", self.id[0]));
        } else {
            ret.push_str(&format!("{{{}", self.id[0]));
            for x in &self.id[1..] {
                ret.push_str(&format!(", {}", x));
            }
            ret.push('}');
        }

        ret.push_str(" -> ");

        if self.dst.len() == 1 {
            ret.push_str(&self.dst[0].to_string(context));
        } else if !self.dst.is_empty() {
            ret.push_str(&format!("{{{}", self.dst[0].to_string(context)));
            for d in &self.dst[1..] {
                ret.push_str(&format!(", {}", d.to_string(context)));
            }
            ret.push('}');
        }

        ret
    }
}

#[derive(Clone)]
pub struct InstDeclaration {
    pub name: StrId,
    pub inputs: Vec<InstInput>,
    pub outputs: Vec<InstOutput>,
    pub component: Component,
}

impl InstDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.outputs {
            for dst in &x.dst {
                dst.eval_assign(context, assign_table, AssignContext::Ff);
            }
        }

        if let Component::SystemVerilog(x) = &self.component {
            for dst in &x.connects {
                dst.eval_assign(context, assign_table, AssignContext::SystemVerilog);
            }
        }
    }

    pub fn to_string(&self, context: &Context) -> String {
        let mut ret = format!("inst {} (\n", self.name);

        for x in &self.inputs {
            let text = format!("{};\n", x.to_string(context));
            ret.push_str(&indent_all_by(2, text));
        }

        for x in &self.outputs {
            let text = format!("{};\n", x.to_string(context));
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push_str(") {\n");

        let text = format!("{}\n", self.component.to_string(context));
        ret.push_str(&indent_all_by(2, text));

        ret.push('}');
        ret
    }
}

#[derive(Clone)]
pub struct InitialDeclaration {
    pub statements: Vec<Statement>,
}

impl InitialDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, AssignContext::Initial, &[]);
        }
    }

    pub fn to_string(&self, context: &Context) -> String {
        let mut ret = "initial {\n".to_string();

        for x in &self.statements {
            let text = format!("{}\n", x.to_string(context));
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret
    }
}

#[derive(Clone)]
pub struct FinalDeclaration {
    pub statements: Vec<Statement>,
}

impl FinalDeclaration {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, AssignContext::Final, &[]);
        }
    }

    pub fn to_string(&self, context: &Context) -> String {
        let mut ret = "final {\n".to_string();

        for x in &self.statements {
            let text = format!("{}\n", x.to_string(context));
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret
    }
}
