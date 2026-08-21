//! Single-reader comb def inlining (`VERYL_COMB_FUSION`) — P1 of the
//! DFG-contraction campaign (local/FUSION-DESIGN.md).
//!
//! A comb def read exactly once by later comb logic is folded into its
//! reader's expression, and the def statement disappears; the freed storage
//! becomes ghost bytes the relayout pass drops.  The statement-level
//! equivalent of the sink→localize→gcc chain, minus the chunk boundary:
//! statement-level inlining is backend-uniform (interp / Cranelift / AOT-C
//! all see the contracted statements), so VALIDATE keeps working.
//!
//! The inlined expression is wrapped in `expr & ((1<<w)-1)`: storage is
//! canonical (every store masks to the declared width), so the reader used
//! to observe a masked value, and the wrapper preserves exactly that.  The
//! mask costs nothing downstream — measured on the same designs, gcc folds
//! or elides these consumer-side masks entirely (see the W2 post-mortem in
//! the design doc).
//!
//! P1 stays conservative — a def is inlined only when ALL of:
//! - the def is a top-level bare `Assign` (no select / dynamic_select /
//!   rhs_select), `0 < dst_width <= 64`, RHS width <= 64, and the RHS does
//!   not read the def's own offset (a settle back-edge reads the PREVIOUS
//!   pass's value);
//! - the offset has exactly one comb read, appearing in a LATER top-level
//!   statement that is a plain Assign / AssignDynamic / If / Case (never
//!   inside For bodies — per-iteration re-evaluation — nor CompiledBlocks,
//!   system calls, or TB methods, whose expressions we cannot or must not
//!   rewrite);
//! - the single read is a full-width scalar load (no select, no
//!   dynamic_select);
//! - the offset is invisible externally: not read by any event statement,
//!   not in the DCE protect set, not referenced by external-component
//!   connects or derived-clock candidates;
//! - no statement between the def and the reader rewrites any input of the
//!   def's RHS, or the def's own offset (the value must be position
//!   independent over that span).

use crate::ir::big_array::BigArrayFold;
use crate::ir::event::Event;
use crate::ir::expression::{ExpressionContext, ProtoExpression};
use crate::ir::opt::lane_vector;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;
use crate::{HashMap, HashSet};
use veryl_analyzer::ir::Op;
use veryl_analyzer::value::Value;

/// Default-on for 2-state storage; `VERYL_COMB_FUSION=0` opts out.
pub fn enabled(use_4state: bool) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !use_4state
        && !FORCE_DISABLED.load(std::sync::atomic::Ordering::Relaxed)
        && *ON.get_or_init(|| std::env::var("VERYL_COMB_FUSION").as_deref() != Ok("0"))
}

/// Set by [`force_disable`]; latches on and is never cleared, so the answer
/// [`enabled`] gives cannot change once the pipeline has acted on it.
static FORCE_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A fused def's storage is never written, so a dump would show it stale
/// while the run still passes.  Must be called before analysis.
pub fn force_disable() {
    FORCE_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_COMB_FUSION_DIAG").as_deref() == Ok("1"))
}

/// `VERYL_COMB_FUSION_LIMIT=N`: stop after N inlines (bisection debug aid).
fn limit() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("VERYL_COMB_FUSION_LIMIT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(usize::MAX)
    })
}

/// `VERYL_COMB_FUSION_CHEAP_KEEP`: bisect aid — duplicate into the readers
/// but keep the def (no retire), separating "a hidden reader sees the
/// retired storage" from "the duplicated read itself changes the value".
/// `1` keeps every def, `last` keeps only the LIMIT_DUP-th (isolate one
/// retire).
fn cheap_keep(v: Option<&str>, is_last: bool) -> bool {
    match v {
        Some("1") => true,
        Some("last") => is_last,
        _ => false,
    }
}

/// `VERYL_COMB_FUSION_LIMIT_DUP=N`: cap the duplication pass alone
/// (bisection debug aid; defaults to the shared limit).
fn limit_dup() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("VERYL_COMB_FUSION_LIMIT_DUP")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(limit)
    })
}

/// A reader statement whose expressions we may rewrite.  `For` is excluded
/// (the body re-evaluates per iteration), and everything opaque
/// (CompiledBlock, system calls, TB methods, sequential blocks) is too.
fn rewritable_reader(s: &ProtoStatement) -> bool {
    matches!(
        s,
        ProtoStatement::Assign(_)
            | ProtoStatement::AssignDynamic(_)
            | ProtoStatement::If(_)
            | ProtoStatement::Case(_)
    )
}

/// Count full-width scalar reads of `off` in the expression tree, and reads
/// of any other shape (selected / dynamic-selected / index positions all
/// count as `other`, which vetoes inlining).
fn count_reads(e: &ProtoExpression, off: isize, full: &mut usize, other: &mut usize) {
    match e {
        ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select,
            width,
            var_full_width,
            expr_context,
        } => {
            if *var_offset == VarOffset::Comb(off) {
                // A replaceable read is a bare unsigned full-width load: a
                // signed-context node sign-extends the canonical storage
                // value at evaluation time, and a width-mismatched load
                // reinterprets it — substituting the (unsigned) producer
                // expression would change the value the parent sees.
                if select.is_none()
                    && dynamic_select.is_none()
                    && !expr_context.signed
                    && width == var_full_width
                {
                    *full += 1;
                } else {
                    *other += 1;
                }
            }
            if let Some(d) = dynamic_select {
                count_reads(&d.index_expr, off, full, other);
            }
        }
        ProtoExpression::DynamicVariable {
            base_offset,
            index_expr,
            dynamic_select,
            ..
        } => {
            if *base_offset == VarOffset::Comb(off) {
                *other += 1;
            }
            count_reads(index_expr, off, full, other);
            if let Some(d) = dynamic_select {
                count_reads(&d.index_expr, off, full, other);
            }
        }
        ProtoExpression::Unary { x, .. } => count_reads(x, off, full, other),
        ProtoExpression::Binary { x, y, .. } => {
            count_reads(x, off, full, other);
            count_reads(y, off, full, other);
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            count_reads(cond, off, full, other);
            count_reads(true_expr, off, full, other);
            count_reads(false_expr, off, full, other);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (x, _, _) in elements {
                count_reads(x, off, full, other);
            }
        }
        ProtoExpression::Value { .. } | ProtoExpression::HierVariable(_) => {}
    }
}

fn count_reads_stmt(s: &ProtoStatement, off: isize, full: &mut usize, other: &mut usize) {
    match s {
        ProtoStatement::Assign(a) => {
            count_reads(&a.expr, off, full, other);
            if let Some(d) = &a.dynamic_select {
                count_reads(&d.index_expr, off, full, other);
            }
        }
        ProtoStatement::AssignDynamic(a) => {
            count_reads(&a.dst_index_expr, off, full, other);
            count_reads(&a.expr, off, full, other);
            if let Some(d) = &a.dynamic_select {
                count_reads(&d.index_expr, off, full, other);
            }
        }
        ProtoStatement::If(x) => {
            if let Some(c) = &x.cond {
                count_reads(c, off, full, other);
            }
            for t in x.true_side.iter().chain(x.false_side.iter()) {
                count_reads_stmt(t, off, full, other);
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &x.arms {
                count_reads(&arm.cond, off, full, other);
                for t in &arm.body {
                    count_reads_stmt(t, off, full, other);
                }
            }
            for t in &x.default {
                count_reads_stmt(t, off, full, other);
            }
        }
        // Non-rewritable readers never reach here (vetoed earlier).
        _ => {}
    }
}

/// Replace the single full-width read of `off` with `repl`.  Returns true
/// when the substitution happened (exactly one is expected by construction).
fn replace_read(e: &mut ProtoExpression, off: isize, repl: &mut Option<ProtoExpression>) -> bool {
    if repl.is_none() {
        return true;
    }
    let is_target = matches!(
        e,
        ProtoExpression::Variable {
            var_offset,
            select: None,
            dynamic_select: None,
            width,
            var_full_width,
            expr_context,
        } if *var_offset == VarOffset::Comb(off)
            && !expr_context.signed
            && width == var_full_width
    );
    if is_target {
        *e = repl.take().unwrap();
        return true;
    }
    match e {
        ProtoExpression::Variable { dynamic_select, .. } => {
            if let Some(d) = dynamic_select {
                replace_read(&mut d.index_expr, off, repl);
            }
        }
        ProtoExpression::DynamicVariable {
            index_expr,
            dynamic_select,
            ..
        } => {
            replace_read(index_expr, off, repl);
            if let Some(d) = dynamic_select {
                replace_read(&mut d.index_expr, off, repl);
            }
        }
        ProtoExpression::Unary { x, .. } => {
            replace_read(x, off, repl);
        }
        ProtoExpression::Binary { x, y, .. } => {
            replace_read(x, off, repl);
            replace_read(y, off, repl);
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            replace_read(cond, off, repl);
            replace_read(true_expr, off, repl);
            replace_read(false_expr, off, repl);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (x, _, _) in elements {
                replace_read(x, off, repl);
            }
        }
        ProtoExpression::Value { .. } | ProtoExpression::HierVariable(_) => {}
    }
    repl.is_none()
}

fn replace_read_stmt(
    s: &mut ProtoStatement,
    off: isize,
    repl: &mut Option<ProtoExpression>,
) -> bool {
    match s {
        ProtoStatement::Assign(a) => {
            replace_read(&mut a.expr, off, repl);
            if let Some(d) = &mut a.dynamic_select {
                replace_read(&mut d.index_expr, off, repl);
            }
        }
        ProtoStatement::AssignDynamic(a) => {
            replace_read(&mut a.dst_index_expr, off, repl);
            replace_read(&mut a.expr, off, repl);
            if let Some(d) = &mut a.dynamic_select {
                replace_read(&mut d.index_expr, off, repl);
            }
        }
        ProtoStatement::If(x) => {
            if let Some(c) = &mut x.cond {
                replace_read(c, off, repl);
            }
            for t in x.true_side.iter_mut().chain(x.false_side.iter_mut()) {
                replace_read_stmt(t, off, repl);
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &mut x.arms {
                replace_read(&mut arm.cond, off, repl);
                for t in &mut arm.body {
                    replace_read_stmt(t, off, repl);
                }
            }
            for t in &mut x.default {
                replace_read_stmt(t, off, repl);
            }
        }
        _ => {}
    }
    repl.is_none()
}

/// Wrap the moved RHS so the reader observes exactly the canonical value the
/// store used to produce: `expr & ((1<<w)-1)`.  Full 64-bit defs need no
/// wrapper (the store never masked either).
fn canonical_wrap(expr: ProtoExpression, w: usize) -> ProtoExpression {
    if w >= 64 {
        return expr;
    }
    let mask = (1u64 << w) - 1;
    ProtoExpression::Binary {
        x: Box::new(expr),
        op: Op::BitAnd,
        y: Box::new(ProtoExpression::Value {
            value: Value::new(mask, w, false),
            width: w,
            expr_context: ExpressionContext {
                width: w,
                signed: false,
            },
        }),
        width: w,
        expr_context: ExpressionContext {
            width: w,
            signed: false,
        },
    }
}

/// `VERYL_COMB_FUSION_COALESCE=0` opts the field-store coalescing out while
/// keeping the single-reader inlining (per-stage A/B lever).
fn coalesce_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_COMB_FUSION_COALESCE").as_deref() != Ok("0"))
}

/// Take a destination wider than a word one 64-bit word at a time
/// (`VERYL_COMB_FUSION_WORD_COALESCE=0` opts out).  Fusing it whole makes one
/// concat as wide as the variable, which forces the multi-word paths and
/// measured sim_s +32%; per word the fused store is an ordinary word-sized
/// value, and a word assembled bit by bit becomes a single store the lane
/// merge can then collapse.
fn word_coalesce_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_COMB_FUSION_WORD_COALESCE").as_deref() != Ok("0"))
}

/// Cheap multi-reader duplication: ON by default (`VERYL_COMB_FUSION_CHEAP=0`
/// opts out).  The long-run miscompile that had parked this stage was rooted
/// in the compact (base+last) dependency gather hiding dynamic reads of an
/// array's MIDDLE elements from the read census — fixed by switching the
/// census and the position-independence gathers to the expanded form.
/// Measured after the fix (alternating x3, full workloads): sim_s -4.7% and
/// -3.7% on the two reference CPU cores.
fn cheap_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if cfg!(test) {
        // Re-read per call so unit tests can toggle the knob.
        return std::env::var("VERYL_COMB_FUSION_CHEAP").as_deref() != Ok("0");
    }
    *ON.get_or_init(|| std::env::var("VERYL_COMB_FUSION_CHEAP").as_deref() != Ok("0"))
}

/// `VERYL_COMB_FUSION_CSE=0` opts the common-subexpression collapse out
/// (per-stage A/B lever).
fn cse_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_COMB_FUSION_CSE").as_deref() != Ok("0"))
}

/// A multi-reader RHS worth recomputing at every use instead of storing and
/// loading: constants, canonical full variable loads, and static selects
/// (one load+shift+mask, at most what the retired load cost).  Everything
/// here is canonical, so no wrap is needed when it fits the def width.
fn cheap_rhs(e: &ProtoExpression) -> bool {
    match e {
        // A signed leaf is not portable: the store sign-extends it (or the
        // reader's wider context would re-sign what was an unsigned read).
        ProtoExpression::Value { value, .. } => !value.signed(),
        ProtoExpression::Variable {
            dynamic_select: None,
            select,
            width,
            var_full_width,
            expr_context,
            ..
        } => !expr_context.signed && (select.is_some() || width == var_full_width),
        _ => false,
    }
}

/// Replace EVERY full-width read of `off` with a clone of `template`.
/// Returns the number of replacements.
fn replace_all_reads(e: &mut ProtoExpression, off: isize, template: &ProtoExpression) -> usize {
    let is_target = matches!(
        e,
        ProtoExpression::Variable {
            var_offset,
            select: None,
            dynamic_select: None,
            width,
            var_full_width,
            expr_context,
        } if *var_offset == VarOffset::Comb(off)
            && !expr_context.signed
            && width == var_full_width
    );
    if is_target {
        *e = template.clone();
        return 1;
    }
    let mut nrep = 0;
    match e {
        ProtoExpression::Variable { dynamic_select, .. } => {
            if let Some(d) = dynamic_select {
                nrep += replace_all_reads(&mut d.index_expr, off, template);
            }
        }
        ProtoExpression::DynamicVariable {
            index_expr,
            dynamic_select,
            ..
        } => {
            nrep += replace_all_reads(index_expr, off, template);
            if let Some(d) = dynamic_select {
                nrep += replace_all_reads(&mut d.index_expr, off, template);
            }
        }
        ProtoExpression::Unary { x, .. } => nrep += replace_all_reads(x, off, template),
        ProtoExpression::Binary { x, y, .. } => {
            nrep += replace_all_reads(x, off, template);
            nrep += replace_all_reads(y, off, template);
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            nrep += replace_all_reads(cond, off, template);
            nrep += replace_all_reads(true_expr, off, template);
            nrep += replace_all_reads(false_expr, off, template);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (x, _, _) in elements {
                nrep += replace_all_reads(x, off, template);
            }
        }
        ProtoExpression::Value { .. } | ProtoExpression::HierVariable(_) => {}
    }
    nrep
}

fn replace_all_reads_stmt(s: &mut ProtoStatement, off: isize, template: &ProtoExpression) -> usize {
    let mut nrep = 0;
    match s {
        ProtoStatement::Assign(a) => {
            nrep += replace_all_reads(&mut a.expr, off, template);
            if let Some(d) = &mut a.dynamic_select {
                nrep += replace_all_reads(&mut d.index_expr, off, template);
            }
        }
        ProtoStatement::AssignDynamic(a) => {
            nrep += replace_all_reads(&mut a.dst_index_expr, off, template);
            nrep += replace_all_reads(&mut a.expr, off, template);
            if let Some(d) = &mut a.dynamic_select {
                nrep += replace_all_reads(&mut d.index_expr, off, template);
            }
        }
        ProtoStatement::If(x) => {
            if let Some(c) = &mut x.cond {
                nrep += replace_all_reads(c, off, template);
            }
            for t in x.true_side.iter_mut().chain(x.false_side.iter_mut()) {
                nrep += replace_all_reads_stmt(t, off, template);
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &mut x.arms {
                nrep += replace_all_reads(&mut arm.cond, off, template);
                for t in &mut arm.body {
                    nrep += replace_all_reads_stmt(t, off, template);
                }
            }
            for t in &mut x.default {
                nrep += replace_all_reads_stmt(t, off, template);
            }
        }
        _ => {}
    }
    nrep
}

/// Collapse identical non-trivial RHS shapes into a copy of the first def:
/// `x = E; ... y = E;` → `y = x;` when every input of `E` and `x` itself are
/// untouched over `(i, j]`.  The copy then feeds the cheap duplication /
/// single-reader inlining downstream.  Identity is a double structural hash
/// (`ProtoExpression` has `Hash` but no `Eq`; two independent seeds make a
/// collision astronomically unlikely, and a false positive would only fuse
/// two expressions that hash equal twice).
fn collapse_common_rhs(stmts: &mut [ProtoStatement]) -> usize {
    use std::hash::{Hash, Hasher};
    let fold = BigArrayFold::from_statements(stmts.iter());
    let mut write_idxs: HashMap<isize, Vec<usize>> = HashMap::default();
    {
        let mut ins: Vec<VarOffset> = vec![];
        let mut outs: Vec<VarOffset> = vec![];
        for (i, s) in stmts.iter().enumerate() {
            ins.clear();
            outs.clear();
            s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
            for off in outs.drain(..) {
                if let VarOffset::Comb(o) = off {
                    write_idxs.entry(o).or_default().push(i);
                }
            }
        }
    }
    let any_write_in = |o: isize, lo: usize, hi_incl: usize| -> bool {
        write_idxs.get(&o).is_some_and(|v| {
            let p = v.partition_point(|&x| x <= lo);
            v.get(p).is_some_and(|&x| x <= hi_incl)
        })
    };
    // (h1, h2) -> (def idx, dst offset, width) of the first occurrence.
    let mut seen: HashMap<(u64, u64), (usize, isize, usize)> = HashMap::default();
    let mut collapsed = 0usize;
    let mut ins: Vec<VarOffset> = vec![];
    let mut outs: Vec<VarOffset> = vec![];
    // Index loop: the body re-borrows stmts[j] mutably on a hit.
    #[allow(clippy::needless_range_loop)]
    for j in 0..stmts.len() {
        let ProtoStatement::Assign(a) = &stmts[j] else {
            continue;
        };
        let VarOffset::Comb(dst) = a.dst else {
            continue;
        };
        // `write_idxs` is array-wide for a folded element, so the window check
        // below could not tell a rewrite of THIS element from a neighbour's.
        if fold.covers(a.dst) {
            continue;
        }
        if a.select.is_some()
            || a.dynamic_select.is_some()
            || a.rhs_select.is_some()
            || a.dst_width == 0
            || a.dst_width > 64
            || a.expr.width() > 64
            || cheap_rhs(&a.expr)
        {
            continue;
        }
        let key = {
            let mut h1 = std::collections::hash_map::DefaultHasher::new();
            a.expr.hash(&mut h1);
            let mut h2 = std::collections::hash_map::DefaultHasher::new();
            0xa5a5_5a5a_u64.hash(&mut h2);
            a.expr.hash(&mut h2);
            (h1.finish(), h2.finish())
        };
        match seen.get(&key) {
            Some(&(i, x, w)) if w == a.dst_width && x != dst => {
                // Inputs of E (gathered from the LATER def — identical shape)
                // and the first dst must be untouched over (i, j].
                ins.clear();
                outs.clear();
                stmts[j].gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
                // E reading x is a settle back-edge: statement i itself
                // rewrites x between the two evaluations, and the window
                // check below starts past i.
                let stable = !ins.contains(&VarOffset::Comb(x))
                    && !any_write_in(x, i, j)
                    && ins.iter().all(|off| match off {
                        VarOffset::Comb(io) => !any_write_in(*io, i, j),
                        VarOffset::Ff(_) => true,
                    });
                if stable {
                    let w = a.dst_width;
                    let ProtoStatement::Assign(a) = &mut stmts[j] else {
                        unreachable!()
                    };
                    a.expr = ProtoExpression::Variable {
                        var_offset: VarOffset::Comb(x),
                        select: None,
                        dynamic_select: None,
                        width: w,
                        var_full_width: w,
                        expr_context: ExpressionContext {
                            width: w,
                            signed: false,
                        },
                    };
                    collapsed += 1;
                } else {
                    // Window broken: the later occurrence becomes the new
                    // canonical def for subsequent duplicates.
                    seen.insert(key, (j, dst, a.dst_width));
                }
            }
            Some(_) => {}
            None => {
                seen.insert(key, (j, dst, a.dst_width));
            }
        }
    }
    collapsed
}

/// Coalesce disjoint static field stores that fully define a destination
/// word into one whole-width concat assignment.
///
///   `x[3:0] = a; x[7:4] = b;`  →  `x = {b, a};`
///
/// Unlike the single-reader inlining, the destination's storage keeps its
/// exact value (full coverage means the old word contributes nothing), so
/// external visibility poses no constraint.
fn coalesce_field_stores(mut stmts: Vec<ProtoStatement>) -> (Vec<ProtoStatement>, usize) {
    let n = stmts.len();
    let fold = BigArrayFold::from_statements(stmts.iter());
    // Group top-level static-select stores by destination offset.
    struct Group {
        idxs: Vec<usize>,
        w: usize,
        bad: bool,
    }
    let mut groups: HashMap<isize, Group> = HashMap::default();
    for (i, s) in stmts.iter().enumerate() {
        if let ProtoStatement::Assign(a) = s
            && let VarOffset::Comb(o) = a.dst
        {
            let g = groups.entry(o).or_insert(Group {
                idxs: Vec::new(),
                w: a.dst_width,
                bad: false,
            });
            // A field STRADDLING a word boundary is fine — the assembly
            // below clips it into a per-word part.  The `< 64` bound is what
            // keeps every part narrower than a word, so `canonical_wrap`
            // always masks it (it returns the expression bare at 64), which is
            // what stops a dirty RHS from leaking past the part's own width.
            let word_ok = a.dst_width <= 64
                || (word_coalesce_enabled()
                    && a.select
                        .is_some_and(|(hi, lo)| hi.max(lo) - hi.min(lo) < 64));
            if a.select.is_some()
                && a.dynamic_select.is_none()
                && a.rhs_select.is_none()
                && a.dst_width > 0
                && word_ok
                && a.dst_width == g.w
                // Array-wide `read_idxs` / `write_idxs` cannot license moving
                // a folded element's stores.
                && !fold.covers(a.dst)
            {
                g.idxs.push(i);
            } else {
                // A full store / dynamic store / width mismatch on the same
                // offset disqualifies the whole group (mixed-driver word).
                g.bad = true;
            }
        }
    }

    // Reads and writes index lists for the legality window.
    let mut read_idxs: HashMap<isize, Vec<usize>> = HashMap::default();
    let mut write_idxs: HashMap<isize, Vec<usize>> = HashMap::default();
    {
        let mut ins: Vec<VarOffset> = vec![];
        let mut outs: Vec<VarOffset> = vec![];
        for (i, s) in stmts.iter().enumerate() {
            ins.clear();
            outs.clear();
            s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
            for off in ins.drain(..) {
                if let VarOffset::Comb(o) = off {
                    read_idxs.entry(o).or_default().push(i);
                }
            }
            for off in outs.drain(..) {
                if let VarOffset::Comb(o) = off {
                    write_idxs.entry(o).or_default().push(i);
                }
            }
        }
    }
    let any_in_window = |v: Option<&Vec<usize>>, lo: usize, hi: usize| -> bool {
        v.is_some_and(|v| {
            let p = v.partition_point(|&x| x < lo);
            v.get(p).is_some_and(|&x| x <= hi)
        })
    };

    let mut fused = 0usize;
    let mut deleted = vec![false; n];
    // Keyed by the group's last statement index, whose place they take —
    // a wide destination lands several.
    let mut extra: HashMap<usize, Vec<ProtoStatement>> = HashMap::default();
    let mut e_ins: Vec<VarOffset> = vec![];
    let mut e_outs: Vec<VarOffset> = vec![];
    let mut group_offs: Vec<isize> = groups.keys().copied().collect();
    group_offs.sort_unstable();
    for o in group_offs {
        let g = &groups[&o];
        if g.bad || g.idxs.len() < 2 {
            continue;
        }
        // Disjoint ranges covering [0, w) exactly.
        let mut ranges: Vec<(usize, usize, usize)> = Vec::with_capacity(g.idxs.len());
        for &i in &g.idxs {
            let ProtoStatement::Assign(a) = &stmts[i] else {
                unreachable!()
            };
            let (hi, lo) = a.select.unwrap();
            // The select store sign-extends this RHS shape to dst_width
            // before inserting the field; a concat element is used
            // unextended.
            if hi < lo || a.expr.store_sign_extend_from(a.dst_width).is_some() {
                ranges.clear();
                break;
            }
            ranges.push((lo, hi, i));
        }
        if ranges.is_empty() {
            continue;
        }
        ranges.sort_unstable();
        // Overlapping stores never fuse (last-writer-wins semantics would
        // need ordering the concat by statement position, not bit position).
        if ranges.windows(2).any(|p| p[1].0 <= p[0].1) {
            continue;
        }
        let first = *g.idxs.iter().min().unwrap();
        let last = *g.idxs.iter().max().unwrap();
        // Every RHS is evaluated at `last`, so nothing in [first, last] may
        // read the destination (the stores' own RHS included) or rewrite an
        // input.
        if any_in_window(read_idxs.get(&o), first, last) {
            continue;
        }
        let writers = write_idxs.get(&o).map(|v| {
            let p = v.partition_point(|&x| x < first);
            v[p..].iter().take_while(|&&x| x <= last).count()
        });
        if writers != Some(g.idxs.len()) {
            continue;
        }
        let mut input_rewritten = false;
        'legality: for &i in &g.idxs {
            e_ins.clear();
            e_outs.clear();
            stmts[i].gather_variable_offsets_expanded(&fold, &mut e_ins, &mut e_outs);
            for off in &e_ins {
                if let VarOffset::Comb(io) = off
                    && any_in_window(write_idxs.get(io), first, last)
                {
                    input_rewritten = true;
                    break 'legality;
                }
            }
        }
        if input_rewritten {
            continue;
        }

        // A bit-select store CLIPS its RHS to the field, but the interpreter
        // inserts a concat element as-is — an unmasked dirty RHS would leak
        // into neighbouring fields.  gcc folds the mask away on the AOT-C
        // side.
        //
        // Partial coverage fuses too: every uncovered bit range becomes a
        // static select read of the destination itself — the fused statement
        // is then a whole-word RMW (`x = {x[15:10], b, a}`), one load and one
        // store instead of one read-modify-write per field.  The window
        // checks above guarantee nothing else touches the destination across
        // the group, so the self-read observes the pre-group value exactly
        // like each original store's RMW did.
        let old_bits = |hi: usize, lo: usize| -> (Box<ProtoExpression>, usize, usize) {
            let ew = hi - lo + 1;
            (
                Box::new(ProtoExpression::Variable {
                    var_offset: VarOffset::Comb(o),
                    select: Some((hi, lo)),
                    dynamic_select: None,
                    width: ew,
                    var_full_width: g.w,
                    expr_context: ExpressionContext {
                        width: ew,
                        signed: false,
                    },
                }),
                1,
                ew,
            )
        };
        // Fusing one field would reproduce that same field store, and a word
        // no field touches keeps its value by not being stored at all.
        //
        // With a straddling field in the group the per-word membership is no
        // longer free to choose: a field must fuse in EVERY word it touches
        // or in none (its store is deleted whole), so take all touched words.
        let has_straddler = g.w > 64 && ranges.iter().any(|&(lo, hi, _)| lo / 64 != hi / 64);
        let slices: Vec<(usize, usize)> = if g.w <= 64 {
            vec![(g.w - 1, 0)]
        } else if has_straddler {
            (0..g.w.div_ceil(64))
                .map(|word| (((word + 1) * 64).min(g.w) - 1, word * 64))
                .filter(|&(whi, wlo)| ranges.iter().any(|&(lo, hi, _)| lo <= whi && hi >= wlo))
                .collect()
        } else {
            (0..g.w.div_ceil(64))
                .map(|word| (((word + 1) * 64).min(g.w) - 1, word * 64))
                .filter(|&(whi, wlo)| {
                    ranges
                        .iter()
                        .filter(|&&(lo, _, _)| lo >= wlo && lo <= whi)
                        .count()
                        > 1
                })
                .collect()
        };
        if slices.is_empty() {
            continue;
        }
        let ProtoStatement::Assign(last_a) = &stmts[last] else {
            unreachable!()
        };
        let (dst, ff_cur, token) = (last_a.dst, last_a.dst_ff_current_offset, last_a.token);
        let mut stores: Vec<ProtoStatement> = Vec::with_capacity(slices.len());
        for &(shi, slo) in &slices {
            let sw = shi - slo + 1;
            let mut elements: Vec<(Box<ProtoExpression>, usize, usize)> = Vec::new();
            let mut cursor = shi + 1; // exclusive upper edge of the uncovered scan
            for &(lo, hi, i) in ranges
                .iter()
                .rev()
                .filter(|&&(lo, hi, _)| lo <= shi && hi >= slo)
            {
                // A straddler contributes one part per word it touches.
                let phi = hi.min(shi);
                let plo = lo.max(slo);
                if phi + 1 < cursor {
                    elements.push(old_bits(cursor - 1, phi + 1));
                }
                let ProtoStatement::Assign(a) = &mut stmts[i] else {
                    unreachable!()
                };
                // A straddler's RHS is consumed by two slices — clone it and
                // leave the (deleted) statement's expression in place.
                let straddles = lo / 64 != hi / 64;
                let expr = if straddles {
                    a.expr.clone()
                } else {
                    std::mem::replace(
                        &mut a.expr,
                        ProtoExpression::Value {
                            value: Value::new(0, 1, false),
                            width: 1,
                            expr_context: ExpressionContext {
                                width: 1,
                                signed: false,
                            },
                        },
                    )
                };
                let ew = hi - lo + 1;
                let pw = phi - plo + 1;
                let elem = if plo == lo && phi == hi {
                    canonical_wrap(expr, ew)
                } else {
                    // Part of a straddling field: `(expr >> (plo-lo)) & mask(pw)`.
                    // `(plo-lo) + pw <= ew`, so bits of a dirty RHS at or above
                    // `ew` never pass the mask — the part stays canonical.
                    let shifted = if plo > lo {
                        ProtoExpression::Binary {
                            x: Box::new(expr),
                            op: Op::LogicShiftR,
                            y: Box::new(ProtoExpression::Value {
                                value: Value::new((plo - lo) as u64, 32, false),
                                width: 32,
                                expr_context: ExpressionContext {
                                    width: 32,
                                    signed: false,
                                },
                            }),
                            width: ew,
                            expr_context: ExpressionContext {
                                width: ew,
                                signed: false,
                            },
                        }
                    } else {
                        expr
                    };
                    canonical_wrap(shifted, pw)
                };
                elements.push((Box::new(elem), 1, pw));
                cursor = plo;
            }
            if cursor > slo {
                elements.push(old_bits(cursor - 1, slo));
            }
            stores.push(ProtoStatement::Assign(
                crate::ir::statement::ProtoAssignStatement {
                    dst,
                    dst_width: g.w,
                    select: (g.w > 64).then_some((shi, slo)),
                    dynamic_select: None,
                    rhs_select: None,
                    expr: ProtoExpression::Concatenation {
                        elements,
                        width: sw,
                        expr_context: ExpressionContext {
                            width: sw,
                            signed: false,
                        },
                    },
                    dst_ff_current_offset: ff_cur,
                    token,
                },
            ));
        }
        if diag() {
            eprintln!(
                "[comb_fusion] coalesced group: off={o:#x} w={} fields={} stores={} span=[{first},{last}]",
                g.w,
                g.idxs.len(),
                stores.len(),
            );
        }
        extra.insert(last, stores);
        for &(lo, _, i) in &ranges {
            if slices.iter().any(|&(shi, slo)| lo >= slo && lo <= shi) {
                deleted[i] = true;
            }
        }
        fused += 1;
    }

    if fused == 0 {
        return (stmts, 0);
    }
    let mut out = Vec::with_capacity(n);
    for (i, s) in stmts.into_iter().enumerate() {
        if !deleted[i] {
            out.push(s);
        }
        if let Some(v) = extra.remove(&i) {
            out.extend(v);
        }
    }
    (out, fused)
}

/// Inline single-reader comb defs into their readers.  `externals` must hold
/// every comb offset visible outside the comb statement list (event reads,
/// DCE protect, external connects, derived-clock candidates).
///
/// Returns the contracted statements plus the offsets whose defs were
/// consumed: their storage is never written again, so anything comparing
/// raw buffer contents (the dual-run checker) must skip them.
pub fn inline_single_readers(
    mut stmts: Vec<ProtoStatement>,
    events: &HashMap<Event, Vec<ProtoStatement>>,
    externals_extra: &HashSet<VarOffset>,
) -> (Vec<ProtoStatement>, Vec<isize>) {
    // Folds this pass's own view of the very large arrays; the event walk
    // below sees the same offsets the comb statements do.
    let fold = BigArrayFold::from_statements(stmts.iter().chain(events.values().flatten()));

    // -- externals: offsets we must never make disappear.
    let mut externals: HashSet<isize> = HashSet::default();
    for off in externals_extra {
        if let VarOffset::Comb(o) = fold.canon(*off) {
            externals.insert(o);
        }
    }
    {
        let mut ins: Vec<VarOffset> = vec![];
        let mut outs: Vec<VarOffset> = vec![];
        for stmts in events.values() {
            for s in stmts {
                ins.clear();
                outs.clear();
                // EXPANDED: the compact form names base + last element only,
                // so an event reading `arr[idx]` would leave a MIDDLE
                // element's def retirable — the event then reads frozen
                // storage.  (Same hole class as the comb-side census.)
                s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
                for off in ins.drain(..) {
                    if let VarOffset::Comb(o) = off {
                        externals.insert(o);
                    }
                }
                // Event WRITES to comb space (misclassified-FF: ICG enables
                // and other event-written comb) matter too: a comb def of
                // such an offset re-establishes the settled value ON TOP of
                // the event's write, so retiring it lets the event's value
                // survive a settle it must not survive.
                for off in outs.drain(..) {
                    if let VarOffset::Comb(o) = off {
                        externals.insert(o);
                    }
                }
            }
        }
    }

    // Bit-lane structure recovery runs around the coalescing; see
    // `lane_vector` for why that order.
    let mut fused_offsets: Vec<isize> = Vec::new();
    let folded = if lane_vector::fold_enabled() {
        lane_vector::transpose_fold(&mut stmts, &fold, &externals, &mut fused_offsets)
    } else {
        0
    };
    let coalesced = if coalesce_enabled() {
        let (out, fused) = coalesce_field_stores(stmts);
        stmts = out;
        fused
    } else {
        0
    };
    let laned = if lane_vector::merge_enabled() {
        lane_vector::lane_merge(&mut stmts)
    } else {
        0
    };
    let collapsed = if cse_enabled() {
        collapse_common_rhs(&mut stmts)
    } else {
        0
    };

    // -- pass 1: reads / writes / opacity over the top-level statements.
    struct ReadInfo {
        count: usize,
        last_reader: usize,
        /// Reader statement indices, deduplicated, ascending.
        readers: Vec<usize>,
    }
    let mut reads: HashMap<isize, ReadInfo> = HashMap::default();
    let mut writes: HashMap<isize, Vec<usize>> = HashMap::default();
    let mut opaque_read: HashSet<isize> = HashSet::default();
    {
        let mut ins: Vec<VarOffset> = vec![];
        let mut outs: Vec<VarOffset> = vec![];
        for (i, s) in stmts.iter().enumerate() {
            ins.clear();
            outs.clear();
            s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
            let opaque = !rewritable_reader(s);
            for off in ins.drain(..) {
                if let VarOffset::Comb(o) = off {
                    let e = reads.entry(o).or_insert(ReadInfo {
                        count: 0,
                        last_reader: 0,
                        readers: Vec::new(),
                    });
                    e.count += 1;
                    e.last_reader = i;
                    if e.readers.last() != Some(&i) {
                        e.readers.push(i);
                    }
                    if opaque {
                        opaque_read.insert(o);
                    }
                }
            }
            for off in outs.drain(..) {
                if let VarOffset::Comb(o) = off {
                    writes.entry(o).or_default().push(i);
                }
            }
        }
    }

    let n = stmts.len();
    let mut deleted = vec![false; n];
    let mut inlined = 0usize;
    let (mut veto_shape, mut veto_reader, mut veto_redef, mut veto_ext) = (0usize, 0, 0, 0);
    // veto_shape sub-classification (diag only): why a comb Assign fell out
    // of the single-reader candidate shape.
    let (mut vs_sel_def, mut vs_wide, mut vs_multi, mut vs_no_fwd_reader, mut vs_partial_read) =
        (0usize, 0, 0, 0, 0);
    let (mut vs_rhs_sel, mut vs_sext, mut vs_dyn_mismatch) = (0usize, 0, 0);
    let mut e_ins: Vec<VarOffset> = vec![];
    let mut e_outs: Vec<VarOffset> = vec![];

    // -- pass 2a: cheap multi-reader duplication — every reader recomputes
    //    the RHS (see `cheap_rhs`) and the def retires when externally
    //    invisible.  All readers must qualify (no partial application),
    //    each windowed like the single-reader path, READER ITSELF INCLUDED.
    let mut duplicated = 0usize;
    if cheap_enabled() {
        let window_has_write =
            |writes: &HashMap<isize, Vec<usize>>, o: isize, lo: usize, hi: usize| -> bool {
                writes.get(&o).is_some_and(|v| {
                    let p = v.partition_point(|&x| x <= lo);
                    v.get(p).is_some_and(|&x| x < hi)
                })
            };
        for i in 0..n {
            if duplicated >= limit_dup() {
                break;
            }
            let Some((o, w)) = (match &stmts[i] {
                ProtoStatement::Assign(a)
                    if a.select.is_none()
                        && a.dynamic_select.is_none()
                        && a.rhs_select.is_none()
                        && a.dst_width > 0
                        && a.dst_width <= 64
                        && a.expr.width() > 0
                        && a.expr.width() <= 64
                        && cheap_rhs(&a.expr)
                        // Array-wide map entries make neither the reader set
                        // nor the redefinition window precise enough to retire
                        // a folded element's def.
                        && !fold.covers(a.dst) =>
                {
                    match a.dst {
                        VarOffset::Comb(o) if reads.get(&o).is_some_and(|ri| ri.count >= 2) => {
                            Some((o, a.dst_width))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }) else {
                continue;
            };
            let readers = reads[&o].readers.clone();
            if readers.first().is_none_or(|&f| f <= i) {
                continue; // backward reader (settle back-edge) or none
            }
            e_ins.clear();
            e_outs.clear();
            stmts[i].gather_variable_offsets_expanded(&fold, &mut e_ins, &mut e_outs);
            if e_ins.contains(&VarOffset::Comb(o)) {
                continue; // self-read
            }
            let all_ok = readers.iter().all(|&r| {
                !deleted[r]
                    && rewritable_reader(&stmts[r])
                    && !window_has_write(&writes, o, i, r + 1)
                    && e_ins.iter().all(|off| match off {
                        VarOffset::Comb(io) => !window_has_write(&writes, *io, i, r + 1),
                        VarOffset::Ff(_) => true,
                    })
            });
            if !all_ok {
                veto_redef += 1;
                continue;
            }
            // The full-load count must equal the gather-based read count:
            // a mismatch means some reader reaches this offset through a
            // dynamic index `count_reads` cannot see, and would silently
            // keep reading the retired storage.
            let (mut tf, mut to) = (0usize, 0usize);
            for &r in &readers {
                count_reads_stmt(&stmts[r], o, &mut tf, &mut to);
            }
            if to != 0 || tf == 0 || tf != reads[&o].count {
                veto_shape += 1;
                vs_dyn_mismatch += 1;
                continue;
            }
            if diag() && duplicated + 1 == limit_dup() {
                eprintln!(
                    "[comb_fusion] LAST dup #{n}: off={o:#x} w={w} def stmt[{i}] = {:?}",
                    stmts[i],
                    n = duplicated + 1,
                );
                for &r in &readers {
                    eprintln!("[comb_fusion] dup reader stmt[{r}] = {:?}", stmts[r]);
                }
                // The position-independence window in full: every writer of
                // every template input, so a (i, r] intruder is visible.
                for off in &e_ins {
                    if let VarOffset::Comb(io) = off {
                        eprintln!(
                            "[comb_fusion] dup input off={io:#x} comb writers={:?} \
                             readers={:?}",
                            writes.get(io),
                            reads.get(io).map(|ri| &ri.readers),
                        );
                    } else {
                        eprintln!("[comb_fusion] dup input {off:?} (ff)");
                    }
                }
                eprintln!(
                    "[comb_fusion] dup dst off={o:#x} all writers={:?} externals={} opaque={}",
                    writes.get(&o),
                    externals.contains(&o),
                    opaque_read.contains(&o),
                );
            }
            let ProtoStatement::Assign(a) = &stmts[i] else {
                unreachable!()
            };
            let template = if a.expr.width() <= w {
                a.expr.clone()
            } else {
                canonical_wrap(a.expr.clone(), w)
            };
            let mut total = 0usize;
            for &r in &readers {
                total += replace_all_reads_stmt(&mut stmts[r], o, &template);
            }
            debug_assert_eq!(
                total, tf,
                "cheap duplication replaced a different read count"
            );
            let keep = cheap_keep(
                std::env::var("VERYL_COMB_FUSION_CHEAP_KEEP")
                    .ok()
                    .as_deref(),
                duplicated + 1 == limit_dup(),
            );
            if !keep && !externals.contains(&o) && !opaque_read.contains(&o) {
                deleted[i] = true;
                fused_offsets.push(o);
            }
            duplicated += 1;
            // The template's reads now occur at every reader's position.
            let last = readers.last().copied().unwrap_or(0);
            for off in &e_ins {
                if let VarOffset::Comb(io) = off
                    && let Some(ri) = reads.get_mut(io)
                {
                    ri.count += total;
                    if ri.last_reader < last {
                        ri.last_reader = last;
                    }
                    let tail = ri.readers.last().copied();
                    ri.readers.extend(
                        readers
                            .iter()
                            .copied()
                            .filter(|&r| tail.is_none_or(|l| r > l)),
                    );
                }
            }
        }
    }

    // -- pass 2b: greedy front-to-back single-reader inlining.  Chains
    //    compose naturally: an earlier def folded into its reader travels
    //    with it when the reader is itself inlined later (its RHS is
    //    re-gathered fresh).
    for i in 0..n {
        if inlined >= limit() {
            break;
        }
        // Candidate shape.
        let Some((o, w, r_idx)) = (match &stmts[i] {
            ProtoStatement::Assign(a)
                if a.select.is_none()
                    && a.dynamic_select.is_none()
                    && a.rhs_select.is_none()
                    && a.dst_width > 0
                    && a.dst_width <= 64
                    && a.expr.width() > 0
                    && a.expr.width() <= 64
                    // Inlining retires an unsigned variable load, so a signed
                    // leaf put in its place would be sign-extended by a wider
                    // reader.
                    && !a.expr.is_signed_store_leaf()
                    // See the duplication pass: a folded element cannot be
                    // told apart from its neighbours in `reads` / `writes`.
                    && !fold.covers(a.dst) =>
            {
                match a.dst {
                    VarOffset::Comb(o) => {
                        if externals.contains(&o) {
                            veto_ext += 1;
                            None
                        } else if opaque_read.contains(&o) {
                            veto_reader += 1;
                            None
                        } else {
                            match reads.get(&o) {
                                Some(ri) if ri.count == 1 && ri.last_reader > i => {
                                    Some((o, a.dst_width, ri.last_reader))
                                }
                                _ => None,
                            }
                        }
                    }
                    VarOffset::Ff(_) => None,
                }
            }
            _ => None,
        }) else {
            if let ProtoStatement::Assign(a) = &stmts[i]
                && !a.dst.is_ff()
            {
                // Already counted as ext/reader inside the candidate match —
                // do not double-count them into the shape buckets.
                let counted_elsewhere = matches!(a.dst, VarOffset::Comb(o)
                    if a.select.is_none()
                        && a.dynamic_select.is_none()
                        && a.rhs_select.is_none()
                        && a.dst_width > 0
                        && a.dst_width <= 64
                        && a.expr.width() > 0
                        && a.expr.width() <= 64
                        && !a.expr.is_signed_store_leaf()
                        && (externals.contains(&o) || opaque_read.contains(&o)));
                if !counted_elsewhere {
                    veto_shape += 1;
                    if a.select.is_some() || a.dynamic_select.is_some() {
                        vs_sel_def += 1;
                    } else if a.rhs_select.is_some() {
                        vs_rhs_sel += 1;
                    } else if a.dst_width > 64 || a.expr.width() > 64 {
                        vs_wide += 1;
                    } else if a.expr.is_signed_store_leaf() {
                        vs_sext += 1;
                    } else if let VarOffset::Comb(o) = a.dst {
                        match reads.get(&o) {
                            Some(ri) if ri.count > 1 => vs_multi += 1,
                            _ => vs_no_fwd_reader += 1,
                        }
                    } else {
                        vs_no_fwd_reader += 1;
                    }
                }
            }
            continue;
        };
        if deleted[r_idx] || !rewritable_reader(&stmts[r_idx]) {
            veto_reader += 1;
            continue;
        }
        // The single read must be a full-width scalar load.
        let (mut full, mut other) = (0usize, 0usize);
        count_reads_stmt(&stmts[r_idx], o, &mut full, &mut other);
        if full != 1 || other != 0 {
            veto_shape += 1;
            vs_partial_read += 1;
            continue;
        }
        // Position independence: nothing between def and reader — nor the
        // READER ITSELF — may rewrite an RHS input or the def's own offset,
        // and the RHS must not read its own previous-pass value.  The
        // reader-itself case is load-bearing: `x = 0; case (s) { arm: x =
        // x | a; }` reads x once, but deleting the init would leave x
        // holding its previous-cycle value on the arms that do not write it
        // (and a reader writing an RHS input makes the evaluation order
        // within the statement significant).  (Deleted defs keep their
        // entries in `writes`; that only vetoes conservatively.)
        e_ins.clear();
        e_outs.clear();
        stmts[i].gather_variable_offsets_expanded(&fold, &mut e_ins, &mut e_outs);
        let window_has_write = |o: isize, lo: usize, hi: usize| -> bool {
            writes.get(&o).is_some_and(|v| {
                let p = v.partition_point(|&x| x <= lo);
                v.get(p).is_some_and(|&x| x < hi)
            })
        };
        let mut self_read = false;
        let mut redef = false;
        for off in &e_ins {
            if let VarOffset::Comb(io) = off {
                if *io == o {
                    self_read = true;
                    break;
                }
                if window_has_write(*io, i, r_idx + 1) {
                    redef = true;
                    break;
                }
            }
        }
        if self_read || redef || window_has_write(o, i, r_idx + 1) {
            veto_redef += 1;
            continue;
        }

        if diag() && inlined + 1 == limit() {
            eprintln!(
                "[comb_fusion] LAST inline #{n}: off={o:#x} w={w} def stmt[{i}] = {:?}",
                stmts[i],
                n = inlined + 1,
            );
            eprintln!("[comb_fusion] reader stmt[{r_idx}] = {:?}", stmts[r_idx],);
        }
        // Commit: move the RHS into the reader, canonically masked.
        let ProtoStatement::Assign(a) = &mut stmts[i] else {
            unreachable!()
        };
        let expr = std::mem::replace(
            &mut a.expr,
            ProtoExpression::Value {
                value: Value::new(0, 1, false),
                width: 1,
                expr_context: ExpressionContext {
                    width: 1,
                    signed: false,
                },
            },
        );
        let mut repl = Some(canonical_wrap(expr, w));
        let (l, r) = stmts.split_at_mut(r_idx);
        let _ = l; // reader is strictly later
        let done = replace_read_stmt(&mut r[0], o, &mut repl);
        debug_assert!(done, "single-reader read not found at substitution time");
        deleted[i] = true;
        fused_offsets.push(o);
        inlined += 1;
        // The moved expression's reads now occur at the reader's position;
        // keep `last_reader` fresh for later single-reader decisions.
        for off in &e_ins {
            if let VarOffset::Comb(io) = off
                && let Some(ri) = reads.get_mut(io)
                && ri.last_reader < r_idx
            {
                ri.last_reader = r_idx;
            }
        }
    }

    if diag() {
        eprintln!(
            "[comb_fusion] stmts={} folded={} coalesced={} laned={} cse={} dup={} inlined={} veto: shape={} reader={} redef={} external={}",
            n,
            folded,
            coalesced,
            laned,
            collapsed,
            duplicated,
            inlined,
            veto_shape,
            veto_reader,
            veto_redef,
            veto_ext,
        );
        eprintln!(
            "[comb_fusion] shape breakdown: store_sel={vs_sel_def} rhs_sel={vs_rhs_sel} wide={vs_wide} sext={vs_sext} multi_reader={vs_multi} no_fwd_reader={vs_no_fwd_reader} partial_read={vs_partial_read} dyn_mismatch={vs_dyn_mismatch}",
        );
        // Field-store census: group size per destination (how many static
        // select stores hit the same comb offset) and that offset's reader
        // count — decides bare-RMW normalization vs group fusion.
        {
            use std::collections::HashMap as Map;
            let mut groups: Map<isize, usize> = Map::new();
            let mut narrow = 0usize;
            let mut wide_sel = 0usize;
            for s in stmts.iter() {
                if let ProtoStatement::Assign(a) = s
                    && a.select.is_some()
                    && a.dynamic_select.is_none()
                    && let VarOffset::Comb(o) = a.dst
                {
                    *groups.entry(o).or_default() += 1;
                    if a.dst_width <= 64 {
                        narrow += 1;
                    } else {
                        wide_sel += 1;
                    }
                }
            }
            let mut size_hist: Map<usize, usize> = Map::new();
            let mut single_dst_single_reader = 0usize;
            let mut single_dst_multi_reader = 0usize;
            for (&o, &cnt) in &groups {
                *size_hist.entry(cnt).or_default() += 1;
                if cnt == 1 {
                    match reads.get(&o).map(|ri| ri.count).unwrap_or(0) {
                        0 | 1 => single_dst_single_reader += 1,
                        _ => single_dst_multi_reader += 1,
                    }
                }
            }
            let mut hist: Vec<_> = size_hist.into_iter().collect();
            hist.sort();
            eprintln!(
                "[comb_fusion] field-store census: dsts={} narrow={narrow} wide={wide_sel} group-size-hist={hist:?} singles: reader<=1 {single_dst_single_reader} multi {single_dst_multi_reader}",
                groups.len(),
            );
        }
    }
    if !deleted.iter().any(|&d| d) {
        return (stmts, fused_offsets);
    }
    let mut out = Vec::with_capacity(n);
    for (i, s) in stmts.into_iter().enumerate() {
        if !deleted[i] {
            out.push(s);
        }
    }
    (out, fused_offsets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::statement::ProtoAssignStatement;

    fn ctx(width: usize) -> ExpressionContext {
        ExpressionContext {
            width,
            signed: false,
        }
    }

    fn var(off: isize, w: usize) -> ProtoExpression {
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
            token: Default::default(),
        })
    }

    fn binary(op: Op, x: ProtoExpression, y: ProtoExpression, w: usize) -> ProtoExpression {
        ProtoExpression::Binary {
            x: Box::new(x),
            op,
            y: Box::new(y),
            width: w,
            expr_context: ctx(w),
        }
    }

    fn run(stmts: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
        inline_single_readers(stmts, &HashMap::default(), &HashSet::default()).0
    }

    fn reads_of(s: &ProtoStatement, off: isize) -> (usize, usize) {
        let (mut f, mut o) = (0, 0);
        count_reads_stmt(s, off, &mut f, &mut o);
        (f, o)
    }

    #[test]
    fn inlines_a_single_reader_def() {
        // 0x0 = 0x100 & 0x108;  0x8 = 0x0 | 0x110  =>  one statement.
        let stmts = vec![
            assign(0x0, 8, binary(Op::BitAnd, var(0x100, 8), var(0x108, 8), 8)),
            assign(0x8, 8, binary(Op::BitOr, var(0x0, 8), var(0x110, 8), 8)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 1);
        // The survivor no longer reads 0x0 but reads both original inputs.
        assert_eq!(reads_of(&out[0], 0x0), (0, 0));
        assert_eq!(reads_of(&out[0], 0x100), (1, 0));
        assert_eq!(reads_of(&out[0], 0x108), (1, 0));
    }

    #[test]
    fn keeps_a_def_with_a_signed_reader() {
        // The reader loads 0x0 in a signed context (sign-extends the
        // canonical storage); substituting the unsigned producer expression
        // would drop the extension.
        let signed_read = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x0),
            select: None,
            dynamic_select: None,
            width: 8,
            var_full_width: 8,
            expr_context: ExpressionContext {
                width: 8,
                signed: true,
            },
        };
        let stmts = vec![
            assign(0x0, 8, binary(Op::Sub, var(0x100, 8), var(0x108, 8), 8)),
            assign(0x8, 8, signed_read),
        ];
        assert_eq!(run(stmts).len(), 2);
    }

    #[test]
    fn keeps_a_def_with_a_width_mismatched_reader() {
        // A narrower bare load reinterprets the storage; it is not a
        // substitutable full-width read.
        let narrow_read = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x0),
            select: None,
            dynamic_select: None,
            width: 4,
            var_full_width: 8,
            expr_context: ctx(4),
        };
        let stmts = vec![assign(0x0, 8, var(0x100, 8)), assign(0x8, 8, narrow_read)];
        assert_eq!(run(stmts).len(), 2);
    }

    #[test]
    fn keeps_a_def_read_through_a_dynamic_index() {
        // 0x8 is the MIDDLE element of a 3-element array read via a runtime
        // index — invisible to the compact base+last gather; the expanded
        // gather must count that reader so the def survives (a retired def
        // would leave the dynamic read on stale storage).
        let dyn_read = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0x0),
            stride: 8,
            element_native_bytes: 8,
            index_expr: Box::new(var(0x110, 8)),
            num_elements: 3,
            select: None,
            dynamic_select: None,
            width: 8,
            expr_context: ctx(8),
        };
        let stmts = vec![
            assign(0x8, 8, var(0x100, 8)), // def of the middle element
            assign(0x200, 8, var(0x8, 8)), // the visible direct reader
            assign(0x208, 8, dyn_read),    // the hidden dynamic reader
        ];
        assert_eq!(
            run(stmts).len(),
            3,
            "the def must not fuse into the direct reader"
        );
    }

    #[test]
    fn keeps_a_def_an_event_writes() {
        // An event also writes 0x0: the comb def re-establishes the settled
        // value on top of the event's write, so it must not retire.
        let stmts = vec![assign(0x0, 8, var(0x100, 8)), assign(0x8, 8, var(0x0, 8))];
        let mut events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        events.insert(Event::Initial, vec![assign(0x0, 8, var(0x200, 8))]);
        let out = inline_single_readers(stmts, &events, &HashSet::default()).0;
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn cheap_two_reader_copy_now_duplicates() {
        // Under the cheap-duplication policy a two-reader COPY retires (both
        // readers read the source directly); an expensive two-reader def is
        // covered by `expensive_multi_reader_def_is_kept`.
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x8, 8, var(0x0, 8)),
            assign(0x10, 8, var(0x0, 8)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 2);
        for s in &out {
            assert_eq!(reads_of(s, 0x0), (0, 0));
        }
    }

    #[test]
    fn keeps_an_event_read_def() {
        let stmts = vec![assign(0x0, 8, var(0x100, 8)), assign(0x8, 8, var(0x0, 8))];
        let mut events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        events.insert(Event::Initial, vec![assign(0x200, 8, var(0x0, 8))]);
        let out = inline_single_readers(stmts, &events, &HashSet::default()).0;
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn keeps_a_protected_def() {
        let stmts = vec![assign(0x0, 8, var(0x100, 8)), assign(0x8, 8, var(0x0, 8))];
        let mut protect: HashSet<VarOffset> = HashSet::default();
        protect.insert(VarOffset::Comb(0x0));
        let out = inline_single_readers(stmts, &HashMap::default(), &protect).0;
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn vetoes_when_an_input_is_rewritten_between() {
        // 0x0 reads 0x100; 0x100 is rewritten before the reader runs.
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x100, 8, var(0x108, 8)),
            assign(0x8, 8, var(0x0, 8)),
        ];
        assert_eq!(run(stmts).len(), 3);
    }

    #[test]
    fn vetoes_a_self_reading_def() {
        // 0x0 = 0x0 + 1 reads its own previous-pass value.
        let stmts = vec![
            assign(0x0, 8, binary(Op::Add, var(0x0, 8), var(0x100, 8), 8)),
            assign(0x8, 8, var(0x0, 8)),
        ];
        assert_eq!(run(stmts).len(), 2);
    }

    #[test]
    fn vetoes_a_selected_read() {
        let sel_read = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x0),
            select: Some((3, 0)),
            dynamic_select: None,
            width: 4,
            var_full_width: 8,
            expr_context: ctx(4),
        };
        let stmts = vec![assign(0x0, 8, var(0x100, 8)), assign(0x8, 4, sel_read)];
        assert_eq!(run(stmts).len(), 2);
    }

    #[test]
    fn vetoes_a_for_reader() {
        use crate::ir::statement::{ProtoForBound, ProtoForRange, ProtoForStatement};
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            ProtoStatement::For(ProtoForStatement {
                var_offset: VarOffset::Comb(0x50),
                var_width: 32,
                var_native_bytes: 4,
                var_signed: false,
                token: veryl_parser::token_range::TokenRange::default(),
                range: ProtoForRange::Forward {
                    start: ProtoForBound::Const(0),
                    end: ProtoForBound::Const(4),
                    inclusive: false,
                    step: 1,
                },
                body: vec![assign(0x8, 8, var(0x0, 8))],
            }),
        ];
        assert_eq!(run(stmts).len(), 2);
    }

    #[test]
    fn vetoes_a_reader_that_also_writes_the_offset() {
        // `x = A; if (c) { x = x | B }` reads x exactly once, but deleting
        // the init would leave x stale on the not-taken path (the
        // case-default init pattern of a reference CPU core).
        use crate::ir::statement::ProtoIfStatement;
        let rmw = ProtoStatement::If(ProtoIfStatement {
            cond: Some(var(0x200, 1)),
            true_side: vec![assign(
                0x0,
                8,
                binary(Op::BitOr, var(0x0, 8), var(0x108, 8), 8),
            )],
            false_side: vec![],
        });
        let stmts = vec![assign(0x0, 8, var(0x100, 8)), rmw];
        assert_eq!(run(stmts).len(), 2);
    }

    #[test]
    fn vetoes_a_reader_that_writes_an_rhs_input() {
        // The reader writes 0x100 (an input of the def) inside itself; the
        // within-statement evaluation order would become significant.
        use crate::ir::statement::ProtoIfStatement;
        let reader = ProtoStatement::If(ProtoIfStatement {
            cond: Some(var(0x0, 8)),
            true_side: vec![assign(0x100, 8, var(0x108, 8))],
            false_side: vec![],
        });
        let stmts = vec![assign(0x0, 8, var(0x100, 8)), reader];
        assert_eq!(run(stmts).len(), 2);
    }

    fn assign_sel(
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
            token: Default::default(),
        })
    }

    #[test]
    fn coalesces_full_coverage_field_stores() {
        // x[3:0] = a; x[7:4] = b  =>  x = {b, a} (one full store, read intact).
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            assign_sel(0x0, 8, 7, 4, var(0x108, 4)),
            assign(0x8, 8, var(0x0, 8)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 1);
        assert_eq!(out.len(), 2);
        let ProtoStatement::Assign(a) = &out[0] else {
            panic!()
        };
        assert!(a.select.is_none());
        let ProtoExpression::Concatenation {
            elements, width, ..
        } = &a.expr
        else {
            panic!("expected concat, got {:?}", a.expr)
        };
        assert_eq!(*width, 8);
        assert_eq!(elements.len(), 2);
        // High-to-low: first element is the [7:4] store's RHS (reads 0x108).
        let (mut f, mut o2) = (0, 0);
        count_reads(&elements[0].0, 0x108, &mut f, &mut o2);
        assert_eq!(f, 1);
    }

    #[test]
    fn coalesce_masks_a_dirty_element() {
        // x[3:0] = a + b (computed wider than the slot): the concat element
        // must carry the canonical mask, and both slots must hold their own
        // RHS at the declared width.
        let stmts = vec![
            assign_sel(
                0x0,
                8,
                3,
                0,
                binary(Op::Add, var(0x100, 4), var(0x108, 4), 4),
            ),
            assign_sel(0x0, 8, 7, 4, var(0x110, 4)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 1);
        let ProtoStatement::Assign(a) = &out[0] else {
            panic!()
        };
        let ProtoExpression::Concatenation { elements, .. } = &a.expr else {
            panic!()
        };
        assert_eq!(elements.len(), 2);
        assert_eq!((elements[0].1, elements[0].2), (1, 4));
        assert_eq!((elements[1].1, elements[1].2), (1, 4));
        // High slot = [7:4]'s RHS; low slot = the masked Add.
        let (mut f, mut o2) = (0, 0);
        count_reads(&elements[0].0, 0x110, &mut f, &mut o2);
        assert_eq!((f, o2), (1, 0));
        let ProtoExpression::Binary {
            op: Op::BitAnd, y, ..
        } = &*elements[1].0
        else {
            panic!("low element must be masked: {:?}", elements[1].0)
        };
        let ProtoExpression::Value { value, .. } = &**y else {
            panic!()
        };
        assert_eq!(value.payload_u64(), 0xf);
    }

    #[test]
    fn coalesce_rejects_a_sign_extending_element() {
        // s (signed, 2 bits) into [7:4]: the select store sign-extends it to
        // the destination width; a concat element would not.
        let signed_leaf = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x100),
            select: None,
            dynamic_select: None,
            width: 2,
            var_full_width: 2,
            expr_context: ExpressionContext {
                width: 2,
                signed: true,
            },
        };
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x108, 4)),
            assign_sel(0x0, 8, 7, 4, signed_leaf),
        ];
        assert_eq!(coalesce_field_stores(stmts).1, 0);
    }

    #[test]
    fn inline_declines_a_sign_extending_def() {
        // The full store sign-extends the bare narrow signed RHS to the def
        // width; the substituted expression would be used unextended.
        let signed_leaf = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x100),
            select: None,
            dynamic_select: None,
            width: 2,
            var_full_width: 2,
            expr_context: ExpressionContext {
                width: 2,
                signed: true,
            },
        };
        let stmts = vec![assign(0x0, 8, signed_leaf), assign(0x8, 8, var(0x0, 8))];
        assert_eq!(run(stmts).len(), 2);
    }

    #[test]
    fn coalesce_rejects_a_dynamic_reader_of_the_word_in_the_window() {
        // 0x108 is the middle element of a 3-element array; arr[idx] between
        // the two field stores reads it — invisible to the compact gather,
        // so the window index must be built from the expanded form.
        let dyn_read = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0x100),
            stride: 8,
            element_native_bytes: 8,
            index_expr: Box::new(var(0x200, 8)),
            num_elements: 3,
            select: None,
            dynamic_select: None,
            width: 8,
            expr_context: ctx(8),
        };
        let stmts = vec![
            assign_sel(0x108, 8, 3, 0, var(0x210, 4)),
            assign(0x218, 8, dyn_read),
            assign_sel(0x108, 8, 7, 4, var(0x220, 4)),
        ];
        assert_eq!(coalesce_field_stores(stmts).1, 0);
    }

    #[test]
    fn coalesce_rejects_a_conditional_writer_in_the_window() {
        // An If writing the word between the stores is a foreign writer the
        // writers count must catch.
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            ProtoStatement::If(crate::ir::statement::ProtoIfStatement {
                cond: Some(var(0x200, 1)),
                true_side: vec![assign(0x0, 8, var(0x208, 8))],
                false_side: vec![],
            }),
            assign_sel(0x0, 8, 7, 4, var(0x108, 4)),
        ];
        assert_eq!(coalesce_field_stores(stmts).1, 0);
    }

    #[test]
    fn coalesce_rejects_a_width_mismatched_group() {
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            assign_sel(0x0, 16, 7, 4, var(0x108, 4)),
        ];
        assert_eq!(coalesce_field_stores(stmts).1, 0);
    }

    #[test]
    fn coalesce_fuses_partial_coverage_with_self_select() {
        // Only [3:0] and [6:4]: bit 7 keeps the old value, read back as a
        // select of the destination itself — x = {x[7:7], b, a}.
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            assign_sel(0x0, 8, 6, 4, var(0x108, 3)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 1);
        assert_eq!(out.len(), 1);
        let ProtoStatement::Assign(a) = &out[0] else {
            panic!()
        };
        assert!(a.select.is_none());
        let ProtoExpression::Concatenation {
            elements, width, ..
        } = &a.expr
        else {
            panic!("expected concat, got {:?}", a.expr)
        };
        assert_eq!(*width, 8);
        assert_eq!(elements.len(), 3);
        // The top element is the old bit 7 of the destination itself.
        let ProtoExpression::Variable {
            var_offset, select, ..
        } = &*elements[0].0
        else {
            panic!("top element must be the old-bits self select")
        };
        assert_eq!(*var_offset, VarOffset::Comb(0x0));
        assert_eq!(*select, Some((7, 7)));
    }

    #[test]
    fn coalesce_leaves_word_sized_fields_of_a_wide_destination_alone() {
        // Each field already is a whole word, so the fused store would be
        // that same store — nothing to gain.
        let stmts = vec![
            assign_sel(0x0, 192, 63, 0, var(0x100, 64)),
            assign_sel(0x0, 192, 127, 64, var(0x108, 64)),
            assign_sel(0x0, 192, 191, 128, var(0x110, 64)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 0);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn coalesce_packs_a_wide_destination_one_word_at_a_time() {
        // A vector written bit by bit becomes one store per word, not one
        // concat as wide as the vector.
        let w = 130;
        let stmts: Vec<ProtoStatement> = (0..w)
            .map(|b| assign_sel(0x0, w, b, b, var(0x100 + b as isize * 8, 1)))
            .collect();
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 1);
        assert_eq!(out.len(), 3, "one store per written word");
        let mut seen: Vec<(Option<(usize, usize)>, usize)> = out
            .iter()
            .map(|s| {
                let ProtoStatement::Assign(a) = s else {
                    panic!("expected an Assign")
                };
                let ProtoExpression::Concatenation { elements, .. } = &a.expr else {
                    panic!("expected a concatenation")
                };
                assert_eq!(a.dst_width, w);
                (a.select, elements.len())
            })
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                (Some((63, 0)), 64),
                (Some((127, 64)), 64),
                (Some((129, 128)), 2),
            ]
        );
    }

    #[test]
    fn coalesce_stores_a_word_holding_only_a_straddler() {
        // `[95:60]` is the only field reaching word 1.  A per-word membership
        // rule keyed on where fields START would never store that word, while
        // the field's own store is deleted — leaving [95:64] undriven.
        let stmts = vec![
            assign_sel(0x0, 96, 59, 0, var(0x100, 60)),
            assign_sel(0x0, 96, 95, 60, var(0x108, 36)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 1);
        let mut words: Vec<Option<(usize, usize)>> = out
            .iter()
            .map(|s| match s {
                ProtoStatement::Assign(a) => a.select,
                _ => panic!("expected an Assign"),
            })
            .collect();
        words.sort();
        assert_eq!(
            words,
            vec![Some((63, 0)), Some((95, 64))],
            "both words the straddler touches must be stored"
        );
    }

    #[test]
    fn coalesce_splits_a_word_straddling_field() {
        // [69:60] crosses the word boundary: its low 4 bits belong to word 0
        // and its high 6 bits to word 1.  The group fuses into one store per
        // word, with the straddler clipped into a shifted part on each side.
        let stmts = vec![
            assign_sel(0x0, 96, 59, 0, var(0x100, 60)),
            assign_sel(0x0, 96, 69, 60, var(0x108, 10)),
            assign_sel(0x0, 96, 95, 70, var(0x110, 26)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 1);
        assert_eq!(out.len(), 2, "one store per word, all field stores gone");
        type StoreShape = (Option<(usize, usize)>, Vec<usize>);
        let mut seen: Vec<StoreShape> = out
            .iter()
            .map(|s| {
                let ProtoStatement::Assign(a) = s else {
                    panic!("expected an Assign")
                };
                let ProtoExpression::Concatenation { elements, .. } = &a.expr else {
                    panic!("expected a concatenation")
                };
                assert_eq!(a.dst_width, 96);
                (a.select, elements.iter().map(|(_, _, w)| *w).collect())
            })
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                // word 0: straddler bits [63:60] then the [59:0] field
                (Some((63, 0)), vec![4, 60]),
                // word 1: the [95:70] field then straddler bits [69:64]
                (Some((95, 64)), vec![26, 6]),
            ]
        );
        // The high part must read the straddler's RHS shifted right by 4.
        let word1 = out
            .iter()
            .find_map(|s| match s {
                ProtoStatement::Assign(a) if a.select == Some((95, 64)) => Some(a),
                _ => None,
            })
            .unwrap();
        let ProtoExpression::Concatenation { elements, .. } = &word1.expr else {
            unreachable!()
        };
        // elements[1] is `(rhs >> 4) & 0x3f`
        let ProtoExpression::Binary {
            op: Op::BitAnd, x, ..
        } = elements[1].0.as_ref()
        else {
            panic!("straddler part must be masked")
        };
        let ProtoExpression::Binary {
            op: Op::LogicShiftR,
            y,
            ..
        } = x.as_ref()
        else {
            panic!("straddler high part must be shifted")
        };
        let ProtoExpression::Value { value, .. } = y.as_ref() else {
            panic!("shift amount must be a constant")
        };
        assert_eq!(value.to_usize(), Some(4));
    }

    #[test]
    fn coalesce_rejects_a_single_partial_store() {
        // One lone field store never fuses (nothing to combine with).
        let stmts = vec![assign_sel(0x0, 8, 3, 0, var(0x100, 4))];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 0);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn coalesce_rejects_a_read_between_stores() {
        // The intermediate read observes the half-written word.
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            assign(0x8, 8, var(0x0, 8)),
            assign_sel(0x0, 8, 7, 4, var(0x108, 4)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 0);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn coalesce_rejects_input_rewritten_between() {
        // 0x100 (input of the first store) is rewritten before the last
        // store, where the fused RHS would be evaluated.
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            assign(0x100, 8, var(0x110, 8)),
            assign_sel(0x0, 8, 7, 4, var(0x108, 4)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 0);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn coalesce_rejects_overlapping_or_mixed_drivers() {
        // Overlap [3:0]+[4:2] never fuses; a full store on the same word
        // disqualifies its group too.
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            assign_sel(0x0, 8, 4, 2, var(0x108, 3)),
            assign_sel(0x10, 8, 3, 0, var(0x100, 4)),
            assign(0x10, 8, var(0x110, 8)),
            assign_sel(0x10, 8, 7, 4, var(0x108, 4)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 0);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn coalesced_store_feeds_single_reader_inlining() {
        // After coalescing, the full-width def has one reader and inlines.
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            assign_sel(0x0, 8, 7, 4, var(0x108, 4)),
            assign(0x8, 8, var(0x0, 8)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 1);
        assert_eq!(reads_of(&out[0], 0x0), (0, 0));
    }

    #[test]
    fn cheap_duplication_retires_a_multi_reader_copy() {
        // x = y (cheap); two readers both fold to y directly, x retires.
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x8, 8, binary(Op::BitAnd, var(0x0, 8), var(0x108, 8), 8)),
            assign(0x10, 8, binary(Op::BitOr, var(0x0, 8), var(0x110, 8), 8)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 2);
        for s in &out {
            assert_eq!(reads_of(s, 0x0), (0, 0));
        }
    }

    #[test]
    fn cheap_duplication_keeps_an_external_def_but_rewrites_readers() {
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x8, 8, var(0x0, 8)),
            assign(0x10, 8, var(0x0, 8)),
        ];
        let mut protect: HashSet<VarOffset> = HashSet::default();
        protect.insert(VarOffset::Comb(0x0));
        let (out, _) = inline_single_readers(stmts, &HashMap::default(), &protect);
        // The def survives (external), but both readers now read 0x100.
        assert_eq!(out.len(), 3);
        assert_eq!(reads_of(&out[1], 0x0), (0, 0));
        assert_eq!(reads_of(&out[2], 0x0), (0, 0));
        assert_eq!(reads_of(&out[1], 0x100), (1, 0));
    }

    #[test]
    fn cheap_duplication_vetoes_when_the_source_is_rewritten() {
        // y (=0x100) changes between the two readers: no duplication.
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x8, 8, var(0x0, 8)),
            assign(0x100, 8, var(0x110, 8)),
            assign(0x10, 8, var(0x0, 8)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 4);
        assert_eq!(reads_of(&out[3], 0x0), (1, 0));
    }

    #[test]
    fn expensive_multi_reader_def_is_kept() {
        // An add is not cheap; two readers keep the store+loads.
        let stmts = vec![
            assign(0x0, 8, binary(Op::Add, var(0x100, 8), var(0x108, 8), 8)),
            assign(0x8, 8, var(0x0, 8)),
            assign(0x10, 8, var(0x0, 8)),
        ];
        // The two copy-readers duplicate off the def's DST (cheap copies of
        // 0x0), but the add def itself must survive with its store.
        let out = run(stmts);
        assert!(
            out.iter()
                .any(|s| matches!(s, ProtoStatement::Assign(a) if a.dst == VarOffset::Comb(0x0))),
            "expensive def must keep its store"
        );
    }

    #[test]
    fn cse_vetoes_a_back_edge_first_def() {
        // x = x + a (settle back-edge): statement i rewrites its own input,
        // so the later identical RHS evaluates differently.
        let stmts = vec![
            assign(0x0, 8, binary(Op::Add, var(0x0, 8), var(0x100, 8), 8)),
            assign(0x8, 8, binary(Op::Add, var(0x0, 8), var(0x100, 8), 8)),
        ];
        let mut s2 = stmts.clone();
        assert_eq!(collapse_common_rhs(&mut s2), 0);
    }

    #[test]
    fn cse_vetoes_when_the_first_dst_is_rewritten_between() {
        // A third statement rewrites the first copy's dst inside the span.
        let stmts = vec![
            assign(0x0, 8, binary(Op::Add, var(0x100, 8), var(0x108, 8), 8)),
            assign(0x0, 8, var(0x110, 8)), // rewrites 0x0
            assign(0x8, 8, binary(Op::Add, var(0x100, 8), var(0x108, 8), 8)),
        ];
        let mut s2 = stmts.clone();
        assert_eq!(collapse_common_rhs(&mut s2), 0);
    }

    #[test]
    fn cse_runs_inside_the_pass() {
        // End-to-end through run(): the collapsed copy then feeds P1/P2 and
        // the pass output carries no duplicate Add.
        let stmts = vec![
            assign(0x0, 8, binary(Op::Add, var(0x100, 8), var(0x108, 8), 8)),
            assign(0x8, 8, binary(Op::Add, var(0x100, 8), var(0x108, 8), 8)),
            assign(0x10, 8, var(0x0, 8)),
            assign(0x18, 8, var(0x8, 8)),
        ];
        let out = run(stmts);
        let adds: usize = out
            .iter()
            .map(|s| {
                let (mut f, mut o2) = (0, 0);
                count_reads_stmt(s, 0x108, &mut f, &mut o2);
                f + o2
            })
            .sum();
        assert_eq!(adds, 1, "one evaluation of the shared RHS: {out:?}");
    }

    #[test]
    fn cheap_duplication_declines_a_signed_source() {
        // A bare signed leaf sign-extends at the store (or re-signs in the
        // reader's wider context); it must not be duplicated.
        let signed_leaf = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x100),
            select: None,
            dynamic_select: None,
            width: 8,
            var_full_width: 8,
            expr_context: ExpressionContext {
                width: 8,
                signed: true,
            },
        };
        let stmts = vec![
            assign(0x0, 8, signed_leaf),
            assign(0x8, 8, var(0x0, 8)),
            assign(0x10, 8, var(0x0, 8)),
        ];
        assert_eq!(run(stmts).len(), 3);
    }

    #[test]
    fn cheap_duplication_takes_a_constant_and_a_static_select() {
        // The other two cheap shapes: an unsigned constant and a static
        // select both duplicate into their readers.
        let konst = ProtoExpression::Value {
            value: Value::new(0x5a, 8, false),
            width: 8,
            expr_context: ctx(8),
        };
        let sel = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(0x100),
            select: Some((11, 4)),
            dynamic_select: None,
            width: 8,
            var_full_width: 16,
            expr_context: ctx(8),
        };
        let stmts = vec![
            assign(0x0, 8, konst),
            assign(0x20, 8, sel),
            assign(0x8, 8, binary(Op::Add, var(0x0, 8), var(0x20, 8), 8)),
            assign(0x10, 8, binary(Op::BitOr, var(0x0, 8), var(0x20, 8), 8)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 2, "both cheap defs retire: {out:?}");
    }

    #[test]
    fn cse_collapses_identical_rhs_into_a_copy() {
        // Two identical ands: the second becomes `= first_dst`, then the
        // cheap duplication folds its reader through.
        let e1 = binary(Op::BitAnd, var(0x100, 8), var(0x108, 8), 8);
        let e2 = binary(Op::BitAnd, var(0x100, 8), var(0x108, 8), 8);
        let stmts = vec![
            assign(0x0, 8, e1),
            assign(0x8, 8, e2),
            assign(0x10, 8, binary(Op::BitOr, var(0x0, 8), var(0x8, 8), 8)),
        ];
        let mut s2 = stmts;
        let collapsed = collapse_common_rhs(&mut s2);
        assert_eq!(collapsed, 1);
        let ProtoStatement::Assign(a) = &s2[1] else {
            panic!()
        };
        assert!(
            matches!(&a.expr, ProtoExpression::Variable { var_offset, .. } if *var_offset == VarOffset::Comb(0x0)),
            "second def collapses to a copy of the first: {:?}",
            a.expr
        );
    }

    #[test]
    fn cse_vetoes_when_an_input_changes_between() {
        let e1 = binary(Op::BitAnd, var(0x100, 8), var(0x108, 8), 8);
        let e2 = binary(Op::BitAnd, var(0x100, 8), var(0x108, 8), 8);
        let mut stmts = vec![
            assign(0x0, 8, e1),
            assign(0x100, 8, var(0x110, 8)),
            assign(0x8, 8, e2),
        ];
        assert_eq!(collapse_common_rhs(&mut stmts), 0);
    }

    #[test]
    fn cheap_duplication_vetoes_a_dynamic_element_reader() {
        // 0x0 (w=8, one element of a 2-entry array based at 0x0 with
        // stride 8... modeled here as base 0x0) is read BOTH directly and
        // through a dynamic index.  The gather-based count sees the dynamic
        // element read; count_reads cannot.  The mismatch must veto, or the
        // retired def would leave the dynamic read on stale storage.
        let dyn_read = ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(0x0),
            stride: 8,
            element_native_bytes: 4,
            index_expr: Box::new(var(0x200, 1)),
            num_elements: 2,
            select: None,
            dynamic_select: None,
            width: 8,
            expr_context: ctx(8),
        };
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x10, 8, var(0x0, 8)),
            assign(0x18, 8, dyn_read),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 3, "dynamic element reader must veto the dup");
        assert!(
            out.iter()
                .any(|s| matches!(s, ProtoStatement::Assign(a) if a.dst == VarOffset::Comb(0x0))),
            "def must survive"
        );
    }

    #[test]
    fn chains_compose_front_to_back() {
        // a -> b -> c collapses to one statement.
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x8, 8, binary(Op::BitAnd, var(0x0, 8), var(0x108, 8), 8)),
            assign(0x10, 8, binary(Op::BitOr, var(0x8, 8), var(0x110, 8), 8)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 1);
        assert_eq!(reads_of(&out[0], 0x100), (1, 0));
        assert_eq!(reads_of(&out[0], 0x108), (1, 0));
    }

    #[test]
    fn masks_the_moved_expression_to_the_def_width() {
        // A narrow def of a dirty producer (add) must arrive masked.
        let stmts = vec![
            assign(0x0, 4, binary(Op::Add, var(0x100, 4), var(0x108, 4), 4)),
            assign(0x8, 8, var(0x0, 4)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 1);
        // The reader's RHS is now And(Add(...), 0xf).
        let ProtoStatement::Assign(a) = &out[0] else {
            panic!()
        };
        let ProtoExpression::Binary {
            op: Op::BitAnd, y, ..
        } = &a.expr
        else {
            panic!("expected canonical mask wrapper, got {:?}", a.expr)
        };
        let ProtoExpression::Value { value, .. } = &**y else {
            panic!()
        };
        let Value::U64(v) = value else { panic!() };
        assert_eq!(v.payload, 0xf);
    }

    fn dyn_read(base: isize, num: usize) -> ProtoExpression {
        ProtoExpression::DynamicVariable {
            base_offset: VarOffset::Comb(base),
            stride: 8,
            element_native_bytes: 4,
            index_expr: Box::new(var(0x300, 1)),
            num_elements: num,
            select: None,
            dynamic_select: None,
            width: 8,
            expr_context: ctx(8),
        }
    }

    #[test]
    fn keep_knob_truth_table() {
        assert!(cheap_keep(Some("1"), false));
        assert!(cheap_keep(Some("last"), true));
        assert!(!cheap_keep(Some("last"), false));
        assert!(!cheap_keep(None, false));
        assert!(!cheap_keep(Some("0"), true));
    }

    #[test]
    fn cheap_duplication_keeps_a_def_an_event_reads_dynamically() {
        // An event reads `arr[idx]` over a 3-element array: the MIDDLE
        // element's def must count as external (the compact base+last
        // event gather hid it, and the event read frozen storage forever).
        let stmts = vec![
            assign(0x8, 8, var(0x100, 8)),
            assign(0x20, 8, var(0x8, 8)),
            assign(0x28, 8, var(0x8, 8)),
        ];
        let mut events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        events.insert(Event::Initial, vec![assign(0x200, 8, dyn_read(0x0, 3))]);
        let out = inline_single_readers(stmts, &events, &HashSet::default()).0;
        assert_eq!(out.len(), 3, "the event-read def must not retire");
        assert!(
            out.iter()
                .any(|s| matches!(s, ProtoStatement::Assign(a) if a.dst == VarOffset::Comb(0x8))),
        );
    }

    #[test]
    fn single_reader_inline_keeps_a_def_an_event_reads_dynamically() {
        // Same hole through the single-reader path.
        let stmts = vec![assign(0x8, 8, var(0x100, 8)), assign(0x20, 8, var(0x8, 8))];
        let mut events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        events.insert(Event::Initial, vec![assign(0x200, 8, dyn_read(0x0, 3))]);
        let out = inline_single_readers(stmts, &events, &HashSet::default()).0;
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn cheap_duplication_keeps_a_def_an_event_writes_dynamically() {
        // An event dynamically writes the whole array: re-running the comb
        // def every settle is what clobbers the event's value, so the
        // middle element's def must stay.
        let ev_write = ProtoStatement::AssignDynamic(crate::ir::ProtoAssignDynamicStatement {
            dst_base: VarOffset::Comb(0x0),
            dst_stride: 8,
            dst_num_elements: 3,
            dst_index_expr: var(0x300, 1),
            dst_width: 8,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: var(0x308, 8),
            dst_ff_current_base_offset: 0,
        });
        let stmts = vec![
            assign(0x8, 8, var(0x100, 8)),
            assign(0x20, 8, var(0x8, 8)),
            assign(0x28, 8, var(0x8, 8)),
        ];
        let mut events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        events.insert(Event::Initial, vec![ev_write]);
        let out = inline_single_readers(stmts, &events, &HashSet::default()).0;
        assert_eq!(out.len(), 3, "the event-written def must not retire");
    }

    #[test]
    fn cheap_duplication_vetoes_an_intervening_def_rewrite() {
        // `x = y; a = x; x = z; b = x`: duplicating y into b would skip the
        // rewrite b actually observes.
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x8, 8, var(0x0, 8)),
            assign(0x0, 8, var(0x108, 8)),
            assign(0x10, 8, var(0x0, 8)),
        ];
        let out = run(stmts);
        assert_eq!(out.len(), 4);
        for r in [1usize, 3] {
            assert!(
                matches!(&out[r], ProtoStatement::Assign(a)
                    if matches!(&a.expr, ProtoExpression::Variable { var_offset, .. }
                        if *var_offset == VarOffset::Comb(0x0))),
                "reader {r} must still read the storage"
            );
        }
    }

    #[test]
    fn cheap_duplication_declines_a_signed_constant() {
        let signed_const = ProtoExpression::Value {
            value: Value::U64(veryl_analyzer::value::ValueU64 {
                payload: 0x7f,
                mask_xz: 0,
                width: 8,
                signed: true,
            }),
            width: 8,
            expr_context: ctx(8),
        };
        let stmts = vec![
            assign(0x0, 8, signed_const),
            assign(0x8, 8, var(0x0, 8)),
            assign(0x10, 8, var(0x0, 8)),
        ];
        assert_eq!(run(stmts).len(), 3);
    }
}
