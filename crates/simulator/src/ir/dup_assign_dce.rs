// Dead-store elimination on full-width ProtoStatement::Assign stores.
// Drops an earlier write to dst when a later write to the same dst
// occurs without any intervening read of dst — under SystemVerilog
// sequential (last-write-wins) semantics the earlier value is
// unobservable.

use super::ProtoStatement;
use crate::ir::variable::VarOffset;

/// Drop an earlier ProtoStatement::Assign whose dst is overwritten by a
/// later same-dst full-width Assign with no intervening read of dst.
///
/// Applied per sibling sequence; recurses into nested structures with
/// pending state cleared at each boundary.
pub fn dce_aggressive(stmts: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    let mut out: Vec<ProtoStatement> = Vec::with_capacity(stmts.len());
    let mut alive: Vec<bool> = Vec::with_capacity(stmts.len());
    // Key must be the full VarOffset (FF/Comb tag + offset), not just
    // the raw offset, because Ff(N) and Comb(N) are distinct storage
    // locations that can share a raw offset within their respective
    // regions (e.g. when force_all_ff promotes a variable that already
    // has a comb intermediate at the same raw offset).
    let mut pending: std::collections::HashMap<VarOffset, usize> = std::collections::HashMap::new();

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

        let mut ins = Vec::new();
        let mut outs = Vec::new();
        s.gather_variable_offsets(&mut ins, &mut outs);

        // A read of a pending dst commits it (the earlier write is live).
        for r in &ins {
            pending.remove(r);
        }

        // Full-width Assign: if there is a pending earlier write to the
        // same dst, mark it dead and replace pending with this one.
        if let ProtoStatement::Assign(a) = &s
            && a.select.is_none()
            && a.dynamic_select.is_none()
            && a.rhs_select.is_none()
        {
            let dst = a.dst;
            if let Some(&prev_idx) = pending.get(&dst) {
                alive[prev_idx] = false;
            }
            out.push(s);
            alive.push(true);
            pending.insert(dst, out.len() - 1);
            continue;
        }

        // Partial writes (Assign with select, AssignDynamic, For body
        // writes, etc.) may overlap an earlier full-width write, so the
        // earlier write must be kept alive.
        for o in &outs {
            pending.remove(o);
        }

        out.push(s);
        alive.push(true);
    }

    out.into_iter()
        .zip(alive)
        .filter_map(|(s, a)| if a { Some(s) } else { None })
        .collect()
}
