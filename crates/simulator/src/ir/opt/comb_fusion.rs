//! Single-reader comb def inlining (`VERYL_COMB_FUSION`) — P1 of the
//! DFG-contraction campaign (local/FUSION-DESIGN.md).
//!
//! A comb def read exactly once by later comb logic is folded into its
//! reader's expression, and the def statement disappears; the freed storage
//! becomes ghost bytes the relayout pass drops.  This is what Verilator's
//! DfgRegularize does for single-sink vertices — the statement-level
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

use crate::ir::event::Event;
use crate::ir::expression::{ExpressionContext, ProtoExpression};
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;
use crate::{HashMap, HashSet};
use veryl_analyzer::ir::Op;
use veryl_analyzer::value::Value;

/// `VERYL_COMB_FUSION=1` opt-in.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_COMB_FUSION").as_deref() == Ok("1"))
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

/// Coalesce disjoint static field stores that fully define a destination
/// word into one whole-width concat assignment — the statement-level
/// analogue of Verilator's `coalesceDrivers` (V3DfgSynthesize).
///
///   `x[3:0] = a; x[7:4] = b;`  →  `x = {b, a};`
///
/// Unlike the single-reader inlining, the destination's storage keeps its
/// exact value (full coverage means the old word contributes nothing), so
/// external visibility poses no constraint.
fn coalesce_field_stores(mut stmts: Vec<ProtoStatement>) -> (Vec<ProtoStatement>, usize) {
    let n = stmts.len();
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
            if a.select.is_some()
                && a.dynamic_select.is_none()
                && a.rhs_select.is_none()
                && a.dst_width > 0
                && a.dst_width <= 64
                && a.dst_width == g.w
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
            s.gather_variable_offsets_expanded(&mut ins, &mut outs);
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
        let contiguous = ranges[0].0 == 0
            && ranges.windows(2).all(|p| p[0].1 + 1 == p[1].0)
            && ranges.last().unwrap().1 + 1 == g.w;
        if !contiguous {
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
            stmts[i].gather_variable_offsets_expanded(&mut e_ins, &mut e_outs);
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
        let mut elements: Vec<(Box<ProtoExpression>, usize, usize)> =
            Vec::with_capacity(ranges.len());
        for &(lo, hi, i) in ranges.iter().rev() {
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
            let ew = hi - lo + 1;
            elements.push((Box::new(canonical_wrap(expr, ew)), 1, ew));
        }
        let w = g.w;
        let concat = ProtoExpression::Concatenation {
            elements,
            width: w,
            expr_context: ExpressionContext {
                width: w,
                signed: false,
            },
        };
        let ProtoStatement::Assign(last_a) = &stmts[last] else {
            unreachable!()
        };
        let fused_stmt = ProtoStatement::Assign(crate::ir::statement::ProtoAssignStatement {
            dst: last_a.dst,
            dst_width: w,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: concat,
            dst_ff_current_offset: last_a.dst_ff_current_offset,
            token: last_a.token,
        });
        stmts[last] = fused_stmt;
        for &i in &g.idxs {
            if i != last {
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
    let coalesced = if coalesce_enabled() {
        let (out, fused) = coalesce_field_stores(stmts);
        stmts = out;
        fused
    } else {
        0
    };
    // -- externals: offsets we must never make disappear.
    let mut externals: HashSet<isize> = HashSet::default();
    for off in externals_extra {
        if let VarOffset::Comb(o) = off {
            externals.insert(*o);
        }
    }
    {
        let mut ins: Vec<VarOffset> = vec![];
        let mut outs: Vec<VarOffset> = vec![];
        for stmts in events.values() {
            for s in stmts {
                ins.clear();
                outs.clear();
                s.gather_variable_offsets(&mut ins, &mut outs);
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

    // -- pass 1: reads / writes / opacity over the top-level statements.
    struct ReadInfo {
        count: usize,
        last_reader: usize,
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
            s.gather_variable_offsets_expanded(&mut ins, &mut outs);
            let opaque = !rewritable_reader(s);
            for off in ins.drain(..) {
                if let VarOffset::Comb(o) = off {
                    let e = reads.entry(o).or_insert(ReadInfo {
                        count: 0,
                        last_reader: 0,
                    });
                    e.count += 1;
                    e.last_reader = i;
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

    // -- pass 2: greedy front-to-back inlining.  Chains compose naturally:
    //    an earlier def folded into its reader travels with it when the
    //    reader is itself inlined later (its RHS is re-gathered fresh).
    let n = stmts.len();
    let mut deleted = vec![false; n];
    let mut fused_offsets: Vec<isize> = Vec::new();
    let mut inlined = 0usize;
    let (mut veto_shape, mut veto_reader, mut veto_redef, mut veto_ext) = (0usize, 0, 0, 0);
    let mut e_ins: Vec<VarOffset> = vec![];
    let mut e_outs: Vec<VarOffset> = vec![];
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
                    // The store sign-extends this shape (a bare signed leaf
                    // narrower than the destination); the substituted
                    // expression would be used unextended.
                    && a.expr.store_sign_extend_from(a.dst_width).is_none() =>
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
            if matches!(&stmts[i], ProtoStatement::Assign(a) if !a.dst.is_ff()) {
                veto_shape += 1;
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
        stmts[i].gather_variable_offsets_expanded(&mut e_ins, &mut e_outs);
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
            "[comb_fusion] stmts={} coalesced={} inlined={} veto: shape={} reader={} redef={} external={}",
            n, coalesced, inlined, veto_shape, veto_reader, veto_redef, veto_ext,
        );
    }
    if inlined == 0 {
        return (stmts, fused_offsets);
    }
    let mut out = Vec::with_capacity(n - inlined);
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
    fn keeps_a_two_reader_def() {
        let stmts = vec![
            assign(0x0, 8, var(0x100, 8)),
            assign(0x8, 8, var(0x0, 8)),
            assign(0x10, 8, var(0x0, 8)),
        ];
        assert_eq!(run(stmts).len(), 3);
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
        // the init would leave x stale on the not-taken path.  Found on a
        // real design: VeeR EH2's case-default init pattern.
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
    fn coalesce_rejects_partial_coverage() {
        // Only [3:0] and [6:4]: bit 7 keeps the old value — no fuse.
        let stmts = vec![
            assign_sel(0x0, 8, 3, 0, var(0x100, 4)),
            assign_sel(0x0, 8, 6, 4, var(0x108, 3)),
        ];
        let (out, fused) = coalesce_field_stores(stmts);
        assert_eq!(fused, 0);
        assert_eq!(out.len(), 2);
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
}
