// Hoist comb-side `let` writes into the FF declaration that consumes
// them.  When a comb-side variable is read by exactly one always_ff
// block (and by nothing else), folding its assignment into the FF body
// removes the per-cycle JIT chunk-dispatch overhead that would otherwise
// be spent settling that net during the comb phase before the FF event
// fires.
//
// Restricted to `Let`-kind direct single-FF readers so the move from
// comb to ff context preserves blocking-assignment semantics.  Variables
// that would flip BA→NBA are skipped.

use crate::HashMap;
use crate::ir::{Declaration, FfTable, Statement, VarId, VarKind, Variable};

/// One unit of hoist work: move the comb-side assignment of `(var_id,
/// var_index)` from `comb_decl_idx` into the body of `ff_decl_idx`.
#[derive(Clone)]
pub struct HoistPlan {
    pub var_id: VarId,
    pub var_index: usize,
    /// Index into Module::declarations where the comb-side write lives.
    pub comb_decl_idx: usize,
    /// Index into Module::declarations of the FF declaration that consumes
    /// this variable.  Must be a `Declaration::Ff`.
    pub ff_decl_idx: usize,
    /// Variable kind at the time of planning.  `Let` is the only safe
    /// case — its BA semantics don't change when moved from comb to ff
    /// context.  `Variable` etc. would silently flip to NBA if hoisted
    /// naively, so they are skipped.
    pub var_kind: VarKind,
}

/// Build the hoist plan for a module.  Covers only the simplest case:
/// variables assigned in exactly one always_comb declaration and read
/// exclusively by exactly one always_ff declaration.
pub fn plan_hoists(
    declarations: &[Declaration],
    ff_table: &FfTable,
    variables: &HashMap<VarId, Variable>,
) -> Vec<HoistPlan> {
    let mut ff_decls: HashMap<usize, ()> = HashMap::default();
    for (i, decl) in declarations.iter().enumerate() {
        if matches!(decl, Declaration::Ff(_)) {
            ff_decls.insert(i, ());
        }
    }

    let mut plans = Vec::new();
    for ((var_id, var_index), entry) in &ff_table.table {
        let Some(comb_decl_idx) = entry.assigned_comb else {
            continue;
        };
        if entry.refered.is_empty() {
            continue;
        }
        // Direct hoist criteria: every reader is from an always_ff block
        // and they all share a single decl index.
        let mut reader_decls: Vec<usize> = entry
            .refered
            .iter()
            .filter_map(|(d, _, from_ff)| if *from_ff { Some(*d) } else { None })
            .collect();
        if reader_decls.len() != entry.refered.len() {
            continue;
        }
        reader_decls.sort_unstable();
        reader_decls.dedup();
        if reader_decls.len() != 1 {
            continue;
        }
        let ff_decl_idx = reader_decls[0];
        if !ff_decls.contains_key(&ff_decl_idx) {
            continue;
        }
        let var_kind = match variables.get(var_id) {
            Some(v) => v.kind,
            None => continue,
        };
        plans.push(HoistPlan {
            var_id: *var_id,
            var_index: *var_index,
            comb_decl_idx,
            ff_decl_idx,
            var_kind,
        });
    }
    plans
}

/// Apply hoists by mutating `declarations` in place.
///
/// Restricted to `Let`-kind plans because `Let` variables retain blocking
/// (BA) semantics whether they live in always_comb or always_ff context.
/// `Variable`-kind plans would silently flip from BA to NBA and break
/// correctness, so they're skipped here.
///
/// Returns the number of hoists actually applied.  May be less than
/// `plans.len()` if some assigns are nested inside If/For (which is not
/// handled here) or if the comb-side write can't be located.
pub fn apply_hoists(
    declarations: &mut [Declaration],
    plans: &[HoistPlan],
    variables: &HashMap<VarId, Variable>,
) -> usize {
    // Optional bisect knobs (VERYL_COMB_HOIST_LIMIT / _SKIP) used to pin
    // a specific hoist when a regression is suspected.  Counter is
    // process-wide thread_local so per-module limits compose cleanly.
    use std::cell::Cell;
    thread_local! {
        static GLOBAL_APPLIED: Cell<usize> = const { Cell::new(0) };
        static GLOBAL_SEEN: Cell<usize> = const { Cell::new(0) };
        static GLOBAL_LIMIT: Cell<Option<usize>> = const { Cell::new(None) };
        static GLOBAL_SKIP: Cell<usize> = const { Cell::new(0) };
        static LIMIT_READ: Cell<bool> = const { Cell::new(false) };
    }
    if !LIMIT_READ.with(|c| c.get()) {
        GLOBAL_LIMIT.with(|c| {
            c.set(
                std::env::var("VERYL_COMB_HOIST_LIMIT")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok()),
            )
        });
        GLOBAL_SKIP.with(|c| {
            c.set(
                std::env::var("VERYL_COMB_HOIST_SKIP")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0),
            )
        });
        LIMIT_READ.with(|c| c.set(true));
    }

    // Group plans by ff_decl_idx → list of (var_id, var_index, comb_decl_idx).
    let mut by_ff: HashMap<usize, Vec<(VarId, usize, usize)>> = HashMap::default();
    for p in plans {
        if !matches!(p.var_kind, VarKind::Let) {
            continue;
        }
        let seen = GLOBAL_SEEN.with(|c| {
            let v = c.get();
            c.set(v + 1);
            v
        });
        if seen < GLOBAL_SKIP.with(|c| c.get()) {
            continue;
        }
        if let Some(lim) = GLOBAL_LIMIT.with(|c| c.get())
            && GLOBAL_APPLIED.with(|c| c.get()) >= lim
        {
            break;
        }
        by_ff
            .entry(p.ff_decl_idx)
            .or_default()
            .push((p.var_id, p.var_index, p.comb_decl_idx));
        GLOBAL_APPLIED.with(|c| c.set(c.get() + 1));
    }

    let trace = std::env::var("VERYL_COMB_HOIST_TRACE").ok().as_deref() == Some("1");
    let mut applied = 0usize;
    for (ff_idx, items) in by_ff {
        // Extract from each comb decl in document order so the
        // topological flow is preserved.
        let mut hoisted_stmts: Vec<Statement> = Vec::new();
        for (var_id, var_index, comb_idx) in &items {
            let extracted = if let Some(Declaration::Comb(comb)) = declarations.get_mut(*comb_idx) {
                extract_top_level_assign(&mut comb.statements, *var_id, *var_index)
            } else {
                None
            };
            if let Some(stmt) = extracted {
                if trace {
                    let path = variables
                        .get(var_id)
                        .map(|v| v.path.to_string())
                        .unwrap_or_else(|| "?".to_string());
                    eprintln!(
                        "[CombHoistTrace] hoist {path} var={:?}[{}] comb_idx={} ff_idx={}",
                        var_id, var_index, comb_idx, ff_idx
                    );
                }
                hoisted_stmts.push(stmt);
                applied += 1;
            } else if trace {
                eprintln!(
                    "[CombHoistTrace] FAILED to extract var={:?}[{}] comb_idx={}",
                    var_id, var_index, comb_idx
                );
            }
            // If extraction failed (nested in If/For etc.), skip silently
            // — the FfTable rebuild will see the comb assignment is still
            // there and the var will remain comb-classified.
        }
        if hoisted_stmts.is_empty() {
            continue;
        }
        if let Some(Declaration::Ff(ff)) = declarations.get_mut(ff_idx) {
            inject_into_ff_body(&mut ff.statements, hoisted_stmts);
        }
    }
    applied
}

/// Find and remove an `AssignStatement` at the top level of `stmts`
/// whose first `dst` matches `var_id`.  Returns the removed
/// `Statement::Assign` on success.  Does not descend into
/// If/For/SequentialBlock and matches on `var_id` only — for scalar
/// `Let` bindings the index is always 0 and there is exactly one writer.
fn extract_top_level_assign(
    stmts: &mut Vec<Statement>,
    var_id: VarId,
    _var_index: usize,
) -> Option<Statement> {
    let pos = stmts.iter().position(|s| match s {
        Statement::Assign(a) => a.dst.first().map(|d| d.id == var_id).unwrap_or(false),
        _ => false,
    })?;
    Some(stmts.remove(pos))
}

fn inject_into_ff_body(stmts: &mut Vec<Statement>, hoisted: Vec<Statement>) {
    // If the first statement is an IfReset, prepend into its false_side
    // (clock-only path) so the hoisted comb assignment runs only on the
    // active clock edge, not during reset.
    if let Some(Statement::IfReset(ifr)) = stmts.first_mut() {
        let mut new_false = hoisted;
        new_false.append(&mut ifr.false_side);
        ifr.false_side = new_false;
        return;
    }
    // No reset gate → prepend at the front of the FF body.
    let mut new_stmts = hoisted;
    new_stmts.append(stmts);
    *stmts = new_stmts;
}
