use crate::AnalyzerError;
use crate::conv::Context;
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::ff_table::AssignTarget;
use crate::ir::utils::{allow_missing_reset_statement, has_cond_type};
use crate::ir::{
    Comptime, Expression, FfTable, FunctionCall, Op, SystemFunctionCall, Type, VarId, VarIndex,
    VarPath, VarSelect,
};
use crate::symbol::Affiliation;
use crate::value::{Value, ValueBigUint};
use indent::indent_all_by;
use std::borrow::Cow;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Default)]
pub struct StatementBlock(pub Vec<Statement>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ControlFlow {
    Continue,
    Break,
}

#[derive(Clone)]
pub enum Statement {
    Assign(AssignStatement),
    If(IfStatement),
    IfReset(IfResetStatement),
    Case(CaseStatement),
    For(ForStatement),
    SystemFunctionCall(Box<SystemFunctionCall>),
    FunctionCall(Box<FunctionCall>),
    TbMethodCall(TbMethodCall),
    Break,
    Unsupported(TokenRange),
    Null,
}

#[derive(Clone)]
pub struct ForStatement {
    pub var_id: VarId,
    pub var_name: StrId,
    pub var_type: Type,
    pub range: ForRange,
    pub body: Vec<Statement>,
    pub token: TokenRange,
}

#[derive(Clone, Debug)]
pub enum ForBound {
    Const(usize),
    Expression(Box<Expression>),
}

impl ForBound {
    pub fn eval_value(&self, context: &mut Context) -> Option<usize> {
        match self {
            Self::Const(x) => Some(*x),
            Self::Expression(exp) => {
                let exp = exp.as_ref().clone();
                exp.eval_value(context)?.to_usize()
            }
        }
    }
}

impl fmt::Display for ForBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForBound::Const(x) => x.fmt(f),
            ForBound::Expression(x) => write!(f, "{}", x.as_ref()),
        }
    }
}

/// Loop iteration range representation.
#[derive(Clone, Debug)]
pub enum ForRange {
    /// start..end with additive step (default step=1)
    Forward {
        start: ForBound,
        end: ForBound,
        inclusive: bool,
        step: usize,
    },
    /// (start..end).rev() with additive step (default step=1)
    Reverse {
        start: ForBound,
        end: ForBound,
        inclusive: bool,
        step: usize,
    },
    /// start..end with arbitrary step operator (e.g., step *= 2)
    Stepped {
        start: ForBound,
        end: ForBound,
        inclusive: bool,
        step: usize,
        op: Op,
    },
}

impl ForRange {
    pub fn eval_iter(&self, context: &mut Context) -> Option<Vec<usize>> {
        let limit = context.config.evaluate_size_limit;
        match self {
            ForRange::Forward {
                start,
                end,
                inclusive,
                step,
            } => {
                let start = start.eval_value(context)?;
                let end = end.eval_value(context)?;
                let end = if *inclusive { end + 1 } else { end };
                if end.saturating_sub(start) > limit {
                    return None;
                }
                if *step == 1 {
                    Some((start..end).collect())
                } else {
                    let mut ret = vec![];
                    let mut i = start;
                    while i < end {
                        ret.push(i);
                        i += step;
                    }
                    Some(ret)
                }
            }
            ForRange::Reverse {
                start,
                end,
                inclusive,
                step,
            } => {
                let start = start.eval_value(context)?;
                let end = end.eval_value(context)?;
                if end.saturating_sub(start) > limit {
                    return None;
                }
                if *step <= 1 {
                    if *inclusive {
                        Some((start..=end).rev().collect())
                    } else {
                        Some((start..end).rev().collect())
                    }
                } else {
                    // Descend from the high bound by `step`, matching the
                    // emitted `for (i = hi; i >= lo; i -= step)`.
                    let hi = if *inclusive {
                        end
                    } else {
                        end.saturating_sub(1)
                    };
                    let mut ret = vec![];
                    let mut i = hi as i64;
                    let lo = start as i64;
                    let step = *step as i64;
                    while i >= lo {
                        ret.push(i as usize);
                        i -= step;
                    }
                    Some(ret)
                }
            }
            ForRange::Stepped {
                start,
                end,
                inclusive,
                step,
                op,
            } => {
                let start = start.eval_value(context)?;
                let end = end.eval_value(context)?;
                let end = if *inclusive { end + 1 } else { end };
                let mut ret = vec![];
                let mut i = start;
                while i < end {
                    if ret.len() > limit {
                        return None;
                    }
                    ret.push(i);
                    let new_i = op.eval(i, *step);
                    if new_i == i {
                        break;
                    }
                    i = new_i;
                }
                Some(ret)
            }
        }
    }
}

#[derive(Clone)]
pub struct TbMethodCall {
    pub inst: StrId,
    pub method: TbMethod,
}

#[derive(Clone)]
pub enum TbMethod {
    ClockNext {
        count: Option<Expression>,
        period: Option<Expression>,
    },
    ResetAssert {
        clock: StrId,
        duration: Option<Expression>,
    },
}

impl Statement {
    pub fn is_null(&self) -> bool {
        matches!(self, Statement::Null)
    }

    pub fn eval_value(&self, context: &mut Context) -> ControlFlow {
        match self {
            Statement::Assign(x) => {
                x.eval_value(context);
                ControlFlow::Continue
            }
            Statement::If(x) => x.eval_value(context),
            Statement::IfReset(_) => ControlFlow::Continue,
            Statement::Case(x) => x.eval_value(context),
            Statement::For(x) => {
                if let Some(iter) = x.range.eval_iter(context) {
                    'outer: for i in iter {
                        if let Some(var) = context.variables.get_mut(&x.var_id)
                            && let Some(total_width) = x.var_type.total_width()
                        {
                            let val = Value::new(i as u64, total_width, x.var_type.signed);
                            var.set_value(&[], val, None);
                        }
                        for s in &x.body {
                            if s.eval_value(context) == ControlFlow::Break {
                                break 'outer;
                            }
                        }
                    }
                }
                ControlFlow::Continue
            }
            Statement::SystemFunctionCall(_) => ControlFlow::Continue,
            Statement::FunctionCall(x) => {
                x.eval_value(context);
                ControlFlow::Continue
            }
            Statement::TbMethodCall(_) => ControlFlow::Continue,
            Statement::Break => ControlFlow::Break,
            Statement::Unsupported(_) => ControlFlow::Continue,
            Statement::Null => ControlFlow::Continue,
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
            Statement::Case(x) => x.eval_assign(context, assign_table, assign_context, base_tables),
            Statement::SystemFunctionCall(x) => {
                x.eval_assign(context, assign_table, assign_context)
            }
            Statement::FunctionCall(x) => x.eval_assign(context, assign_table, assign_context),
            Statement::For(x) => {
                for s in &x.body {
                    s.eval_assign(context, assign_table, assign_context, base_tables);
                }
            }
            Statement::TbMethodCall(_) | Statement::Break => (),
            Statement::Unsupported(_) => (),
            Statement::Null => (),
        }
    }

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        match self {
            Statement::Assign(x) => x.gather_ff(context, table, decl),
            Statement::If(x) => x.gather_ff(context, table, decl),
            Statement::IfReset(x) => x.gather_ff(context, table, decl),
            Statement::Case(x) => x.gather_ff(context, table, decl),
            Statement::FunctionCall(x) => x.gather_ff(context, table, decl, None, true),
            Statement::SystemFunctionCall(x) => x.gather_ff(context, table, decl, true),
            Statement::For(x) => {
                for s in &x.body {
                    s.gather_ff(context, table, decl);
                }
            }
            Statement::TbMethodCall(_)
            | Statement::Break
            | Statement::Unsupported(_)
            | Statement::Null => (),
        }
    }

    pub fn gather_ff_comb_assign(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        match self {
            Statement::Assign(x) => x.gather_ff_comb_assign(context, table, decl),
            Statement::If(x) => {
                // Record the condition expression's reads so analyses
                // that distinguish comb vs ff context (e.g. comb-to-FF
                // hoist eligibility) see them.  `from_ff=false` keeps
                // these out of `is_ff` classification.
                x.cond.gather_ff(context, table, decl, None, false);
                for s in &x.true_side {
                    s.gather_ff_comb_assign(context, table, decl);
                }
                for s in &x.false_side {
                    s.gather_ff_comb_assign(context, table, decl);
                }
            }
            Statement::Case(x) => x.gather_ff_comb_assign(context, table, decl),
            Statement::IfReset(x) => {
                for s in &x.true_side {
                    s.gather_ff_comb_assign(context, table, decl);
                }
                for s in &x.false_side {
                    s.gather_ff_comb_assign(context, table, decl);
                }
            }
            Statement::FunctionCall(x) => x.gather_ff_comb_assign(context, table, decl),
            Statement::SystemFunctionCall(x) => x.gather_ff(context, table, decl, false),
            Statement::For(x) => {
                for s in &x.body {
                    s.gather_ff_comb_assign(context, table, decl);
                }
            }
            _ => (),
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        match self {
            Statement::Assign(x) => x.set_index(index),
            Statement::If(x) => x.set_index(index),
            Statement::IfReset(x) => x.set_index(index),
            Statement::Case(x) => x.set_index(index),
            Statement::SystemFunctionCall(_) => (),
            Statement::FunctionCall(x) => x.set_index(index),
            Statement::For(x) => {
                for s in &mut x.body {
                    s.set_index(index);
                }
            }
            Statement::TbMethodCall(_) | Statement::Break => (),
            Statement::Unsupported(_) => (),
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
            Statement::Case(x) => x.fmt(f),
            Statement::SystemFunctionCall(x) => format!("{x};").fmt(f),
            Statement::FunctionCall(x) => format!("{x};").fmt(f),
            Statement::TbMethodCall(x) => match &x.method {
                TbMethod::ClockNext { count, .. } => {
                    if let Some(c) = count {
                        write!(f, "{}.next({c});", x.inst)
                    } else {
                        write!(f, "{}.next();", x.inst)
                    }
                }
                TbMethod::ResetAssert { clock, duration } => {
                    if let Some(d) = duration {
                        write!(f, "{}.assert({clock}, {d});", x.inst)
                    } else {
                        write!(f, "{}.assert({clock});", x.inst)
                    }
                }
            },
            Statement::For(x) => {
                let range_op = if let ForRange::Reverse { .. } = &x.range {
                    "rev "
                } else {
                    ""
                };
                let (start, end, inclusive, step_info) = match &x.range {
                    ForRange::Forward {
                        start,
                        end,
                        inclusive,
                        step,
                    } => (
                        start,
                        end,
                        *inclusive,
                        if *step == 1 {
                            None
                        } else {
                            Some(format!("+= {step}"))
                        },
                    ),
                    ForRange::Reverse {
                        start,
                        end,
                        inclusive,
                        ..
                    } => (start, end, *inclusive, None),
                    ForRange::Stepped {
                        start,
                        end,
                        inclusive,
                        step,
                        op,
                    } => (start, end, *inclusive, Some(format!("{op}= {step}"))),
                };
                let dots = if inclusive { "..=" } else { ".." };
                if let Some(step_str) = step_info {
                    writeln!(
                        f,
                        "for {} in {range_op}{start}{dots}{end} step {step_str} {{",
                        x.var_name
                    )?;
                } else {
                    writeln!(f, "for {} in {range_op}{start}{dots}{end} {{", x.var_name)?;
                }
                for s in &x.body {
                    writeln!(f, "  {s}")?;
                }
                write!(f, "}}")
            }
            Statement::Break => "break;".fmt(f),
            Statement::Unsupported(_) => "/* unsupported */".fmt(f),
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
    pub comptime: Comptime,
    pub token: TokenRange,
}

impl AssignDestination {
    pub fn total_width(&self, context: &mut Context) -> Option<usize> {
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
            let is_index_const = self.index.is_const();
            let is_select_const = self.select.is_const();
            let is_const = is_index_const & is_select_const;

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
                ValueBigUint::gen_mask(width)
            } else {
                let Some((beg, end)) = self.select.eval_value(context, &variable.r#type, false)
                else {
                    return;
                };
                ValueBigUint::gen_mask_range(beg, end)
            };

            let mut errors = vec![];
            if let Some((beg, end)) = range {
                // `insert_assign` / `check_refered` bail out on arrays over
                // `array_limit`; iterating beg..=end just to hit that guard
                // is wasted work for any whole-array assignment.
                let skip_large_array =
                    variable.r#type.total_array().unwrap_or(0) > assign_table.array_limit;

                if !skip_large_array {
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
            }

            for e in errors {
                context.insert_error(e);
            }
        }
    }

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        if let Some(variable) = context.get_variable_info(self.id) {
            // Let-bound variables use blocking assignment (BA) semantics
            // and must not be registered as FF in the table.
            if variable.kind == crate::ir::VarKind::Let {
                return;
            }
            // Vars declared inside an always_ff block are procedural locals
            // (BA semantics) and must not be registered as FF either.
            if variable.affiliation == Affiliation::AlwaysFf {
                return;
            }
            if let Some(index) = self.index.eval_value(context) {
                if let Some(index) = variable.r#type.array.calc_index(&index) {
                    table.insert_assigned(self.id, index, decl);
                }
            } else if let Some(total_array) = variable.r#type.total_array() {
                for i in 0..total_array {
                    table.insert_assigned(self.id, i, decl);
                }
            }
        }
    }

    pub fn gather_ff_comb_assign(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        if let Some(variable) = context.get_variable_info(self.id) {
            if let Some(index) = self.index.eval_value(context) {
                if let Some(index) = variable.r#type.array.calc_index(&index) {
                    table.insert_assigned_comb(self.id, index, decl);
                }
            } else if let Some(total_array) = variable.r#type.total_array() {
                for i in 0..total_array {
                    table.insert_assigned_comb(self.id, i, decl);
                }
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

fn compute_assign_target(
    dst: Option<&AssignDestination>,
    context: &mut Context,
) -> Option<AssignTarget> {
    let dst = dst?;
    let var_info = context.get_variable_info(dst.id)?;
    let arr_idx = dst
        .index
        .eval_value(context)
        .and_then(|v| var_info.r#type.array.calc_index(&v));
    let mask = if let Some((beg, end)) = dst.select.eval_value(context, &var_info.r#type, false) {
        ValueBigUint::gen_mask_range(beg, end)
    } else if let Some(width) = var_info.total_width() {
        ValueBigUint::gen_mask(width)
    } else {
        crate::BigUint::default()
    };
    Some((dst.id, arr_idx, mask))
}

#[derive(Clone)]
pub struct AssignStatement {
    pub dst: Vec<AssignDestination>,
    pub width: Option<usize>,
    pub expr: Expression,
    pub token: TokenRange,
}

impl AssignStatement {
    pub fn eval_value(&self, context: &mut Context) {
        if let Some(value) = self.expr.eval_value(context) {
            // TODO multiple dst
            if let Some(index) = self.dst[0].index.eval_value(context)
                && let Some((beg, end)) =
                    self.dst[0]
                        .select
                        .eval_value(context, &self.dst[0].comptime.r#type, false)
                && let Some(variable) = context.variables.get_mut(&self.dst[0].id)
            {
                variable.set_value(&index, value, Some((beg, end)));
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

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        let assign_target = compute_assign_target(self.dst.first(), context);
        self.expr
            .gather_ff(context, table, decl, assign_target.as_ref(), true);
        for dst in &self.dst {
            dst.gather_ff(context, table, decl);
        }
    }

    pub fn gather_ff_comb_assign(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        let assign_target = compute_assign_target(self.dst.first(), context);
        self.expr
            .gather_ff(context, table, decl, assign_target.as_ref(), false);
        for dst in &self.dst {
            dst.gather_ff_comb_assign(context, table, decl);
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
    pub fn eval_value(&self, context: &mut Context) -> ControlFlow {
        if let Some(cond) = self.cond.eval_value(context) {
            if cond.to_usize().unwrap_or(0) != 0 {
                for stmt in &self.true_side {
                    if stmt.eval_value(context) == ControlFlow::Break {
                        return ControlFlow::Break;
                    }
                }
            } else {
                for stmt in &self.false_side {
                    if stmt.eval_value(context) == ControlFlow::Break {
                        return ControlFlow::Break;
                    }
                }
            }
        }
        ControlFlow::Continue
    }

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

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        self.cond.gather_ff(context, table, decl, None, true);
        for x in &self.true_side {
            x.gather_ff(context, table, decl);
        }
        for x in &self.false_side {
            x.gather_ff(context, table, decl);
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

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        for x in &self.true_side {
            x.gather_ff(context, table, decl);
        }
        for x in &self.false_side {
            x.gather_ff(context, table, decl);
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

/// `case <expr> { ... }` as a flat list of arms.  Multiple patterns
/// within an arm are OR-ed.  `switch { ... }` and hand-written nested
/// `if-else` stay as `Statement::If`.
#[derive(Clone)]
pub struct CaseStatement {
    pub arms: Vec<CaseArm>,
    pub default: Vec<Statement>,
    pub case_target: Box<Expression>,
    pub token: TokenRange,
}

#[derive(Clone)]
pub struct CaseArm {
    pub patterns: Vec<CasePattern>,
    pub body: Vec<Statement>,
    pub token: TokenRange,
}

#[derive(Clone)]
pub enum CasePattern {
    /// `case_target === value` (case-equality).
    Eq(Box<Expression>),
    /// `lo <= case_target` and `case_target < hi` (or `<= hi` when `inclusive`).
    Range {
        lo: Box<Expression>,
        hi: Box<Expression>,
        inclusive: bool,
    },
}

impl CasePattern {
    /// `None` when either side is non-const.
    fn matches(&self, target: &Value, context: &mut Context) -> Option<bool> {
        let target_n = target.to_usize()?;
        match self {
            CasePattern::Eq(e) => {
                let v = e.eval_value(context)?.to_usize()?;
                Some(target_n == v)
            }
            CasePattern::Range { lo, hi, inclusive } => {
                let lo_n = lo.eval_value(context)?.to_usize()?;
                let hi_n = hi.eval_value(context)?.to_usize()?;
                Some(
                    lo_n <= target_n
                        && if *inclusive {
                            target_n <= hi_n
                        } else {
                            target_n < hi_n
                        },
                )
            }
        }
    }

    fn lower_to_cond(&self, target: &Expression) -> Expression {
        let comptime = Box::new(Comptime::create_unknown(self.token_range()));
        match self {
            CasePattern::Eq(val) => Expression::Binary(
                Box::new(target.clone()),
                Op::EqWildcard,
                val.clone(),
                comptime,
            ),
            CasePattern::Range { lo, hi, inclusive } => {
                let lo_cond = Expression::Binary(
                    lo.clone(),
                    Op::LessEq,
                    Box::new(target.clone()),
                    comptime.clone(),
                );
                let hi_op = if *inclusive { Op::LessEq } else { Op::Less };
                let hi_cond = Expression::Binary(
                    Box::new(target.clone()),
                    hi_op,
                    hi.clone(),
                    comptime.clone(),
                );
                Expression::Binary(Box::new(lo_cond), Op::LogicAnd, Box::new(hi_cond), comptime)
            }
        }
    }

    fn token_range(&self) -> TokenRange {
        match self {
            CasePattern::Eq(e) => e.token_range(),
            CasePattern::Range { lo, .. } => lo.token_range(),
        }
    }

    fn for_each_expr(&self, mut f: impl FnMut(&Expression)) {
        match self {
            CasePattern::Eq(e) => f(e),
            CasePattern::Range { lo, hi, .. } => {
                f(lo);
                f(hi);
            }
        }
    }

    fn for_each_expr_mut(&mut self, mut f: impl FnMut(&mut Expression)) {
        match self {
            CasePattern::Eq(e) => f(e),
            CasePattern::Range { lo, hi, .. } => {
                f(lo);
                f(hi);
            }
        }
    }
}

impl CaseStatement {
    pub fn eval_value(&self, context: &mut Context) -> ControlFlow {
        let Some(tgt) = self.case_target.eval_value(context) else {
            return ControlFlow::Continue;
        };
        for arm in &self.arms {
            let mut matched = false;
            let mut undecided = false;
            for pat in &arm.patterns {
                match pat.matches(&tgt, context) {
                    Some(true) => {
                        matched = true;
                        break;
                    }
                    Some(false) => {}
                    None => undecided = true,
                }
            }
            if matched {
                for stmt in &arm.body {
                    if stmt.eval_value(context) == ControlFlow::Break {
                        return ControlFlow::Break;
                    }
                }
                return ControlFlow::Continue;
            }
            if undecided {
                return ControlFlow::Continue;
            }
        }
        for stmt in &self.default {
            if stmt.eval_value(context) == ControlFlow::Break {
                return ControlFlow::Break;
            }
        }
        ControlFlow::Continue
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
        base_tables: &[&AssignTable],
    ) {
        // Detach `refernced` before borrowing `assign_table` for base_tables.
        let mut prev_referenced = std::mem::take(&mut assign_table.refernced);

        let base_tables = if assign_table.table.is_empty() {
            Cow::Borrowed(base_tables)
        } else {
            let mut bt = base_tables.to_vec();
            bt.push(assign_table);
            Cow::Owned(bt)
        };

        // `refernced` is rotated through branches so reads accumulate
        // across the whole case, not just within one arm.
        let n_branches = self.arms.len() + 1;
        let mut branch_tables: Vec<AssignTable> = Vec::with_capacity(n_branches);
        for arm in &self.arms {
            let mut t = AssignTable::new(context);
            t.refernced = prev_referenced;
            for s in &arm.body {
                s.eval_assign(context, &mut t, assign_context, &base_tables);
            }
            prev_referenced = std::mem::take(&mut t.refernced);
            branch_tables.push(t);
        }
        let mut default_table = AssignTable::new(context);
        default_table.refernced = prev_referenced;
        for s in &self.default {
            s.eval_assign(context, &mut default_table, assign_context, &base_tables);
        }
        let final_referenced = std::mem::take(&mut default_table.refernced);
        branch_tables.push(default_table);

        if assign_context.is_comb() && !has_cond_type(&self.token) {
            let refs: Vec<&AssignTable> = branch_tables.iter().collect();
            AssignTable::check_uncoverd_n_way(context, &refs, &base_tables);
        }

        let mut acc = branch_tables.pop().expect("default_table");
        for mut t in branch_tables.into_iter().rev() {
            t.merge_by_or(context, &mut acc, false);
            acc = t;
        }
        assign_table.merge_by_or(context, &mut acc, false);
        assign_table.refernced = final_referenced;
    }

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        self.case_target.gather_ff(context, table, decl, None, true);
        for arm in &self.arms {
            for p in &arm.patterns {
                p.for_each_expr(|e| e.gather_ff(context, table, decl, None, true));
            }
            for s in &arm.body {
                s.gather_ff(context, table, decl);
            }
        }
        for s in &self.default {
            s.gather_ff(context, table, decl);
        }
    }

    pub fn gather_ff_comb_assign(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        // `from_ff=false` so the scrutinee/pattern reads stay out of FF
        // classification while remaining visible to comb-vs-ff analyses.
        self.case_target
            .gather_ff(context, table, decl, None, false);
        for arm in &self.arms {
            for p in &arm.patterns {
                p.for_each_expr(|e| e.gather_ff(context, table, decl, None, false));
            }
            for s in &arm.body {
                s.gather_ff_comb_assign(context, table, decl);
            }
        }
        for s in &self.default {
            s.gather_ff_comb_assign(context, table, decl);
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        self.case_target.set_index(index);
        for arm in &mut self.arms {
            for p in &mut arm.patterns {
                p.for_each_expr_mut(|e| e.set_index(index));
            }
            for s in &mut arm.body {
                s.set_index(index);
            }
        }
        for s in &mut self.default {
            s.set_index(index);
        }
    }

    /// Expand back to a nested `if/else` chain for consumers that only
    /// understand `Statement::If`.
    pub fn lower_to_nested_if(&self) -> Vec<Statement> {
        let mut tail: Vec<Statement> = self.default.clone();
        for arm in self.arms.iter().rev() {
            let cond = arm_cond(&self.case_target, &arm.patterns);
            let if_stmt = IfStatement {
                cond,
                true_side: arm.body.clone(),
                false_side: std::mem::take(&mut tail),
                token: arm.token,
            };
            tail = vec![Statement::If(if_stmt)];
        }
        tail
    }
}

/// OR-combine an arm's patterns into a single boolean expression.
fn arm_cond(target: &Expression, patterns: &[CasePattern]) -> Expression {
    let mut iter = patterns.iter();
    let first = iter
        .next()
        .expect("CaseArm must have at least one pattern")
        .lower_to_cond(target);
    iter.fold(first, |acc, p| {
        let next = p.lower_to_cond(target);
        let comptime = Box::new(Comptime::create_unknown(next.token_range()));
        Expression::Binary(Box::new(acc), Op::LogicOr, Box::new(next), comptime)
    })
}

impl fmt::Display for CaseStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("case {} {{\n", self.case_target);
        for arm in &self.arms {
            let header = arm
                .patterns
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let mut body = format!("{header}: {{\n");
            for s in &arm.body {
                let text = format!("{}\n", s);
                body.push_str(&indent_all_by(2, text));
            }
            body.push_str("}\n");
            ret.push_str(&indent_all_by(2, body));
        }
        if !self.default.is_empty() {
            let mut def = "default: {\n".to_string();
            for s in &self.default {
                let text = format!("{}\n", s);
                def.push_str(&indent_all_by(2, text));
            }
            def.push_str("}\n");
            ret.push_str(&indent_all_by(2, def));
        }
        ret.push('}');
        ret.fmt(f)
    }
}

impl fmt::Display for CasePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CasePattern::Eq(e) => e.fmt(f),
            CasePattern::Range { lo, hi, inclusive } => {
                let op = if *inclusive { "..=" } else { ".." };
                write!(f, "{lo}{op}{hi}")
            }
        }
    }
}
