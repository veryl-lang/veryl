//! Self-references an always_ff element cannot make without a register.
//!
//! Dropping the register is sound only while a self-reference still sees the
//! value the block started with.  Statements write in place, so a write that
//! has already run is what the read gets: `s = s + 1; s = s + 1` would
//! increment twice, and `s[7:0] = a; s[15:8] = s[15:8] + s[7:0]` would take
//! the new `s[7:0]`.  Both the overlap and the order are needed —
//! `x = x + 1; if c { x = 0 }` reads nothing it has written, and a shift run
//! from the far end (`for i in rev 0..N { q[i + 1] = q[i] }`) reaches each
//! bit only after reading it.
//!
//! Branch arms are alternatives, so what leaves a branch is the union of what
//! its arms may have written.  Const bounds are walked per iteration, which is
//! what makes each index and select concrete; without them every write the
//! body holds counts as already done, since it runs again.

use crate::BigUint;
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::{AssignDestination, Declaration, FfTable, Statement, VarId, VarKind};
use crate::symbol::Affiliation;
use crate::value::{Value, ValueBigUint};

/// Elements that must keep their register, as `(declaration index, VarId,
/// array element)`.
pub type UnsafeSelfReads = HashSet<(usize, VarId, usize)>;

/// Bits touched at one element; `None` is unknown, taken as every bit.
type Mask = Option<BigUint>;

/// Bits written so far on the path being walked.
type Written = HashMap<(VarId, usize), Mask>;

pub fn unsafe_self_reads(decls: &[Declaration], context: &mut Context) -> UnsafeSelfReads {
    let mut result = UnsafeSelfReads::default();
    for (decl, x) in decls.iter().enumerate() {
        if let Declaration::Ff(ff) = x {
            let mut written = Written::default();
            walk_seq(&ff.statements, decl, context, &mut written, &mut result);
        }
    }
    result
}

fn overlaps(read: &Mask, written: &Mask) -> bool {
    match (read, written) {
        (Some(r), Some(w)) => (r & w) != BigUint::ZERO,
        _ => true,
    }
}

fn merge(into: &mut Mask, from: Mask) {
    match (into.as_mut(), from) {
        (Some(a), Some(b)) => *a |= b,
        _ => *into = None,
    }
}

fn walk_seq(
    stmts: &[Statement],
    decl: usize,
    context: &mut Context,
    written: &mut Written,
    out: &mut UnsafeSelfReads,
) {
    for s in stmts {
        walk_one(s, decl, context, written, out);
    }
}

fn walk_one(
    stmt: &Statement,
    decl: usize,
    context: &mut Context,
    written: &mut Written,
    out: &mut UnsafeSelfReads,
) {
    match stmt {
        Statement::Assign(x) => {
            let mut dsts: Vec<((VarId, usize), Mask)> = Vec::new();
            for dst in &x.dst {
                add_dst(dst, context, &mut dsts);
            }
            // The source runs before the store, so only an earlier
            // statement's write can be observed.
            let reads = element_reads(&x.expr, decl, context);
            for (key, _) in &dsts {
                if let Some(read) = reads.get(key)
                    && let Some(prior) = written.get(key)
                    && overlaps(read, prior)
                {
                    out.insert((decl, key.0, key.1));
                }
            }
            for (key, mask) in dsts {
                merge(
                    written
                        .entry(key)
                        .or_insert_with(|| Some(BigUint::default())),
                    mask,
                );
            }
        }
        Statement::If(x) => walk_branches(&x.true_side, &x.false_side, decl, context, written, out),
        Statement::IfReset(x) => {
            walk_branches(&x.true_side, &x.false_side, decl, context, written, out)
        }
        Statement::Case(x) => walk_seq(&x.lower_to_nested_if(), decl, context, written, out),
        Statement::For(x) => {
            if let Some(iter) = x.range.eval_iter(context) {
                for i in iter {
                    if let Some(var) = context.variables.get_mut(&x.var_id)
                        && let Some(total_width) = x.var_type.total_width()
                    {
                        let val = Value::new(i as u64, total_width, x.var_type.signed);
                        var.set_value(&[], val, None);
                    }
                    walk_seq(&x.body, decl, context, written, out);
                }
            } else {
                // Without concrete iterations there is no order to walk, and
                // the body runs again.
                let mut body: Vec<((VarId, usize), Mask)> = Vec::new();
                collect_writes(&x.body, context, &mut body);
                for (key, mask) in body {
                    merge(
                        written
                            .entry(key)
                            .or_insert_with(|| Some(BigUint::default())),
                        mask,
                    );
                }
                walk_seq(&x.body, decl, context, written, out);
            }
        }
        Statement::FunctionCall(x) => {
            // The body is opaque here, so an output may read back bits a
            // write has already reached.
            for outputs in x.outputs.values() {
                let mut dsts: Vec<((VarId, usize), Mask)> = Vec::new();
                for dst in outputs {
                    add_dst(dst, context, &mut dsts);
                }
                for (key, mask) in dsts {
                    if let Some(prior) = written.get(&key)
                        && overlaps(&mask, prior)
                    {
                        out.insert((decl, key.0, key.1));
                    }
                    merge(
                        written
                            .entry(key)
                            .or_insert_with(|| Some(BigUint::default())),
                        mask,
                    );
                }
            }
        }
        Statement::SystemFunctionCall(_)
        | Statement::TbMethodCall(_)
        | Statement::Break
        | Statement::Unsupported(_)
        | Statement::Null => {}
    }
}

fn walk_branches(
    true_side: &[Statement],
    false_side: &[Statement],
    decl: usize,
    context: &mut Context,
    written: &mut Written,
    out: &mut UnsafeSelfReads,
) {
    let mut wt = written.clone();
    let mut wf = std::mem::take(written);
    walk_seq(true_side, decl, context, &mut wt, out);
    walk_seq(false_side, decl, context, &mut wf, out);
    for (key, mask) in wf {
        merge(
            wt.entry(key).or_insert_with(|| Some(BigUint::default())),
            mask,
        );
    }
    *written = wt;
}

/// Everything the statements may write, ignoring order.
fn collect_writes(
    stmts: &[Statement],
    context: &mut Context,
    out: &mut Vec<((VarId, usize), Mask)>,
) {
    for s in stmts {
        match s {
            Statement::Assign(x) => {
                for dst in &x.dst {
                    add_dst(dst, context, out);
                }
            }
            Statement::If(x) => {
                collect_writes(&x.true_side, context, out);
                collect_writes(&x.false_side, context, out);
            }
            Statement::IfReset(x) => {
                collect_writes(&x.true_side, context, out);
                collect_writes(&x.false_side, context, out);
            }
            Statement::Case(x) => collect_writes(&x.lower_to_nested_if(), context, out),
            Statement::For(x) => collect_writes(&x.body, context, out),
            Statement::FunctionCall(x) => {
                for outputs in x.outputs.values() {
                    for dst in outputs {
                        add_dst(dst, context, out);
                    }
                }
            }
            Statement::SystemFunctionCall(_)
            | Statement::TbMethodCall(_)
            | Statement::Break
            | Statement::Unsupported(_)
            | Statement::Null => {}
        }
    }
}

/// Bits an expression reads per element, gathered the way the table gathers
/// them so the keys and masks line up.
fn element_reads(
    expr: &crate::ir::Expression,
    decl: usize,
    context: &mut Context,
) -> HashMap<(VarId, usize), Mask> {
    let mut table = FfTable::default();
    expr.gather_ff(context, &mut table, decl, None, true);
    let mut out: HashMap<(VarId, usize), Mask> = HashMap::default();
    for (key, entry) in table.table {
        let mut mask: Mask = Some(BigUint::default());
        for (_, _, src_read_mask, _) in entry.refered {
            // The gather leaves an empty mask when the range is not const.
            if src_read_mask == BigUint::ZERO {
                mask = None;
                break;
            }
            merge(&mut mask, Some(src_read_mask));
        }
        out.insert(key, mask);
    }
    out
}

/// Mirrors `AssignDestination`'s gather so a write always has a matching
/// table entry.
fn add_dst(dst: &AssignDestination, context: &mut Context, out: &mut Vec<((VarId, usize), Mask)>) {
    let Some(variable) = context.get_variable_info(dst.id) else {
        return;
    };
    if variable.kind == VarKind::Let || variable.affiliation == Affiliation::AlwaysFf {
        return;
    }
    let r#type = variable.r#type.clone();
    let mask: Mask = dst
        .select
        .eval_value(context, &r#type, false)
        .map(|(beg, end)| ValueBigUint::gen_mask_range(beg, end));
    if let Some(index) = dst.index.eval_value(context) {
        if let Some(variable) = context.get_variable_info(dst.id)
            && let Some(index) = variable.r#type.array.calc_index(&index)
        {
            out.push(((dst.id, index), mask));
        }
    } else if let Some(total_array) = context
        .get_variable_info(dst.id)
        .and_then(|v| v.r#type.total_array())
    {
        for i in 0..total_array {
            out.push(((dst.id, i), mask.clone()));
        }
    }
}
