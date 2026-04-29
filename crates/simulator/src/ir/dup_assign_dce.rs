// DSE: drop earlier same-dst full-width ProtoStatement::Assign stores
// within unified_sorted whose value is overwritten before any
// intervening read of dst (last-write-wins under SystemVerilog
// sequential semantics).

use super::ProtoStatement;

/// Aggressive DCE: drop a same-dst earlier ProtoStatement::Assign even
/// when other stmts intervene, as long as no intervening stmt READS
/// dst.  Provably equivalent to sequential semantics: the second
/// store's value would overwrite the first, and no intervening stmt
/// could observe the first's value.
///
/// Applied per sibling sequence; recursive into nested structures with
/// state reset at boundaries.
pub fn dce_aggressive(stmts: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    let mut out: Vec<ProtoStatement> = Vec::with_capacity(stmts.len());
    let mut alive: Vec<bool> = Vec::with_capacity(stmts.len());
    // dst.raw() -> index into out (still pending: can be overwritten by
    // a later same-dst write iff no intervening read of dst).
    let mut pending: std::collections::HashMap<isize, usize> =
        std::collections::HashMap::new();

    for s in stmts {
        let s = match s {
            ProtoStatement::SequentialBlock(body) => {
                pending.clear();
                ProtoStatement::SequentialBlock(dce_aggressive(body))
            }
            ProtoStatement::CompiledBlock(mut cb) => {
                pending.clear();
                if !cb.original_stmts.is_empty() {
                    cb.original_stmts = dce_aggressive(cb.original_stmts);
                }
                ProtoStatement::CompiledBlock(cb)
            }
            ProtoStatement::If(mut if_stmt) => {
                pending.clear();
                if_stmt.true_side = dce_aggressive(if_stmt.true_side);
                if_stmt.false_side = dce_aggressive(if_stmt.false_side);
                ProtoStatement::If(if_stmt)
            }
            ProtoStatement::For(mut for_stmt) => {
                pending.clear();
                for_stmt.body = dce_aggressive(for_stmt.body);
                ProtoStatement::For(for_stmt)
            }
            other => other,
        };

        // Gather inputs/outputs of this stmt to update pending.
        let mut ins = Vec::new();
        let mut outs = Vec::new();
        s.gather_variable_offsets(&mut ins, &mut outs);

        // Any read of a pending dst forces that pending to commit (live).
        for r in &ins {
            pending.remove(&r.raw());
        }

        // For ProtoStatement::Assign with full-width write: if an earlier
        // pending write to same dst exists, the earlier is dead.  Replace
        // its alive flag with false; record this stmt as the new pending.
        if let ProtoStatement::Assign(a) = &s
            && a.select.is_none()
            && a.dynamic_select.is_none()
            && a.rhs_select.is_none()
        {
            let dst = a.dst.raw();
            if let Some(&prev_idx) = pending.get(&dst) {
                alive[prev_idx] = false;
            }
            out.push(s);
            alive.push(true);
            pending.insert(dst, out.len() - 1);
            continue;
        }

        // Any other stmt with outputs (Assign with select, AssignDynamic,
        // For with body that writes, etc.): conservatively commit those
        // dsts as live and remove from pending (since partial writes may
        // overlap).
        for o in &outs {
            pending.remove(&o.raw());
        }

        out.push(s);
        alive.push(true);
    }

    out.into_iter()
        .zip(alive)
        .filter_map(|(s, a)| if a { Some(s) } else { None })
        .collect()
}
