//! Preserve function exits using ordinary control-flow IR, shared by all consumers.
use super::Context;
use crate::ir::{
    AssignDestination, AssignStatement, Comptime, Expression, Factor, IfStatement, Statement, Type,
    TypeKind, VarId, VarIndex, VarKind, VarPath, VarSelect, Variable,
};
use crate::symbol::ClockDomain;
use crate::value::Value;
use veryl_parser::{resource_table, token_range::TokenRange};

fn is_return(statement: &Statement, ret: VarId) -> bool {
    matches!(statement, Statement::Assign(x) if x.dst.iter().any(|dst| dst.id == ret))
}

/// Whether a return can bypass more statements (including another loop iteration).
fn needs_exit(statements: &[Statement], ret: VarId, followed: bool) -> bool {
    statements.iter().enumerate().any(|(i, statement)| {
        let followed = followed || i + 1 < statements.len();
        match statement {
            x if is_return(x, ret) => followed,
            Statement::If(x) => {
                needs_exit(&x.true_side, ret, followed) || needs_exit(&x.false_side, ret, followed)
            }
            Statement::Case(x) => {
                x.arms
                    .iter()
                    .any(|arm| needs_exit(&arm.body, ret, followed))
                    || needs_exit(&x.default, ret, followed)
            }
            Statement::For(x) => needs_exit(&x.body, ret, true),
            _ => false,
        }
    })
}

pub(super) fn lower_returns(
    context: &mut Context,
    statements: &mut Vec<Statement>,
    ret: Option<VarId>,
    token: TokenRange,
) {
    let Some(ret) = ret else { return };
    if !needs_exit(statements, ret, false) {
        return;
    }

    // A dot cannot occur in a source identifier, so this function-local name
    // cannot shadow a user variable. Initialize it on every invocation.
    let path = VarPath::new(resource_table::insert_str("return.active"));
    let r#type = Type::new(TypeKind::Bit);
    let comptime = Comptime::from_type(r#type.clone(), ClockDomain::None, token);
    let id = context.insert_var_path(path.clone(), comptime.clone());
    let variable = Variable::new(
        id,
        path.clone(),
        VarKind::Variable,
        r#type,
        vec![Value::new(0, 1, false)],
        context.get_affiliation(),
        &token,
        context.config.evaluate_array_limit,
    );
    context.insert_variable(id, variable);
    let active = AssignDestination {
        id,
        path,
        index: VarIndex::default(),
        select: VarSelect::default(),
        comptime,
        token,
    };
    lower_block(statements, ret, &active);
    statements.insert(0, set_active(&active, true));
}

fn set_active(active: &AssignDestination, value: bool) -> Statement {
    Statement::Assign(AssignStatement {
        dst: vec![active.clone()],
        hier_dst: None,
        width: Some(1),
        expr: Expression::create_value(Value::new(value as u64, 1, false), active.token),
        token: active.token,
    })
}

fn branch(
    active: &AssignDestination,
    true_side: Vec<Statement>,
    false_side: Vec<Statement>,
) -> Statement {
    Statement::If(IfStatement {
        cond: Expression::Term(Box::new(Factor::Variable(
            active.id,
            VarIndex::default(),
            VarSelect::default(),
            active.comptime.clone(),
        ))),
        true_side,
        false_side,
        token: active.token,
    })
}

/// Returns whether this block can exit the function. Guard the entire remaining
/// suffix once, so conditions and side effects are not evaluated after a return.
fn lower_block(statements: &mut Vec<Statement>, ret: VarId, active: &AssignDestination) -> bool {
    let mut source = std::mem::take(statements).into_iter();
    while let Some(mut statement) = source.next() {
        if is_return(&statement, ret) {
            statements.push(statement);
            statements.push(set_active(active, false));
            return true;
        }
        let exits = match &mut statement {
            Statement::If(x) => {
                let left = lower_block(&mut x.true_side, ret, active);
                let right = lower_block(&mut x.false_side, ret, active);
                left || right
            }
            Statement::Case(x) => {
                let mut exits = lower_block(&mut x.default, ret, active);
                for arm in &mut x.arms {
                    exits |= lower_block(&mut arm.body, ret, active);
                }
                exits
            }
            Statement::For(x) => {
                let exits = lower_block(&mut x.body, ret, active);
                if exits {
                    // Break this loop immediately; enclosing loops get their own
                    // check so a return unwinds them all, unlike a source break.
                    x.body.push(branch(active, vec![], vec![Statement::Break]));
                }
                exits
            }
            _ => false,
        };
        statements.push(statement);
        if exits {
            let mut suffix = source.collect::<Vec<_>>();
            if !suffix.is_empty() {
                lower_block(&mut suffix, ret, active);
                statements.push(branch(active, suffix, vec![]));
            }
            return true;
        }
    }
    false
}
