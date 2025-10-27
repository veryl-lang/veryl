use crate::AnalyzerError;
use crate::conv::Context;
use crate::ir::assign_table::AssignTable;
use crate::ir::utils::{allow_missing_reset_statement, has_cond_type};
use crate::ir::{
    Expression, FunctionCall, SystemFunctionCall, Type, VarId, VarIndex, VarPath, VarSelect,
};
use crate::value::gen_mask_range;
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
    FunctionCall(Box<FunctionCall>),
    Null,
}

impl Statement {
    pub fn is_null(&self) -> bool {
        matches!(self, Statement::Null)
    }

    pub fn eval_value(&self, context: &mut Context) {
        // TODO
        match self {
            Statement::Assign(x) => x.eval_value(context),
            Statement::If(_) => (),
            Statement::IfReset(_) => (),
            Statement::SystemFunctionCall(_) => (),
            Statement::FunctionCall(x) => {
                x.eval_value(context);
            }
            Statement::Null => (),
        }
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
            Statement::SystemFunctionCall(x) => x.eval_assign(context, assign_table, in_comb),
            Statement::FunctionCall(x) => x.eval_assign(context, assign_table, in_comb),
            Statement::Null => (),
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        match self {
            Statement::Assign(x) => x.set_index(index),
            Statement::If(x) => x.set_index(index),
            Statement::IfReset(x) => x.set_index(index),
            Statement::SystemFunctionCall(_) => (),
            Statement::FunctionCall(x) => x.set_index(index),
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
    pub path: VarPath,
    pub index: VarIndex,
    pub select: VarSelect,
    pub r#type: Type,
    pub token: TokenRange,
}

impl AssignDestination {
    pub fn total_width(&self, context: &mut Context) -> Option<usize> {
        let (beg, end) = self.select.eval_value(context, &self.r#type.width)?;
        Some(beg - end + 1)
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        in_comb: bool,
    ) {
        if let Some(index) = self.index.eval_value(context)
            && let Some(variable) = context.variables.get(&self.id).cloned()
            && let Some((beg, end)) = self.select.eval_value(context, &variable.r#type.width)
        {
            let mask = gen_mask_range(beg, end);
            let mut errors = vec![];

            // If index is empty, assign to the whole array
            if index.is_empty() {
                for i in 0..variable.r#type.total_array() {
                    let index = VarIndex::from_index(i, &variable.r#type.array);
                    if let Some(index) = index.eval_value(context) {
                        let (success, tokens) =
                            assign_table.insert(&variable, index, mask.clone(), self.token);
                        if !success & !in_comb {
                            errors.push(AnalyzerError::multiple_assignment(
                                &variable.path.to_string(),
                                &self.token,
                                &tokens,
                            ));
                        }
                    }
                }
            } else {
                let (success, tokens) = assign_table.insert(&variable, index, mask, self.token);
                if !success & !in_comb {
                    errors.push(AnalyzerError::multiple_assignment(
                        &variable.path.to_string(),
                        &self.token,
                        &tokens,
                    ));
                }
            }

            for e in errors {
                context.insert_error(e);
            }
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        self.index.add_prelude(index);
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
    pub width: usize,
    pub expr: Expression,
    pub token: TokenRange,
}

impl AssignStatement {
    pub fn eval_value(&self, context: &mut Context) {
        if let Some(value) = self.expr.eval_value(context, Some(self.width)) {
            // TODO multiple dst
            if let Some(index) = self.dst[0].index.eval_value(context)
                && let Some(variable) = context.variables.get_mut(&self.dst[0].id)
            {
                variable.set_value(&index, value);
            }
        }
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        in_comb: bool,
    ) {
        self.expr.eval_assign(context, assign_table, in_comb);
        for dst in &self.dst {
            dst.eval_assign(context, assign_table, in_comb);
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        for dst in &mut self.dst {
            dst.set_index(index);
        }
        self.expr.set_index(index);
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

    pub fn set_index(&mut self, index: &VarIndex) {
        self.cond.set_index(index);
        for x in &mut self.true_side {
            x.set_index(index);
        }
        for x in &mut self.false_side {
            x.set_index(index);
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

    pub fn set_index(&mut self, index: &VarIndex) {
        for x in &mut self.true_side {
            x.set_index(index);
        }
        for x in &mut self.false_side {
            x.set_index(index);
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
