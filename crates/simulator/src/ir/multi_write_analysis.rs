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
