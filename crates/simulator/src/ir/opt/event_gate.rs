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

use super::cone_gate::{ConeGateInputs, has_side_effects};
use crate::ir::ProtoExpression;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::{VarOffset, native_bytes};
use crate::{HashMap, HashSet};

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

/// Bytes ahead of a gate's shadows: `[0]` idle (the last fire's entries all
/// repeated the current values and it changed no direct comb write), `[2]`
/// off, `[4..8)` the dirty
/// streak while on and the fires since turning off while off, `[8..12)` the
/// off period, doubled each time the gate turns off again.
pub const GATE_STATE_HEADER_BYTES: usize = 16;
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
    /// two ranges costing bandwidth and a span apart costing a call.
    pub fn compare_spans(&self) -> Vec<Span> {
        let mut best: Option<(usize, Vec<Span>)> = None;
        let mut gap: u32 = 8;
        loop {
            let mut out: Vec<Span> = Vec::with_capacity(self.compare.len());
            for &(is_ff, a, b) in &self.compare {
                match out.last_mut() {
                    Some(p) if p.0 == is_ff && a <= p.2 + gap => p.2 = p.2.max(b),
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

pub(crate) fn diag() -> bool {
    std::env::var("VERYL_EVENT_GATE_DIAG").as_deref() == Ok("1")
}

/// `VERYL_EVENT_GATE=0` opts out, for A/B and bisection.
pub(crate) fn enabled() -> bool {
    std::env::var("VERYL_EVENT_GATE").as_deref() != Ok("0")
}

/// `VERYL_EVENT_GATE_CHECK=1`: a gate that would skip runs its range anyway
/// and reports the first run that wrote something (a wrong skip).  Emit-time,
/// so it changes the generated C.
pub(crate) fn check() -> bool {
    std::env::var("VERYL_EVENT_GATE_CHECK").as_deref() == Ok("1")
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
        ProtoExpression::Unary { x, .. } => expr_reads(x, out),
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
fn merge(mut v: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    v.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(v.len());
    for (s, e) in v {
        match out.last_mut() {
            Some(p) if s < p.1 => p.1 = p.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Does `[s, e)` overlap any range of the sorted, merged `ranges`?
fn overlaps(ranges: &[(usize, usize)], s: usize, e: usize) -> bool {
    let i = ranges.partition_point(|&(rs, _)| rs < e);
    i > 0 && ranges[i - 1].1 > s
}

/// Is `[s, e)` inside one range of the sorted, merged `ranges`?
fn inside(ranges: &[(usize, usize)], s: usize, e: usize) -> bool {
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

/// Plan the gates of one event.  `comb_stmts` is the settled comb list,
/// `closure_touched` every offset the derived-clock closure reads or writes,
/// `comb_touched` every offset the comb touches.
pub fn plan(
    stmts: &[ProtoStatement],
    inputs: &ConeGateInputs,
    comb_stmts: &[ProtoStatement],
    closure_touched: &HashSet<VarOffset>,
    comb_touched: &HashSet<VarOffset>,
    label: &str,
) -> Vec<EventGate> {
    let parent = |m: u32| -> Option<u32> {
        let p = inputs.node_parent[m as usize];
        (p != u32::MAX).then_some(p)
    };
    let is_desc = |mut m: u32, a: u32| -> bool {
        loop {
            if m == a {
                return true;
            }
            match parent(m) {
                Some(p) => m = p,
                None => return false,
            }
        }
    };
    let depth = |mut m: u32| -> usize {
        let mut d = 0;
        while let Some(p) = parent(m) {
            d += 1;
            m = p;
        }
        d
    };
    let lca = |mut a: u32, mut b: u32| -> u32 {
        let (mut da, mut db) = (depth(a), depth(b));
        while da > db {
            a = parent(a).unwrap();
            da -= 1;
        }
        while db > da {
            b = parent(b).unwrap();
            db -= 1;
        }
        while a != b {
            a = parent(a).unwrap();
            b = parent(b).unwrap();
        }
        a
    };
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
                Some((_, _, id)) => n = Some(n.map_or(id, |a| lca(a, id))),
                None => block = true,
            }
        }
        node.push(n);
        reads.push(ins.clone());
        writes.push(outs.clone());
        blocked.push(block);
        mass.push(s.statement_mass());
    }

    // Comb statements by owning node (the LCA of the output owners, as the
    // cone plan attributes them) with their byte ranges.  A statement whose
    // outputs no variable owns belongs to nobody: what it produces then
    // counts as a boundary read for any subtree consuming it.
    let mut combs: Vec<CombInfo> = Vec::with_capacity(comb_stmts.len());
    for s in comb_stmts {
        ins.clear();
        outs.clear();
        stmt_ranges(s, &mut ins, &mut outs);
        let mut n: Option<u32> = None;
        for &(is_ff, start, _) in &outs {
            if !is_ff && let Some((_, _, id)) = owner_span(&inputs.comb_var, start) {
                n = Some(n.map_or(id, |a| lca(a, id)));
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
    let comb_touched_comb: Vec<(usize, usize)> = merge(
        comb_touched
            .iter()
            .filter(|o| !o.is_ff() && o.raw() >= 0)
            .map(|o| (o.raw() as usize, o.raw() as usize + 1))
            .collect(),
    );

    // Candidate subtrees: every node owning event statements, outer nodes
    // first so the emitter nests the inner gates.
    let nnodes = inputs.node_parent.len() as u32;
    let mut nodes: Vec<u32> = (0..nnodes)
        .filter(|&n| node.iter().any(|m| m.is_some_and(|m| is_desc(m, n))))
        .collect();
    nodes.sort_by_key(|&n| depth(n));

    let mut gates: Vec<EventGate> = Vec::new();
    let mut counts = [0usize; 5];
    let mut by_range: HashMap<(usize, usize), usize> = HashMap::default();
    for n in nodes {
        let path = &inputs.node_path[n as usize];
        let inside_comb: Vec<&CombInfo> = combs.iter().filter(|c| is_desc(c.node, n)).collect();
        let produced = merge(
            inside_comb
                .iter()
                .flat_map(|c| c.out_comb.iter().copied())
                .collect(),
        );
        // Maximal runs of the subtree's statements: a statement whose skip
        // could lose an effect, or one of another subtree, ends a run and is
        // left ungated.  Runs over one node gate independently: each
        // compares what the other writes.  A run starts and ends on the
        // subtree's own statements: unowned ones at either end would let the
        // runs of two unrelated subtrees overlap, and the emitter can only
        // nest gates.
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut start: Option<usize> = None;
        for i in 0..=stmts.len() {
            let member = i < stmts.len() && node[i].is_none_or(|m| is_desc(m, n)) && !blocked[i];
            match (start, member) {
                (None, true) => start = Some(i),
                (Some(a), false) => {
                    let owned = |j: &usize| node[*j].is_some();
                    if let Some(lo) = (a..i).find(owned) {
                        let hi = (a..i).rev().find(owned).unwrap() + 1;
                        runs.push((lo, hi));
                    }
                    start = None;
                }
                _ => {}
            }
        }
        for (lo, hi) in runs {
            let total_mass: usize = mass[lo..hi].iter().sum();
            if total_mass < MIN_MASS {
                counts[0] += 1;
                continue;
            }
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
                .filter(|&&(s, e)| !overlaps(&comb_touched_comb, s, e))
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
                                && !inside(&produced, s, e)
                                && !overlaps(&comb_touched_comb, s, e)
                                && owner_span(&inputs.comb_var, s).is_none()
                        }
                })
            });
            if scratch_flow {
                counts[3] += 1;
                if diag() {
                    eprintln!("[event_gate] {label} skip {path} [{lo}..{hi}) scratch flow");
                }
                continue;
            }
            let mut cmp_ff: Vec<(usize, usize)> = Vec::new();
            let mut cmp_comb: Vec<(usize, usize)> = Vec::new();
            // Diagnostics: `(bytes, source, is_ff, start)` of every range taken.
            let mut taken: Vec<(usize, &str, bool, usize)> = Vec::new();
            for c in &inside_comb {
                for &(s, e) in &c.in_comb {
                    // A direct comb write of the gated statements is shadowed
                    // and compared after each run, so an idle gate holds it.
                    if (!inside(&produced, s, e) && !inside(&written_comb, s, e))
                        || overlaps(&closure_comb, s, e)
                    {
                        cmp_comb.push((s, e));
                        taken.push((e - s, "comb-boundary", false, s));
                    }
                }
                for &(s, e) in &c.in_ff {
                    // An FF the gated statements write is at its committed
                    // value while the gate is idle.
                    if !inside(&written_ff, s, e) || overlaps(&closure_ff, s, e) {
                        cmp_ff.push((s, e));
                        taken.push((e - s, "comb-ff", true, s));
                    }
                }
            }
            // The gated statements' own reads that neither they nor the
            // subtree's comb produce.
            for r in &reads[lo..hi] {
                for &(is_ff, s, e) in r {
                    if is_ff {
                        if !inside(&written_ff, s, e) || overlaps(&closure_ff, s, e) {
                            cmp_ff.push((s, e));
                            taken.push((e - s, "event-ff", true, s));
                        }
                    } else if !inside(&written_comb, s, e)
                        && (!inside(&produced, s, e) || overlaps(&closure_comb, s, e))
                    {
                        cmp_comb.push((s, e));
                        taken.push((e - s, "event-comb", false, s));
                    }
                }
            }
            if diag() {
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
                            Some((_, _, id)) if !is_desc(id, n) => outside += 1,
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
            for (is_ff, v) in [(true, merge(cmp_ff)), (false, merge(cmp_comb))] {
                for (s, e) in v {
                    compare.push((is_ff, s as u32, e as u32));
                }
            }
            // Direct comb writes the comb reads: shadowed, not logged.
            let out_comb: Vec<(u32, u32)> = written_comb
                .iter()
                .filter(|&&(s, e)| overlaps(&comb_touched_comb, s, e))
                .map(|&(s, e)| (s as u32, e as u32))
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
                if diag() {
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
            if diag() {
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
    if diag() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::opt::cone_gate::FfOwner;
    use crate::ir::{ExpressionContext, ProtoAssignStatement};
    use veryl_analyzer::value::{Value, ValueU64};
    use veryl_parser::token_range::TokenRange;

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
            token: TokenRange::default(),
        };
        ProtoStatement::SequentialBlock(vec![ProtoStatement::Assign(assign); n])
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
        };
        let mut scratch = ff_block(0, 1);
        if let ProtoStatement::SequentialBlock(b) = &mut scratch
            && let ProtoStatement::Assign(a) = &mut b[0]
        {
            a.dst = VarOffset::Comb(0x100);
            a.dst_ff_current_offset = -1;
        }
        let stmts = [ff_block(0, MIN_MASS), scratch, ff_block(4, MIN_MASS)];
        let gates = plan(
            &stmts,
            &inputs,
            &[],
            &HashSet::default(),
            &HashSet::default(),
            "t",
        );
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
}
