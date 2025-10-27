use crate::conv::Context;
use crate::ir::assign_table::AssignTable;
use crate::ir::bigint::gen_mask_range;
use crate::ir::utils::{allow_missing_reset_statement, has_cond_type};
use crate::ir::{Expression, FunctionCall, SystemFunctionKind, Type, VarId, VarIndex, VarSelect};
use crate::{AnalyzerError, HashMap};
use indent::indent_all_by;
use std::fmt;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Default)]
pub struct StatementBlock(pub Vec<Statement>);

#[derive(Clone)]
pub enum Statement {
    Assign(AssignStatement),
    If(IfStatement),
    IfReset(IfResetStatement),
    SystemFunctionCall(SystemFunctionCall),
    FunctionCall(FunctionCall),
    Null,
}

impl Statement {
    pub fn is_null(&self) -> bool {
        matches!(self, Statement::Null)
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        in_comb: bool,
    ) {
        match self {
            Statement::Assign(x) => x.eval_assign(context, assign_table, in_comb),
            Statement::If(x) => x.eval_assign(context, assign_table, in_comb),
            Statement::IfReset(x) => x.eval_assign(context, assign_table),
            Statement::SystemFunctionCall(_) => (),
            Statement::FunctionCall(x) => x.eval_assign(context, assign_table, in_comb),
            Statement::Null => (),
        }
    }

    pub fn rename(&mut self, table: &HashMap<VarId, VarId>) {
        match self {
            Statement::Assign(x) => x.rename(table),
            Statement::If(x) => x.rename(table),
            Statement::IfReset(x) => x.rename(table),
            Statement::SystemFunctionCall(_) => (),
            Statement::FunctionCall(x) => x.rename(table),
            Statement::Null => (),
        }
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Assign(x) => x.fmt(f),
            Statement::If(x) => x.fmt(f),
            Statement::IfReset(x) => x.fmt(f),
            Statement::SystemFunctionCall(x) => format!("{x};").fmt(f),
            Statement::FunctionCall(x) => format!("{x};").fmt(f),
            Statement::Null => "".fmt(f),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssignDestination {
    pub id: VarId,
    pub index: VarIndex,
    pub select: VarSelect,
    pub r#type: Type,
    pub token: TokenRange,
}

impl AssignDestination {
    pub fn rename(&mut self, table: &HashMap<VarId, VarId>) {
        if let Some(x) = table.get(&self.id) {
            self.id = *x;
        }
    }
}

impl fmt::Display for AssignDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = format!("{}{}{}", self.id, self.index, self.select);
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct AssignStatement {
    pub dst: Vec<AssignDestination>,
    pub expr: Expression,
    pub token: TokenRange,
}

impl AssignStatement {
    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        in_comb: bool,
    ) {
        self.expr.eval_assign(context, assign_table, in_comb);
        for dst in &self.dst {
            if let Some(index) = dst.index.eval(&context.variables)
                && let Some(variable) = context.variables.get(&dst.id)
                && let Some((beg, end)) =
                    dst.select.eval(&context.variables, &variable.r#type.width)
            {
                let mask = gen_mask_range(beg, end);
                let (success, tokens) = assign_table.insert(variable, index, mask, self.token);
                if !success & !in_comb {
                    context.insert_error(AnalyzerError::multiple_assignment(
                        &variable.path.to_string(),
                        &self.token,
                        &tokens,
                    ));
                }
            }
        }
    }

    pub fn rename(&mut self, table: &HashMap<VarId, VarId>) {
        for dst in &mut self.dst {
            dst.rename(table);
        }
        self.expr.rename(table);
    }
}

impl fmt::Display for AssignStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        if self.dst.len() == 1 {
            ret.push_str(&format!("{} = {};", self.dst[0], self.expr));
        } else {
            ret.push_str(&format!("{{{}", self.dst[0]));
            for d in &self.dst[1..] {
                ret.push_str(&format!(", {}", d));
            }
            ret.push_str(&format!("}} = {};", self.expr));
        }
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct IfStatement {
    pub cond: Expression,
    pub true_side: Vec<Statement>,
    pub false_side: Vec<Statement>,
    pub token: TokenRange,
}

impl IfStatement {
    pub fn insert_leaf_false(&mut self, false_side: Vec<Statement>) {
        if self.false_side.is_empty() {
            self.false_side = false_side;
        } else if let Statement::If(x) = &mut self.false_side[0] {
            x.insert_leaf_false(false_side);
        }
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        in_comb: bool,
    ) {
        let mut true_table = AssignTable::default();
        let mut false_table = AssignTable::default();
        for x in &self.true_side {
            x.eval_assign(context, &mut true_table, in_comb);
        }
        for x in &self.false_side {
            x.eval_assign(context, &mut false_table, in_comb);
        }

        if in_comb && !has_cond_type(&self.token) {
            true_table.check_uncoverd(context, &false_table);
        }

        true_table.merge_by_or(context, &mut false_table, false);
        assign_table.merge_by_or(context, &mut true_table, !in_comb);
    }

    pub fn rename(&mut self, table: &HashMap<VarId, VarId>) {
        self.cond.rename(table);
        for x in &mut self.true_side {
            x.rename(table);
        }
        for x in &mut self.false_side {
            x.rename(table);
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
    pub token: TokenRange,
}

impl IfResetStatement {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        let mut true_table = AssignTable::default();
        let mut false_table = AssignTable::default();
        for x in &self.true_side {
            x.eval_assign(context, &mut true_table, false);
        }
        for x in &self.false_side {
            x.eval_assign(context, &mut false_table, false);
        }

        if !allow_missing_reset_statement(&self.token) {
            true_table.check_missing_reset(context, &false_table);
        }

        true_table.merge_by_or(context, &mut false_table, false);
        assign_table.merge_by_or(context, &mut true_table, true);
    }

    pub fn rename(&mut self, table: &HashMap<VarId, VarId>) {
        for x in &mut self.true_side {
            x.rename(table);
        }
        for x in &mut self.false_side {
            x.rename(table);
        }
    }
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

#[derive(Clone, Debug)]
pub struct SystemFunctionCall {
    pub kind: SystemFunctionKind,
    pub args: Vec<Expression>,
}

impl fmt::Display for SystemFunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut args = String::new();
        for y in &self.args {
            args.push_str(&format!("{y}, "));
        }
        let args = if args.is_empty() {
            &args
        } else {
            &args[0..args.len() - 2]
        };
        format!("{}({})", self.kind, args).fmt(f)
    }
}
