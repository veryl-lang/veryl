use crate::conv::Context;
use crate::ir::assign_table::AssignTable;
use crate::ir::bigint::gen_mask_range;
use crate::ir::{AssignDestination, Component, Expression, Statement, VarId};
use crate::{AnalyzerError, HashMap};
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

    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        match self {
            Declaration::Comb(x) => x.eval_assign(context, assign_table),
            Declaration::Ff(x) => x.eval_assign(context, assign_table),
            Declaration::Inst(x) => x.eval_assign(context, assign_table),
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
                if let Some(index) = dst.index.eval(&context.variables) {
                    let variable = context.variables.get(&dst.id).unwrap();
                    if let Some((beg, end)) =
                        dst.select.eval(&context.variables, &variable.r#type.width)
                    {
                        let mask = gen_mask_range(beg, end);
                        let (success, tokens) =
                            assign_table.insert(variable, index, mask, dst.token);
                        if !success {
                            context.insert_error(AnalyzerError::multiple_assignment(
                                &variable.path.to_string(),
                                &dst.token,
                                &tokens,
                            ));
                        }
                    }
                }
            }
        }
    }
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
