//! Reports procedural `for` loops whose body can visibly mutate an input of
//! the continuation bound. Writes that retain true FF/NBA semantics are
//! intentionally ignored because they do not become visible until the loop
//! has completed.

use crate::analyzer_error::AnalyzerError;
use crate::attribute::{AllowItem, Attribute};
use crate::attribute_table;
use crate::conv::Context;
use crate::ir::{
    AssignDestination, CasePattern, Component, Declaration, Expression, Factor, ForBound, ForRange,
    ForStatement, FunctionCall, Ir, Module, Statement, SystemFunctionCall, SystemFunctionKind,
    VarId, VarIndex, VarSelect,
};
use crate::symbol::Affiliation;
use crate::value::ValueBigUint;
use crate::{BigUint, HashSet};

#[derive(Clone)]
struct Access {
    id: VarId,
    /// A flattened array element. `None` means any element may be accessed.
    index: Option<usize>,
    mask: BigUint,
    /// The write is deferred until NBA commit and is invisible to this loop.
    nba: bool,
}

pub fn check(ir: &Ir) -> Vec<AnalyzerError> {
    let mut errors = Vec::new();
    for component in &ir.components {
        if let Component::Module(module) = component {
            check_module(module, &mut errors);
        }
    }
    errors
}

fn check_module(module: &Module, errors: &mut Vec<AnalyzerError>) {
    if module.suppress_unassigned {
        return;
    }
    let mut context = Context::default();
    context.variables = module.variables.clone();
    context.functions = module.functions.clone();

    for declaration in &module.declarations {
        let statements = match declaration {
            Declaration::Comb(x) => Some((x.statements.as_slice(), false)),
            Declaration::Ff(x) => Some((x.statements.as_slice(), true)),
            Declaration::Initial(x) => Some((x.statements.as_slice(), false)),
            Declaration::Final(x) => Some((x.statements.as_slice(), false)),
            Declaration::Inst(_)
            | Declaration::External(_)
            | Declaration::Unsupported(_)
            | Declaration::Null => None,
        };
        if let Some((statements, from_ff)) = statements {
            check_statements(statements, module, &mut context, errors, from_ff);
        }
    }

    for function in module.functions.values() {
        for body in &function.functions {
            check_statements(&body.statements, module, &mut context, errors, false);
        }
    }
}

fn check_statements(
    statements: &[Statement],
    module: &Module,
    context: &mut Context,
    errors: &mut Vec<AnalyzerError>,
    from_ff: bool,
) {
    for statement in statements {
        match statement {
            Statement::If(x) => {
                check_statements(&x.true_side, module, context, errors, from_ff);
                check_statements(&x.false_side, module, context, errors, from_ff);
            }
            Statement::IfReset(x) => {
                check_statements(&x.true_side, module, context, errors, from_ff);
                check_statements(&x.false_side, module, context, errors, from_ff);
            }
            Statement::Case(x) => {
                for arm in &x.arms {
                    check_statements(&arm.body, module, context, errors, from_ff);
                }
                check_statements(&x.default, module, context, errors, from_ff);
            }
            Statement::For(x) => {
                check_for(x, module, context, errors, from_ff);
                check_statements(&x.body, module, context, errors, from_ff);
            }
            Statement::Assign(_)
            | Statement::FunctionCall(_)
            | Statement::SystemFunctionCall(_)
            | Statement::TbMethodCall(_)
            | Statement::Break
            | Statement::Unsupported(_)
            | Statement::Null => {}
        }
    }
}

fn check_for(
    statement: &ForStatement,
    module: &Module,
    context: &mut Context,
    errors: &mut Vec<AnalyzerError>,
    from_ff: bool,
) {
    let bound = match &statement.range {
        ForRange::Forward { end, .. } | ForRange::Stepped { end, .. } => end,
        ForRange::Reverse { start, .. } => start,
    };
    let ForBound::Expression(bound) = bound else {
        return;
    };

    let mut reads = Vec::new();
    let mut visited = HashSet::default();
    collect_expr_reads(bound, module, context, &mut reads, &mut visited);

    let mut writes = Vec::new();
    let mut visited = HashSet::default();
    collect_writes(
        &statement.body,
        module,
        context,
        &mut writes,
        &mut visited,
        from_ff,
    );

    if reads
        .iter()
        .any(|read| writes.iter().any(|write| accesses_conflict(read, write)))
        && !attribute_table::contains(
            &statement.token.beg,
            Attribute::Allow(AllowItem::MutableForBound),
        )
    {
        errors.push(AnalyzerError::mutable_for_bound(&statement.token));
    }
}

fn accesses_conflict(read: &Access, write: &Access) -> bool {
    if write.nba || read.id != write.id || (&read.mask & &write.mask) == BigUint::default() {
        return false;
    }
    if let (Some(read), Some(write)) = (read.index, write.index)
        && read != write
    {
        return false;
    }

    true
}

fn collect_writes(
    statements: &[Statement],
    module: &Module,
    context: &mut Context,
    writes: &mut Vec<Access>,
    visited_functions: &mut HashSet<VarId>,
    from_ff: bool,
) {
    for statement in statements {
        match statement {
            Statement::Assign(x) => {
                for destination in &x.dst {
                    push_destination(destination, context, writes, from_ff);
                }
            }
            Statement::If(x) => {
                collect_writes(
                    &x.true_side,
                    module,
                    context,
                    writes,
                    visited_functions,
                    from_ff,
                );
                collect_writes(
                    &x.false_side,
                    module,
                    context,
                    writes,
                    visited_functions,
                    from_ff,
                );
            }
            Statement::IfReset(x) => {
                collect_writes(
                    &x.true_side,
                    module,
                    context,
                    writes,
                    visited_functions,
                    from_ff,
                );
                collect_writes(
                    &x.false_side,
                    module,
                    context,
                    writes,
                    visited_functions,
                    from_ff,
                );
            }
            Statement::Case(x) => {
                for arm in &x.arms {
                    collect_writes(
                        &arm.body,
                        module,
                        context,
                        writes,
                        visited_functions,
                        from_ff,
                    );
                }
                collect_writes(
                    &x.default,
                    module,
                    context,
                    writes,
                    visited_functions,
                    from_ff,
                );
            }
            Statement::For(x) => {
                collect_writes(&x.body, module, context, writes, visited_functions, from_ff);
            }
            Statement::FunctionCall(call) => {
                for destinations in call.outputs.values() {
                    for destination in destinations {
                        push_destination(destination, context, writes, from_ff);
                    }
                }
                collect_function_writes(call, module, context, writes, visited_functions);
            }
            Statement::SystemFunctionCall(call) => {
                if let SystemFunctionKind::Readmemh(_, output) = &call.kind {
                    for destination in &output.0 {
                        push_destination(destination, context, writes, from_ff);
                    }
                }
            }
            Statement::TbMethodCall(call) => {
                if let Some(destination) = &call.ret {
                    push_destination(destination, context, writes, from_ff);
                }
            }
            Statement::Break | Statement::Unsupported(_) | Statement::Null => {}
        }
    }
}

fn collect_function_writes(
    call: &FunctionCall,
    module: &Module,
    context: &mut Context,
    writes: &mut Vec<Access>,
    visited: &mut HashSet<VarId>,
) {
    if !visited.insert(call.id) {
        return;
    }
    if let Some(function) = module.functions.get(&call.id) {
        let body = call
            .index
            .as_deref()
            .and_then(|index| function.get_function(index))
            .or_else(|| function.get_function(&[]));
        if let Some(body) = body {
            let start = writes.len();
            // Assignments inside a function body use blocking semantics.
            // Output actuals were recorded above in the caller's context.
            collect_writes(&body.statements, module, context, writes, visited, false);
            writes[start..].iter_mut().for_each(|access| {
                if module
                    .variables
                    .get(&access.id)
                    .is_some_and(|variable| variable.affiliation == Affiliation::Function)
                {
                    access.mask = BigUint::default();
                }
            });
        }
    }
    visited.remove(&call.id);
}

fn collect_expr_reads(
    expression: &Expression,
    module: &Module,
    context: &mut Context,
    reads: &mut Vec<Access>,
    visited_functions: &mut HashSet<VarId>,
) {
    match expression {
        Expression::Term(factor) => {
            collect_factor_reads(factor, module, context, reads, visited_functions)
        }
        Expression::Unary(_, expression, _) => {
            collect_expr_reads(expression, module, context, reads, visited_functions)
        }
        Expression::Binary(lhs, _, rhs, _) => {
            collect_expr_reads(lhs, module, context, reads, visited_functions);
            collect_expr_reads(rhs, module, context, reads, visited_functions);
        }
        Expression::Ternary(cond, lhs, rhs, _) => {
            collect_expr_reads(cond, module, context, reads, visited_functions);
            collect_expr_reads(lhs, module, context, reads, visited_functions);
            collect_expr_reads(rhs, module, context, reads, visited_functions);
        }
        Expression::Concatenation(elements, _) => {
            for (expression, repeat) in elements {
                collect_expr_reads(expression, module, context, reads, visited_functions);
                if let Some(repeat) = repeat {
                    collect_expr_reads(repeat, module, context, reads, visited_functions);
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            for (_, expression) in fields {
                collect_expr_reads(expression, module, context, reads, visited_functions);
            }
        }
        Expression::ArrayLiteral(_, _) => {}
    }
}

fn collect_factor_reads(
    factor: &Factor,
    module: &Module,
    context: &mut Context,
    reads: &mut Vec<Access>,
    visited: &mut HashSet<VarId>,
) {
    match factor {
        Factor::Variable(id, index, select, _) => {
            collect_index_reads(index, module, context, reads, visited);
            collect_select_reads(select, module, context, reads, visited);
            if let Some(access) = variable_access(*id, index, select, context, false) {
                reads.push(access);
            }
        }
        Factor::FunctionCall(call) => {
            for input in call.inputs.values() {
                collect_expr_reads(input, module, context, reads, visited);
            }
            collect_function_reads(call, module, context, reads, visited);
        }
        Factor::SystemFunctionCall(call) => {
            collect_system_function_reads(call, module, context, reads, visited)
        }
        Factor::HierVariable(x) => {
            collect_index_reads(&x.index, module, context, reads, visited);
            collect_select_reads(&x.select, module, context, reads, visited);
        }
        Factor::Value(_) | Factor::Anonymous(_) | Factor::Unknown(_) => {}
    }
}

fn collect_function_reads(
    call: &FunctionCall,
    module: &Module,
    context: &mut Context,
    reads: &mut Vec<Access>,
    visited: &mut HashSet<VarId>,
) {
    if !visited.insert(call.id) {
        return;
    }
    if let Some(function) = module.functions.get(&call.id) {
        let body = call
            .index
            .as_deref()
            .and_then(|index| function.get_function(index))
            .or_else(|| function.get_function(&[]));
        if let Some(body) = body {
            let start = reads.len();
            collect_statement_reads(&body.statements, module, context, reads, visited);
            reads[start..].iter_mut().for_each(|access| {
                if module
                    .variables
                    .get(&access.id)
                    .is_some_and(|variable| variable.affiliation == Affiliation::Function)
                {
                    access.mask = BigUint::default();
                }
            });
        }
    }
    visited.remove(&call.id);
}

fn collect_statement_reads(
    statements: &[Statement],
    module: &Module,
    context: &mut Context,
    reads: &mut Vec<Access>,
    visited: &mut HashSet<VarId>,
) {
    for statement in statements {
        match statement {
            Statement::Assign(x) => collect_expr_reads(&x.expr, module, context, reads, visited),
            Statement::If(x) => {
                collect_expr_reads(&x.cond, module, context, reads, visited);
                collect_statement_reads(&x.true_side, module, context, reads, visited);
                collect_statement_reads(&x.false_side, module, context, reads, visited);
            }
            Statement::IfReset(x) => {
                collect_statement_reads(&x.true_side, module, context, reads, visited);
                collect_statement_reads(&x.false_side, module, context, reads, visited);
            }
            Statement::Case(x) => {
                collect_expr_reads(&x.case_target, module, context, reads, visited);
                for arm in &x.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            CasePattern::Eq(expression) => {
                                collect_expr_reads(expression, module, context, reads, visited)
                            }
                            CasePattern::Range { lo, hi, .. } => {
                                collect_expr_reads(lo, module, context, reads, visited);
                                collect_expr_reads(hi, module, context, reads, visited);
                            }
                        }
                    }
                    collect_statement_reads(&arm.body, module, context, reads, visited);
                }
                collect_statement_reads(&x.default, module, context, reads, visited);
            }
            Statement::For(x) => {
                for bound in x.range.dynamic_bounds() {
                    collect_expr_reads(bound, module, context, reads, visited);
                }
                collect_statement_reads(&x.body, module, context, reads, visited);
            }
            Statement::FunctionCall(call) => {
                for input in call.inputs.values() {
                    collect_expr_reads(input, module, context, reads, visited);
                }
                collect_function_reads(call, module, context, reads, visited);
            }
            Statement::SystemFunctionCall(call) => {
                collect_system_function_reads(call, module, context, reads, visited)
            }
            Statement::TbMethodCall(_)
            | Statement::Break
            | Statement::Unsupported(_)
            | Statement::Null => {}
        }
    }
}

fn collect_system_function_reads(
    call: &SystemFunctionCall,
    module: &Module,
    context: &mut Context,
    reads: &mut Vec<Access>,
    visited: &mut HashSet<VarId>,
) {
    let mut collect =
        |expression: &Expression| collect_expr_reads(expression, module, context, reads, visited);
    match &call.kind {
        SystemFunctionKind::Bits(x)
        | SystemFunctionKind::Size(x)
        | SystemFunctionKind::Clog2(x)
        | SystemFunctionKind::Onehot(x)
        | SystemFunctionKind::Signed(x)
        | SystemFunctionKind::Unsigned(x) => collect(&x.0),
        SystemFunctionKind::Readmemh(filename, _) => collect(&filename.0),
        SystemFunctionKind::Display(args) | SystemFunctionKind::Write(args) => {
            for argument in args {
                collect(&argument.0);
            }
        }
        SystemFunctionKind::Assert { cond, args, .. } => {
            collect(&cond.0);
            for argument in args {
                collect(&argument.0);
            }
        }
        SystemFunctionKind::Finish => {}
    }
}

fn collect_index_reads(
    index: &VarIndex,
    module: &Module,
    context: &mut Context,
    reads: &mut Vec<Access>,
    visited: &mut HashSet<VarId>,
) {
    for expression in &index.0 {
        collect_expr_reads(expression, module, context, reads, visited);
    }
}

fn collect_select_reads(
    select: &VarSelect,
    module: &Module,
    context: &mut Context,
    reads: &mut Vec<Access>,
    visited: &mut HashSet<VarId>,
) {
    for expression in &select.0 {
        collect_expr_reads(expression, module, context, reads, visited);
    }
    if let Some((_, expression)) = &select.1 {
        collect_expr_reads(expression, module, context, reads, visited);
    }
}

fn push_destination(
    destination: &AssignDestination,
    context: &mut Context,
    accesses: &mut Vec<Access>,
    from_ff: bool,
) {
    let nba = from_ff
        && context
            .variables
            .get(&destination.id)
            .is_some_and(|variable| {
                variable.kind != crate::ir::VarKind::Let
                    && variable.affiliation != Affiliation::AlwaysFf
            });
    if let Some(access) = variable_access(
        destination.id,
        &destination.index,
        &destination.select,
        context,
        nba,
    ) {
        accesses.push(access);
    }
}

fn variable_access(
    id: VarId,
    index: &VarIndex,
    select: &VarSelect,
    context: &mut Context,
    nba: bool,
) -> Option<Access> {
    let variable = context.variables.get(&id)?.clone();
    let index = index
        .is_const()
        .then(|| index.eval_value(context))
        .flatten()
        .and_then(|index| variable.r#type.array.calc_index(&index));
    let mask = if select.is_const_with_range() {
        select
            .eval_value(context, &variable.r#type, false)
            .map(|(beg, end)| ValueBigUint::gen_mask_range(beg, end))
    } else {
        None
    }
    .or_else(|| variable.total_width().map(ValueBigUint::gen_mask))?;
    Some(Access {
        id,
        index,
        mask,
        nba,
    })
}

trait DynamicBounds {
    fn dynamic_bounds(&self) -> Vec<&Expression>;
}

impl DynamicBounds for ForRange {
    fn dynamic_bounds(&self) -> Vec<&Expression> {
        let (start, end) = match self {
            ForRange::Forward { start, end, .. }
            | ForRange::Reverse { start, end, .. }
            | ForRange::Stepped { start, end, .. } => (start, end),
        };
        [start, end]
            .into_iter()
            .filter_map(|bound| match bound {
                ForBound::Expression(expression) => Some(expression.as_ref()),
                ForBound::Const(_) => None,
            })
            .collect()
    }
}
