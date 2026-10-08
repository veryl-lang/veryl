//! Dataflow settle planning (`VERYL_DATAFLOW=1`).
//!
//! The comb list is regrouped into CELLS that a settle runs only when a byte
//! they read has changed since their last run.  The change is signalled by
//! the writer: a cell compares its boundary outputs around its run and marks
//! the cells that read them, and the simulator marks the readers of what an
//! event or a commit changed.  Nothing compares on the consumer side, so a
//! cell nobody marked costs nothing.
//!
//! Soundness rests on every comb byte having exactly one writing cell.  A
//! NODE is the union of all writers of a variable, plus every statement
//! between two of those writers that reads bits a later writer still changes
//! (it consumes an intermediate value, so it must stay between them); the
//! node graph's cycles are collapsed.
//! Inside a node the statements keep their original order, and a read after
//! a variable's last writer only needs that writer's node to run first.  A
//! skipped cell therefore always holds its own final values, whoever reads
//! them and whenever.
//!
//! Cells group nodes whose transitive state inputs (FF spans and comb spans
//! no statement writes) agree under a MinHash signature: such nodes are
//! dirtied by the same events, so grouping them saves dispatches without
//! running much extra.  A node joins a cell only when that keeps the cell
//! order topological.

use super::cone_gate::{ConeGateInputs, has_side_effects};
use crate::ir::big_array::BigArrayFold;
use crate::ir::expression::ProtoExpression;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;
use crate::{HashMap, HashSet};

/// `VERYL_DATAFLOW_DIAG=1`, read once: the runtime asks on hot paths.
pub(crate) fn diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_DATAFLOW_DIAG").as_deref() == Ok("1"))
}

fn bail<T>(why: std::fmt::Arguments) -> Option<T> {
    if diag() {
        eprintln!("[dataflow] declined: {why}");
    }
    None
}

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_DATAFLOW").as_deref() == Ok("1"))
}

/// Why a module built under `config` with this pass count gets no dataflow
/// settle, judged before planning; `None` when it is planned.
/// `VERYL_DATAFLOW=0` is not a reason here: callers check [`enabled`].
pub(crate) fn decline_reason(
    config: &crate::ir::Config,
    required_comb_passes: usize,
) -> Option<String> {
    if !config.aot_c {
        Some("aot_c off".into())
    } else if !crate::component::loader::native_loading_supported() {
        Some("native loading unsupported".into())
    } else if config.use_4state {
        Some("4-state".into())
    } else if required_comb_passes > 1 {
        Some(format!("{required_comb_passes} comb passes required"))
    } else {
        None
    }
}

/// Statements a cell may hold before it stops absorbing co-active nodes.
const CELL_CAP_STMTS: u32 = 64;

/// A byte range and the cells that read it.
pub type Readers = Vec<(u32, u32, Vec<u32>)>;

#[derive(Clone, Debug, Default)]
pub struct DataflowPlan {
    /// Statement indices per cell, in execution order.  Cells are numbered
    /// in a topological order: a cell only marks cells after it.
    pub cells: Vec<Vec<u32>>,
    /// Comb ranges each cell writes that a later cell reads.
    pub boundary: Vec<Vec<(u32, u32)>>,
    /// `(word, mask)` of the cells reading each cell's boundary outputs.
    pub consumers: Vec<Vec<(u32, u64)>>,
    /// Cells holding a print, assert or `$finish`: dirty at every settle.
    pub always_run: Vec<u32>,
    /// Cells that write FF storage another cell reads (a comb assignment to
    /// an element of an otherwise sequential array).  Their boundary is not
    /// in the comb buffer, so they mark their consumers on every run.
    pub mark_always: Vec<bool>,
    /// Comb ranges any cell reads, with the reading cells.  Sorted by start,
    /// disjoint.
    pub comb_readers: Readers,
    /// `comb_readers` plus the cells writing each range: an outside write
    /// also has to rerun a co-writer, or the outside value stands.
    pub comb_marked: Readers,
    /// The FF counterpart.
    pub ff_marked: Readers,
    /// Every comb range a cell writes, sorted, with its writing cell.
    pub written: Vec<(u32, u32)>,
    /// Every FF range a cell writes, sorted (checked by `VERYL_DATAFLOW_CHECK`).
    pub written_ff: Vec<(u32, u32)>,
    pub writer_cell: Vec<u32>,
    /// What each cell reads, `(is_ff, start, end)` (diagnostics).
    pub cell_inputs: Vec<Vec<(bool, u32, u32)>>,
}

impl DataflowPlan {
    pub fn words(&self) -> usize {
        self.cells.len().div_ceil(64)
    }

    /// Dirty buffer length in words: the shadow holds every boundary range,
    /// in cell order.
    pub fn buffer_words(&self) -> usize {
        let shadow: usize = self
            .boundary
            .iter()
            .flatten()
            .map(|&(a, b)| (b - a) as usize)
            .sum();
        self.words() + shadow.div_ceil(8)
    }

    /// `comb_marked` with each cell list folded to `(word, mask)` pairs, for
    /// code that marks them at a store.
    pub fn store_marks(&self) -> MarkTable {
        fold_marks(&self.comb_marked)
    }

    /// `store_marks` for FF storage, marked at the log push.
    pub fn ff_store_marks(&self) -> MarkTable {
        fold_marks(&self.ff_marked)
    }
}

fn fold_marks(table: &Readers) -> MarkTable {
    table
        .iter()
        .map(|(a, b, readers)| {
            let mut words: Vec<(u32, u64)> = Vec::new();
            for &c in readers {
                let (w, m) = (c / 64, 1u64 << (c % 64));
                match words.iter_mut().find(|x| x.0 == w) {
                    Some(x) => x.1 |= m,
                    None => words.push((w, m)),
                }
            }
            (*a, *b, words)
        })
        .collect()
}

struct StmtIo {
    in_comb: Vec<(usize, usize)>,
    in_ff: Vec<(usize, usize)>,
    out_comb: Vec<(usize, usize)>,
    /// FF spans written, keyed by the current slot readers see.
    out_ff: Vec<(usize, usize)>,
    /// FF bytes actually written, next slots included.
    out_ff_raw: Vec<(usize, usize)>,
    /// Prints, asserts or finishes: must run every settle, in order.
    effect: bool,
}

/// Whether `reader` reads only bits of span `key` (`[a, b)`) that none of
/// `writers` writes.  Both sides come from the statements themselves: each
/// access's own width and select, every element for a runtime index.
fn disjoint_bits<'a>(
    reader: &ProtoStatement,
    writers: impl Iterator<Item = &'a ProtoStatement>,
    key: u64,
    (a, b): (usize, usize),
) -> bool {
    let is_ff = key >> 63 == 1;
    let in_span = |v: &[(VarOffset, usize, usize)]| -> Vec<(usize, usize)> {
        v.iter()
            .filter(|(o, ..)| o.is_ff() == is_ff && o.raw() >= 0)
            .filter(|(o, ..)| (a..b).contains(&(o.raw() as usize)))
            .map(|&(o, lo, hi)| {
                let base = (o.raw() as usize - a) * 8;
                (base + lo, base + hi)
            })
            .collect()
    };
    let mut r = Vec::new();
    if !read_bits(reader, &mut r) {
        return false;
    }
    let mut w = Vec::new();
    for s in writers {
        if !write_bits(s, &mut w) {
            return false;
        }
    }
    let (read, written) = (in_span(&r), in_span(&w));
    // Nothing found in the span means the walk could not place the access.
    !read.is_empty()
        && !written.is_empty()
        && read
            .iter()
            .all(|&(rl, rh)| written.iter().all(|&(wl, wh)| rh < wl || wh < rl))
}

/// The bits `e` reads, `(variable, lo, hi)` relative to each variable.  False
/// for an access it cannot place.
fn expr_read_bits(e: &ProtoExpression, out: &mut Vec<(VarOffset, usize, usize)>) -> bool {
    match e {
        ProtoExpression::Value { .. } => true,
        ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select,
            var_full_width,
            ..
        } => {
            let full = (0, var_full_width.max(&1) - 1);
            let (lo, hi) = match (select, dynamic_select) {
                (Some((hi, lo)), None) => (*lo, *hi),
                _ => full,
            };
            out.push((*var_offset, lo, hi));
            dynamic_select
                .as_ref()
                .is_none_or(|d| expr_read_bits(&d.index_expr, out))
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
            for i in 0..*num_elements {
                let off =
                    VarOffset::new(base_offset.is_ff(), base_offset.raw() + stride * i as isize);
                out.push((off, 0, element_native_bytes.max(&1) * 8 - 1));
            }
            expr_read_bits(index_expr, out)
                && dynamic_select
                    .as_ref()
                    .is_none_or(|d| expr_read_bits(&d.index_expr, out))
        }
        ProtoExpression::Unary { x, .. } | ProtoExpression::Resize { x, .. } => {
            expr_read_bits(x, out)
        }
        ProtoExpression::Binary { x, y, .. } => expr_read_bits(x, out) && expr_read_bits(y, out),
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            expr_read_bits(cond, out)
                && expr_read_bits(true_expr, out)
                && expr_read_bits(false_expr, out)
        }
        ProtoExpression::Concatenation { elements, .. } => {
            elements.iter().all(|(e, _, _)| expr_read_bits(e, out))
        }
        ProtoExpression::HierVariable(_) => false,
    }
}

/// `expr_read_bits` over every expression `s` evaluates.
fn read_bits(s: &ProtoStatement, out: &mut Vec<(VarOffset, usize, usize)>) -> bool {
    match s {
        ProtoStatement::Assign(x) => {
            expr_read_bits(&x.expr, out)
                && x.dynamic_select
                    .as_ref()
                    .is_none_or(|d| expr_read_bits(&d.index_expr, out))
        }
        ProtoStatement::AssignDynamic(x) => {
            expr_read_bits(&x.expr, out)
                && expr_read_bits(&x.dst_index_expr, out)
                && x.dynamic_select
                    .as_ref()
                    .is_none_or(|d| expr_read_bits(&d.index_expr, out))
        }
        ProtoStatement::If(x) => {
            x.cond.as_ref().is_none_or(|c| expr_read_bits(c, out))
                && x.true_side
                    .iter()
                    .chain(x.false_side.iter())
                    .all(|s| read_bits(s, out))
        }
        ProtoStatement::Case(x) => {
            x.arms
                .iter()
                .all(|a| expr_read_bits(&a.cond, out) && a.body.iter().all(|s| read_bits(s, out)))
                && x.default.iter().all(|s| read_bits(s, out))
        }
        ProtoStatement::SequentialBlock(b) => b.iter().all(|s| read_bits(s, out)),
        ProtoStatement::Break => true,
        _ => false,
    }
}

/// The bits each assignment in `s` writes, `(target, lo, hi)` relative to the
/// target, from the assignment's own width and select.  False when a write
/// cannot be bounded that way (a runtime index).
fn write_bits(s: &ProtoStatement, out: &mut Vec<(VarOffset, usize, usize)>) -> bool {
    match s {
        ProtoStatement::Assign(x) => {
            let (hi, lo) = x.select.unwrap_or((x.dst_width.max(1) - 1, 0));
            out.push((x.dst, lo, hi));
            true
        }
        ProtoStatement::If(x) => x
            .true_side
            .iter()
            .chain(x.false_side.iter())
            .all(|s| write_bits(s, out)),
        ProtoStatement::Case(x) => x
            .arms
            .iter()
            .flat_map(|a| a.body.iter())
            .chain(x.default.iter())
            .all(|s| write_bits(s, out)),
        ProtoStatement::For(x) => {
            out.push((x.var_offset, 0, x.var_native_bytes.max(1) * 8 - 1));
            x.body.iter().all(|s| write_bits(s, out))
        }
        ProtoStatement::SequentialBlock(b) => b.iter().all(|s| write_bits(s, out)),
        ProtoStatement::SystemFunctionCall(_) | ProtoStatement::Break => true,
        _ => false,
    }
}

/// The storage of every loop variable in `s`, which the loop stores at each
/// iteration and the offset gathers do not report.
pub fn loop_vars(s: &ProtoStatement, out: &mut Vec<VarOffset>) {
    match s {
        ProtoStatement::For(x) => {
            out.push(x.var_offset);
            x.body.iter().for_each(|s| loop_vars(s, out));
        }
        ProtoStatement::If(x) => x
            .true_side
            .iter()
            .chain(x.false_side.iter())
            .for_each(|s| loop_vars(s, out)),
        ProtoStatement::Case(x) => x
            .arms
            .iter()
            .flat_map(|a| a.body.iter())
            .chain(x.default.iter())
            .for_each(|s| loop_vars(s, out)),
        ProtoStatement::SequentialBlock(b) => b.iter().for_each(|s| loop_vars(s, out)),
        _ => {}
    }
}

/// Side effects a cell can carry by running every settle: output, checks and
/// `$finish`, none of which writes state the plan would have to track.
fn output_effects_only(s: &ProtoStatement) -> bool {
    use crate::ir::statement::ProtoSystemFunctionCall;
    match s {
        ProtoStatement::Assign(_) | ProtoStatement::AssignDynamic(_) | ProtoStatement::Break => {
            true
        }
        ProtoStatement::If(x) => x
            .true_side
            .iter()
            .chain(x.false_side.iter())
            .all(output_effects_only),
        ProtoStatement::Case(x) => x
            .arms
            .iter()
            .flat_map(|a| a.body.iter())
            .chain(x.default.iter())
            .all(output_effects_only),
        ProtoStatement::For(x) => x.body.iter().all(output_effects_only),
        ProtoStatement::SequentialBlock(b) => b.iter().all(output_effects_only),
        ProtoStatement::SystemFunctionCall(f) => {
            !matches!(f, ProtoSystemFunctionCall::Readmemh { .. })
        }
        _ => false,
    }
}

/// Span keys shared by comb and FF spans: FF starts carry the top bit.
fn key_comb(a: usize) -> u64 {
    a as u64
}

fn key_ff(a: usize) -> u64 {
    (a as u64) | (1 << 63)
}

impl StmtIo {
    fn ins(&self) -> impl Iterator<Item = u64> + '_ {
        let c = self.in_comb.iter().map(|&(a, _)| key_comb(a));
        c.chain(self.in_ff.iter().map(|&(a, _)| key_ff(a)))
    }

    fn in_spans(&self) -> impl Iterator<Item = (u64, (usize, usize))> + '_ {
        let c = self.in_comb.iter().map(|&sp| (key_comb(sp.0), sp));
        c.chain(self.in_ff.iter().map(|&sp| (key_ff(sp.0), sp)))
    }

    fn outs(&self) -> impl Iterator<Item = u64> + '_ {
        let c = self.out_comb.iter().map(|&(a, _)| key_comb(a));
        c.chain(self.out_ff.iter().map(|&(a, _)| key_ff(a)))
    }
}

/// `None` when a statement falls outside what the plan can track: an access
/// outside every owner span or a side effect.
fn stmt_io(stmts: &[ProtoStatement], inputs: &ConeGateInputs) -> Option<Vec<StmtIo>> {
    // A byte must belong to one span, or a writer would mark the readers of
    // one span while a reader of the same bytes is keyed on another.  Owner
    // spans can nest (a folded array's span over bytes another entry also
    // claims), so overlapping spans merge into one.
    let mut owner: Vec<(usize, usize)> =
        inputs.comb_owner.iter().map(|&(s, e, _)| (s, e)).collect();
    owner.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(owner.len());
    for (s, e) in owner {
        match merged.last_mut() {
            Some(last) if s < last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let span_in = |x: usize| -> Option<(usize, usize)> {
        let i = merged.partition_point(|&(s, _)| s <= x).checked_sub(1)?;
        let (s, e) = merged[i];
        (x < e).then_some((s, e))
    };
    let fold = BigArrayFold::from_statements(stmts.iter());
    let mut ins: Vec<VarOffset> = Vec::new();
    let mut outs: Vec<VarOffset> = Vec::new();
    let mut io = Vec::with_capacity(stmts.len());
    for (i, s) in stmts.iter().enumerate() {
        let effect = has_side_effects(s);
        if effect && !output_effects_only(s) {
            return bail(format_args!("statement {i} has a side effect on state"));
        }
        ins.clear();
        outs.clear();
        s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
        loop_vars(s, &mut outs);
        let mut x = StmtIo {
            in_comb: Vec::new(),
            in_ff: Vec::new(),
            out_comb: Vec::new(),
            out_ff: Vec::new(),
            out_ff_raw: Vec::new(),
            effect,
        };
        for o in &outs {
            match o {
                VarOffset::Comb(off) if *off >= 0 => match span_in(*off as usize) {
                    Some(sp) => x.out_comb.push(sp),
                    None => {
                        return bail(format_args!("statement {i} writes untracked comb {off:#x}"));
                    }
                },
                VarOffset::Ff(off) if *off >= 0 => {
                    let off = *off as usize;
                    // A dual-slot element is written at its next slot too;
                    // readers see the current one, so that is its span.
                    let next = inputs
                        .ff_next_alias
                        .binary_search_by_key(&off, |&(n, _)| n)
                        .ok()
                        .map(|k| inputs.ff_next_alias[k].1);
                    match inputs.ff_owner.span(next.unwrap_or(off)) {
                        Some((a, b)) => {
                            x.out_ff.push((a, b));
                            let at = if next.is_some() { off } else { a };
                            x.out_ff_raw.push((at, at + (b - a)));
                        }
                        None => {
                            return bail(format_args!(
                                "statement {i} writes untracked ff {off:#x}"
                            ));
                        }
                    }
                }
                _ => return bail(format_args!("statement {i} writes {o:?}")),
            }
        }
        for o in &ins {
            match o {
                VarOffset::Comb(off) if *off >= 0 => match span_in(*off as usize) {
                    Some(sp) => x.in_comb.push(sp),
                    None => {
                        return bail(format_args!("statement {i} reads untracked comb {off:#x}"));
                    }
                },
                VarOffset::Ff(off) if *off >= 0 => match inputs.ff_owner.span(*off as usize) {
                    Some(sp) => x.in_ff.push(sp),
                    None => return bail(format_args!("statement {i} reads untracked ff {off:#x}")),
                },
                _ => return bail(format_args!("statement {i} reads {o:?}")),
            }
        }
        for v in [
            &mut x.in_comb,
            &mut x.in_ff,
            &mut x.out_comb,
            &mut x.out_ff,
            &mut x.out_ff_raw,
        ] {
            v.sort_unstable();
            v.dedup();
        }
        io.push(x);
    }
    Some(io)
}

fn find(uf: &mut [u32], mut x: u32) -> u32 {
    while uf[x as usize] != x {
        uf[x as usize] = uf[uf[x as usize] as usize];
        x = uf[x as usize];
    }
    x
}

fn union(uf: &mut [u32], a: u32, b: u32) {
    let (x, y) = (find(uf, a), find(uf, b));
    if x != y {
        uf[x as usize] = y;
    }
}

/// Strongly connected components, numbered in reverse topological order of
/// discovery (iterative Tarjan).
fn scc(adj: &[Vec<u32>]) -> Vec<u32> {
    let n = adj.len();
    let mut index = vec![u32::MAX; n];
    let mut low = vec![0u32; n];
    let mut on = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut comp = vec![u32::MAX; n];
    let (mut ncomp, mut next) = (0u32, 0u32);
    let mut call: Vec<(u32, usize)> = Vec::new();
    for s0 in 0..n as u32 {
        if index[s0 as usize] != u32::MAX {
            continue;
        }
        call.push((s0, 0));
        while let Some(&mut (v, ref mut k)) = call.last_mut() {
            let vu = v as usize;
            if *k == 0 && index[vu] == u32::MAX {
                index[vu] = next;
                low[vu] = next;
                next += 1;
                stack.push(v);
                on[vu] = true;
            }
            if *k < adj[vu].len() {
                let w = adj[vu][*k];
                *k += 1;
                if index[w as usize] == u32::MAX {
                    call.push((w, 0));
                } else if on[w as usize] {
                    low[vu] = low[vu].min(index[w as usize]);
                }
                continue;
            }
            if low[vu] == index[vu] {
                loop {
                    let w = stack.pop().expect("tarjan stack holds v");
                    on[w as usize] = false;
                    comp[w as usize] = ncomp;
                    if w == v {
                        break;
                    }
                }
                ncomp += 1;
            }
            call.pop();
            if let Some(&(p, _)) = call.last() {
                low[p as usize] = low[p as usize].min(low[vu]);
            }
        }
    }
    comp
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

pub fn plan(stmts: &[ProtoStatement], inputs: &ConeGateInputs) -> Option<DataflowPlan> {
    let n = stmts.len();
    if n == 0 {
        return None;
    }
    let io = stmt_io(stmts, inputs)?;

    // Writers per span, in statement order.
    let mut writers: HashMap<u64, Vec<u32>> = HashMap::default();
    for (i, x) in io.iter().enumerate() {
        for k in x.outs() {
            writers.entry(k).or_default().push(i as u32);
        }
    }

    // Nodes: all writers of a span, plus the readers between them.  A read
    // BEFORE the first writer consumes the previous settle's value, which a
    // reordered schedule cannot reproduce.
    let mut uf: Vec<u32> = (0..n as u32).collect();
    for w in writers.values() {
        for p in w.windows(2) {
            union(&mut uf, p[0], p[1]);
        }
    }
    // A read before the first writer is fine when the bits it reads are
    // ones no writer of the span touches; such a pair orders nothing.
    let mut unordered: HashSet<(u32, u64)> = HashSet::default();
    for (r, x) in io.iter().enumerate() {
        let r = r as u32;
        for (a, span) in x.in_spans() {
            let Some(w) = writers.get(&a) else { continue };
            if w.binary_search(&r).is_ok() {
                continue;
            }
            let (lo, hi) = (w[0], w[w.len() - 1]);
            if r < lo {
                if disjoint_bits(
                    &stmts[r as usize],
                    w.iter().map(|&i| &stmts[i as usize]),
                    a,
                    span,
                ) {
                    unordered.insert((r, a));
                    continue;
                }
                return bail(format_args!(
                    "statement {r} reads span {a:#x} before its first writer {lo}"
                ));
            }
            // Between the writers, only a read of bits a later writer still
            // changes sees an intermediate value; any other read sees the
            // final one, so it can run after the writers like a later read.
            if r < hi {
                let later = &w[w.partition_point(|&i| i < r)..];
                if !disjoint_bits(
                    &stmts[r as usize],
                    later.iter().map(|&i| &stmts[i as usize]),
                    a,
                    span,
                ) {
                    union(&mut uf, lo, r);
                }
            }
        }
    }
    let mut node_id = vec![u32::MAX; n];
    let mut node = vec![0u32; n];
    let mut nn = 0u32;
    for (i, slot) in node.iter_mut().enumerate() {
        let root = find(&mut uf, i as u32) as usize;
        if node_id[root] == u32::MAX {
            node_id[root] = nn;
            nn += 1;
        }
        *slot = node_id[root];
    }
    let nn = nn as usize;
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); nn];
    for (r, x) in io.iter().enumerate() {
        for a in x.ins() {
            if unordered.contains(&(r as u32, a)) {
                continue;
            }
            for &w in writers.get(&a).into_iter().flatten() {
                let (u, v) = (node[w as usize], node[r]);
                if u != v {
                    adj[u as usize].push(v);
                }
            }
        }
    }
    // Effects keep their schedule order.
    let effects: Vec<usize> = (0..n).filter(|&i| io[i].effect).collect();
    for p in effects.windows(2) {
        let (u, v) = (node[p[0]], node[p[1]]);
        if u != v {
            adj[u as usize].push(v);
        }
    }
    for a in &mut adj {
        a.sort_unstable();
        a.dedup();
    }
    let comp_of_node = scc(&adj);
    let nc = comp_of_node
        .iter()
        .map(|&c| c as usize + 1)
        .max()
        .unwrap_or(0);
    let comp: Vec<u32> = node.iter().map(|&v| comp_of_node[v as usize]).collect();
    let mut csize = vec![0u32; nc];
    let mut cfirst = vec![u32::MAX; nc];
    for (i, &c) in comp.iter().enumerate() {
        csize[c as usize] += 1;
        cfirst[c as usize] = cfirst[c as usize].min(i as u32);
    }

    // Component DAG in topological order, ties broken by first statement.
    let mut cadj: Vec<Vec<u32>> = vec![Vec::new(); nc];
    for (u, a) in adj.iter().enumerate() {
        for &v in a {
            let (cu, cv) = (comp_of_node[u], comp_of_node[v as usize]);
            if cu != cv {
                cadj[cu as usize].push(cv);
            }
        }
    }
    let mut indeg = vec![0u32; nc];
    for a in &mut cadj {
        a.sort_unstable();
        a.dedup();
        for &v in a.iter() {
            indeg[v as usize] += 1;
        }
    }
    // Per-component reads (minus its own outputs) and outputs, as span keys.
    let mut c_in: Vec<Vec<u64>> = vec![Vec::new(); nc];
    let mut c_out: Vec<Vec<u64>> = vec![Vec::new(); nc];
    for (i, x) in io.iter().enumerate() {
        let c = comp[i] as usize;
        c_out[c].extend(x.outs());
        c_in[c].extend(x.ins());
    }
    for c in 0..nc {
        c_out[c].sort_unstable();
        c_out[c].dedup();
        let outs = std::mem::take(&mut c_out[c]);
        c_in[c].sort_unstable();
        c_in[c].dedup();
        c_in[c].retain(|x| outs.binary_search(x).is_err());
        c_out[c] = outs;
    }
    let mut span_writer: HashMap<u64, u32> = HashMap::default();
    for (c, outs) in c_out.iter().enumerate() {
        for &x in outs {
            span_writer.insert(x, c as u32);
        }
    }

    // Co-activity signature: one MinHash of the transitive state inputs.  Two
    // statements share it with probability equal to the Jaccard similarity of
    // their input sets; requiring more hashes to agree splits a dense design
    // into cells so small that dispatch costs more than the work it skips.
    // A component's writers come before it, so its signature is known once it
    // is ready.
    let mut sig = vec![u64::MAX; nc];
    let signature = |c: usize, sig: &[u64]| -> u64 {
        let mut m = u64::MAX;
        for &x in &c_in[c] {
            m = m.min(match span_writer.get(&x) {
                Some(&w) => sig[w as usize],
                None => splitmix(x ^ (1u64 << 56)),
            });
        }
        m
    };

    // Topological order.  Among the ready components, one sharing the last
    // one's signature goes next, so co-active components sit together for
    // the bundling below; otherwise the first in statement order.
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    type Ready = BinaryHeap<Reverse<(u32, u32)>>;
    let mut any: Ready = BinaryHeap::new();
    let mut by_sig: HashMap<u64, Ready> = HashMap::default();
    let mut done = vec![false; nc];
    for c in 0..nc {
        if indeg[c] == 0 {
            sig[c] = signature(c, &sig);
            any.push(Reverse((cfirst[c], c as u32)));
            by_sig
                .entry(sig[c])
                .or_default()
                .push(Reverse((cfirst[c], c as u32)));
        }
    }
    let pop_live = |h: &mut Ready, done: &[bool]| -> Option<u32> {
        while let Some(Reverse((_, c))) = h.pop() {
            if !done[c as usize] {
                return Some(c);
            }
        }
        None
    };
    let mut topo: Vec<u32> = Vec::with_capacity(nc);
    let mut last: Option<u64> = None;
    loop {
        let next = last
            .and_then(|s| by_sig.get_mut(&s))
            .and_then(|h| pop_live(h, &done))
            .or_else(|| pop_live(&mut any, &done));
        let Some(c) = next else { break };
        done[c as usize] = true;
        topo.push(c);
        last = Some(sig[c as usize]);
        for &v in &cadj[c as usize] {
            let vu = v as usize;
            indeg[vu] -= 1;
            if indeg[vu] == 0 {
                sig[vu] = signature(vu, &sig);
                any.push(Reverse((cfirst[vu], v)));
                by_sig
                    .entry(sig[vu])
                    .or_default()
                    .push(Reverse((cfirst[vu], v)));
            }
        }
    }
    if topo.len() != nc {
        return bail(format_args!("component graph is cyclic"));
    }
    let mut cell_of = vec![u32::MAX; nc];
    let mut cell_size: Vec<u32> = Vec::new();
    let mut open: HashMap<u64, u32> = HashMap::default();
    // A component with an effect runs every settle, so it gets a cell of its
    // own rather than dragging co-active statements along.
    let mut c_effect = vec![false; nc];
    for &i in &effects {
        c_effect[comp[i] as usize] = true;
    }
    for &c in &topo {
        let cu = c as usize;
        if c_effect[cu] {
            cell_of[cu] = cell_size.len() as u32;
            cell_size.push(csize[cu]);
            continue;
        }
        let floor = c_in[cu]
            .iter()
            .filter_map(|x| span_writer.get(x))
            .map(|&w| cell_of[w as usize])
            .max();
        let k = match open.get(&sig[cu]) {
            Some(&k)
                if floor.is_none_or(|f| f <= k)
                    && cell_size[k as usize] + csize[cu] <= CELL_CAP_STMTS =>
            {
                k
            }
            _ => {
                let k = cell_size.len() as u32;
                cell_size.push(0);
                open.insert(sig[cu], k);
                k
            }
        };
        cell_of[cu] = k;
        cell_size[k as usize] += csize[cu];
    }
    let ncells = cell_size.len();
    let cell = |i: usize| cell_of[comp[i] as usize];
    let mut always_run: Vec<u32> = effects.iter().map(|&i| cell(i)).collect();
    always_run.sort_unstable();
    always_run.dedup();

    let mut cells: Vec<Vec<u32>> = vec![Vec::new(); ncells];
    for i in 0..n {
        cells[cell(i) as usize].push(i as u32);
    }

    // Readers per span, in cells.
    let mut comb_map: HashMap<(usize, usize), Vec<u32>> = HashMap::default();
    let mut ff_map: HashMap<(usize, usize), Vec<u32>> = HashMap::default();
    for (i, x) in io.iter().enumerate() {
        for &sp in &x.in_comb {
            comb_map.entry(sp).or_default().push(cell(i));
        }
        for &sp in &x.in_ff {
            ff_map.entry(sp).or_default().push(cell(i));
        }
    }
    let finish = |m: HashMap<(usize, usize), Vec<u32>>| -> Readers {
        let mut v: Readers = m
            .into_iter()
            .map(|((a, b), mut r)| {
                r.sort_unstable();
                r.dedup();
                (a as u32, b as u32, r)
            })
            .collect();
        v.sort_unstable_by_key(|x| x.0);
        v
    };
    let (mut comb_out, mut ff_out) = (comb_map.clone(), ff_map.clone());
    for (i, x) in io.iter().enumerate() {
        for &sp in &x.out_comb {
            comb_out.entry(sp).or_default().push(cell(i));
        }
        for &sp in &x.out_ff {
            ff_out.entry(sp).or_default().push(cell(i));
        }
    }
    let comb_readers = finish(comb_map);
    let ff_readers = finish(ff_map);
    let comb_marked = finish(comb_out);
    let ff_marked = finish(ff_out);

    let mut boundary: Vec<Vec<(u32, u32)>> = vec![Vec::new(); ncells];
    let mut consumers: Vec<Vec<(u32, u64)>> = vec![Vec::new(); ncells];
    let mut mark_always = vec![false; ncells];
    let spans = comb_readers
        .iter()
        .map(|r| (false, r))
        .chain(ff_readers.iter().map(|r| (true, r)));
    for (is_ff, (a, b, readers)) in spans {
        let key = if is_ff {
            key_ff(*a as usize)
        } else {
            key_comb(*a as usize)
        };
        let Some(&wc) = span_writer.get(&key) else {
            continue;
        };
        let wcell = cell_of[wc as usize];
        let mut any = false;
        for &rc in readers {
            // An earlier cell reads only bits this one leaves alone (see
            // `unordered`): nothing to mark.
            if rc <= wcell {
                continue;
            }
            any = true;
            let (w, m) = (rc / 64, 1u64 << (rc % 64));
            let list = &mut consumers[wcell as usize];
            match list.iter_mut().find(|x| x.0 == w) {
                Some(x) => x.1 |= m,
                None => list.push((w, m)),
            }
        }
        if any {
            if is_ff {
                mark_always[wcell as usize] = true;
            } else {
                boundary[wcell as usize].push((*a, *b));
            }
        }
    }
    for c in &mut consumers {
        c.sort_unstable();
    }
    // Abutting spans compare as one: the marks are per cell, not per span.
    for b in &mut boundary {
        b.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(b.len());
        for &(lo, hi) in b.iter() {
            match merged.last_mut() {
                Some(last) if last.1 >= lo => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        *b = merged;
    }

    let mut written: Vec<(u32, u32, u32)> = io
        .iter()
        .enumerate()
        .flat_map(|(i, x)| {
            x.out_comb
                .iter()
                .map(move |&(a, b)| (a as u32, b as u32, i))
        })
        .map(|(a, b, i)| (a, b, cell(i)))
        .collect();
    written.sort_unstable();
    written.dedup_by_key(|w| (w.0, w.1));
    let mut written_ff: Vec<(u32, u32)> = io
        .iter()
        .flat_map(|x| x.out_ff_raw.iter().map(|&(a, b)| (a as u32, b as u32)))
        .collect();
    written_ff.sort_unstable();
    written_ff.dedup();
    let writer_cell: Vec<u32> = written.iter().map(|w| w.2).collect();
    let written: Vec<(u32, u32)> = written.iter().map(|w| (w.0, w.1)).collect();
    let mut cell_inputs: Vec<Vec<(bool, u32, u32)>> = vec![Vec::new(); ncells];
    for (i, x) in io.iter().enumerate() {
        let c = &mut cell_inputs[cell(i) as usize];
        c.extend(x.in_comb.iter().map(|&(a, b)| (false, a as u32, b as u32)));
        c.extend(x.in_ff.iter().map(|&(a, b)| (true, a as u32, b as u32)));
    }
    for c in &mut cell_inputs {
        c.sort_unstable();
        c.dedup();
    }
    Some(DataflowPlan {
        cells,
        boundary,
        consumers,
        mark_always,
        written_ff,
        always_run,
        comb_readers,
        comb_marked,
        ff_marked,
        written,
        writer_cell,
        cell_inputs,
    })
}

/// A planned dataflow settle and its compiled module.
pub struct DataflowRt {
    pub plan: DataflowPlan,
    /// Empty until the `.so` lands; the settle stays on the regular path
    /// meanwhile.
    #[cfg(not(target_family = "wasm"))]
    pub cell: crate::backend::aot_c::emit::AotCell,
    /// `(offset, bytes)` kept in C locals, never stored to the buffer.
    pub localized: Vec<(isize, usize)>,
    /// The same cells compiled by Cranelift, run until the `.so` lands.
    #[cfg(not(target_family = "wasm"))]
    pub jit: Option<DataflowJit>,
}

/// One Cranelift function per cell, indexed like `DataflowPlan::cells`.
#[cfg(not(target_family = "wasm"))]
pub struct DataflowJit {
    /// `None` for a cell Cranelift could not compile; `interp` holds it.
    pub funcs: Vec<Option<crate::FuncPtr>>,
    /// The cells the dispatcher interprets, with their statements.
    pub interp: Vec<(usize, Vec<ProtoStatement>)>,
    _keepalive: Vec<memmap2::Mmap>,
}

/// Compile every cell with Cranelift, in parallel, to run until the `.so`
/// lands.
#[cfg(not(target_family = "wasm"))]
pub fn compile_jit(
    config: &crate::ir::Config,
    stmts: &[ProtoStatement],
    plan: &DataflowPlan,
) -> Option<DataflowJit> {
    use crate::backend::cranelift::runtime::{CellWatch, build_cell_binary};
    let t = std::time::Instant::now();
    let n = plan.cells.len();
    let body = |k: usize| -> Vec<ProtoStatement> {
        plan.cells[k]
            .iter()
            .map(|&i| stmts[i as usize].clone())
            .collect()
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |p| p.get())
        .clamp(1, 8);
    let per = n.div_ceil(threads).max(1);
    type Compiled = Option<(crate::FuncPtr, Option<memmap2::Mmap>)>;
    let results: Vec<Compiled> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..n)
            .step_by(per)
            .map(|lo| {
                std::thread::Builder::new()
                    .stack_size(crate::IR_WALK_STACK_BYTES)
                    .spawn_scoped(sc, move || {
                        (lo..(lo + per).min(n))
                            .map(|k| {
                                let watch = CellWatch {
                                    boundary: &plan.boundary[k],
                                    consumers: &plan.consumers[k],
                                    always: plan.mark_always[k],
                                };
                                build_cell_binary(config, body(k), &watch)
                            })
                            .collect::<Vec<Compiled>>()
                    })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.ok().and_then(|h| h.join().ok()).unwrap_or_default())
            .collect()
    });
    if results.len() != n {
        return None;
    }
    let mut funcs = Vec::with_capacity(n);
    let mut interp = Vec::new();
    let mut keep = Vec::new();
    for (k, r) in results.into_iter().enumerate() {
        match r {
            Some((f, m)) => {
                funcs.push(Some(f));
                keep.extend(m);
            }
            None => {
                funcs.push(None);
                interp.push((k, body(k)));
            }
        }
    }
    if diag() {
        eprintln!(
            "[dataflow] jit: {n} cells in {:.2} s, {} interpreted",
            t.elapsed().as_secs_f64(),
            interp.len()
        );
    }
    Some(DataflowJit {
        funcs,
        interp,
        _keepalive: keep,
    })
}

/// Dataflow settles by the comb pipeline's structural key: a module converted
/// again (another test, another instance) reuses the plan and the compile.
#[cfg(not(target_family = "wasm"))]
static CACHE: std::sync::Mutex<Option<HashMap<u128, std::sync::Arc<DataflowRt>>>> =
    std::sync::Mutex::new(None);

/// The `CACHE` key: the comb pipeline's key pins the statements and the plan,
/// so this adds what the compile reads besides them.  The cache outlives a
/// build session, so it sees every config a process builds with.
#[cfg(not(target_family = "wasm"))]
pub fn cache_key(
    comb_key: u128,
    config: &crate::ir::Config,
    localize: Option<&crate::backend::LocalizeInfo>,
) -> u128 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // An async cell lands later, with Cranelift cells built to bridge the gap.
    config.aot_c_async.hash(&mut h);
    // The FF layout the plan's FF spans and next-slot aliases come from.
    config.disable_ff_opt.hash(&mut h);
    // A dump comes from the compile that asked for it.
    config.dump_cranelift.hash(&mut h);
    config.dump_asm.hash(&mut h);
    // The bytes the emitter may keep in C locals.
    localize
        .map(|(block, ranges)| {
            let mut block: Vec<isize> = block.iter().copied().collect();
            block.sort_unstable();
            (block, ranges)
        })
        .hash(&mut h);
    comb_key ^ u128::from(h.finish())
}

#[cfg(not(target_family = "wasm"))]
pub fn cache_get(key: u128) -> Option<std::sync::Arc<DataflowRt>> {
    CACHE.lock().ok()?.as_ref()?.get(&key).cloned()
}

#[cfg(not(target_family = "wasm"))]
pub fn cache_put(key: u128, rt: std::sync::Arc<DataflowRt>) {
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(HashMap::default).insert(key, rt);
    }
}

/// A byte range `[lo, hi)` with the `(word, mask)` dirty bits of the cells
/// reading it.
pub type MarkSpan = (u32, u32, Vec<(u32, u64)>);
/// Mark spans sorted by start.
pub type MarkTable = Vec<MarkSpan>;

/// The marks event code sets: at comb stores, and at FF log pushes whose
/// value differs from the current one.
#[derive(Hash)]
pub struct EventMarks {
    pub comb: MarkTable,
    pub ff: MarkTable,
}

pub type StoreMarks = std::sync::Arc<EventMarks>;

/// The comb range a store statement can write, or `None` for anything else.
pub fn store_span(stmt: &ProtoStatement) -> Option<(u32, u32)> {
    match stmt {
        ProtoStatement::Assign(a) => match a.dst {
            VarOffset::Comb(off) if off >= 0 => Some((off as u32, off as u32 + 1)),
            _ => None,
        },
        ProtoStatement::AssignDynamic(a) => match a.dst_base {
            VarOffset::Comb(base) if base >= 0 => {
                let nb = crate::ir::native_bytes(a.dst_width).max(1);
                let (lo, hi) = crate::ir::strided_span(base, a.dst_stride, a.dst_num_elements, nb)?;
                Some((u32::try_from(lo.max(0)).ok()?, u32::try_from(hi).ok()?))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The marked ranges `[lo, hi)` overlaps: their combined span and the dirty
/// bits of all of them.  `None` when it overlaps none.
pub fn store_hits(marks: &StoreMarks, lo: u32, hi: u32) -> Option<MarkSpan> {
    table_hits(&marks.comb, lo, hi)
}

/// `store_hits` for the FF bytes `[lo, hi)` a log push writes.
pub fn ff_push_hits(marks: &StoreMarks, lo: u32, hi: u32) -> Option<MarkSpan> {
    table_hits(&marks.ff, lo, hi)
}

fn table_hits(table: &MarkTable, lo: u32, hi: u32) -> Option<MarkSpan> {
    let first = table.partition_point(|t| t.1 <= lo);
    let hit: Vec<&MarkSpan> = table[first..].iter().take_while(|t| t.0 < hi).collect();
    let (a, b) = (hit.first()?.0, hit.last()?.1);
    let mut words: Vec<(u32, u64)> = Vec::new();
    for (_, _, c) in &hit {
        for &(w, m) in c {
            match words.iter_mut().find(|x| x.0 == w) {
                Some(x) => x.1 |= m,
                None => words.push((w, m)),
            }
        }
    }
    Some((a, b, words))
}

/// Does `stmt` write a marked comb byte anywhere inside it?
pub fn writes_marked(stmt: &ProtoStatement, marks: &StoreMarks) -> bool {
    let (mut ins, mut outs) = (Vec::new(), Vec::new());
    stmt.gather_variable_offsets(&mut ins, &mut outs);
    outs.iter().any(|o| match o {
        VarOffset::Comb(off) if *off >= 0 => {
            store_hits(marks, *off as u32, *off as u32 + 1).is_some()
        }
        VarOffset::Comb(_) => true,
        _ => false,
    })
}

/// Does `stmt` write FF storage, or run code compiled without the marks (a
/// `CompiledBlock` reused from another instance)?  The Cranelift event codegen
/// marks the FF pushes it emits itself, nothing else.
pub fn writes_ff_unmarked(stmt: &ProtoStatement) -> bool {
    if matches!(stmt, ProtoStatement::CompiledBlock(_)) {
        return true;
    }
    let (mut ins, mut outs) = (Vec::new(), Vec::new());
    stmt.gather_variable_offsets(&mut ins, &mut outs);
    outs.iter().any(|o| o.is_ff())
}

/// Does `s` contain a statement matching `leaf`?
fn contains(s: &ProtoStatement, leaf: &dyn Fn(&ProtoStatement) -> bool) -> bool {
    leaf(s)
        || match s {
            ProtoStatement::If(x) => x
                .true_side
                .iter()
                .chain(x.false_side.iter())
                .any(|s| contains(s, leaf)),
            ProtoStatement::Case(x) => x
                .arms
                .iter()
                .flat_map(|a| a.body.iter())
                .chain(x.default.iter())
                .any(|s| contains(s, leaf)),
            ProtoStatement::For(x) => x.body.iter().any(|s| contains(s, leaf)),
            ProtoStatement::SequentialBlock(v) => v.iter().any(|s| contains(s, leaf)),
            ProtoStatement::CompiledBlock(x) => x.original_stmts.iter().any(|s| contains(s, leaf)),
            _ => false,
        }
}

/// `$readmemh` stores its elements directly, with no mark and no log push,
/// and `gather_variable_offsets` reports none of them.
fn is_readmemh(s: &ProtoStatement) -> bool {
    matches!(
        s,
        ProtoStatement::SystemFunctionCall(crate::ir::ProtoSystemFunctionCall::Readmemh { .. })
    )
}

/// Are all of `stmts`' FF writes log pushes the Cranelift event codegen marks?
pub fn ff_pushes_all_marked(stmts: &[ProtoStatement]) -> bool {
    !stmts.iter().any(|s| {
        contains(s, &|s| {
            matches!(s, ProtoStatement::CompiledBlock(_)) || is_readmemh(s)
        })
    })
}

/// Are all of `stmts`' marked comb writes stores that the Cranelift event
/// codegen wraps with a mark?
pub fn stores_all_marked(stmts: &[ProtoStatement], marks: &StoreMarks) -> bool {
    if stmts.iter().any(|s| contains(s, &is_readmemh)) {
        return false;
    }
    stmts.iter().all(|s| match s {
        ProtoStatement::Assign(_) | ProtoStatement::AssignDynamic(_) => {
            store_span(s).is_some() || !writes_marked(s, marks)
        }
        ProtoStatement::If(x) => {
            stores_all_marked(&x.true_side, marks) && stores_all_marked(&x.false_side, marks)
        }
        ProtoStatement::Case(x) => {
            x.arms.iter().all(|a| stores_all_marked(&a.body, marks))
                && stores_all_marked(&x.default, marks)
        }
        ProtoStatement::SequentialBlock(v) => stores_all_marked(v, marks),
        // The body's stores are marked one by one; the loop variable is
        // written without a mark.
        ProtoStatement::For(x) => {
            let var_marked = match x.var_offset {
                VarOffset::Comb(off) if off >= 0 => {
                    store_hits(marks, off as u32, off as u32 + 1).is_some()
                }
                VarOffset::Comb(_) => true,
                VarOffset::Ff(_) => false,
            };
            !var_marked && stores_all_marked(&x.body, marks)
        }
        _ => !writes_marked(s, marks),
    })
}
