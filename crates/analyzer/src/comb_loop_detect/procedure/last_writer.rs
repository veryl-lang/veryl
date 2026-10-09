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
use crate::comb_loop_detect::region::{ArraySpan, NodeKey, PackedSpan};
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
    /// The values the loop takes.
    values: CountedIterator,
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

/// The last writers of the bits of a key, each with whether it writes on the
/// read's own iteration.
pub(super) type KeySources = Vec<(Source, bool)>;

impl LastWriterRead {
    pub(super) fn id(&self) -> VarId {
        self.id
    }
}

/// A read whose last writers are solved for the bits of each key it takes.
pub(super) struct LastWriterRead {
    id: VarId,
    elements: Option<AffineElements>,
    select: VarSelect,
    member_select_domain: Option<MemberSelectDomain>,
    /// The last writers solved for each span of bits, `None` for a key
    /// without one, each with whether it writes on the read's own iteration.
    solved: HashMap<Option<(usize, usize)>, Option<KeySources>>,
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
    /// The variables each write reads, with those of the branches on data
    /// around it, by the variable written.
    feeds: HashMap<VarId, crate::HashSet<VarId>>,
    /// The variables the branches on data around the statements scanned
    /// read.
    guards: Vec<VarId>,
    /// Each enclosing loop that breaks, at its place, and whether a break of
    /// it has been scanned: what follows a break may not run. A runtime loop
    /// is one from its start.
    breaking: Vec<(Vec<Step>, bool)>,
}

impl Scan {
    /// Record that writes of each of `written` read what `statement` and
    /// the branches on data around it read.
    fn feed(&mut self, written: impl Iterator<Item = VarId>, statement: &Statement) {
        let reads = statement_reads(statement)
            .into_iter()
            .map(|(id, ..)| id)
            .chain(self.guards.iter().copied())
            .collect::<Vec<_>>();
        for id in written {
            self.feeds
                .entry(id)
                .or_default()
                .extend(reads.iter().copied());
        }
    }

    /// The variables whose values differ between instances of their writes:
    /// written at positions that move with the iterators, on only some
    /// iterations, from what reads an iterator or such a variable, or from
    /// another variable the nest writes, whose first iteration reads what
    /// the loop entered with and the others what an iteration wrote; and
    /// that variable too. Every other variable takes, on each iteration
    /// that writes it, a value of the same sources or of its own previous
    /// one, which the iteration's closure relates exactly.
    fn instance_dependent(&self) -> crate::HashSet<VarId> {
        let iterators = self
            .loops
            .iter()
            .map(|scope_loop| scope_loop.iterator)
            .collect::<crate::HashSet<_>>();
        let moves = |sub: &SubWriter| {
            sub.access
                .elements
                .iter()
                .chain([&sub.access.flat])
                .any(|element| !element.terms.is_empty())
                || matches!(&sub.access.bits, Bits::Range(low, _) if !low.terms.is_empty())
        };
        let mut dependent = self
            .writers
            .iter()
            .filter(|(id, subs)| self.confined.contains(*id) || subs.iter().any(moves))
            .map(|(&id, _)| id)
            .collect::<crate::HashSet<_>>();
        for (&id, reads) in &self.feeds {
            for read in reads {
                if *read != id && self.writers.contains_key(read) {
                    dependent.insert(id);
                    dependent.insert(*read);
                }
            }
        }
        loop {
            let before = dependent.len();
            for (&id, reads) in &self.feeds {
                if !dependent.contains(&id)
                    && reads
                        .iter()
                        .any(|read| iterators.contains(read) || dependent.contains(read))
                {
                    dependent.insert(id);
                }
            }
            if dependent.len() == before {
                return dependent;
            }
        }
    }
}

/// An affine expression over the read's iterators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Lin {
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
}

/// Each unknown solved as an affine value of the read's iterators, over a
/// denominator, with the constraints the solution leaves.
type Solution = (Vec<Option<(Lin, i128)>>, Vec<Constraint>);

/// `sum(u[j] * unknown[j]) == rhs`.
#[derive(Clone)]
struct Equation {
    u: Vec<i128>,
    rhs: Lin,
}

#[derive(Clone)]
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
    /// The level from which the candidate is every instance up to the one
    /// at that level, at any iterations of the loops inside it.
    earlier: Option<usize>,
    /// The levels at which the candidate is the instance `instance` gives:
    /// those the read's iterations give and, inside `earlier`, those of
    /// loops no branch skips, at their last value.
    fixed: Vec<bool>,
}

impl Candidate<'_> {
    /// The write instance as the value of each of the writer's loops over the
    /// read's iterators: the read's own iteration on the loops before
    /// `first_unknown`. `None` when one is not a whole affine value.
    /// Each value is a numerator over a denominator.
    fn instance_map(&self, levels: usize) -> Option<Vec<(Lin, i128)>> {
        (0..self.writer.loops.len())
            .map(|level| {
                if level < self.first_unknown {
                    return Some((Lin::iterator(levels, level), 1));
                }
                self.instance[level].clone()
            })
            .collect()
    }
}

/// A last write of a read: the write, or the value from before the outermost
/// loop when `None`, and the instance as the value of each of the writer's
/// loops over the read's iterators, when it is known exactly.
#[derive(Clone, Debug)]
pub(super) struct LastWrite {
    pub(super) source: Source,
    pub(super) instance: Option<Vec<(Lin, i128)>>,
    /// The level from which the write is every instance up to `instance`
    /// there, at any iterations of the loops inside it: the instances a
    /// write that may not take place leaves to earlier ones.
    pub(super) earlier: Option<usize>,
    /// The levels at which the write is the instance `instance` gives; with
    /// `earlier`, the instances take every value at the others after it.
    pub(super) fixed: Vec<bool>,
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

    /// The positions of the table of `writer` for a key with `domain`: one
    /// for each of its instances on the array axis, and on the packed axis
    /// the key's bits that it can write. A table has no value where its write
    /// never stores.
    pub(super) fn table_domain(
        &self,
        id: VarId,
        writer: WriterId,
        domain: crate::comb_loop_detect::ssa::PositionDomain,
    ) -> crate::comb_loop_detect::ssa::PositionDomain {
        let Some((_, positions)) = self.table_index(id, writer) else {
            return domain;
        };
        let bits = self.extents.get(&(id, writer)).and_then(|(_, bits)| *bits);
        let clip = |start: usize, length: usize, (low, high): (isize, isize)| {
            let low = usize::try_from(low.max(0)).ok()?.max(start);
            let high = usize::try_from(high).ok()?.min(start + length - 1);
            (low <= high).then(|| (low, high - low + 1))
        };
        let packed = match bits {
            Some(bits) => clip(domain.packed_start, domain.packed_length, bits),
            None => Some((domain.packed_start, domain.packed_length)),
        };
        match packed {
            Some((packed_start, packed_length)) => crate::comb_loop_detect::ssa::PositionDomain {
                array_start: positions.start,
                array_length: positions.length,
                packed_start,
                packed_length,
            },
            None => domain,
        }
    }

    /// The index of the tables of `writer` over the iterators of its loops,
    /// and the positions it takes. A write whose element tells its instance
    /// apart is indexed by the element, as storage is; any other, such as a
    /// write in place or one an inner loop repeats, by its instance.
    pub(super) fn table_index(
        &self,
        id: VarId,
        writer: WriterId,
    ) -> Option<(AffineIndex, ArraySpan)> {
        let sub = self.writers.get(&id)?.iter().find(|sub| sub.id == writer)?;
        if self.tells_instances_apart(&sub.access.flat, &sub.loops)
            && let Some(&((first, last), _)) = self.extents.get(&(id, writer))
        {
            let start = usize::try_from(first).ok()?;
            let length = usize::try_from(last)
                .ok()?
                .checked_sub(start)?
                .checked_add(1)?;
            return Some((sub.access.flat.clone(), ArraySpan { start, length }));
        }
        let (index, instances) = self.instance_index(id, writer)?;
        Some((
            index,
            ArraySpan {
                start: 0,
                length: instances,
            },
        ))
    }

    /// The map from the positions of the tables of `writer` to the index
    /// of the instance each holds, see `instance_index`. `None` when the
    /// positions are elements that several loops move.
    fn instance_map(
        &self,
        id: VarId,
        writer: WriterId,
    ) -> Option<crate::comb_loop_detect::position::Map> {
        let (index, _) = self.table_index(id, writer)?;
        let (instances, _) = self.instance_index(id, writer)?;
        if index == instances {
            return Some(crate::comb_loop_detect::position::Map {
                crossed: false,
                modulus: 1,
                residue: 0,
                base: 0,
                step: 1,
            });
        }
        // An element `coefficient * iterator + constant` of the one loop.
        let sub = self.writers.get(&id)?.iter().find(|sub| sub.id == writer)?;
        let [level] = sub.loops.as_slice() else {
            return None;
        };
        let scope_loop = &self.loops[*level];
        let [(iterator, coefficient)] = index.terms.as_slice() else {
            return None;
        };
        if *iterator != scope_loop.iterator || *coefficient == 0 {
            return None;
        }
        // The element is `residue + modulus * t`, the instance
        // `iterator - min`.
        let modulus = coefficient.checked_abs()?;
        let residue = index.constant.rem_euclid(modulus);
        let quotient = (index.constant - residue) / modulus;
        let min = scope_loop.values.min;
        let (base, step) = if *coefficient > 0 {
            // t = iterator + quotient.
            (quotient.checked_neg()?.checked_sub(min)?, 1)
        } else {
            // t = quotient - iterator.
            (quotient.checked_sub(min)?, -1)
        };
        Some(crate::comb_loop_detect::position::Map {
            crossed: false,
            modulus,
            residue,
            base,
            step,
        })
    }

    /// The positions the tables of `writer` take.
    pub(super) fn table_domain_span(&self, id: VarId, writer: WriterId) -> Option<ArraySpan> {
        self.table_index(id, writer).map(|(_, span)| span)
    }

    /// Whether `index` takes a different position on each iteration of
    /// `loops`: every loop with more than one value moves it, each by a
    /// step beyond the positions of the finer ones.
    fn tells_instances_apart(&self, index: &AffineIndex, loops: &[usize]) -> bool {
        let mut terms = Vec::new();
        for &level in loops {
            let scope_loop = &self.loops[level];
            let values = scope_loop.values;
            if values.min == values.max {
                continue;
            }
            let Some(&(_, coefficient)) = index
                .terms
                .iter()
                .find(|(id, _)| *id == scope_loop.iterator)
            else {
                return false;
            };
            let coefficient = (coefficient as i128).abs();
            terms.push((
                coefficient * values.modulus as i128,
                coefficient * (values.max as i128 - values.min as i128),
            ));
        }
        terms.sort_unstable();
        let mut covered = 0i128;
        for (step, range) in terms {
            if step <= covered {
                return false;
            }
            covered += range;
        }
        true
    }

    /// The position of each instance of `writer` in its tables, as an affine
    /// index over the iterators of its loops, and the number of positions.
    /// Each loop's values take consecutive positions within one value of the
    /// loops around it, so a stride leaves positions no instance takes.
    pub(super) fn instance_index(
        &self,
        id: VarId,
        writer: WriterId,
    ) -> Option<(AffineIndex, usize)> {
        let sub = self.writers.get(&id)?.iter().find(|sub| sub.id == writer)?;
        let mut index = AffineIndex::default();
        let mut stride = 1isize;
        for &level in sub.loops.iter().rev() {
            let scope_loop = &self.loops[level];
            let values = scope_loop.values;
            let mut term = AffineIndex::default();
            term.add_scaled(&AffineIndex::variable(scope_loop.iterator), stride)?;
            term.constant = values.min.checked_mul(stride)?.checked_neg()?;
            index.add_scaled(&term, 1)?;
            stride = stride.checked_mul(values.max.checked_sub(values.min)?.checked_add(1)?)?;
        }
        Some((index, usize::try_from(stride).ok()?))
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
        // Each scope keeps tables of its own.
        self.writer_scopes += 1;
        let iterator = self.counted_iterators[0];
        // The evaluation of each loop forgets the value its iterator may keep
        // from conversion; so does the scan, or branches on it would look
        // constant.
        self.forget_runtime_iterator_value(statement.var_id);
        let mut scan = Scan::default();
        scan.loops.push(ScopeLoop {
            iterator: statement.var_id,
            ascending: !matches!(statement.range, ForRange::Reverse { .. }),
            values: iterator,
        });
        let mut path = Vec::new();
        let mut branches = Vec::new();
        let scanned = self.scan_block(&mut scan, &statement.body, &mut path, &mut branches);
        self.counted_iterators.truncate(1);
        self.counted_iterators[0] = iterator;
        if scanned.is_none() {
            return;
        }
        let dependent = scan.instance_dependent();
        let Scan {
            loops,
            mut writers,
            places,
            unknown,
            ..
        } = scan;
        // Storage whose writes stay in place is strongly updated by the
        // symbolic iteration, which reads the right instance unless the read
        // or the write is on only some iterations: an earlier iteration's
        // write is then all a later read can see, never the value from
        // before the loop.
        // Every write whose instances differ keeps a table of them, so
        // that a read or the storage after the loops takes the instance it
        // reaches.
        writers.retain(|id, _| !unknown.contains(id) && dependent.contains(id));
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
                // What the key holds before the loop, as state: a read of it
                // takes it as data.
                snapshots.insert(key, self.ssa.retained(value, position_domain(key.1, span)));
                for &writer in &ids {
                    // Each table starts empty, as its own value: tables are
                    // separate inputs of the loop transfer. A table that is
                    // a circuit node is read as what the loops leave there.
                    if self.external_tables {
                        continue;
                    }
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
        self.instance_tables = true;
        extents.retain(|(id, _), _| tracked.contains_key(id));
        let scope = Rc::new(WriterScope {
            loops,
            writers: tracked,
            extents,
            places,
            snapshots,
            call_depth: self.call_frames.len(),
        });
        if self.external_tables {
            let mut ids = scope.writers.keys().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            for id in ids {
                let mut writers = scope.writers[&id]
                    .iter()
                    .map(|sub| sub.id)
                    .collect::<Vec<_>>();
                writers.sort_unstable();
                writers.dedup();
                for key in self.keys_for_id(id) {
                    let Some(span) = self.key_span(key) else {
                        continue;
                    };
                    for &writer in &writers {
                        let domain = scope.table_domain(id, writer, position_domain(key.1, span));
                        let arms = self.instance_arms(&scope, id, writer);
                        let table = self.writer_key(key, writer);
                        self.register_table(table, domain, arms);
                    }
                }
            }
        }
        self.writer_scope = Some(scope);
    }

    /// The arms the writes of `writer` take, each on the instance a position
    /// of its tables holds. Inside a branch, loops of instances numbered
    /// consecutively give each branch instance as many consecutive positions
    /// as they take values: one map for each of those, over the positions it
    /// takes, charged to the procedure's work. A table whose positions give
    /// its instances by no map takes none.
    fn instance_arms(
        &mut self,
        scope: &WriterScope,
        id: VarId,
        writer: WriterId,
    ) -> Vec<crate::comb_loop_detect::graph::InstanceArm> {
        let Some(sub) = scope
            .writers
            .get(&id)
            .and_then(|subs| subs.iter().find(|sub| sub.id == writer))
        else {
            return Vec::new();
        };
        let Some(instance) = scope.instance_map(id, writer) else {
            return Vec::new();
        };
        let instance_indexed = instance.modulus == 1 && instance.base == 0 && instance.step == 1;
        let mut arms = Vec::new();
        for branch in &sub.branches {
            let around = place_loops(&branch.place).len();
            if around > sub.loops.len() {
                continue;
            }
            // The positions each branch instance takes.
            let inside = sub.loops[around..]
                .iter()
                .try_fold(1isize, |count, &level| {
                    let values = scope.loops[level].values;
                    count.checked_mul(values.max.checked_sub(values.min)?.checked_add(1)?)
                });
            let maps = match inside {
                Some(1) => vec![instance],
                Some(count) if instance_indexed => {
                    let Some(work) = usize::try_from(count).ok() else {
                        continue;
                    };
                    if !self.reserve_guard_work(work) {
                        self.exhaust_work();
                        return Vec::new();
                    }
                    (0..count)
                        .map(|residue| crate::comb_loop_detect::position::Map {
                            crossed: false,
                            modulus: count,
                            residue,
                            base: 0,
                            step: 1,
                        })
                        .collect()
                }
                _ => continue,
            };
            let identity = self.instance_branch(&branch.place);
            for map in maps {
                arms.push(crate::comb_loop_detect::graph::InstanceArm {
                    branch: identity,
                    arm: branch.arm,
                    arms: branch.arms,
                    instance: map,
                });
            }
        }
        arms
    }

    pub(super) fn close_writer_scope(&mut self) {
        self.writer_scope = None;
        self.imprecise_tables.clear();
        self.held_tables.clear();
        self.statement_places.clear();
    }

    /// The table of `writer` for `key` as a read takes it: what the loops
    /// leave there when the table is a circuit node, else what the
    /// iterations evaluated so far left.
    pub(super) fn read_table(&mut self, key: NodeKey, writer: WriterId) -> VersionId {
        let table = self.writer_key(key, writer);
        if self.table_indices.contains_key(&table) {
            self.ssa.entry(table)
        } else {
            self.ssa.read(table)
        }
    }

    pub(super) fn writer_key(&self, key: NodeKey, writer: WriterId) -> SsaKey {
        SsaKey {
            writer: Some((writer, self.writer_scopes)),
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
            let level = self.counted_iterators.len();
            let iterator = ordinal_iterator(statement, level, isize::try_from(ordinal).ok()?)?;
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
        let guards = match statement {
            Statement::If(_) | Statement::Case(_) => statement_reads(statement)
                .into_iter()
                .map(|(id, ..)| id)
                .collect(),
            _ => Vec::new(),
        };
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
                scan.feed(
                    assign.dst.iter().map(|destination| destination.id),
                    statement,
                );
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
                let guarded = scan.guards.len();
                scan.guards.extend(guards.iter().copied());
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
                scan.guards.truncate(guarded);
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
                let guarded = scan.guards.len();
                scan.guards.extend(guards.iter().copied());
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
                scan.guards.truncate(guarded);
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
                        match ordinal_iterator(statement, self.counted_iterators.len(), 0) {
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
                        ascending: is_ordinal(iterator.id)
                            || !matches!(statement.range, ForRange::Reverse { .. }),
                        values: iterator,
                    });
                }
                let breaks = crate::ir::peel::has_own_break(&statement.body);
                path.push(Step::Loop(id));
                // A break skips the later iterations of the loop it breaks.
                if breaks {
                    scan.breaking.push((path.clone(), false));
                }
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
                scan.feed(
                    destinations.iter().map(|destination| destination.id),
                    statement,
                );
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
    ) -> Option<KeySources> {
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
                let mut sources: Vec<(Source, bool)> = Vec::new();
                for (cell, cell_sources) in cells {
                    let flat = read.elements.as_ref().map(|elements| &elements.flat);
                    let own = self.written_on_each_iteration(read.id, flat, &cell_sources, &cell);
                    for source in cell_sources {
                        let own = own.contains(&source);
                        match sources.iter_mut().find(|(other, _)| *other == source) {
                            Some((_, both)) => *both &= own,
                            None => sources.push((source, own)),
                        }
                    }
                }
                sources
            });
        read.solved.insert(bits, solved.clone());
        solved
    }

    /// The last writers `sources` of a read of `id` on the iterations of
    /// `cell` whose values are written on the read's own iteration: those
    /// that always write before the read, when on each iteration of the cell
    /// one of them does, so no value of an earlier iteration is the last.
    /// Each then holds the value of its own iterations, which the
    /// definitions it holds keep to.
    fn written_on_each_iteration(
        &self,
        id: VarId,
        read: Option<&AffineIndex>,
        sources: &[Source],
        cell: &Cell,
    ) -> Vec<Source> {
        let (Some(scope), Some(place)) = (self.active_writer_scope(), self.statement_places.last())
        else {
            return Vec::new();
        };
        let loops = place_loops(place);
        let Some(subs) = scope.writers.get(&id) else {
            return Vec::new();
        };
        // A write in loops that finished before the read, on every one of
        // their iterations, at positions they alone move, writes the same
        // positions on each iteration of the read's loops. One that also
        // moves with those writes the element read on each of them when
        // one inner value reaches it.
        let finished = |sub: &SubWriter| {
            let inner = &sub.loops[loops.len()..];
            let iterators = inner
                .iter()
                .map(|&id| scope.loops[id].iterator)
                .collect::<Vec<_>>();
            if !inner
                .iter()
                .zip(&sub.domains[loops.len()..])
                .all(|(&id, reached)| *reached == scope.loops[id].values)
            {
                return false;
            }
            let terms = sub
                .access
                .elements
                .iter()
                .chain(match &sub.access.bits {
                    Bits::Range(low, _) => Some(low),
                    Bits::Whole => None,
                })
                .flat_map(|index| &index.terms)
                .collect::<Vec<_>>();
            if terms.iter().all(|(id, _)| iterators.contains(id)) {
                return true;
            }
            let (Some(read), Bits::Whole | Bits::Range(..)) = (read, &sub.access.bits) else {
                return false;
            };
            matches!(&sub.access.bits, Bits::Whole)
                && reaches_each_iteration(&sub.access.flat, read, cell, &loops, &scope, inner)
        };
        let before = subs
            .iter()
            .filter(|sub| {
                sources.contains(&Some(sub.id))
                    && sub.branches.is_empty()
                    && sub.loops.starts_with(&loops)
                    && (sub.loops.len() == loops.len() || finished(sub))
                    && relation(place, &sub.path) == Relation::Before
            })
            .collect::<Vec<_>>();
        // The iterations of each writer within the cell, which differ from
        // the cell's on at most one level, and together cover it there.
        let covers = (0..cell.len()).any(|level| {
            let mut counted = 0isize;
            let mut pieces: Vec<CountedIterator> = Vec::new();
            for sub in &before {
                let mut piece = None;
                for (at, (read, reached)) in cell.iter().zip(&sub.domains).enumerate() {
                    let inside =
                        read.confined(reached.min, reached.max, reached.modulus, reached.residue);
                    match inside {
                        Ok(Some(inside)) if at == level => piece = Some(inside),
                        Ok(Some(inside)) if inside == *read => {}
                        _ => {
                            piece = None;
                            break;
                        }
                    }
                }
                let Some(piece) = piece else {
                    continue;
                };
                // Pieces of exclusive writers never meet.
                if pieces
                    .iter()
                    .any(|other| matches!(intersect(other, &piece), Ok(Some(_)) | Err(_)))
                {
                    return false;
                }
                let Some(count) = piece.count().and_then(|count| counted.checked_add(count)) else {
                    return false;
                };
                counted = count;
                pieces.push(piece);
            }
            Some(counted) == cell[level].count()
        });
        if !covers {
            return Vec::new();
        }
        before.iter().map(|sub| Some(sub.id)).collect()
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

    /// The last write instances of the bits of `key` that `read` takes, on
    /// each set of its iterations.
    pub(super) fn key_last_writes(
        &mut self,
        read: &LastWriterRead,
        key: NodeKey,
    ) -> Option<Vec<(Cell, Vec<LastWrite>)>> {
        if self.runtime_loop_depth > 0 {
            return None;
        }
        let scope = self.active_writer_scope()?;
        let writers = scope.writers.get(&read.id)?;
        let place = self.statement_places.last()?.clone();
        let mut access = self.access_forms(
            read.id,
            read.elements.as_ref(),
            &read.select,
            read.member_select_domain,
        )?;
        if let Some(span) = self.key_span(key) {
            access.bits = access.bits.within(span);
        }
        self.solve_last_writes(&scope, writers, &place, &access, &[])
    }

    /// The position in the tables of `writer` of `instance`, the value of
    /// each of its loops over the read's iterators, as an affine index over
    /// those iterators.
    pub(super) fn table_position(
        &self,
        id: VarId,
        writer: WriterId,
        instance: &[(Lin, i128)],
    ) -> Option<AffineIndex> {
        let iterators = self
            .counted_iterators
            .iter()
            .map(|iterator| iterator.id)
            .collect::<Vec<_>>();
        self.table_position_over(id, writer, instance, &iterators)
    }

    /// The positions in the tables of `writer` that `write` takes on the
    /// iterations being evaluated, when it is every instance up to its own
    /// from a level: the hull of those instances' positions. `None` for a
    /// write of one instance or one not known exactly.
    pub(super) fn earlier_region(
        &mut self,
        id: VarId,
        write: &LastWrite,
        iterators: &[VarId],
    ) -> Option<ArraySpan> {
        let level = write.earlier?;
        let writer = write.source?;
        let instance = write.instance.as_ref()?;
        let scope = self.active_writer_scope()?;
        let sub = scope
            .writers
            .get(&id)?
            .iter()
            .find(|sub| sub.id == writer)?;
        let levels = iterators.len();
        let (latest, 1) = instance.get(level)? else {
            return None;
        };
        let mut latest_index = AffineIndex {
            terms: Vec::new(),
            constant: isize::try_from(latest.c).ok()?,
        };
        for (position, &coefficient) in latest.k.iter().enumerate() {
            if coefficient != 0 {
                let term = AffineIndex::variable(*iterators.get(position)?);
                latest_index.add_scaled(&term, isize::try_from(coefficient).ok()?)?;
            }
        }
        let (low, high) = self.affine_hull(&latest_index)?;
        // `level` and each level inside it the read does not give are free
        // iterators over the values their instances may take.
        let free_levels = (level..sub.loops.len())
            .filter(|&inner| inner == level || !write.fixed.get(inner).copied().unwrap_or(false))
            .collect::<Vec<_>>();
        let free = free_levels.len();
        let mut extended = iterators.to_vec();
        let mut ranges = Vec::new();
        for (offset, &inner) in free_levels.iter().enumerate() {
            let domain = &sub.domains[inner];
            let id = ordinal_id(u32::MAX as usize / 2 + 1 + offset)?;
            let mut range = CountedIterator { id, ..*domain };
            if offset == 0 {
                if scope.loops[sub.loops[level]].ascending {
                    range.max = high.min(domain.max);
                } else {
                    range.min = low.max(domain.min);
                }
                if range.min > range.max {
                    return None;
                }
            }
            extended.push(id);
            ranges.push(range);
        }
        let widen = |lin: &Lin| Lin {
            k: lin
                .k
                .iter()
                .copied()
                .chain(std::iter::repeat_n(0, free))
                .collect(),
            c: lin.c,
        };
        let mut spread = Vec::with_capacity(sub.loops.len());
        for (inner, (value, denominator)) in instance.iter().enumerate() {
            spread.push(match free_levels.iter().position(|&free| free == inner) {
                Some(offset) => (Lin::iterator(levels + free, levels + offset), 1),
                None => (widen(value), *denominator),
            });
        }
        let position = self.table_position_over(id, writer, &spread, &extended)?;
        let saved = self.counted_iterators.len();
        self.counted_iterators.extend(ranges);
        let hull = self.affine_hull(&position);
        self.counted_iterators.truncate(saved);
        let (first, last) = hull?;
        let start = usize::try_from(first).ok()?;
        let length = usize::try_from(last)
            .ok()?
            .checked_sub(start)?
            .checked_add(1)?;
        Some(ArraySpan { start, length })
    }

    /// `table_position` where an iterator taking positions a step apart
    /// gives an instance at a fraction of them: over the iterator `u` of
    /// consecutive values with `iterator = residue + modulus * u`. Gives the
    /// position, `destination` over the same iterators, and those iterators.
    pub(super) fn stepped_table_position(
        &self,
        id: VarId,
        writer: WriterId,
        instance: &[(Lin, i128)],
        destination: &super::SampledAffineIndex,
    ) -> Option<(AffineIndex, super::SampledAffineIndex, Cell)> {
        let level = self
            .counted_iterators
            .iter()
            .position(|iterator| iterator.modulus > 1 && iterator.min < iterator.max)?;
        let stepped = self.counted_iterators[level];
        let (modulus, residue) = (stepped.modulus, stepped.min);
        let consecutive = ordinal_id(u32::MAX as usize / 2 + 64)?;
        let mut cell = self.counted_iterators.clone();
        cell[level] = CountedIterator::new(consecutive, 0, (stepped.max - residue) / modulus);
        let instance = instance
            .iter()
            .map(|(value, denominator)| {
                let mut value = value.clone();
                let coefficient = *value.k.get(level)?;
                value.c = value
                    .c
                    .checked_add(coefficient.checked_mul(residue as i128)?)?;
                value.k[level] = coefficient.checked_mul(modulus as i128)?;
                Some((value, *denominator))
            })
            .collect::<Option<Vec<_>>>()?;
        let iterators = cell.iter().map(|iterator| iterator.id).collect::<Vec<_>>();
        let position = self.table_position_over(id, writer, &instance, &iterators)?;
        // The destination over the same iterators.
        let mut index = AffineIndex {
            terms: Vec::new(),
            constant: destination.index.constant,
        };
        for &(term, coefficient) in &destination.index.terms {
            if term == stepped.id {
                index.constant = index
                    .constant
                    .checked_add(coefficient.checked_mul(residue)?)?;
                index.add_scaled(
                    &AffineIndex::variable(consecutive),
                    coefficient.checked_mul(modulus)?,
                )?;
            } else {
                index.add_scaled(&AffineIndex::variable(term), coefficient)?;
            }
        }
        // The iterator of consecutive values is no variable a read samples.
        let mut versions = destination.versions.clone();
        for (term, read) in &mut versions {
            if *term == stepped.id {
                *term = consecutive;
                read.clear();
            }
        }
        Some((
            position,
            super::SampledAffineIndex { index, versions },
            cell,
        ))
    }

    /// `table_position` over the read's iterators `iterators`.
    fn table_position_over(
        &self,
        id: VarId,
        writer: WriterId,
        instance: &[(Lin, i128)],
        iterators: &[VarId],
    ) -> Option<AffineIndex> {
        let scope = self.active_writer_scope()?;
        let sub = scope
            .writers
            .get(&id)?
            .iter()
            .find(|sub| sub.id == writer)?;
        let (index, _) = scope.table_index(id, writer)?;
        let levels = iterators.len();
        // An iterator with one value on the iterations being evaluated is
        // that value.
        let fixed = iterators
            .iter()
            .map(|&iterator| {
                self.counted_iterator(iterator)
                    .filter(|values| values.min == values.max)
                    .map(|values| values.min as i128)
            })
            .collect::<Vec<_>>();
        let instance = instance
            .iter()
            .map(|(value, denominator)| {
                let mut value = value.clone();
                for (level, fixed) in fixed.iter().enumerate() {
                    if let (Some(fixed), Some(coefficient)) = (fixed, value.k.get(level).copied()) {
                        value.c = value.c.checked_add(coefficient.checked_mul(*fixed)?)?;
                        value.k[level] = 0;
                    }
                }
                Some((value, *denominator))
            })
            .collect::<Option<Vec<_>>>()?;
        let instance = instance.as_slice();
        // The position over a common denominator, which must divide it.
        let mut position = Lin::constant(levels, index.constant as i128);
        let mut denominator = 1i128;
        for &(iterator, stride) in &index.terms {
            let level = sub
                .loops
                .iter()
                .position(|&lp| scope.loops[lp].iterator == iterator)?;
            let (value, value_denominator) = instance.get(level)?;
            position = position
                .scaled(*value_denominator)?
                .plus(&value.scaled(denominator)?, stride as i128)?;
            denominator = denominator.checked_mul(*value_denominator)?;
        }
        if position.c % denominator != 0 || position.k.iter().any(|k| k % denominator != 0) {
            return None;
        }
        let position = Lin {
            k: position.k.iter().map(|k| k / denominator).collect(),
            c: position.c / denominator,
        };
        let mut affine = AffineIndex {
            terms: Vec::new(),
            constant: isize::try_from(position.c).ok()?,
        };
        for (level, &coefficient) in position.k.iter().enumerate() {
            if coefficient != 0 {
                let term = AffineIndex::variable(*iterators.get(level)?);
                affine.add_scaled(&term, isize::try_from(coefficient).ok()?)?;
            }
        }
        Some(affine)
    }

    /// Bind each storage key the scope covers to what its outermost loop
    /// leaves there, on its last iteration `last`, after its `statements`:
    /// at each position, the table of the last write instance there, or the
    /// value from before the loop. A key whose last writes are not solved
    /// keeps the value the loop's evaluation left.
    pub(super) fn bind_exit_values(&mut self, statements: usize, last: CountedIterator) {
        let Some(scope) = self.active_writer_scope() else {
            return;
        };
        let Some(position_id) = ordinal_id(u32::MAX as usize / 2) else {
            return;
        };
        let saved = std::mem::replace(&mut self.counted_iterators, vec![last]);
        let place = vec![Step::Statement(statements)];
        let mut ids = scope.writers.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        for id in ids {
            let subs = &scope.writers[&id];
            for key in self.keys_for_id(id) {
                // A table a write could not take at its instances does not
                // tell which instance left a position.
                if subs.iter().any(|sub| {
                    self.imprecise_tables
                        .get(&(key, sub.id))
                        .is_some_and(|&count| count > 0)
                }) {
                    continue;
                }
                if let Some(value) = self.exit_value(&scope, subs, &place, key, position_id) {
                    self.bind_key(key, value);
                }
            }
        }
        self.counted_iterators = saved;
    }

    fn exit_value(
        &mut self,
        scope: &WriterScope,
        subs: &[SubWriter],
        place: &[Step],
        key: NodeKey,
        position_id: VarId,
    ) -> Option<VersionId> {
        let packed = self.key_span(key)?;
        let first = isize::try_from(key.1.start).ok()?;
        let positions = CountedIterator::new(
            position_id,
            first,
            first.checked_add(isize::try_from(key.1.length).ok()?)? - 1,
        );
        let access = Access {
            elements: Vec::new(),
            flat: AffineIndex::variable(position_id),
            bits: Bits::Range(
                AffineIndex {
                    terms: Vec::new(),
                    constant: isize::try_from(packed.start).ok()?,
                },
                packed.length,
            ),
        };
        let cells = self.solve_last_writes(scope, subs, place, &access, &[positions])?;
        let iterators = [self.counted_iterators[0].id, position_id];
        let destination = super::SampledAffineIndex {
            index: AffineIndex::variable(position_id),
            versions: vec![(position_id, Vec::new())],
        };
        // The pieces of the key: each source, the map to the key's positions
        // and the positions it reaches there with those it is read at.
        // Each span with the step between the positions the cell takes.
        type Spans = Vec<(ArraySpan, usize, ArraySpan)>;
        let mut groups: Vec<(Source, super::PositionRelation, Spans)> = Vec::new();
        for (cell, writes) in cells {
            let span = ArraySpan {
                start: usize::try_from(cell[1].min).ok()?,
                length: usize::try_from(cell[1].max - cell[1].min + 1).ok()?,
            };
            // Positions a step apart are those of an iterator `u` over
            // consecutive values, `residue + modulus * u`, at which an
            // instance taken at a fraction of them is whole.
            let stepped = cell[1].modulus > 1;
            let (cell_iterators, destination, iterators) = if stepped {
                let (modulus, residue) = (cell[1].modulus, cell[1].residue);
                let id = ordinal_id(u32::MAX as usize / 2 + 64)?;
                let consecutive = CountedIterator::new(
                    id,
                    (cell[1].min - residue) / modulus,
                    (cell[1].max - residue) / modulus,
                );
                let mut index = AffineIndex {
                    terms: Vec::new(),
                    constant: residue,
                };
                index.add_scaled(&AffineIndex::variable(id), modulus)?;
                (
                    vec![cell[0], consecutive],
                    super::SampledAffineIndex {
                        index,
                        versions: vec![(id, Vec::new())],
                    },
                    [iterators[0], id],
                )
            } else {
                (cell.clone(), destination.clone(), iterators)
            };
            let over_cell = |lin: &Lin| -> Option<Lin> {
                if !stepped {
                    return Some(lin.clone());
                }
                let (modulus, residue) = (cell[1].modulus as i128, cell[1].residue as i128);
                let mut lin = lin.clone();
                lin.c = lin.c.checked_add(lin.k[1].checked_mul(residue)?)?;
                lin.k[1] = lin.k[1].checked_mul(modulus)?;
                Some(lin)
            };
            let saved = std::mem::replace(&mut self.counted_iterators, cell_iterators);
            let mut pieces = Vec::new();
            for mut write in writes {
                let Some(writer) = write.source else {
                    pieces.push((None, super::PositionRelation::identity(), span));
                    continue;
                };
                write.instance = write.instance.and_then(|instance| {
                    instance
                        .iter()
                        .map(|(value, denominator)| Some((over_cell(value)?, *denominator)))
                        .collect()
                });
                if write.earlier.is_some() {
                    // Any of the instances may be the last.
                    let region = match self.earlier_region(key.0, &write, &iterators) {
                        Some(region) => region,
                        None => scope.table_domain_span(key.0, writer)?,
                    };
                    pieces.push((
                        Some(writer),
                        super::PositionRelation {
                            array: super::Link::Unlinked,
                            packed: super::Link::IDENTITY,
                        },
                        region,
                    ));
                    continue;
                }
                let position = write.instance.as_ref().and_then(|instance| {
                    self.table_position_over(key.0, writer, instance, &iterators)
                });
                let link = position.and_then(|position| {
                    let source = super::SampledAffineIndex {
                        versions: position
                            .terms
                            .iter()
                            .map(|&(id, _)| (id, Vec::new()))
                            .collect(),
                        index: position,
                    };
                    let source = self.fold_single_values(&source);
                    let destination = self.fold_single_values(&destination);
                    let link = self.affine_link(&destination, &source, false)?;
                    let (low, high) = self.affine_hull(&source.index)?;
                    let start = usize::try_from(low).ok()?;
                    let length = usize::try_from(high).ok()?.checked_sub(start)? + 1;
                    Some((link, ArraySpan { start, length }))
                });
                pieces.push(match link {
                    Some((link, region)) => (
                        Some(writer),
                        super::PositionRelation {
                            array: link,
                            packed: super::Link::IDENTITY,
                        },
                        region,
                    ),
                    // An instance not known exactly may be any of them.
                    None => (
                        Some(writer),
                        super::PositionRelation {
                            array: super::Link::Unlinked,
                            packed: super::Link::IDENTITY,
                        },
                        scope.table_domain_span(key.0, writer)?,
                    ),
                });
            }
            self.counted_iterators = saved;
            let step = usize::try_from(cell[1].modulus).ok()?;
            for (source, relation, region) in pieces {
                match groups.iter_mut().find(|(other, other_relation, _)| {
                    *other == source && *other_relation == relation
                }) {
                    Some((_, _, spans)) => spans.push((span, step, region)),
                    None => groups.push((source, relation, vec![(span, step, region)])),
                }
            }
        }
        let mut values = Vec::new();
        for (source, relation, mut spans) in groups {
            let base = match source {
                Some(writer) => self.read_table(key, writer),
                None => *scope.snapshots.get(&key)?,
            };
            // Positions next to one another read the same way as one piece.
            spans.sort_unstable_by_key(|(span, step, _)| (span.start, *step));
            let mut merged: Spans = Vec::new();
            for (span, step, region) in spans {
                // A map relates each position of the merged span to the
                // region it read; positions unrelated to their sources read
                // every position of the region, so only an equal one merges.
                if let Some((last, 1, last_region)) = merged.last_mut()
                    && step == 1
                    && span.start <= last.start + last.length
                    && (relation.array != super::Link::Unlinked || *last_region == region)
                {
                    let end = (last.start + last.length).max(span.start + span.length);
                    last.length = end - last.start;
                    let low = last_region.start.min(region.start);
                    let high =
                        (last_region.start + last_region.length).max(region.start + region.length);
                    *last_region = ArraySpan {
                        start: low,
                        length: high - low,
                    };
                    continue;
                }
                merged.push((span, step, region));
            }
            for (span, step, region) in merged {
                // A position no write reaches keeps its value, which is state,
                // not a value computed from itself.
                if source.is_none() {
                    values.push(self.ssa.retained(base, position_domain(span, packed)));
                    continue;
                }
                let read = self.ssa.projected(base, position_domain(region, packed));
                let value = self.ssa.related_definition(vec![(read, relation)]);
                // Only the positions of the progression take the value.
                let value = self.progression_value(
                    value,
                    position_domain(span, packed),
                    super::Axis::Array,
                    step,
                )?;
                values.push(value);
            }
        }
        Some(self.ssa.phi(values))
    }

    /// The value of `key` before the outermost loop of the scope.
    pub(super) fn scope_snapshot(&self, key: NodeKey) -> Option<VersionId> {
        self.active_writer_scope()?.snapshots.get(&key).copied()
    }

    /// The value of `key` from the last writers `sources`.
    /// A value written on an earlier iteration is kept apart from the
    /// assignment that wrote it, which the read's iteration did not run.
    pub(super) fn last_writer_value(
        &mut self,
        key: NodeKey,
        sources: &[(Source, bool)],
    ) -> VersionId {
        let Some(scope) = self.writer_scope.clone() else {
            return self.read_key(key);
        };
        let mut versions = Vec::new();
        for &(source, own) in sources {
            match source {
                // A table is indexed by instances, which a read that takes
                // no instance cannot tell apart.
                Some(writer) => {
                    let _ = own;
                    let value = self.read_table(key, writer);
                    versions.push(self.ssa.related_definition(vec![(
                        value,
                        super::PositionRelation {
                            array: super::Link::Unlinked,
                            packed: super::Link::IDENTITY,
                        },
                    )]));
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
                // A table is indexed by the write's instances, which a value
                // at element positions does not tell apart: each instance
                // may hold any of its elements. A write that can be taken
                // again at its instance replaces this, see
                // `record_instance_write`.
                let version = self.ssa.related_definition(vec![(
                    version,
                    super::PositionRelation {
                        array: super::Link::Unlinked,
                        packed: super::Link::IDENTITY,
                    },
                )]);
                let table = self.writer_key(key, writer);
                let held = self.ssa.read(table);
                // On only some of the write's iterations, the others keep
                // what the table held.
                let version = if self.writes_partially(&scope, key.0, writer) {
                    self.ssa.phi(vec![held, version])
                } else {
                    version
                };
                // The table's positions bound what it holds.
                let version = match self.key_span(key) {
                    Some(packed) => {
                        let domain =
                            scope.table_domain(key.0, writer, position_domain(key.1, packed));
                        self.ssa.retained(version, domain)
                    }
                    None => version,
                };
                self.ssa.bind(table, version);
                self.held_tables.insert((key, writer), held);
                *self.imprecise_tables.entry((key, writer)).or_default() += 1;
            }
            _ => self.status = self.status.max(AnalysisStatus::Barrier),
        }
    }

    /// Whether the iterations being evaluated are only some of those of
    /// `writer`'s loops.
    fn writes_partially(&self, scope: &WriterScope, id: VarId, writer: WriterId) -> bool {
        scope
            .writers
            .get(&id)
            .and_then(|subs| subs.iter().find(|sub| sub.id == writer))
            .is_none_or(|sub| {
                sub.loops.iter().any(|&lp| {
                    let scope_loop = &scope.loops[lp];
                    self.counted_iterator(scope_loop.iterator) != Some(scope_loop.values)
                })
            })
    }

    /// Whether the active scope keeps tables of the writes to `id`.
    pub(super) fn covers_writes_of(&self, id: VarId) -> bool {
        self.active_writer_scope()
            .is_some_and(|scope| scope.writers.contains_key(&id))
    }

    /// The current write's value at element positions, which the table
    /// holds, is the same at each instance, so the table holds it exactly.
    /// `false` when the write is on only some of its iterations, whose
    /// positions the value at element positions does not tell.
    pub(super) fn record_uniform_write(&mut self, key: NodeKey) -> bool {
        let (Some(writer), Some(scope)) = (self.current_writer, self.active_writer_scope()) else {
            return true;
        };
        if self.writes_partially(&scope, key.0, writer) {
            return false;
        }
        self.held_tables.remove(&(key, writer));
        if let Some(count) = self.imprecise_tables.get_mut(&(key, writer)) {
            *count = count.saturating_sub(1);
        }
        true
    }

    /// The writer being recorded for `key` and the index of its instance in
    /// its table, sampled at the iterators being evaluated, with the
    /// positions those take. `None` outside a scope that covers the key.
    pub(super) fn instance_anchor(
        &mut self,
        key: NodeKey,
    ) -> Option<(WriterId, super::SampledAffineIndex, ArraySpan, usize)> {
        let scope = self.active_writer_scope()?;
        let writer = self.current_writer?;
        if !scope.has_writer(key.0, writer) {
            return None;
        }
        let (index, positions) = scope.table_index(key.0, writer)?;
        let sampled = self.sample_iterator_index(index);
        let (first, last) = self.affine_hull(&sampled.index)?;
        let start = usize::try_from(first).ok()?;
        let length = usize::try_from(last)
            .ok()?
            .checked_sub(start)?
            .checked_add(1)?;
        Some((
            writer,
            sampled,
            ArraySpan { start, length },
            positions.length,
        ))
    }

    /// Bind the table of the current writer for `key` to `version`, a value
    /// taken again at its instance index.
    /// The write reaches only the table positions `reached` that the
    /// iterations being evaluated take; on only some of the write's
    /// iterations, the others keep what the table held.
    pub(super) fn record_instance_write(
        &mut self,
        key: NodeKey,
        writer: WriterId,
        version: VersionId,
        anchor: &super::AffineIndex,
    ) {
        let Some(scope) = self.active_writer_scope() else {
            return;
        };
        let table = self.writer_key(key, writer);
        let partial = self.writes_partially(&scope, key.0, writer);
        let version = match self.key_span(key) {
            Some(packed) => {
                let domain = scope.table_domain(key.0, writer, position_domain(key.1, packed));
                if partial {
                    // The positions those iterations take, each progression
                    // of them exactly.
                    let progressions = self.affine_progressions(anchor).unwrap_or_default();
                    let pieces = progressions
                        .into_iter()
                        .filter_map(|(span, step)| {
                            self.progression_value(
                                version,
                                position_domain(span, packed),
                                super::Axis::Array,
                                step,
                            )
                        })
                        .collect();
                    let written = self.ssa.phi(pieces);
                    // What the table held before this write's value at
                    // element positions replaced it.
                    let held = match self.held_tables.remove(&(key, writer)) {
                        Some(held) => held,
                        None => self.ssa.read(table),
                    };
                    let merged = self.ssa.phi(vec![held, written]);
                    self.ssa.retained(merged, domain)
                } else {
                    self.ssa.retained(version, domain)
                }
            }
            None => version,
        };
        self.ssa.bind(table, version);
        // The same write's value at element positions is replaced.
        if let Some(count) = self.imprecise_tables.get_mut(&(key, writer)) {
            *count = count.saturating_sub(1);
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
        let cells = self.solve_last_writes(scope, writers, place, access, &[])?;
        Some(
            cells
                .into_iter()
                .map(|(cell, writes)| {
                    let mut sources = Vec::new();
                    for write in writes {
                        if !sources.contains(&write.source) {
                            sources.push(write.source);
                        }
                    }
                    (cell, sources)
                })
                .collect(),
        )
    }

    /// The last write instances of a read on each set of its iterations,
    /// latest first.
    /// `extra` are iterators of the read beyond those of its loops, such as
    /// the positions of a read after the loops.
    fn solve_last_writes(
        &mut self,
        scope: &WriterScope,
        writers: &[SubWriter],
        place: &[Step],
        access: &Access,
        extra: &[CountedIterator],
    ) -> Option<Vec<(Cell, Vec<LastWrite>)>> {
        let read_loops = place_loops(place);
        if self.counted_iterators.len() != read_loops.len()
            || read_loops
                .iter()
                .zip(&self.counted_iterators)
                .any(|(&id, iterator)| scope.loops[id].iterator != iterator.id)
        {
            return None;
        }
        let mut domain = self.counted_iterators.clone();
        domain.extend_from_slice(extra);
        let levels = domain.len();
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
                candidates.extend(self.candidate(
                    scope,
                    writer,
                    access,
                    &domain,
                    &read_lin,
                    rank,
                    first_unknown,
                    carried,
                )?);
            }
        }
        self.charge_last_writer_work(candidates.len().saturating_add(1))?;
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
        let mut pending = cells;
        while let Some(cell) = pending.pop() {
            // The candidates on the cell, latest first, with ties in one
            // group. Where the order of two depends on the iteration, the
            // cell is split where it changes.
            let mut order: Vec<Vec<usize>> = Vec::new();
            let mut split = None;
            'ordering: for index in 0..candidates.len() {
                if intersect_cells(&cell, &candidates[index].cell)
                    .ok()?
                    .is_none()
                {
                    continue;
                }
                let mut position = order.len();
                let mut tie = None;
                for (group, members) in order.iter().enumerate() {
                    let ordering = match self.compare(
                        scope,
                        &candidates[index],
                        &candidates[members[0]],
                        &cell,
                    )? {
                        Ok(ordering) => ordering,
                        Err(difference) => {
                            split = Some(difference);
                            break 'ordering;
                        }
                    };
                    match ordering {
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
            if let Some(difference) = split {
                for constraint in [
                    Constraint::Range(difference.clone(), None, Some(-1)),
                    Constraint::Range(difference.clone(), Some(0), Some(0)),
                    Constraint::Range(difference, Some(1), None),
                ] {
                    pending.extend(self.restrict_split(cell.clone(), &constraint)?);
                }
                self.charge_last_writer_work(pending.len())?;
                continue;
            }
            let mut writes = Vec::new();
            let mut covered = false;
            let mut later: Vec<&Candidate<'_>> = Vec::new();
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
                // A write that a later present one always follows in the
                // same iterations is never the last.
                // Ties are writes no order separates from the group's first,
                // such as on other arms, which may still follow each other.
                let group = present.clone();
                present.retain(|candidate| {
                    !later.iter().chain(&group).any(|after| {
                        !std::ptr::eq(*after, *candidate)
                            && follows(scope, after, candidate, levels, &cell)
                    })
                });
                later.extend(present.iter().copied());
                if present.is_empty() {
                    continue;
                }
                for candidate in &present {
                    writes.push(LastWrite {
                        source: Some(candidate.writer.id),
                        instance: candidate.instance_map(levels),
                        earlier: candidate.earlier,
                        fixed: candidate.fixed.clone(),
                    });
                }
                if overwrites(&present, place) {
                    covered = true;
                    break;
                }
            }
            if !covered {
                writes.push(LastWrite {
                    source: None,
                    instance: None,
                    earlier: None,
                    fixed: Vec::new(),
                });
            }
            result.push((cell, writes));
        }
        Some(result)
    }

    /// The instances of `writer` before the read in one group: at the read's
    /// iteration of the loops before `first_unknown`, and, for a carried
    /// group, at an earlier iteration of loop `carried`. One for each set of
    /// the read's iterations the constraints leave, none when no instance
    /// precedes the read; `None` when they are not solved exactly.
    #[allow(clippy::too_many_arguments)]
    fn candidate<'w>(
        &mut self,
        scope: &WriterScope,
        writer: &'w SubWriter,
        access: &Access,
        domain: &Cell,
        read_lin: &dyn Fn(&AffineIndex) -> Option<Lin>,
        rank: usize,
        first_unknown: usize,
        carried: Option<usize>,
    ) -> Option<Vec<Candidate<'w>>> {
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
        // A read without element coordinates takes positions of the
        // flattened array.
        let mut equations = if access.elements.is_empty() {
            vec![equation(&writer.access.flat, &access.flat)?]
        } else {
            if writer.access.elements.len() != access.elements.len() {
                return None;
            }
            writer
                .access
                .elements
                .iter()
                .zip(&access.elements)
                .map(|(written, read)| equation(written, read))
                .collect::<Option<Vec<_>>>()?
        };
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
                    return Some(Vec::new());
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
                None => return Some(Vec::new()),
            }
        }
        let systems = self.solved_systems(equations, writer, first_unknown, levels)?;
        let mut found = Vec::new();
        for (determined, mut constraints) in systems {
            let fixed = (0..unknowns)
                .map(|level| level < first_unknown || determined[level].is_some())
                .collect::<Vec<_>>();
            let mut instance = vec![None; unknowns];
            let mut alternatives = Vec::new();
            let mut carried_level = None;
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
                        // read takes the same values and that iteration is
                        // one the write takes, else the write's last.
                        // Every value the read takes is one the write's
                        // iterations take as well.
                        let read = &domain[level];
                        let modulus = iterator.modulus.max(1);
                        if (read.min == read.max || read.modulus % modulus == 0)
                            && read.min.rem_euclid(modulus) == iterator.residue.rem_euclid(modulus)
                        {
                            let modulus = iterator.modulus as i128;
                            let lin = Lin::iterator(levels, level);
                            let mut previous = lin.clone();
                            previous.c = if ascending { -modulus } else { modulus };
                            let (near, beyond, last) = if ascending {
                                let bound = iterator.max as i128 + modulus;
                                (
                                    Constraint::Range(lin.clone(), None, Some(bound)),
                                    Constraint::Range(lin, Some(bound + 1), None),
                                    iterator.max,
                                )
                            } else {
                                let bound = iterator.min as i128 - modulus;
                                (
                                    Constraint::Range(lin.clone(), Some(bound), None),
                                    Constraint::Range(lin, None, Some(bound - 1)),
                                    iterator.min,
                                )
                            };
                            alternatives = vec![
                                (near, (previous, 1)),
                                (beyond, (Lin::constant(levels, last as i128), 1)),
                            ];
                            carried_level = Some(level);
                        }
                    }
                    None => {
                        let last = scope.loops[writer.loops[level]].last(iterator);
                        instance[level] = Some((Lin::constant(levels, last as i128), 1));
                    }
                }
            }
            // Each way the latest instance of the carried loop is taken.
            let ways = match carried_level {
                Some(level) => alternatives
                    .into_iter()
                    .map(|(constraint, value)| {
                        let mut instance = instance.clone();
                        instance[level] = Some(value);
                        let mut constraints = constraints.clone();
                        constraints.push(constraint);
                        (constraints, instance)
                    })
                    .collect(),
                None => vec![(constraints, instance)],
            };
            // A write that may not take place leaves the instances before
            // its latest to be the last: at each level its instance is not
            // given by the read, every earlier value of that level.
            // An iteration of a loop skips the write only on a branch
            // inside it; every other iteration of that loop writes once its
            // enclosing iteration does.
            let skips = |level: usize| {
                !covers
                    || writer
                        .branches
                        .iter()
                        .any(|branch| place_loops(&branch.place).len() > level)
            };
            let mut ranges = Vec::new();
            {
                for (constraints, instance) in &ways {
                    for level in first_unknown..unknowns {
                        if determined[level].is_some() || !skips(level) {
                            continue;
                        }
                        let Some((value, 1)) = &instance[level] else {
                            continue;
                        };
                        let iterator = &writer.domains[level];
                        let modulus = iterator.modulus as i128;
                        let mut constraints = constraints.clone();
                        let mut instance = instance.clone();
                        let mut latest = value.clone();
                        if scope.loops[writer.loops[level]].ascending {
                            latest.c -= modulus;
                            constraints.push(Constraint::Range(
                                latest.clone(),
                                Some(iterator.min as i128),
                                None,
                            ));
                        } else {
                            latest.c += modulus;
                            constraints.push(Constraint::Range(
                                latest.clone(),
                                None,
                                Some(iterator.max as i128),
                            ));
                        }
                        instance[level] = Some((latest, 1));
                        // An inner loop no branch skips writes on each of its
                        // iterations, so its last one is the latest there.
                        let range_fixed = (0..unknowns)
                            .map(|inner| {
                                inner < level || fixed[inner] || (inner > level && !skips(inner))
                            })
                            .collect::<Vec<_>>();
                        ranges.push((constraints, instance, Some(level), range_fixed));
                    }
                }
            }
            let ways = ways
                .into_iter()
                .map(|(constraints, instance)| (constraints, instance, None, fixed.clone()))
                .chain(ranges);
            for (constraints, instance, earlier, fixed) in ways {
                let mut cells = vec![cell.clone()];
                for constraint in constraints {
                    let mut restricted = Vec::new();
                    for cell in cells {
                        restricted.extend(self.restrict_split(cell, &constraint)?);
                    }
                    cells = restricted;
                }
                found.extend(cells.into_iter().map(|cell| {
                    // Earlier instances that are only the first value on the
                    // cell, at loops inside given by the read, are that one.
                    let earlier = earlier.filter(|&level| {
                        let domain = &writer.domains[level];
                        let first = if scope.loops[writer.loops[level]].ascending {
                            domain.min
                        } else {
                            domain.max
                        } as i128;
                        let single = (level + 1..unknowns).all(|inner| fixed[inner])
                            && matches!(&instance[level], Some((latest, 1))
                                if lin_extent(latest, &cell) == Some((first, first)));
                        !single
                    });
                    Candidate {
                        writer,
                        rank,
                        first_unknown,
                        cell,
                        instance: instance.clone(),
                        covers,
                        earlier,
                        fixed: fixed.clone(),
                    }
                }));
            }
        }
        Some(found)
    }

    /// The unknowns of `equations` each solved as an affine value of the
    /// read's iterators, with the constraints they leave. Unknowns that the
    /// equations leave related to one another are fixed to each value the
    /// writer takes, outermost first, charged to the procedure's work; each
    /// assignment is its own solution. `None` when they cannot be solved.
    fn solved_systems(
        &mut self,
        equations: Vec<Equation>,
        writer: &SubWriter,
        first_unknown: usize,
        levels: usize,
    ) -> Option<Vec<Solution>> {
        let unknowns = writer.loops.len();
        let mut pending = vec![(equations, Vec::<usize>::new())];
        let mut solved = Vec::new();
        while let Some((system, fixed)) = pending.pop() {
            let mut equations = system.clone();
            let mut constraints = Vec::new();
            if let Some(determined) = eliminate(&mut equations, unknowns, levels, &mut constraints)
            {
                solved.push((determined, constraints));
                continue;
            }
            let level = (first_unknown..unknowns).find(|level| {
                !fixed.contains(level) && system.iter().any(|equation| equation.u[*level] != 0)
            })?;
            let domain = writer.domains[level];
            self.charge_last_writer_work(usize::try_from(domain.count()?).ok()?)?;
            let step = domain.modulus.unsigned_abs();
            for value in (domain.min..=domain.max).step_by(step) {
                let mut u = vec![0i128; unknowns];
                u[level] = 1;
                let mut system = system.clone();
                system.push(Equation {
                    u,
                    rhs: Lin::constant(levels, value as i128),
                });
                let mut fixed = fixed.clone();
                fixed.push(level);
                pending.push((system, fixed));
            }
        }
        Some(solved)
    }

    /// The parts of `cell` that satisfy `constraint`. A constraint over
    /// several of its iterators keeps the one with the most values and
    /// takes each value of the others in turn, charged to the procedure's
    /// work; `None` when it is exhausted or the arithmetic overflows.
    fn restrict_split(&mut self, mut cell: Cell, constraint: &Constraint) -> Option<Vec<Cell>> {
        // An iterator confined to one value is that value.
        let mut constraint = constraint.clone();
        let lin = match &mut constraint {
            Constraint::Range(lin, ..) | Constraint::Mod(lin, ..) => lin,
        };
        for (level, iterator) in cell.iter().enumerate() {
            if iterator.min == iterator.max && lin.k[level] != 0 {
                lin.c = lin
                    .c
                    .checked_add(lin.k[level].checked_mul(iterator.min as i128)?)?;
                lin.k[level] = 0;
            }
        }
        let levels = lin
            .k
            .iter()
            .enumerate()
            .filter(|(_, coefficient)| **coefficient != 0)
            .map(|(level, _)| level)
            .collect::<Vec<_>>();
        if levels.len() <= 1 {
            return Some(match restrict(&mut cell, constraint)? {
                true => vec![cell],
                false => Vec::new(),
            });
        }
        let kept = *levels.iter().max_by_key(|&&level| cell[level].count())?;
        let taken = levels
            .into_iter()
            .filter(|&level| level != kept)
            .collect::<Vec<_>>();
        let count = taken.iter().try_fold(1usize, |count, &level| {
            count.checked_mul(usize::try_from(cell[level].count()?).ok()?)
        })?;
        self.charge_last_writer_work(count)?;
        let mut pieces = vec![cell];
        for level in taken {
            let mut next = Vec::new();
            for piece in pieces {
                let iterator = piece[level];
                let step = iterator.modulus.unsigned_abs();
                for value in (iterator.min..=iterator.max).step_by(step) {
                    let mut piece = piece.clone();
                    piece[level] = iterator.within(value, value).ok()??;
                    next.push(piece);
                }
            }
            pieces = next;
        }
        let mut restricted = Vec::new();
        for piece in pieces {
            restricted.extend(self.restrict_split(piece, &constraint)?);
        }
        Some(restricted)
    }

    /// Whether `left` is a later write instance than `right`.
    /// The order of two candidates on the iterations of `cell`, or the
    /// difference of their instances whose sign the cell does not fix.
    /// `None` when they cannot be ordered.
    fn compare(
        &self,
        scope: &WriterScope,
        left: &Candidate<'_>,
        right: &Candidate<'_>,
        cell: &Cell,
    ) -> Option<Result<Ordering, Lin>> {
        if left.rank != right.rank {
            return Some(Ok(left.rank.cmp(&right.rank)));
        }
        // Instances of one write are ordered where their difference is
        // known, and are otherwise taken together.
        let same = left.writer.path == right.writer.path;
        let shared = common_loops(&left.writer.path, &right.writer.path);
        for level in left.first_unknown.max(right.first_unknown)..shared {
            let difference = match (&left.instance[level], &right.instance[level]) {
                (Some((left_value, left_denominator)), Some((right_value, right_denominator))) => {
                    left_value
                        .scaled(*right_denominator)
                        .and_then(|left| left.plus(right_value, -*left_denominator))
                }
                _ => None,
            };
            let Some(difference) = difference else {
                return same.then_some(Ok(Ordering::Equal));
            };
            // Denominators are positive, so the sign is that of the
            // difference of the values.
            let sign = match lin_extent(&difference, cell) {
                Some((low, _)) if low > 0 => Ordering::Greater,
                Some((_, high)) if high < 0 => Ordering::Less,
                Some((0, 0)) => Ordering::Equal,
                _ if same => return Some(Ok(Ordering::Equal)),
                Some(_) => return Some(Err(difference)),
                None => return None,
            };
            let ascending = scope.loops[left.writer.loops[level]].ascending;
            match (sign, ascending) {
                (Ordering::Equal, _) => {}
                (ordering, true) => return Some(Ok(ordering)),
                (ordering, false) => return Some(Ok(ordering.reverse())),
            }
        }
        if same {
            return Some(Ok(Ordering::Equal));
        }
        Some(Ok(match relation(&left.writer.path, &right.writer.path) {
            Relation::Before => Ordering::Greater,
            Relation::After => Ordering::Less,
            Relation::Exclusive => Ordering::Equal,
        }))
    }
}

/// The least and greatest values of `lin` over the iterations of `cell`.
fn lin_extent(lin: &Lin, cell: &Cell) -> Option<(i128, i128)> {
    let (mut low, mut high) = (lin.c, lin.c);
    for (&coefficient, iterator) in lin.k.iter().zip(cell) {
        let (first, last) = (
            coefficient.checked_mul(iterator.min as i128)?,
            coefficient.checked_mul(iterator.max as i128)?,
        );
        low = low.checked_add(first.min(last))?;
        high = high.checked_add(first.max(last))?;
    }
    Some((low, high))
}

/// Whether `after` overwrites every bit the read takes whenever `before`
/// writes it: on the same iterations of the loops around both, after it,
/// and on every path that reaches it.
fn follows(
    scope: &WriterScope,
    after: &Candidate<'_>,
    before: &Candidate<'_>,
    levels: usize,
    cell: &Cell,
) -> bool {
    if !after.covers {
        return false;
    }
    if after.writer.path == before.writer.path {
        return follows_itself(scope, after, before, levels, cell);
    }
    if relation(&after.writer.path, &before.writer.path) != Relation::Before {
        return false;
    }
    let common = common_loops(&after.writer.path, &before.writer.path);
    // Earlier instances of `before` at loops `after` is not in all precede
    // the one of `after`; at a loop around both, only earlier instances of
    // `after` from the same level follow each.
    if let Some(level) = before.earlier.filter(|&level| level < common)
        && after.earlier != Some(level)
    {
        return false;
    }
    let (Some(after_instance), Some(before_instance)) =
        (after.instance_map(levels), before.instance_map(levels))
    else {
        return false;
    };
    // The extent over the cell of the instance of `after` less that of
    // `before` at a level, scaled by their positive denominators.
    let difference = |level: usize| {
        let ((left, left_denominator), (right, right_denominator)) =
            (&after_instance[level], &before_instance[level]);
        let difference = left
            .scaled(*right_denominator)?
            .plus(right, -*left_denominator)?;
        lin_extent(&difference, cell)
    };
    // The same instance of the loops around both, or for every earlier
    // instance from a level, one up to the latest there.
    let same = (0..common.min(after.earlier.unwrap_or(common)))
        .all(|level| difference(level) == Some((0, 0)));
    if !same {
        return false;
    }
    if let Some(level) = after.earlier.filter(|&level| level < common) {
        let Some((low, high)) = difference(level) else {
            return false;
        };
        let ascending = scope.loops[after.writer.loops[level]].ascending;
        if (ascending && low < 0) || (!ascending && high > 0) {
            return false;
        }
        // Inside it, `after` takes every value only at loops the read does
        // not give; at the others, its instance must be that of `before`.
        let fixed = |candidate: &Candidate<'_>, inner: usize| {
            candidate.fixed.get(inner).copied().unwrap_or(false)
        };
        if !(level + 1..common).all(|inner| {
            !fixed(after, inner)
                || ((before.earlier.is_none_or(|from| inner < from) || fixed(before, inner))
                    && difference(inner) == Some((0, 0)))
        }) {
            return false;
        }
    }
    after
        .writer
        .branches
        .iter()
        .all(|branch| before.writer.branches.contains(branch))
}

/// Whether a later instance of the same write always follows `before`:
/// they are the same instance outside the first loop at which `after` is
/// later, and no branch inside that loop can skip `after`'s iteration.
fn follows_itself(
    scope: &WriterScope,
    after: &Candidate<'_>,
    before: &Candidate<'_>,
    levels: usize,
    cell: &Cell,
) -> bool {
    if after.earlier.is_some() || before.earlier.is_some() {
        return false;
    }
    let (Some(after_instance), Some(before_instance)) =
        (after.instance_map(levels), before.instance_map(levels))
    else {
        return false;
    };
    for (level, ((left, left_denominator), (right, right_denominator))) in
        after_instance.iter().zip(&before_instance).enumerate()
    {
        let Some(extent) = left
            .scaled(*right_denominator)
            .and_then(|left| left.plus(right, -*left_denominator))
            .and_then(|difference| lin_extent(&difference, cell))
        else {
            return false;
        };
        if extent == (0, 0) {
            continue;
        }
        let ascending = scope.loops[after.writer.loops[level]].ascending;
        let later = if ascending {
            extent.0 > 0
        } else {
            extent.1 < 0
        };
        return later
            && after
                .writer
                .branches
                .iter()
                .all(|branch| place_loops(&branch.place).len() <= level);
    }
    false
}

/// Whether the latest present writers overwrite every bit the read takes on
/// each of its iterations: one of them always writes, or they are the arms of
/// one branch that write on every path through it.
fn overwrites(present: &[&Candidate<'_>], place: &[Step]) -> bool {
    if !present.iter().all(|candidate| candidate.covers) {
        return false;
    }
    // A write runs whenever the read does when each branch around it is
    // one the read is on, around only loops at which the write is the
    // read's instance: those before `first_unknown`.
    let runs_with_read = |candidate: &Candidate<'_>| {
        candidate.earlier.is_none()
            && candidate.writer.branches.iter().all(|branch| {
                place_loops(&branch.place).len() <= candidate.first_unknown
                    && place.starts_with(&branch.place)
                    && place.get(branch.place.len()) == Some(&Step::Arm(branch.arm))
            })
    };
    if present
        .iter()
        .any(|candidate| candidate.writer.branches.is_empty() || runs_with_read(candidate))
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

/// Whether a write at `written`, moving with one inner loop of `inner` and
/// with the read's loops, takes the element `read` takes at one value of
/// the inner iterator on each iteration of `cell`: `written` is
/// `coefficient * inner + outer`, so the inner value is
/// `(read - outer) / coefficient`, an affine value over the cell that must
/// be whole and within the inner loop's values everywhere.
fn reaches_each_iteration(
    written: &AffineIndex,
    read: &AffineIndex,
    cell: &Cell,
    loops: &[usize],
    scope: &WriterScope,
    inner: &[usize],
) -> bool {
    let [inner] = inner else {
        return false;
    };
    let inner = &scope.loops[*inner];
    if inner.values.modulus != 1 {
        return false;
    }
    let level_of = |id: VarId| loops.iter().position(|&lp| scope.loops[lp].iterator == id);
    let mut coefficient = 0i128;
    let mut numerator = vec![0i128; cell.len()];
    let mut constant = read.constant as i128 - written.constant as i128;
    for &(id, factor) in &read.terms {
        let Some(level) = level_of(id) else {
            return false;
        };
        numerator[level] += factor as i128;
    }
    for &(id, factor) in &written.terms {
        if id == inner.iterator {
            coefficient += factor as i128;
            continue;
        }
        let Some(level) = level_of(id) else {
            return false;
        };
        numerator[level] -= factor as i128;
    }
    if coefficient == 0 {
        return false;
    }
    // Whole at the first iteration and at each step of every level.
    for (level, iterator) in cell.iter().enumerate() {
        constant += numerator[level] * iterator.min as i128;
        if iterator.min != iterator.max
            && (numerator[level] * iterator.modulus as i128) % coefficient != 0
        {
            return false;
        }
    }
    if constant % coefficient != 0 {
        return false;
    }
    // Within the inner values at the extremes of the cell.
    let (mut low, mut high) = (constant, constant);
    for (level, iterator) in cell.iter().enumerate() {
        let span = numerator[level] * (iterator.max as i128 - iterator.min as i128);
        if span < 0 {
            low += span;
        } else {
            high += span;
        }
    }
    let (low, high) = if coefficient > 0 {
        (low / coefficient, high / coefficient)
    } else {
        (high / coefficient, low / coefficient)
    };
    inner.values.min as i128 <= low && high <= inner.values.max as i128
}

/// For a loop whose values do not step additively or that breaks, the
/// iterator over the order of its iterations, at the one numbered `ordinal`.
/// No expression reads it, so a position that moves with the loop's own
/// values is not affine in it.
pub(super) fn ordinal_iterator(
    statement: &ForStatement,
    level: usize,
    ordinal: isize,
) -> Option<CountedIterator> {
    (matches!(statement.range, ForRange::Stepped { .. })
        || crate::ir::peel::has_own_break(&statement.body))
    .then(|| Some(CountedIterator::new(ordinal_id(level)?, ordinal, ordinal)))
    .flatten()
}

/// The iterator over the order of the iterations of the loop at `level` of
/// a nest, which no variable is.
fn ordinal_id(level: usize) -> Option<VarId> {
    Some(VarId::from_raw(
        u32::MAX
            .checked_sub(1)?
            .checked_sub(u32::try_from(level).ok()?)?,
    ))
}

/// Whether `id` is the iterator over the order of the iterations of a loop.
fn is_ordinal(id: VarId) -> bool {
    (0..64).any(|level| ordinal_id(level) == Some(id))
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
