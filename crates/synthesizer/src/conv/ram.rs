//! RAM inference.
//!
//! Large arrays written through a single dynamic-address port and read through
//! dynamic addresses are far cheaper as a RAM macro than as `depth × width`
//! flip-flops plus address decode/mux trees. This module detects the arrays
//! that qualify; the actual port wiring is built during statement/expression
//! conversion (`conv::statement` / `conv::expression`) and assembled in
//! `conv::finalize`.
//!
//! Criteria (thresholds come from [`RamConfig`], set in `Veryl.toml`'s
//! `[synth]` section):
//!   * array variable, at least `min_bits` stored bits and depth ≥ 2;
//!   * 1..=`max_write_ports` *clocked* writes `mem[addr] = data`, each a
//!     single destination, single dynamic index dimension, whole-word (no
//!     bit/part select). A multi-write array (cache data filled two words per
//!     beat, a superscalar register file) gets one port per write site;
//!   * every read is also `mem[addr]` — single dynamic index, whole word — and
//!     there are at most `max_read_ports` distinct read addresses.
//!
//! A reset (`if_reset`) array stays flip-flops: real SRAM has no reset, so an
//! array meant to be SRAM is written reset-less in the RTL. (Small reset
//! bit-arrays such as a cache `valid` stay as flops regardless via the
//! `min_bits` floor.)
//!
//! Partial / sub-word writes remain out of scope and keep the array as
//! flip-flops.
//!
//! A dynamically-indexed array that fails inference but exceeds `max_ff_bits`
//! is rejected rather than expanded into the `O(depth × width)` gates that
//! would exhaust memory ([`oversized_ff_array`], issue #2941).

use std::collections::HashMap;
use std::fmt::Write;

use veryl_analyzer::ir::{
    self as air, AssignDestination, Declaration, Expression, Factor, Statement,
};

use crate::RamConfig;

/// A variable that passed inference. `ram_idx` is its stable index into
/// `GateModule::ram_blocks`, assigned by ascending `VarId` so net `RamRead`
/// drivers can reference it before the block is assembled.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RamCandidate {
    pub ram_idx: usize,
    pub depth: usize,
    pub width: usize,
}

/// Detect every array variable in `module` that qualifies for RAM inference.
/// A reset array stays flip-flops — an array meant to be SRAM is written
/// reset-less in the RTL (real SRAM has no reset).
pub(crate) fn infer_ram_vars(
    module: &air::Module,
    ram: &RamConfig,
) -> HashMap<air::VarId, RamCandidate> {
    // Stable ordering: sort candidate VarIds so ram_idx assignment is
    // deterministic regardless of HashMap iteration order.
    let mut accepted: Vec<(air::VarId, usize, usize)> = Vec::new();

    let mut ids: Vec<air::VarId> = module.variables.keys().copied().collect();
    ids.sort();

    for vid in ids {
        let Some(var) = module.variables.get(&vid) else {
            continue;
        };
        let Some(width) = var.r#type.total_width() else {
            continue;
        };
        let Some(depth) = var.r#type.array.total() else {
            continue;
        };
        // Reads/writes address with a single index, so only 1-D arrays map to a
        // memory. A multi-dim array indexed below full depth would be mis-shaped
        // and its word truncated to the element width.
        if var.r#type.array.dims() != 1 {
            continue;
        }
        if depth < 2 || width == 0 || width * depth < ram.min_bits {
            continue;
        }

        if !write_pattern_ok(module, vid, ram) {
            continue;
        }
        if !read_pattern_ok(module, vid, ram) {
            continue;
        }
        accepted.push((vid, depth, width));
    }

    accepted
        .into_iter()
        .enumerate()
        .map(|(ram_idx, (vid, depth, width))| {
            (
                vid,
                RamCandidate {
                    ram_idx,
                    depth,
                    width,
                },
            )
        })
        .collect()
}

/// The first array that would blow up if expanded into flip-flops: dynamically
/// indexed, over `ram.max_ff_bits` stored bits, and not inferred as RAM — so the
/// caller can reject it (#2941) instead of building `depth × width` gates.
/// Only 1-D: a dynamic multi-dim index errors elsewhere, and static indexing
/// gives a flat FF bank (linear, no decode). `VarId` order for determinism.
pub(crate) fn oversized_ff_array(
    module: &air::Module,
    ram_vars: &HashMap<air::VarId, RamCandidate>,
    ram: &RamConfig,
) -> Option<(air::VarId, usize)> {
    let limit = ram.max_ff_bits;
    let mut ids: Vec<air::VarId> = module.variables.keys().copied().collect();
    ids.sort();

    for vid in ids {
        if ram_vars.contains_key(&vid) {
            continue;
        }
        let Some(var) = module.variables.get(&vid) else {
            continue;
        };
        let Some(width) = var.r#type.total_width() else {
            continue;
        };
        let Some(depth) = var.r#type.array.total() else {
            continue;
        };
        if depth < 2 || width == 0 || var.r#type.array.dims() != 1 {
            continue;
        }
        let bits = width * depth;
        if bits <= limit {
            continue;
        }
        if is_dynamically_indexed(module, vid) {
            return Some((vid, bits));
        }
    }
    None
}

/// Does `vid` have any dynamic (non-constant) array-index access?
fn is_dynamically_indexed(module: &air::Module, vid: air::VarId) -> bool {
    let is_dyn = |index: &air::VarIndex| !index.0.is_empty() && !index.is_const();

    for decl in &module.declarations {
        let mut dynamic = false;
        match decl {
            Declaration::Ff(ff) => {
                for st in &ff.statements {
                    walk_writes(st, vid, false, &mut |dst, _, _| {
                        if is_dyn(&dst.index) {
                            dynamic = true;
                        }
                    });
                }
            }
            Declaration::Comb(comb) => {
                for st in &comb.statements {
                    walk_writes(st, vid, false, &mut |dst, _, _| {
                        if is_dyn(&dst.index) {
                            dynamic = true;
                        }
                    });
                }
            }
            // An inst-output `foo.o: arr[i]` writes at a dynamic index that
            // `flatten_inst` expands like any clocked/comb write.
            Declaration::Inst(inst) => {
                for o in &inst.outputs {
                    for d in &o.dst {
                        if d.id == vid && is_dyn(&d.index) {
                            dynamic = true;
                        }
                    }
                }
            }
            _ => {}
        }
        if dynamic {
            return true;
        }
    }

    let mut dynamic = false;
    for decl in &module.declarations {
        for_each_read_in_decl(decl, vid, &mut |index, _select| {
            if is_dyn(index) {
                dynamic = true;
            }
        });
    }
    dynamic
}

/// Does any statement in `stmts` (recursively) write a RAM-inferred variable?
/// `synth_case_chain` uses this to bail when an arm writes a RAM. Cheap no-op
/// when `ram_vars` is empty — guard on that first.
pub(crate) fn stmts_write_ram(
    ram_vars: &HashMap<air::VarId, RamCandidate>,
    stmts: &[Statement],
) -> bool {
    stmts.iter().any(|s| stmt_writes_ram(ram_vars, s))
}

fn stmt_writes_ram(ram_vars: &HashMap<air::VarId, RamCandidate>, stmt: &Statement) -> bool {
    match stmt {
        Statement::Assign(a) => a.dst.iter().any(|d| ram_vars.contains_key(&d.id)),
        Statement::If(i) => {
            stmts_write_ram(ram_vars, &i.true_side) || stmts_write_ram(ram_vars, &i.false_side)
        }
        Statement::IfReset(i) => {
            stmts_write_ram(ram_vars, &i.true_side) || stmts_write_ram(ram_vars, &i.false_side)
        }
        Statement::Case(c) => {
            c.arms
                .iter()
                .any(|arm| stmts_write_ram(ram_vars, &arm.body))
                || stmts_write_ram(ram_vars, &c.default)
        }
        Statement::For(fs) => stmts_write_ram(ram_vars, &fs.body),
        Statement::FunctionCall(call) => call
            .outputs
            .values()
            .any(|dsts| dsts.iter().any(|d| ram_vars.contains_key(&d.id))),
        _ => false,
    }
}

/// 1..=`max_write_ports` clocked writes, each a single dynamic whole-word
/// destination, no comb / static-index write. A reset (`if_reset`) write
/// disqualifies the array (real SRAM has no reset; an SRAM-intended array is
/// written reset-less in the RTL).
fn write_pattern_ok(module: &air::Module, vid: air::VarId, ram: &RamConfig) -> bool {
    let mut clocked_writes = 0usize;
    let mut ok = true;

    for decl in &module.declarations {
        match decl {
            Declaration::Ff(ff) => {
                for st in &ff.statements {
                    walk_writes(st, vid, false, &mut |dst, in_reset, dst_count| {
                        if in_reset {
                            // Reset assignment → keep as flip-flops.
                            ok = false;
                            return;
                        }
                        clocked_writes += 1;
                        if dst_count != 1 || !is_dynamic_whole_word(dst) {
                            ok = false;
                        }
                    });
                }
            }
            Declaration::Comb(comb) => {
                // A combinational driver makes it a wire/latch, not a RAM.
                for st in &comb.statements {
                    walk_writes(st, vid, false, &mut |_, _, _| ok = false);
                }
            }
            Declaration::Inst(inst) => {
                // A submodule output driving the array is a continuous (unclocked)
                // driver, not a memory write — and `flatten_inst` would still
                // route it through `record_ram_write`. Reject it.
                for o in &inst.outputs {
                    if o.dst.iter().any(|d| d.id == vid) {
                        ok = false;
                    }
                }
            }
            _ => {}
        }
        if !ok {
            return false;
        }
    }

    ok && (1..=ram.max_write_ports).contains(&clocked_writes)
}

/// Every read of `vid` is a single dynamic whole-word `mem[addr]`, with at most
/// `max_read_ports` distinct addresses.
fn read_pattern_ok(module: &air::Module, vid: air::VarId, ram: &RamConfig) -> bool {
    // Collect (whole_word_dynamic?, addr_key) for every read, then validate —
    // collecting first sidesteps borrowing `ok` across the visitor closure.
    // A trailing bit/part select (`mem[addr][1]`) is fine — the port returns
    // the whole word and the select is applied to the read data. Only the
    // address dimension must be a single dynamic index.
    let mut reads: Vec<(bool, String)> = Vec::new();
    for decl in &module.declarations {
        for_each_read_in_decl(decl, vid, &mut |index, _select| {
            let ok = index.0.len() == 1 && !index.is_const();
            let key = if ok {
                addr_signature(&index.0[0])
            } else {
                String::new()
            };
            reads.push((ok, key));
        });
    }

    if reads.is_empty() || reads.iter().any(|(ok, _)| !ok) {
        return false;
    }
    let mut addrs: Vec<&String> = reads.iter().map(|(_, k)| k).collect();
    addrs.sort();
    addrs.dedup();
    addrs.len() <= ram.max_read_ports
}

/// Source-location-independent signature of an address expression, so reads of
/// the same address at different sites dedup to one read port. `format!("{:?}",
/// expr)` keys them apart because `Comptime` embeds a `TokenRange`. Injective
/// across distinct addresses, so two different addresses never collide into one
/// port. Shared by [`read_pattern_ok`] (counting ports) and `read_ram`
/// (allocating them); both must agree.
pub(crate) fn addr_signature(expr: &Expression) -> String {
    let mut s = String::new();
    write_expr_sig(&mut s, expr);
    s
}

fn write_expr_sig(s: &mut String, expr: &Expression) {
    match expr {
        Expression::Term(factor) => write_factor_sig(s, factor),
        Expression::Unary(op, x, _) => {
            let _ = write!(s, "U{op:?}(");
            write_expr_sig(s, x);
            s.push(')');
        }
        Expression::Binary(x, op, y, _) => {
            let _ = write!(s, "B{op:?}(");
            write_expr_sig(s, x);
            s.push(',');
            write_expr_sig(s, y);
            s.push(')');
        }
        Expression::Ternary(x, y, z, _) => {
            s.push_str("T(");
            write_expr_sig(s, x);
            s.push(',');
            write_expr_sig(s, y);
            s.push(',');
            write_expr_sig(s, z);
            s.push(')');
        }
        Expression::Concatenation(items, _) => {
            s.push_str("Cat(");
            for (e, rep) in items {
                write_expr_sig(s, e);
                if let Some(r) = rep {
                    s.push('*');
                    write_expr_sig(s, r);
                }
                s.push(',');
            }
            s.push(')');
        }
        Expression::StructConstructor(name, fields, _) => {
            let _ = write!(s, "Struct{name:?}(");
            for (fname, e) in fields {
                let _ = write!(s, "{fname:?}=");
                write_expr_sig(s, e);
                s.push(',');
            }
            s.push(')');
        }
        Expression::ArrayLiteral(items, _) => {
            s.push_str("Arr(");
            for item in items {
                match item {
                    air::ArrayLiteralItem::Value(e, rep) => {
                        write_expr_sig(s, e);
                        if let Some(r) = rep {
                            s.push('*');
                            write_expr_sig(s, r);
                        }
                    }
                    air::ArrayLiteralItem::Defaul(e) => {
                        s.push_str("def:");
                        write_expr_sig(s, e);
                    }
                }
                s.push(',');
            }
            s.push(')');
        }
    }
}

fn write_factor_sig(s: &mut String, factor: &Factor) {
    match factor {
        Factor::Variable(id, index, select, _) => {
            let _ = write!(s, "V{id:?}");
            for e in &index.0 {
                s.push('[');
                write_expr_sig(s, e);
                s.push(']');
            }
            for e in &select.0 {
                s.push('{');
                write_expr_sig(s, e);
                s.push('}');
            }
            if let Some((op, e)) = &select.1 {
                let _ = write!(s, "{op:?}");
                write_expr_sig(s, e);
            }
        }
        Factor::Value(ct) => {
            // The evaluated value, not its source token.
            let _ = write!(s, "C{:?}", ct.value);
        }
        // Function calls as an address are rare; fall back to the full Debug
        // (token-sensitive, so it may over-count, but never collides two
        // different addresses into one port — which would be unsafe).
        other => {
            let _ = write!(s, "{other:?}");
        }
    }
}

/// If `expr` is a masked read-modify-write of RAM `vid` at `wr_index` —
/// `(vid[wr_index] & ~m) | (d & m)`, each `&`/`|` commuting — return `(d, m)`.
/// The retention read `vid[wr_index]` folds into the mask, so it costs no read
/// port and a lookup-plus-RMW array stays 1R1W. Port counting (`read_pattern_ok`)
/// and building (`conv::statement`) both call this, so they agree on which reads
/// are retention reads.
pub(crate) fn match_masked_write<'a>(
    vid: air::VarId,
    wr_index: &air::VarIndex,
    expr: &'a Expression,
) -> Option<(&'a Expression, &'a Expression)> {
    let Expression::Binary(a, air::Op::BitOr, b, _) = expr else {
        return None;
    };
    match_masked_arms(vid, wr_index, a, b).or_else(|| match_masked_arms(vid, wr_index, b, a))
}

/// `retain` = `vid[wr_index] & ~m`, `write` = `d & m`. The two masks must be
/// structurally identical, else it isn't a clean masked write — some bits would
/// be both kept and written, or neither.
fn match_masked_arms<'a>(
    vid: air::VarId,
    wr_index: &air::VarIndex,
    retain: &'a Expression,
    write: &'a Expression,
) -> Option<(&'a Expression, &'a Expression)> {
    let Expression::Binary(ra, air::Op::BitAnd, rb, _) = retain else {
        return None;
    };
    let notm: &Expression = if is_self_read(vid, wr_index, ra) {
        rb
    } else if is_self_read(vid, wr_index, rb) {
        ra
    } else {
        return None;
    };
    let Expression::Unary(air::Op::BitNot, m_retain, _) = notm else {
        return None;
    };
    let Expression::Binary(wa, air::Op::BitAnd, wb, _) = write else {
        return None;
    };
    let m_sig = addr_signature(m_retain);
    if addr_signature(wb) == m_sig {
        Some((wa, wb))
    } else if addr_signature(wa) == m_sig {
        Some((wb, wa))
    } else {
        None
    }
}

/// `expr` is exactly `vid[wr_index]`: a whole-word self-read at the write's own
/// index, no bit/part select.
fn is_self_read(vid: air::VarId, wr_index: &air::VarIndex, expr: &Expression) -> bool {
    let Expression::Term(factor) = expr else {
        return false;
    };
    let Factor::Variable(id, index, select, _) = &**factor else {
        return false;
    };
    *id == vid
        && select.is_empty()
        && index.0.len() == 1
        && wr_index.0.len() == 1
        && addr_signature(&index.0[0]) == addr_signature(&wr_index.0[0])
}

/// `mem[addr]` with a single dynamic index dimension and no bit/part select.
fn is_dynamic_whole_word(dst: &AssignDestination) -> bool {
    dst.index.0.len() == 1
        && !dst.index.is_const()
        && dst.select.is_empty()
        && dst.comptime.part_select.is_none()
}

/// Visit every destination targeting `vid`. `f(dst, in_reset, dst_count)` where
/// `dst_count` is the destination count of the enclosing assignment (a
/// single-target write has 1; a concat-LHS more).
fn walk_writes(
    stmt: &Statement,
    vid: air::VarId,
    in_reset: bool,
    f: &mut impl FnMut(&AssignDestination, bool, usize),
) {
    match stmt {
        Statement::Assign(a) => {
            for d in &a.dst {
                if d.id == vid {
                    f(d, in_reset, a.dst.len());
                }
            }
        }
        Statement::If(i) => {
            for s in &i.true_side {
                walk_writes(s, vid, in_reset, f);
            }
            for s in &i.false_side {
                walk_writes(s, vid, in_reset, f);
            }
        }
        Statement::IfReset(i) => {
            for s in &i.true_side {
                walk_writes(s, vid, true, f);
            }
            for s in &i.false_side {
                walk_writes(s, vid, in_reset, f);
            }
        }
        Statement::Case(c) => {
            for arm in &c.arms {
                for s in &arm.body {
                    walk_writes(s, vid, in_reset, f);
                }
            }
            for s in &c.default {
                walk_writes(s, vid, in_reset, f);
            }
        }
        Statement::For(fs) => {
            for s in &fs.body {
                walk_writes(s, vid, in_reset, f);
            }
        }
        Statement::FunctionCall(call) => {
            for dsts in call.outputs.values() {
                for d in dsts {
                    if d.id == vid {
                        f(d, in_reset, 1);
                    }
                }
            }
        }
        _ => {}
    }
}

type ReadVisitor<'a> = dyn FnMut(&air::VarIndex, &air::VarSelect) + 'a;

/// Visit `(index, select)` of every `Factor::Variable(vid, …)` read in `decl`.
fn for_each_read_in_decl(decl: &Declaration, vid: air::VarId, f: &mut ReadVisitor) {
    match decl {
        Declaration::Comb(c) => {
            for s in &c.statements {
                for_each_read_in_stmt(s, vid, f);
            }
        }
        Declaration::Ff(ff) => {
            for s in &ff.statements {
                for_each_read_in_stmt(s, vid, f);
            }
        }
        Declaration::Inst(inst) => {
            for input in &inst.inputs {
                for_each_read_in_expr(&input.expr, vid, f);
            }
        }
        _ => {}
    }
}

fn for_each_read_in_dsts(dsts: &[AssignDestination], vid: air::VarId, f: &mut ReadVisitor) {
    // Destination address/select expressions are themselves read expressions.
    for d in dsts {
        for e in &d.index.0 {
            for_each_read_in_expr(e, vid, f);
        }
        for e in &d.select.0 {
            for_each_read_in_expr(e, vid, f);
        }
        if let Some((_, e)) = &d.select.1 {
            for_each_read_in_expr(e, vid, f);
        }
    }
}

fn for_each_read_in_stmt(stmt: &Statement, vid: air::VarId, f: &mut ReadVisitor) {
    match stmt {
        Statement::Assign(a) => {
            // A masked write folds its retention read into the mask (see
            // `match_masked_write`), so count only `d`/`m`/dst, not that read. A
            // genuine read at the same index elsewhere is still a distinct
            // factor and counted.
            if a.dst.len() == 1
                && a.dst[0].id == vid
                && let Some((d, m)) = match_masked_write(vid, &a.dst[0].index, &a.expr)
            {
                for_each_read_in_expr(d, vid, f);
                for_each_read_in_expr(m, vid, f);
                for_each_read_in_dsts(&a.dst, vid, f);
            } else {
                for_each_read_in_expr(&a.expr, vid, f);
                for_each_read_in_dsts(&a.dst, vid, f);
            }
        }
        Statement::If(i) => {
            for_each_read_in_expr(&i.cond, vid, f);
            for s in &i.true_side {
                for_each_read_in_stmt(s, vid, f);
            }
            for s in &i.false_side {
                for_each_read_in_stmt(s, vid, f);
            }
        }
        Statement::IfReset(i) => {
            for s in &i.true_side {
                for_each_read_in_stmt(s, vid, f);
            }
            for s in &i.false_side {
                for_each_read_in_stmt(s, vid, f);
            }
        }
        Statement::Case(c) => {
            for_each_read_in_expr(&c.case_target, vid, f);
            for arm in &c.arms {
                for p in &arm.patterns {
                    match p {
                        air::CasePattern::Eq(e) => for_each_read_in_expr(e, vid, f),
                        air::CasePattern::Range { lo, hi, .. } => {
                            for_each_read_in_expr(lo, vid, f);
                            for_each_read_in_expr(hi, vid, f);
                        }
                    }
                }
                for s in &arm.body {
                    for_each_read_in_stmt(s, vid, f);
                }
            }
            for s in &c.default {
                for_each_read_in_stmt(s, vid, f);
            }
        }
        Statement::For(fs) => {
            for s in &fs.body {
                for_each_read_in_stmt(s, vid, f);
            }
        }
        Statement::FunctionCall(call) => {
            // conv synthesizes a statement-form call's args and output-dst index
            // expressions, so a read of `vid` here is a real read port. (conv
            // drops SystemFunctionCall, so that variant is left untraversed.)
            for arg in call.inputs.values() {
                for_each_read_in_expr(arg, vid, f);
            }
            for dsts in call.outputs.values() {
                for_each_read_in_dsts(dsts, vid, f);
            }
        }
        _ => {}
    }
}

fn for_each_read_in_expr(expr: &Expression, vid: air::VarId, f: &mut ReadVisitor) {
    match expr {
        Expression::Term(factor) => for_each_read_in_factor(factor, vid, f),
        Expression::Unary(_, x, _) => for_each_read_in_expr(x, vid, f),
        Expression::Binary(x, _, y, _) => {
            for_each_read_in_expr(x, vid, f);
            for_each_read_in_expr(y, vid, f);
        }
        Expression::Ternary(x, y, z, _) => {
            for_each_read_in_expr(x, vid, f);
            for_each_read_in_expr(y, vid, f);
            for_each_read_in_expr(z, vid, f);
        }
        Expression::Concatenation(items, _) => {
            for (e, rep) in items {
                for_each_read_in_expr(e, vid, f);
                if let Some(r) = rep {
                    for_each_read_in_expr(r, vid, f);
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            for (_, e) in fields {
                for_each_read_in_expr(e, vid, f);
            }
        }
        Expression::ArrayLiteral(items, _) => {
            for item in items {
                match item {
                    air::ArrayLiteralItem::Value(e, rep) => {
                        for_each_read_in_expr(e, vid, f);
                        if let Some(r) = rep {
                            for_each_read_in_expr(r, vid, f);
                        }
                    }
                    air::ArrayLiteralItem::Defaul(e) => for_each_read_in_expr(e, vid, f),
                }
            }
        }
    }
}

fn for_each_read_in_factor(factor: &Factor, vid: air::VarId, f: &mut ReadVisitor) {
    match factor {
        Factor::Variable(id, index, select, _) => {
            if *id == vid {
                f(index, select);
            }
            // Index/select sub-expressions can read `vid` too (e.g. mem[mem[k]]).
            for e in &index.0 {
                for_each_read_in_expr(e, vid, f);
            }
            for e in &select.0 {
                for_each_read_in_expr(e, vid, f);
            }
            if let Some((_, e)) = &select.1 {
                for_each_read_in_expr(e, vid, f);
            }
        }
        Factor::FunctionCall(call) => {
            for arg in call.inputs.values() {
                for_each_read_in_expr(arg, vid, f);
            }
        }
        _ => {}
    }
}
