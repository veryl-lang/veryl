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
use crate::ir::ff_table::Refered;
use crate::ir::{AssignDestination, Declaration, FfTable, Statement, VarId, VarKind};
use crate::symbol::Affiliation;
use crate::value::{Value, ValueBigUint};

/// Elements that must keep their register, per `(declaration index, VarId)`.
#[derive(Clone, Debug, Default)]
pub struct UnsafeSelfReads {
    elements: HashMap<(usize, VarId), HashSet<usize>>,
    /// Contiguous reads remain intervals, including whole-array accesses.
    ranges: HashMap<(usize, VarId), Vec<std::ops::Range<usize>>>,
}

impl UnsafeSelfReads {
    pub fn contains(&self, decl: usize, id: VarId, index: usize) -> bool {
        self.ranges
            .get(&(decl, id))
            .is_some_and(|ranges| ranges.iter().any(|r| r.contains(&index)))
            || self
                .elements
                .get(&(decl, id))
                .is_some_and(|x| x.contains(&index))
    }

    fn insert(&mut self, decl: usize, id: VarId, index: usize) {
        self.elements.entry((decl, id)).or_default().insert(index);
    }

    fn insert_whole(&mut self, decl: usize, id: VarId, len: usize) {
        self.insert_range(decl, id, 0..len);
    }

    fn insert_range(&mut self, decl: usize, id: VarId, mut range: std::ops::Range<usize>) {
        if range.is_empty() {
            return;
        }
        let ranges = self.ranges.entry((decl, id)).or_default();
        ranges.retain(|other| {
            if range.start <= other.end && other.start <= range.end {
                range.start = range.start.min(other.start);
                range.end = range.end.max(other.end);
                false
            } else {
                true
            }
        });
        ranges.push(range);
    }

    #[cfg(test)]
    pub fn stored(&self) -> usize {
        self.elements.values().map(HashSet::len).sum::<usize>()
            + self.ranges.values().map(Vec::len).sum::<usize>()
    }
}

/// Bits touched at one element; `None` is unknown, taken as every bit.
type Mask = Option<BigUint>;

/// A destination: one element, or every element below the length when the
/// index does not evaluate.
enum Dst {
    Element(VarId, usize, Mask),
    Whole(VarId, usize, Mask),
}

/// Bits written so far on the path being walked. A write through an index
/// that does not evaluate is kept once, not per element.
#[derive(Clone, Default)]
struct Written {
    elements: HashMap<VarId, HashMap<usize, Mask>>,
    whole: HashMap<VarId, Vec<(usize, Mask)>>,
}

impl Written {
    fn get(&self, id: VarId, index: usize) -> Option<Mask> {
        let own = self.elements.get(&id).and_then(|x| x.get(&index));
        let whole = self
            .whole
            .get(&id)
            .into_iter()
            .flatten()
            .filter(|(len, _)| index < *len);
        union(own.into_iter().chain(whole.map(|(_, m)| m)))
    }

    /// What every element has in common.
    fn get_whole(&self, id: VarId) -> Option<Mask> {
        union(self.whole.get(&id).into_iter().flatten().map(|(_, m)| m))
    }

    fn indices(&self, id: VarId) -> impl Iterator<Item = usize> + '_ {
        self.elements
            .get(&id)
            .into_iter()
            .flatten()
            .map(|(i, _)| *i)
    }

    fn add(&mut self, dst: Dst) {
        match dst {
            Dst::Element(id, index, mask) => merge(
                self.elements
                    .entry(id)
                    .or_default()
                    .entry(index)
                    .or_insert_with(|| Some(BigUint::default())),
                mask,
            ),
            Dst::Whole(id, len, mask) => {
                let whole = self.whole.entry(id).or_default();
                match whole.iter_mut().find(|(l, _)| *l == len) {
                    Some((_, m)) => merge(m, mask),
                    None => whole.push((len, mask)),
                }
            }
        }
    }

    fn absorb(&mut self, other: Written) {
        for (id, elements) in other.elements {
            for (index, mask) in elements {
                self.add(Dst::Element(id, index, mask));
            }
        }
        for (id, whole) in other.whole {
            for (len, mask) in whole {
                self.add(Dst::Whole(id, len, mask));
            }
        }
    }
}

/// The merge of `masks`, `None` when there are none.
fn union<'a>(masks: impl Iterator<Item = &'a Mask>) -> Option<Mask> {
    let mut out = None;
    for m in masks {
        merge(
            out.get_or_insert_with(|| Some(BigUint::default())),
            m.clone(),
        );
    }
    out
}

pub fn unsafe_self_reads(
    decls: &[Declaration],
    context: &mut Context,
    per_element: bool,
) -> UnsafeSelfReads {
    let mut walk = Walk {
        context,
        out: UnsafeSelfReads::default(),
        decl: 0,
        per_element,
    };
    for (decl, x) in decls.iter().enumerate() {
        if let Declaration::Ff(ff) = x {
            walk.decl = decl;
            let mut written = Written::default();
            walk.seq(&ff.statements, &mut written);
        }
    }
    walk.out
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

struct Walk<'a> {
    context: &'a mut Context,
    out: UnsafeSelfReads,
    decl: usize,
    per_element: bool,
}

impl Walk<'_> {
    fn seq(&mut self, stmts: &[Statement], written: &mut Written) {
        for s in stmts {
            self.one(s, written);
        }
    }

    fn one(&mut self, stmt: &Statement, written: &mut Written) {
        match stmt {
            Statement::Assign(x) => {
                let mut dsts = Vec::new();
                for dst in &x.dst {
                    self.add_dst(dst, &mut dsts);
                }
                // The source runs before the store, so only an earlier
                // statement's write can be observed.
                let reads = element_reads(&x.expr, self.decl, self.context);
                for dst in &dsts {
                    self.check(dst, Some(&reads), written);
                }
                for dst in dsts {
                    written.add(dst);
                }
            }
            Statement::If(x) => self.branches(&x.true_side, &x.false_side, written),
            Statement::IfReset(x) => self.branches(&x.true_side, &x.false_side, written),
            Statement::Case(x) => self.seq(&x.lower_to_nested_if(), written),
            Statement::For(x) => {
                if let Some(iter) = x.range.eval_iter(self.context) {
                    for i in iter {
                        if let Some(var) = self.context.variable_mut(&x.var_id)
                            && let Some(total_width) = x.var_type.total_width()
                        {
                            let val = Value::new(i as u64, total_width, x.var_type.signed);
                            var.set_value(&[], val, None);
                        }
                        self.seq(&x.body, written);
                    }
                } else {
                    // Without concrete iterations there is no order to walk,
                    // and the body runs again.
                    let mut body = Vec::new();
                    self.collect_writes(&x.body, &mut body);
                    for dst in body {
                        written.add(dst);
                    }
                    self.seq(&x.body, written);
                }
            }
            Statement::FunctionCall(x) => {
                // The body is opaque here, so an output may read back bits a
                // write has already reached.
                for outputs in x.outputs.values() {
                    let mut dsts = Vec::new();
                    for dst in outputs {
                        self.add_dst(dst, &mut dsts);
                    }
                    for dst in dsts {
                        self.check(&dst, None, written);
                        written.add(dst);
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

    /// Records the elements of `dst` whose prior write the statement reads:
    /// `reads` is its source, or `None` for a function output, which reads
    /// the bits it writes.
    fn check(&mut self, dst: &Dst, reads: Option<&FfTable>, written: &Written) {
        let (id, len, mask) = match dst {
            Dst::Element(id, index, mask) => {
                if hit(reads, mask, *id, *index, written) {
                    self.out.insert(self.decl, *id, *index);
                }
                return;
            }
            Dst::Whole(id, len, mask) => (*id, *len, mask),
        };
        let whole_reads = reads
            .and_then(|x| x.range_refered.get(&id))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let prior = written.get_whole(id);
        if let Some(prior) = prior {
            match reads {
                Some(_) => {
                    for (range, reference) in whole_reads {
                        if let Some(read) = read_mask(std::iter::once(reference))
                            && overlaps(&read, &prior)
                        {
                            self.out
                                .insert_range(self.decl, id, range.start..range.end.min(len));
                        }
                    }
                }
                None if overlaps(mask, &prior) => self.out.insert_whole(self.decl, id, len),
                None => {}
            }
        }
        let own_reads = reads
            .into_iter()
            .flat_map(|x| x.table.keys())
            .filter(|(i, _)| *i == id)
            .map(|(_, index)| *index);
        let named: HashSet<usize> = own_reads.chain(written.indices(id)).collect();
        for i in named {
            if i < len && hit(reads, mask, id, i, written) {
                self.out.insert(self.decl, id, i);
            }
        }
    }

    fn branches(
        &mut self,
        true_side: &[Statement],
        false_side: &[Statement],
        written: &mut Written,
    ) {
        let mut wt = written.clone();
        let mut wf = std::mem::take(written);
        self.seq(true_side, &mut wt);
        self.seq(false_side, &mut wf);
        wt.absorb(wf);
        *written = wt;
    }

    /// Everything the statements may write, ignoring order.
    fn collect_writes(&mut self, stmts: &[Statement], out: &mut Vec<Dst>) {
        for s in stmts {
            match s {
                Statement::Assign(x) => {
                    for dst in &x.dst {
                        self.add_dst(dst, out);
                    }
                }
                Statement::If(x) => {
                    self.collect_writes(&x.true_side, out);
                    self.collect_writes(&x.false_side, out);
                }
                Statement::IfReset(x) => {
                    self.collect_writes(&x.true_side, out);
                    self.collect_writes(&x.false_side, out);
                }
                Statement::Case(x) => self.collect_writes(&x.lower_to_nested_if(), out),
                Statement::For(x) => self.collect_writes(&x.body, out),
                Statement::FunctionCall(x) => {
                    for outputs in x.outputs.values() {
                        for dst in outputs {
                            self.add_dst(dst, out);
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

    /// Mirrors `AssignDestination`'s gather so a write always has a matching
    /// table entry.
    fn add_dst(&mut self, dst: &AssignDestination, out: &mut Vec<Dst>) {
        let context = &mut *self.context;
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
        let range = if let Some(index) = dst.index.eval_value(context) {
            r#type.array.calc_range(&index)
        } else {
            r#type
                .total_array()
                .and_then(|n| n.checked_sub(1).map(|end| (0, end)))
        };
        if let Some((start, end)) = range {
            if !self.per_element && start == 0 && Some(end + 1) == r#type.total_array() && end > 0 {
                out.push(Dst::Whole(dst.id, end + 1, mask));
            } else {
                for index in start..=end {
                    out.push(Dst::Element(dst.id, index, mask.clone()));
                }
            }
        }
    }
}

/// Whether the statement reads a bit of element `index` that an earlier
/// write reached: see [`Walk::check`].
fn hit(reads: Option<&FfTable>, mask: &Mask, id: VarId, index: usize, written: &Written) -> bool {
    let read = match reads {
        Some(reads) => read_mask(reads.refered(id, index)),
        None => Some(mask.clone()),
    };
    matches!(
        (read, written.get(id, index)),
        (Some(read), Some(prior)) if overlaps(&read, &prior)
    )
}

/// What an expression reads, gathered the way the table gathers it so the
/// keys and masks line up.
fn element_reads(expr: &crate::ir::Expression, decl: usize, context: &mut Context) -> FfTable {
    let mut table = FfTable::default();
    expr.gather_ff(context, &mut table, decl, None, true);
    table
}

/// Bits read through `refs`, `None` when there are none.
fn read_mask<'a>(refs: impl Iterator<Item = &'a Refered>) -> Option<Mask> {
    let mut refs = refs.peekable();
    refs.peek()?;
    let mut mask: Mask = Some(BigUint::default());
    for (_, _, src_read_mask, _) in refs {
        // The gather leaves an empty mask when the range is not const.
        if *src_read_mask == BigUint::ZERO {
            return Some(None);
        }
        merge(&mut mask, Some(src_read_mask.clone()));
    }
    Some(mask)
}
