//! Derived (gated / divided) clock support.
//!
//! A derived clock is a non-port `clock`-typed variable whose value is
//! produced inside the module — by a comb expression (`let clk_g: clock
//! = i_clk & i_en;`) or an `always_ff` write.  The simulator detects
//! 0→1 transitions after each `step()` and synthesizes
//! `Event::Clock` for downstream `always_ff(derived_clk)`.
//!
//! Derived-clock values are refreshed by the regular comb settle.
//!
//! A non-port ASYNC RESET the design produces itself rides the same
//! machinery: no testbench drives it, so its assertion is a transition of
//! an internal net just like a derived clock's rising edge, and firing
//! `Event::Reset` there is what keeps `if_reset` from waiting for
//! the next clock edge.

use crate::HashMap;
use crate::HashSet;
use crate::ir::expression::ProtoExpression;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;
use smallvec::SmallVec;
use veryl_analyzer::ir::Op;
use veryl_analyzer::ir::VarId;

#[derive(Clone, Debug)]
pub struct DerivedClock {
    pub var_id: VarId,
    /// `is_ff()` selects between `ff_values` and `comb_values`.
    pub current_offset: VarOffset,
    /// Always 1 for a clock; carried for the `read_native_value` ABI.
    pub native_bytes: usize,
    /// The comb closure reaches a master input clock (`gclk = clk & en`):
    /// fires pre-commit with the master edge (ICG semantics) instead of
    /// in the post-commit loop.  See `step_with_derived_clocks`.
    pub master_gated: bool,
    /// Declared `clock_negedge`: the active edge is the net's FALL, so the
    /// monitored bit is read inverted and every 0→1 test below means "fell".
    pub negedge: bool,
}

/// An internally produced async reset, monitored for its ASSERTION.
#[derive(Clone, Debug)]
pub struct DerivedReset {
    /// The resets that assert with this net; their events fire together.
    pub members: Vec<VarId>,
    /// The start of the members' copy chain, possibly none of them.
    /// `is_ff()` selects between `ff_values` and `comb_values`.
    pub current_offset: VarOffset,
    /// Always 1 for a reset; carried for the `read_native_value` ABI.
    pub native_bytes: usize,
    /// The net asserts when it reads 0.
    pub active_low: bool,
}

/// A net to monitor: `(var, offset, native bytes, polarity, negedge)`, where
/// the polarity is `None` for a clock and `Some(active_low)` for an async
/// reset, and `negedge` marks a `clock_negedge` (meaningless for a reset).
pub type EdgeCandidate = (VarId, VarOffset, usize, Option<bool>, bool);

/// Statement indices that write one offset, ascending.  A single writer is
/// the ordinary case; sibling conditional arms are what make it more.
type Writers = SmallVec<[usize; 1]>;

#[derive(Clone, Debug, Default)]
pub struct DerivedClockSchedule {
    pub clocks: Vec<DerivedClock>,
    pub resets: Vec<DerivedReset>,
    /// Input clocks toggled 0→1 in `step()` so gated-clock expressions
    /// see a rising edge.  Boundary inputs of the dependency closure
    /// that match a top-module clock-typed variable — either an input
    /// port or a testbench `$tb::clock_gen` inst output.
    pub master_input_clocks: SmallVec<[VarId; 4]>,
    /// `(copy, source)` for a derived clock that only copies another clock:
    /// its flops fire on the source's event, sampling the same values.
    pub aliases: Vec<(VarId, VarId)>,
    /// A derived reset's comb reads a master input clock, so its level
    /// differs between the master's two phases.
    pub resets_read_master: bool,
}

impl DerivedClockSchedule {
    pub fn is_empty(&self) -> bool {
        self.clocks.is_empty() && self.resets.is_empty()
    }

    /// The clock whose event fires for `clock`'s edges.
    pub fn source_of(&self, clock: VarId) -> VarId {
        self.aliases
            .iter()
            .find(|(copy, _)| *copy == clock)
            .map_or(clock, |(_, source)| *source)
    }
}

pub fn build_schedule(
    candidates: &[EdgeCandidate],
    pre_jit_stmts: &[ProtoStatement],
    input_clock_offsets: &HashMap<VarOffset, VarId>,
    negedge_inputs: &HashSet<VarId>,
) -> DerivedClockSchedule {
    // FF outputs go through the event/commit path.  Every writer, not the
    // last: an offset written by sibling conditional arms has one fragment
    // per arm, and keeping only the last drops the others from the closure.
    let mut output_to_writer: HashMap<VarOffset, Writers> = HashMap::default();
    let mut scratch_in: Vec<VarOffset> = Vec::new();
    let mut scratch_out: Vec<VarOffset> = Vec::new();
    for (i, stmt) in pre_jit_stmts.iter().enumerate() {
        scratch_in.clear();
        scratch_out.clear();
        stmt.gather_variable_offsets(&mut scratch_in, &mut scratch_out);
        for off in &scratch_out {
            if !off.is_ff() {
                // A statement holding sibling arms gathers its output once per
                // arm, and only WHICH statements to run matters below.  Its
                // pushes are adjacent, so the tail is enough to dedupe.
                let writers = output_to_writer.entry(*off).or_default();
                if writers.last() != Some(&i) {
                    writers.push(i);
                }
            }
        }
    }

    // Skip nets with no writer: testbench-driven (e.g. `inst clk:
    // $tb::clock_gen`, `inst rst: $tb::reset_gen`) nets have their edges
    // supplied directly by the testbench, so monitoring them would just
    // push the module onto `step_with_derived_clocks` for nothing.
    // FF-storage nets always pass; their writer is the always_ff stmt,
    // which `output_to_writer` doesn't track.
    let driven = |off: &VarOffset| off.is_ff() || output_to_writer.contains_key(off);

    let mut clocks: Vec<DerivedClock> = candidates
        .iter()
        .filter(|(_, off, _, polarity, _)| polarity.is_none() && driven(off))
        .map(|(var_id, off, nb, _, negedge)| DerivedClock {
            var_id: *var_id,
            current_offset: *off,
            native_bytes: *nb,
            master_gated: false,
            negedge: *negedge,
        })
        .collect();

    // A reset that only copies another asserts with it, so one read of the
    // chain's source stands for all of them.
    let mut resets: Vec<DerivedReset> = Vec::new();
    let mut reset_at: HashMap<(VarOffset, bool), usize> = HashMap::default();
    for (var_id, off, nb, polarity, _) in candidates {
        let Some(active_low) = polarity.filter(|_| driven(off)) else {
            continue;
        };
        let source = copy_root(*off, pre_jit_stmts, &output_to_writer);
        match reset_at.get(&(source, active_low)) {
            Some(&i) => resets[i].members.push(*var_id),
            None => {
                reset_at.insert((source, active_low), resets.len());
                resets.push(DerivedReset {
                    members: vec![*var_id],
                    current_offset: source,
                    native_bytes: *nb,
                    active_low,
                });
            }
        }
    }

    // FF-derived first, then comb-derived in topo order — matches the
    // chain-fire fixpoint's natural firing order.
    clocks.sort_by_key(|c| {
        if c.current_offset.is_ff() {
            (0u32, 0u32)
        } else {
            // The offset only holds its settled value once every fragment
            // that can write it has run.
            let writer = output_to_writer
                .get(&c.current_offset)
                .and_then(|w| w.last().copied())
                .unwrap_or(usize::MAX);
            (1u32, writer as u32)
        }
    });

    let clock_at: HashMap<VarOffset, (VarId, bool)> = clocks
        .iter()
        .map(|c| (c.current_offset, (c.var_id, c.negedge)))
        .collect();
    let mut aliases = Vec::new();
    clocks.retain(|clk| {
        if clk.negedge || clk.current_offset.is_ff() {
            return true;
        }
        let source = copied_source(
            clk.current_offset,
            pre_jit_stmts,
            &output_to_writer,
            input_clock_offsets,
            &clock_at,
        )
        .filter(|source| *source != clk.var_id && !negedge_inputs.contains(source));
        if let Some(source) = source {
            aliases.push((clk.var_id, source));
        }
        source.is_none()
    });

    let master_input_clocks = mark_master_gated(
        &mut clocks,
        pre_jit_stmts,
        &output_to_writer,
        input_clock_offsets,
    );
    let resets_read_master = reads_master(
        resets.iter().map(|r| r.current_offset),
        &master_input_clocks,
        pre_jit_stmts,
        &output_to_writer,
        input_clock_offsets,
    );

    DerivedClockSchedule {
        clocks,
        resets,
        master_input_clocks,
        aliases,
        resets_read_master,
    }
}

/// Whether the comb closure of any of `offsets` reads one of `masters`.
fn reads_master(
    offsets: impl Iterator<Item = VarOffset>,
    masters: &[VarId],
    pre_jit_stmts: &[ProtoStatement],
    output_to_writer: &HashMap<VarOffset, Writers>,
    input_clock_offsets: &HashMap<VarOffset, VarId>,
) -> bool {
    let mut probes: Vec<DerivedClock> = offsets
        .filter(|o| !o.is_ff())
        .map(|current_offset| DerivedClock {
            var_id: VarId::default(),
            current_offset,
            native_bytes: 1,
            master_gated: false,
            negedge: false,
        })
        .collect();
    if probes.is_empty() || masters.is_empty() {
        return false;
    }
    // A reset that copies a master resolves to the master's own offset.
    if probes.iter().any(|p| {
        input_clock_offsets
            .get(&p.current_offset)
            .is_some_and(|v| masters.contains(v))
    }) {
        return true;
    }
    mark_master_gated(
        &mut probes,
        pre_jit_stmts,
        output_to_writer,
        input_clock_offsets,
    )
    .iter()
    .any(|m| masters.contains(m))
}

/// A hierarchy cell `(partition, cell)` the cone writes into.
pub(crate) type ConeMark = (u32, u32);

/// The comb statements the master input clocks reach: running just these, in
/// order, with the masters raised brings a settled comb to the full settle's
/// state.  `None` when the cone depends on statement order beyond that (an
/// offset with several writers) or is not much smaller than the comb.
pub(crate) fn master_cone(
    schedule: &DerivedClockSchedule,
    pre_jit_stmts: &[ProtoStatement],
    input_clock_offsets: &HashMap<VarOffset, VarId>,
    partitions: &crate::ir::hier_cells::PartitionIndex,
) -> Option<(Vec<ProtoStatement>, Vec<ConeMark>)> {
    fn leaves<'a>(s: &'a ProtoStatement, out: &mut Vec<Leaf<'a>>) {
        match s {
            ProtoStatement::CompiledBlock(cb) if cb.hier.is_none() && cb.has_originals() => {
                for x in cb.original_stmts.iter() {
                    leaves(x, out);
                }
            }
            _ => out.push(Leaf::Plain(s)),
        }
    }
    // A hierarchy class block's statements are made one block at a time, so
    // the whole comb is never copied out at once.
    enum Leaf<'a> {
        Plain(&'a ProtoStatement),
        Class(&'a crate::ir::statement::CompiledBlockStatement, usize),
    }
    let diag = std::env::var("VERYL_MASTER_CONE_DIAG").is_ok();
    let decline = |why: &str| {
        if diag {
            eprintln!("[master_cone] declined: {why}");
        }
        None
    };
    if schedule.master_input_clocks.is_empty() {
        return decline("no master clock");
    }
    let mut flat: Vec<Leaf> = Vec::new();
    // Where a leaf sits inside a hierarchy class block: the cell to mark
    // dirty once the cone has rewritten it outside the cell.
    let mut origin: Vec<Option<ConeMark>> = Vec::new();
    let fold = crate::ir::big_array::BigArrayFold::default();
    let mut io: Vec<(Vec<VarOffset>, Vec<VarOffset>)> = Vec::new();
    let gather = |s: &ProtoStatement, io: &mut Vec<(Vec<VarOffset>, Vec<VarOffset>)>| {
        let mut ins = Vec::new();
        let mut outs = Vec::new();
        s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
        ins.retain(|o| !o.is_ff());
        outs.retain(|o| !o.is_ff());
        io.push((ins, outs));
    };
    for s in pre_jit_stmts {
        if let ProtoStatement::CompiledBlock(cb) = s {
            if !cb.has_originals() {
                return decline("a compiled block without its statements");
            }
            if let Some(h) = &cb.hier {
                let key = (
                    std::sync::Arc::as_ptr(&h.cells) as usize,
                    cb.comb_delta_bytes,
                );
                let Some(&partition) = partitions.get(&key) else {
                    return decline("a hierarchy class block outside the comb list");
                };
                for (i, x) in cb.originals().iter().enumerate() {
                    gather(x, &mut io);
                    flat.push(Leaf::Class(cb, i));
                    origin.push(Some((partition, h.cells.cell_of(h.class, i))));
                }
                continue;
            }
        }
        let first = flat.len();
        leaves(s, &mut flat);
        for leaf in &flat[first..] {
            if let Leaf::Plain(x) = leaf {
                gather(x, &mut io);
            }
        }
        origin.resize(flat.len(), None);
    }
    let mut writers: HashMap<VarOffset, SmallVec<[u32; 1]>> = HashMap::default();
    for (i, (_, outs)) in io.iter().enumerate() {
        for o in outs {
            writers.entry(*o).or_default().push(i as u32);
        }
    }
    let mut tainted: HashSet<VarOffset> = input_clock_offsets
        .iter()
        .filter(|(_, v)| schedule.master_input_clocks.contains(v))
        .map(|(o, _)| *o)
        .collect();
    let mut cone: Vec<usize> = Vec::new();
    let mut in_cone = vec![false; flat.len()];
    for (i, (ins, outs)) in io.iter().enumerate() {
        if ins.iter().any(|o| tainted.contains(o)) {
            cone.push(i);
            in_cone[i] = true;
            tainted.extend(outs.iter().copied());
        }
    }
    // Run alone, a cone statement reads the settled (last) value of each
    // offset, and its writes stand unless a later writer reruns too.
    let later = |o: &VarOffset, i: usize| -> &[u32] {
        let w = writers.get(o).map_or(&[][..], |w| w.as_slice());
        &w[w.partition_point(|&j| j as usize <= i)..]
    };
    for &i in &cone {
        let (ins, outs) = &io[i];
        if ins.iter().any(|o| !later(o, i).is_empty()) {
            return decline("a cone statement reads an offset written again later");
        }
        if outs
            .iter()
            .any(|o| later(o, i).iter().any(|&j| !in_cone[j as usize]))
        {
            return decline("a later statement outside the cone rewrites a cone write");
        }
    }
    // Taint flows forward, so a statement outside the cone reading a cone
    // write sits before its writer: the comb needs another pass to see it.
    if io
        .iter()
        .enumerate()
        .any(|(i, (ins, _))| !in_cone[i] && ins.iter().any(|o| tainted.contains(o)))
    {
        return decline("a statement before the cone reads a cone write");
    }
    if diag {
        eprintln!("[master_cone] {} of {} statements", cone.len(), flat.len());
    }
    if cone.is_empty() || cone.len() * 8 > flat.len() {
        return decline("the cone is not much smaller than the comb");
    }
    let mut marks: Vec<ConeMark> = cone.iter().filter_map(|&i| origin[i]).collect();
    marks.sort_unstable();
    marks.dedup();
    let stmts = cone
        .into_iter()
        .map(|i| match flat[i] {
            Leaf::Plain(s) => s.clone(),
            Leaf::Class(cb, j) => cb.original_at(j),
        })
        .collect();
    Some((stmts, marks))
}

/// The master input clock the copy chain from `offset` reaches, else the
/// clock nearest the chain's source.
fn copied_source(
    mut offset: VarOffset,
    pre_jit_stmts: &[ProtoStatement],
    output_to_writer: &HashMap<VarOffset, Writers>,
    input_clock_offsets: &HashMap<VarOffset, VarId>,
    clock_at: &HashMap<VarOffset, (VarId, bool)>,
) -> Option<VarId> {
    let mut nearest = None;
    // A chain longer than the statement count would revisit a writer.
    for _ in 0..=pre_jit_stmts.len() {
        if let Some(&(clock, negedge)) = clock_at.get(&offset) {
            if negedge {
                return None;
            }
            nearest = Some(clock);
        }
        let Some(writers) = output_to_writer.get(&offset) else {
            return input_clock_offsets.get(&offset).copied().or(nearest);
        };
        let next = match writers.as_slice() {
            [writer] => pre_jit_stmts.get(*writer).and_then(copy_source),
            _ => None,
        };
        match next {
            Some(next) => offset = next,
            None => return nearest,
        }
    }
    None
}

fn copy_root(
    mut offset: VarOffset,
    pre_jit_stmts: &[ProtoStatement],
    output_to_writer: &HashMap<VarOffset, Writers>,
) -> VarOffset {
    // A chain longer than the statement count would revisit a writer.
    for _ in 0..=pre_jit_stmts.len() {
        let next = match output_to_writer.get(&offset).map(|w| w.as_slice()) {
            Some([writer]) => pre_jit_stmts.get(*writer).and_then(copy_source),
            _ => None,
        };
        match next {
            Some(next) => offset = next,
            None => return offset,
        }
    }
    offset
}

/// `src` of a one-bit `dst = src`, also under the `& 1` a logic-to-clock
/// conversion lowers to.
fn copy_source(stmt: &ProtoStatement) -> Option<VarOffset> {
    let ProtoStatement::Assign(a) = stmt else {
        return None;
    };
    if a.dst_width != 1
        || a.dst.is_ff()
        || a.select.is_some()
        || a.dynamic_select.is_some()
        || a.rhs_select.is_some()
    {
        return None;
    }
    copied_bit(&a.expr)
}

/// The one-bit variable `e` copies.  Inlining a chain of copies leaves one
/// `& 1` per link.
fn copied_bit(e: &ProtoExpression) -> Option<VarOffset> {
    let is_one = |e: &ProtoExpression| matches!(e, ProtoExpression::Value { value, width: 1, .. } if value.to_u64() == Some(1));
    match e {
        ProtoExpression::Binary {
            x,
            op: Op::BitAnd,
            y,
            width: 1,
            ..
        } if is_one(y) => copied_bit(x),
        ProtoExpression::Binary {
            x,
            op: Op::BitAnd,
            y,
            width: 1,
            ..
        } if is_one(x) => copied_bit(y),
        e => one_bit_variable(e),
    }
}

fn one_bit_variable(e: &ProtoExpression) -> Option<VarOffset> {
    match e {
        ProtoExpression::Variable {
            var_offset,
            select: None,
            dynamic_select: None,
            width: 1,
            var_full_width: 1,
            ..
        } if !var_offset.is_ff() => Some(*var_offset),
        _ => None,
    }
}

/// Sets `master_gated` on each comb clock whose comb closure reads a master
/// input clock, and returns the masters some gated clock's closure reads.  A
/// master is an input clock offset no statement writes; FF reads end the
/// closure.  The clocks' closures overlap heavily, so each walk visits a
/// statement once for all of them.
fn mark_master_gated(
    clocks: &mut [DerivedClock],
    pre_jit_stmts: &[ProtoStatement],
    output_to_writer: &HashMap<VarOffset, Writers>,
    input_clock_offsets: &HashMap<VarOffset, VarId>,
) -> SmallVec<[VarId; 4]> {
    let comb_writers = |clk: &DerivedClock| -> &[usize] {
        if clk.current_offset.is_ff() {
            return &[];
        }
        output_to_writer
            .get(&clk.current_offset)
            .map_or(&[][..], |w| w.as_slice())
    };

    // The union of the closures, with each member's comb reads.
    let mut reads: HashMap<usize, Vec<VarOffset>> = HashMap::default();
    let mut stack: Vec<usize> = clocks.iter().flat_map(comb_writers).copied().collect();
    let mut scratch_out: Vec<VarOffset> = Vec::new();
    while let Some(i) = stack.pop() {
        if reads.contains_key(&i) {
            continue;
        }
        let Some(stmt) = pre_jit_stmts.get(i) else {
            continue;
        };
        let mut ins: Vec<VarOffset> = Vec::new();
        scratch_out.clear();
        stmt.gather_variable_offsets(&mut ins, &mut scratch_out);
        ins.retain(|o| !o.is_ff());
        ins.sort_unstable_by_key(|o| o.raw());
        ins.dedup();
        for off in &ins {
            if let Some(writers) = output_to_writer.get(off) {
                stack.extend(writers.iter().filter(|w| !reads.contains_key(w)));
            }
        }
        reads.insert(i, ins);
    }

    // Forward from the members reading a master, along the closure's edges.
    let mut readers: HashMap<usize, Vec<usize>> = HashMap::default();
    let mut reached: HashSet<usize> = HashSet::default();
    let mut work: Vec<usize> = Vec::new();
    for (&i, ins) in &reads {
        for off in ins {
            match output_to_writer.get(off) {
                Some(writers) => {
                    for &w in writers {
                        readers.entry(w).or_default().push(i);
                    }
                }
                None => {
                    if input_clock_offsets.contains_key(off) && reached.insert(i) {
                        work.push(i);
                    }
                }
            }
        }
    }
    while let Some(w) = work.pop() {
        for &r in readers.get(&w).map_or(&[][..], |r| r.as_slice()) {
            if reached.insert(r) {
                work.push(r);
            }
        }
    }

    let mut stack: Vec<usize> = Vec::new();
    for clk in clocks.iter_mut() {
        let writers = comb_writers(clk);
        if writers.iter().any(|w| reached.contains(w)) {
            clk.master_gated = true;
            stack.extend_from_slice(writers);
        }
    }
    // The masters the gated clocks' closures read.
    let mut seen: HashSet<usize> = HashSet::default();
    let mut found: SmallVec<[VarId; 4]> = SmallVec::new();
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        for off in reads.get(&i).map_or(&[][..], |r| r.as_slice()) {
            match output_to_writer.get(off) {
                Some(writers) => stack.extend(writers.iter().filter(|w| !seen.contains(w))),
                None => {
                    if let Some(&vid) = input_clock_offsets.get(off)
                        && !found.contains(&vid)
                    {
                        found.push(vid);
                    }
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Value;
    use crate::ir::expression::ExpressionContext;

    fn ctx() -> ExpressionContext {
        ExpressionContext {
            width: 1,
            signed: false,
        }
    }

    fn bit(offset: isize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset: VarOffset::Comb(offset),
            select: None,
            dynamic_select: None,
            width: 1,
            var_full_width: 1,
            expr_context: ctx(),
        }
    }

    fn and(x: ProtoExpression, y: ProtoExpression) -> ProtoExpression {
        ProtoExpression::Binary {
            x: Box::new(x),
            op: Op::BitAnd,
            y: Box::new(y),
            width: 1,
            expr_context: ctx(),
        }
    }

    fn one() -> ProtoExpression {
        ProtoExpression::Value {
            value: Value::new(1, 1, false),
            width: 1,
            expr_context: ctx(),
        }
    }

    fn assign(dst: isize, expr: ProtoExpression) -> ProtoStatement {
        ProtoStatement::Assign(crate::ir::ProtoAssignStatement {
            dst: VarOffset::Comb(dst),
            dst_width: 1,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr,
            dst_ff_current_offset: -1,
            comb_direct: false,
            token: veryl_parser::token_range::TokenRange::default(),
        })
    }

    fn comb_clock(id: u32, offset: isize) -> DerivedClock {
        DerivedClock {
            var_id: VarId::from_raw(id),
            current_offset: VarOffset::Comb(offset),
            native_bytes: 1,
            master_gated: false,
            negedge: false,
        }
    }

    #[test]
    fn a_clock_is_master_gated_when_its_closure_reads_an_undriven_input_clock() {
        // Input clocks at 0 and 8, but 8 is driven, so only 0 is a master.
        // 40 reaches it through 32, 56 through 40, and 48 not at all.
        let stmts = vec![
            assign(8, bit(16)),
            assign(32, and(bit(0), bit(16))),
            assign(40, and(bit(32), bit(16))),
            assign(48, bit(16)),
            assign(56, and(bit(40), bit(8))),
        ];
        let mut output_to_writer: HashMap<VarOffset, Writers> = HashMap::default();
        for (i, s) in stmts.iter().enumerate() {
            let ProtoStatement::Assign(a) = s else {
                unreachable!()
            };
            output_to_writer.entry(a.dst).or_default().push(i);
        }
        let input_clock_offsets: HashMap<VarOffset, VarId> = [
            (VarOffset::Comb(0), VarId::from_raw(1)),
            (VarOffset::Comb(8), VarId::from_raw(2)),
        ]
        .into_iter()
        .collect();
        let mut clocks = vec![comb_clock(10, 40), comb_clock(11, 48), comb_clock(12, 56)];
        let masters =
            mark_master_gated(&mut clocks, &stmts, &output_to_writer, &input_clock_offsets);
        let gated: Vec<bool> = clocks.iter().map(|c| c.master_gated).collect();
        assert_eq!(gated, [true, false, true]);
        assert_eq!(masters.as_slice(), [VarId::from_raw(1)]);
    }

    #[test]
    fn the_master_cone_declines_when_an_earlier_statement_reads_a_cone_write() {
        // 24 is gated from the master at 0; 16 reads 24 before it is written,
        // so a full settle needs a second pass the cone alone would skip.
        let schedule = DerivedClockSchedule {
            master_input_clocks: [VarId::from_raw(1)].into_iter().collect(),
            ..Default::default()
        };
        let input_clock_offsets: HashMap<VarOffset, VarId> =
            [(VarOffset::Comb(0), VarId::from_raw(1))]
                .into_iter()
                .collect();
        let gate = assign(24, and(bit(0), bit(8)));
        let filler: Vec<ProtoStatement> = (0..16).map(|k| assign(100 + 8 * k, bit(400))).collect();
        let mut alone = vec![gate.clone()];
        alone.extend(filler.iter().cloned());
        let partitions = HashMap::default();
        let (cone, _) =
            master_cone(&schedule, &alone, &input_clock_offsets, &partitions).expect("a cone");
        assert_eq!(cone.len(), 1);
        let mut late = vec![assign(16, bit(24)), gate];
        late.extend(filler);
        assert!(master_cone(&schedule, &late, &input_clock_offsets, &partitions).is_none());
    }

    #[test]
    fn a_reset_reads_the_master_through_its_comb() {
        // The net at 24 qualifies a reset with the master at 0; 32 does not.
        let stmts = vec![assign(24, and(bit(0), bit(16))), assign(32, bit(16))];
        let mut output_to_writer: HashMap<VarOffset, Writers> = HashMap::default();
        output_to_writer
            .entry(VarOffset::Comb(24))
            .or_default()
            .push(0);
        output_to_writer
            .entry(VarOffset::Comb(32))
            .or_default()
            .push(1);
        let input_clock_offsets: HashMap<VarOffset, VarId> =
            [(VarOffset::Comb(0), VarId::from_raw(1))]
                .into_iter()
                .collect();
        let masters = [VarId::from_raw(1)];
        let reads = |o: isize| {
            reads_master(
                std::iter::once(VarOffset::Comb(o)),
                &masters,
                &stmts,
                &output_to_writer,
                &input_clock_offsets,
            )
        };
        assert!(reads(24));
        assert!(!reads(32));
        // A reset copied from the master is the master itself.
        assert!(reads(0));
    }

    #[test]
    fn a_copy_wrapped_once_per_inlined_link_is_still_a_copy() {
        let stacked = and(and(and(bit(8), one()), one()), one());
        assert_eq!(copied_bit(&stacked), Some(VarOffset::Comb(8)));
        assert_eq!(
            copied_bit(&and(one(), and(bit(8), one()))),
            Some(VarOffset::Comb(8))
        );
    }

    #[test]
    fn a_gate_under_the_wrappers_is_not_a_copy() {
        let gated = and(and(bit(8), bit(16)), one());
        assert_eq!(copied_bit(&gated), None);
        assert_eq!(copied_bit(&and(and(bit(8), one()), bit(16))), None);
        let inverted = ProtoExpression::Unary {
            op: Op::BitNot,
            x: Box::new(bit(8)),
            width: 1,
            expr_context: ctx(),
        };
        assert_eq!(copied_bit(&and(inverted, one())), None);
        let wide = ProtoExpression::Variable {
            var_offset: VarOffset::Comb(8),
            select: None,
            dynamic_select: None,
            width: 8,
            var_full_width: 8,
            expr_context: ctx(),
        };
        assert_eq!(copied_bit(&and(wide, one())), None);
    }
}
