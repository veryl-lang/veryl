//! Version-split (select fusion) pass for multi-write comb variables.
//!
//! Rewrites the dominant shape — an unconditional full-width base write
//! followed by guarded full/partial overrides inside one `always_comb`
//! (a priority chain, e.g. an unrolled `for` + `if` scan) — into a single
//! unconditional write whose RHS is a ternary select chain.  Bit-disjoint
//! writers (per-field assigns) fold the same way, into a concatenation.
//! One writer per variable lets the whole-comb backends localize and fuse
//! storage they otherwise must keep materialized.
//!
//! Runs on the merged comb list before dependency analysis, while
//! `SequentialBlock`s (one per multi-statement `always_comb`) are intact,
//! so program order inside a block is the sequential-semantics order.
//!
//! Turning a guarded write into an always-evaluated select operand is
//! sound here because every partial expression form is total in this IR:
//! division guards y==0, dynamic indices clamp to the element range, and
//! shift counts clamp to the width.
//!
//! Validation caveat: this pass runs before compile, so a dual-run checker
//! compares the *already-fused* statements — a semantic error introduced
//! here is applied to both sides and is therefore invisible to it.  Changes
//! to this pass must be gated on golden SystemVerilog co-simulation.

use crate::ir::big_array::BigArrayFold;
use crate::ir::expression::{ExpressionContext, ProtoExpression};
use crate::ir::statement::{ProtoAssignStatement, ProtoIfStatement, ProtoStatement};
use crate::ir::variable::VarOffset;
use crate::ir::{Op, Value};
use crate::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use veryl_parser::token_range::TokenRange;

// ---------------------------------------------------------------------------
// Transformation pass
// ---------------------------------------------------------------------------

/// The single-writer form widens what the whole-comb backends can localize
/// and fuse, so it pays on the full sweep.
pub fn pass_enabled(use_4state: bool) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !use_4state && *ON.get_or_init(|| std::env::var("VERYL_VSPLIT").as_deref() != Ok("0"))
}

#[derive(Default, Debug)]
pub struct RunStats {
    pub blocks: usize,
    pub fused_vars: usize,
    pub fused_writes: usize,
    pub removed_stmts: usize,
    pub skip_opaque: usize,
    pub skip_read: usize,
    pub skip_unstable: usize,
    pub skip_width: usize,
    pub skip_fold: usize,
    /// Chain skipped: the folded expression exceeds [`MAX_FUSED_NODES`].
    pub skip_budget: usize,
    /// Whole block skipped: it contains a block-scoped `Break` (from an
    /// unrolled static for-loop), whose abort-the-rest control flow the
    /// write-position fold cannot model.
    pub skip_break: usize,
    /// Chains replaced by a table lookup.
    pub lut_vars: usize,
}

type Counter = (&'static str, fn(&RunStats) -> usize);

/// Report order.  Pairing each label with its accessor here is what keeps a
/// counter from being reported under another one's name.
const COUNTERS: [Counter; 12] = [
    ("blocks", |s| s.blocks),
    ("fused_vars", |s| s.fused_vars),
    ("fused_writes", |s| s.fused_writes),
    ("removed_stmts", |s| s.removed_stmts),
    ("skip.opaque", |s| s.skip_opaque),
    ("skip.read", |s| s.skip_read),
    ("skip.unstable", |s| s.skip_unstable),
    ("skip.width", |s| s.skip_width),
    ("skip.fold", |s| s.skip_fold),
    ("skip.budget", |s| s.skip_budget),
    ("skip.break", |s| s.skip_break),
    ("lut_vars", |s| s.lut_vars),
];

/// Process-wide totals: the pass runs per always_comb during conv (including
/// inside cross-test cached subtrees), so per-call logging would be noise.
static TOTALS: [AtomicUsize; COUNTERS.len()] = [const { AtomicUsize::new(0) }; COUNTERS.len()];

pub fn accumulate(s: &RunStats) {
    for (t, (_, get)) in TOTALS.iter().zip(COUNTERS) {
        let v = get(s);
        if v > 0 {
            t.fetch_add(v, Relaxed);
        }
    }
}

/// Formatted process-wide totals for diagnostics.
pub fn totals_line() -> String {
    COUNTERS
        .iter()
        .zip(TOTALS.iter())
        .map(|((label, _), t)| format!("{label}={}", t.load(Relaxed)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rewrite multi-write comb variables inside each `SequentialBlock` into
/// single-writer select chains.  Returns per-shape statistics.
pub fn run(stmts: &mut [ProtoStatement], alloc: &mut dyn FnMut(usize) -> isize) -> RunStats {
    let mut stats = RunStats::default();
    for stmt in stmts.iter_mut() {
        if let ProtoStatement::SequentialBlock(body) = stmt {
            stats.blocks += 1;
            split_block(body, &mut stats, alloc);
        }
    }
    stats
}

/// Maximum If nesting depth folded recursively (else-if chains from `case`
/// lowering can be hundreds deep; the recursion is one frame per level).
const MAX_TREE_DEPTH: usize = 512;

/// Ceiling on one folded RHS's emit-weighted size (`VERYL_VSPLIT_MAX_NODES`
/// overrides; 0 disables).  The snapshot above keeps a fold proportional to
/// its source, so what is left here is a size/work trade, and both directions
/// cost: folding removes work but grows the emitted code, and on a design
/// whose settle streams through more code than the instruction cache holds
/// the code costs more than the work saved, while folding too little leaves
/// the multi-writer form the whole-comb passes cannot localize or fuse.
/// Measured across the suite the response is a shallow bowl — this value wins
/// several percent on the widest designs and is neutral on the rest, a
/// quarter of it loses on both counts — and it doubles as a backstop for any
/// growth the snapshot does not cover.
const MAX_FUSED_NODES: usize = 2_048;

#[cfg(test)]
thread_local! {
    /// Per-test cap override — thread-local so parallel tests cannot race.
    static TEST_MAX_NODES: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn max_fused_nodes() -> usize {
    #[cfg(test)]
    if let Some(n) = TEST_MAX_NODES.with(|c| c.get()) {
        return n;
    }
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("VERYL_VSPLIT_MAX_NODES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(MAX_FUSED_NODES)
    })
}

/// Emit-weighted node count with an early exit above `cap`.  A wide
/// constant emits one literal per 64-bit word, so `Value` nodes charge
/// their word count — the budget then tracks emitted bytes, not tree
/// shape.  Iterative: the trees this exists to reject are deep enough to
/// overflow a recursive walk.
fn expr_nodes_capped(root: &ProtoExpression, cap: usize) -> usize {
    let mut n = 0usize;
    let mut stack: Vec<&ProtoExpression> = vec![root];
    while let Some(e) = stack.pop() {
        n = n.saturating_add(match e {
            ProtoExpression::Value { width, .. } => width.div_ceil(64).max(1),
            _ => 1,
        });
        if n > cap {
            return n;
        }
        match e {
            ProtoExpression::Value { .. }
            | ProtoExpression::Variable {
                dynamic_select: None,
                ..
            }
            | ProtoExpression::HierVariable(_) => {}
            ProtoExpression::Variable {
                dynamic_select: Some(d),
                ..
            } => stack.push(&d.index_expr),
            ProtoExpression::Unary { x, .. } => stack.push(x),
            ProtoExpression::Binary { x, y, .. } => {
                stack.push(x);
                stack.push(y);
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                stack.push(cond);
                stack.push(true_expr);
                stack.push(false_expr);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (e, _, _) in elements {
                    stack.push(e);
                }
            }
            ProtoExpression::DynamicVariable {
                index_expr,
                dynamic_select,
                ..
            } => {
                stack.push(index_expr);
                if let Some(d) = dynamic_select {
                    stack.push(&d.index_expr);
                }
            }
        }
    }
    n
}

/// One captured write site of an eventable statement tree (metadata only;
/// the expressions are consumed later by the tree fold).
struct Ev {
    dst: i64,
    full_width: usize,
    /// Highest written bit (inclusive); `dst_width - 1` when no select.
    hi: u32,
    stmt_idx: usize,
    /// Statement-order position within the block (Assign granularity).
    pos: u32,
    token: TokenRange,
}

#[derive(Default)]
struct BlockCol {
    events: Vec<Ev>,
    /// dst -> comb offsets read by exprs/conds of the trees writing dst.
    ev_reads: HashMap<i64, Vec<i64>>,
    /// Comb offset -> statement-order positions where it is read (exprs,
    /// conds, opaque statements).  Only reads strictly between a variable's
    /// first and last write disqualify fusion: earlier/later readers see the
    /// previous-pass/final value both before and after the rewrite.
    read_pos: HashMap<i64, Vec<u32>>,
    /// Comb offsets written by non-eventable statements.
    opaque_writes: HashSet<i64>,
    /// Comb offset -> last write position in the block (events + opaque).
    max_write_pos: HashMap<i64, u32>,
    pos: u32,
}

fn gather_comb_reads(fold: &BigArrayFold, expr: &ProtoExpression, out: &mut Vec<i64>) {
    let mut ins = Vec::new();
    expr.gather_variable_offsets_expanded(fold, &mut ins);
    for off in ins {
        if let VarOffset::Comb(o) = off {
            out.push(o as i64);
        }
    }
}

/// True when the statement tree consists solely of capturable comb Assigns
/// under (possibly nested) If statements.
fn is_eventable_tree(fold: &BigArrayFold, stmt: &ProtoStatement, depth: usize) -> bool {
    match stmt {
        ProtoStatement::Assign(x) => {
            matches!(x.dst, VarOffset::Comb(_))
                && x.dynamic_select.is_none()
                && x.rhs_select.is_none()
                // `read_pos` / `max_write_pos` are array-wide for a folded
                // element, too coarse to place this write precisely.
                && !fold.covers(x.dst)
        }
        ProtoStatement::If(x) => {
            depth < MAX_TREE_DEPTH
                && x.cond.is_some()
                && x.true_side
                    .iter()
                    .all(|s| is_eventable_tree(fold, s, depth + 1))
                && x.false_side
                    .iter()
                    .all(|s| is_eventable_tree(fold, s, depth + 1))
        }
        _ => false,
    }
}

/// Record write events and read positions of one eventable tree.
/// `tree_reads` accumulates every comb offset read anywhere in the tree
/// (conds + exprs); the caller attributes it to all dsts written by the tree.
fn record_tree(
    fold: &BigArrayFold,
    stmt: &ProtoStatement,
    stmt_idx: usize,
    col: &mut BlockCol,
    tree_reads: &mut Vec<i64>,
    tree_dsts: &mut HashSet<i64>,
) {
    match stmt {
        ProtoStatement::Assign(x) => {
            let VarOffset::Comb(o) = x.dst else {
                unreachable!("checked by is_eventable_tree")
            };
            let dst = o as i64;
            col.pos += 1;
            let pos = col.pos;
            let mut reads = Vec::new();
            gather_comb_reads(fold, &x.expr, &mut reads);
            // Reads of the destination itself (RMW self-reads) are the
            // fold's own business — it substitutes the accumulated value —
            // so they count neither as intermediate readers nor as
            // stability inputs.
            reads.retain(|&r| r != dst);
            for &r in &reads {
                col.read_pos.entry(r).or_default().push(pos);
            }
            tree_reads.extend(reads);
            tree_dsts.insert(dst);
            let hi = match x.select {
                Some((a, b)) => a.max(b) as u32,
                None => x.dst_width.saturating_sub(1) as u32,
            };
            col.events.push(Ev {
                dst,
                full_width: x.dst_width,
                hi,
                stmt_idx,
                pos,
                token: x.token,
            });
            let e = col.max_write_pos.entry(dst).or_insert(0);
            *e = (*e).max(pos);
        }
        ProtoStatement::If(x) => {
            let cond = x.cond.as_ref().unwrap();
            let mut cond_reads = Vec::new();
            gather_comb_reads(fold, cond, &mut cond_reads);
            // Position the cond read at the next statement position (the
            // guard is evaluated together with its first guarded write).
            let cpos = col.pos + 1;
            for &r in &cond_reads {
                col.read_pos.entry(r).or_default().push(cpos);
            }
            tree_reads.extend(cond_reads);
            for s in x.true_side.iter().chain(&x.false_side) {
                record_tree(fold, s, stmt_idx, col, tree_reads, tree_dsts);
            }
        }
        _ => unreachable!("checked by is_eventable_tree"),
    }
}

/// Record a non-eventable statement's comb reads and writes.
fn record_opaque(stmt: &ProtoStatement, col: &mut BlockCol) {
    let mut deps = crate::ir::deps::StmtDeps::default();
    crate::ir::deps::collect_stmt_deps(stmt, &mut deps);
    col.pos += 1;
    let pos = col.pos;
    for d in &deps.ins {
        if let VarOffset::Comb(o) = d.off {
            col.read_pos.entry(o as i64).or_default().push(pos);
        }
    }
    for d in &deps.outs {
        if let VarOffset::Comb(o) = d.off {
            col.opaque_writes.insert(o as i64);
            let e = col.max_write_pos.entry(o as i64).or_insert(0);
            *e = (*e).max(pos);
        }
    }
}

/// A `Break` scoped to THIS block (i.e. not inside a nested `For`, whose
/// breaks terminate only that loop): it aborts every remaining statement of
/// the block, a control-flow effect the write-position fold cannot model.
/// Static for-loops are unrolled before this pass, leaving their breaks
/// bare / under `If`s in the block body.
fn contains_block_break(stmt: &ProtoStatement) -> bool {
    match stmt {
        ProtoStatement::Break => true,
        ProtoStatement::If(x) => x
            .true_side
            .iter()
            .chain(&x.false_side)
            .any(contains_block_break),
        ProtoStatement::SequentialBlock(body) => body.iter().any(contains_block_break),
        _ => false,
    }
}

fn split_block(
    body: &mut Vec<ProtoStatement>,
    stats: &mut RunStats,
    alloc: &mut dyn FnMut(usize) -> isize,
) {
    // A block-scoped `Break` aborts the remaining statements when it fires;
    // fusing would move earlier write sites to the last-write position,
    // executing them (or skipping them) on the wrong side of the break.
    // Rare (unrolled for-loops with break) — skip the whole block.
    if body.iter().any(contains_block_break) {
        stats.skip_break += 1;
        return;
    }
    // Pass 1: classify statements and capture write events.
    let fold = BigArrayFold::from_statements(body.iter());
    let mut col = BlockCol::default();
    let mut eventable: Vec<bool> = Vec::with_capacity(body.len());
    for (idx, stmt) in body.iter().enumerate() {
        let ok = is_eventable_tree(&fold, stmt, 0);
        eventable.push(ok);
        if ok {
            let mut tree_reads = Vec::new();
            let mut tree_dsts = HashSet::default();
            record_tree(&fold, stmt, idx, &mut col, &mut tree_reads, &mut tree_dsts);
            // Attribute the tree's reads (conds included) to every dst it
            // writes, for the input-stability check.
            for &dst in &tree_dsts {
                col.ev_reads
                    .entry(dst)
                    .or_default()
                    .extend(tree_reads.iter().copied());
            }
        } else {
            record_opaque(stmt, &mut col);
        }
    }

    // Pass 2: pick fusable variables.
    let mut by_dst: HashMap<i64, Vec<usize>> = HashMap::default();
    for (i, ev) in col.events.iter().enumerate() {
        by_dst.entry(ev.dst).or_default().push(i);
    }

    struct Fused {
        last_stmt_idx: usize,
        temps: Vec<ProtoAssignStatement>,
        assign: ProtoAssignStatement,
        writes: usize,
    }
    let mut fused: Vec<Fused> = Vec::new();
    let mut fused_dsts: HashSet<i64> = HashSet::default();

    for (&dst, idxs) in &by_dst {
        if idxs.len() < 2 {
            continue;
        }
        if col.opaque_writes.contains(&dst) {
            stats.skip_opaque += 1;
            continue;
        }
        let evs: Vec<&Ev> = idxs.iter().map(|&i| &col.events[i]).collect();
        // Intermediate readers (strictly between first and last write) see a
        // partial version; fusing to the last-write position would change
        // what they observe.  Earlier readers keep seeing the previous-pass
        // value and later readers the final value, both unchanged.
        let first_pos = evs[0].pos;
        let last_pos = evs[evs.len() - 1].pos;
        let has_mid_reader = col
            .read_pos
            .get(&dst)
            .is_some_and(|ps| ps.iter().any(|&p| p > first_pos && p < last_pos));
        if has_mid_reader {
            stats.skip_read += 1;
            continue;
        }
        // Input stability: everything the chain reads must have its final
        // in-block value before this chain's first write, otherwise moving
        // an early version's expression to the fused write position could
        // observe a different value.  Inputs written only earlier are fine
        // (their own fused writes stay at their last-write position, which
        // precedes this chain).  This also rejects self-RMW chains (dst
        // reading itself, since dst's own writes extend past first_pos).
        let stable = col.ev_reads.get(&dst).is_none_or(|reads| {
            reads
                .iter()
                .all(|r| col.max_write_pos.get(r).is_none_or(|&p| p < first_pos))
        });
        if !stable {
            stats.skip_unstable += 1;
            continue;
        }
        let width = evs[0].full_width;
        if evs
            .iter()
            .any(|e| e.full_width != width || e.hi as usize >= width)
        {
            stats.skip_width += 1;
            continue;
        }
        // Fold the writer statements (program order, deduped) into one RHS.
        let mut writer_stmts: Vec<usize> = evs.iter().map(|e| e.stmt_idx).collect();
        writer_stmts.dedup();
        let folded = fold_var(body, &writer_stmts, dst, width, alloc, evs[0].token);
        match folded {
            Some((temps, expr)) => {
                // The fold's rename temps stay: a surviving leaf expression
                // may read a snapshot; unreferenced ones are dead stores.
                let (temps, expr) = if lut_enabled() {
                    match try_lut_compress(&expr, width, alloc, evs[0].token)
                        .filter(|_| lut_budget_ok())
                    {
                        Some((lut_temps, lut_expr)) => {
                            stats.lut_vars += 1;
                            lut_diag(&format!("compressed dst={dst:#x} width={width}"));
                            let mut t = temps;
                            t.extend(lut_temps);
                            (t, lut_expr)
                        }
                        None => (temps, expr),
                    }
                } else {
                    (temps, expr)
                };
                let cap = max_fused_nodes();
                if cap != 0 {
                    let mut nodes = expr_nodes_capped(&expr, cap);
                    for t in &temps {
                        if nodes > cap {
                            break;
                        }
                        nodes = nodes.saturating_add(expr_nodes_capped(&t.expr, cap));
                    }
                    if nodes > cap {
                        stats.skip_budget += 1;
                        continue;
                    }
                }
                let last_stmt_idx = *writer_stmts.last().unwrap();
                fused.push(Fused {
                    last_stmt_idx,
                    temps,
                    assign: ProtoAssignStatement {
                        dst: VarOffset::Comb(dst as isize),
                        dst_width: width,
                        select: None,
                        dynamic_select: None,
                        rhs_select: None,
                        expr,
                        dst_ff_current_offset: 0,
                        token: evs[0].token,
                    },
                    writes: evs.len(),
                });
                fused_dsts.insert(dst);
            }
            None => {
                stats.skip_fold += 1;
            }
        }
    }

    if fused.is_empty() {
        return;
    }

    // Pass 3: rebuild the block — drop consumed writes, insert fused assigns
    // at each variable's last-write position.
    let mut insert_at: HashMap<usize, Vec<ProtoAssignStatement>> = HashMap::default();
    for f in &fused {
        stats.fused_vars += 1;
        stats.fused_writes += f.writes;
    }
    // Deterministic order for same-position inserts; each variable's rename
    // temps go right before its fused write (program order within the fold).
    let mut fused_sorted = fused;
    fused_sorted.sort_by_key(|f| f.assign.dst.raw());
    for f in fused_sorted {
        let slot = insert_at.entry(f.last_stmt_idx).or_default();
        slot.extend(f.temps);
        slot.push(f.assign);
    }

    let old = std::mem::take(body);
    let before = old.len();
    let mut inserted = 0usize;
    for (idx, stmt) in old.into_iter().enumerate() {
        if let Some(s) = prune_stmt(stmt, &fused_dsts) {
            body.push(s);
        }
        if let Some(assigns) = insert_at.remove(&idx) {
            for a in assigns {
                inserted += 1;
                body.push(ProtoStatement::Assign(a));
            }
        }
    }
    stats.removed_stmts += (before + inserted).saturating_sub(body.len());
}

/// Remove consumed Assigns; drop Ifs emptied by the removal.
fn prune_stmt(stmt: ProtoStatement, dsts: &HashSet<i64>) -> Option<ProtoStatement> {
    match stmt {
        ProtoStatement::Assign(x) => {
            if let VarOffset::Comb(o) = x.dst
                && dsts.contains(&(o as i64))
            {
                None
            } else {
                Some(ProtoStatement::Assign(x))
            }
        }
        ProtoStatement::If(mut x) => {
            x.true_side = x
                .true_side
                .into_iter()
                .filter_map(|s| prune_stmt(s, dsts))
                .collect();
            x.false_side = x
                .false_side
                .into_iter()
                .filter_map(|s| prune_stmt(s, dsts))
                .collect();
            if x.true_side.is_empty() && x.false_side.is_empty() {
                None
            } else {
                Some(ProtoStatement::If(x))
            }
        }
        other => Some(other),
    }
}

// ---------------------------------------------------------------------------
// Tree fold: interval map + ternary merges
// ---------------------------------------------------------------------------

/// A fold operand.  `Val` and `Var` can be split at any bit boundary
/// (constant select / narrower variable read); `Op` is opaque.
///
/// The interval map starts as a `Var` read of the destination itself, so an
/// interval no write covers — an unfired guard, or a bit no field assign
/// touches — materializes as a self-read.  The settle evaluates it against
/// the previous pass's value, the same fixpoint the original statements
/// converge to.
#[derive(Clone)]
enum Fe {
    Val(Value),
    /// Read of `var_offset`'s bits [hi:lo] (variable-local bit coordinates).
    Var {
        var_offset: VarOffset,
        var_full_width: usize,
        hi: u32,
        lo: u32,
    },
    Op(ProtoExpression),
}

impl Fe {
    fn from_expr(expr: ProtoExpression) -> Fe {
        match expr {
            ProtoExpression::Value { value, .. } => Fe::Val(value),
            ProtoExpression::Variable {
                var_offset,
                select,
                dynamic_select: None,
                width,
                var_full_width,
                ..
            } => {
                let (hi, lo) = match select {
                    Some((a, b)) => (a.max(b) as u32, a.min(b) as u32),
                    None => (width.saturating_sub(1) as u32, 0),
                };
                Fe::Var {
                    var_offset,
                    var_full_width,
                    hi,
                    lo,
                }
            }
            other => Fe::Op(other),
        }
    }

    /// Extract local bits [hi:lo] (relative to this operand's LSB).
    fn split(&self, hi: u32, lo: u32) -> Option<Fe> {
        match self {
            Fe::Val(v) => Some(Fe::Val(v.select(hi as usize, lo as usize))),
            Fe::Var {
                var_offset,
                var_full_width,
                lo: base_lo,
                ..
            } => Some(Fe::Var {
                var_offset: *var_offset,
                var_full_width: *var_full_width,
                hi: base_lo + hi,
                lo: base_lo + lo,
            }),
            Fe::Op(_) => None,
        }
    }

    /// True when this operand is a bare read of `dst` — the form an interval
    /// no write covers carries.  A composed value that reads `dst` inside a
    /// merge is not one: only an unfired guard puts it there, and a variable
    /// a guard may leave unwritten depends on its own previous value however
    /// the fold spells it.
    fn is_self_read(&self, dst: i64) -> bool {
        matches!(self, Fe::Var { var_offset: VarOffset::Comb(o), .. } if *o as i64 == dst)
    }

    fn to_expr(&self, width: usize) -> Option<ProtoExpression> {
        let ctx = ExpressionContext {
            width,
            signed: false,
        };
        match self {
            Fe::Val(v) => {
                let mut v = v.clone();
                if v.width() != width {
                    v.trunc(width);
                }
                Some(ProtoExpression::Value {
                    value: v,
                    width,
                    expr_context: ctx,
                })
            }
            Fe::Var {
                var_offset,
                var_full_width,
                hi,
                lo,
            } => {
                let full = *var_full_width == width && *lo == 0;
                Some(ProtoExpression::Variable {
                    var_offset: *var_offset,
                    select: if full {
                        None
                    } else {
                        Some((*hi as usize, *lo as usize))
                    },
                    dynamic_select: None,
                    width,
                    var_full_width: *var_full_width,
                    expr_context: ctx,
                })
            }
            Fe::Op(e) => Some(e.clone()),
        }
    }
}

/// One disjoint bit range of the accumulated value.  `ver` tracks write
/// generations: an interval whose `ver` matches the pre-If snapshot was not
/// touched by that branch.
#[derive(Clone)]
struct Interval {
    hi: u32,
    lo: u32,
    fe: Fe,
    ver: u32,
}

#[derive(Clone)]
struct IMap {
    /// LSB-first, contiguous, covering [0, width).
    iv: Vec<Interval>,
    next_ver: u32,
    /// The variable being folded; the only offset the map may self-read.
    dst: i64,
}

impl IMap {
    fn new(width: usize, dst: i64) -> IMap {
        IMap {
            dst,
            iv: vec![Interval {
                hi: width.saturating_sub(1) as u32,
                lo: 0,
                fe: Fe::Var {
                    var_offset: VarOffset::Comb(dst as isize),
                    var_full_width: width,
                    hi: width.saturating_sub(1) as u32,
                    lo: 0,
                },
                ver: 0,
            }],
            next_ver: 1,
        }
    }

    /// Snapshot the accumulated value into a fresh comb temp so RMW
    /// self-reads can slice it cheaply.  No-op when the map is already one
    /// full-width variable read (the initial self-read state or a previous
    /// snapshot).  The temp assign lands just before the fused write.
    ///
    /// Self-read intervals stay put and the temp holds zero over their bits.
    /// The fused write is `dst`'s only writer, so a self-read inside the temp
    /// becomes a read that write answers — turning a self-loop into a
    /// two-statement cycle the comb scheduler rejects.
    fn snapshot_to_temp(
        &mut self,
        width: usize,
        temps: &mut Vec<ProtoAssignStatement>,
        alloc: &mut dyn FnMut(usize) -> isize,
        token: TokenRange,
    ) -> Option<()> {
        if self.iv.len() == 1
            && let Fe::Var {
                hi,
                lo,
                var_full_width,
                ..
            } = &self.iv[0].fe
            && *lo == 0
            && (*hi as usize) == width - 1
            && *var_full_width == width
        {
            return Some(());
        }
        let keep: Vec<bool> = self
            .iv
            .iter()
            .map(|iv| iv.fe.is_self_read(self.dst))
            .collect();
        if keep.iter().all(|&k| k) {
            return Some(());
        }
        let mut snap = self.clone();
        for (iv, &k) in snap.iv.iter_mut().zip(&keep) {
            if k {
                iv.fe = Fe::Val(Value::new(0, (iv.hi - iv.lo + 1) as usize, false));
            }
        }
        let expr = snap.to_expr(width)?;
        let off = alloc(width);
        temps.push(ProtoAssignStatement {
            dst: VarOffset::Comb(off),
            dst_width: width,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr,
            dst_ff_current_offset: 0,
            token,
        });
        let ver = self.next_ver;
        self.next_ver += 1;
        // Adjacent snapshotted intervals coalesce: the temp holds them at
        // their own bit positions, so one read spans the whole run — and an
        // all-snapshotted map collapses back to the single full-width read.
        let mut iv: Vec<Interval> = Vec::with_capacity(self.iv.len());
        let mut snapped: Vec<bool> = Vec::with_capacity(self.iv.len());
        for (old, &k) in self.iv.iter().zip(&keep) {
            if !k && snapped.last() == Some(&true) {
                iv.last_mut()?.hi = old.hi;
                continue;
            }
            iv.push(Interval {
                hi: old.hi,
                lo: old.lo,
                fe: old.fe.clone(),
                ver: if k { old.ver } else { ver },
            });
            snapped.push(!k);
        }
        for (iv, &s) in iv.iter_mut().zip(&snapped) {
            if s {
                iv.fe = Fe::Var {
                    var_offset: VarOffset::Comb(off),
                    var_full_width: width,
                    hi: iv.hi,
                    lo: iv.lo,
                };
            }
        }
        self.iv = iv;
        Some(())
    }

    /// The accumulated value's bits [hi:lo] as one operand: a slice when one
    /// interval covers the range, a concatenation of the pieces otherwise.
    fn fe_range(&self, hi: u32, lo: u32) -> Option<Fe> {
        if hi < lo || hi as usize >= self.total_width() {
            return None;
        }
        let slice = |iv: &Interval, hi: u32, lo: u32| -> Option<Fe> {
            if iv.lo == lo && iv.hi == hi {
                Some(iv.fe.clone())
            } else {
                iv.fe.split(hi - iv.lo, lo - iv.lo)
            }
        };
        if let Some(iv) = self.iv.iter().find(|iv| iv.lo <= lo && hi <= iv.hi) {
            return slice(iv, hi, lo);
        }
        let width = (hi - lo + 1) as usize;
        // MSB-first concatenation, matching `to_expr`.
        let mut elements = Vec::new();
        for iv in self.iv.iter().rev() {
            if iv.hi < lo || iv.lo > hi {
                continue;
            }
            let (phi, plo) = (iv.hi.min(hi), iv.lo.max(lo));
            let w = (phi - plo + 1) as usize;
            elements.push((Box::new(slice(iv, phi, plo)?.to_expr(w)?), 1usize, w));
        }
        Some(Fe::Op(ProtoExpression::Concatenation {
            elements,
            width,
            expr_context: ExpressionContext {
                width,
                signed: false,
            },
        }))
    }

    fn total_width(&self) -> usize {
        self.iv.last().map_or(0, |iv| iv.hi as usize + 1)
    }

    /// Emit-weighted size of the accumulated value, counting only the
    /// composed subtrees (a plain read or literal is free to duplicate).
    fn op_nodes(&self, cap: usize) -> usize {
        let mut n = 0usize;
        for iv in &self.iv {
            if let Fe::Op(e) = &iv.fe {
                n = n.saturating_add(expr_nodes_capped(e, cap));
                if n > cap {
                    break;
                }
            }
        }
        n
    }

    /// Split intervals so that `lo` and `hi + 1` fall on boundaries.
    fn split_at(&mut self, hi: u32, lo: u32) -> Option<()> {
        for cut in [lo, hi + 1] {
            let pos = self.iv.iter().position(|iv| iv.lo < cut && cut <= iv.hi);
            if let Some(p) = pos {
                let iv = &self.iv[p];
                let upper = iv.fe.split(iv.hi - iv.lo, cut - iv.lo)?;
                let lower = iv.fe.split(cut - 1 - iv.lo, 0)?;
                let (ivhi, ivlo, ver) = (iv.hi, iv.lo, iv.ver);
                self.iv.splice(
                    p..=p,
                    [
                        Interval {
                            hi: cut - 1,
                            lo: ivlo,
                            fe: lower,
                            ver,
                        },
                        Interval {
                            hi: ivhi,
                            lo: cut,
                            fe: upper,
                            ver,
                        },
                    ],
                );
            }
        }
        Some(())
    }

    /// Overwrite bits [hi:lo] with `fe` as a new generation.
    fn write(&mut self, hi: u32, lo: u32, fe: Fe) -> Option<()> {
        self.split_at(hi, lo)?;
        let first = self.iv.iter().position(|iv| iv.lo == lo)?;
        let mut last = first;
        while self.iv[last].hi != hi {
            last += 1;
            if last >= self.iv.len() {
                return None;
            }
        }
        let ver = self.next_ver;
        self.next_ver += 1;
        self.iv.splice(first..=last, [Interval { hi, lo, fe, ver }]);
        self.coalesce_vals();
        Some(())
    }

    /// Merge adjacent constant intervals (both written) into one literal so
    /// priority chains over multi-field constants stay a single interval
    /// (e.g. `dec = '0; dec[i] = '1'` folds to a one-hot literal).
    fn coalesce_vals(&mut self) {
        let mut i = 0;
        while i + 1 < self.iv.len() {
            let a = &self.iv[i];
            let b = &self.iv[i + 1];
            if a.ver > 0 && b.ver > 0 && matches!(a.fe, Fe::Val(_)) && matches!(b.fe, Fe::Val(_)) {
                let w = (b.hi - a.lo + 1) as usize;
                let mut v = Value::new(0, w, false);
                if let (Fe::Val(va), Fe::Val(vb)) = (&a.fe, &b.fe) {
                    v.assign(va.clone(), (a.hi - a.lo) as usize, 0);
                    v.assign(vb.clone(), (b.hi - a.lo) as usize, (b.lo - a.lo) as usize);
                }
                let merged = Interval {
                    hi: b.hi,
                    lo: a.lo,
                    fe: Fe::Val(v),
                    ver: a.ver.max(b.ver),
                };
                self.iv.splice(i..=i + 1, [merged]);
            } else {
                i += 1;
            }
        }
    }

    fn to_expr(&self, total_width: usize) -> Option<ProtoExpression> {
        if self.iv.len() == 1 {
            return self.iv[0].fe.to_expr(total_width);
        }
        // MSB-first concatenation.
        let mut elements = Vec::with_capacity(self.iv.len());
        for iv in self.iv.iter().rev() {
            let w = (iv.hi - iv.lo + 1) as usize;
            elements.push((Box::new(iv.fe.to_expr(w)?), 1usize, w));
        }
        Some(ProtoExpression::Concatenation {
            elements,
            width: total_width,
            expr_context: ExpressionContext {
                width: total_width,
                signed: false,
            },
        })
    }
}

/// Fold the value of `dst` across `writer_stmts` (top-level statements of
/// the block, program order).  Returns rename temps (in program order) plus
/// the fused full-width RHS, or None when the shape is unsupported.
/// Intervals never written materialize as self-reads (previous-pass value),
/// so guard-only chains fold too.
fn fold_var(
    body: &[ProtoStatement],
    writer_stmts: &[usize],
    dst: i64,
    width: usize,
    alloc: &mut dyn FnMut(usize) -> isize,
    token: TokenRange,
) -> Option<(Vec<ProtoAssignStatement>, ProtoExpression)> {
    let mut map = IMap::new(width, dst);
    let mut temps: Vec<ProtoAssignStatement> = Vec::new();
    for &si in writer_stmts {
        fold_stmt(&body[si], dst, &mut map, &mut temps, alloc, token)?;
    }
    let expr = map.to_expr(width)?;
    Some((temps, expr))
}

// ---------------------------------------------------------------------------
// Selector-table compression
// ---------------------------------------------------------------------------
//
// An instruction decoder writes each of its outputs from ONE priority chain
// over a shared narrow selector (`casez ({inst[31:26], inst[14:12]})` …), and
// the per-variable fold above replicates the WHOLE chain of arm conditions
// into every output's RHS — hundreds of compares re-evaluated per output per
// settle.  When every arm condition is a masked equality on one common
// selector expression, the chain IS a function of the selector alone:
// evaluate it per selector value at build time and replace the RHS with a
// table lookup (constant words + shift).

fn lut_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_VSPLIT_LUT").as_deref() != Ok("0"))
}

/// Bisection knob: stop compressing after N chains.
fn lut_budget_ok() -> bool {
    static MAX: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    static USED: AtomicUsize = AtomicUsize::new(0);
    match MAX.get_or_init(|| {
        std::env::var("VERYL_VSPLIT_LUT_MAX")
            .ok()
            .and_then(|s| s.parse().ok())
    }) {
        None => true,
        Some(n) => USED.fetch_add(1, Relaxed) < *n,
    }
}

/// Table size cap in bits: at most 64 constant 64-bit words materialized
/// as a balanced mux.
const LUT_MAX_TABLE_BITS: usize = 4096;
/// Chains shorter than this stay as compares — the win is not worth churn.
const LUT_MIN_ARMS: usize = 8;
/// Because the table stores an INDEX, a handful of non-constant leaves (an
/// `!inst[25]` among hundreds of constant arms) still compresses.  Every
/// leaf is then evaluated unconditionally, so each must stay small.
const LUT_MAX_LEAVES: usize = 8;
/// Emit-weighted size cap per non-constant leaf.
const LUT_MAX_LEAF_NODES: usize = 16;

fn ctx_of(width: usize) -> ExpressionContext {
    ExpressionContext {
        width,
        signed: false,
    }
}

fn lut_const_u64(e: &ProtoExpression) -> Option<u64> {
    if let ProtoExpression::Value {
        value: Value::U64(v),
        ..
    } = e
        && v.mask_xz == 0
    {
        Some(v.payload)
    } else {
        None
    }
}

/// Match a chain condition against `sel == k`, `(sel & m) == k`, or the
/// wildcard forms (a `==?` constant's mask_xz bits are don't-cares).
/// Returns the selector expression with the effective mask and value.
fn lut_cond(e: &ProtoExpression) -> Option<(&ProtoExpression, u64, u64)> {
    use ProtoExpression as PE;
    let PE::Binary { x, op, y, .. } = e else {
        return None;
    };
    let wild = match op {
        Op::Eq => false,
        Op::EqWildcard => true,
        _ => return None,
    };
    let PE::Value {
        value: Value::U64(k),
        ..
    } = y.as_ref()
    else {
        return None;
    };
    if k.signed || (k.mask_xz != 0 && !wild) {
        return None;
    }
    let kw = k.width.min(64);
    let width_mask = if kw >= 64 { !0u64 } else { (1u64 << kw) - 1 };
    let mut mask = width_mask & !k.mask_xz;
    let sel = match x.as_ref() {
        PE::Binary {
            x: ax,
            op: Op::BitAnd,
            y: ay,
            ..
        } => {
            let m = lut_const_u64(ay)?;
            mask &= m;
            ax.as_ref()
        }
        other => other,
    };
    // The compare must see the selector bits unmodified: a selector wider
    // than the compare would truncate, a narrower one zero-extends (fine).
    if sel_width(sel)? > k.width as usize {
        return None;
    }
    Some((sel, mask, k.payload & mask))
}

fn sel_width(e: &ProtoExpression) -> Option<usize> {
    use ProtoExpression as PE;
    Some(match e {
        PE::Variable { width, .. }
        | PE::Value { width, .. }
        | PE::Unary { width, .. }
        | PE::Binary { width, .. }
        | PE::Concatenation { width, .. }
        | PE::Ternary { width, .. }
        | PE::DynamicVariable { width, .. } => *width,
        PE::HierVariable(_) => return None,
    })
}

/// A folded priority chain `c1 ? l1 : c2 ? l2 : … : ldef` reduced to
/// `(selector, (mask, value, leaf index) per arm, default leaf, leaves)`.
/// Arm values are deduplicated into the leaf list, so the arms carry indices
/// rather than the values themselves.
type LutChain<'a> = (
    &'a ProtoExpression,
    Vec<(u64, u64, usize)>,
    usize,
    Vec<&'a ProtoExpression>,
);

fn lut_chain<'a>(mut e: &'a ProtoExpression, width: usize) -> Option<LutChain<'a>> {
    use ProtoExpression as PE;
    // A one-element concatenation wraps a chain without changing it.
    while let PE::Concatenation { elements, .. } = e {
        let [(inner, 1, _)] = elements.as_slice() else {
            return None;
        };
        e = inner.as_ref();
    }
    if width > 64 || !width.is_power_of_two() {
        return None;
    }
    let mut sel: Option<(&ProtoExpression, String)> = None;
    let mut arms: Vec<(u64, u64, usize)> = Vec::new();
    let mut leaves: Vec<&'a ProtoExpression> = Vec::new();
    let mut leaf_keys: Vec<String> = Vec::new();
    let leaf_of = |leaves: &mut Vec<&'a ProtoExpression>,
                   keys: &mut Vec<String>,
                   e: &'a ProtoExpression|
     -> Option<usize> {
        let key = format!("{e:?}");
        if let Some(i) = keys.iter().position(|k| *k == key) {
            return Some(i);
        }
        if leaves.len() >= LUT_MAX_LEAVES {
            lut_diag(&format!("too many leaves: {key:.200}"));
            return None;
        }
        if lut_const_u64(e).is_none()
            && expr_nodes_capped(e, LUT_MAX_LEAF_NODES) > LUT_MAX_LEAF_NODES
        {
            lut_diag(&format!("big leaf: {key:.200}"));
            return None;
        }
        leaves.push(e);
        keys.push(key);
        Some(leaves.len() - 1)
    };
    loop {
        match e {
            PE::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                let Some((s, m, k)) = lut_cond(cond) else {
                    lut_diag(&format!(
                        "cond mismatch after {} arms: {:.200}",
                        arms.len(),
                        format!("{cond:?}")
                    ));
                    return None;
                };
                let li = leaf_of(&mut leaves, &mut leaf_keys, true_expr)?;
                match &sel {
                    None => sel = Some((s, format!("{s:?}"))),
                    Some((_, d)) => {
                        if format!("{s:?}") != *d {
                            lut_diag(&format!(
                                "selector change after {} arms: {:.200}",
                                arms.len(),
                                format!("{s:?}")
                            ));
                            return None;
                        }
                    }
                }
                arms.push((m, k, li));
                e = false_expr;
            }
            _ => {
                let def = leaf_of(&mut leaves, &mut leaf_keys, e)?;
                let (sel, _) = sel?;
                if arms.len() < LUT_MIN_ARMS {
                    return None;
                }
                return Some((sel, arms, def, leaves));
            }
        }
    }
}

/// Diagnostic: why a chain refused to compress.
fn lut_diag(msg: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var("VERYL_VSPLIT_LUT_DIAG").as_deref() == Ok("1")) {
        eprintln!("[vsplit_lut] {msg}");
    }
}

/// Try to replace a folded chain with a selector-table lookup, returning the
/// rename temps to place ahead of the fused write and the new RHS.
fn try_lut_compress(
    expr: &ProtoExpression,
    width: usize,
    alloc: &mut dyn FnMut(usize) -> isize,
    token: TokenRange,
) -> Option<(Vec<ProtoAssignStatement>, ProtoExpression)> {
    use ProtoExpression as PE;
    let (sel, arms, def, leaves) = lut_chain(expr, width)?;
    let selw = sel_width(sel)?;
    // `VERYL_VSPLIT_LUT_DUMP=1`: dump every accepted chain's ingredients.
    static DUMP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *DUMP.get_or_init(|| std::env::var("VERYL_VSPLIT_LUT_DUMP").as_deref() == Ok("1")) {
        eprintln!(
            "[lut_dump] width={width} selw={selw} arms={:x?}\n  sel={:.400}\n  def={def} leaves={:.600}",
            arms,
            format!("{sel:?}"),
            format!("{leaves:?}"),
        );
    }
    // The table stores leaf INDICES, so a lookup is followed by a leaf
    // select.  A narrow all-constant chain instead stores the values
    // themselves and drops the select.
    let const_leaves: Option<Vec<u64>> = leaves.iter().map(|l| lut_const_u64(l)).collect();
    let direct = const_leaves
        .as_ref()
        .filter(|vs| width <= 2 && vs.iter().all(|&v| v < (1 << width)))
        .is_some();
    let ew = if direct {
        width
    } else if leaves.len() <= 4 {
        2
    } else {
        4
    };
    if selw == 0 || selw > 12 || (1usize << selw) * ew > LUT_MAX_TABLE_BITS {
        return None;
    }
    // Bake the table: for each selector value, the FIRST matching arm wins.
    let n = 1usize << selw;
    let entries_per_word = 64 / ew;
    let mut words = vec![0u64; n.div_ceil(entries_per_word)];
    let emask = if ew >= 64 { !0u64 } else { (1u64 << ew) - 1 };
    for s in 0..n {
        let sv = s as u64;
        let li = arms
            .iter()
            .find(|&&(m, k, _)| sv & m == k)
            .map_or(def, |&(_, _, li)| li);
        let entry = if direct {
            const_leaves.as_ref().unwrap()[li]
        } else {
            li as u64
        };
        words[s / entries_per_word] |= (entry & emask) << ((s % entries_per_word) * ew);
    }
    // Materialize the selector.  Each output gets its OWN temp even when
    // siblings share the selector expression: every fused assign lands at
    // its variable's last-write position, and a temp shared across
    // positions would be read before it is written by the earlier ones.
    let mut temps: Vec<ProtoAssignStatement> = Vec::new();
    let sel_off = {
        let off = alloc(selw);
        temps.push(ProtoAssignStatement {
            dst: VarOffset::Comb(off),
            dst_width: selw,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: sel.clone(),
            dst_ff_current_offset: 0,
            token,
        });
        off
    };
    let sel_var = |w: usize| PE::Variable {
        var_offset: VarOffset::Comb(sel_off),
        select: None,
        dynamic_select: None,
        width: w,
        var_full_width: selw,
        expr_context: ctx_of(w),
    };
    let val = |v: u64, w: usize| PE::Value {
        value: Value::U64(veryl_analyzer::value::ValueU64 {
            payload: v,
            mask_xz: 0,
            width: w as u32,
            signed: false,
        }),
        width: w,
        expr_context: ctx_of(w),
    };
    let bin = |x: PE, op: Op, y: PE, w: usize| PE::Binary {
        x: Box::new(x),
        op,
        y: Box::new(y),
        width: w,
        expr_context: ctx_of(w),
    };
    // Balanced constant mux over the table words, indexed by the selector's
    // upper bits.
    fn word_mux(
        words: &[u64],
        lo: usize,
        hi: usize,
        idx: &dyn Fn(usize) -> ProtoExpression,
        val: &dyn Fn(u64, usize) -> ProtoExpression,
    ) -> ProtoExpression {
        if hi - lo == 1 {
            return val(words[lo], 64);
        }
        let mid = lo + (hi - lo) / 2;
        ProtoExpression::Ternary {
            cond: Box::new(idx(mid)),
            true_expr: Box::new(word_mux(words, mid, hi, idx, val)),
            false_expr: Box::new(word_mux(words, lo, mid, idx, val)),
            width: 64,
            expr_context: ctx_of(64),
        }
    }
    let word = if words.len() == 1 {
        val(words[0], 64)
    } else {
        // idx(mid) = (sel >> log2(entries_per_word)) >= mid
        let epw_shift = entries_per_word.trailing_zeros() as u64;
        let idx = |mid: usize| {
            bin(
                bin(sel_var(selw), Op::LogicShiftR, val(epw_shift, selw), selw),
                Op::GreaterEq,
                val(mid as u64, selw),
                1,
            )
        };
        word_mux(&words, 0, words.len(), &idx, &val)
    };
    // shift = (sel % entries_per_word) * ew, both powers of two.
    let shift = if entries_per_word == 1 {
        val(0, 6)
    } else {
        let within = bin(
            sel_var(selw),
            Op::BitAnd,
            val((entries_per_word - 1) as u64, selw),
            selw,
        );
        if ew == 1 {
            within
        } else {
            bin(
                within,
                Op::LogicShiftL,
                val(ew.trailing_zeros() as u64, selw),
                12,
            )
        }
    };
    let entry = bin(
        bin(word, Op::LogicShiftR, shift, 64),
        Op::BitAnd,
        val(emask, ew),
        ew,
    );
    if direct {
        return Some((temps, entry));
    }
    // Leaf select: `idx == n ? leaf_n : …`, the chain's default innermost.
    let mut out = leaves[def].clone();
    for (li, leaf) in leaves.iter().enumerate() {
        if li == def {
            continue;
        }
        out = PE::Ternary {
            cond: Box::new(bin(entry.clone(), Op::Eq, val(li as u64, ew), 1)),
            true_expr: Box::new((*leaf).clone()),
            false_expr: Box::new(out),
            width,
            expr_context: ctx_of(width),
        };
    }
    Some((temps, out))
}

/// Replace plain / statically-selected reads of `dst` with the value `base`
/// has accumulated over the same bits — a snapshot read for bits a write
/// covers, `dst` itself for bits none does.  Dynamic selects on the
/// destination are unsupported (None).
fn replace_dst_reads(e: &mut ProtoExpression, dst: i64, base: &IMap) -> Option<()> {
    use ProtoExpression as PE;
    if let PE::Variable {
        var_offset,
        select,
        dynamic_select,
        width,
        ..
    } = e
        && let VarOffset::Comb(o) = var_offset
        && *o as i64 == dst
    {
        if dynamic_select.is_some() {
            return None;
        }
        let w = *width;
        let (hi, lo) = match select {
            None => (w.saturating_sub(1) as u32, 0),
            Some((a, b)) => ((*a).max(*b) as u32, (*a).min(*b) as u32),
        };
        *e = base.fe_range(hi, lo)?.to_expr(w)?;
        return Some(());
    }
    match e {
        PE::Variable { dynamic_select, .. } => {
            if let Some(d) = dynamic_select {
                replace_dst_reads(&mut d.index_expr, dst, base)?;
            }
            Some(())
        }
        PE::Value { .. } | PE::HierVariable(_) => Some(()),
        PE::Unary { x, .. } => replace_dst_reads(x, dst, base),
        PE::Binary { x, y, .. } => {
            replace_dst_reads(x, dst, base)?;
            replace_dst_reads(y, dst, base)
        }
        PE::Concatenation { elements, .. } => {
            for (x, _, _) in elements {
                replace_dst_reads(x, dst, base)?;
            }
            Some(())
        }
        PE::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            replace_dst_reads(cond, dst, base)?;
            replace_dst_reads(true_expr, dst, base)?;
            replace_dst_reads(false_expr, dst, base)
        }
        PE::DynamicVariable {
            base_offset,
            index_expr,
            dynamic_select,
            ..
        } => {
            if let VarOffset::Comb(o) = base_offset
                && *o as i64 == dst
            {
                return None;
            }
            replace_dst_reads(index_expr, dst, base)?;
            if let Some(d) = dynamic_select {
                replace_dst_reads(&mut d.index_expr, dst, base)?;
            }
            Some(())
        }
    }
}

/// Bits of `dst` that `stmts` assigns: `all` on every path, `any` on some.
/// Bits outside `all` keep their incoming value, so the fold carries that
/// value into the merged result; bits outside `any` are not merged at all.
fn written_bits(
    stmts: &[ProtoStatement],
    dst: i64,
    width: usize,
    all: &mut [bool],
    any: &mut [bool],
) {
    for stmt in stmts {
        match stmt {
            ProtoStatement::Assign(x) => {
                if !matches!(x.dst, VarOffset::Comb(o) if o as i64 == dst) {
                    continue;
                }
                let (hi, lo) = match x.select {
                    Some((a, b)) => (a.max(b), a.min(b)),
                    None => (x.dst_width.saturating_sub(1), 0),
                };
                for i in lo..=hi.min(width - 1) {
                    all[i] = true;
                    any[i] = true;
                }
            }
            ProtoStatement::If(x) => {
                let (mut ta, mut tn) = (vec![false; width], vec![false; width]);
                let (mut fa, mut fn_) = (vec![false; width], vec![false; width]);
                written_bits(&x.true_side, dst, width, &mut ta, &mut tn);
                written_bits(&x.false_side, dst, width, &mut fa, &mut fn_);
                for i in 0..width {
                    all[i] |= ta[i] && fa[i];
                    any[i] |= tn[i] || fn_[i];
                }
            }
            _ => {}
        }
    }
}

/// Size above which the accumulated value is worth hiding behind a temp
/// rather than letting a merge duplicate it.  Below this the temp — an
/// extra comb slot plus its own statement — costs more than the copy.
const SNAPSHOT_MIN_NODES: usize = 32;

/// True when folding `x` would emit the accumulated value more than once.
/// A merged bit carries that value from whichever side leaves it unwritten,
/// so a bit that both sides may leave — while some path still writes it —
/// emits it on both arms of the ternary.  Nesting that shape doubles the
/// value per level.
fn duplicates_entry(x: &ProtoIfStatement, dst: i64, width: usize) -> bool {
    let (mut ta, mut tn) = (vec![false; width], vec![false; width]);
    let (mut fa, mut fn_) = (vec![false; width], vec![false; width]);
    written_bits(&x.true_side, dst, width, &mut ta, &mut tn);
    written_bits(&x.false_side, dst, width, &mut fa, &mut fn_);
    (0..width).any(|i| (tn[i] || fn_[i]) && !ta[i] && !fa[i])
}

fn expr_reads_dst(e: &ProtoExpression, dst: i64) -> bool {
    e.reads_offset(VarOffset::Comb(dst as isize))
}

fn fold_stmt(
    stmt: &ProtoStatement,
    dst: i64,
    map: &mut IMap,
    temps: &mut Vec<ProtoAssignStatement>,
    alloc: &mut dyn FnMut(usize) -> isize,
    token: TokenRange,
) -> Option<()> {
    match stmt {
        ProtoStatement::Assign(x) => {
            let VarOffset::Comb(o) = x.dst else {
                return Some(());
            };
            if o as i64 != dst {
                return Some(());
            }
            let (hi, lo) = match x.select {
                Some((a, b)) => (a.max(b) as u32, a.min(b) as u32),
                None => (x.dst_width.saturating_sub(1) as u32, 0),
            };
            // The fused ternary/concat needs the operand sized exactly to
            // its written range.  A store truncates a wider RHS to the
            // written width and zero-extends a narrower unsigned one; both
            // become an explicit mask at min(got, want) bits (safe against
            // an unmasked width-growing root, see `build_binary_root`).
            // Sign-extension and the I64/I128 register-representation
            // boundary are left to the fallback (statement stays as-is).
            let want = (hi - lo + 1) as usize;
            let got = x.expr.width();
            let mut expr = if got == want {
                x.expr.clone()
            } else if got.max(want) <= 64 && !(got < want && x.expr.expr_context().signed) {
                let mask_w = got.min(want);
                let mask = if mask_w == 64 {
                    u64::MAX
                } else {
                    (1u64 << mask_w) - 1
                };
                let ctx = ExpressionContext {
                    width: want,
                    signed: false,
                };
                ProtoExpression::Binary {
                    x: Box::new(x.expr.clone()),
                    op: crate::ir::Op::BitAnd,
                    y: Box::new(ProtoExpression::Value {
                        value: Value::new(mask, want, false),
                        width: want,
                        expr_context: ctx,
                    }),
                    width: want,
                    expr_context: ctx,
                }
            } else {
                return None;
            };
            // A read of `dst` in the RHS means "the value as of this point
            // in the chain" (sequential RMW).  Snapshot the accumulated
            // value into a fresh temp and redirect the read there — using
            // the final fused value instead would be circular, and the raw
            // self-read would see the previous pass.
            if expr_reads_dst(&expr, dst) {
                map.snapshot_to_temp(x.dst_width, temps, alloc, x.token)?;
                replace_dst_reads(&mut expr, dst, map)?;
            }
            map.write(hi, lo, Fe::from_expr(expr))
        }
        ProtoStatement::If(x) => {
            let cond = x.cond.as_ref()?;
            // Behind a temp each level adds a read instead of a copy.  Left
            // in place, a deep nest reached millions of nodes from seventeen
            // writes on one design.
            let width = map.total_width();
            if map.op_nodes(SNAPSHOT_MIN_NODES) > SNAPSHOT_MIN_NODES
                && width > 0
                && duplicates_entry(x, dst, width)
            {
                map.snapshot_to_temp(width, temps, alloc, token)?;
            }
            let entry_snapshot: Vec<(u32, u32)> = map.iv.iter().map(|iv| (iv.lo, iv.ver)).collect();
            let mut t = map.clone();
            let mut f = map.clone();
            // Keep generation counters unique across the three maps so a
            // branch-written interval can never collide with an entry ver.
            t.next_ver = map.next_ver;
            f.next_ver = map.next_ver + 1_000_000;
            for s in &x.true_side {
                fold_stmt(s, dst, &mut t, temps, alloc, token)?;
            }
            for s in &x.false_side {
                fold_stmt(s, dst, &mut f, temps, alloc, token)?;
            }
            merge_if(cond, t, f, &entry_snapshot, map)
        }
        _ => Some(()),
    }
}

/// Merge the true/false branch maps into `out`: untouched-by-both intervals
/// keep the entry value; anything else becomes `cond ? t : f`.
fn merge_if(
    cond: &ProtoExpression,
    mut t: IMap,
    mut f: IMap,
    entry: &[(u32, u32)],
    out: &mut IMap,
) -> Option<()> {
    // Align boundaries: union of both maps' cut points.
    let cuts: Vec<(u32, u32)> =
        t.iv.iter()
            .chain(f.iv.iter())
            .map(|iv| (iv.hi, iv.lo))
            .collect();
    for &(hi, lo) in &cuts {
        t.split_at(hi, lo)?;
        f.split_at(hi, lo)?;
    }
    debug_assert_eq!(t.iv.len(), f.iv.len());

    let entry_ver = |lo: u32| -> u32 {
        // Entry interval covering `lo` (entry list is LSB-sorted).
        let mut v = 0;
        for &(elo, ever) in entry {
            if elo <= lo {
                v = ever;
            } else {
                break;
            }
        }
        v
    };

    // Fresh generations for merged intervals.  They MUST be unique (not a
    // shared sentinel): a nested if takes its entry snapshot from intervals
    // that may themselves be merge results, and its branch maps produce
    // further merge results — with a shared sentinel (the old `u32::MAX`)
    // all three generations compare equal and the untouched-by-both test
    // below wrongly keeps the true branch verbatim, dropping the condition
    // and the false branch (mis-folded heliodor's fp_corner `ok` chain).
    let mut next = t.next_ver.max(f.next_ver).max(out.next_ver) + 1;
    let mut merged: Vec<Interval> = Vec::with_capacity(t.iv.len());
    for (ti, fi) in t.iv.iter().zip(f.iv.iter()) {
        debug_assert_eq!((ti.hi, ti.lo), (fi.hi, fi.lo));
        let ever = entry_ver(ti.lo);
        if ti.ver == ever && fi.ver == ever {
            merged.push(ti.clone());
            continue;
        }
        let w = (ti.hi - ti.lo + 1) as usize;
        let te = ti.fe.to_expr(w)?;
        let fe_ = fi.fe.to_expr(w)?;
        merged.push(Interval {
            hi: ti.hi,
            lo: ti.lo,
            fe: Fe::Op(ProtoExpression::Ternary {
                cond: Box::new(cond.clone()),
                true_expr: Box::new(te),
                false_expr: Box::new(fe_),
                width: w,
                expr_context: ExpressionContext {
                    width: w,
                    signed: false,
                },
            }),
            ver: {
                let v = next;
                next += 1;
                v
            },
        });
    }
    out.iv = merged;
    out.next_ver = next;
    out.coalesce_vals();
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ExpressionContext, Op, ProtoAssignStatement, ProtoIfStatement, VarOffset};
    use veryl_analyzer::value::{Value, ValueU64};
    use veryl_parser::token_range::TokenRange;

    fn ctx(width: usize) -> ExpressionContext {
        ExpressionContext {
            width,
            signed: false,
        }
    }

    fn lit(v: u64, w: usize) -> ProtoExpression {
        ProtoExpression::Value {
            value: Value::U64(ValueU64 {
                payload: v,
                mask_xz: 0,
                width: w as u32,
                signed: false,
            }),
            width: w,
            expr_context: ctx(w),
        }
    }

    fn cvar(off: isize, w: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: None,
            dynamic_select: None,
            width: w,
            var_full_width: w,
            expr_context: ctx(w),
        }
    }

    fn assign(dst: isize, w: usize, expr: ProtoExpression) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(dst),
            dst_width: w,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr,
            dst_ff_current_offset: 0,
            token: TokenRange::default(),
        })
    }

    fn assign_bits(
        dst: isize,
        w: usize,
        hi: usize,
        lo: usize,
        expr: ProtoExpression,
    ) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(dst),
            dst_width: w,
            select: Some((hi, lo)),
            dynamic_select: None,
            rhs_select: None,
            expr,
            dst_ff_current_offset: 0,
            token: TokenRange::default(),
        })
    }

    fn cvar_bits(off: isize, full_w: usize, hi: usize, lo: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: Some((hi, lo)),
            dynamic_select: None,
            width: hi - lo + 1,
            var_full_width: full_w,
            expr_context: ctx(hi - lo + 1),
        }
    }

    fn cond_write(cond_off: isize, dst: isize, w: usize, val: u64) -> ProtoStatement {
        ProtoStatement::If(ProtoIfStatement {
            cond: Some(cvar(cond_off, 1)),
            true_side: vec![assign(dst, w, lit(val, w))],
            false_side: vec![],
        })
    }

    #[test]
    fn node_count_walks_a_dynamic_bit_select_index() {
        // The subtree under a dynamic bit-select on an array-element read
        // must be counted, or a bomb hiding there passes the budget.
        let deep = {
            let mut e = lit(0, 8);
            for _ in 0..40 {
                e = ProtoExpression::Unary {
                    op: Op::LogicNot,
                    x: Box::new(e),
                    width: 8,
                    expr_context: ctx(8),
                };
            }
            e
        };
        let e = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0x0),
            stride: 8,
            element_native_bytes: 4,
            index_expr: Box::new(lit(0, 8)),
            num_elements: 4,
            select: None,
            dynamic_select: Some(crate::ir::ProtoDynamicBitSelect {
                index_expr: Box::new(deep),
                elem_width: 8,
                window: 1,
                num_elements: 4,
            }),
            width: 8,
            expr_context: ctx(8),
        };
        assert!(expr_nodes_capped(&e, usize::MAX) > 40);
    }

    #[test]
    fn node_count_charges_a_wide_constant_per_word() {
        // A 512-bit literal emits one word literal per 64 bits; the budget
        // tracks emitted bytes, so it must cost 8 units, not 1.
        assert_eq!(expr_nodes_capped(&lit(0, 512), usize::MAX), 8);
        assert_eq!(expr_nodes_capped(&lit(0, 64), usize::MAX), 1);
    }

    #[test]
    fn node_count_is_exact_and_caps_early() {
        // (a & b) | 1  = 5 nodes.
        let e = ProtoExpression::Binary {
            x: Box::new(ProtoExpression::Binary {
                x: Box::new(cvar(0x0, 8)),
                op: Op::BitAnd,
                y: Box::new(cvar(0x8, 8)),
                width: 8,
                expr_context: ctx(8),
            }),
            op: Op::BitOr,
            y: Box::new(lit(1, 8)),
            width: 8,
            expr_context: ctx(8),
        };
        assert_eq!(expr_nodes_capped(&e, 100), 5);
        assert!(expr_nodes_capped(&e, 2) > 2);
    }

    #[test]
    fn node_count_survives_a_deep_chain() {
        // The counter must be iterative: the trees it exists to reject are
        // deep enough that a recursive walk would itself overflow.
        let mut e = lit(0, 8);
        for _ in 0..30_000 {
            e = ProtoExpression::Unary {
                op: Op::LogicNot,
                x: Box::new(e),
                width: 8,
                expr_context: ctx(8),
            };
        }
        assert!(expr_nodes_capped(&e, usize::MAX) > 30_000);
        // Iterative drop too: hand the chain back leaf-first.
        while let ProtoExpression::Unary { x, .. } = e {
            e = *x;
        }
    }

    #[test]
    fn budget_rejects_an_oversized_fold_and_keeps_the_writes() {
        // Two overlapping conditional writes fuse into a ternary chain of
        // ~10 nodes; a 4-node cap must reject the fold and leave the block
        // untouched, a large cap must fuse it.
        let block = || vec![cond_write(0x100, 0x0, 8, 1), cond_write(0x108, 0x0, 8, 2)];
        let mut alloc_at = 0x1000isize;
        let mut alloc = |w: usize| -> isize {
            let off = alloc_at;
            alloc_at += crate::ir::variable::native_bytes(w) as isize;
            off
        };

        TEST_MAX_NODES.with(|c| c.set(Some(4)));
        let mut body = block();
        let mut stats = RunStats::default();
        split_block(&mut body, &mut stats, &mut alloc);
        assert_eq!(stats.skip_budget, 1, "the fold must be rejected");
        assert_eq!(stats.fused_vars, 0);
        assert_eq!(body.len(), 2, "the original writes must survive");

        TEST_MAX_NODES.with(|c| c.set(Some(1 << 20)));
        let mut body = block();
        let mut stats = RunStats::default();
        split_block(&mut body, &mut stats, &mut alloc);
        assert_eq!(stats.skip_budget, 0);
        assert_eq!(stats.fused_vars, 1, "a large cap must still fuse");
        TEST_MAX_NODES.with(|c| c.set(None));
    }

    #[test]
    fn a_field_chain_reading_an_earlier_field_leaves_no_read_of_its_own_destination() {
        // `v[2:0] = a; v[5:3] = v[2:0] + 1` writes every bit of `v` and the
        // self-read names bits an earlier write covers, so the pair is
        // acyclic.  The rename temp must not read `v`: the fused write is
        // `v`'s only writer, so a read there closes a two-statement cycle
        // and the comb scheduler refuses to elaborate the design.
        let mut body = vec![
            assign_bits(0x0, 6, 2, 0, cvar(0x100, 3)),
            assign_bits(
                0x0,
                6,
                5,
                3,
                ProtoExpression::Binary {
                    x: Box::new(cvar_bits(0x0, 6, 2, 0)),
                    op: Op::Add,
                    y: Box::new(lit(1, 3)),
                    width: 3,
                    expr_context: ctx(3),
                },
            ),
        ];
        let mut alloc_at = 0x1000isize;
        let mut alloc = |w: usize| -> isize {
            let off = alloc_at;
            alloc_at += crate::ir::variable::native_bytes(w) as isize;
            off
        };
        let mut stats = RunStats::default();
        split_block(&mut body, &mut stats, &mut alloc);

        assert_eq!(stats.fused_vars, 1, "the chain must fuse");
        for s in &body {
            let ProtoStatement::Assign(a) = s else {
                continue;
            };
            assert!(
                !a.expr.reads_offset(VarOffset::Comb(0x0)),
                "a fused statement reads the destination the chain fully covers"
            );
        }
    }

    #[test]
    fn a_nested_conditional_chain_folds_to_a_linear_size() {
        // `if a { if b { d = k } }` leaves the incoming value on both arms
        // of its merge, so folding a run of them copies that value once per
        // statement — 2^n for n statements.  The fold must stay
        // proportional to the source instead.
        const N: usize = 20;
        let mut body: Vec<ProtoStatement> = (0..N)
            .map(|i| {
                ProtoStatement::If(ProtoIfStatement {
                    cond: Some(cvar(0x100 + i as isize * 8, 1)),
                    true_side: vec![ProtoStatement::If(ProtoIfStatement {
                        cond: Some(cvar(0x200 + i as isize * 8, 1)),
                        true_side: vec![assign(0x0, 8, lit(i as u64, 8))],
                        false_side: vec![],
                    })],
                    false_side: vec![],
                })
            })
            .collect();
        let mut alloc_at = 0x1000isize;
        let mut alloc = |w: usize| -> isize {
            let off = alloc_at;
            alloc_at += crate::ir::variable::native_bytes(w) as isize;
            off
        };

        TEST_MAX_NODES.with(|c| c.set(Some(1 << 20)));
        let mut stats = RunStats::default();
        split_block(&mut body, &mut stats, &mut alloc);
        TEST_MAX_NODES.with(|c| c.set(None));

        assert_eq!(stats.fused_vars, 1, "the chain must fuse");
        assert_eq!(stats.skip_budget, 0);
        let nodes: usize = body
            .iter()
            .map(|s| match s {
                ProtoStatement::Assign(a) => expr_nodes_capped(&a.expr, usize::MAX),
                _ => 0,
            })
            .sum();
        assert!(nodes < 40 * N, "fold grew super-linearly: {nodes} nodes");
    }
}
