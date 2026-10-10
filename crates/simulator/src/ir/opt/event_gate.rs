//! Idle-subtree gates over an event's statements.
//!
//! An `always_ff` fire reads the FF and comb values its statements name and
//! writes its own FFs.  When every value it reads is what it was at its last
//! fire, and that fire left every FF as it found it, the fire is a no-op and
//! can be skipped.  The comb values inside a subtree are functions of the
//! subtree's external inputs and of the FFs its comb reads, so a gate over a
//! subtree compares only those: the comb boundary (reads of the subtree's
//! comb statements that nothing inside produces), the FF reads not written by
//! the gated statements themselves, and the gated statements' own reads that
//! no comb statement of the subtree produces.  Whatever the derived-clock
//! closure touches is compared regardless, since the settle filter lets it
//! change under a skipped settle.  The other half is the write log: the
//! entries a fire pushed are compared against the values they would commit,
//! so a fire whose entries all repeat them wrote nothing new; direct comb
//! writes, which the log does not record, are shadowed and compared instead.
//!
//! Reads and writes are byte ranges taken from the expressions themselves
//! (a variable's full width, an array's whole extent), so storage the
//! variable tables do not describe (split fields, temps) is covered too.

use super::cone_gate::{ConeGateInputs, Tour, has_side_effects};
use crate::ir::ProtoExpression;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::{VarOffset, native_bytes};
use crate::ir::write_log::WriteLogBuffer;
use crate::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

/// One gated range `[lo, hi)` of an event's top-level statements.
#[derive(Clone, Debug)]
pub struct EventGate {
    pub lo: usize,
    pub hi: usize,
    /// Comb-buffer offset of this gate's state (`GATE_STATE_HEADER_BYTES`,
    /// then the shadows of `compare` and `out_comb`).  Zero until allocated.
    pub state_off: u32,
    /// `(is_ff, start, end)` byte ranges compared against the shadow.
    pub compare: Vec<(bool, u32, u32)>,
    /// Comb byte ranges the gated statements write directly, which no log
    /// entry records: a run that leaves them all unchanged wrote nothing
    /// new there.
    pub out_comb: Vec<(u32, u32)>,
    /// Subtree path (diagnostics).
    pub cone: String,
}

/// Bytes ahead of a gate's shadows, named by both the Rust runtime
/// (`RtEventGate`) and the C the AOT-C emitter writes: the two take turns on
/// one gate's state, so they must read one layout.
pub const GATE_STATE_HEADER_BYTES: usize = 16;
/// The last fire's entries all repeated the current values and it changed no
/// direct comb write.
pub(crate) const GATE_IDLE: usize = 0;
/// The gate is off: its range runs unchecked until the period below passes.
pub(crate) const GATE_OFF: usize = 2;
/// The dirty streak while on, the fires since turning off while off.
pub(crate) const GATE_COUNT: usize = 4;
/// The off period, doubled each time the gate turns off again.
pub(crate) const GATE_PERIOD: usize = 8;
/// Rough check cost, in nanoseconds: each span is a compare call and a
/// branch, each byte a slice of memory bandwidth.
const SPAN_NS: usize = 16;
const BYTES_PER_NS: usize = 8;
/// What a skipped nested statement saves.
const STMT_NS: usize = 5;

fn check_cost<'a>(spans: impl Iterator<Item = (&'a u32, &'a u32)>) -> usize {
    let (mut n, mut bytes) = (0, 0);
    for (a, b) in spans {
        n += 1;
        bytes += (b - a) as usize;
    }
    n * SPAN_NS + bytes / BYTES_PER_NS
}

impl EventGate {
    /// `compare` joined across gaps into the spans the emitter checks and
    /// shadows: the joining that makes a check cheapest, the bytes between
    /// two ranges costing bandwidth and a span apart costing a call.  No join
    /// grows a span past the watch cap, past which the emitter ungates the
    /// whole range.
    pub fn compare_spans(&self) -> Vec<Span> {
        let mut best: Option<(usize, Vec<Span>)> = None;
        let mut gap: u32 = 8;
        loop {
            let mut out: Vec<Span> = Vec::with_capacity(self.compare.len());
            for &(is_ff, a, b) in &self.compare {
                match out.last_mut() {
                    Some(p)
                        if p.0 == is_ff
                            && a <= p.2 + gap
                            && (p.2.max(b) - p.1) as usize <= crate::simulator::WATCH_CAP_BYTES =>
                    {
                        p.2 = p.2.max(b)
                    }
                    _ => out.push((is_ff, a, b)),
                }
            }
            let cost = check_cost(out.iter().map(|(_, a, b)| (a, b)));
            if best.as_ref().is_none_or(|(c, _)| cost < *c) {
                best = Some((cost, out));
            }
            if gap >= 1 << 24 {
                return best.unwrap().1;
            }
            gap *= 4;
        }
    }

    /// Nanoseconds one check of this gate costs, direct comb writes
    /// included (compared after every run).
    pub fn cost(&self) -> usize {
        check_cost(self.compare_spans().iter().map(|(_, a, b)| (a, b)))
            + check_cost(self.out_comb.iter().map(|(a, b)| (a, b)))
    }

    /// Bytes of shadow the gate's state holds after its header.
    pub fn shadow_bytes(&self) -> usize {
        self.compare_spans()
            .iter()
            .map(|&(_, a, b)| (b - a) as usize)
            .sum::<usize>()
            + self
                .out_comb
                .iter()
                .map(|&(a, b)| (b - a) as usize)
                .sum::<usize>()
    }
}
/// Nested statements a range must hold to be worth a compare.
const MIN_MASS: usize = 64;

pub fn diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_EVENT_GATE_DIAG").as_deref() == Ok("1"))
}

/// `VERYL_EVENT_GATE=0` opts out, for A/B and bisection.
pub(crate) fn enabled() -> bool {
    std::env::var("VERYL_EVENT_GATE").as_deref() != Ok("0")
}

/// `VERYL_EVENT_GATE_CHECK=1`: a gate that would skip runs its range anyway
/// and reports the first run that wrote something (a wrong skip).  Emit-time,
/// so it changes the generated C.
pub(crate) fn check() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_EVENT_GATE_CHECK").as_deref() == Ok("1"))
}

/// `(is_ff, start, end)`: a byte range in one value buffer.
type Range = (bool, usize, usize);
/// The emitter's form of a range: `(is_ff, start, end)` in `u32`.
type Span = (bool, u32, u32);
/// Sorted, merged `(start, end)` byte ranges of one buffer.
type Spans = Vec<(usize, usize)>;

fn range_of(off: VarOffset, bytes: usize) -> Option<Range> {
    let raw = off.raw();
    (raw >= 0).then(|| (off.is_ff(), raw as usize, raw as usize + bytes.max(1)))
}

fn expr_reads(e: &ProtoExpression, out: &mut Vec<Range>) {
    match e {
        ProtoExpression::Variable {
            var_offset,
            dynamic_select,
            var_full_width,
            ..
        } => {
            out.extend(range_of(*var_offset, native_bytes(*var_full_width)));
            if let Some(d) = dynamic_select {
                expr_reads(&d.index_expr, out);
            }
        }
        ProtoExpression::Value { .. } => {}
        ProtoExpression::Unary { x, .. } | ProtoExpression::Resize { x, .. } => expr_reads(x, out),
        ProtoExpression::Binary { x, y, .. } => {
            expr_reads(x, out);
            expr_reads(y, out);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (x, _, _) in elements {
                expr_reads(x, out);
            }
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            expr_reads(cond, out);
            expr_reads(true_expr, out);
            expr_reads(false_expr, out);
        }
        ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            element_native_bytes,
            index_expr,
            num_elements,
            dynamic_select,
            ..
        } => {
            expr_reads(index_expr, out);
            if let Some(d) = dynamic_select {
                expr_reads(&d.index_expr, out);
            }
            let span =
                stride.unsigned_abs() * num_elements.saturating_sub(1) + element_native_bytes;
            out.extend(range_of(*base_offset, span));
        }
        ProtoExpression::HierVariable(_) => {}
    }
}

/// Every byte range a statement may read (`reads`) or write (`writes`).
/// A dual-slot FF write names both slots, so a read of the current one
/// counts as the writer's own.
fn stmt_ranges(s: &ProtoStatement, reads: &mut Vec<Range>, writes: &mut Vec<Range>) {
    match s {
        ProtoStatement::Assign(a) => {
            expr_reads(&a.expr, reads);
            if let Some(d) = &a.dynamic_select {
                expr_reads(&d.index_expr, reads);
            }
            let nb = native_bytes(a.dst_width);
            writes.extend(range_of(a.dst, nb));
            if a.dst.is_ff() && a.dst_ff_current_offset >= 0 {
                writes.push((
                    true,
                    a.dst_ff_current_offset as usize,
                    a.dst_ff_current_offset as usize + nb,
                ));
            }
        }
        ProtoStatement::AssignDynamic(a) => {
            expr_reads(&a.expr, reads);
            expr_reads(&a.dst_index_expr, reads);
            if let Some(d) = &a.dynamic_select {
                expr_reads(&d.index_expr, reads);
            }
            let nb = native_bytes(a.dst_width);
            let span = a.dst_stride.unsigned_abs() * a.dst_num_elements.saturating_sub(1) + nb;
            writes.extend(range_of(a.dst_base, span));
            if a.dst_base.is_ff() && a.dst_ff_current_base_offset >= 0 {
                let b = a.dst_ff_current_base_offset as usize;
                writes.push((true, b, b + span));
            }
        }
        ProtoStatement::If(x) => {
            if let Some(c) = &x.cond {
                expr_reads(c, reads);
            }
            for t in x.true_side.iter().chain(x.false_side.iter()) {
                stmt_ranges(t, reads, writes);
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &x.arms {
                expr_reads(&arm.cond, reads);
                for t in &arm.body {
                    stmt_ranges(t, reads, writes);
                }
            }
            for t in &x.default {
                stmt_ranges(t, reads, writes);
            }
        }
        ProtoStatement::For(x) => {
            for e in x.range.dynamic_bounds() {
                expr_reads(e, reads);
            }
            writes.extend(range_of(x.var_offset, x.var_native_bytes));
            reads.extend(range_of(x.var_offset, x.var_native_bytes));
            for t in &x.body {
                stmt_ranges(t, reads, writes);
            }
        }
        ProtoStatement::SequentialBlock(b) => {
            for t in b {
                stmt_ranges(t, reads, writes);
            }
        }
        ProtoStatement::CompiledBlock(cb) => {
            for t in cb.original_stmts.iter() {
                stmt_ranges(t, reads, writes);
            }
        }
        ProtoStatement::SystemFunctionCall(_)
        | ProtoStatement::TbMethodCall { .. }
        | ProtoStatement::Break => {}
        ProtoStatement::HierAssign(_) => {
            unreachable!("hierarchical assignment is resolved by resolve_hier_refs")
        }
    }
}

fn owner_span(owner: &[(usize, usize, u32)], x: usize) -> Option<(usize, usize, u32)> {
    let i = owner.partition_point(|&(s, _, _)| s <= x).checked_sub(1)?;
    let (s, e, id) = owner[i];
    (x < e).then_some((s, e, id))
}

/// Sorted ranges with overlaps joined.  Touching ranges stay apart: adjacent
/// variables are the rule, and joining them would let one subtree's writes
/// or products swallow its neighbour's.
fn merge<T: Copy + Ord>(mut v: Vec<(T, T)>) -> Vec<(T, T)> {
    v.sort_unstable();
    join_sorted(v)
}

fn join_sorted<T: Copy + Ord>(v: impl IntoIterator<Item = (T, T)>) -> Vec<(T, T)> {
    let v = v.into_iter();
    let mut out: Vec<(T, T)> = Vec::with_capacity(v.size_hint().0);
    for (s, e) in v {
        match out.last_mut() {
            Some(p) if s < p.1 => p.1 = p.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// `merge` of the union of three sorted lists.  A list may already be
/// joined: no range is empty, so joining yields the union's overlap-connected
/// pieces however the lists split them.
fn join_union(
    a: impl Iterator<Item = (u32, u32)>,
    b: impl Iterator<Item = (u32, u32)>,
    c: impl Iterator<Item = (u32, u32)>,
) -> Vec<(u32, u32)> {
    let (mut a, mut b, mut c) = (a.peekable(), b.peekable(), c.peekable());
    join_sorted(std::iter::from_fn(move || {
        let (x, y, z) = (a.peek().copied(), b.peek().copied(), c.peek().copied());
        let min = [x, y, z].into_iter().flatten().min()?;
        if x == Some(min) {
            a.next()
        } else if y == Some(min) {
            b.next()
        } else {
            c.next()
        }
    }))
}

/// Is `[s, e)` inside one range of a subtree's products?
fn produces(produced: &[(u32, u32)], s: usize, e: usize) -> bool {
    let (s, e) = narrow((s, e));
    inside(produced, s, e)
}

/// A byte range in the `u32` offsets the emitter's spans and gate state use.
fn narrow((s, e): (usize, usize)) -> (u32, u32) {
    let e = u32::try_from(e).expect("event gate ranges lie within u32 offsets");
    (s as u32, e)
}

/// Does `[s, e)` overlap any range of the sorted, merged `ranges`?
fn overlaps(ranges: &[(usize, usize)], s: usize, e: usize) -> bool {
    let i = ranges.partition_point(|&(rs, _)| rs < e);
    i > 0 && ranges[i - 1].1 > s
}

/// Is `[s, e)` inside one range of the sorted, merged `ranges`?
fn inside<T: Copy + Ord>(ranges: &[(T, T)], s: T, e: T) -> bool {
    let i = ranges.partition_point(|&(rs, _)| rs <= s);
    i > 0 && ranges[i - 1].1 >= e
}

fn split(v: &[Range]) -> (Spans, Spans) {
    let ff = v.iter().filter(|r| r.0).map(|r| (r.1, r.2)).collect();
    let comb = v.iter().filter(|r| !r.0).map(|r| (r.1, r.2)).collect();
    (merge(ff), merge(comb))
}

/// A comb statement's owning node and byte ranges.
struct CombInfo {
    node: u32,
    in_comb: Vec<(usize, usize)>,
    in_ff: Vec<(usize, usize)>,
    out_comb: Vec<(usize, usize)>,
}

/// What every event of a module shares.  Building it walks the whole comb
/// list, which dwarfs any one event's statements, so a module builds it once
/// and plans each event over it.
pub struct CombContext<'a> {
    inputs: &'a ConeGateInputs,
    /// Sorted by the owner's tour entry number, so a subtree is one slice.
    combs: Vec<CombInfo>,
    comb_touched_comb: Spans,
    closure_ff: Spans,
    closure_comb: Spans,
    /// Kept across events, as a subtree's products and reads do not vary by
    /// event.  The reads are gathered only once a run of the subtree passes
    /// the scratch check.
    produced: HashMap<u32, Vec<(u32, u32)>>,
    reads: HashMap<u32, SubtreeReads>,
    /// Scratch for `plan`'s candidate walk, all false between calls.
    candidate: Vec<bool>,
}

/// A subtree's comb reads, split by what decides whether a gate over the
/// subtree compares them.  A read the derived-clock closure touches is
/// always compared, so those are kept joined.  Any other comb read is
/// compared unless the subtree produces it or the gated statements write
/// it, and any other FF read unless the gated statements write it; a gate
/// filters those one by one, so they are only deduplicated.
struct SubtreeReads {
    comb_always: Vec<(u32, u32)>,
    comb_unless_written: Vec<(u32, u32)>,
    ff_always: Vec<(u32, u32)>,
    ff_unless_written: Vec<(u32, u32)>,
}

impl<'a> CombContext<'a> {
    /// `comb_stmts` is the settled comb list, `closure_touched` every offset
    /// the derived-clock closure reads or writes, `comb_touched` every offset
    /// the comb touches.
    pub fn new(
        inputs: &'a ConeGateInputs,
        comb_stmts: &[ProtoStatement],
        closure_touched: &HashSet<VarOffset>,
        comb_touched: &HashSet<VarOffset>,
    ) -> Self {
        let (closure_ff, closure_comb): (Spans, Spans) = {
            let mut ff = Vec::new();
            let mut comb = Vec::new();
            for o in closure_touched {
                let raw = o.raw();
                if raw < 0 {
                    continue;
                }
                let r = (raw as usize, raw as usize + 1);
                if o.is_ff() { ff.push(r) } else { comb.push(r) }
            }
            (merge(ff), merge(comb))
        };
        // Comb statements by owning node (the LCA of the output owners, as
        // the cone plan attributes them) with their byte ranges.  A statement
        // whose outputs no variable owns belongs to nobody: what it produces
        // then counts as a boundary read for any subtree consuming it.
        let mut combs: Vec<CombInfo> = Vec::with_capacity(comb_stmts.len());
        let (mut ins, mut outs) = (Vec::new(), Vec::new());
        for s in comb_stmts {
            ins.clear();
            outs.clear();
            stmt_ranges(s, &mut ins, &mut outs);
            let mut n: Option<u32> = None;
            for &(is_ff, start, _) in &outs {
                if !is_ff && let Some((_, _, id)) = owner_span(&inputs.comb_var, start) {
                    n = Some(n.map_or(id, |a| inputs.lca(a, id)));
                }
            }
            let Some(n) = n else {
                continue;
            };
            let (in_ff, in_comb) = split(&ins);
            let (_, out_comb) = split(&outs);
            combs.push(CombInfo {
                node: n,
                in_comb,
                in_ff,
                out_comb,
            });
        }
        combs.sort_by_key(|c| inputs.tour().entry(c.node));
        let comb_touched_comb: Spans = merge(
            comb_touched
                .iter()
                .filter(|o| !o.is_ff() && o.raw() >= 0)
                .map(|o| (o.raw() as usize, o.raw() as usize + 1))
                .collect(),
        );
        CombContext {
            inputs,
            combs,
            comb_touched_comb,
            closure_ff,
            closure_comb,
            produced: HashMap::default(),
            reads: HashMap::default(),
            candidate: vec![false; inputs.node_parent.len()],
        }
    }

    fn subtree<'c>(tour: &Tour, combs: &'c [CombInfo], n: u32) -> &'c [CombInfo] {
        debug_assert!(
            combs
                .windows(2)
                .all(|w| tour.entry(w[0].node) <= tour.entry(w[1].node)),
            "subtree slices `combs` by the tour, so it must stay in entry order"
        );
        let (lo, hi) = (tour.entry(n), tour.exit(n));
        let s = combs.partition_point(|c| tour.entry(c.node) < lo);
        let e = combs.partition_point(|c| tour.entry(c.node) < hi);
        &combs[s..e]
    }
}

fn subtree_reads(
    inside_comb: &[CombInfo],
    produced: &[(u32, u32)],
    closure_ff: &Spans,
    closure_comb: &Spans,
) -> SubtreeReads {
    let dedup = |mut v: Vec<(u32, u32)>| {
        v.sort_unstable();
        v.dedup();
        v
    };
    let (mut comb_always, mut comb_unless_written) = (Vec::new(), Vec::new());
    let (mut ff_always, mut ff_unless_written) = (Vec::new(), Vec::new());
    for c in inside_comb {
        for &(s, e) in &c.in_comb {
            if overlaps(closure_comb, s, e) {
                comb_always.push(narrow((s, e)));
            } else if !produces(produced, s, e) {
                comb_unless_written.push(narrow((s, e)));
            }
        }
        for &(s, e) in &c.in_ff {
            if overlaps(closure_ff, s, e) {
                ff_always.push(narrow((s, e)));
            } else {
                ff_unless_written.push(narrow((s, e)));
            }
        }
    }
    SubtreeReads {
        comb_always: join_sorted(dedup(comb_always)),
        comb_unless_written: dedup(comb_unless_written),
        ff_always: join_sorted(dedup(ff_always)),
        ff_unless_written: dedup(ff_unless_written),
    }
}

pub fn plan(stmts: &[ProtoStatement], ctx: &mut CombContext, label: &str) -> Vec<EventGate> {
    debug_assert!(
        ctx.candidate.iter().all(|&c| !c),
        "the candidate scratch is left all false"
    );
    let CombContext {
        inputs,
        combs,
        comb_touched_comb,
        closure_ff,
        closure_comb,
        produced: produced_by_node,
        reads: reads_by_node,
        candidate,
    } = ctx;
    let inputs: &ConeGateInputs = inputs;
    let tour = inputs.tour();
    let diag_on = diag();
    // Event statements: owning node (by the FFs written), reads, writes, and
    // whether a skip could lose an effect.  A statement writing only
    // event-scoped scratch belongs to whatever subtree surrounds it.
    let mut node: Vec<Option<u32>> = Vec::with_capacity(stmts.len());
    let mut reads: Vec<Vec<Range>> = Vec::with_capacity(stmts.len());
    let mut writes: Vec<Vec<Range>> = Vec::with_capacity(stmts.len());
    let mut blocked: Vec<bool> = Vec::with_capacity(stmts.len());
    let mut mass: Vec<usize> = Vec::with_capacity(stmts.len());
    let (mut ins, mut outs) = (Vec::new(), Vec::new());
    for s in stmts {
        ins.clear();
        outs.clear();
        stmt_ranges(s, &mut ins, &mut outs);
        let mut n: Option<u32> = None;
        let mut block = has_side_effects(s);
        for &(is_ff, start, _) in &outs {
            if !is_ff {
                continue;
            }
            match owner_span(&inputs.ff_node, start) {
                Some((_, _, id)) => n = Some(n.map_or(id, |a| inputs.lca(a, id))),
                None => block = true,
            }
        }
        node.push(n);
        reads.push(ins.clone());
        writes.push(outs.clone());
        blocked.push(block);
        mass.push(s.statement_mass());
    }

    // Candidate subtrees: every node owning event statements, outer nodes
    // first so the emitter nests the inner gates.  A node qualifies exactly
    // when it is an owner or an owner's ancestor.
    let mut nodes: Vec<u32> = Vec::new();
    for &m in node.iter().flatten() {
        let mut x = m;
        while !candidate[x as usize] {
            candidate[x as usize] = true;
            nodes.push(x);
            match inputs.parent(x) {
                Some(p) => x = p,
                None => break,
            }
        }
    }
    for &n in &nodes {
        candidate[n as usize] = false;
    }
    nodes.sort_unstable_by_key(|&n| (tour.depth(n), n));

    let mut gates: Vec<EventGate> = Vec::new();
    let mut counts = [0usize; 5];
    let mut by_range: HashMap<(usize, usize), usize> = HashMap::default();
    for n in nodes {
        let path = &inputs.node_path[n as usize];
        // Maximal runs of the subtree's statements: a statement whose skip
        // could lose an effect, or one of another subtree, ends a run and is
        // left ungated.  Runs over one node gate independently: each
        // compares what the other writes.  A run starts and ends on the
        // subtree's own statements: unowned ones at either end would let the
        // runs of two unrelated subtrees overlap, and the emitter can only
        // nest gates.
        let mut runs: Vec<(usize, usize, usize)> = Vec::new();
        let mut start: Option<usize> = None;
        for i in 0..=stmts.len() {
            let member =
                i < stmts.len() && node[i].is_none_or(|m| tour.is_desc(m, n)) && !blocked[i];
            match (start, member) {
                (None, true) => start = Some(i),
                (Some(a), false) => {
                    let owned = |j: &usize| node[*j].is_some();
                    if let Some(lo) = (a..i).find(owned) {
                        let hi = (a..i).rev().find(owned).unwrap() + 1;
                        let total_mass: usize = mass[lo..hi].iter().sum();
                        // Dropping it here may spare the subtree's comb scan.
                        if total_mass < MIN_MASS {
                            counts[0] += 1;
                        } else {
                            runs.push((lo, hi, total_mass));
                        }
                    }
                    start = None;
                }
                _ => {}
            }
        }
        if runs.is_empty() {
            continue;
        }
        let inside_comb = CombContext::subtree(tour, combs, n);
        let produced = produced_by_node.entry(n).or_insert_with(|| {
            merge(
                inside_comb
                    .iter()
                    .flat_map(|c| c.out_comb.iter().map(|&r| narrow(r)))
                    .collect(),
            )
        });
        let mut gated: Vec<(usize, usize, usize, Spans, Spans)> = Vec::new();
        for (lo, hi, total_mass) in runs {
            let (written_ff, written_comb) = split(
                &(lo..hi)
                    .flat_map(|i| writes[i].iter().copied())
                    .collect::<Vec<_>>(),
            );
            // Event-scoped scratch (comb storage no comb statement reads)
            // flows between an always_ff's statements.  A skipped run leaves
            // its scratch stale, so no statement outside the run may read
            // scratch the run writes, and the run may not read scratch
            // written outside it.
            let scratch_written: Vec<(usize, usize)> = written_comb
                .iter()
                .filter(|&&(s, e)| !overlaps(comb_touched_comb, s, e))
                .copied()
                .collect();
            let scratch_flow = (0..stmts.len()).any(|i| {
                let outside = i < lo || i >= hi;
                reads[i].iter().any(|&(is_ff, s, e)| {
                    !is_ff
                        && if outside {
                            overlaps(&scratch_written, s, e)
                        } else {
                            !inside(&written_comb, s, e)
                                && !produces(produced, s, e)
                                && !overlaps(comb_touched_comb, s, e)
                                && owner_span(&inputs.comb_var, s).is_none()
                        }
                })
            });
            if scratch_flow {
                counts[3] += 1;
                if diag_on {
                    eprintln!("[event_gate] {label} skip {path} [{lo}..{hi}) scratch flow");
                }
                continue;
            }
            gated.push((lo, hi, total_mass, written_ff, written_comb));
        }
        if gated.is_empty() {
            continue;
        }
        let sub = reads_by_node
            .entry(n)
            .or_insert_with(|| subtree_reads(inside_comb, produced, closure_ff, closure_comb));
        for (lo, hi, total_mass, written_ff, written_comb) in gated {
            // A direct comb write of the gated statements is shadowed and
            // compared after each run, so an idle gate holds it; an FF they
            // write is at its committed value while the gate is idle.
            let boundary = sub
                .comb_unless_written
                .iter()
                .filter(|&&(s, e)| !inside(&written_comb, s as usize, e as usize));
            let comb_ff = sub
                .ff_unless_written
                .iter()
                .filter(|&&(s, e)| !inside(&written_ff, s as usize, e as usize));
            // The gated statements' own reads that neither they nor the
            // subtree's comb produce.
            let (mut event_ff, mut event_comb) = (Vec::new(), Vec::new());
            for r in &reads[lo..hi] {
                for &(is_ff, s, e) in r {
                    if is_ff {
                        if !inside(&written_ff, s, e) || overlaps(closure_ff, s, e) {
                            event_ff.push(narrow((s, e)));
                        }
                    } else if !inside(&written_comb, s, e)
                        && (!produces(produced, s, e) || overlaps(closure_comb, s, e))
                    {
                        event_comb.push(narrow((s, e)));
                    }
                }
            }
            event_ff.sort_unstable();
            event_comb.sort_unstable();
            if diag_on {
                // `(bytes, source, is_ff, start)` of every range taken.
                let mut taken: Vec<(usize, &str, bool, usize)> = Vec::new();
                let mut take = |src, is_ff, v: &mut dyn Iterator<Item = &(u32, u32)>| {
                    for &(s, e) in v {
                        taken.push(((e - s) as usize, src, is_ff, s as usize));
                    }
                };
                take(
                    "comb-boundary",
                    false,
                    &mut sub.comb_always.iter().chain(boundary.clone()),
                );
                take(
                    "comb-ff",
                    true,
                    &mut sub.ff_always.iter().chain(comb_ff.clone()),
                );
                take("event-ff", true, &mut event_ff.iter());
                take("event-comb", false, &mut event_comb.iter());
                taken.sort_unstable();
                taken.dedup();
                for src in ["comb-boundary", "comb-ff", "event-ff", "event-comb"] {
                    let (mut cnt, mut bytes, mut unowned, mut outside) = (0, 0, 0, 0);
                    for &(b, k, is_ff, s) in &taken {
                        if k != src {
                            continue;
                        }
                        cnt += 1;
                        bytes += b;
                        let own = if is_ff {
                            owner_span(&inputs.ff_node, s)
                        } else {
                            owner_span(&inputs.comb_var, s)
                        };
                        match own {
                            None => unowned += 1,
                            Some((_, _, id)) if !tour.is_desc(id, n) => outside += 1,
                            _ => {}
                        }
                    }
                    if cnt > 0 {
                        eprintln!(
                            "[event_gate] {label}   {path} [{lo}..{hi}) {src}: {cnt} ranges {bytes} bytes, unowned {unowned}, outside {outside}"
                        );
                    }
                }
            }
            let mut compare: Vec<(bool, u32, u32)> = Vec::new();
            let ff = join_union(
                sub.ff_always.iter().copied(),
                comb_ff.copied(),
                event_ff.into_iter(),
            );
            let comb = join_union(
                sub.comb_always.iter().copied(),
                boundary.copied(),
                event_comb.into_iter(),
            );
            for (is_ff, v) in [(true, ff), (false, comb)] {
                for (s, e) in v {
                    compare.push((is_ff, s, e));
                }
            }
            // Direct comb writes the comb reads: shadowed, not logged.
            let out_comb: Vec<(u32, u32)> = written_comb
                .iter()
                .filter(|&&(s, e)| overlaps(comb_touched_comb, s, e))
                .map(|&r| narrow(r))
                .collect();
            let gate = EventGate {
                lo,
                hi,
                state_off: 0,
                compare,
                out_comb,
                cone: path.clone(),
            };
            // Worth it when a check costs no more than half of what a skip
            // saves: a gate skipping less than that turns itself off.
            let cost = gate.cost();
            if cost > total_mass * STMT_NS / 2 {
                counts[1] += 1;
                if diag_on {
                    eprintln!(
                        "[event_gate] {label} skip {path} [{lo}..{hi}) mass={total_mass} check {cost} ns"
                    );
                }
                continue;
            }
            // Gates nest or stay apart; a partial overlap has no place in
            // the emitted structure.
            if gates.iter().any(|g| {
                lo < g.hi && g.lo < hi && !(g.lo <= lo && hi <= g.hi) && !(lo <= g.lo && g.hi <= hi)
            }) {
                debug_assert!(false, "event gate {path} [{lo}..{hi}) overlaps another");
                counts[4] += 1;
                continue;
            }
            // Nested nodes over the same statements: one gate, the cheaper.
            if let Some(&k) = by_range.get(&(lo, hi)) {
                if cost < gates[k].cost() {
                    gates[k] = gate;
                }
                counts[2] += 1;
                continue;
            }
            if diag_on {
                eprintln!(
                    "[event_gate] {label} gate {path} [{lo}..{hi}) mass={total_mass} check {cost} ns, {} spans, {} ranges",
                    gate.compare_spans().len(),
                    gate.compare.len() + gate.out_comb.len()
                );
            }
            by_range.insert((lo, hi), gates.len());
            gates.push(gate);
        }
    }
    // Outer ranges first; the emitter nests inner ones inside them.
    gates.sort_by_key(|g| (g.lo, std::cmp::Reverse(g.hi)));
    if diag_on {
        eprintln!(
            "[event_gate] {label}: stmts={} gates={} small={} wide={} merged={} scratch={} overlap={}",
            stmts.len(),
            gates.len(),
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            counts[4]
        );
    }
    gates
}

/// Consecutive dirty checks after which an event gate stops checking.
pub(crate) const EVENT_GATE_AUTO_OFF_STREAK: u32 = 64;
/// Fires an auto-offed gate stays off before it checks again; doubled on
/// each further turn-off up to the cap, reset by a skip.
pub(crate) const EVENT_GATE_REARM_FIRES: u32 = 1024;
pub(crate) const EVENT_GATE_REARM_CAP: u32 = 1 << 16;

/// A planned gate with the range of chunk blocks its statements went into.
#[derive(Debug)]
pub struct ChunkedGate {
    pub gate: EventGate,
    pub blocks: (usize, usize),
}

/// A gate over the per-statement event path: `[lo, hi)` of the event's
/// runtime statements, kept by the rules of the AOT-C emitter's gate on the
/// same state bytes, so the two paths can take turns on one gate.
#[derive(Debug)]
pub struct RtEventGate {
    pub lo: usize,
    pub hi: usize,
    state_off: usize,
    /// `compare_spans()` of the planned gate, in shadow order.
    spans: Vec<Span>,
    out_comb: Vec<(u32, u32)>,
    /// Where the direct comb writes' shadows start, after the spans'.
    out_comb_off: usize,
    cone: String,
    /// The gates nested in this one, in range order.
    pub children: Vec<usize>,
    /// Check mode: this gate's wrong skip has been reported.
    said: AtomicBool,
}

/// One event's gates; `roots` are the ones nested in no other.
#[derive(Debug)]
pub struct RtEventGates {
    pub gates: Vec<RtEventGate>,
    pub roots: Vec<usize>,
}

/// What a fire does with a gate's range.
pub enum GateEntry {
    /// Turned off: the range runs unjudged.
    Plain,
    /// Idle since the last fire and every read held.
    Skip,
    /// The range runs and is judged from the log counts at entry; `checked`
    /// when the reads were compared (and their shadows refreshed) on the way.
    Run { n0: u32, w0: u32, checked: bool },
}

impl RtEventGates {
    /// Partial overlaps never reach here (the planner drops them), so the
    /// ranges nest.
    pub fn new<'a>(gates: impl Iterator<Item = (&'a EventGate, (usize, usize))>) -> Self {
        let mut items: Vec<(&EventGate, (usize, usize))> =
            gates.filter(|(_, (lo, hi))| lo < hi).collect();
        // Outer gates first: by start, the longer range ahead on a tie.
        items.sort_by_key(|&(_, (lo, hi))| (lo, std::cmp::Reverse(hi)));
        let mut out: Vec<RtEventGate> = Vec::with_capacity(items.len());
        let mut roots = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        for (g, (lo, hi)) in items {
            while open.last().is_some_and(|&p| lo >= out[p].hi) {
                open.pop();
            }
            let idx = out.len();
            match open.last() {
                Some(&p) => out[p].children.push(idx),
                None => roots.push(idx),
            }
            let spans = g.compare_spans();
            out.push(RtEventGate {
                lo,
                hi,
                state_off: g.state_off as usize,
                out_comb_off: spans.iter().map(|&(_, a, b)| (b - a) as usize).sum(),
                spans,
                out_comb: g.out_comb.clone(),
                cone: g.cone.clone(),
                children: Vec::new(),
                said: AtomicBool::new(false),
            });
            open.push(idx);
        }
        Self { gates: out, roots }
    }
}

unsafe fn read_u32(p: *const u8) -> u32 {
    unsafe { p.cast::<u32>().read_unaligned() }
}

unsafe fn write_u32(p: *mut u8, v: u32) {
    unsafe { p.cast::<u32>().write_unaligned(v) }
}

/// A span is usually one variable's 1-8 bytes, which a typed load settles
/// without the libcall a slice compare would take.
///
/// # Safety
/// Both pointers must be valid for `len` bytes.
unsafe fn bytes_eq(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        match len {
            1 => *a == *b,
            2 => a.cast::<u16>().read_unaligned() == b.cast::<u16>().read_unaligned(),
            4 => a.cast::<u32>().read_unaligned() == b.cast::<u32>().read_unaligned(),
            8 => a.cast::<u64>().read_unaligned() == b.cast::<u64>().read_unaligned(),
            _ => std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len),
        }
    }
}

/// Did every entry pushed since counts `n0` / `w0` repeat the value it
/// would commit?
///
/// # Safety
/// `ff` must be valid for every entry's bytes.
pub unsafe fn log_unchanged(ff: *const u8, log: &WriteLogBuffer, n0: u32, w0: u32) -> bool {
    let narrow = &log.narrow_entries_slice()[n0 as usize..log.narrow_count as usize];
    for e in narrow {
        let cur = unsafe { ff.add(e.offset as usize) };
        let held = unsafe {
            match e.width_class {
                1 => *cur == e.payload as u8,
                2 => cur.cast::<u16>().read_unaligned() == e.payload as u16,
                4 => cur.cast::<u32>().read_unaligned() == e.payload as u32,
                _ => cur.cast::<u64>().read_unaligned() == e.payload,
            }
        };
        if !held {
            return false;
        }
    }
    let wide = &log.wide_entries_slice()[w0 as usize..log.wide_count as usize];
    for e in wide {
        let n = e.native_bytes as usize;
        let cur = unsafe { std::slice::from_raw_parts(ff.add(e.offset as usize), n) };
        if cur != &e.payload[..n] {
            return false;
        }
    }
    true
}

impl RtEventGate {
    /// The gate's state bytes and, after the header, its shadows.
    ///
    /// # Safety
    /// `comb` must be valid for the gate's state.
    unsafe fn state(&self, comb: *mut u8) -> (*mut u8, *mut u8) {
        let eg = unsafe { comb.add(self.state_off) };
        (eg, unsafe { eg.add(GATE_STATE_HEADER_BYTES) })
    }

    /// Off: the range runs plain until the re-arm period passes; the first
    /// fire after re-arming runs and takes a fresh snapshot.  On: a checked
    /// fire whose reads all matched skips, resetting the dirty streak;
    /// otherwise the range runs, a dirty streak turning the gate off for
    /// twice as long each time.
    ///
    /// # Safety
    /// `ff` and `comb` must be valid for the gate's spans and state.
    pub unsafe fn enter(&self, ff: *const u8, comb: *mut u8, log: &WriteLogBuffer) -> GateEntry {
        let (eg, shadow) = unsafe { self.state(comb) };
        let mut run = true;
        let mut checked = false;
        unsafe {
            if *eg.add(GATE_OFF) != 0 {
                let mut n = read_u32(eg.add(GATE_COUNT)) + 1;
                if n >= read_u32(eg.add(GATE_PERIOD)) {
                    *eg.add(GATE_OFF) = 0;
                    *eg.add(GATE_IDLE) = 0;
                    n = 0;
                }
                write_u32(eg.add(GATE_COUNT), n);
                return GateEntry::Plain;
            }
            if *eg.add(GATE_IDLE) != 0 {
                run = false;
                checked = true;
                let mut sh = shadow;
                for &(is_ff, a, b) in &self.spans {
                    let len = (b - a) as usize;
                    let src = if is_ff {
                        ff.add(a as usize)
                    } else {
                        comb.add(a as usize)
                    };
                    // A mismatching span refreshes its shadow on the spot: the
                    // run that follows leaves the compared values as they are.
                    if !bytes_eq(sh, src, len) {
                        std::ptr::copy_nonoverlapping(src, sh, len);
                        run = true;
                    }
                    sh = sh.add(len);
                }
            }
            if !run {
                write_u32(eg.add(GATE_COUNT), 0);
                write_u32(eg.add(GATE_PERIOD), 0);
                return GateEntry::Skip;
            }
            let mut streak = read_u32(eg.add(GATE_COUNT)) + 1;
            if streak >= EVENT_GATE_AUTO_OFF_STREAK {
                let off = read_u32(eg.add(GATE_PERIOD));
                let off = if off == 0 {
                    EVENT_GATE_REARM_FIRES
                } else {
                    (off * 2).min(EVENT_GATE_REARM_CAP)
                };
                write_u32(eg.add(GATE_PERIOD), off);
                *eg.add(GATE_OFF) = 1;
                streak = 0;
            }
            write_u32(eg.add(GATE_COUNT), streak);
            self.snapshot_out_comb(comb);
        }
        GateEntry::Run {
            n0: log.narrow_count,
            w0: log.wide_count,
            checked,
        }
    }

    /// After a `Run`: idle when the run's entries all repeated their values
    /// and no direct comb write changed; an idle run whose reads were not
    /// compared on entry snapshots them now.
    ///
    /// # Safety
    /// As `enter`, and `ff` valid for the log's entries.
    pub unsafe fn exit(
        &self,
        ff: *const u8,
        comb: *mut u8,
        log: &WriteLogBuffer,
        n0: u32,
        w0: u32,
        checked: bool,
    ) {
        let (eg, shadow) = unsafe { self.state(comb) };
        let idle = unsafe { log_unchanged(ff, log, n0, w0) && self.out_comb_held(comb) };
        if idle && !checked {
            let mut sh = shadow;
            for &(is_ff, a, b) in &self.spans {
                let len = (b - a) as usize;
                unsafe {
                    let src = if is_ff {
                        ff.add(a as usize)
                    } else {
                        comb.add(a as usize)
                    };
                    std::ptr::copy_nonoverlapping(src, sh, len);
                    sh = sh.add(len);
                }
            }
        }
        unsafe { *eg.add(GATE_IDLE) = u8::from(idle) };
    }

    /// Check mode: a `Skip` ran its range anyway from the log counts `n0` /
    /// `w0`; report the first such run that wrote something.
    ///
    /// # Safety
    /// As `exit`.
    pub unsafe fn check_skip(
        &self,
        ff: *const u8,
        comb: *mut u8,
        log: &WriteLogBuffer,
        n0: u32,
        w0: u32,
    ) {
        let wrote = unsafe { !(log_unchanged(ff, log, n0, w0) && self.out_comb_held(comb)) };
        if wrote && !self.said.swap(true, Ordering::Relaxed) {
            eprintln!(
                "[event_gate] WRONG SKIP gate {} (per-statement path)",
                self.cone
            );
        }
    }

    /// The shadow of the direct comb writes, after the spans' shadows.
    unsafe fn out_comb_shadow(&self, comb: *mut u8) -> *mut u8 {
        let (_, shadow) = unsafe { self.state(comb) };
        unsafe { shadow.add(self.out_comb_off) }
    }

    /// Shadow the direct comb writes before a run.
    ///
    /// # Safety
    /// `comb` must be valid for the gate's state and `out_comb`.
    pub unsafe fn snapshot_out_comb(&self, comb: *mut u8) {
        let mut sh = unsafe { self.out_comb_shadow(comb) };
        for &(a, b) in &self.out_comb {
            let len = (b - a) as usize;
            unsafe {
                std::ptr::copy_nonoverlapping(comb.add(a as usize), sh, len);
                sh = sh.add(len);
            }
        }
    }

    unsafe fn out_comb_held(&self, comb: *mut u8) -> bool {
        let mut sh = unsafe { self.out_comb_shadow(comb) };
        for &(a, b) in &self.out_comb {
            let len = (b - a) as usize;
            unsafe {
                if !bytes_eq(sh, comb.add(a as usize), len) {
                    return false;
                }
                sh = sh.add(len);
            }
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::opt::cone_gate::FfOwner;
    use crate::ir::{ExpressionContext, ProtoAssignStatement};
    use veryl_analyzer::value::{Value, ValueU64};
    use veryl_parser::token_range::TokenRange;

    /// Densely packed small ranges join cheapest into one span far past the
    /// cap; the emitter would then drop the gate.
    #[test]
    fn joined_compare_spans_stay_within_the_watch_cap() {
        let compare: Vec<(bool, u32, u32)> =
            (0..400u32).map(|i| (false, i * 12, i * 12 + 4)).collect();
        let gate = EventGate {
            lo: 0,
            hi: 1,
            state_off: 0,
            compare,
            out_comb: Vec::new(),
            cone: String::new(),
        };
        let spans = gate.compare_spans();
        assert!(spans.len() > 1);
        for (_, a, b) in spans {
            assert!((b - a) as usize <= crate::simulator::WATCH_CAP_BYTES);
        }
    }

    /// A block of `n` constant writes to the 4-byte FF at `off`.
    fn ff_block(off: isize, n: usize) -> ProtoStatement {
        let assign = ProtoAssignStatement {
            dst: VarOffset::Ff(off),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Value {
                value: Value::U64(ValueU64 {
                    payload: 1,
                    mask_xz: 0,
                    width: 32,
                    signed: false,
                }),
                width: 32,
                expr_context: ExpressionContext {
                    width: 32,
                    signed: false,
                },
            },
            dst_ff_current_offset: off,
            comb_direct: false,
            token: TokenRange::default(),
        };
        ProtoStatement::SequentialBlock(vec![ProtoStatement::Assign(assign); n])
    }

    /// A constant write to the 4-byte comb variable at `off`.
    fn comb_write(off: isize) -> ProtoStatement {
        let assign = ProtoAssignStatement {
            dst: VarOffset::Comb(off),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Value {
                value: Value::U64(ValueU64 {
                    payload: 1,
                    mask_xz: 0,
                    width: 32,
                    signed: false,
                }),
                width: 32,
                expr_context: ExpressionContext {
                    width: 32,
                    signed: false,
                },
            },
            dst_ff_current_offset: -1,
            comb_direct: false,
            token: TokenRange::default(),
        };
        ProtoStatement::Assign(assign)
    }

    /// The whole point of the tour: slicing `combs` by `[entry, exit)` has to
    /// name the same statements as asking each one whether it is under `n`.
    #[test]
    fn a_subtree_slice_names_what_an_ancestor_test_names() {
        // top { a { p, q }, b }, one comb variable per node.
        let inputs = ConeGateInputs {
            node_parent: vec![u32::MAX, 0, 1, 1, 0],
            node_path: ["top", "top.a", "top.a.p", "top.a.q", "top.b"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            comb_owner: Vec::new(),
            ff_owner: FfOwner::default(),
            comb_var: vec![(0, 8, 0), (8, 16, 1), (16, 24, 2), (24, 32, 3), (32, 40, 4)],
            ff_node: Vec::new(),
            event_written_comb: vec![],
            trigger_events: Vec::new(),
            event_writes: Vec::new(),
            master_clocks: Vec::new(),
            ff_next_alias: Vec::new(),
            tour: Default::default(),
        };
        // Deliberately not in tour order, so a missing sort shows up.
        let stmts: Vec<ProtoStatement> = [32, 16, 0, 24, 8].map(comb_write).into_iter().collect();
        let ctx = CombContext::new(&inputs, &stmts, &HashSet::default(), &HashSet::default());
        assert_eq!(ctx.combs.len(), 5, "every statement has an owner");
        let tour = inputs.tour();
        for n in 0..inputs.node_parent.len() as u32 {
            let mut sliced: Vec<u32> = CombContext::subtree(tour, &ctx.combs, n)
                .iter()
                .map(|c| c.node)
                .collect();
            let mut filtered: Vec<u32> = ctx
                .combs
                .iter()
                .filter(|c| tour.is_desc(c.node, n))
                .map(|c| c.node)
                .collect();
            sliced.sort_unstable();
            filtered.sort_unstable();
            assert_eq!(sliced, filtered, "subtree of node {n}");
        }
        // And the shape is the one the tree implies, not just self-consistent.
        let owned = |n: u32| -> Vec<u32> {
            let mut v: Vec<u32> = CombContext::subtree(tour, &ctx.combs, n)
                .iter()
                .map(|c| c.node)
                .collect();
            v.sort_unstable();
            v
        };
        assert_eq!(owned(0), vec![0, 1, 2, 3, 4]);
        assert_eq!(owned(1), vec![1, 2, 3]);
        assert_eq!(owned(4), vec![4]);
    }

    #[test]
    fn a_run_never_ends_on_an_unowned_statement() {
        // root 0 { 1, 2 }.  An event-scoped scratch write between the two
        // siblings' blocks belongs to neither run: taking it would make the
        // runs overlap, and the emitter can only nest gates.
        let inputs = ConeGateInputs {
            node_parent: vec![u32::MAX, 0, 0],
            node_path: ["top", "top.a", "top.b"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            comb_owner: Vec::new(),
            ff_owner: FfOwner::default(),
            comb_var: Vec::new(),
            ff_node: vec![(0, 4, 1), (4, 8, 2)],
            event_written_comb: vec![],
            trigger_events: Vec::new(),
            event_writes: Vec::new(),
            master_clocks: Vec::new(),
            ff_next_alias: Vec::new(),
            tour: Default::default(),
        };
        let mut scratch = ff_block(0, 1);
        if let ProtoStatement::SequentialBlock(b) = &mut scratch
            && let ProtoStatement::Assign(a) = &mut b[0]
        {
            a.dst = VarOffset::Comb(0x100);
            a.dst_ff_current_offset = -1;
        }
        let stmts = [ff_block(0, MIN_MASS), scratch, ff_block(4, MIN_MASS)];
        let mut ctx = CombContext::new(&inputs, &[], &HashSet::default(), &HashSet::default());
        let gates = plan(&stmts, &mut ctx, "t");
        let mut ranges: Vec<(usize, usize, &str)> = gates
            .iter()
            .map(|g| (g.lo, g.hi, g.cone.as_str()))
            .collect();
        ranges.sort();
        assert_eq!(
            ranges,
            vec![(0, 1, "top.a"), (0, 3, "top"), (2, 3, "top.b")]
        );
    }

    /// `dst = srcs[0] + srcs[1] + ...` over 4-byte variables.
    fn sum(dst: VarOffset, srcs: &[VarOffset]) -> ProtoStatement {
        let ctx = ExpressionContext {
            width: 32,
            signed: false,
        };
        let var = |o: VarOffset| ProtoExpression::Variable {
            var_offset: o,
            select: None,
            dynamic_select: None,
            width: 32,
            var_full_width: 32,
            expr_context: ctx,
        };
        let mut expr = var(srcs[0]);
        for &o in &srcs[1..] {
            expr = ProtoExpression::Binary {
                x: Box::new(expr),
                op: crate::ir::Op::Add,
                y: Box::new(var(o)),
                width: 32,
                expr_context: ctx,
            };
        }
        let ProtoStatement::Assign(mut a) = comb_write(0) else {
            unreachable!()
        };
        a.dst = dst;
        a.dst_ff_current_offset = if dst.is_ff() { dst.raw() } else { -1 };
        a.expr = expr;
        ProtoStatement::Assign(a)
    }

    fn bytes(o: VarOffset) -> impl Iterator<Item = VarOffset> {
        (0..4).map(move |i| match o {
            VarOffset::Ff(x) => VarOffset::Ff(x + i),
            VarOffset::Comb(x) => VarOffset::Comb(x + i),
        })
    }

    fn two_leaf_inputs() -> ConeGateInputs {
        // top { a, b }: `a` owns comb 0x100.. and FF 0..0x40, `b` FF 0x40..0x80.
        ConeGateInputs {
            node_parent: vec![u32::MAX, 0, 0],
            node_path: ["top", "top.a", "top.b"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            comb_owner: Vec::new(),
            ff_owner: FfOwner::default(),
            comb_var: vec![(0x100, 0x140, 1)],
            ff_node: vec![(0, 0x40, 1), (0x40, 0x80, 2)],
            event_written_comb: vec![],
            trigger_events: Vec::new(),
            event_writes: Vec::new(),
            master_clocks: Vec::new(),
            ff_next_alias: Vec::new(),
            tour: Default::default(),
        }
    }

    /// `a`'s comb reads, one of each kind a gate treats differently, and an
    /// event whose run writes some of them.
    fn a_comb_and_event() -> (Vec<ProtoStatement>, Vec<ProtoStatement>) {
        use VarOffset::{Comb, Ff};
        let comb = vec![
            // 0x200 a boundary read, 0x300 closure-touched, FF 0x10 unwritten.
            sum(Comb(0x100), &[Comb(0x200), Comb(0x300), Ff(0x10)]),
            // 0x100 produced, 0x108 produced but closure-touched, 0x400
            // written by the run.
            sum(Comb(0x104), &[Comb(0x100), Comb(0x108), Comb(0x400)]),
            // FF 0 written by the run, FF 0x20 closure-touched.
            sum(Comb(0x108), &[Ff(0), Ff(0x20)]),
        ];
        let event = vec![
            ff_block(0, MIN_MASS),
            comb_write(0x400),
            sum(Ff(0), &[Comb(0x500), Ff(0x30)]),
        ];
        (comb, event)
    }

    fn a_context<'a>(inputs: &'a ConeGateInputs, comb: &[ProtoStatement]) -> CombContext<'a> {
        use VarOffset::{Comb, Ff};
        let closure: HashSet<VarOffset> = [Comb(0x300), Comb(0x108), Ff(0x20)]
            .into_iter()
            .flat_map(bytes)
            .collect();
        let touched: HashSet<VarOffset> = [Comb(0x100), Comb(0x104), Comb(0x108), Comb(0x400)]
            .into_iter()
            .chain([Comb(0x500)])
            .flat_map(bytes)
            .collect();
        CombContext::new(inputs, comb, &closure, &touched)
    }

    type GateKey = (usize, usize, Vec<(bool, u32, u32)>, Vec<(u32, u32)>, String);

    fn key(gates: &[EventGate]) -> Vec<GateKey> {
        gates
            .iter()
            .map(|g| {
                (
                    g.lo,
                    g.hi,
                    g.compare.clone(),
                    g.out_comb.clone(),
                    g.cone.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn a_gate_compares_what_its_run_cannot_hold() {
        let inputs = two_leaf_inputs();
        let (comb, event) = a_comb_and_event();
        let mut ctx = a_context(&inputs, &comb);
        let gates = plan(&event, &mut ctx, "t");
        // `top` and `top.a` gate the same run at the same cost; the outer one
        // is kept.
        assert_eq!(gates.len(), 1);
        let g = &gates[0];
        assert_eq!((g.lo, g.hi, g.cone.as_str()), (0, 3, "top"));
        assert_eq!(
            g.compare,
            vec![
                (true, 0x10, 0x14),
                (true, 0x20, 0x24),
                (true, 0x30, 0x34),
                (false, 0x108, 0x10c),
                (false, 0x200, 0x204),
                (false, 0x300, 0x304),
                (false, 0x500, 0x504),
            ]
        );
        assert_eq!(g.out_comb, vec![(0x400, 0x404)]);
        assert_eq!(g.cost(), 102);
    }

    #[test]
    fn events_planned_on_one_context_match_fresh_contexts() {
        let inputs = two_leaf_inputs();
        let (comb, a) = a_comb_and_event();
        // `b`'s candidates include `a`'s: a mark left over from planning `a`
        // would drop them.
        let b = vec![ff_block(0x40, MIN_MASS), ff_block(0, MIN_MASS)];
        let fresh =
            |event: &[ProtoStatement]| key(&plan(event, &mut a_context(&inputs, &comb), "t"));
        let (want_a, want_b) = (fresh(&a), fresh(&b));
        assert!(
            want_b
                .iter()
                .any(|g| (g.0, g.1, g.4.as_str()) == (0, 2, "top"))
        );
        let mut ctx = a_context(&inputs, &comb);
        for (event, want) in [(&a, &want_a), (&b, &want_b), (&a, &want_a)] {
            assert_eq!(&key(&plan(event, &mut ctx, "t")), want);
        }
    }
}
