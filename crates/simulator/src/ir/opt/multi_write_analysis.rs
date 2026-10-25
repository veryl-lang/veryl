//! Analyzer-IR multi-RMW write analysis.
//!
//! Walks `air::Declaration` (post comb_to_ff_hoist) and computes
//! per-`(VarId, element_index)` max writes per commit.  An element
//! with ≥2 writes in any commit is "multi-RMW" and requires dual-slot
//! storage (the packed layout cannot be applied).  A runtime-index write
//! counts against every element of its array and is reported at element 0.
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
/// writes in any single commit.  `decls` should be the post-hoist
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
    // Writes accumulate over every declaration that can land in one commit:
    // the `always_ff` blocks on a clock share its event whatever their reset,
    // a copied clock's event joins its source's, and blocks on different
    // clocks sharing an async reset fire in one reset event.
    let mut events: HashMap<EventKey, Vec<&Vec<air::Statement>>> = HashMap::default();

    for decl in decls {
        match decl {
            air::Declaration::Ff(ff) => {
                events.entry(EventKey::Ff).or_default().push(&ff.statements);
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
            let sub = count_writes_seq(stmts, analyzer_ctx, false);
            for (k, n) in sub {
                *event_counts.entry(k).or_insert(0) += n;
            }
        }
        let dynamic: HashMap<VarId, u32> = event_counts
            .iter()
            .filter(|((_, elem), _)| *elem == DYNAMIC)
            .map(|((id, _), n)| (*id, *n))
            .collect();
        for ((id, elem), n) in event_counts {
            let total = if elem == DYNAMIC {
                n
            } else {
                n + dynamic.get(&id).copied().unwrap_or(0)
            };
            if total >= 2 {
                result.insert((id, if elem == DYNAMIC { 0 } else { elem }));
            }
        }
    }

    result
}

#[derive(Eq, PartialEq, Hash, Clone)]
enum EventKey {
    Ff,
    Comb,
}

/// Sequential block: writes sum across statements.
fn count_writes_seq(
    stmts: &[air::Statement],
    ctx: &mut AnalyzerContext,
    in_for: bool,
) -> HashMap<(VarId, usize), u32> {
    let mut result: HashMap<(VarId, usize), u32> = HashMap::default();
    for s in stmts {
        let sub = count_writes_one(s, ctx, in_for);
        for (k, n) in sub {
            *result.entry(k).or_insert(0) += n;
        }
    }
    result
}

/// Branches take per-key max; repeated loop writes require dual-slot storage.
fn count_writes_one(
    stmt: &air::Statement,
    ctx: &mut AnalyzerContext,
    in_for: bool,
) -> HashMap<(VarId, usize), u32> {
    use air::Statement;
    let mut result: HashMap<(VarId, usize), u32> = HashMap::default();
    match stmt {
        Statement::Assign(a) => {
            for dst in &a.dst {
                add_dst_write(dst, ctx, &mut result, in_for);
            }
        }
        Statement::If(i) => {
            let t = count_writes_seq(&i.true_side, ctx, in_for);
            let f = count_writes_seq(&i.false_side, ctx, in_for);
            merge_branches_max(&t, &f, &mut result);
        }
        Statement::Case(c) => {
            let lowered = c.lower_to_nested_if();
            return count_writes_seq(&lowered, ctx, in_for);
        }
        Statement::IfReset(i) => {
            let t = count_writes_seq(&i.true_side, ctx, in_for);
            let f = count_writes_seq(&i.false_side, ctx, in_for);
            merge_branches_max(&t, &f, &mut result);
        }
        Statement::For(f) => {
            let iterations = f.range.eval_iter(ctx);
            if iterations.as_ref().is_some_and(Vec::is_empty) {
                return result;
            }
            let repeated = iterations.as_ref().is_none_or(|values| values.len() > 1);
            return count_writes_seq(&f.body, ctx, in_for || repeated);
        }
        Statement::FunctionCall(call) => {
            for outputs in call.outputs.values() {
                for dst in outputs {
                    add_dst_write(dst, ctx, &mut result, in_for);
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

/// Per-key max, except that an element's count is taken together with its
/// array's runtime-index count, so that their sum stays the worst path's.
fn merge_branches_max(
    a: &HashMap<(VarId, usize), u32>,
    b: &HashMap<(VarId, usize), u32>,
    out: &mut HashMap<(VarId, usize), u32>,
) {
    let dynamic =
        |m: &HashMap<(VarId, usize), u32>, id: VarId| m.get(&(id, DYNAMIC)).copied().unwrap_or(0);
    for k in a.keys().chain(b.keys()) {
        let (id, elem) = *k;
        let (da, db) = (dynamic(a, id), dynamic(b, id));
        let d = da.max(db);
        let n = if elem == DYNAMIC {
            d
        } else {
            let na = a.get(k).copied().unwrap_or(0) + da;
            let nb = b.get(k).copied().unwrap_or(0) + db;
            na.max(nb) - d
        };
        out.insert(*k, n);
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

/// The element key of a write that may land on any element: a runtime index,
/// a whole array, or a partial index into a multi-dimensional one.
const DYNAMIC: usize = usize::MAX;

/// `AssignDestination::gather_ff` element resolution, except that a runtime
/// index is keyed at `DYNAMIC`.
fn add_dst_write(
    dst: &air::AssignDestination,
    ctx: &mut AnalyzerContext,
    out: &mut HashMap<(VarId, usize), u32>,
    in_for: bool,
) {
    let variable = match ctx.get_variable_info(dst.id) {
        Some(v) => v,
        None => return,
    };
    if variable.kind == air::VarKind::Let {
        return;
    }

    // A retained loop can write the same slot on successive iterations. A
    // packed FF merges a PARTIAL write against the slot's pre-edge value, which
    // keeps only the last repeat to land in a word; two writes earn the dual
    // slot the merges forward.  A whole element per iteration needs no merge.
    let n = if in_for { 2 } else { 1 };

    if let Some(idx_vec) = dst.index.eval_value(ctx)
        && let Some(flat) = variable.r#type.array.calc_index(&idx_vec)
    {
        *out.entry((dst.id, flat)).or_insert(0) += n;
        return;
    }

    *out.entry((dst.id, DYNAMIC)).or_insert(0) += n;
}
