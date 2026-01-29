use crate::AnalyzerError;
use crate::conv::{Context, EvalContext};
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::utils::{allow_missing_reset_statement, has_cond_type};
use crate::ir::{
    Comptime, Expression, FunctionCall, SystemFunctionCall, VarId, VarIndex, VarPath, VarSelect,
};
use crate::value::{gen_mask, gen_mask_range};
use indent::indent_all_by;
use std::borrow::Cow;
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

    pub fn eval_value<T: EvalContext>(&self, context: &mut T) {
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
        assign_context: AssignContext,
        base_tables: &[&AssignTable],
    ) {
        match self {
            Statement::Assign(x) => x.eval_assign(context, assign_table, assign_context),
            Statement::If(x) => x.eval_assign(context, assign_table, assign_context, base_tables),
            Statement::IfReset(x) => x.eval_assign(context, assign_table, base_tables),
            Statement::SystemFunctionCall(x) => {
                x.eval_assign(context, assign_table, assign_context)
            }
            Statement::FunctionCall(x) => x.eval_assign(context, assign_table, assign_context),
            Statement::Null => (),
        }
    }

    pub fn eval_step<T: EvalContext>(&self, context: &mut T, reset: bool) {
        match self {
            Statement::Assign(x) => x.eval_step(context),
            Statement::If(x) => x.eval_step(context),
            Statement::IfReset(x) => x.eval_step(context, reset),
            Statement::SystemFunctionCall(_) => (),
            Statement::FunctionCall(x) => x.eval_step(context),
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

pub enum IndexOrRaw {
    Index(Option<Vec<usize>>),
    Raw(usize, Option<usize>),
}

impl IndexOrRaw {
    pub fn is_index(&self) -> bool {
        matches!(self, IndexOrRaw::Index(_))
    }

    pub fn is_raw(&self) -> bool {
        matches!(self, IndexOrRaw::Raw(_, _))
    }
}

#[derive(Clone, Debug)]
pub enum AssignDestination {
    Default(AssignDestinationDefault),
    Opt(AssignDestinationOpt),
}

impl AssignDestination {
    pub fn id(&self) -> VarId {
        match self {
            AssignDestination::Default(x) => x.id,
            AssignDestination::Opt(x) => x.id,
        }
    }

    pub fn path(&self) -> &VarPath {
        match self {
            AssignDestination::Default(x) => &x.path,
            AssignDestination::Opt(x) => &x.path,
        }
    }

    pub fn index(&self) -> &VarIndex {
        match self {
            AssignDestination::Default(x) => &x.index,
            AssignDestination::Opt(x) => &x.index,
        }
    }

    pub fn index_mut(&mut self) -> &mut VarIndex {
        match self {
            AssignDestination::Default(x) => &mut x.index,
            AssignDestination::Opt(x) => &mut x.index,
        }
    }

    pub fn select(&self) -> &VarSelect {
        match self {
            AssignDestination::Default(x) => &x.select,
            AssignDestination::Opt(x) => &x.select,
        }
    }

    pub fn select_mut(&mut self) -> &mut VarSelect {
        match self {
            AssignDestination::Default(x) => &mut x.select,
            AssignDestination::Opt(x) => &mut x.select,
        }
    }

    pub fn comptime(&self) -> &Comptime {
        match self {
            AssignDestination::Default(x) => &x.comptime,
            AssignDestination::Opt(x) => &x.comptime,
        }
    }

    pub fn token(&self) -> &TokenRange {
        match self {
            AssignDestination::Default(x) => &x.token,
            AssignDestination::Opt(x) => &x.token,
        }
    }

    pub fn total_width(&self, context: &mut Context) -> Option<usize> {
        match self {
            AssignDestination::Default(x) => x.total_width(context),
            AssignDestination::Opt(x) => x.total_width(context),
        }
    }

    pub fn eval_comptime<T: EvalContext>(&mut self, context: &mut T) {
        match self {
            AssignDestination::Default(x) => {
                let is_const_index = x.index.is_const(context);
                let index = x.index.eval_value(context);
                if is_const_index
                    && let Some(variable) = context.variables().get(&x.id)
                    && let Some(index) = index.as_ref()
                    && x.select.is_empty()
                    && let Some(raw_index) = variable.get_raw_index(index)
                {
                    *self = AssignDestination::Opt(AssignDestinationOpt {
                        id: x.id,
                        path: x.path.clone(),
                        index: x.index.clone(),
                        select: x.select.clone(),
                        comptime: x.comptime.clone(),
                        token: x.token,
                        raw_index,
                        total_width: x.total_width(context),
                    });
                }
            }
            _ => (),
        }
    }

    pub fn eval_index<T: EvalContext>(&self, context: &mut T) -> IndexOrRaw {
        match self {
            AssignDestination::Default(x) => IndexOrRaw::Index(x.index.eval_value(context)),
            AssignDestination::Opt(x) => IndexOrRaw::Raw(x.raw_index, x.total_width.clone()),
        }
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        match self {
            AssignDestination::Default(x) => x.eval_assign(context, assign_table, assign_context),
            AssignDestination::Opt(x) => x.eval_assign(context, assign_table, assign_context),
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        match self {
            AssignDestination::Default(x) => x.set_index(index),
            AssignDestination::Opt(x) => x.set_index(index),
        }
    }
}

impl fmt::Display for AssignDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssignDestination::Default(x) => x.fmt(f),
            AssignDestination::Opt(x) => x.fmt(f),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AssignDestinationDefault {
    pub id: VarId,
    pub path: VarPath,
    pub index: VarIndex,
    pub select: VarSelect,
    pub comptime: Comptime,
    pub token: TokenRange,
}

impl AssignDestinationDefault {
    pub fn total_width<T: EvalContext>(&self, context: &mut T) -> Option<usize> {
        let (beg, end) = self
            .select
            .eval_value(context, &self.comptime.r#type, false)?;
        Some(beg - end + 1)
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        if let Some(variable) = context.get_variable_info(self.id) {
            let is_index_const = self.index.is_const(context);
            let is_select_const = self.select.is_const(context);
            let is_const = is_index_const & is_select_const;

            // If index is not const, assign to the whole array
            let range = if !is_index_const {
                variable.r#type.array.calc_range(&[])
            } else {
                let Some(index) = self.index.eval_value(context) else {
                    return;
                };
                variable.r#type.array.calc_range(&index)
            };

            // If select is not const, assign to the whole width
            let mask = if !is_select_const {
                let Some(width) = variable.total_width() else {
                    return;
                };
                gen_mask(width)
            } else {
                let Some((beg, end)) = self.select.eval_value(context, &variable.r#type, false)
                else {
                    return;
                };
                gen_mask_range(beg, end)
            };

            let mut errors = vec![];
            if let Some((beg, end)) = range {
                for i in beg..=end {
                    let index = VarIndex::from_index(i, &variable.r#type.array);
                    if let Some(index) = index.eval_value(context) {
                        if assign_context.is_comb()
                            && assign_table.check_refered(&variable.id, &index, &mask)
                        {
                            let mut text = variable.path.to_string();
                            for i in &index {
                                text.push_str(&format!("[{i}]"));
                            }
                            // ignore `#[allow(unassign_variable)]` attribute
                            errors.push(AnalyzerError::unassign_variable(&text, &self.token));
                        }

                        let maybe = !is_const | assign_context.is_system_verilog();
                        let _ = assign_table.insert_assign(
                            &variable,
                            index,
                            mask.clone(),
                            maybe,
                            self.token,
                        );
                    }
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

impl fmt::Display for AssignDestinationDefault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = format!("{}{}{}", self.id, self.index, self.select);
        ret.fmt(f)
    }
}

#[derive(Clone, Debug, Default)]
pub struct AssignDestinationOpt {
    pub id: VarId,
    pub path: VarPath,
    pub index: VarIndex,
    pub select: VarSelect,
    pub comptime: Comptime,
    pub token: TokenRange,
    pub raw_index: usize,
    pub total_width: Option<usize>,
}

impl AssignDestinationOpt {
    pub fn total_width(&self, _context: &mut Context) -> Option<usize> {
        self.comptime.r#type.total_width()
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        if let Some(variable) = context.get_variable_info(self.id) {
            let Some(width) = variable.total_width() else {
                return;
            };
            let mask = gen_mask(width);

            let mut errors = vec![];
            let index = VarIndex::from_index(self.raw_index, &variable.r#type.array);
            if let Some(index) = index.eval_value(context).as_ref() {
                if assign_context.is_comb()
                    && assign_table.check_refered(&variable.id, index, &mask)
                {
                    let mut text = variable.path.to_string();
                    for i in index {
                        text.push_str(&format!("[{i}]"));
                    }
                    // ignore `#[allow(unassign_variable)]` attribute
                    errors.push(AnalyzerError::unassign_variable(&text, &self.token));
                }

                let maybe = assign_context.is_system_verilog();
                let _ = assign_table.insert_assign(
                    &variable,
                    index.clone(),
                    mask.clone(),
                    maybe,
                    self.token,
                );
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

impl fmt::Display for AssignDestinationOpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = format!("opt {}[{}]", self.id, self.raw_index);
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct AssignStatement {
    pub dst: Vec<AssignDestination>,
    pub width: Option<usize>,
    pub expr: Expression,
    pub token: TokenRange,
}

impl AssignStatement {
    pub fn eval_value<T: EvalContext>(&self, context: &mut T) {
        // TODO multiple dst
        let dst = &self.dst[0];

        //crate::stopwatch::start("expr");
        let value = self.expr.eval_value(context, self.width);
        //crate::stopwatch::stop("expr");
        if let Some(value) = value {
            //crate::stopwatch::start("index");
            let index = dst.eval_index(context);
            //crate::stopwatch::stop("index");

            //crate::stopwatch::start("select");
            let select = if index.is_index() {
                dst.select()
                    .eval_value(context, &dst.comptime().r#type, false)
            } else {
                None
            };
            //crate::stopwatch::stop("select");

            //crate::stopwatch::start("get_var");
            let variable = context.variables().get_mut(&dst.id());
            //crate::stopwatch::stop("get_var");
            if let Some(variable) = variable {
                //crate::stopwatch::start("set_var");
                match index {
                    IndexOrRaw::Index(index) => {
                        if let Some(index) = index {
                            if let Some((beg, end)) = select {
                                variable.set_value(&index, value.into_owned(), Some((beg, end)));
                            }
                        }
                    }
                    IndexOrRaw::Raw(raw_index, total_width) => {
                        variable.set_value_from_raw_index(
                            raw_index,
                            value.into_owned(),
                            None,
                            total_width,
                        );
                    }
                }
                //crate::stopwatch::stop("set_var");
            }
        }
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        self.expr.eval_assign(context, assign_table, assign_context);
        for dst in &self.dst {
            dst.eval_assign(context, assign_table, assign_context);
        }
    }

    pub fn eval_step<T: EvalContext>(&self, context: &mut T) {
        self.eval_value(context);
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
        assign_context: AssignContext,
        base_tables: &[&AssignTable],
    ) {
        let mut true_table = AssignTable::new(context);
        let mut false_table = AssignTable::new(context);

        std::mem::swap(&mut true_table.refernced, &mut assign_table.refernced);

        let base_tables = if assign_table.table.is_empty() {
            Cow::Borrowed(base_tables)
        } else {
            let mut base_tables = base_tables.to_vec();
            base_tables.push(assign_table);
            Cow::Owned(base_tables)
        };

        for x in &self.true_side {
            x.eval_assign(context, &mut true_table, assign_context, &base_tables);
        }

        std::mem::swap(&mut false_table.refernced, &mut true_table.refernced);
        for x in &self.false_side {
            x.eval_assign(context, &mut false_table, assign_context, &base_tables);
        }

        if assign_context.is_comb() && !has_cond_type(&self.token) {
            true_table.check_uncoverd(context, &false_table, &base_tables);
        }

        true_table.merge_by_or(context, &mut false_table, false);
        assign_table.merge_by_or(context, &mut true_table, false);
        std::mem::swap(&mut assign_table.refernced, &mut false_table.refernced);
    }

    pub fn eval_step<T: EvalContext>(&self, context: &mut T) {
        let cond = self.cond.eval_value(context, None);

        if let Some(cond) = cond {
            if cond.to_usize() != 0 {
                for x in &self.true_side {
                    x.eval_step(context, false);
                }
            } else {
                for x in &self.false_side {
                    x.eval_step(context, false);
                }
            }
        } else {
            // TODO eval failed
        }
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
    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        base_tables: &[&AssignTable],
    ) {
        let mut true_table = AssignTable::new(context);
        let mut false_table = AssignTable::new(context);

        std::mem::swap(&mut true_table.refernced, &mut assign_table.refernced);

        let mut base_tables = base_tables.to_vec();
        base_tables.push(assign_table);

        for x in &self.true_side {
            x.eval_assign(context, &mut true_table, AssignContext::Ff, &base_tables);
        }

        std::mem::swap(&mut false_table.refernced, &mut true_table.refernced);
        for x in &self.false_side {
            x.eval_assign(context, &mut false_table, AssignContext::Ff, &base_tables);
        }

        if !allow_missing_reset_statement(&self.token) {
            true_table.check_missing_reset(context, &false_table);
        }

        true_table.merge_by_or(context, &mut false_table, false);
        assign_table.merge_by_or(context, &mut true_table, false);
        std::mem::swap(&mut assign_table.refernced, &mut false_table.refernced);
    }

    pub fn eval_step<T: EvalContext>(&self, context: &mut T, reset: bool) {
        if reset {
            for x in &self.true_side {
                x.eval_step(context, false);
            }
        } else {
            for x in &self.false_side {
                x.eval_step(context, false);
            }
        }
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
