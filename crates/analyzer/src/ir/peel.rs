//! Peels, for the native simulator, the runtime `for` loops of an
//! `always_comb` whose `break`s are decided by values the procedure itself
//! computes, such as a step count initialised from a parameter.
//!
//! A loop with `break` converts as a runtime `for`, since unrolling it at
//! conversion would strand a runtime-conditional `break`. Only the copy of
//! the declarations the simulator builds from is rewritten; the emitted
//! SystemVerilog and the analyzer's checks keep the loop. Each iteration
//! becomes a copy of the body with the iterator as its constant, and a decided
//! `if` or `case` is replaced by the side it takes. A loop is peeled only when
//! every `break` it reaches is decided, and a peel that would leave more
//! runtime loops than it removes is dropped.
//!
//! Decisions use only the expressions `decide` evaluates, on which every
//! backend agrees. A loop whose body calls a function or holds an `if_reset`
//! is not peeled, nor one whose iterator is read outside it or written inside.

use crate::conv::Context;
use crate::ir::{
    self, ArrayLiteralItem, AssignDestination, CasePattern, Declaration, Expression, Factor,
    ForBound, ForRange, Function, Module, Op, Statement, SystemFunctionCall, SystemFunctionKind,
    TypeKind, ValueVariant, VarId, VarIndex, VarKind, VarSelect,
};
use crate::symbol::SymbolKind;
use crate::symbol_table;
use crate::value::Value;
use crate::{HashMap, HashSet};

/// Most statement and expression nodes one peeled loop may emit, counting
/// what nested bodies hold, so that the copies stay as small as a small
/// unrolled loop however large one body is.
const EMIT_LIMIT: usize = 32768;

/// Most statements peeling one loop may look at, which also bounds the
/// iterations of any loop it peels.
const VISIT_LIMIT: usize = 16384;

/// Most values known at once. An undecided branch copies them for each of
/// its sides; a value assigned while this many are known is not tracked.
const TRACKED_LIMIT: usize = 256;

/// True when `stmts` hold a `break` of the loop they are the body of.
pub(crate) fn has_own_break(stmts: &[Statement]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Statement::Break => true,
        Statement::If(x) => has_own_break(&x.true_side) || has_own_break(&x.false_side),
        Statement::IfReset(x) => has_own_break(&x.true_side) || has_own_break(&x.false_side),
        Statement::Case(x) => {
            has_own_break(&x.default) || x.arms.iter().any(|a| has_own_break(&a.body))
        }
        _ => false,
    })
}

fn is_break_loop(stmt: &Statement) -> bool {
    matches!(stmt, Statement::For(x) if has_own_break(&x.body))
}

/// True when an `always_comb` of `decls` holds a top-level loop with a
/// `break`, which `peel_decided_loops` may peel.
pub fn has_break_loop(decls: &[Declaration]) -> bool {
    decls
        .iter()
        .any(|decl| matches!(decl, Declaration::Comb(x) if x.statements.iter().any(is_break_loop)))
}

fn contains_for(stmts: &[Statement]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Statement::For(_) => true,
        Statement::If(x) => contains_for(&x.true_side) || contains_for(&x.false_side),
        Statement::IfReset(x) => contains_for(&x.true_side) || contains_for(&x.false_side),
        Statement::Case(x) => {
            contains_for(&x.default) || x.arms.iter().any(|arm| contains_for(&arm.body))
        }
        _ => false,
    })
}

/// Whether a combinational or sequential procedure contains a loop.
/// Backends can skip copying declarations when no loop needs lowering.
pub fn has_for_loop(decls: &[Declaration]) -> bool {
    decls.iter().any(|decl| match decl {
        Declaration::Comb(x) => contains_for(&x.statements),
        Declaration::Ff(x) => contains_for(&x.statements),
        _ => false,
    })
}

/// Lower bounded constant loops for a backend that requires concrete write
/// lanes. The caller owns a private copy; shared analysis IR stays compact.
/// Loops containing a break remain available to the decided-loop peeler.
/// Under `keep_reset` only reset-side loops are bounded: they form a reset
/// network, while a kept logic loop reads and writes its arrays whole and
/// costs the settle its one-pass order.
/// `keep_reset` keeps a reset-side loop, outside the budget, when every array
/// it indexes at runtime already is outside reset, so no array changes layout.
pub fn lower_constant_loops(
    context: &mut Context,
    module: &Module,
    declarations: &mut [Declaration],
    statement_limit: usize,
    keep_reset: bool,
) -> bool {
    if is_test_module(module) {
        return false;
    }
    let mut changed = false;
    for decl in declarations.iter_mut() {
        let (stmts, limit) = match decl {
            Declaration::Comb(x) => (&mut x.statements, usize::MAX),
            Declaration::Ff(x) if !keep_reset => (&mut x.statements, statement_limit),
            Declaration::Ff(x) => (&mut x.statements, usize::MAX),
            _ => continue,
        };
        changed |= lower_constant_loop_body(context, stmts, limit, keep_reset);
    }
    if keep_reset {
        let mut dynamic = HashSet::default();
        for decl in declarations.iter() {
            if let Declaration::Comb(x) = decl {
                runtime_indexed(&x.statements, false, &mut dynamic);
            } else if let Declaration::Ff(x) = decl {
                runtime_indexed(&x.statements, false, &mut dynamic);
            }
        }
        for decl in declarations.iter_mut() {
            let stmts = match decl {
                Declaration::Comb(x) => &mut x.statements,
                Declaration::Ff(x) => &mut x.statements,
                _ => continue,
            };
            changed |= lower_reset_loops(context, stmts, &dynamic, statement_limit, false);
        }
    }
    changed
}

fn runtime_indexed(stmts: &[Statement], in_reset: bool, out: &mut HashSet<VarId>) {
    fn dst(x: &AssignDestination, out: &mut HashSet<VarId>) {
        if !x.index.is_const() {
            out.insert(x.id);
        }
    }
    for stmt in stmts {
        match stmt {
            Statement::Assign(x) => x.dst.iter().for_each(|x| dst(x, out)),
            Statement::FunctionCall(x) => x.outputs.values().flatten().for_each(|x| dst(x, out)),
            Statement::If(x) => {
                runtime_indexed(&x.true_side, in_reset, out);
                runtime_indexed(&x.false_side, in_reset, out);
            }
            Statement::IfReset(x) => {
                if in_reset {
                    runtime_indexed(&x.true_side, in_reset, out);
                }
                runtime_indexed(&x.false_side, in_reset, out);
            }
            Statement::Case(x) => {
                for arm in &x.arms {
                    runtime_indexed(&arm.body, in_reset, out);
                }
                runtime_indexed(&x.default, in_reset, out);
            }
            Statement::For(x) => runtime_indexed(&x.body, in_reset, out),
            _ => {}
        }
    }
}

/// Lowers each reset-side loop that would make an array runtime-indexed.
fn lower_reset_loops(
    context: &mut Context,
    stmts: &mut Vec<Statement>,
    dynamic: &HashSet<VarId>,
    statement_limit: usize,
    in_reset: bool,
) -> bool {
    let mut changed = false;
    let mut out = Vec::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        match &mut stmt {
            Statement::IfReset(x) => {
                changed |=
                    lower_reset_loops(context, &mut x.true_side, dynamic, statement_limit, true);
            }
            Statement::If(x) => {
                changed |= lower_reset_loops(
                    context,
                    &mut x.true_side,
                    dynamic,
                    statement_limit,
                    in_reset,
                );
                changed |= lower_reset_loops(
                    context,
                    &mut x.false_side,
                    dynamic,
                    statement_limit,
                    in_reset,
                );
            }
            Statement::Case(x) => {
                for arm in &mut x.arms {
                    changed |= lower_reset_loops(
                        context,
                        &mut arm.body,
                        dynamic,
                        statement_limit,
                        in_reset,
                    );
                }
                changed |=
                    lower_reset_loops(context, &mut x.default, dynamic, statement_limit, in_reset);
            }
            Statement::For(_) if in_reset => {
                let mut written = HashSet::default();
                runtime_indexed(std::slice::from_ref(&stmt), true, &mut written);
                if !written.is_subset(dynamic) {
                    let mut lowered = vec![stmt.clone()];
                    if lower_constant_loop_body(context, &mut lowered, statement_limit, false) {
                        out.extend(lowered);
                        changed = true;
                        continue;
                    }
                }
            }
            _ => {}
        }
        out.push(stmt);
    }
    *stmts = out;
    changed
}

/// Lower constant loops in a backend-owned procedure or function body.
/// Exceeding the caller's statement budget leaves the original body intact.
pub fn lower_constant_loop_body(
    context: &mut Context,
    stmts: &mut Vec<Statement>,
    statement_limit: usize,
    keep_reset: bool,
) -> bool {
    fn lower(
        context: &mut Context,
        stmts: &[Statement],
        budget: &mut usize,
        changed: &mut bool,
        keep_reset: bool,
    ) -> Option<Vec<Statement>> {
        let mut out = Vec::new();
        for stmt in stmts {
            *budget = budget.checked_sub(1)?;
            if let Statement::For(x) = stmt
                && !has_own_break(&x.body)
                && let Some(iterations) = x.range.eval_iter(context)
            {
                for iteration in iterations {
                    let body = specialize_iteration(context, x, iteration)?;
                    out.extend(lower(context, &body, budget, changed, keep_reset)?);
                }
                *changed = true;
                continue;
            }
            let mut stmt = stmt.clone();
            match &mut stmt {
                Statement::For(x) => x.body = lower(context, &x.body, budget, changed, keep_reset)?,
                Statement::If(x) => {
                    x.true_side = lower(context, &x.true_side, budget, changed, keep_reset)?;
                    x.false_side = lower(context, &x.false_side, budget, changed, keep_reset)?;
                }
                Statement::IfReset(x) => {
                    if !keep_reset {
                        x.true_side = lower(context, &x.true_side, budget, changed, keep_reset)?;
                    }
                    x.false_side = lower(context, &x.false_side, budget, changed, keep_reset)?;
                }
                Statement::Case(x) => {
                    for arm in &mut x.arms {
                        arm.body = lower(context, &arm.body, budget, changed, keep_reset)?;
                    }
                    x.default = lower(context, &x.default, budget, changed, keep_reset)?;
                }
                _ => {}
            }
            out.push(stmt);
        }
        Some(out)
    }

    if !contains_for(stmts) {
        return false;
    }
    let mut budget = statement_limit;
    let mut expanded = false;
    if let Some(lowered) = lower(context, stmts, &mut budget, &mut expanded, keep_reset)
        && expanded
    {
        *stmts = lowered;
        true
    } else {
        false
    }
}

/// Peels the decided loops of the `always_comb` declarations in `decls`, a
/// copy of `module`'s declarations, with `context` holding `module`'s
/// variables. Returns whether any declaration changed; nothing outside a
/// peeled loop does. A testbench module is left as it is.
pub fn peel_decided_loops(
    context: &mut Context,
    module: &Module,
    decls: &mut [Declaration],
) -> bool {
    if !has_break_loop(decls) || is_test_module(module) {
        return false;
    }
    let confined = confined_iterators(decls, &module.functions);
    let mut changed = false;
    for decl in decls.iter_mut() {
        if let Declaration::Comb(x) = decl
            && x.statements.iter().any(is_break_loop)
            && let Some(stmts) = peel_procedure(context, &x.statements, &confined)
        {
            x.statements = stmts;
            changed = true;
        }
    }
    changed
}

fn is_test_module(module: &Module) -> bool {
    symbol_table::get(module.signature.symbol)
        .is_some_and(|x| matches!(&x.kind, SymbolKind::Module(x) if x.test.is_some()))
}

/// `stmts`, the body of an `always_comb`, with each top-level loop whose
/// `break`s are decided peeled; `None` when none is.
fn peel_procedure(
    context: &mut Context,
    stmts: &[Statement],
    confined: &HashSet<VarId>,
) -> Option<Vec<Statement>> {
    let mut env = KnownValues::default();
    let mut ret = Vec::with_capacity(stmts.len());
    let mut changed = false;
    for stmt in stmts {
        if let Statement::For(looped) = stmt
            && has_own_break(&looped.body)
            && let Some((copies, after)) = peel_top(context, &env, looped, confined)
        {
            env = after;
            ret.extend(copies);
            changed = true;
            continue;
        }
        apply(context, &mut env, std::slice::from_ref(stmt));
        ret.push(stmt.clone());
    }
    changed.then_some(ret)
}

fn peel_top(
    context: &mut Context,
    env: &KnownValues,
    looped: &ir::ForStatement,
    confined: &HashSet<VarId>,
) -> Option<(Vec<Statement>, KnownValues)> {
    if range_has_call(&looped.range) || stmts_have_call(&looped.body) {
        return None;
    }
    let Iterations::Known(iterations) = eval_range(context, env, &looped.range)? else {
        return None;
    };
    let mut peeler = Peeler {
        confined,
        emitted: 0,
        visited: 0,
    };
    let (copies, after) = peeler.peel(context, env.clone(), looped, iterations, &[])?;
    let before = 1 + count_loops(&looped.body);
    (count_loops(&copies) <= before).then_some((copies, after))
}

fn count_loops(stmts: &[Statement]) -> usize {
    stmts
        .iter()
        .map(|stmt| match stmt {
            Statement::For(x) => 1 + count_loops(&x.body),
            Statement::If(x) => count_loops(&x.true_side) + count_loops(&x.false_side),
            Statement::IfReset(x) => count_loops(&x.true_side) + count_loops(&x.false_side),
            Statement::Case(x) => {
                count_loops(&x.default) + x.arms.iter().map(|a| count_loops(&a.body)).sum::<usize>()
            }
            _ => 0,
        })
        .sum()
}

/// The tracked scalars, each by its value at its declared width.
#[derive(Clone, Default)]
struct KnownValues {
    vars: HashMap<VarId, u64>,
}

impl KnownValues {
    fn forget_all(&mut self) {
        self.vars.clear();
    }

    fn keep_common(&mut self, other: &KnownValues) {
        self.vars.retain(|id, v| other.vars.get(id) == Some(v));
    }
}

enum Flow {
    Continue,
    Break,
}

struct Peeler<'a> {
    confined: &'a HashSet<VarId>,
    /// Statements emitted and looked at so far for the outermost loop; what
    /// a nested peel that fails has used is not given back.
    emitted: usize,
    visited: usize,
}

impl Peeler<'_> {
    fn visit(&mut self) -> Option<()> {
        self.visited += 1;
        (self.visited <= VISIT_LIMIT).then_some(())
    }

    fn emit(&mut self, out: &mut Vec<Statement>, stmt: Statement) -> Option<()> {
        self.emitted += size(std::slice::from_ref(&stmt));
        (self.emitted <= EMIT_LIMIT).then_some(())?;
        out.push(stmt);
        Some(())
    }

    /// The straight-line copies of `looped`'s `iterations` up to the `break`
    /// they take, and what is known after them; `None` when a `break` is not
    /// decided or a limit is reached. `subs` gives the iterators of the loops
    /// around it.
    fn peel(
        &mut self,
        context: &mut Context,
        mut env: KnownValues,
        looped: &ir::ForStatement,
        iterations: Vec<usize>,
        subs: &[(VarId, Value)],
    ) -> Option<(Vec<Statement>, KnownValues)> {
        if !self.confined.contains(&looped.var_id) || !looped.var_type.array.is_empty() {
            return None;
        }
        let width = looped.var_type.total_width()?;
        if !(1..=64).contains(&width) {
            return None;
        }
        let signed = looped.var_type.signed;
        let mut subs = subs.to_vec();
        subs.push((looped.var_id, Value::new(0, width, signed)));
        let mut ret = vec![];
        for k in iterations {
            self.visit()?;
            // An iterator too narrow for its range wraps at runtime.
            if width < usize::BITS as usize && k >> width != 0 {
                return None;
            }
            let value = Value::new(k as u64, width, signed);
            subs.last_mut()?.1 = value.clone();
            env.vars.insert(looped.var_id, k as u64);
            let flow = self.specialize(context, &mut env, &looped.body, &subs, &mut ret)?;
            if let Flow::Break = flow {
                break;
            }
        }
        env.vars.remove(&looped.var_id);
        Some((ret, env))
    }

    /// Appends to `out` what `stmts` run under `env`, through `Subst` with the
    /// iterators of `subs`, replacing each decided branch by the side it
    /// takes and unrolling each loop whose range becomes known; stops at a
    /// `break`.
    ///
    /// A statement is substituted only when an iteration reaches it, so a
    /// constant index out of range in a statement after the decided `break`
    /// does not keep the loop.
    fn specialize(
        &mut self,
        context: &mut Context,
        env: &mut KnownValues,
        stmts: &[Statement],
        subs: &[(VarId, Value)],
        out: &mut Vec<Statement>,
    ) -> Option<Flow> {
        for stmt in stmts {
            self.visit()?;
            let kept = match stmt {
                Statement::Break => return Some(Flow::Break),
                Statement::If(x) => {
                    let mut cond = x.cond.clone();
                    Subst::new(context, subs).expr(&mut cond)?;
                    if let Some(taken) = eval_cond(context, env, &cond) {
                        let side = if taken { &x.true_side } else { &x.false_side };
                        if let Flow::Break = self.specialize(context, env, side, subs, out)? {
                            return Some(Flow::Break);
                        }
                        continue;
                    }
                    if has_own_break(&x.true_side) || has_own_break(&x.false_side) {
                        return None;
                    }
                    Statement::If(ir::IfStatement {
                        cond,
                        true_side: substituted(context, &x.true_side, subs)?,
                        false_side: substituted(context, &x.false_side, subs)?,
                        token: x.token,
                    })
                }
                Statement::Case(x) => {
                    let mut target = x.case_target.as_ref().clone();
                    let mut patterns: Vec<Vec<CasePattern>> =
                        x.arms.iter().map(|a| a.patterns.clone()).collect();
                    let mut subst = Subst::new(context, subs);
                    subst.expr(&mut target)?;
                    for pattern in patterns.iter_mut().flatten() {
                        subst.pattern(pattern)?;
                    }
                    let arms = patterns.iter().map(|a| a.as_slice());
                    if let Some(i) = eval_case(context, env, &target, arms) {
                        let side = x.arms.get(i).map(|a| &a.body).unwrap_or(&x.default);
                        if let Flow::Break = self.specialize(context, env, side, subs, out)? {
                            return Some(Flow::Break);
                        }
                        continue;
                    }
                    if has_own_break(std::slice::from_ref(stmt)) {
                        return None;
                    }
                    let mut arms = Vec::with_capacity(x.arms.len());
                    for (arm, patterns) in x.arms.iter().zip(patterns) {
                        arms.push(ir::CaseArm {
                            patterns,
                            body: substituted(context, &arm.body, subs)?,
                            token: arm.token,
                        });
                    }
                    Statement::Case(ir::CaseStatement {
                        arms,
                        default: substituted(context, &x.default, subs)?,
                        case_target: Box::new(target),
                        token: x.token,
                    })
                }
                Statement::For(x) => {
                    let mut range = x.range.clone();
                    Subst::new(context, subs).range(&mut range)?;
                    if let Iterations::Known(iterations) = eval_range(context, env, &range)?
                        && let Some((copies, after)) =
                            self.peel(context, env.clone(), x, iterations, subs)
                    {
                        *env = after;
                        out.extend(copies);
                        continue;
                    }
                    Statement::For(Box::new(ir::ForStatement {
                        var_id: x.var_id,
                        var_name: x.var_name,
                        var_type: x.var_type.clone(),
                        range,
                        body: substituted(context, &x.body, subs)?,
                        token: x.token,
                    }))
                }
                _ => {
                    let [kept] = substituted(context, std::slice::from_ref(stmt), subs)?
                        .try_into()
                        .ok()?;
                    kept
                }
            };
            apply(context, env, std::slice::from_ref(&kept));
            self.emit(out, kept)?;
        }
        Some(Flow::Continue)
    }
}

fn substituted(
    context: &mut Context,
    stmts: &[Statement],
    subs: &[(VarId, Value)],
) -> Option<Vec<Statement>> {
    let mut stmts = stmts.to_vec();
    Subst::new(context, subs).stmts(&mut stmts)?;
    Some(stmts)
}

/// Of the iterators of the top-level `break` loops of `decls` and of every
/// loop inside one, those that nothing outside their own loop, including a
/// function body, reads and nothing inside it writes. A later name can
/// resolve to an earlier loop's iterator, and such a read would see what the
/// runtime loop left there, which the copies do not leave.
fn confined_iterators(
    decls: &[Declaration],
    functions: &HashMap<VarId, Function>,
) -> HashSet<VarId> {
    let mut loops = vec![];
    for decl in decls {
        if let Declaration::Comb(x) = decl {
            let candidates = x.statements.iter().filter(|x| is_break_loop(x));
            collect_loops(candidates, &mut loops);
        }
    }
    let ids: HashSet<VarId> = loops.iter().map(|x| x.var_id).collect();
    // Reads of each iterator anywhere, less those inside its own loop.
    let mut outside: HashMap<VarId, usize> = HashMap::default();
    let mut count = |id: VarId| {
        if ids.contains(&id) {
            *outside.entry(id).or_default() += 1;
        }
    };
    if !decls.iter().all(|x| declaration_reads(x, &mut count)) {
        return HashSet::default();
    }
    for body in functions.values().flat_map(|x| &x.functions) {
        if !visit_reads(&body.statements, &mut count) {
            return HashSet::default();
        }
    }
    let mut confined = HashSet::default();
    for x in &loops {
        let mut inside = 0;
        let mut own = |id: VarId| inside += usize::from(id == x.var_id);
        range_reads(&x.range, &mut own);
        visit_reads(&x.body, &mut own);
        let mut written = false;
        visit_writes(&x.body, &mut |id| written |= id == x.var_id);
        if !written && outside.get(&x.var_id).copied().unwrap_or(0) == inside {
            confined.insert(x.var_id);
        }
    }
    confined
}

fn collect_loops<'a>(
    stmts: impl IntoIterator<Item = &'a Statement>,
    out: &mut Vec<&'a ir::ForStatement>,
) {
    for stmt in stmts {
        match stmt {
            Statement::For(x) => {
                out.push(x);
                collect_loops(&x.body, out);
            }
            Statement::If(x) => {
                collect_loops(&x.true_side, out);
                collect_loops(&x.false_side, out);
            }
            Statement::IfReset(x) => {
                collect_loops(&x.true_side, out);
                collect_loops(&x.false_side, out);
            }
            Statement::Case(x) => {
                collect_loops(&x.default, out);
                for arm in &x.arms {
                    collect_loops(&arm.body, out);
                }
            }
            _ => {}
        }
    }
}

fn size(stmts: &[Statement]) -> usize {
    stmts
        .iter()
        .map(|stmt| match stmt {
            Statement::Assign(x) => 1 + expr_size(&x.expr),
            Statement::If(x) => 1 + expr_size(&x.cond) + size(&x.true_side) + size(&x.false_side),
            Statement::IfReset(x) => 1 + size(&x.true_side) + size(&x.false_side),
            Statement::Case(x) => {
                1 + expr_size(&x.case_target)
                    + size(&x.default)
                    + x.arms.iter().map(|a| size(&a.body)).sum::<usize>()
            }
            Statement::For(x) => 1 + size(&x.body),
            _ => 1,
        })
        .sum()
}

fn expr_size(expr: &Expression) -> usize {
    match expr {
        Expression::Term(_) => 1,
        Expression::Unary(_, x, _) => 1 + expr_size(x),
        Expression::Binary(x, _, y, _) => 1 + expr_size(x) + expr_size(y),
        Expression::Ternary(x, y, z, _) => 1 + expr_size(x) + expr_size(y) + expr_size(z),
        Expression::Concatenation(x, _) => {
            1 + x
                .iter()
                .map(|(x, r)| expr_size(x) + r.as_ref().map_or(0, expr_size))
                .sum::<usize>()
        }
        Expression::ArrayLiteral(x, _) => 1 + x.len(),
        Expression::StructConstructor(_, x, _) => {
            1 + x.iter().map(|(_, x)| expr_size(x)).sum::<usize>()
        }
    }
}

enum Iterations {
    Known(Vec<usize>),
    /// A bound is not decided, or the range is too long to peel.
    Runtime,
}

/// The iterations of `range` when `decide` settles its bounds under `env`
/// and there are at most `VISIT_LIMIT` of them. `None`, which keeps every
/// loop around it as well, when a bound is above `i32::MAX`: the emitted
/// loop iterates an `int`.
fn eval_range(context: &mut Context, env: &KnownValues, range: &ForRange) -> Option<Iterations> {
    let mut range = range.clone();
    let (start, end) = range.bounds_mut();
    for bound in [start, end] {
        let (value, signed) = match bound {
            ForBound::Const(x, signed) => (*x as u64, *signed),
            ForBound::Expression(x) => {
                let Some(value) = decide(context, env, x) else {
                    return Some(Iterations::Runtime);
                };
                (value, x.comptime().r#type.signed)
            }
        };
        if value > i32::MAX as u64 {
            return None;
        }
        *bound = ForBound::Const(value as usize, signed);
    }
    let limit = std::mem::replace(&mut context.config.evaluate_size_limit, VISIT_LIMIT);
    let ret = range.eval_iter(context);
    context.config.evaluate_size_limit = limit;
    Some(ret.map_or(Iterations::Runtime, Iterations::Known))
}

pub(crate) fn specialize_expression(
    context: &mut Context,
    expression: &Expression,
    id: VarId,
    value: Value,
) -> Option<Expression> {
    let mut expression = expression.clone();
    let values = [(id, value)];
    Subst {
        context,
        subs: &values,
        analysis: true,
    }
    .expr(&mut expression)?;
    Some(expression)
}

/// Specialize one iteration for a consumer that needs concrete positions.
/// The shared IR keeps the original loop; this temporary body is discarded
/// after the iteration and does not allocate per-iteration variables.
pub(crate) fn specialize_iteration(
    context: &mut Context,
    statement: &ir::ForStatement,
    iteration: usize,
) -> Option<Vec<Statement>> {
    let width = statement.var_type.total_width()?;
    let values = [(
        statement.var_id,
        Value::new(iteration as u64, width, statement.var_type.signed),
    )];
    let mut body = statement.body.clone();
    let mut subst = Subst {
        context,
        subs: &values,
        analysis: true,
    };
    subst.stmts(&mut body)?;
    Some(body)
}

/// Reads each iterator of the loops being peeled as its constant, as an
/// ordinary unrolled loop has them, keeping the read's type and context, and
/// folds each index and select made only of constants. A constant index or
/// select out of range, a dynamic select of an iterator and an iterator in a
/// hierarchical reference abort the peel.
struct Subst<'a> {
    context: &'a mut Context,
    subs: &'a [(VarId, Value)],
    analysis: bool,
}

impl<'a> Subst<'a> {
    fn new(context: &'a mut Context, subs: &'a [(VarId, Value)]) -> Self {
        Self {
            context,
            subs,
            analysis: false,
        }
    }

    fn call(&mut self, call: &mut ir::FunctionCall) -> Option<()> {
        for input in call.inputs.values_mut() {
            self.expr(input)?;
        }
        for outputs in call.outputs.values_mut() {
            for output in outputs {
                self.selector(
                    output.id,
                    &mut output.index,
                    &mut output.select,
                    &output.comptime,
                )?;
            }
        }
        Some(())
    }

    fn system_call(&mut self, call: &mut ir::SystemFunctionCall) -> Option<()> {
        match &mut call.kind {
            SystemFunctionKind::Bits(x)
            | SystemFunctionKind::Clog2(x)
            | SystemFunctionKind::Onehot(x)
            | SystemFunctionKind::Signed(x)
            | SystemFunctionKind::Unsigned(x) => self.expr(&mut x.0)?,
            SystemFunctionKind::Size(x, y) => {
                self.expr(&mut x.0)?;
                if let Some(y) = y {
                    self.expr(&mut y.0)?;
                }
            }
            SystemFunctionKind::Readmemh(x, y) => {
                self.expr(&mut x.0)?;
                if let ir::system_function::Output::Local(outputs) = y {
                    for output in outputs {
                        self.selector(
                            output.id,
                            &mut output.index,
                            &mut output.select,
                            &output.comptime,
                        )?;
                    }
                }
            }
            SystemFunctionKind::Display(xs) | SystemFunctionKind::Write(xs) => {
                for x in xs {
                    self.expr(&mut x.0)?;
                }
            }
            SystemFunctionKind::Assert { cond, args, .. } => {
                self.expr(&mut cond.0)?;
                for x in args {
                    self.expr(&mut x.0)?;
                }
            }
            SystemFunctionKind::Finish => {}
        }
        Some(())
    }

    fn value_of(&self, id: VarId) -> Option<&Value> {
        self.subs.iter().find(|x| x.0 == id).map(|x| &x.1)
    }

    fn pattern(&mut self, pattern: &mut CasePattern) -> Option<()> {
        match pattern {
            CasePattern::Eq(x) => self.expr(x),
            CasePattern::Range { lo, hi, .. } => {
                self.expr(lo)?;
                self.expr(hi)
            }
        }
    }

    fn range(&mut self, range: &mut ForRange) -> Option<()> {
        let (start, end) = range.bounds_mut();
        for bound in [start, end] {
            if let ForBound::Expression(x) = bound {
                self.expr(x)?;
            }
        }
        Some(())
    }

    fn stmts(&mut self, stmts: &mut [Statement]) -> Option<()> {
        for stmt in stmts {
            match stmt {
                Statement::Assign(x) => {
                    self.expr(&mut x.expr)?;
                    for dst in &mut x.dst {
                        self.selector(dst.id, &mut dst.index, &mut dst.select, &dst.comptime)?;
                    }
                    if let Some(dst) = &x.hier_dst
                        && self.selector_reads_iter(&dst.index, &dst.select)
                    {
                        return None;
                    }
                }
                Statement::If(x) => {
                    self.expr(&mut x.cond)?;
                    // The untaken side may index out of range at this iteration.
                    match self.decided_sides(&x.cond) {
                        (true, _) => x.false_side.clear(),
                        (_, true) => x.true_side.clear(),
                        _ => {}
                    }
                    self.stmts(&mut x.true_side)?;
                    self.stmts(&mut x.false_side)?;
                }
                Statement::IfReset(x) => {
                    self.stmts(&mut x.true_side)?;
                    self.stmts(&mut x.false_side)?;
                }
                Statement::Case(x) => {
                    self.expr(&mut x.case_target)?;
                    for arm in &mut x.arms {
                        for pattern in &mut arm.patterns {
                            self.pattern(pattern)?;
                        }
                        self.stmts(&mut arm.body)?;
                    }
                    self.stmts(&mut x.default)?;
                }
                Statement::For(x) => {
                    self.range(&mut x.range)?;
                    self.stmts(&mut x.body)?;
                }
                Statement::Break | Statement::Null | Statement::Unsupported(_) => {}
                Statement::FunctionCall(call) if self.analysis => self.call(call)?,
                Statement::SystemFunctionCall(call) if self.analysis => self.system_call(call)?,
                Statement::SystemFunctionCall(_)
                | Statement::FunctionCall(_)
                | Statement::TbMethodCall(_) => return None,
            }
        }
        Some(())
    }

    /// The constant a read of an iterator, whole or a constant select of
    /// it, stands for.
    fn const_read(&mut self, factor: &Factor) -> Option<Factor> {
        let Factor::Variable(id, index, select, comptime) = factor else {
            return None;
        };
        let mut value = self.value_of(*id)?.clone();
        if !index.indices.is_empty() || comptime.part_select.is_some() {
            return None;
        }
        if !select.is_empty() {
            // The select is evaluated from what the analyzer holds, which a
            // variable it reads is not.
            let mut reads = false;
            selector_reads(index, select, &mut |_| reads = true);
            if reads {
                return None;
            }
            let r#type = self.context.variables.get(id)?.r#type.clone();
            let (beg, end) = select.eval_value(self.context, &r#type, false)?;
            value = value.select(beg, end);
        }
        if comptime.r#type.total_width() != Some(value.width()) {
            return None;
        }
        value.set_signed(comptime.r#type.signed);
        let mut comptime = comptime.clone();
        comptime.value = ValueVariant::Numeric(value);
        comptime.is_const = true;
        Some(Factor::Value(comptime))
    }

    /// `(true_side_only, false_side_only)`, as conversion decides an unrolled `if`.
    fn decided_sides(&mut self, cond: &Expression) -> (bool, bool) {
        let mut cond = cond.clone();
        // Errors on the copy were reported at conversion if they are real.
        self.context.begin_analysis_transaction();
        cond.gather_context(self.context);
        let sides = crate::conv::statement::eval_cond_true_false(self.context, &cond);
        self.context.rollback_analysis_transaction();
        sides
    }

    /// `expr` as a constant when it reads no variable and evaluates free of
    /// X and Z, as conversion folds an index of an unrolled loop.
    fn folded(&mut self, expr: &Expression) -> Option<Expression> {
        let mut reads = false;
        expr_reads(expr, &mut |_| reads = true);
        if reads || matches!(expr, Expression::Term(x) if matches!(x.as_ref(), Factor::Value(_))) {
            return None;
        }
        let value = expr.eval_value(self.context)?;
        if value.is_xz() {
            return None;
        }
        let mut comptime = expr.comptime().clone();
        comptime.value = ValueVariant::Numeric(value);
        comptime.is_const = true;
        Some(Expression::Term(Box::new(Factor::Value(comptime))))
    }

    /// Substitutes inside the index and the select of `id` and folds each
    /// of their expressions that becomes constant.
    fn selector(
        &mut self,
        id: VarId,
        index: &mut VarIndex,
        select: &mut VarSelect,
        comptime: &ir::Comptime,
    ) -> Option<()> {
        let mut changed = false;
        for x in index
            .expressions_mut()
            .chain(select.0.iter_mut())
            .chain(select.1.as_mut().map(|x| &mut x.1))
        {
            if !self.reads_iter(x) {
                continue;
            }
            self.expr(x)?;
            if let Some(c) = self.folded(x) {
                *x = c;
            }
            changed = true;
        }
        if changed && !self.analysis {
            self.check_in_range(id, index, select, comptime)?;
        }
        Some(())
    }

    fn reads_iter(&self, expr: &Expression) -> bool {
        let mut ret = false;
        expr_reads(expr, &mut |id| ret |= self.value_of(id).is_some());
        ret
    }

    fn selector_reads_iter(&self, index: &VarIndex, select: &VarSelect) -> bool {
        let mut ret = false;
        selector_reads(index, select, &mut |id| ret |= self.value_of(id).is_some());
        ret
    }

    fn expr(&mut self, expr: &mut Expression) -> Option<()> {
        let result = match expr {
            Expression::Term(x) => {
                if let Factor::Variable(id, ..) = x.as_ref()
                    && self.value_of(*id).is_some()
                {
                    **x = self.const_read(x)?;
                    return Some(());
                }
                self.factor(x)
            }
            Expression::Unary(_, x, _) => self.expr(x),
            Expression::Binary(x, _, y, _) => {
                self.expr(x)?;
                self.expr(y)
            }
            Expression::Ternary(x, y, z, _) => {
                self.expr(x)?;
                self.expr(y)?;
                self.expr(z)
            }
            Expression::Concatenation(items, _) => {
                for (x, rep) in items {
                    self.expr(x)?;
                    if let Some(rep) = rep {
                        self.expr(rep)?;
                    }
                }
                Some(())
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(x, rep) => {
                            self.expr(x)?;
                            if let Some(rep) = rep {
                                self.expr(rep)?;
                            }
                        }
                        ArrayLiteralItem::Defaul(x) => self.expr(x)?,
                    }
                }
                Some(())
            }
            Expression::StructConstructor(_, items, _) => {
                for (_, x) in items {
                    self.expr(x)?;
                }
                Some(())
            }
        };
        result?;
        if self.analysis
            && (!expr_has_call(expr) || expr.gather_context(self.context).is_const)
            && let Some(folded) = self.folded(expr)
        {
            *expr = folded;
        }
        Some(())
    }

    fn factor(&mut self, factor: &mut Factor) -> Option<()> {
        match factor {
            Factor::Variable(id, index, select, comptime) => {
                let comptime = comptime.clone();
                self.selector(*id, index, select, &comptime)
            }
            Factor::HierVariable(x) => {
                (!self.selector_reads_iter(&x.index, &x.select)).then_some(())
            }
            Factor::SystemFunctionCall(x) if self.analysis => self.system_call(x),
            Factor::SystemFunctionCall(x) => match &mut x.kind {
                SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                    self.expr(&mut input.0)
                }
                _ => Some(()),
            },
            Factor::FunctionCall(call) if self.analysis => self.call(call),
            Factor::FunctionCall(_) => None,
            Factor::Value(_) | Factor::Anonymous(_) | Factor::Unknown(_) => Some(()),
        }
    }

    /// `None` when a constant index or select of `id` is out of range, or
    /// is into a struct, a multi-dimensional vector or a stepped range, whose
    /// bounds are not checked here. A dynamic one is left to run as it does.
    fn check_in_range(
        &mut self,
        id: VarId,
        index: &VarIndex,
        select: &VarSelect,
        comptime: &ir::Comptime,
    ) -> Option<()> {
        let r#type = self.context.variables.get(&id)?.r#type.clone();
        if index.is_const() {
            if index.indices.len() > r#type.array.dims() {
                return None;
            }
            for (x, dim) in index.indices.iter().zip(r#type.array.iter()) {
                if self.usize_of(x)? >= (*dim)? {
                    return None;
                }
            }
        }
        if !select.is_empty() && select.is_const_with_range() {
            if comptime.part_select.is_some()
                || r#type.is_struct_union()
                || r#type.width().dims() > 1
                || select.0.len() > 1
            {
                return None;
            }
            let width = r#type.total_width()?;
            let x = self.usize_of(&select.0[0])?;
            let (hi, lo) = match &select.1 {
                None => (x, x),
                Some((ir::VarSelectOp::Colon, y)) => (x, self.usize_of(y)?),
                Some((ir::VarSelectOp::PlusColon, y)) => {
                    (x.checked_add(self.usize_of(y)?)?.checked_sub(1)?, x)
                }
                Some((ir::VarSelectOp::MinusColon, y)) => {
                    (x, x.checked_add(1)?.checked_sub(self.usize_of(y)?)?)
                }
                Some((ir::VarSelectOp::Step, _)) => return None,
            };
            if hi < lo || hi >= width {
                return None;
            }
        }
        Some(())
    }

    fn usize_of(&mut self, expr: &Expression) -> Option<usize> {
        let value = expr.eval_value(self.context)?;
        if value.is_xz() {
            return None;
        }
        value.to_usize()
    }
}

fn mask(width: usize) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1 << width) - 1
    }
}

/// Where an operand sits, which decides whether its sign can matter.
#[derive(Clone, Copy, PartialEq)]
enum Parent {
    /// A whole condition, assignment source or bound, or a self-determined
    /// operand.
    Root,
    /// An operand of a comparison.
    Compare,
    /// An operand of arithmetic, bitwise logic or a shift.
    Arith,
}

/// The value of `expr` at the width its context evaluates it at, when it is
/// a decision this pass may take: integers of at most 64 bits free of X and
/// Z, read from constants, iterator values and tracked locals, combined by
/// `+ - * / %`, comparisons, `&& || !`, `& | ^ ~` and shifts by a constant
/// amount within the width. Every operation must evaluate unsigned, and an
/// operand whose type or context is signed must be non-negative, so that
/// sign and zero extension agree; a signed context is accepted only where
/// nothing is computed in it, a comparison of two operands or the whole
/// expression. A division or modulo by zero, and anything else, is left
/// undecided.
fn decide(context: &Context, env: &KnownValues, expr: &Expression) -> Option<u64> {
    decide_at(context, env, expr, Parent::Root)
}

fn decide_at(
    context: &Context,
    env: &KnownValues,
    expr: &Expression,
    parent: Parent,
) -> Option<u64> {
    let comptime = expr.comptime();
    let ctx = &comptime.expr_context;
    if let Expression::Term(x) = expr {
        let (value, width) = leaf(context, env, x)?;
        // A whole expression folded at conversion carries no context.
        let ctx_width = if ctx.width == 0 && parent == Parent::Root {
            width
        } else {
            ctx.width
        };
        let signed = comptime.r#type.signed || ctx.signed;
        if width > ctx_width || ctx_width > 64 || (signed && value >> (width - 1) != 0) {
            return None;
        }
        if ctx.signed && parent == Parent::Arith {
            return None;
        }
        return Some(value);
    }
    if ctx.width == 0
        || ctx.width > 64
        || ctx.signed
        || comptime.r#type.signed
        || comptime
            .r#type
            .total_width()
            .is_none_or(|x| x == 0 || x > 64)
    {
        return None;
    }
    let width = ctx.width;
    match expr {
        Expression::Unary(Op::LogicNot, x, _) => {
            Some(u64::from(decide_at(context, env, x, Parent::Root)? == 0))
        }
        Expression::Unary(Op::BitNot, x, _) => {
            Some(!decide_at(context, env, x, Parent::Arith)? & mask(width))
        }
        Expression::Binary(x, op, y, _) => {
            let operand = |e: &Expression, p: Parent| decide_at(context, env, e, p);
            let value = match op {
                Op::LogicAnd | Op::LogicOr => {
                    let x = operand(x, Parent::Root)? != 0;
                    let y = operand(y, Parent::Root)? != 0;
                    u64::from(if *op == Op::LogicAnd { x && y } else { x || y })
                }
                Op::Eq | Op::Ne | Op::Less | Op::LessEq | Op::Greater | Op::GreaterEq => {
                    let x = operand(x, Parent::Compare)?;
                    let y = operand(y, Parent::Compare)?;
                    u64::from(match op {
                        Op::Eq => x == y,
                        Op::Ne => x != y,
                        Op::Less => x < y,
                        Op::LessEq => x <= y,
                        Op::Greater => x > y,
                        _ => x >= y,
                    })
                }
                Op::LogicShiftL | Op::LogicShiftR => {
                    let Expression::Term(amount) = y.as_ref() else {
                        return None;
                    };
                    if matches!(amount.as_ref(), Factor::Variable(id, ..) if env.vars.contains_key(id))
                    {
                        return None;
                    }
                    let amount = operand(y, Parent::Root)?;
                    if amount >= width as u64 {
                        return None;
                    }
                    let x = operand(x, Parent::Arith)?;
                    if *op == Op::LogicShiftL {
                        x << amount
                    } else {
                        x >> amount
                    }
                }
                Op::Add
                | Op::Sub
                | Op::Mul
                | Op::Div
                | Op::Rem
                | Op::BitAnd
                | Op::BitOr
                | Op::BitXor => {
                    let x = operand(x, Parent::Arith)?;
                    let y = operand(y, Parent::Arith)?;
                    match op {
                        Op::Add => x.wrapping_add(y),
                        Op::Sub => x.wrapping_sub(y),
                        Op::Mul => x.wrapping_mul(y),
                        Op::Div => x.checked_div(y)?,
                        Op::Rem => x.checked_rem(y)?,
                        Op::BitAnd => x & y,
                        Op::BitOr => x | y,
                        _ => x ^ y,
                    }
                }
                _ => return None,
            };
            Some(value & mask(width))
        }
        _ => None,
    }
}

/// A leaf's value and declared width: a literal, a scalar constant, or a
/// tracked scalar.
fn leaf(context: &Context, env: &KnownValues, factor: &Factor) -> Option<(u64, usize)> {
    let (comptime, value) = match factor {
        Factor::Value(x) => (x, x.get_value().ok()?.clone()),
        Factor::Variable(id, index, select, x)
            if index.indices.is_empty() && select.is_empty() && x.part_select.is_none() =>
        {
            if let Some(value) = env.vars.get(id) {
                let width = x.r#type.total_width()?;
                return integer_type(x).then_some((*value, width));
            }
            let var = context.variables.get(id)?;
            if !x.is_const
                || !matches!(var.kind, VarKind::Param | VarKind::Const)
                || !var.r#type.array.is_empty()
            {
                return None;
            }
            let [value] = var.value.as_slice() else {
                return None;
            };
            (x, value.clone())
        }
        _ => return None,
    };
    let width = comptime.r#type.total_width()?;
    if !integer_type(comptime) || value.is_xz() || value.width() > 64 {
        return None;
    }
    // A value narrower than its type, from a fold at conversion, extends by
    // its own sign, so it is taken only where that cannot matter.
    let own = value.width();
    let signed = value.signed() || comptime.r#type.signed || comptime.expr_context.signed;
    let value = value.to_u64()?;
    let negative = own > 0 && value >> (own - 1) != 0;
    (own > 0 && value & !mask(width) == 0 && !(signed && negative)).then_some((value, width))
}

fn integer_type(comptime: &ir::Comptime) -> bool {
    let r#type = &comptime.r#type;
    matches!(
        r#type.kind,
        TypeKind::Bit | TypeKind::Logic | TypeKind::Enum(_)
    ) && r#type.array.is_empty()
        && r#type.total_width().is_some_and(|x| (1..=64).contains(&x))
}

fn eval_cond(context: &Context, env: &KnownValues, cond: &Expression) -> Option<bool> {
    Some(decide(context, env, cond)? != 0)
}

/// Index of the arm a `case` takes (`arms.len()` for `default`), when the
/// target is a whole unsigned constant or tracked scalar and every pattern
/// tried before the match is a constant `decide` settles. Every value is then
/// compared as the non-negative integer it is, whatever width the comparison
/// runs at.
fn eval_case<'a>(
    context: &Context,
    env: &KnownValues,
    target: &Expression,
    arms: impl IntoIterator<Item = &'a [CasePattern]>,
) -> Option<usize> {
    let constant = |x: &Expression| {
        let Expression::Term(factor) = x else {
            return None;
        };
        if matches!(factor.as_ref(), Factor::Variable(id, ..) if env.vars.contains_key(id)) {
            return None;
        }
        decide(context, env, x)
    };
    if target.comptime().r#type.signed || !matches!(target, Expression::Term(_)) {
        return None;
    }
    let target = decide(context, env, target)?;
    let mut count = 0;
    for (i, patterns) in arms.into_iter().enumerate() {
        count = i + 1;
        for pattern in patterns {
            let hit = match pattern {
                CasePattern::Eq(x) => constant(x)? == target,
                CasePattern::Range { lo, hi, inclusive } => {
                    let lo = constant(lo)?;
                    let hi = constant(hi)?;
                    lo <= target
                        && if *inclusive {
                            target <= hi
                        } else {
                            target < hi
                        }
                }
            };
            if hit {
                return Some(i);
            }
        }
    }
    Some(count)
}

/// The value a whole-scalar blocking assignment leaves, or `None` when the
/// destination is not such a scalar or the value is not decided.
fn assigned_value(
    context: &Context,
    env: &KnownValues,
    x: &ir::AssignStatement,
) -> Option<(VarId, u64)> {
    let [dst] = x.dst.as_slice() else {
        return None;
    };
    if !dst.index.indices.is_empty() || !dst.select.is_empty() || dst.comptime.part_select.is_some()
    {
        return None;
    }
    let var = context.variables.get(&dst.id)?;
    let width = var.r#type.total_width()?;
    if var.value.len() != 1
        || !var.r#type.array.is_empty()
        || !matches!(
            var.r#type.kind,
            TypeKind::Bit | TypeKind::Logic | TypeKind::Enum(_)
        )
        || !(1..=64).contains(&width)
    {
        return None;
    }
    Some((dst.id, decide(context, env, &x.expr)? & mask(width)))
}

/// Advances `env` over `stmts` as the procedure runs them. Returns false
/// when a `break` ends the list.
fn apply(context: &mut Context, env: &mut KnownValues, stmts: &[Statement]) -> bool {
    for stmt in stmts {
        match stmt {
            Statement::Assign(x) => {
                if expr_has_call(&x.expr) || x.dst.iter().any(dst_has_call) {
                    env.forget_all();
                    continue;
                }
                let known = assigned_value(context, env, x);
                for dst in &x.dst {
                    env.vars.remove(&dst.id);
                }
                if let Some((id, value)) = known
                    && env.vars.len() < TRACKED_LIMIT
                {
                    env.vars.insert(id, value);
                }
            }
            Statement::If(x) => {
                if expr_has_call(&x.cond) {
                    env.forget_all();
                    continue;
                }
                match eval_cond(context, env, &x.cond) {
                    Some(true) => {
                        if !apply(context, env, &x.true_side) {
                            return false;
                        }
                    }
                    Some(false) => {
                        if !apply(context, env, &x.false_side) {
                            return false;
                        }
                    }
                    None => {
                        let mut other = env.clone();
                        apply(context, &mut other, &x.true_side);
                        apply(context, env, &x.false_side);
                        env.keep_common(&other);
                    }
                }
            }
            Statement::Case(x) => {
                if expr_has_call(&x.case_target)
                    || x.arms
                        .iter()
                        .any(|a| a.patterns.iter().any(pattern_has_call))
                {
                    env.forget_all();
                    continue;
                }
                let arms = x.arms.iter().map(|a| a.patterns.as_slice());
                match eval_case(context, env, &x.case_target, arms) {
                    Some(i) => {
                        let body = x.arms.get(i).map(|a| &a.body).unwrap_or(&x.default);
                        if !apply(context, env, body) {
                            return false;
                        }
                    }
                    None => {
                        let before = env.clone();
                        apply(context, env, &x.default);
                        for arm in &x.arms {
                            let mut other = before.clone();
                            apply(context, &mut other, &arm.body);
                            env.keep_common(&other);
                        }
                    }
                }
            }
            Statement::For(x) => {
                if range_has_call(&x.range) || stmts_have_call(&x.body) {
                    env.forget_all();
                } else {
                    forget_writes(env, &x.body);
                }
            }
            Statement::Break => return false,
            Statement::Null => {}
            Statement::IfReset(_)
            | Statement::SystemFunctionCall(_)
            | Statement::FunctionCall(_)
            | Statement::TbMethodCall(_)
            | Statement::Unsupported(_) => env.forget_all(),
        }
    }
    true
}

fn expr_has_call(expr: &Expression) -> bool {
    match expr {
        Expression::Term(x) => factor_has_call(x),
        Expression::Unary(_, x, _) => expr_has_call(x),
        Expression::Binary(x, _, y, _) => expr_has_call(x) || expr_has_call(y),
        Expression::Ternary(x, y, z, _) => expr_has_call(x) || expr_has_call(y) || expr_has_call(z),
        Expression::Concatenation(items, _) => items
            .iter()
            .any(|(x, rep)| expr_has_call(x) || rep.as_ref().is_some_and(expr_has_call)),
        Expression::ArrayLiteral(items, _) => items.iter().any(|x| match x {
            ArrayLiteralItem::Value(x, rep) => {
                expr_has_call(x) || rep.as_deref().is_some_and(expr_has_call)
            }
            ArrayLiteralItem::Defaul(x) => expr_has_call(x),
        }),
        Expression::StructConstructor(_, items, _) => items.iter().any(|(_, x)| expr_has_call(x)),
    }
}

fn factor_has_call(factor: &Factor) -> bool {
    match factor {
        Factor::Variable(_, index, select, _) => {
            index
                .expressions()
                .chain(select.0.iter())
                .any(expr_has_call)
                || select.1.as_ref().is_some_and(|(_, x)| expr_has_call(x))
        }
        Factor::HierVariable(x) => {
            x.index
                .indices
                .iter()
                .chain(x.select.0.iter())
                .any(expr_has_call)
                || x.select.1.as_ref().is_some_and(|(_, x)| expr_has_call(x))
        }
        Factor::FunctionCall(_) => true,
        Factor::SystemFunctionCall(x) => match &x.kind {
            SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                expr_has_call(&input.0)
            }
            // Its arguments are not inspected, so only a folded one is known
            // free.
            _ => !x.comptime.is_const,
        },
        Factor::Value(_) | Factor::Anonymous(_) | Factor::Unknown(_) => false,
    }
}

fn dst_has_call(dst: &AssignDestination) -> bool {
    dst.index
        .indices
        .iter()
        .chain(dst.select.0.iter())
        .any(expr_has_call)
        || dst.select.1.as_ref().is_some_and(|(_, x)| expr_has_call(x))
}

fn pattern_has_call(pattern: &CasePattern) -> bool {
    match pattern {
        CasePattern::Eq(x) => expr_has_call(x),
        CasePattern::Range { lo, hi, .. } => expr_has_call(lo) || expr_has_call(hi),
    }
}

fn range_has_call(range: &ForRange) -> bool {
    let (start, end) = range.bounds();
    [start, end]
        .into_iter()
        .any(|x| matches!(x, ForBound::Expression(x) if expr_has_call(x)))
}

/// True when anything in `stmts`, at any depth, calls a function, which may
/// write its output arguments behind any value known here, or is a statement
/// nothing is known across.
fn stmts_have_call(stmts: &[Statement]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Statement::Assign(x) => {
            expr_has_call(&x.expr) || x.dst.iter().any(dst_has_call) || x.hier_dst.is_some()
        }
        Statement::If(x) => {
            expr_has_call(&x.cond)
                || stmts_have_call(&x.true_side)
                || stmts_have_call(&x.false_side)
        }
        Statement::Case(x) => {
            expr_has_call(&x.case_target)
                || x.arms
                    .iter()
                    .any(|a| a.patterns.iter().any(pattern_has_call) || stmts_have_call(&a.body))
                || stmts_have_call(&x.default)
        }
        Statement::For(x) => range_has_call(&x.range) || stmts_have_call(&x.body),
        Statement::IfReset(_)
        | Statement::FunctionCall(_)
        | Statement::SystemFunctionCall(_)
        | Statement::TbMethodCall(_)
        | Statement::Unsupported(_) => true,
        Statement::Break | Statement::Null => false,
    })
}

/// Forgets every variable `stmts`, which hold no call, may write.
fn forget_writes(env: &mut KnownValues, stmts: &[Statement]) {
    visit_writes(stmts, &mut |id| {
        env.vars.remove(&id);
    });
}

/// Calls `f` with each variable an assignment in `stmts` writes. A call's
/// outputs are not seen.
fn visit_writes(stmts: &[Statement], f: &mut impl FnMut(VarId)) {
    for stmt in stmts {
        match stmt {
            Statement::Assign(x) => {
                for dst in &x.dst {
                    f(dst.id);
                }
            }
            Statement::If(x) => {
                visit_writes(&x.true_side, f);
                visit_writes(&x.false_side, f);
            }
            Statement::IfReset(x) => {
                visit_writes(&x.true_side, f);
                visit_writes(&x.false_side, f);
            }
            Statement::Case(x) => {
                visit_writes(&x.default, f);
                for arm in &x.arms {
                    visit_writes(&arm.body, f);
                }
            }
            Statement::For(x) => visit_writes(&x.body, f),
            _ => {}
        }
    }
}

/// Calls `f` with each variable `stmts` read, once per read; false when
/// they hold a testbench method call, whose arguments are not seen.
fn visit_reads(stmts: &[Statement], f: &mut impl FnMut(VarId)) -> bool {
    for stmt in stmts {
        let seen = match stmt {
            Statement::Assign(x) => {
                expr_reads(&x.expr, f);
                for dst in &x.dst {
                    selector_reads(&dst.index, &dst.select, f);
                }
                if let Some(dst) = &x.hier_dst {
                    selector_reads(&dst.index, &dst.select, f);
                }
                true
            }
            Statement::If(x) => {
                expr_reads(&x.cond, f);
                visit_reads(&x.true_side, f) && visit_reads(&x.false_side, f)
            }
            Statement::IfReset(x) => visit_reads(&x.true_side, f) && visit_reads(&x.false_side, f),
            Statement::Case(x) => {
                expr_reads(&x.case_target, f);
                for pattern in x.arms.iter().flat_map(|a| &a.patterns) {
                    match pattern {
                        CasePattern::Eq(x) => expr_reads(x, f),
                        CasePattern::Range { lo, hi, .. } => {
                            expr_reads(lo, f);
                            expr_reads(hi, f);
                        }
                    }
                }
                x.arms.iter().all(|a| visit_reads(&a.body, f)) && visit_reads(&x.default, f)
            }
            Statement::For(x) => {
                range_reads(&x.range, f);
                visit_reads(&x.body, f)
            }
            Statement::FunctionCall(x) => {
                call_reads(x, f);
                true
            }
            Statement::SystemFunctionCall(x) => {
                system_reads(x, f);
                true
            }
            Statement::TbMethodCall(_) => false,
            Statement::Break | Statement::Null | Statement::Unsupported(_) => true,
        };
        if !seen {
            return false;
        }
    }
    true
}

/// Calls `f` with each variable `decl` reads, once per read; false when it
/// holds a testbench method call, whose arguments are not seen.
fn declaration_reads(decl: &Declaration, f: &mut impl FnMut(VarId)) -> bool {
    match decl {
        Declaration::Comb(x) => visit_reads(&x.statements, f),
        Declaration::Ff(x) => {
            f(x.clock.id);
            selector_reads(&x.clock.index, &x.clock.select, f);
            if let Some(reset) = &x.reset {
                f(reset.id);
                selector_reads(&reset.index, &reset.select, f);
            }
            visit_reads(&x.statements, f)
        }
        Declaration::Inst(x) => {
            for x in x.inputs.iter().map(|x| &x.expr) {
                expr_reads(x, f);
            }
            for dst in x.outputs.iter().flat_map(|x| &x.dst) {
                selector_reads(&dst.index, &dst.select, f);
            }
            for x in &x.interface_bindings {
                f(x.parent);
                selector_reads(&x.index, &x.select, f);
            }
            true
        }
        Declaration::External(x) => {
            for x in &x.connects {
                expr_reads(&x.expr, f);
                if let Some(dst) = &x.output {
                    selector_reads(&dst.index, &dst.select, f);
                }
            }
            true
        }
        Declaration::Initial(x) => visit_reads(&x.statements, f),
        Declaration::Final(x) => visit_reads(&x.statements, f),
        Declaration::Unsupported(_) | Declaration::Null => true,
    }
}

fn range_reads(range: &ForRange, f: &mut impl FnMut(VarId)) {
    let (start, end) = range.bounds();
    for bound in [start, end] {
        if let ForBound::Expression(x) = bound {
            expr_reads(x, f);
        }
    }
}

fn selector_reads(index: &VarIndex, select: &VarSelect, f: &mut impl FnMut(VarId)) {
    for x in index.expressions().chain(&select.0) {
        expr_reads(x, f);
    }
    if let Some((_, x)) = &select.1 {
        expr_reads(x, f);
    }
}

fn call_reads(call: &ir::FunctionCall, f: &mut impl FnMut(VarId)) {
    for x in call.inputs.values() {
        expr_reads(x, f);
    }
    for dst in call.outputs.values().flatten() {
        selector_reads(&dst.index, &dst.select, f);
    }
}

fn system_reads(call: &SystemFunctionCall, f: &mut impl FnMut(VarId)) {
    match &call.kind {
        SystemFunctionKind::Bits(x)
        | SystemFunctionKind::Clog2(x)
        | SystemFunctionKind::Onehot(x)
        | SystemFunctionKind::Signed(x)
        | SystemFunctionKind::Unsigned(x) => expr_reads(&x.0, f),
        SystemFunctionKind::Size(x, y) => {
            expr_reads(&x.0, f);
            if let Some(y) = y {
                expr_reads(&y.0, f);
            }
        }
        SystemFunctionKind::Readmemh(x, out) => {
            expr_reads(&x.0, f);
            for dst in out.local() {
                selector_reads(&dst.index, &dst.select, f);
            }
        }
        SystemFunctionKind::Display(args) | SystemFunctionKind::Write(args) => {
            for x in args {
                expr_reads(&x.0, f);
            }
        }
        SystemFunctionKind::Assert { cond, args, .. } => {
            expr_reads(&cond.0, f);
            for x in args {
                expr_reads(&x.0, f);
            }
        }
        SystemFunctionKind::Finish => {}
    }
}

fn expr_reads(expr: &Expression, f: &mut impl FnMut(VarId)) {
    match expr {
        Expression::Term(x) => match x.as_ref() {
            Factor::Variable(id, index, select, _) => {
                f(*id);
                selector_reads(index, select, f);
            }
            Factor::HierVariable(x) => selector_reads(&x.index, &x.select, f),
            Factor::FunctionCall(x) => call_reads(x, f),
            Factor::SystemFunctionCall(x) => system_reads(x, f),
            Factor::Value(_) | Factor::Anonymous(_) | Factor::Unknown(_) => {}
        },
        Expression::Unary(_, x, _) => expr_reads(x, f),
        Expression::Binary(x, _, y, _) => {
            expr_reads(x, f);
            expr_reads(y, f);
        }
        Expression::Ternary(x, y, z, _) => {
            expr_reads(x, f);
            expr_reads(y, f);
            expr_reads(z, f);
        }
        Expression::Concatenation(items, _) => {
            for (x, rep) in items {
                expr_reads(x, f);
                if let Some(rep) = rep {
                    expr_reads(rep, f);
                }
            }
        }
        Expression::ArrayLiteral(items, _) => {
            for item in items {
                match item {
                    ArrayLiteralItem::Value(x, rep) => {
                        expr_reads(x, f);
                        if let Some(rep) = rep {
                            expr_reads(rep, f);
                        }
                    }
                    ArrayLiteralItem::Defaul(x) => expr_reads(x, f),
                }
            }
        }
        Expression::StructConstructor(_, items, _) => {
            for (_, x) in items {
                expr_reads(x, f);
            }
        }
    }
}
