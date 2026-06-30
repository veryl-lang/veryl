//! Analyzer-IR multi-RMW write analysis.
//!
//! Walks `air::Declaration` (post comb_to_ff_hoist) and computes
//! per-`(VarId, element_index)` max writes per cycle event.  An element
//! with ≥2 writes in any event is "multi-RMW" and requires dual-slot
//! storage (the packed layout cannot be applied).
//!
//! Operating on analyzer IR lets `create_variable_meta` allocate
//! packed-aware FF storage from the start, avoiding a post-hoc remap
//! pass that would conflict with child-level JIT caches.
//!
//! Result key `(VarId, usize)` matches `air::FfTable` indexing (flat
//! array element index produced by `r#type.array.calc_index`).

use crate::{HashMap, HashSet};
use veryl_analyzer::conv::Context as AnalyzerContext;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::VarId;

/// Returns the set of `(VarId, element_index)` pairs that receive ≥2
/// writes in any single cycle event.  `decls` should be the post-hoist
/// declaration list so comb-to-FF hoist writes are attributed to FF
/// blocks.
///
/// When `force_all_ff` is set (corresponds to `--disable-ff-opt`),
/// `Declaration::Comb` blocks are also analyzed as a synthetic event,
/// matching the simulator side where comb writes become NBA-semantic.
pub fn analyze_multi_write(
    decls: &[air::Declaration],
    analyzer_ctx: &mut AnalyzerContext,
    force_all_ff: bool,
) -> HashSet<(VarId, usize)> {
    // Group declarations by event identity.  Multiple `FfDeclaration`s
    // with the same `(clock, reset)` are merged at simulator level
    // (`events.values()` in `collect_multi_write_offsets`), so writes
    // within them accumulate per cycle.
    let mut events: HashMap<EventKey, Vec<&Vec<air::Statement>>> = HashMap::default();

    for decl in decls {
        match decl {
            air::Declaration::Ff(ff) => {
                let key = EventKey::Ff {
                    clock: ff.clock.id,
                    reset: ff.reset.as_ref().map(|r| r.id),
                };
                events.entry(key).or_default().push(&ff.statements);
            }
            air::Declaration::Comb(c) if force_all_ff => {
                events
                    .entry(EventKey::Comb)
                    .or_default()
                    .push(&c.statements);
            }
            _ => {}
        }
    }

    let mut result: HashSet<(VarId, usize)> = HashSet::default();
    for stmt_lists in events.values() {
        let mut event_counts: HashMap<(VarId, usize), u32> = HashMap::default();
        for stmts in stmt_lists {
            let sub = count_writes_seq(stmts, analyzer_ctx);
            for (k, n) in sub {
                *event_counts.entry(k).or_insert(0) += n;
            }
        }
        for (k, n) in event_counts {
            if n >= 2 {
                result.insert(k);
            }
        }
    }

    result
}

#[derive(Eq, PartialEq, Hash, Clone)]
enum EventKey {
    Ff { clock: VarId, reset: Option<VarId> },
    Comb,
}

/// Sequential block: writes sum across statements.
fn count_writes_seq(
    stmts: &[air::Statement],
    ctx: &mut AnalyzerContext,
) -> HashMap<(VarId, usize), u32> {
    let mut result: HashMap<(VarId, usize), u32> = HashMap::default();
    for s in stmts {
        let sub = count_writes_one(s, ctx);
        for (k, n) in sub {
            *result.entry(k).or_insert(0) += n;
        }
    }
    result
}

/// Per-statement write counts.  Branches take per-key max; for loops
/// approximate one iteration to mirror the simulator-side
/// `collect_max_writes_one`.
fn count_writes_one(
    stmt: &air::Statement,
    ctx: &mut AnalyzerContext,
) -> HashMap<(VarId, usize), u32> {
    use air::Statement;
    let mut result: HashMap<(VarId, usize), u32> = HashMap::default();
    match stmt {
        Statement::Assign(a) => {
            for dst in &a.dst {
                add_dst_write(dst, ctx, &mut result);
            }
        }
        Statement::If(i) => {
            let t = count_writes_seq(&i.true_side, ctx);
            let f = count_writes_seq(&i.false_side, ctx);
            merge_branches_max(&t, &f, &mut result);
        }
        Statement::Case(c) => {
            let lowered = c.lower_to_nested_if();
            return count_writes_seq(&lowered, ctx);
        }
        Statement::IfReset(i) => {
            let t = count_writes_seq(&i.true_side, ctx);
            let f = count_writes_seq(&i.false_side, ctx);
            merge_branches_max(&t, &f, &mut result);
        }
        Statement::For(f) => {
            return count_writes_seq(&f.body, ctx);
        }
        Statement::FunctionCall(call) => {
            for outputs in call.outputs.values() {
                for dst in outputs {
                    add_dst_write(dst, ctx, &mut result);
                }
            }
        }
        Statement::SystemFunctionCall(_)
        | Statement::TbMethodCall(_)
        | Statement::Break
        | Statement::Unsupported(_)
        | Statement::Null => {}
    }
    result
}

fn merge_branches_max(
    a: &HashMap<(VarId, usize), u32>,
    b: &HashMap<(VarId, usize), u32>,
    out: &mut HashMap<(VarId, usize), u32>,
) {
    for (k, n) in a {
        let e = out.entry(*k).or_insert(0);
        if *n > *e {
            *e = *n;
        }
    }
    for (k, n) in b {
        let e = out.entry(*k).or_insert(0);
        if *n > *e {
            *e = *n;
        }
    }
}

/// `VarId`s of arrays written through a runtime (non-const) index.  These are
/// accessed as a `DynamicVariable` needing a uniform single-buffer layout, but
/// the `FfTable` flags only the base element `(id, 0)` of such a write, so
/// `create_variable_meta` force-FFs them.  Statically indexed arrays are left to
/// per-element classification (they may be genuinely mixed comb/FF).
pub fn collect_dyn_indexed_vars(decls: &[air::Declaration]) -> HashSet<VarId> {
    let mut out: HashSet<VarId> = HashSet::default();
    for decl in decls {
        let stmts = match decl {
            air::Declaration::Ff(ff) => &ff.statements,
            air::Declaration::Comb(c) => &c.statements,
            _ => continue,
        };
        for s in stmts {
            collect_dyn_one(s, &mut out);
        }
    }
    out
}

fn collect_dyn_one(stmt: &air::Statement, out: &mut HashSet<VarId>) {
    use air::Statement;
    match stmt {
        Statement::Assign(a) => {
            for dst in &a.dst {
                check_dyn_dst(dst, out);
            }
        }
        Statement::If(i) => {
            for s in i.true_side.iter().chain(&i.false_side) {
                collect_dyn_one(s, out);
            }
        }
        Statement::IfReset(i) => {
            for s in i.true_side.iter().chain(&i.false_side) {
                collect_dyn_one(s, out);
            }
        }
        Statement::Case(c) => {
            for s in c.lower_to_nested_if() {
                collect_dyn_one(&s, out);
            }
        }
        Statement::For(f) => {
            for s in &f.body {
                collect_dyn_one(s, out);
            }
        }
        Statement::FunctionCall(call) => {
            for outputs in call.outputs.values() {
                for dst in outputs {
                    check_dyn_dst(dst, out);
                }
            }
        }
        _ => {}
    }
}

/// Flags `dst.id` on a non-const (runtime) array index.  Uses `is_const`, not
/// `eval_value`, which would wrongly resolve a runtime `For` loop variable.
fn check_dyn_dst(dst: &air::AssignDestination, out: &mut HashSet<VarId>) {
    if !dst.index.is_const() {
        out.insert(dst.id);
    }
}

/// Mirror of `AssignDestination::gather_ff` element resolution.  A
/// non-const index in the simulator becomes `AssignDynamic` at
/// `base_offset = element[0].current_offset`, so we key the
/// indeterminate case at `(id, 0)` to match.
fn add_dst_write(
    dst: &air::AssignDestination,
    ctx: &mut AnalyzerContext,
    out: &mut HashMap<(VarId, usize), u32>,
) {
    let variable = match ctx.get_variable_info(dst.id) {
        Some(v) => v,
        None => return,
    };
    if variable.kind == air::VarKind::Let {
        return;
    }

    if let Some(idx_vec) = dst.index.eval_value(ctx)
        && let Some(flat) = variable.r#type.array.calc_index(&idx_vec)
    {
        *out.entry((dst.id, flat)).or_insert(0) += 1;
        return;
    }

    *out.entry((dst.id, 0)).or_insert(0) += 1;
}
