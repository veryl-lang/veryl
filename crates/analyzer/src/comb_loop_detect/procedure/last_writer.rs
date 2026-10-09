//! Exact last writers of the reads inside statically counted loops.
//!
//! A counted loop is evaluated once with symbolic iterators, and a write at a
//! position that moves with them only weakly updates its storage, which keeps
//! its previous value at every position. A read of that storage would see the
//! value from before the loop even where an earlier iteration, or an earlier
//! statement of the same iteration, has overwritten it.
//!
//! Instead, each write to such storage keeps a table of the values it writes,
//! indexed by position, and each read takes, for every iteration that reaches
//! it, the last write instance before it at the position it reads. This is
//! array dataflow analysis over the affine positions and the iteration order
//! of the loops (Feautrier): the instances are solved exactly, never
//! enumerated. Iterations whose position has no earlier write in the loops
//! read the value from before the outermost loop. A statement whose reads have
//! different last writers on different iterations is evaluated once for each
//! such set of iterations, with the iterators confined to it.
//!
//! The analysis covers a loop nest when every loop in it is evaluated
//! symbolically and calls inside it only read their inputs. Storage is
//! covered when every write to it in the nest has affine element and bit
//! positions. A read whose last writers cannot be solved exactly reads the
//! weakly updated storage as before.

use super::children::children;
use super::{
    AffineElements, AffineIndex, AnalysisStatus, CountedIterator, ProcedureAnalysis, SsaKey,
    position_domain,
};
use crate::HashMap;
use crate::comb_loop_detect::position::{
    Overflow, ceil_div_wide, extended_gcd, floor_div_wide, solve_congruence,
};
use crate::comb_loop_detect::region::{NodeKey, PackedSpan};
use crate::comb_loop_detect::ssa::VersionId;
use crate::ir::{
    AssignDestination, CasePattern, Expression, Factor, ForRange, ForStatement, MemberSelectDomain,
    Statement, SystemFunctionKind, VarId, VarIndex, VarSelect,
};
use std::cmp::Ordering;
use std::rc::Rc;
use veryl_parser::token_range::TokenRange;

/// A destination of an assignment statement.
pub(super) type WriterId = (TokenRange, usize);

/// A step from the body of the outermost loop towards a statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Step {
    /// The statement at this index of its block.
    Statement(usize),
    /// The arm of a branching statement.
    Arm(usize),
    /// The body of a loop, by its index in the scope.
    Loop(usize),
}

struct ScopeLoop {
    iterator: VarId,
    ascending: bool,
}

/// An arm of a branch whose condition does not depend only on iterators.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DataBranch {
    place: Vec<Step>,
    arm: usize,
    arms: usize,
}

#[derive(Clone, Debug)]
enum Bits {
    Whole,
    /// The lowest bit and the width.
    Range(AffineIndex, usize),
}

impl Bits {
    /// The bits within `span`, which they overlap. Bits that move with the
    /// iterators stay as they are.
    fn within(&self, span: PackedSpan) -> Self {
        let constant = |low: usize, width: usize| {
            Some(Self::Range(
                AffineIndex {
                    terms: Vec::new(),
                    constant: isize::try_from(low).ok()?,
                },
                width,
            ))
        };
        let narrowed = match self {
            Self::Whole => constant(span.start, span.length),
            Self::Range(low, width) if low.terms.is_empty() => (|| {
                let low = usize::try_from(low.constant).ok()?;
                let start = low.max(span.start);
                let end = (low.checked_add(*width)?).min(span.start.checked_add(span.length)?);
                (start < end)
                    .then(|| constant(start, end - start))
                    .flatten()
            })(),
            Self::Range(..) => None,
        };
        narrowed.unwrap_or_else(|| self.clone())
    }
}

/// A read whose last writers are solved for the bits of each key it takes.
pub(super) struct LastWriterRead {
    id: VarId,
    elements: Option<AffineElements>,
    select: VarSelect,
    member_select_domain: Option<MemberSelectDomain>,
    /// The last writers solved for each span of bits, `None` for a key
    /// without one.
    solved: HashMap<Option<(usize, usize)>, Option<Vec<Source>>>,
}

/// Affine element coordinates and bits of an access.
#[derive(Clone, Debug)]
struct Access {
    elements: Vec<AffineIndex>,
    /// The position of the element in the flattened array.
    flat: AffineIndex,
    bits: Bits,
}

/// A write on the iterations of the loops around it that reach it.
struct SubWriter {
    id: WriterId,
    path: Vec<Step>,
    /// The loops around the write, outermost first.
    loops: Vec<usize>,
    /// The iterations of each of those loops that reach the write.
    domains: Vec<CountedIterator>,
    access: Access,
    branches: Vec<DataBranch>,
}

pub(super) struct WriterScope {
    loops: Vec<ScopeLoop>,
    writers: HashMap<VarId, Vec<SubWriter>>,
    /// The inclusive extents of the elements and bits each write can take.
    extents: HashMap<(VarId, WriterId), Extent>,
    places: HashMap<*const Statement, Vec<Step>>,
    snapshots: HashMap<NodeKey, VersionId>,
    call_depth: usize,
}

/// The inclusive extents of the elements and, unless the whole element, the
/// bits that a write can take.
type Extent = ((isize, isize), Option<(isize, isize)>);

/// The last writer of a read on a set of iterations: a write, or the value
/// from before the outermost loop.
pub(super) type Source = Option<WriterId>;

/// Iterations of the read's loops, outermost first.
type Cell = Vec<CountedIterator>;

#[derive(Default)]
struct Scan {
    loops: Vec<ScopeLoop>,
    /// The loops nested in the outermost one, by their statements.
    loop_ids: HashMap<*const Statement, usize>,
    writers: HashMap<VarId, Vec<SubWriter>>,
    places: HashMap<*const Statement, Vec<Step>>,
    unknown: crate::HashSet<VarId>,
    /// Storage read or written on only some iterations of its loops.
    confined: crate::HashSet<VarId>,
    /// Whether the statements scanned are on only some iterations.
    confining: bool,
    /// Each enclosing loop that breaks, at its place, and whether a break of
    /// it has been scanned: what follows a break may not run. A runtime loop
    /// is one from its start.
    breaking: Vec<(Vec<Step>, bool)>,
}

/// An affine expression over the read's iterators.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Lin {
    k: Vec<i128>,
    c: i128,
}

impl Lin {
    fn constant(levels: usize, c: i128) -> Self {
        Self {
            k: vec![0; levels],
            c,
        }
    }

    fn iterator(levels: usize, level: usize) -> Self {
        let mut lin = Self::constant(levels, 0);
        lin.k[level] = 1;
        lin
    }

    fn scaled(&self, factor: i128) -> Option<Self> {
        Some(Self {
            k: self
                .k
                .iter()
                .map(|x| x.checked_mul(factor))
                .collect::<Option<_>>()?,
            c: self.c.checked_mul(factor)?,
        })
    }

    fn plus(&self, other: &Self, factor: i128) -> Option<Self> {
        Some(Self {
            k: self
                .k
                .iter()
                .zip(&other.k)
                .map(|(x, y)| x.checked_add(y.checked_mul(factor)?))
                .collect::<Option<_>>()?,
            c: self.c.checked_add(other.c.checked_mul(factor)?)?,
        })
    }

    fn as_constant(&self) -> Option<i128> {
        self.k.iter().all(|&x| x == 0).then_some(self.c)
    }
}

/// `sum(u[j] * unknown[j]) == rhs`.
struct Equation {
    u: Vec<i128>,
    rhs: Lin,
}

enum Constraint {
    /// `lin` within inclusive bounds.
    Range(Lin, Option<i128>, Option<i128>),
    /// `lin` congruent to the residue modulo the modulus.
    Mod(Lin, i128, i128),
}

/// A write instance before the read, as the value of each loop of the write:
/// `None` for a loop at the read's iteration or a value that is not known as
/// an affine function of the read's iterators.
struct Candidate<'s> {
    writer: &'s SubWriter,
    rank: usize,
    first_unknown: usize,
    cell: Cell,
    instance: Vec<Option<(Lin, i128)>>,
    covers: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Relation {
    Before,
    After,
    Exclusive,
}

/// Where `other` runs relative to `place` within one iteration of their
/// common loops.
fn relation(place: &[Step], other: &[Step]) -> Relation {
    let common = place
        .iter()
        .zip(other)
        .take_while(|(left, right)| left == right)
        .count();
    if common == place.len() || common == other.len() {
        // The same statement, or one nested in the other: a statement reads
        // before it writes.
        return Relation::After;
    }
    match (place[common], other[common]) {
        (Step::Statement(own), Step::Statement(other)) if other < own => Relation::Before,
        (Step::Arm(_), Step::Arm(_)) => Relation::Exclusive,
        _ => Relation::After,
    }
}

fn common_loops(left: &[Step], right: &[Step]) -> usize {
    1 + left
        .iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .filter(|(step, _)| matches!(step, Step::Loop(_)))
        .count()
}

fn place_loops(place: &[Step]) -> Vec<usize> {
    std::iter::once(0)
        .chain(place.iter().filter_map(|step| match step {
            Step::Loop(id) => Some(*id),
            _ => None,
        }))
        .collect()
}

fn intersect(
    left: &CountedIterator,
    right: &CountedIterator,
) -> Result<Option<CountedIterator>, Overflow> {
    left.confined(right.min, right.max, right.modulus, right.residue)
}

/// The cell both cells take, `None` when they share no iteration.
fn intersect_cells(left: &Cell, right: &Cell) -> Result<Option<Cell>, Overflow> {
    let mut cell = Vec::with_capacity(left.len());
    for (left, right) in left.iter().zip(right) {
        let Some(iterator) = intersect(left, right)? else {
            return Ok(None);
        };
        cell.push(iterator);
    }
    Ok(Some(cell))
}

impl ScopeLoop {
    fn last(&self, domain: &CountedIterator) -> isize {
        if self.ascending {
            domain.max
        } else {
            domain.min
        }
    }
}

impl WriterScope {
    fn has_writer(&self, id: VarId, writer: WriterId) -> bool {
        self.writers
            .get(&id)
            .is_some_and(|writers| writers.iter().any(|sub| sub.id == writer))
    }

    /// The positions of `domain` that the table of `writer` can hold: a
    /// table has no value where its write never stores.
    pub(super) fn table_domain(
        &self,
        id: VarId,
        writer: WriterId,
        domain: crate::comb_loop_detect::ssa::PositionDomain,
    ) -> crate::comb_loop_detect::ssa::PositionDomain {
        let Some(&((first, last), bits)) = self.extents.get(&(id, writer)) else {
            return domain;
        };
        let clip = |start: usize, length: usize, (low, high): (isize, isize)| {
            let low = usize::try_from(low.max(0)).ok()?.max(start);
            let high = usize::try_from(high).ok()?.min(start + length - 1);
            (low <= high).then(|| (low, high - low + 1))
        };
        let array = clip(domain.array_start, domain.array_length, (first, last));
        let packed = match bits {
            Some(bits) => clip(domain.packed_start, domain.packed_length, bits),
            None => Some((domain.packed_start, domain.packed_length)),
        };
        match (array, packed) {
            (Some((array_start, array_length)), Some((packed_start, packed_length))) => {
                crate::comb_loop_detect::ssa::PositionDomain {
                    array_start,
                    array_length,
                    packed_start,
                    packed_length,
                }
            }
            _ => domain,
        }
    }
}

impl<'a, 's> ProcedureAnalysis<'a, 's> {
    /// Open the scope of the outermost counted loop `statement`, whose
    /// iterator is the only counted iterator. Takes the values from before
    /// the loop, so it must run before the loop's checkpoint.
    pub(super) fn open_writer_scope(&mut self, statement: &ForStatement) {
        if self.writer_scope.is_some() || self.counted_iterators.len() != 1 {
            return;
        }
        let iterator = self.counted_iterators[0];
        // The evaluation of each loop forgets the value its iterator may keep
        // from conversion; so does the scan, or branches on it would look
        // constant.
        self.forget_runtime_iterator_value(statement.var_id);
        let mut scan = Scan::default();
        scan.loops.push(ScopeLoop {
            iterator: statement.var_id,
            ascending: !matches!(statement.range, ForRange::Reverse { .. }),
        });
        let mut path = Vec::new();
        let mut branches = Vec::new();
        let scanned = self.scan_block(&mut scan, &statement.body, &mut path, &mut branches);
        self.counted_iterators.truncate(1);
        self.counted_iterators[0] = iterator;
        if scanned.is_none() {
            return;
        }
        let Scan {
            loops,
            mut writers,
            places,
            unknown,
            confined,
            ..
        } = scan;
        // Storage whose writes stay in place is strongly updated by the
        // symbolic iteration, which reads the right instance unless the read
        // or the write is on only some iterations: an earlier iteration's
        // write is then all a later read can see, never the value from
        // before the loop.
        writers.retain(|id, subs| {
            !unknown.contains(id)
                && (confined.contains(id) || subs.iter().any(|sub| {
                    sub.access
                        .elements
                        .iter()
                        .any(|element| !element.terms.is_empty())
                        || matches!(&sub.access.bits, Bits::Range(low, _) if !low.terms.is_empty())
                }))
        });
        let mut snapshots = HashMap::default();
        let mut tracked = HashMap::default();
        let mut extents: HashMap<_, Extent> = HashMap::default();
        for (&id, subs) in &writers {
            for sub in subs {
                let range = |variable: VarId| {
                    let level = sub
                        .loops
                        .iter()
                        .position(|&lp| loops[lp].iterator == variable)?;
                    let domain = sub.domains[level];
                    Some((domain.min as i128, domain.max as i128))
                };
                let Some((first, last)) = sub.access.flat.extent_with(range) else {
                    continue;
                };
                let bits = match &sub.access.bits {
                    Bits::Whole => None,
                    Bits::Range(low, width) => {
                        let Some((low, high)) = low.extent_with(range) else {
                            continue;
                        };
                        Some((low, high + *width as i128 - 1))
                    }
                };
                let narrow = |value: i128| isize::try_from(value).ok();
                let (Some(first), Some(last)) = (narrow(first), narrow(last)) else {
                    continue;
                };
                let bits = match bits {
                    Some((low, high)) => match (narrow(low), narrow(high)) {
                        (Some(low), Some(high)) => Some((low, high)),
                        _ => continue,
                    },
                    None => None,
                };
                extents
                    .entry((id, sub.id))
                    .and_modify(|((low, high), packed): &mut Extent| {
                        *low = (*low).min(first);
                        *high = (*high).max(last);
                        *packed = match (*packed, bits) {
                            (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
                            _ => None,
                        };
                    })
                    .or_insert(((first, last), bits));
            }
        }
        for (id, subs) in writers {
            let keys = self.keys_for_id(id);
            let spans = keys
                .iter()
                .map(|&key| Some((key, self.key_span(key)?)))
                .collect::<Option<Vec<_>>>();
            let Some(spans) = spans else {
                continue;
            };
            let mut ids = subs.iter().map(|sub| sub.id).collect::<Vec<_>>();
            ids.sort_unstable();
            ids.dedup();
            for (key, span) in spans {
                let value = self.read_key(key);
                snapshots.insert(key, self.ssa.projected(value, position_domain(key.1, span)));
                for &writer in &ids {
                    // Each table starts empty, as its own value: tables are
                    // separate inputs of the loop transfer.
                    let empty = self.ssa.phi(Vec::new());
                    let table = self.writer_key(key, writer);
                    self.ssa.bind(table, empty);
                }
            }
            tracked.insert(id, subs);
        }
        if tracked.is_empty() {
            return;
        }
        extents.retain(|(id, _), _| tracked.contains_key(id));
        self.writer_scope = Some(Rc::new(WriterScope {
            loops,
            writers: tracked,
            extents,
            places,
            snapshots,
            call_depth: self.call_frames.len(),
        }));
    }

    pub(super) fn close_writer_scope(&mut self) {
        self.writer_scope = None;
        self.statement_places.clear();
    }

    fn writer_key(&self, key: NodeKey, writer: WriterId) -> SsaKey {
        SsaKey {
            writer: Some(writer),
            ..self.ssa_key(key)
        }
    }

    /// Whether statements are evaluated in the active scope.
    pub(super) fn in_writer_scope(&self) -> bool {
        self.active_writer_scope().is_some()
    }

    fn active_writer_scope(&self) -> Option<Rc<WriterScope>> {
        let scope = self.writer_scope.as_ref()?;
        (self.call_frames.len() == scope.call_depth).then(|| Rc::clone(scope))
    }

    /// Record each of `writes` at `path` on the current iterations.
    fn record_writers<'d>(
        &mut self,
        scan: &mut Scan,
        path: &[Step],
        branches: &[DataBranch],
        writes: impl Iterator<Item = (WriterId, &'d AssignDestination)>,
    ) {
        for (id, destination) in writes {
            let elements = self.affine_elements(destination.id, &destination.index);
            let access = self.access_forms(
                destination.id,
                elements.as_ref(),
                &destination.select,
                destination.comptime.member_select_domain,
            );
            let Some(access) = access else {
                scan.unknown.insert(destination.id);
                continue;
            };
            // A write after a break runs only when the loop did not break,
            // as on an arm of a branch on data.
            let mut branches = branches.to_vec();
            branches.extend(scan.breaking.iter().filter(|(_, broken)| *broken).map(
                |(place, _)| DataBranch {
                    place: place.clone(),
                    arm: 0,
                    arms: 2,
                },
            ));
            scan.writers
                .entry(destination.id)
                .or_default()
                .push(SubWriter {
                    id,
                    path: path.to_vec(),
                    loops: place_loops(path),
                    domains: self.counted_iterators.clone(),
                    access,
                    branches,
                });
        }
    }

    /// Scan each iteration of a loop over `values` as the one numbered by
    /// its order, its body specialized to its value. Charged to the
    /// procedure's work, one for each value.
    fn scan_ordinals(
        &mut self,
        scan: &mut Scan,
        statement: &ForStatement,
        values: Vec<usize>,
        path: &mut Vec<Step>,
        branches: &mut Vec<DataBranch>,
    ) -> Option<()> {
        self.charge_last_writer_work(values.len())?;
        for (ordinal, value) in values.into_iter().enumerate() {
            let iterator = ordinal_iterator(statement, isize::try_from(ordinal).ok()?)?;
            let body = crate::ir::peel::specialize_iteration(&mut self.ctx, statement, value);
            self.forget_runtime_iterator_value(statement.var_id);
            let mut origins = Vec::new();
            if let Some(body) = &body {
                statement_origins(&statement.body, body, &mut origins);
                self.statement_origins.extend(origins.iter().copied());
            }
            self.counted_iterators.push(iterator);
            let scanned = self.scan_block(
                scan,
                body.as_deref().unwrap_or(&statement.body),
                path,
                branches,
            );
            self.counted_iterators.pop();
            for (specialized, _) in origins {
                self.statement_origins.remove(&specialized);
            }
            scanned?;
        }
        Some(())
    }

    /// The statement a statement of a specialized iteration body comes from.
    pub(super) fn original_statement(&self, statement: &Statement) -> *const Statement {
        let mut statement = std::ptr::from_ref(statement);
        while let Some(&original) = self.statement_origins.get(&statement) {
            statement = original;
        }
        statement
    }

    fn scan_block(
        &mut self,
        scan: &mut Scan,
        statements: &[Statement],
        path: &mut Vec<Step>,
        branches: &mut Vec<DataBranch>,
    ) -> Option<()> {
        for (index, statement) in statements.iter().enumerate() {
            path.push(Step::Statement(index));
            scan.places
                .insert(self.original_statement(statement), path.clone());
            if scan.confining {
                scan.confined.extend(statement_accesses(statement));
            }
            let scanned = self.scan_statement(scan, statement, path, branches);
            path.pop();
            scanned?;
        }
        Some(())
    }

    fn scan_statement(
        &mut self,
        scan: &mut Scan,
        statement: &Statement,
        path: &mut Vec<Step>,
        branches: &mut Vec<DataBranch>,
    ) -> Option<()> {
        let original = self.original_statement(statement);
        match statement {
            Statement::Assign(assign) => {
                // A return ends the iterations early.
                if assign.hier_dst.is_some()
                    || self.is_return_assignment(&assign.dst)
                    || !self.reads_only(&assign.expr)
                {
                    return None;
                }
                for destination in &assign.dst {
                    let selectors = destination
                        .index
                        .expressions()
                        .chain(destination.select.0.iter())
                        .chain(destination.select.1.as_ref().map(|(_, end)| end));
                    for expression in selectors {
                        if !self.reads_only(expression) {
                            return None;
                        }
                    }
                }
                let record = |this: &mut Self, scan: &mut Scan, path: &[Step]| {
                    let writes = assign
                        .dst
                        .iter()
                        .enumerate()
                        .map(|(index, destination)| ((assign.token, index), destination));
                    this.record_writers(scan, path, branches, writes);
                };
                if let Expression::Ternary(condition, ..) = &assign.expr
                    && let Some(split) = self.iterator_condition_split(condition)
                {
                    scan.confined.extend(statement_accesses(statement));
                    for (arm, iterators) in self.exact_arms(&split)? {
                        let previous = self.counted_iterators[split.position];
                        for iterator in iterators {
                            self.counted_iterators[split.position] = iterator;
                            path.push(Step::Arm(arm));
                            record(self, scan, path);
                            path.pop();
                        }
                        self.counted_iterators[split.position] = previous;
                    }
                } else {
                    record(self, scan, path);
                }
                Some(())
            }
            Statement::If(statement) => {
                if !self.reads_only(&statement.cond) {
                    return None;
                }
                let sides = [&statement.true_side, &statement.false_side];
                if let Some(truth) = self.constant_truth(&statement.cond) {
                    let arm = usize::from(!truth);
                    path.push(Step::Arm(arm));
                    let scanned = self.scan_block(scan, sides[arm], path, branches);
                    path.pop();
                    return scanned;
                }
                if let Some(split) = self.iterator_condition_split(&statement.cond) {
                    let confining = std::mem::replace(&mut scan.confining, true);
                    for (arm, iterators) in self.exact_arms(&split)? {
                        let previous = self.counted_iterators[split.position];
                        for iterator in iterators {
                            self.counted_iterators[split.position] = iterator;
                            path.push(Step::Arm(arm));
                            let scanned = self.scan_block(scan, sides[arm], path, branches);
                            path.pop();
                            scanned?;
                        }
                        self.counted_iterators[split.position] = previous;
                    }
                    scan.confining = confining;
                    return Some(());
                }
                for (arm, side) in sides.into_iter().enumerate() {
                    branches.push(DataBranch {
                        place: path.clone(),
                        arm,
                        arms: 2,
                    });
                    path.push(Step::Arm(arm));
                    let scanned = self.scan_block(scan, side, path, branches);
                    path.pop();
                    branches.pop();
                    scanned?;
                }
                Some(())
            }
            Statement::Case(statement) => {
                if !self.reads_only(&statement.case_target) {
                    return None;
                }
                for pattern in statement.arms.iter().flat_map(|arm| &arm.patterns) {
                    let supported = match pattern {
                        CasePattern::Eq(value) => self.reads_only(value),
                        CasePattern::Range { lo, hi, .. } => {
                            self.reads_only(lo) && self.reads_only(hi)
                        }
                    };
                    if !supported {
                        return None;
                    }
                }
                let arms = statement.arms.len() + 1;
                let sides = statement
                    .arms
                    .iter()
                    .map(|arm| &arm.body)
                    .chain(std::iter::once(&statement.default));
                for (arm, side) in sides.enumerate() {
                    branches.push(DataBranch {
                        place: path.clone(),
                        arm,
                        arms,
                    });
                    path.push(Step::Arm(arm));
                    let scanned = self.scan_block(scan, side, path, branches);
                    path.pop();
                    branches.pop();
                    scanned?;
                }
                Some(())
            }
            Statement::For(statement) => {
                if self.for_range_is_proven_empty(&statement.range) {
                    return Some(());
                }
                // A loop evaluated with a symbolic iterator is scanned, and so
                // is one that takes each of its values: its iterations run in
                // order, each evaluated as the one of its value. A loop whose
                // step multiplies or that breaks is scanned value by value,
                // each as its own iteration with the positions its value
                // gives; after a break, nothing is certain to run.
                let mut ordinals = None;
                let iterator = match super::loop_evaluation(statement, &mut self.ctx) {
                    super::LoopEvaluation::Counted(iterations) => {
                        if iterations.count == 0 {
                            return Some(());
                        }
                        CountedIterator::of(statement, iterations)?
                    }
                    super::LoopEvaluation::Enumerated(values) => {
                        if values.is_empty() {
                            return Some(());
                        }
                        let last = isize::try_from(values.len() - 1).ok()?;
                        match ordinal_iterator(statement, 0) {
                            Some(ordinal) => {
                                ordinals = Some(values);
                                CountedIterator {
                                    max: last,
                                    ..ordinal
                                }
                            }
                            None => CountedIterator::of(
                                statement,
                                statement.range.eval_counted(&mut self.ctx)?,
                            )?,
                        }
                    }
                    // A runtime loop may run its body any number of times, so
                    // none of its writes is certain, and its reads, which may
                    // see a later write of its previous run, are not solved.
                    super::LoopEvaluation::Runtime => {
                        let (start, end) = statement.range.bounds();
                        for bound in [start, end] {
                            if let crate::ir::ForBound::Expression(expression) = bound
                                && !self.reads_only(expression)
                            {
                                return None;
                            }
                        }
                        scan.breaking.push((path.clone(), true));
                        let scanned = self.scan_block(scan, &statement.body, path, branches);
                        scan.breaking.pop();
                        return scanned;
                    }
                    super::LoopEvaluation::OverLimit => return None,
                };
                let next = scan.loops.len();
                let id = *scan.loop_ids.entry(original).or_insert(next);
                if id == next {
                    scan.loops.push(ScopeLoop {
                        iterator: iterator.id,
                        ascending: iterator.id == VarId::SYNTHETIC
                            || !matches!(statement.range, ForRange::Reverse { .. }),
                    });
                }
                let breaks = crate::ir::peel::has_own_break(&statement.body);
                if breaks {
                    scan.breaking.push((path.clone(), false));
                }
                path.push(Step::Loop(id));
                self.forget_runtime_iterator_value(statement.var_id);
                let scanned = match ordinals {
                    Some(values) => self.scan_ordinals(scan, statement, values, path, branches),
                    None => {
                        self.counted_iterators.push(iterator);
                        let scanned = self.scan_block(scan, &statement.body, path, branches);
                        self.counted_iterators.pop();
                        scanned
                    }
                };
                path.pop();
                if breaks {
                    scan.breaking.pop();
                }
                scanned
            }
            // A break belongs to the innermost loop, which breaks.
            Statement::Break => {
                scan.breaking.last_mut()?.1 = true;
                Some(())
            }
            // A call that writes nothing but its own variables and outputs
            // writes each output as an assignment would.
            Statement::FunctionCall(call) => {
                if !self.function_writes_only_its_own(call)
                    || !call.inputs.values().all(|input| self.reads_only(input))
                {
                    return None;
                }
                let destinations = call.outputs.values().flatten().collect::<Vec<_>>();
                for destination in &destinations {
                    let selectors = destination
                        .index
                        .expressions()
                        .chain(destination.select.0.iter())
                        .chain(destination.select.1.as_ref().map(|(_, end)| end));
                    for expression in selectors {
                        if !self.reads_only(expression) {
                            return None;
                        }
                    }
                }
                let writes = destinations
                    .into_iter()
                    .map(|destination| (output_writer(destination), destination));
                self.record_writers(scan, path, branches, writes);
                Some(())
            }
            Statement::SystemFunctionCall(call) => {
                let inputs = match &call.kind {
                    SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                        vec![&input.0]
                    }
                    _ => Vec::new(),
                };
                inputs
                    .into_iter()
                    .all(|input| self.reads_only(input))
                    .then_some(())
            }
            Statement::Null => Some(()),
            Statement::IfReset(_) | Statement::TbMethodCall(_) | Statement::Unsupported(_) => None,
        }
    }

    /// The arms of an iterator split, each with the iterations that take it
    /// as disjoint progressions. One side may be given as all iterations;
    /// it is then the complement of the other. `None` when the scan gives up.
    fn exact_arms(
        &mut self,
        split: &super::IteratorSplit,
    ) -> Option<[(usize, Vec<CountedIterator>); 2]> {
        let domain = self.counted_iterators[split.position];
        let total = |side: &[CountedIterator]| {
            side.iter()
                .try_fold(0isize, |total, piece| total.checked_add(piece.count()?))
        };
        let (holds, fails) = (split.holds.clone(), split.fails.clone());
        let (hold_count, fail_count) = (total(&holds)?, total(&fails)?);
        if hold_count.checked_add(fail_count)? == domain.count()? {
            return Some([(0, holds), (1, fails)]);
        }
        let mut others = |side: &[CountedIterator]| {
            let mut pieces = vec![domain];
            for part in side {
                let mut outside = Vec::new();
                for piece in pieces {
                    match intersect(&piece, part).ok()? {
                        Some(inside) => outside.extend(self.complement(&piece, &inside)?),
                        None => outside.push(piece),
                    }
                }
                pieces = outside;
            }
            Some(pieces)
        };
        Some(if hold_count <= fail_count {
            let fails = others(&holds)?;
            [(0, holds), (1, fails)]
        } else {
            let holds = others(&fails)?;
            [(0, holds), (1, fails)]
        })
    }

    /// The values of `whole` outside `part`, which lies within it, as
    /// disjoint progressions: those before and after it, and one for each
    /// other residue between. They are charged to the procedure's work
    /// before they are built. `None` when the scan gives up.
    fn complement(
        &mut self,
        whole: &CountedIterator,
        part: &CountedIterator,
    ) -> Option<Vec<CountedIterator>> {
        let ratio = part.modulus / whole.modulus;
        self.charge_last_writer_work(usize::try_from(ratio).ok()?.saturating_add(1))?;
        let mut pieces = Vec::new();
        // `part` lies within `whole`, so a piece before or after it is
        // there only when `part` stops short of that end.
        if whole.min < part.min {
            pieces.extend(whole.within(whole.min, part.min - 1).ok()?);
        }
        if part.max < whole.max {
            pieces.extend(whole.within(part.max + 1, whole.max).ok()?);
        }
        for step in 1..ratio {
            // `whole.modulus * step` is below `part.modulus`.
            let residue = part
                .residue
                .checked_add(whole.modulus * step)?
                .rem_euclid(part.modulus);
            pieces.extend(
                whole
                    .confined(part.min, part.max, part.modulus, residue)
                    .ok()?,
            );
        }
        Some(pieces)
    }

    /// The cells of `cell` inside `set` and outside it. `None` when the scan
    /// gives up.
    fn split_cell(&mut self, cell: &Cell, set: &Cell) -> Option<(Option<Cell>, Vec<Cell>)> {
        let Some(inside) = intersect_cells(cell, set).ok()? else {
            return Some((None, vec![cell.clone()]));
        };
        let mut outside = Vec::new();
        for level in 0..cell.len() {
            for piece in self.complement(&cell[level], &inside[level])? {
                let mut part = inside[..level].to_vec();
                part.push(piece);
                part.extend_from_slice(&cell[level + 1..]);
                outside.push(part);
            }
        }
        Some((Some(inside), outside))
    }

    /// Whether evaluating `expression` writes nothing: every call in it
    /// writes only its own variables. What a call reads is evaluated in its
    /// own frame, outside the scope, so it reads the weakly updated storage.
    fn reads_only(&mut self, expression: &Expression) -> bool {
        if let Expression::Term(factor) = expression {
            match factor.as_ref() {
                Factor::FunctionCall(call)
                    if !call.outputs.is_empty() || !self.function_writes_only_its_own(call) =>
                {
                    return false;
                }
                // Only a sign cast reads the value of its input.
                Factor::SystemFunctionCall(call) => {
                    return match &call.kind {
                        SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                            self.reads_only(&input.0)
                        }
                        _ => true,
                    };
                }
                _ => {}
            }
        }
        children(expression)
            .into_iter()
            .all(|child| self.reads_only(child))
    }

    /// The element coordinates and bits of an access whose positions are
    /// affine in the counted iterators.
    fn access_forms(
        &mut self,
        id: VarId,
        elements: Option<&AffineElements>,
        select: &VarSelect,
        member_select_domain: Option<MemberSelectDomain>,
    ) -> Option<Access> {
        if member_select_domain.is_some() {
            return None;
        }
        let variable = self.ctx.variables.get(&id)?.clone();
        let iterators = |terms: &AffineIndex, counted: &[CountedIterator]| {
            terms
                .terms
                .iter()
                .all(|(id, _)| counted.iter().any(|iterator| iterator.id == *id))
        };
        let AffineElements { elements, flat } = elements?.clone();
        if !elements
            .iter()
            .all(|element| iterators(element, &self.counted_iterators))
        {
            return None;
        }
        let bits = if select.is_empty() {
            Bits::Whole
        } else if select.is_const_with_range() {
            let (high, low) = select.eval_value(&mut self.ctx, &variable.r#type, false)?;
            Bits::Range(
                AffineIndex {
                    terms: Vec::new(),
                    constant: isize::try_from(low).ok()?,
                },
                high.checked_sub(low)?.checked_add(1)?,
            )
        } else {
            let (low, width) = self.packed_affine_select(id, select)?;
            if !iterators(&low, &self.counted_iterators) {
                return None;
            }
            Bits::Range(low, width)
        };
        Some(Access {
            elements,
            flat,
            bits,
        })
    }

    /// The place of a statement about to be evaluated in the active scope.
    /// A statement of an iteration's specialized body takes the place of the
    /// statement it was specialized from.
    pub(super) fn enter_statement_place(&mut self, statement: &Statement) -> bool {
        let Some(scope) = self.active_writer_scope() else {
            return false;
        };
        let statement = self.original_statement(statement);
        let place = scope
            .places
            .get(&statement)
            .cloned()
            .or_else(|| self.statement_places.last().cloned());
        match place {
            Some(place) => {
                self.statement_places.push(place);
                true
            }
            None => false,
        }
    }

    /// A read whose last writers are solved for each key it takes, or `None`
    /// when no storage it reads is covered.
    pub(super) fn last_writer_sources(
        &mut self,
        id: VarId,
        index: &VarIndex,
        select: &VarSelect,
        member_select_domain: Option<MemberSelectDomain>,
    ) -> Option<LastWriterRead> {
        if !self.active_writer_scope()?.writers.contains_key(&id) {
            return None;
        }
        let elements = self.affine_elements(id, index);
        self.last_writer_read(id, elements, select, member_select_domain)
    }

    /// `last_writer_sources` of an access with the affine coordinates
    /// `elements`.
    pub(super) fn last_writer_read(
        &mut self,
        id: VarId,
        elements: Option<AffineElements>,
        select: &VarSelect,
        member_select_domain: Option<MemberSelectDomain>,
    ) -> Option<LastWriterRead> {
        self.active_writer_scope()?
            .writers
            .contains_key(&id)
            .then(|| LastWriterRead {
                id,
                elements,
                select: select.clone(),
                member_select_domain,
                solved: HashMap::default(),
            })
    }

    /// The last writers of the bits of `key` that `read` takes on its
    /// iterations, or `None` when they are not solved exactly and the read
    /// takes the weakly updated storage. Keys of the same bits share them.
    pub(super) fn key_last_writers(
        &mut self,
        read: &mut LastWriterRead,
        key: NodeKey,
    ) -> Option<Vec<Source>> {
        let span = self.key_span(key);
        let bits = span.map(|span| (span.start, span.length));
        if let Some(solved) = read.solved.get(&bits) {
            return solved.clone();
        }
        let solved = self
            .last_writer_cells(
                read.id,
                read.elements.as_ref(),
                &read.select,
                read.member_select_domain,
                span,
            )
            .map(|cells| {
                let mut sources = Vec::new();
                for (_, cell_sources) in cells {
                    for source in cell_sources {
                        if !sources.contains(&source) {
                            sources.push(source);
                        }
                    }
                }
                sources
            });
        read.solved.insert(bits, solved.clone());
        solved
    }

    /// The last writers of a read on each set of its iterations, of the bits
    /// within `key_bits` when given.
    fn last_writer_cells(
        &mut self,
        id: VarId,
        elements: Option<&AffineElements>,
        select: &VarSelect,
        member_select_domain: Option<MemberSelectDomain>,
        key_bits: Option<PackedSpan>,
    ) -> Option<Vec<(Cell, Vec<Source>)>> {
        if self.runtime_loop_depth > 0 {
            return None;
        }
        let scope = self.active_writer_scope()?;
        let writers = scope.writers.get(&id)?;
        let place = self.statement_places.last()?.clone();
        let mut access = self.access_forms(id, elements, select, member_select_domain)?;
        if let Some(span) = key_bits {
            access.bits = access.bits.within(span);
        }
        self.solve_last_writers(&scope, writers, &place, &access)
    }

    /// The value of `key` from the last writers `sources`.
    pub(super) fn last_writer_value(&mut self, key: NodeKey, sources: &[Source]) -> VersionId {
        let Some(scope) = self.writer_scope.clone() else {
            return self.read_key(key);
        };
        let mut versions = Vec::new();
        for source in sources {
            match source {
                Some(writer) => {
                    let table = self.writer_key(key, *writer);
                    versions.push(self.ssa.read(table));
                }
                None => match scope.snapshots.get(&key) {
                    Some(&snapshot) => versions.push(snapshot),
                    None => return self.read_key(key),
                },
            }
        }
        self.ssa.phi(versions)
    }

    /// Record a write of `version` to `key` in the table of the current
    /// writer. A write to covered storage from anywhere else would not be
    /// seen by the reads, so the analysis stops.
    pub(super) fn record_table_write(&mut self, key: NodeKey, version: VersionId) {
        let Some(scope) = self.active_writer_scope() else {
            return;
        };
        if !scope.writers.contains_key(&key.0) {
            return;
        }
        match self.current_writer {
            Some(writer) if scope.has_writer(key.0, writer) => {
                // A table keeps its own bounds: the storage's projection is
                // also what the weak update reads.
                let version = match self.key_span(key) {
                    Some(packed) => {
                        let domain =
                            scope.table_domain(key.0, writer, position_domain(key.1, packed));
                        self.ssa.projected(version, domain)
                    }
                    None => version,
                };
                let table = self.writer_key(key, writer);
                self.ssa.bind(table, version);
            }
            _ => self.status = self.status.max(AnalysisStatus::Barrier),
        }
    }

    /// The sets of iterations of `statement` on which the reads it makes
    /// before nested statements have the same last writers, when there is
    /// more than one.
    pub(super) fn last_writer_split(&mut self, statement: &Statement) -> Option<Vec<Cell>> {
        let scope = self.active_writer_scope()?;
        if self.split_statement == Some(std::ptr::from_ref(statement)) {
            return None;
        }
        if !matches!(
            statement,
            Statement::Assign(_)
                | Statement::If(_)
                | Statement::Case(_)
                | Statement::FunctionCall(_)
        ) {
            return None;
        }
        let mut reads = statement_reads(statement);
        reads.retain(|(id, ..)| scope.writers.contains_key(id));
        if reads.is_empty() {
            return None;
        }
        let mut cells: Vec<(Cell, Vec<Vec<Source>>)> =
            vec![(self.counted_iterators.clone(), Vec::new())];
        for (id, index, select, member_select_domain) in reads {
            let elements = self.affine_elements(id, &index);
            let Some(read_cells) =
                self.last_writer_cells(id, elements.as_ref(), &select, member_select_domain, None)
            else {
                continue;
            };
            let mut refined = Vec::new();
            for (cell, sources) in &cells {
                for (read_cell, read_sources) in &read_cells {
                    if let Some(inside) = intersect_cells(cell, read_cell).ok()? {
                        let mut sources = sources.clone();
                        sources.push(read_sources.clone());
                        refined.push((inside, sources));
                    }
                }
            }
            cells = refined;
            self.charge_last_writer_work(cells.len())?;
        }
        if cells.len() < 2 || cells.iter().all(|(_, sources)| *sources == cells[0].1) {
            return None;
        }
        Some(cells.into_iter().map(|(cell, _)| cell).collect())
    }

    fn charge_last_writer_work(&mut self, count: usize) -> Option<()> {
        if self.reserve_guard_work(count) {
            Some(())
        } else {
            None
        }
    }

    fn solve_last_writers(
        &mut self,
        scope: &WriterScope,
        writers: &[SubWriter],
        place: &[Step],
        access: &Access,
    ) -> Option<Vec<(Cell, Vec<Source>)>> {
        let read_loops = place_loops(place);
        let levels = read_loops.len();
        if self.counted_iterators.len() != levels
            || read_loops
                .iter()
                .zip(&self.counted_iterators)
                .any(|(&id, iterator)| scope.loops[id].iterator != iterator.id)
        {
            return None;
        }
        let domain = self.counted_iterators.clone();
        let read_iterators = domain.iter().map(|x| x.id).collect::<Vec<_>>();
        let read_lin = |index: &AffineIndex| -> Option<Lin> {
            let mut lin = Lin::constant(levels, index.constant as i128);
            for &(id, coefficient) in &index.terms {
                let level = read_iterators.iter().position(|&x| x == id)?;
                lin.k[level] += coefficient as i128;
            }
            Some(lin)
        };
        let mut candidates = Vec::new();
        for writer in writers {
            let common = common_loops(place, &writer.path);
            let order = relation(place, &writer.path);
            let mut groups = Vec::new();
            if order == Relation::Before {
                groups.push((2 * common - 1, common, None));
            }
            for carried in (0..common).rev() {
                groups.push((2 * carried, carried, Some(carried)));
            }
            for (rank, first_unknown, carried) in groups {
                let Some(candidate) = self.candidate(
                    scope,
                    writer,
                    access,
                    &domain,
                    &read_lin,
                    rank,
                    first_unknown,
                    carried,
                )?
                else {
                    continue;
                };
                candidates.push(candidate);
            }
        }
        self.charge_last_writer_work(candidates.len().saturating_add(1))?;
        // Latest first, with ties in one group.
        let mut order: Vec<Vec<usize>> = Vec::new();
        for index in 0..candidates.len() {
            let mut position = order.len();
            let mut tie = None;
            for (group, members) in order.iter().enumerate() {
                match self.compare(scope, &candidates[index], &candidates[members[0]])? {
                    Ordering::Greater => {
                        position = group;
                        break;
                    }
                    Ordering::Equal => {
                        tie = Some(group);
                        break;
                    }
                    Ordering::Less => {}
                }
            }
            match tie {
                Some(group) => order[group].push(index),
                None => order.insert(position, vec![index]),
            }
        }
        let mut cells = vec![domain];
        for candidate in &candidates {
            let mut refined = Vec::new();
            for cell in cells {
                let (inside, outside) = self.split_cell(&cell, &candidate.cell)?;
                refined.extend(inside);
                refined.extend(outside);
            }
            cells = refined;
            self.charge_last_writer_work(cells.len())?;
        }
        let mut result = Vec::new();
        for cell in cells {
            let mut sources = Vec::new();
            let mut covered = false;
            for group in &order {
                let mut present = Vec::new();
                for &index in group {
                    let candidate = &candidates[index];
                    if intersect_cells(&cell, &candidate.cell).ok()?.is_some() {
                        present.push(candidate);
                    }
                }
                if present.is_empty() {
                    continue;
                }
                for candidate in &present {
                    let source = Some(candidate.writer.id);
                    if !sources.contains(&source) {
                        sources.push(source);
                    }
                }
                if overwrites(&present) {
                    covered = true;
                    break;
                }
            }
            if !covered {
                sources.push(None);
            }
            result.push((cell, sources));
        }
        Some(result)
    }

    /// The instances of `writer` before the read in one group: at the read's
    /// iteration of the loops before `first_unknown`, and, for a carried
    /// group, at an earlier iteration of loop `carried`. `Ok(None)` when no
    /// instance precedes the read; `None` when they are not solved exactly.
    #[allow(clippy::too_many_arguments)]
    fn candidate<'w>(
        &self,
        scope: &WriterScope,
        writer: &'w SubWriter,
        access: &Access,
        domain: &Cell,
        read_lin: &dyn Fn(&AffineIndex) -> Option<Lin>,
        rank: usize,
        first_unknown: usize,
        carried: Option<usize>,
    ) -> Option<Option<Candidate<'w>>> {
        let levels = domain.len();
        let unknowns = writer.loops.len();
        let writer_iterators = writer
            .loops
            .iter()
            .map(|&id| scope.loops[id].iterator)
            .collect::<Vec<_>>();
        // `sum(u * unknown) + c` of a write coordinate; known loops move into
        // the read side.
        let equation = |written: &AffineIndex, read: &AffineIndex| -> Option<Equation> {
            let mut rhs = read_lin(read)?;
            rhs.c -= written.constant as i128;
            let mut u = vec![0i128; unknowns];
            for &(id, coefficient) in &written.terms {
                let level = writer_iterators.iter().position(|&x| x == id)?;
                if level < first_unknown {
                    rhs.k[level] -= coefficient as i128;
                } else {
                    u[level] += coefficient as i128;
                }
            }
            Some(Equation { u, rhs })
        };
        if writer.access.elements.len() != access.elements.len() {
            return None;
        }
        let mut equations = writer
            .access
            .elements
            .iter()
            .zip(&access.elements)
            .map(|(written, read)| equation(written, read))
            .collect::<Option<Vec<_>>>()?;
        let covers = match (&writer.access.bits, &access.bits) {
            (Bits::Whole, _) => true,
            (Bits::Range(..), Bits::Whole) => false,
            // Constant bits are compared directly: a write elsewhere is not
            // a candidate, and one over all the read's bits overwrites them.
            (Bits::Range(written, written_width), Bits::Range(read, read_width))
                if written.terms.is_empty() && read.terms.is_empty() =>
            {
                let last = |low: &AffineIndex, width: usize| {
                    low.constant
                        .checked_add(isize::try_from(width).ok()?.checked_sub(1)?)
                };
                let (written_last, read_last) =
                    (last(written, *written_width)?, last(read, *read_width)?);
                if written_last < read.constant || read_last < written.constant {
                    return Some(None);
                }
                written.constant <= read.constant && read_last <= written_last
            }
            // Bits that do not move together may write some of the read's,
            // as a part of the element does.
            (Bits::Range(written, written_width), Bits::Range(read, read_width)) => {
                if written_width != read_width
                    || (*written_width != 1 && !aligned(written, read, *written_width))
                {
                    false
                } else {
                    equations.push(equation(written, read)?);
                    true
                }
            }
        };
        let mut cell = domain.clone();
        // The read's iterations of the known loops reach the write.
        for (iterator, reached) in cell.iter_mut().zip(&writer.domains).take(first_unknown) {
            match intersect(iterator, reached).ok()? {
                Some(confined) => *iterator = confined,
                None => return Some(None),
            }
        }
        let mut constraints = Vec::new();
        let determined = eliminate(&mut equations, unknowns, levels, &mut constraints)?;
        let mut instance = vec![None; unknowns];
        for level in first_unknown..unknowns {
            let iterator = &writer.domains[level];
            let ascending = scope.loops[writer.loops[level]].ascending;
            match &determined[level] {
                Some((numerator, denominator)) => {
                    constraints.push(Constraint::Range(
                        numerator.clone(),
                        Some(denominator * iterator.min as i128),
                        Some(denominator * iterator.max as i128),
                    ));
                    let modulus = denominator * iterator.modulus as i128;
                    if modulus > 1 {
                        constraints.push(Constraint::Mod(
                            numerator.clone(),
                            modulus,
                            denominator * iterator.residue as i128,
                        ));
                    }
                    if carried == Some(level) {
                        // An earlier iteration of the carried loop.
                        let difference =
                            numerator.plus(&Lin::iterator(levels, level), -denominator)?;
                        constraints.push(if ascending {
                            Constraint::Range(difference, None, Some(-1))
                        } else {
                            Constraint::Range(difference, Some(1), None)
                        });
                    }
                    instance[level] = Some((numerator.clone(), *denominator));
                }
                None if carried == Some(level) => {
                    // Some value of the write's iterations precedes the read's.
                    let lin = Lin::iterator(levels, level);
                    constraints.push(if ascending {
                        Constraint::Range(lin, Some(iterator.min as i128 + 1), None)
                    } else {
                        Constraint::Range(lin, None, Some(iterator.max as i128 - 1))
                    });
                    // The latest of them is the previous iteration when the
                    // read takes the same values.
                    let read = &domain[level];
                    if read.modulus == iterator.modulus
                        && read.residue.rem_euclid(read.modulus)
                            == iterator.residue.rem_euclid(iterator.modulus)
                    {
                        let step = if ascending {
                            -(iterator.modulus as i128)
                        } else {
                            iterator.modulus as i128
                        };
                        let mut previous = Lin::iterator(levels, level);
                        previous.c = step;
                        instance[level] = Some((previous, 1));
                    }
                }
                None => {
                    let last = scope.loops[writer.loops[level]].last(iterator);
                    instance[level] = Some((Lin::constant(levels, last as i128), 1));
                }
            }
        }
        for constraint in constraints {
            match restrict(&mut cell, constraint)? {
                true => {}
                false => return Some(None),
            }
        }
        Some(Some(Candidate {
            writer,
            rank,
            first_unknown,
            cell,
            instance,
            covers,
        }))
    }

    /// Whether `left` is a later write instance than `right`.
    fn compare(
        &self,
        scope: &WriterScope,
        left: &Candidate<'_>,
        right: &Candidate<'_>,
    ) -> Option<Ordering> {
        if left.rank != right.rank {
            return Some(left.rank.cmp(&right.rank));
        }
        if left.writer.path == right.writer.path {
            return Some(Ordering::Equal);
        }
        let shared = common_loops(&left.writer.path, &right.writer.path);
        for level in left.first_unknown.max(right.first_unknown)..shared {
            let (Some((left_value, left_denominator)), Some((right_value, right_denominator))) =
                (&left.instance[level], &right.instance[level])
            else {
                return None;
            };
            let difference = left_value
                .scaled(*right_denominator)?
                .plus(right_value, -*left_denominator)?
                .as_constant()?;
            let ascending = scope.loops[left.writer.loops[level]].ascending;
            match (difference.cmp(&0), ascending) {
                (Ordering::Equal, _) => {}
                (ordering, true) => return Some(ordering),
                (ordering, false) => return Some(ordering.reverse()),
            }
        }
        Some(match relation(&left.writer.path, &right.writer.path) {
            Relation::Before => Ordering::Greater,
            Relation::After => Ordering::Less,
            Relation::Exclusive => Ordering::Equal,
        })
    }
}

/// Whether the latest present writers overwrite every bit the read takes on
/// each of its iterations: one of them always writes, or they are the arms of
/// one branch that write on every path through it.
fn overwrites(present: &[&Candidate<'_>]) -> bool {
    if !present.iter().all(|candidate| candidate.covers) {
        return false;
    }
    if present
        .iter()
        .any(|candidate| candidate.writer.branches.is_empty())
    {
        return true;
    }
    let first = &present[0].writer.branches;
    let [branch] = first.as_slice() else {
        return false;
    };
    let mut arms = vec![false; branch.arms];
    for candidate in present {
        let [other] = candidate.writer.branches.as_slice() else {
            return false;
        };
        if other.place != branch.place || other.arms != branch.arms {
            return false;
        }
        arms[other.arm] = true;
    }
    arms.into_iter().all(|arm| arm)
}

/// Whether two bit ranges of `width` bits either coincide or are disjoint.
fn aligned(left: &AffineIndex, right: &AffineIndex, width: usize) -> bool {
    let width = width as isize;
    left.terms
        .iter()
        .chain(&right.terms)
        .all(|(_, coefficient)| coefficient % width == 0)
        && left.constant.rem_euclid(width) == right.constant.rem_euclid(width)
}

/// Solve each unknown that an equation determines, as `numerator / denominator`
/// over the read's iterators. Equations left with no unknown constrain the
/// read; one left with several makes the solution inexact.
fn eliminate(
    equations: &mut [Equation],
    unknowns: usize,
    levels: usize,
    constraints: &mut Vec<Constraint>,
) -> Option<Vec<Option<(Lin, i128)>>> {
    let mut determined: Vec<Option<(Lin, i128)>> = vec![None; unknowns];
    let mut used = vec![false; equations.len()];
    loop {
        let mut progress = false;
        for (index, equation) in equations.iter_mut().enumerate() {
            if used[index] {
                continue;
            }
            for (level, solved) in determined.iter().enumerate() {
                let Some((numerator, denominator)) = solved else {
                    continue;
                };
                let coefficient = equation.u[level];
                if coefficient == 0 {
                    continue;
                }
                for u in &mut equation.u {
                    *u = u.checked_mul(*denominator)?;
                }
                equation.u[level] = 0;
                equation.rhs = equation
                    .rhs
                    .scaled(*denominator)?
                    .plus(numerator, -coefficient)?;
            }
            let remaining = equation
                .u
                .iter()
                .enumerate()
                .filter(|(_, u)| **u != 0)
                .collect::<Vec<_>>();
            match remaining.as_slice() {
                [] => {
                    used[index] = true;
                    progress = true;
                    constraints.push(Constraint::Range(equation.rhs.clone(), Some(0), Some(0)));
                }
                [(level, coefficient)] => {
                    let (level, coefficient) = (*level, **coefficient);
                    used[index] = true;
                    progress = true;
                    let (mut numerator, mut denominator) = if coefficient < 0 {
                        (equation.rhs.scaled(-1)?, -coefficient)
                    } else {
                        (equation.rhs.clone(), coefficient)
                    };
                    let divisor = numerator
                        .k
                        .iter()
                        .fold(extended_gcd(numerator.c, denominator).0, |g, &x| {
                            extended_gcd(g, x).0
                        });
                    if divisor > 1 {
                        numerator = Lin {
                            k: numerator.k.iter().map(|x| x / divisor).collect(),
                            c: numerator.c / divisor,
                        };
                        denominator /= divisor;
                    }
                    debug_assert_eq!(numerator.k.len(), levels);
                    determined[level] = Some((numerator, denominator));
                }
                _ => {}
            }
        }
        if !progress {
            break;
        }
    }
    used.iter().all(|&used| used).then_some(determined)
}

/// Confine `cell` by a constraint. `Some(false)` when no iteration is left;
/// `None` when the constraint relates several iterators.
fn restrict(cell: &mut Cell, constraint: Constraint) -> Option<bool> {
    let lin = match &constraint {
        Constraint::Range(lin, ..) | Constraint::Mod(lin, ..) => lin,
    };
    let terms = lin
        .k
        .iter()
        .enumerate()
        .filter(|(_, x)| **x != 0)
        .map(|(level, x)| (level, *x))
        .collect::<Vec<_>>();
    match terms.as_slice() {
        [] => Some(match constraint {
            Constraint::Range(lin, low, high) => {
                low.is_none_or(|low| lin.c >= low) && high.is_none_or(|high| lin.c <= high)
            }
            Constraint::Mod(lin, modulus, residue) => (lin.c - residue).rem_euclid(modulus) == 0,
        }),
        [(level, coefficient)] => {
            let (level, coefficient) = (*level, *coefficient);
            let iterator = cell[level];
            let confined = match constraint {
                Constraint::Range(lin, low, high) => {
                    // low <= coefficient * k + c <= high
                    let (low, high) = if coefficient > 0 {
                        (
                            low.map(|low| ceil_div_wide(low - lin.c, coefficient)),
                            high.map(|high| floor_div_wide(high - lin.c, coefficient)),
                        )
                    } else {
                        (
                            high.map(|high| ceil_div_wide(high - lin.c, coefficient)),
                            low.map(|low| floor_div_wide(low - lin.c, coefficient)),
                        )
                    };
                    let clamp = |value: i128| {
                        value.clamp(iterator.min as i128 - 1, iterator.max as i128 + 1) as isize
                    };
                    let low = low.map_or(iterator.min, clamp);
                    let high = high.map_or(iterator.max, clamp);
                    iterator.within(low, high).ok()?
                }
                Constraint::Mod(lin, modulus, residue) => {
                    let modulus = isize::try_from(modulus).ok()?;
                    let coefficient =
                        isize::try_from(coefficient.rem_euclid(modulus as i128)).ok()?;
                    let constant =
                        isize::try_from((residue - lin.c).rem_euclid(modulus as i128)).ok()?;
                    match solve_congruence(coefficient, Some(constant), modulus)? {
                        Some((first, period)) => iterator
                            .confined(iterator.min, iterator.max, period, first)
                            .ok()?,
                        None => None,
                    }
                }
            };
            match confined {
                Some(confined) => {
                    cell[level] = confined;
                    Some(true)
                }
                None => Some(false),
            }
        }
        _ => None,
    }
}

type Read = (VarId, VarIndex, VarSelect, Option<MemberSelectDomain>);

/// For a loop whose values do not step additively or that breaks, the
/// iterator over the order of its iterations, at the one numbered `ordinal`.
/// No expression reads it, so a position that moves with the loop's own
/// values is not affine in it.
pub(super) fn ordinal_iterator(
    statement: &ForStatement,
    ordinal: isize,
) -> Option<CountedIterator> {
    (matches!(statement.range, ForRange::Stepped { .. })
        || crate::ir::peel::has_own_break(&statement.body))
    .then(|| CountedIterator::new(VarId::SYNTHETIC, ordinal, ordinal))
}

/// Each statement of `specialized`, a body specialized from `original`
/// with the same shape, paired with the statement it comes from. A side the
/// specialization dropped pairs nothing.
pub(super) fn statement_origins(
    original: &[Statement],
    specialized: &[Statement],
    origins: &mut Vec<(*const Statement, *const Statement)>,
) {
    for (original, specialized) in original.iter().zip(specialized) {
        origins.push((
            std::ptr::from_ref(specialized),
            std::ptr::from_ref(original),
        ));
        match (original, specialized) {
            (Statement::If(original), Statement::If(specialized)) => {
                statement_origins(&original.true_side, &specialized.true_side, origins);
                statement_origins(&original.false_side, &specialized.false_side, origins);
            }
            (Statement::IfReset(original), Statement::IfReset(specialized)) => {
                statement_origins(&original.true_side, &specialized.true_side, origins);
                statement_origins(&original.false_side, &specialized.false_side, origins);
            }
            (Statement::Case(original), Statement::Case(specialized)) => {
                for (original, specialized) in original.arms.iter().zip(&specialized.arms) {
                    statement_origins(&original.body, &specialized.body, origins);
                }
                statement_origins(&original.default, &specialized.default, origins);
            }
            (Statement::For(original), Statement::For(specialized)) => {
                statement_origins(&original.body, &specialized.body, origins);
            }
            _ => {}
        }
    }
}

/// The writer of a call output: the output's own place, as each output of a
/// call is one lvalue.
pub(super) fn output_writer(destination: &AssignDestination) -> WriterId {
    (destination.token, 0)
}

/// The variable reads a statement makes before its nested statements.
fn statement_reads(statement: &Statement) -> Vec<Read> {
    let mut reads = Vec::new();
    let mut collect = |expression: &Expression| collect_reads(expression, &mut reads);
    match statement {
        Statement::Assign(assign) => {
            collect(&assign.expr);
            for destination in &assign.dst {
                for expression in destination
                    .index
                    .expressions()
                    .chain(destination.select.0.iter())
                    .chain(destination.select.1.as_ref().map(|(_, end)| end))
                {
                    collect(expression);
                }
            }
        }
        Statement::FunctionCall(call) => {
            for input in call.inputs.values() {
                collect(input);
            }
            for destination in call.outputs.values().flatten() {
                for expression in destination
                    .index
                    .expressions()
                    .chain(destination.select.0.iter())
                    .chain(destination.select.1.as_ref().map(|(_, end)| end))
                {
                    collect(expression);
                }
            }
        }
        Statement::If(statement) => collect(&statement.cond),
        Statement::Case(statement) => {
            collect(&statement.case_target);
            for pattern in statement.arms.iter().flat_map(|arm| &arm.patterns) {
                match pattern {
                    CasePattern::Eq(value) => collect(value),
                    CasePattern::Range { lo, hi, .. } => {
                        collect(lo);
                        collect(hi);
                    }
                }
            }
        }
        _ => {}
    }
    reads
}

/// The variables a statement reads before its nested statements and those
/// it writes.
fn statement_accesses(statement: &Statement) -> Vec<VarId> {
    let mut ids = statement_reads(statement)
        .into_iter()
        .map(|(id, ..)| id)
        .collect::<Vec<_>>();
    match statement {
        Statement::Assign(assign) => {
            ids.extend(assign.dst.iter().map(|destination| destination.id));
        }
        Statement::FunctionCall(call) => {
            ids.extend(
                call.outputs
                    .values()
                    .flatten()
                    .map(|destination| destination.id),
            );
        }
        _ => {}
    }
    ids
}

/// The variable reads of an expression, including those in coordinates.
fn collect_reads(expression: &Expression, reads: &mut Vec<Read>) {
    if let Expression::Term(factor) = expression {
        match factor.as_ref() {
            Factor::Variable(id, index, select, comptime) => reads.push((
                *id,
                index.clone(),
                select.clone(),
                comptime.member_select_domain,
            )),
            // Only a sign cast reads the value of its input.
            Factor::SystemFunctionCall(call) => {
                if let SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) =
                    &call.kind
                {
                    collect_reads(&input.0, reads);
                }
                return;
            }
            _ => {}
        }
    }
    for child in children(expression) {
        collect_reads(child, reads);
    }
}

impl<'a, 's> ProcedureAnalysis<'a, 's> {
    /// Evaluate `statement` once for each set of iterations on which its
    /// reads have the same last writers, with the iterators confined to it.
    pub(super) fn eval_split_statement(
        &mut self,
        statement: &Statement,
        cells: Vec<Cell>,
        controls: &[crate::comb_loop_detect::ssa::VersionId],
    ) -> super::FlowResult {
        let branch = self.next_branch_id(cells.len());
        let parent_condition = self.path_condition.clone();
        let previous_split = self.split_statement.replace(std::ptr::from_ref(statement));
        let iterators = self.counted_iterators.clone();
        let mut branches = Vec::with_capacity(cells.len());
        for (arm, cell) in cells.into_iter().enumerate() {
            self.choose_path(&parent_condition, branch, arm);
            let checkpoint = self.ssa.checkpoint();
            self.counted_iterators = cell;
            let flow = self.eval_statement(statement, controls);
            self.counted_iterators.clone_from(&iterators);
            let state = self.ssa.capture_and_rollback(checkpoint);
            branches.push((flow, state, self.path_condition.clone()));
        }
        self.split_statement = previous_split;
        self.path_condition = parent_condition;
        self.merge_branches(branches, &[])
    }
}
