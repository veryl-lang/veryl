//! Syntactic access footprints of a statically counted loop body.
//!
//! A counted loop is evaluated once with a symbolic iterator. Its one-
//! iteration SSA transfer weakly binds every affine element write, which
//! loses two facts that hold for every iteration:
//!
//! - an unconditional write at `alpha * i + b` stores to every position of
//!   its iteration space, so after the loop the pre-loop value is no longer
//!   retained there (a kill);
//! - with `|alpha * step| == 1`, a read at `alpha * i + c` observes the
//!   pre-loop value of a written position only if that position has not been
//!   written by an earlier iteration, which happens exactly when
//!   `(b - c) / (alpha * step) <= 0`.
//!
//! Must-writes intersect across conditional arms and stop at an early return.
//! Reads are may-accesses; a read that is not an affine element access of the
//! same symbolic frame, or any call that may read state, disables the
//! restriction. Both facts only remove dependencies that cannot occur, and
//! each fallback keeps the conservative one-iteration closure.

use super::{
    AffineIndex, CountedCoverage, CountedIterator, LoopEvaluation, ProcedureAnalysis,
    loop_evaluation,
};
use crate::HashMap;
use crate::comb_loop_detect::region::{ArraySpan, PackedSpan};
use crate::ir::{
    AssignDestination, CasePattern, Expression, Factor, Statement, VarId, VarIndex, VarSelect,
};

/// Consecutive array positions accessed from an affine base position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Footprint {
    base: AffineIndex,
    length: usize,
    /// Packed bits of every element, `None` for the whole element.
    packed: Option<PackedSpan>,
}

#[derive(Clone, Default)]
pub(super) struct LoopAccesses {
    /// Footprints written by every complete execution of the body.
    writes: HashMap<VarId, Vec<Footprint>>,
    /// Affine element reads, `None` for any other read of the variable.
    reads: HashMap<VarId, Vec<Option<Footprint>>>,
    /// The body may read state that is not visible in its statements.
    opaque: bool,
}

/// Effect of a counted loop on one storage key.
#[derive(Default)]
pub(super) struct KeyCoverage {
    /// Every position of the key is written by some iteration.
    pub(super) killed: bool,
    /// Array positions at which an iteration can observe the pre-loop value.
    /// `None` keeps every position.
    pub(super) exposed: Option<Vec<ArraySpan>>,
}

impl LoopAccesses {
    /// Close the accesses of one iteration over the iterator of its loop.
    pub(super) fn close(self, iterator: &CountedIterator, step: isize) -> Self {
        let writes = self
            .writes
            .into_iter()
            .map(|(id, footprints)| {
                let footprints = footprints
                    .into_iter()
                    .filter_map(|footprint| footprint.close_must(iterator, step))
                    .collect::<Vec<_>>();
                (id, footprints)
            })
            .filter(|(_, footprints)| !footprints.is_empty())
            .collect();
        let reads = self
            .reads
            .into_iter()
            .map(|(id, footprints)| {
                let footprints = footprints
                    .into_iter()
                    .map(|footprint| footprint.and_then(|x| x.close_may(iterator)))
                    .collect();
                (id, footprints)
            })
            .collect();
        Self {
            writes,
            reads,
            opaque: self.opaque,
        }
    }

    fn read(&mut self, id: VarId, footprint: Option<Footprint>) {
        self.reads.entry(id).or_default().push(footprint);
    }

    fn extend_reads(&mut self, other: LoopAccesses) {
        for (id, reads) in other.reads {
            self.reads.entry(id).or_default().extend(reads);
        }
        self.opaque |= other.opaque;
    }

    fn intersect_writes(
        arms: Vec<HashMap<VarId, Vec<Footprint>>>,
    ) -> HashMap<VarId, Vec<Footprint>> {
        let mut arms = arms.into_iter();
        let Some(mut common) = arms.next() else {
            return HashMap::default();
        };
        for arm in arms {
            common.retain(|id, footprints| {
                let Some(other) = arm.get(id) else {
                    return false;
                };
                footprints.retain(|footprint| other.contains(footprint));
                !footprints.is_empty()
            });
        }
        common
    }
}

impl Footprint {
    fn iterator_coefficient(&self, iterator: VarId) -> isize {
        self.base
            .terms
            .iter()
            .find(|(id, _)| *id == iterator)
            .map_or(0, |(_, coefficient)| *coefficient)
    }

    fn without_iterator(&self, iterator: VarId) -> AffineIndex {
        let mut base = self.base.clone();
        base.terms.retain(|(id, _)| *id != iterator);
        base
    }

    /// The union of all iterations is a must-write only if it has no gaps.
    fn close_must(self, iterator: &CountedIterator, step: isize) -> Option<Self> {
        let coefficient = self.iterator_coefficient(iterator.id);
        if coefficient == 0 {
            return Some(self);
        }
        let advance = coefficient.checked_mul(step)?.unsigned_abs();
        if advance > self.length {
            return None;
        }
        self.close_may(iterator)
    }

    /// A may-read covers the hull of all iterations.
    fn close_may(self, iterator: &CountedIterator) -> Option<Self> {
        let coefficient = self.iterator_coefficient(iterator.id);
        if coefficient == 0 {
            return Some(self);
        }
        let mut base = self.without_iterator(iterator.id);
        let first = coefficient.checked_mul(iterator.min)?;
        let last = coefficient.checked_mul(iterator.max)?;
        base.constant = base.constant.checked_add(first.min(last))?;
        let span = first.max(last).checked_sub(first.min(last))?.unsigned_abs();
        Some(Self {
            base,
            length: span.checked_add(self.length)?,
            packed: self.packed,
        })
    }

    /// Absolute array positions, if the footprint has no symbolic terms.
    fn absolute(&self) -> Option<ArraySpan> {
        if !self.base.terms.is_empty() {
            return None;
        }
        Some(ArraySpan {
            start: usize::try_from(self.base.constant).ok()?,
            length: self.length,
        })
    }
}

impl<'a, 's> ProcedureAnalysis<'a, 's> {
    /// Accesses of one execution of a counted loop body whose iterator is the
    /// innermost entry of `counted_iterators`.
    pub(super) fn loop_accesses(&mut self, statements: &[Statement]) -> LoopAccesses {
        let mut accesses = LoopAccesses::default();
        let mut exits = false;
        let mut rest = statements.iter();
        for statement in rest.by_ref() {
            let statement_accesses = self.statement_accesses(statement, &mut exits);
            accesses.extend_reads(LoopAccesses {
                writes: HashMap::default(),
                reads: statement_accesses.reads,
                opaque: statement_accesses.opaque,
            });
            if exits {
                break;
            }
            for (id, footprints) in statement_accesses.writes {
                accesses.writes.entry(id).or_default().extend(footprints);
            }
        }
        // Statements after an early return are not executed by every
        // iteration; their reads remain may-accesses and their writes are
        // not guaranteed.
        for statement in rest {
            let mut ignored = false;
            let statement_accesses = self.statement_accesses(statement, &mut ignored);
            accesses.extend_reads(LoopAccesses {
                writes: HashMap::default(),
                reads: statement_accesses.reads,
                opaque: statement_accesses.opaque,
            });
        }
        accesses
    }

    fn statement_accesses(&mut self, statement: &Statement, exits: &mut bool) -> LoopAccesses {
        let mut accesses = LoopAccesses::default();
        match statement {
            Statement::Assign(assign) => {
                self.expression_accesses(&assign.expr, &mut accesses);
                if assign.hier_dst.is_some() {
                    accesses.opaque = true;
                }
                for destination in &assign.dst {
                    self.destination_selector_accesses(destination, &mut accesses);
                    if let Some(footprint) = self.destination_footprint(destination) {
                        accesses
                            .writes
                            .entry(destination.id)
                            .or_default()
                            .push(footprint);
                    }
                }
                if self.is_return_assignment(&assign.dst) {
                    *exits = true;
                }
            }
            Statement::If(statement) => {
                self.expression_accesses(&statement.cond, &mut accesses);
                let true_side = self.loop_accesses_nested(&statement.true_side, exits);
                let false_side = self.loop_accesses_nested(&statement.false_side, exits);
                accesses.writes = LoopAccesses::intersect_writes(vec![
                    true_side.writes.clone(),
                    false_side.writes.clone(),
                ]);
                accesses.extend_reads(true_side);
                accesses.extend_reads(false_side);
            }
            Statement::IfReset(statement) => {
                let true_side = self.loop_accesses_nested(&statement.true_side, exits);
                let false_side = self.loop_accesses_nested(&statement.false_side, exits);
                accesses.writes = LoopAccesses::intersect_writes(vec![
                    true_side.writes.clone(),
                    false_side.writes.clone(),
                ]);
                accesses.extend_reads(true_side);
                accesses.extend_reads(false_side);
            }
            Statement::Case(statement) => {
                self.expression_accesses(&statement.case_target, &mut accesses);
                let mut arms = Vec::new();
                for arm in &statement.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            CasePattern::Eq(expression) => {
                                self.expression_accesses(expression, &mut accesses)
                            }
                            CasePattern::Range { lo, hi, .. } => {
                                self.expression_accesses(lo, &mut accesses);
                                self.expression_accesses(hi, &mut accesses);
                            }
                        }
                    }
                    let body = self.loop_accesses_nested(&arm.body, exits);
                    arms.push(body.writes.clone());
                    accesses.extend_reads(body);
                }
                let default = self.loop_accesses_nested(&statement.default, exits);
                arms.push(default.writes.clone());
                accesses.extend_reads(default);
                accesses.writes = LoopAccesses::intersect_writes(arms);
            }
            Statement::For(statement) => {
                let (start, end) = statement.range.bounds();
                for bound in [start, end] {
                    if let crate::ir::ForBound::Expression(expression) = bound {
                        self.expression_accesses(expression, &mut accesses);
                    }
                }
                // Only a loop evaluated with a symbolic iterator closes over
                // it; the others are read conservatively below.
                let counted = match loop_evaluation(statement, &mut self.ctx) {
                    LoopEvaluation::Counted(iterations) if iterations.count > 0 => Some(iterations),
                    _ => None,
                };
                let step = for_range_step(&statement.range);
                if let Some((iterations, step)) = counted.zip(step) {
                    let Some(iterator) = CountedIterator::of(statement, iterations) else {
                        accesses.opaque = true;
                        return accesses;
                    };
                    self.counted_iterators.push(iterator);
                    let mut nested_exits = false;
                    let body = self.loop_accesses_nested(&statement.body, &mut nested_exits);
                    self.counted_iterators.pop();
                    *exits |= nested_exits;
                    let closed = body.close(&iterator, step);
                    if !nested_exits {
                        accesses.writes = closed.writes.clone();
                    }
                    accesses.extend_reads(closed);
                } else {
                    // A loop that may execute zero times or break writes
                    // nothing for certain; its reads are not affine here.
                    let mut nested_exits = false;
                    let body = self.loop_accesses_nested(&statement.body, &mut nested_exits);
                    *exits |= nested_exits;
                    accesses.opaque |= body.opaque;
                    for (id, reads) in body.reads {
                        for _ in reads {
                            accesses.read(id, None);
                        }
                    }
                }
            }
            Statement::FunctionCall(call) => {
                accesses.opaque = true;
                for input in call.inputs.values() {
                    self.expression_accesses(input, &mut accesses);
                }
            }
            Statement::SystemFunctionCall(_) | Statement::TbMethodCall(_) => {
                accesses.opaque = true;
            }
            Statement::Break => *exits = true,
            Statement::Unsupported(_) => accesses.opaque = true,
            Statement::Null => {}
        }
        accesses
    }

    fn loop_accesses_nested(&mut self, statements: &[Statement], exits: &mut bool) -> LoopAccesses {
        let mut nested_exits = false;
        let mut accesses = LoopAccesses::default();
        for statement in statements {
            let statement_accesses = self.statement_accesses(statement, &mut nested_exits);
            if !nested_exits {
                for (id, footprints) in statement_accesses.writes.clone() {
                    accesses.writes.entry(id).or_default().extend(footprints);
                }
            }
            accesses.extend_reads(statement_accesses);
        }
        *exits |= nested_exits;
        accesses
    }

    fn destination_footprint(&mut self, destination: &AssignDestination) -> Option<Footprint> {
        let base = self.flattened_affine_index(destination.id, &destination.index)?;
        let packed = self.select_footprint(destination.id, &destination.select)?;
        Some(Footprint {
            base,
            length: 1,
            packed,
        })
    }

    fn select_footprint(&mut self, id: VarId, select: &VarSelect) -> Option<Option<PackedSpan>> {
        if select.is_empty() {
            return Some(None);
        }
        if !select.is_const_with_range() {
            return None;
        }
        let variable = self.ctx.variables.get(&id)?.clone();
        let (high, low) = select.eval_value(&mut self.ctx, &variable.r#type, false)?;
        Some(Some(PackedSpan::from_select(high, low)?))
    }

    fn destination_selector_accesses(
        &mut self,
        destination: &AssignDestination,
        accesses: &mut LoopAccesses,
    ) {
        for expression in destination
            .index
            .indices
            .iter()
            .chain(destination.select.0.iter())
        {
            self.expression_accesses(expression, accesses);
        }
        if let Some((_, expression)) = &destination.select.1 {
            self.expression_accesses(expression, accesses);
        }
    }

    fn variable_read(
        &mut self,
        id: VarId,
        index: &VarIndex,
        select: &VarSelect,
    ) -> Option<Footprint> {
        let base = self.flattened_affine_index(id, index)?;
        // A dynamic bit select still reads within the selected element.
        let packed = self.select_footprint(id, select).unwrap_or(None);
        Some(Footprint {
            base,
            length: 1,
            packed,
        })
    }

    fn expression_accesses(&mut self, expression: &Expression, accesses: &mut LoopAccesses) {
        if let Expression::Term(factor) = expression {
            match factor.as_ref() {
                Factor::Variable(id, index, select, _) => {
                    let footprint = self.variable_read(*id, index, select);
                    accesses.read(*id, footprint);
                }
                Factor::FunctionCall(_) => accesses.opaque = true,
                Factor::SystemFunctionCall(_)
                | Factor::HierVariable(_)
                | Factor::Anonymous(_)
                | Factor::Unknown(_) => {
                    accesses.opaque = true;
                    return;
                }
                Factor::Value(_) => {}
            }
        }
        for child in super::children::children(expression) {
            self.expression_accesses(child, accesses);
        }
    }
}

impl CountedCoverage {
    /// Effect of the counted loop on one storage key.
    pub(super) fn key_coverage(
        &self,
        id: VarId,
        array: ArraySpan,
        packed: PackedSpan,
    ) -> KeyCoverage {
        let mut coverage = KeyCoverage::default();
        let Some(writes) = self.closed.writes.get(&id) else {
            return coverage;
        };
        let covers_packed = |footprint: &Footprint| {
            footprint
                .packed
                .is_none_or(|span| span.intersection(packed) == Some(packed))
        };
        coverage.killed = writes.iter().any(|footprint| {
            covers_packed(footprint)
                && footprint
                    .absolute()
                    .is_some_and(|span| span.intersection(array) == Some(array))
        });
        coverage.exposed = (!self.closed.opaque)
            .then(|| exposed_positions(&self.per_iteration, id, &self.iterator, self.step))
            .flatten();
        coverage
    }
}

/// Positions at which an iteration can read the pre-loop value of `id`:
/// every position never written, and each read position written no earlier
/// than the iteration reading it.
fn exposed_positions(
    per_iteration: &LoopAccesses,
    id: VarId,
    iterator: &CountedIterator,
    step: isize,
) -> Option<Vec<ArraySpan>> {
    if per_iteration.opaque {
        return None;
    }
    let [write] = per_iteration.writes.get(&id)?.as_slice() else {
        return None;
    };
    let alpha = write.iterator_coefficient(iterator.id);
    if write.length != 1
        || write.packed.is_some()
        || alpha.checked_mul(step)?.unsigned_abs() != 1
        || write.base.terms.len() != 1
    {
        return None;
    }
    let advance = alpha.checked_mul(step)?;
    let hull = |constant: isize| -> Option<(isize, isize)> {
        let first = alpha.checked_mul(iterator.min)?.checked_add(constant)?;
        let last = alpha.checked_mul(iterator.max)?.checked_add(constant)?;
        Some((first.min(last), first.max(last)))
    };
    let written = hull(write.base.constant)?;
    let mut exposed = vec![(isize::MIN, written.0 - 1), (written.1 + 1, isize::MAX)];
    for read in per_iteration.reads.get(&id).into_iter().flatten() {
        let read = read.as_ref()?;
        if read.length != 1 || read.base.terms != write.base.terms {
            return None;
        }
        // Iterations between the write of a position and its read.
        let distance = write.base.constant.checked_sub(read.base.constant)?;
        if distance % advance != 0 || distance / advance <= 0 {
            let (low, high) = hull(read.base.constant)?;
            exposed.push((low.max(written.0), high.min(written.1)));
        }
    }
    let mut spans = exposed
        .into_iter()
        .filter(|(low, high)| low <= high)
        .filter_map(|(low, high)| {
            let low = usize::try_from(low.max(0)).ok()?;
            // A span that ends below zero addresses no element.
            let high = usize::try_from(high).ok()?;
            Some(ArraySpan {
                start: low,
                length: high.checked_sub(low)?.checked_add(1)?,
            })
        })
        .collect::<Vec<_>>();
    spans.sort_unstable();
    Some(spans)
}

pub(super) fn for_range_step(range: &crate::ir::ForRange) -> Option<isize> {
    match range {
        crate::ir::ForRange::Forward { step, .. } => isize::try_from(*step).ok(),
        crate::ir::ForRange::Reverse { step, .. } => {
            isize::try_from((*step).max(1)).ok()?.checked_neg()
        }
        crate::ir::ForRange::Stepped { .. } => None,
    }
}
