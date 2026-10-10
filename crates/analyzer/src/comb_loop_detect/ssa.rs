//! IR-independent statement-ordered SSA state.

mod dag;
mod log;
mod repeated;

use super::region::{ArraySpan, PackedSpan};
use crate::{HashMap, HashSet};
use std::collections::VecDeque;
use std::hash::Hash;
use std::rc::Rc;
use veryl_parser::token_range::TokenRange;

#[cfg(test)]
thread_local! {
    static SOURCE_WALK_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ITERATION_IMPORT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static IMPORT_BINDING_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_import_binding_visits() {
    IMPORT_BINDING_VISITS.set(0);
}

#[cfg(test)]
pub(crate) fn import_binding_visits() -> usize {
    IMPORT_BINDING_VISITS.get()
}

#[cfg(test)]
pub(crate) fn reset_source_walk_visits() {
    SOURCE_WALK_VISITS.set(0);
}
#[cfg(test)]
pub(crate) fn source_walk_visits() -> usize {
    SOURCE_WALK_VISITS.get()
}

pub(super) type VersionId = usize;

type SourceMap<K> = HashMap<(K, PositionRelation), PathCondition>;

pub(super) struct SourceCache<K> {
    summaries: HashMap<(VersionId, bool), Rc<SourceMap<K>>>,
    ignore_position: bool,
}

impl<K> Default for SourceCache<K> {
    fn default() -> Self {
        Self {
            summaries: HashMap::default(),
            ignore_position: false,
        }
    }
}

#[derive(Clone)]
enum Version<K> {
    Entry(K),
    Definition {
        sources: Vec<(VersionId, PositionRelation)>,
        condition: PathCondition,
    },
    Phi(Vec<VersionId>),
    Guarded {
        source: VersionId,
        condition: PathCondition,
    },
    Imported {
        graph: Rc<DependencyDag<K>>,
        root: Option<usize>,
        bindings: Rc<HashMap<K, Vec<(VersionId, PositionRelation)>>>,
        branches: Rc<HashMap<BranchId, BranchId>>,
    },
    Projected {
        source: VersionId,
        domain: PositionDomain,
    },
    Replicated {
        source: VersionId,
        domain: PositionDomain,
        replication: Replication,
    },
    /// A write of `region`: positions there come from `above`, and
    /// `below` remains at the `retained` positions (every position for a weak
    /// write). Both inputs keep their retention semantics, like a `Phi`.
    Overlay {
        below: VersionId,
        retained: Option<Rc<[PositionDomain]>>,
        above: VersionId,
    },
    /// `source` at `domain` only. Unlike `Projected`, this is not a read: the
    /// source keeps its retention semantics, like the retained part of an
    /// `Overlay`.
    Restricted {
        source: VersionId,
        domain: PositionDomain,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct BranchId {
    procedure: usize,
    local: usize,
    arms: usize,
}

impl BranchId {
    pub(super) const fn new(procedure: usize, local: usize, arms: usize) -> Self {
        Self {
            procedure,
            local,
            arms,
        }
    }

    pub(super) const fn arms(self) -> usize {
        self.arms
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct BranchConstraint {
    branch: BranchId,
    allowed: ArmSet,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ArmSet {
    ranges: Vec<(usize, usize)>,
}

impl ArmSet {
    fn range(start: usize, end: usize) -> Self {
        Self {
            ranges: (start < end).then_some((start, end)).into_iter().collect(),
        }
    }

    fn intersection(&self, other: &Self) -> Self {
        let mut ranges = Vec::new();
        let mut left = 0;
        let mut right = 0;
        while left < self.ranges.len() && right < other.ranges.len() {
            let a = self.ranges[left];
            let b = other.ranges[right];
            let start = a.0.max(b.0);
            let end = a.1.min(b.1);
            if start < end {
                ranges.push((start, end));
            }
            if a.1 < b.1 {
                left += 1;
            } else {
                right += 1;
            }
        }
        Self { ranges }
    }

    fn union(&self, other: &Self) -> Self {
        let mut ranges = self
            .ranges
            .iter()
            .chain(&other.ranges)
            .copied()
            .collect::<Vec<_>>();
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for range in ranges {
            if let Some(previous) = merged.last_mut()
                && range.0 <= previous.1
            {
                previous.1 = previous.1.max(range.1);
            } else {
                merged.push(range);
            }
        }
        Self { ranges: merged }
    }

    fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    fn is_all(&self, arms: usize) -> bool {
        self.ranges.as_slice() == [(0, arms)]
    }

    fn is_subset_of(&self, other: &Self) -> bool {
        self.intersection(other) == *self
    }
}

/// A compact Cartesian over-approximation of feasible branch choices.
///
/// Correlations between distinct syntactic branches are intentionally not
/// retained. Choices of the same branch remain exact, which is sufficient to
/// reject cycles assembled from mutually exclusive arms without enumerating
/// every combination of independent conditions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct PathCondition {
    constraints: Rc<Vec<BranchConstraint>>,
}

impl PathCondition {
    pub(super) fn branch_count(&self) -> usize {
        self.constraints.len()
    }

    /// Count a constraint and its first arm range as one work unit, preserving
    /// the cost of compact guards. Every additional disjoint range costs
    /// another unit, so a single branch cannot hide an unbounded payload.
    pub(super) fn work_size(&self) -> usize {
        self.constraints.iter().fold(0usize, |cost, constraint| {
            cost.saturating_add(constraint.allowed.ranges.len().max(1))
        })
    }

    pub(super) fn is_unconditional(&self) -> bool {
        self.constraints.is_empty()
    }

    pub(super) fn with_choice(&self, branch: BranchId, arm: usize) -> Self {
        self.with_choice_range(branch, arm, arm.saturating_add(1))
    }

    pub(super) fn with_choice_range(&self, branch: BranchId, start: usize, end: usize) -> Self {
        debug_assert!(start < end && end <= branch.arms);
        let mut constraints = self.constraints.as_ref().clone();
        let constraint = BranchConstraint {
            branch,
            allowed: ArmSet::range(start, end),
        };
        match constraints.binary_search_by_key(&branch, |constraint| constraint.branch) {
            Ok(index) => constraints[index] = constraint,
            Err(index) => constraints.insert(index, constraint),
        }
        Self {
            constraints: Rc::new(constraints),
        }
    }

    /// Joins alternative paths into the least Cartesian condition that covers
    /// every input condition.
    pub(super) fn try_disjoin_all<'a>(
        conditions: impl IntoIterator<Item = &'a Self>,
        work: &mut usize,
    ) -> Option<Self> {
        let mut conditions = conditions.into_iter();
        let Some(first) = conditions.next() else {
            return Some(Self::default());
        };
        let mut combined = first.clone();
        for condition in conditions {
            // The accumulated union can grow at each step even when every
            // input has just one range. Charge before allocating its copy.
            reserve_guard_work(work, [&combined, condition])?;
            combined = combined.disjoin(condition);
        }
        Some(combined)
    }

    pub(super) fn conjoin_if_compatible(&self, other: &Self) -> Option<Self> {
        let mut constraints = Vec::with_capacity(self.constraints.len() + other.constraints.len());
        let mut left = self.constraints.iter().peekable();
        let mut right = other.constraints.iter().peekable();
        loop {
            match (left.peek(), right.peek()) {
                (Some(a), Some(b)) if a.branch == b.branch => {
                    let allowed = a.allowed.intersection(&b.allowed);
                    if allowed.is_empty() {
                        return None;
                    }
                    constraints.push(BranchConstraint {
                        branch: a.branch,
                        allowed,
                    });
                    left.next();
                    right.next();
                }
                (Some(a), Some(b)) if a.branch < b.branch => {
                    constraints.push((*a).clone());
                    left.next();
                }
                (Some(_), Some(b)) => {
                    constraints.push((*b).clone());
                    right.next();
                }
                (Some(a), None) => {
                    constraints.push((*a).clone());
                    left.next();
                }
                (None, Some(b)) => {
                    constraints.push((*b).clone());
                    right.next();
                }
                (None, None) => break,
            }
        }
        Some(Self {
            constraints: Rc::new(constraints),
        })
    }

    /// Returns true when every branch valuation admitted by `other` is also
    /// admitted by `self`.
    pub(super) fn covers(&self, other: &Self) -> bool {
        self.constraints.iter().all(|constraint| {
            other
                .constraints
                .binary_search_by_key(&constraint.branch, |other| other.branch)
                .ok()
                .is_some_and(|index| {
                    other.constraints[index]
                        .allowed
                        .is_subset_of(&constraint.allowed)
                })
        })
    }

    pub(super) fn branches(&self) -> impl Iterator<Item = BranchId> {
        self.constraints
            .iter()
            .map(|constraint| constraint.branch)
            .collect::<Vec<_>>()
            .into_iter()
    }

    pub(super) fn remapped(&self, branches: &HashMap<BranchId, BranchId>) -> Self {
        let mut constraints = self
            .constraints
            .iter()
            .map(|constraint| BranchConstraint {
                branch: branches
                    .get(&constraint.branch)
                    .copied()
                    .unwrap_or(constraint.branch),
                allowed: constraint.allowed.clone(),
            })
            .collect::<Vec<_>>();
        constraints.sort_unstable_by_key(|constraint| constraint.branch);
        Self {
            constraints: Rc::new(constraints),
        }
    }

    /// Returns the least Cartesian condition covering either input.
    pub(super) fn disjoin(&self, other: &Self) -> Self {
        let mut constraints = Vec::new();
        for constraint in self.constraints.iter() {
            let Ok(index) = other
                .constraints
                .binary_search_by_key(&constraint.branch, |other| other.branch)
            else {
                continue;
            };
            let allowed = constraint.allowed.union(&other.constraints[index].allowed);
            if !allowed.is_all(constraint.branch.arms) {
                constraints.push(BranchConstraint {
                    branch: constraint.branch,
                    allowed,
                });
            }
        }
        Self {
            constraints: Rc::new(constraints),
        }
    }

    /// An exact union is Cartesian when the two cubes differ on at most one
    /// branch. Unlike `disjoin`, this never drops cross-branch correlations.
    pub(super) fn disjoin_exact(&self, other: &Self) -> Option<Self> {
        if self.covers(other) {
            return Some(self.clone());
        }
        if other.covers(self) {
            return Some(other.clone());
        }
        let mut differences = 0;
        for left in self.constraints.iter() {
            let different = match other
                .constraints
                .binary_search_by_key(&left.branch, |c| c.branch)
            {
                Ok(index) => left.allowed != other.constraints[index].allowed,
                Err(_) => !left.allowed.is_all(left.branch.arms),
            };
            differences += usize::from(different);
            if differences > 1 {
                return None;
            }
        }
        for right in other.constraints.iter() {
            if self
                .constraints
                .binary_search_by_key(&right.branch, |c| c.branch)
                .is_err()
                && !right.allowed.is_all(right.branch.arms)
            {
                differences += 1;
                if differences > 1 {
                    return None;
                }
            }
        }
        Some(self.disjoin(other))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct PositionRelation {
    pub(super) array: Option<isize>,
    pub(super) packed: Option<isize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Replication {
    Array(isize),
    Packed(isize),
}

impl Replication {
    fn stride(self) -> isize {
        match self {
            Self::Array(stride) | Self::Packed(stride) => stride,
        }
    }

    pub(super) fn relation(self) -> PositionRelation {
        match self {
            Self::Array(stride) => PositionRelation {
                array: Some(stride),
                packed: Some(0),
            },
            Self::Packed(stride) => PositionRelation {
                array: Some(0),
                packed: Some(stride),
            },
        }
    }

    fn forget_position(self, mut relation: PositionRelation) -> PositionRelation {
        match self {
            Self::Array(_) => relation.array = None,
            Self::Packed(_) => relation.packed = None,
        }
        relation
    }
}

#[derive(Clone)]
pub(super) enum DependencyDagNode<K> {
    External(K),
    Internal,
    /// Zero or more positive translations along one axis within this node's domain.
    /// Kept as an operation in the DAG; only the circuit graph adds a self edge.
    Replicated {
        replication: Replication,
    },
}

#[derive(Clone)]
pub(super) struct DependencyDagEdge {
    pub(super) source: usize,
    pub(super) destination: usize,
    pub(super) relation: PositionRelation,
    pub(super) condition: PathCondition,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DefinitionSite<N> {
    pub(super) token: TokenRange,
    pub(super) data_inputs: Vec<N>,
}

#[derive(Clone)]
pub(super) struct DependencyDag<K> {
    pub(super) nodes: Vec<DependencyDagNode<K>>,
    pub(super) edges: Vec<DependencyDagEdge>,
    pub(super) roots: Vec<Option<usize>>,
    pub(super) domains: Vec<Vec<PositionDomain>>,
    pub(super) sites: HashMap<usize, DefinitionSite<usize>>,
}

/// Work ran out before an answer was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RanOut;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct PositionDomain {
    pub(super) array_start: usize,
    pub(super) array_length: usize,
    pub(super) packed_start: usize,
    pub(super) packed_length: usize,
}

impl Default for PositionRelation {
    fn default() -> Self {
        Self {
            array: Some(0),
            packed: Some(0),
        }
    }
}

impl PositionRelation {
    pub(super) const fn whole() -> Self {
        Self {
            array: None,
            packed: None,
        }
    }

    pub(super) fn compose(self, other: Self) -> Self {
        Self {
            array: compose_axis(self.array, other.array),
            packed: compose_axis(self.packed, other.packed),
        }
    }

    #[cfg(test)]
    pub(super) fn union(self, other: Self) -> Self {
        Self {
            array: (self.array == other.array).then_some(self.array).flatten(),
            packed: (self.packed == other.packed)
                .then_some(self.packed)
                .flatten(),
        }
    }
}

fn compose_axis(left: Option<isize>, right: Option<isize>) -> Option<isize> {
    match (left, right) {
        (Some(left), Some(right)) => Some(
            left.checked_add(right)
                .expect("composed position offset must fit in isize"),
        ),
        _ => None,
    }
}

impl PositionDomain {
    pub(super) fn new(array: ArraySpan, packed: PackedSpan) -> Self {
        Self {
            array_start: array.start,
            array_length: array.length,
            packed_start: packed.start,
            packed_length: packed.length,
        }
    }

    /// The smallest box containing both, with ends saturated at `usize::MAX`.
    pub(super) fn hull(self, other: Self) -> Self {
        let array_start = self.array_start.min(other.array_start);
        let array_end = self
            .array_start
            .saturating_add(self.array_length)
            .max(other.array_start.saturating_add(other.array_length));
        let packed_start = self.packed_start.min(other.packed_start);
        let packed_end = self
            .packed_start
            .saturating_add(self.packed_length)
            .max(other.packed_start.saturating_add(other.packed_length));
        Self {
            array_start,
            array_length: array_end - array_start,
            packed_start,
            packed_length: packed_end - packed_start,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Checkpoint {
    undo_start: usize,
    depth: usize,
    version_start: usize,
}

pub(super) struct BranchState<K> {
    bindings: HashMap<K, VersionId>,
}

impl<K> BranchState<K> {
    pub(super) fn len(&self) -> usize {
        self.bindings.len()
    }

    pub(super) fn unchanged() -> Self {
        Self {
            bindings: HashMap::default(),
        }
    }
}

struct Undo<K> {
    key: K,
    previous: Option<VersionId>,
}

pub(super) struct SsaStore<K> {
    versions: Vec<Version<K>>,
    // Append-only ranges generated by runtime transfer closure. Their origin
    // survives conversion to ordinary SSA so enclosing loops can budget copies.
    repeated_versions: Vec<std::ops::Range<VersionId>>,
    entries: HashMap<K, VersionId>,
    current: HashMap<K, VersionId>,
    undo: Vec<Undo<K>>,
    checkpoints: Vec<usize>,
    sites: HashMap<VersionId, DefinitionSite<VersionId>>,
    // Indexed write histories of overlay versions for regional reads.
    logs: HashMap<VersionId, Rc<log::LogNode>>,
    // Versions below this are opaque to regional reads. A loop body reads
    // its iteration inputs, which must stay visible to transfer closure.
    log_floor: VersionId,
    // Regional reads already resolved. A log is fixed when its version is
    // created and versions are never discarded, so a result stays valid.
    logged_reads: HashMap<(VersionId, PositionDomain), VersionId>,
    // Per version, whether it may still hold an entry value. Inputs precede
    // the versions built from them and versions never change, so this is
    // extended in creation order.
    retains_entry: std::cell::RefCell<Vec<bool>>,
}

impl<K> Default for SsaStore<K> {
    fn default() -> Self {
        Self {
            versions: Vec::new(),
            repeated_versions: Vec::new(),
            entries: HashMap::default(),
            current: HashMap::default(),
            undo: Vec::new(),
            checkpoints: Vec::new(),
            sites: HashMap::default(),
            logs: HashMap::default(),
            log_floor: 0,
            logged_reads: HashMap::default(),
            retains_entry: Default::default(),
        }
    }
}

impl<K> SsaStore<K>
where
    K: Copy + Eq + Hash,
{
    pub(super) fn record_site(
        &mut self,
        version: VersionId,
        token: TokenRange,
        controls: &[VersionId],
    ) {
        let data_inputs = match &self.versions[version] {
            Version::Definition { sources, .. } => sources
                .iter()
                .map(|(source, _)| *source)
                .filter(|source| !controls.contains(source))
                .collect(),
            _ => Vec::new(),
        };
        self.sites
            .insert(version, DefinitionSite { token, data_inputs });
    }

    fn entry(&mut self, key: K) -> VersionId {
        if let Some(version) = self.entries.get(&key) {
            return *version;
        }
        let version = self.versions.len();
        self.versions.push(Version::Entry(key));
        self.entries.insert(key, version);
        version
    }

    pub(super) fn read(&mut self, key: K) -> VersionId {
        if let Some(version) = self.current.get(&key) {
            *version
        } else {
            self.entry(key)
        }
    }

    pub(super) fn definition(&mut self, sources: Vec<VersionId>) -> VersionId {
        self.definition_guarded(sources, &PathCondition::default())
    }

    pub(super) fn definition_guarded(
        &mut self,
        sources: Vec<VersionId>,
        condition: &PathCondition,
    ) -> VersionId {
        self.related_definition_guarded(
            sources
                .into_iter()
                .map(|source| (source, PositionRelation::whole()))
                .collect(),
            condition,
        )
    }

    pub(super) fn related_definition(
        &mut self,
        sources: Vec<(VersionId, PositionRelation)>,
    ) -> VersionId {
        self.related_definition_guarded(sources, &PathCondition::default())
    }

    pub(super) fn related_definition_guarded(
        &mut self,
        sources: Vec<(VersionId, PositionRelation)>,
        condition: &PathCondition,
    ) -> VersionId {
        let mut sources = sources;
        sources.sort_unstable();
        sources.dedup();
        let version = self.versions.len();
        self.versions.push(Version::Definition {
            sources,
            condition: condition.clone(),
        });
        version
    }

    pub(super) fn imported(
        &mut self,
        graph: Rc<DependencyDag<K>>,
        root: Option<usize>,
        bindings: Rc<HashMap<K, Vec<(VersionId, PositionRelation)>>>,
        branches: Rc<HashMap<BranchId, BranchId>>,
    ) -> VersionId {
        let version = self.versions.len();
        self.versions.push(Version::Imported {
            graph,
            root,
            bindings,
            branches,
        });
        version
    }

    /// Write `above` at `region` over `below`. A strong write replaces the
    /// previous value there; a weak write may leave it.
    pub(super) fn overlay(
        &mut self,
        below: VersionId,
        above: VersionId,
        region: PositionDomain,
        extent: PositionDomain,
        strong: bool,
    ) -> VersionId {
        let retained = if strong {
            let complement = complement(extent, region);
            if complement.is_empty() {
                return above;
            }
            Some(complement.into())
        } else {
            None
        };
        let log = self.log(below).push(log::Layer::Write {
            region,
            version: above,
            strong,
        });
        let version = self.versions.len();
        self.versions.push(Version::Overlay {
            below,
            retained,
            above,
        });
        self.logs.insert(version, log);
        version
    }

    fn log(&self, version: VersionId) -> Rc<log::LogNode> {
        self.logs
            .get(&version)
            .filter(|_| version >= self.log_floor)
            .cloned()
            .unwrap_or_else(|| log::LogNode::root(version))
    }

    /// The positions of `extent` that `version` definitely overwrites with
    /// respect to the key's entry value, and those it may overwrite only.
    /// A position supplied by a version that may still hold the entry value,
    /// such as a join with an untaken arm, is only possibly written, unless
    /// the entry value is `fresh` and so cannot outlive the call.
    /// `None` when resolving the write history runs out of `work`.
    pub(super) fn written_regions(
        &self,
        version: VersionId,
        extent: PositionDomain,
        fresh: bool,
        work: &mut usize,
    ) -> Option<Regions> {
        // Joins and the versions below write histories can nest as deep as
        // the branches that made them, so they are visited from an explicit
        // stack, each version and region once.
        let mut results: HashMap<(VersionId, PositionDomain), Regions> = HashMap::default();
        let mut stack = vec![(version, extent, false)];
        while let Some((current, domain, expanded)) = stack.pop() {
            if results.contains_key(&(current, domain)) {
                continue;
            }
            *work = work.checked_sub(1)?;
            let parts = self.written_parts(current, domain, work)?;
            let missing = parts
                .dependencies()
                .filter(|part| !results.contains_key(part))
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                debug_assert!(!expanded, "dependencies are resolved before revisiting");
                stack.push((current, domain, true));
                stack.extend(
                    missing
                        .into_iter()
                        .map(|(version, domain)| (version, domain, false)),
                );
                continue;
            }
            let regions = match parts {
                WrittenParts::Join(inputs) => {
                    let inputs = inputs
                        .iter()
                        .map(|input| &results[input])
                        .collect::<Vec<_>>();
                    Self::joined_written_regions(&inputs, work)?
                }
                WrittenParts::Logged {
                    entry,
                    below,
                    writes,
                } => self.logged_written_regions(&entry, &below, writes, &results, fresh, work)?,
                WrittenParts::Leaf => self.leaf_written_regions(current, domain, fresh),
            };
            results.insert((current, domain), regions);
        }
        results.remove(&(version, extent))
    }

    /// How `version` supplies `domain`: from the inputs of a join without a
    /// log, from its write history, or as one value.
    fn written_parts(
        &self,
        version: VersionId,
        domain: PositionDomain,
        work: &mut usize,
    ) -> Option<WrittenParts> {
        let is_entry = |version: VersionId| matches!(self.versions[version], Version::Entry(_));
        let Some(log) = self
            .logs
            .get(&version)
            .filter(|_| version >= self.log_floor)
        else {
            return Some(match &self.versions[version] {
                Version::Phi(inputs) => {
                    WrittenParts::Join(inputs.iter().map(|&input| (input, domain)).collect())
                }
                _ => WrittenParts::Leaf,
            });
        };
        // Resolve the whole history: a partial answer would only say that
        // every position may be written.
        let base = log.base();
        let mut entry = Vec::new();
        let mut below = Vec::new();
        let mut writes = Vec::new();
        for piece in log.resolve(domain, work)? {
            if piece.version != base {
                writes.push(piece);
            } else if is_entry(base) {
                entry.push(piece.domain);
            } else {
                // The version below the history supplies these positions;
                // what it writes there is resolved in turn.
                below.push((base, piece.domain));
            }
        }
        Some(WrittenParts::Logged {
            entry,
            below,
            writes,
        })
    }

    /// A version that is not a join and has no history writes all of
    /// `extent`, possibly so when it may still hold the entry value.
    fn leaf_written_regions(
        &self,
        version: VersionId,
        extent: PositionDomain,
        fresh: bool,
    ) -> Regions {
        if matches!(self.versions[version], Version::Entry(_)) {
            (Vec::new(), Vec::new())
        } else if !fresh && self.may_retain_entry(version) {
            (Vec::new(), vec![extent])
        } else {
            (vec![extent], Vec::new())
        }
    }

    /// The regions of a write history: the writes over the positions that
    /// still hold the entry value or what the version below writes there.
    fn logged_written_regions(
        &self,
        entry: &[PositionDomain],
        below: &[(VersionId, PositionDomain)],
        writes: Vec<log::Piece>,
        results: &HashMap<(VersionId, PositionDomain), Regions>,
        fresh: bool,
        work: &mut usize,
    ) -> Option<Regions> {
        let mut definite = Vec::new();
        let mut maybe = Vec::new();
        // Positions where a weak write may leave the entry value.
        let mut kept = log::Fragments::default();
        for domain in entry {
            kept.insert(*domain);
        }
        for part in below {
            let (below_definite, below_maybe) = &results[part];
            *work = work.checked_sub(below_definite.len().saturating_add(below_maybe.len()))?;
            definite.extend(below_definite.iter().copied());
            maybe.extend(below_maybe.iter().copied());
            // Below the history, a possible write or no write keeps the entry.
            let mut unwritten = vec![part.1];
            for written in below_definite {
                unwritten = unwritten
                    .into_iter()
                    .flat_map(|domain| complement(domain, *written))
                    .collect();
            }
            for domain in unwritten {
                kept.insert(domain);
            }
        }
        let (retaining, writes): (Vec<_>, Vec<_>) = writes
            .into_iter()
            .partition(|piece| !fresh && self.may_retain_entry(piece.version));
        maybe.extend(retaining.into_iter().map(|piece| piece.domain));
        // Each written piece meets only the kept positions that overlap it.
        for piece in writes {
            let mut parts = vec![piece.domain];
            for kept in kept.overlapping(piece.domain, work)? {
                maybe.extend(
                    parts
                        .iter()
                        .filter_map(|part| log::intersection(*part, kept)),
                );
                parts = parts
                    .into_iter()
                    .flat_map(|part| complement(part, kept))
                    .collect();
            }
            definite.extend(parts);
        }
        Some((definite, maybe))
    }

    /// `written_regions` of a join without a log, from those of its inputs:
    /// a position is definitely written only where every input definitely
    /// writes it.
    fn joined_written_regions(inputs: &[&Regions], work: &mut usize) -> Option<Regions> {
        let mut definite: Option<Vec<PositionDomain>> = None;
        let mut touched = Vec::new();
        for (input_definite, input_maybe) in inputs {
            *work = work.checked_sub(
                input_definite
                    .len()
                    .saturating_add(input_maybe.len())
                    .saturating_add(1),
            )?;
            touched.extend(input_definite.iter().copied());
            touched.extend(input_maybe.iter().copied());
            let input_definite = input_definite.clone();
            definite = Some(match definite {
                None => input_definite,
                Some(definite) => {
                    *work =
                        work.checked_sub(definite.len().saturating_mul(input_definite.len()))?;
                    definite
                        .iter()
                        .flat_map(|left| {
                            input_definite
                                .iter()
                                .filter_map(|right| log::intersection(*left, *right))
                        })
                        .collect()
                }
            });
        }
        let definite = definite.unwrap_or_default();
        let mut maybe = Vec::new();
        for part in touched {
            *work = work.checked_sub(definite.len().saturating_add(1))?;
            let mut parts = vec![part];
            for written in &definite {
                parts = parts
                    .into_iter()
                    .flat_map(|part| complement(part, *written))
                    .collect();
            }
            maybe.extend(parts);
        }
        let mut definite = definite;
        for regions in [&mut definite, &mut maybe] {
            regions.sort_unstable_by_key(|domain| {
                (
                    domain.array_start,
                    domain.packed_start,
                    domain.array_length,
                    domain.packed_length,
                )
            });
            regions.dedup();
        }
        Some((definite, maybe))
    }

    /// Whether some position of `version` may still be the key's entry
    /// value: the entry is reachable through values that keep retention
    /// rather than read it. Imported values may.
    fn may_retain_entry(&self, version: VersionId) -> bool {
        let mut retains = self.retains_entry.borrow_mut();
        for next in retains.len()..=version {
            let value = match &self.versions[next] {
                Version::Entry(_) | Version::Imported { .. } => true,
                Version::Phi(inputs) => inputs.iter().any(|input| retains[*input]),
                Version::Overlay { below, above, .. } => retains[*below] || retains[*above],
                Version::Guarded { source, .. } | Version::Restricted { source, .. } => {
                    retains[*source]
                }
                Version::Definition { .. }
                | Version::Projected { .. }
                | Version::Replicated { .. } => false,
            };
            retains.push(value);
        }
        retains[version]
    }

    /// The writes of the history of `version` that may supply `extent`, each
    /// with its region, including the version below the history. `Ok(None)`
    /// when it has no history, and `Err` when resolving it runs out of
    /// `work`.
    pub(super) fn logged_pieces(
        &self,
        version: VersionId,
        extent: PositionDomain,
        work: &mut usize,
    ) -> Result<Option<Vec<(VersionId, PositionDomain)>>, RanOut> {
        let Some(log) = self
            .logs
            .get(&version)
            .filter(|_| version >= self.log_floor)
        else {
            return Ok(None);
        };
        let pieces = log.resolve(extent, work).ok_or(RanOut)?;
        Ok(Some(
            pieces
                .into_iter()
                .map(|piece| (piece.version, piece.domain))
                .collect(),
        ))
    }

    /// Make every existing version opaque to regional reads until the
    /// returned floor is restored.
    pub(super) fn raise_log_floor(&mut self) -> VersionId {
        std::mem::replace(&mut self.log_floor, self.versions.len())
    }

    pub(super) fn restore_log_floor(&mut self, floor: VersionId) {
        self.log_floor = floor;
    }

    /// Read `domain` from the write history of `version`: only the writes
    /// that may supply the domain. `Ok(None)` when it has no history, and
    /// `Err` when resolving the history runs out of `work`.
    pub(super) fn read_logged(
        &mut self,
        version: VersionId,
        domain: PositionDomain,
        work: &mut usize,
    ) -> Result<Option<VersionId>, RanOut> {
        if version < self.log_floor {
            return Ok(None);
        }
        if let Some(&read) = self.logged_reads.get(&(version, domain)) {
            return Ok(Some(read));
        }
        let Some(log) = self.logs.get(&version) else {
            return Ok(None);
        };
        let pieces = log.resolve(domain, work).ok_or(RanOut)?;
        let inputs = pieces
            .into_iter()
            .map(|piece| {
                let version = self.versions.len();
                self.versions.push(Version::Restricted {
                    source: piece.version,
                    domain: piece.domain,
                });
                version
            })
            .collect::<Vec<_>>();
        let read = if inputs.is_empty() {
            self.definition(Vec::new())
        } else {
            self.phi(inputs)
        };
        self.logged_reads.insert((version, domain), read);
        Ok(Some(read))
    }

    pub(super) fn projected(&mut self, source: VersionId, domain: PositionDomain) -> VersionId {
        let version = self.versions.len();
        self.versions.push(Version::Projected { source, domain });
        version
    }

    /// Export into a destination that already enforces `domain`. Other SSA
    /// readers retain the original projection and its intermediate bounds.
    /// `source` restricted to `domain` without reading it: unlike a
    /// projection, a value kept only through restrictions keeps the key's
    /// entry value as retention.
    pub(super) fn restricted(&mut self, source: VersionId, domain: PositionDomain) -> VersionId {
        let version = self.versions.len();
        self.versions.push(Version::Restricted { source, domain });
        version
    }

    pub(super) fn root_in_domain(&self, version: VersionId, domain: PositionDomain) -> VersionId {
        match &self.versions[version] {
            Version::Projected {
                source,
                domain: projected,
            } if *projected == domain => *source,
            _ => version,
        }
    }

    pub(super) fn replicated(
        &mut self,
        source: VersionId,
        domain: PositionDomain,
        replication: Replication,
    ) -> VersionId {
        assert!(
            replication.stride() > 0,
            "replication must advance its axis"
        );
        let version = self.versions.len();
        self.versions.push(Version::Replicated {
            source,
            domain,
            replication,
        });
        version
    }

    pub(super) fn has_structural_dependency(&self, version: VersionId) -> bool {
        let mut visited = HashSet::default();
        let mut queue = VecDeque::from([version]);
        while let Some(version) = queue.pop_front() {
            if !visited.insert(version) {
                continue;
            }
            match &self.versions[version] {
                Version::Imported { .. }
                | Version::Projected { .. }
                | Version::Replicated { .. }
                | Version::Overlay { .. }
                | Version::Restricted { .. } => return true,
                Version::Definition { sources, .. } => {
                    queue.extend(sources.iter().map(|(source, _)| *source));
                }
                Version::Phi(inputs) => queue.extend(inputs.iter().copied()),
                Version::Guarded { source, .. } => queue.push_back(*source),
                Version::Entry(_) => {}
            }
        }
        false
    }

    pub(super) fn bind(&mut self, key: K, version: VersionId) {
        let previous = self.current.insert(key, version);
        if !self.checkpoints.is_empty() {
            self.undo.push(Undo { key, previous });
        }
    }

    pub(super) fn weak_bind(&mut self, key: K, version: VersionId) {
        let previous = self.read(key);
        let version = self.phi(vec![previous, version]);
        self.bind(key, version);
    }

    pub(super) fn checkpoint(&mut self) -> Checkpoint {
        let checkpoint = Checkpoint {
            undo_start: self.undo.len(),
            depth: self.checkpoints.len(),
            version_start: self.versions.len(),
        };
        self.checkpoints.push(checkpoint.undo_start);
        checkpoint
    }

    pub(super) fn capture_and_rollback(&mut self, checkpoint: Checkpoint) -> BranchState<K> {
        assert_eq!(checkpoint.depth + 1, self.checkpoints.len());
        assert_eq!(self.checkpoints.pop(), Some(checkpoint.undo_start));

        let mut bindings = HashMap::default();
        for undo in &self.undo[checkpoint.undo_start..] {
            let version = self
                .current
                .get(&undo.key)
                .copied()
                .expect("a branch binding must exist until rollback");
            bindings.insert(undo.key, version);
        }

        while self.undo.len() > checkpoint.undo_start {
            let undo = self.undo.pop().expect("undo length checked above");
            if let Some(previous) = undo.previous {
                self.current.insert(undo.key, previous);
            } else {
                self.current.remove(&undo.key);
            }
        }
        bindings.retain(|key, version| self.current.get(key).copied() != Some(*version));
        BranchState { bindings }
    }

    /// Capture bindings changed since an enclosing checkpoint without
    /// disturbing the current transaction. This records an early-exit path
    /// before its nearer branch checkpoint rolls back.
    pub(super) fn snapshot_since(&self, checkpoint: Checkpoint) -> BranchState<K> {
        assert!(checkpoint.depth < self.checkpoints.len());
        assert_eq!(self.checkpoints[checkpoint.depth], checkpoint.undo_start);

        let mut bindings = HashMap::default();
        for undo in &self.undo[checkpoint.undo_start..] {
            if let Some(version) = self.current.get(&undo.key) {
                bindings.insert(undo.key, *version);
            }
        }
        BranchState { bindings }
    }

    pub(super) fn merge<'b>(&mut self, states: impl IntoIterator<Item = &'b BranchState<K>>)
    where
        K: 'b,
    {
        let mut inputs_by_key: HashMap<K, (Vec<VersionId>, usize)> = HashMap::default();
        let mut state_count = 0;
        for state in states {
            state_count += 1;
            for (&key, &version) in &state.bindings {
                let (inputs, bound_branches) =
                    inputs_by_key.entry(key).or_insert_with(|| (Vec::new(), 0));
                inputs.push(version);
                *bound_branches += 1;
            }
        }
        for (key, (mut inputs, bound_branches)) in inputs_by_key {
            let fallback = self
                .current
                .get(&key)
                .copied()
                .unwrap_or_else(|| self.entry(key));
            if bound_branches < state_count {
                inputs.push(fallback);
            }
            let log = (inputs.iter().any(|input| self.logs.contains_key(input)))
                .then(|| {
                    let common = self.log(fallback);
                    let branches = inputs.iter().map(|input| self.log(*input)).collect();
                    log::LogNode::merge(&common, branches)
                })
                .flatten();
            let first_new = self.versions.len();
            let version = self.phi(inputs);
            if let Some(log) = log
                && version >= first_new
            {
                self.logs.insert(version, log);
            }
            self.bind(key, version);
        }
    }

    /// Merge both expression arms, including bindings retained by either arm.
    /// Each conditional write also depends on the condition's value.
    pub(super) fn merge_conditional(
        &mut self,
        states: [(&BranchState<K>, &PathCondition); 2],
        controls: &[VersionId],
        domain: impl Fn(K) -> Option<PositionDomain>,
    ) {
        let keys = states
            .iter()
            .flat_map(|(state, _)| state.bindings.keys().copied())
            .collect::<HashSet<_>>();
        if keys.is_empty() {
            return;
        }
        let control = self.definition(controls.to_vec());
        for key in keys {
            let fallback = self.read(key);
            let inputs = states
                .into_iter()
                .map(|(state, condition)| {
                    let value = state.bindings.get(&key).copied().unwrap_or(fallback);
                    let source = self.phi(vec![value, control]);
                    let version = self.versions.len();
                    // A guard is an alias, not a read. Bare entry versions
                    // retained on the skipped path must remain state retention
                    // until another expression actually reads the merged value.
                    self.versions.push(Version::Guarded {
                        source,
                        condition: condition.clone(),
                    });
                    version
                })
                .collect();
            let version = self.phi(inputs);
            let version = domain(key).map_or(version, |domain| self.projected(version, domain));
            self.bind(key, version);
        }
    }

    /// Apply the transitive closure of a runtime loop's may-dependency
    /// transfer without enumerating runtime iterator values or iterations.
    ///
    /// `single_iteration` maps each written key to its output after one
    /// abstract iteration. Versions that predate `iteration_checkpoint` are
    /// that iteration's inputs, so they form the nodes of a finite transfer
    /// graph. Condensing its recurrence components models arbitrary positive
    /// iteration counts without enumerating positions or paths. `may_skip`
    /// additionally retains each key's loop-entry version.
    pub(super) fn try_close_repeated_transfer(
        &mut self,
        single_iteration: &BranchState<K>,
        iteration_checkpoint: Checkpoint,
        may_skip: bool,
        work: &mut usize,
        domain: impl Fn(K) -> Option<PositionDomain>,
    ) -> Option<()> {
        repeated::try_close(
            self,
            single_iteration,
            iteration_checkpoint,
            may_skip,
            work,
            domain,
        )
    }

    #[cfg(test)]
    pub(super) fn close_repeated_transfer(
        &mut self,
        single_iteration: &BranchState<K>,
        iteration_checkpoint: Checkpoint,
        may_skip: bool,
        domain: impl Fn(K) -> Option<PositionDomain>,
    ) {
        let mut work = usize::MAX;
        self.try_close_repeated_transfer(
            single_iteration,
            iteration_checkpoint,
            may_skip,
            &mut work,
            domain,
        )
        .expect("unlimited runtime transfer construction");
    }

    #[cfg(test)]
    pub(super) fn root_sources(&self, version: VersionId) -> HashSet<K> {
        self.root_source_relations(version).into_keys().collect()
    }

    #[cfg(test)]
    pub(super) fn root_source_relations(&self, version: VersionId) -> HashMap<K, PositionRelation> {
        let mut sources: HashMap<K, PositionRelation> = HashMap::default();
        for (source, relation, _) in self.root_source_relations_guarded(version) {
            sources
                .entry(source)
                .and_modify(|existing| *existing = existing.union(relation))
                .or_insert(relation);
        }
        sources
    }

    #[cfg(test)]
    pub(super) fn root_source_relations_guarded(
        &self,
        version: VersionId,
    ) -> Vec<(K, PositionRelation, PathCondition)> {
        let mut work = usize::MAX;
        self.try_root_source_relations_guarded(version, &mut work)
            .expect("unlimited source query")
    }

    pub(super) fn try_root_source_relations_guarded(
        &self,
        version: VersionId,
        work: &mut usize,
    ) -> Option<Vec<(K, PositionRelation, PathCondition)>> {
        self.try_root_source_relations_guarded_cached(version, &mut SourceCache::default(), work)
    }

    #[cfg(test)]
    pub(super) fn root_source_keys_guarded(&self, version: VersionId) -> Vec<(K, PathCondition)> {
        let mut work = usize::MAX;
        self.try_root_source_keys_guarded(version, &mut work)
            .expect("unlimited source query")
    }

    /// Whole-value reads need source identities and guards, not every possible
    /// sum of shifts through an imported DAG. Forget positions before walking.
    pub(super) fn try_root_source_keys_guarded(
        &self,
        version: VersionId,
        work: &mut usize,
    ) -> Option<Vec<(K, PathCondition)>> {
        let mut cache = SourceCache {
            ignore_position: true,
            ..SourceCache::default()
        };
        Some(
            self.try_root_source_relations_guarded_cached(version, &mut cache, work)?
                .into_iter()
                .map(|(key, _, condition)| (key, condition))
                .collect(),
        )
    }

    fn try_root_source_relations_guarded_cached(
        &self,
        version: VersionId,
        cache: &mut SourceCache<K>,
        work: &mut usize,
    ) -> Option<Vec<(K, PositionRelation, PathCondition)>> {
        // SSA versions form a DAG. Summarize each (version, relation) once and
        // combine branch alternatives at the join instead of re-walking the
        // same suffix for every feasible path.
        let sources = self.source_summary(version, false, cache, work)?;
        Some(
            sources
                .iter()
                .map(|(&(source, relation), condition)| (source, relation, condition.clone()))
                .collect(),
        )
    }

    #[cfg(test)]
    pub(super) fn dependency_dag(
        &self,
        roots: &[VersionId],
        allowed: impl Fn(&K) -> bool,
    ) -> DependencyDag<K>
    where
        K: Ord,
    {
        let mut work = usize::MAX;
        self.try_dependency_dag(roots, allowed, &mut work)
            .expect("unlimited dependency export")
    }

    /// Export the dependencies of `roots`, counting each step of the walk and
    /// of every imported summary down from `work`. `None` when it runs out.
    pub(super) fn try_dependency_dag(
        &self,
        roots: &[VersionId],
        allowed: impl Fn(&K) -> bool,
        work: &mut usize,
    ) -> Option<DependencyDag<K>>
    where
        K: Ord,
    {
        let mut states = HashSet::default();
        let mut visited_bindings = HashSet::default();
        let mut queue = VecDeque::new();
        for &root in roots {
            if states.insert((root, false)) {
                queue.push_back((root, false));
            }
        }
        while let Some((version, include_entry)) = queue.pop_front() {
            *work = work.checked_sub(1)?;
            let mut enqueue = |state| {
                if states.insert(state) {
                    queue.push_back(state);
                }
            };
            match &self.versions[version] {
                Version::Entry(_) => {}
                Version::Definition { sources, .. } => {
                    for (source, _) in sources {
                        enqueue((*source, true));
                    }
                }
                Version::Phi(inputs) => {
                    for input in inputs {
                        enqueue((*input, include_entry));
                    }
                }
                Version::Guarded { source, .. } => enqueue((*source, include_entry)),
                Version::Overlay { below, above, .. } => {
                    enqueue((*below, include_entry));
                    enqueue((*above, include_entry));
                }
                Version::Restricted { source, .. } => enqueue((*source, include_entry)),
                Version::Imported { bindings, .. } => {
                    // All output roots of a call share the same actuals.
                    if visited_bindings.insert(Rc::as_ptr(bindings)) {
                        #[cfg(test)]
                        IMPORT_BINDING_VISITS.set(IMPORT_BINDING_VISITS.get() + bindings.len());
                        *work = work.checked_sub(bindings.len())?;
                        for sources in bindings.values() {
                            *work = work.checked_sub(sources.len())?;
                            for (source, _) in sources {
                                enqueue((*source, true));
                            }
                        }
                    }
                }
                Version::Projected { source, .. } | Version::Replicated { source, .. } => {
                    enqueue((*source, true))
                }
            }
        }

        // Intern only the exported graph. SSA version identities must remain
        // distinct for checkpoint boundaries, runtime transfers and writes.
        let mut builder = dag::Builder::new();
        let mut mapped: HashMap<(VersionId, bool), Option<usize>> = HashMap::default();
        let mut imports = dag::Imports::default();

        let mut ordered = states.into_iter().collect::<Vec<_>>();
        ordered.sort_unstable();
        for state @ (version, include_entry) in ordered {
            *work = work.checked_sub(1)?;
            let site = self.sites.get(&version).map(|site| DefinitionSite {
                token: site.token,
                data_inputs: site
                    .data_inputs
                    .iter()
                    .filter_map(|input| mapped.get(&(*input, true)).copied().flatten())
                    .collect(),
            });
            let node = match &self.versions[version] {
                Version::Entry(key) => {
                    (include_entry && allowed(key)).then(|| builder.external(*key))
                }
                Version::Definition { sources, condition } => {
                    *work = work.checked_sub(
                        sources
                            .len()
                            .saturating_mul(condition.work_size().saturating_add(1)),
                    )?;
                    let inputs = sources
                        .iter()
                        .filter_map(|(source, relation)| {
                            mapped[&(*source, true)]
                                .map(|source| (source, *relation, condition.clone()))
                        })
                        .collect();
                    Some(builder.internal(inputs, Vec::new(), site))
                }
                Version::Phi(inputs) => {
                    let inputs = inputs
                        .iter()
                        .filter_map(|input| {
                            mapped[&(*input, include_entry)].map(|source| {
                                (
                                    source,
                                    PositionRelation::default(),
                                    PathCondition::default(),
                                )
                            })
                        })
                        .collect();
                    Some(builder.internal(inputs, Vec::new(), site))
                }
                Version::Overlay {
                    below,
                    retained,
                    above,
                } => {
                    let identity = |source| {
                        (
                            source,
                            PositionRelation::default(),
                            PathCondition::default(),
                        )
                    };
                    let mut inputs = Vec::new();
                    if let Some(below) = mapped[&(*below, include_entry)] {
                        let below = match retained {
                            Some(domains) => {
                                builder.internal(vec![identity(below)], domains.to_vec(), None)
                            }
                            None => below,
                        };
                        inputs.push(identity(below));
                    }
                    inputs.extend(mapped[&(*above, include_entry)].map(identity));
                    Some(builder.internal(inputs, Vec::new(), site))
                }
                Version::Restricted { source, domain } => {
                    let inputs = mapped[&(*source, include_entry)]
                        .map(|source| {
                            (
                                source,
                                PositionRelation::default(),
                                PathCondition::default(),
                            )
                        })
                        .into_iter()
                        .collect();
                    Some(builder.internal(inputs, vec![*domain], site))
                }
                Version::Guarded { source, condition } => {
                    *work = work.checked_sub(condition.work_size().saturating_add(1))?;
                    let inputs = mapped[&(*source, include_entry)]
                        .map(|source| (source, PositionRelation::default(), condition.clone()))
                        .into_iter()
                        .collect();
                    Some(builder.internal(inputs, Vec::new(), site))
                }
                Version::Imported {
                    graph,
                    root,
                    bindings,
                    branches,
                } => imports.inline(
                    graph,
                    *root,
                    bindings,
                    branches,
                    &mapped,
                    &mut builder,
                    work,
                )?,
                Version::Projected { source, domain } => {
                    let inputs = mapped[&(*source, true)]
                        .map(|source| {
                            (
                                source,
                                PositionRelation::default(),
                                PathCondition::default(),
                            )
                        })
                        .into_iter()
                        .collect();
                    Some(builder.internal(inputs, vec![*domain], site))
                }
                Version::Replicated {
                    source,
                    domain,
                    replication,
                } => {
                    let inputs = mapped[&(*source, true)]
                        .map(|source| {
                            (
                                source,
                                PositionRelation::default(),
                                PathCondition::default(),
                            )
                        })
                        .into_iter()
                        .collect();
                    Some(builder.replicated(inputs, vec![*domain], site, *replication))
                }
            };
            mapped.insert(state, node);
        }

        builder.graph.roots = roots.iter().map(|root| mapped[&(*root, false)]).collect();
        Some(builder.graph)
    }

    fn phi(&mut self, mut inputs: Vec<VersionId>) -> VersionId {
        inputs.sort_unstable();
        inputs.dedup();
        if inputs.len() == 1 {
            return inputs[0];
        }
        let version = self.versions.len();
        self.versions.push(Version::Phi(inputs));
        version
    }

    fn source_summary(
        &self,
        version: VersionId,
        include_entry: bool,
        cache: &mut SourceCache<K>,
        work: &mut usize,
    ) -> Option<Rc<SourceMap<K>>> {
        let cache_key = (version, include_entry);
        if let Some(sources) = cache.summaries.get(&cache_key) {
            return Some(sources.clone());
        }

        let mut sources = HashMap::default();
        let initial_relation = if cache.ignore_position {
            PositionRelation::whole()
        } else {
            PositionRelation::default()
        };
        let start = (version, include_entry, initial_relation);
        let mut reached = HashMap::default();
        reached.insert(start, PathCondition::default());
        let mut queued = HashSet::default();
        queued.insert(start);
        let mut queue = VecDeque::from([start]);

        while let Some(state @ (current, include_entry, relation)) = queue.pop_front() {
            #[cfg(test)]
            SOURCE_WALK_VISITS.set(SOURCE_WALK_VISITS.get() + 1);
            queued.remove(&state);
            let condition = reached[&state].clone();

            if current != version
                && let Some(cached) = cache.summaries.get(&(current, include_entry))
            {
                merge_source_summaries(
                    &mut sources,
                    cached,
                    Some(&condition),
                    Some(relation),
                    work,
                )?;
                continue;
            }

            let mut enqueue = |next: (VersionId, bool, PositionRelation),
                               condition: PathCondition,
                               work: &mut usize| {
                let changed = if let Some(existing) = reached.get_mut(&next) {
                    reserve_guard_work(work, [&*existing, &condition])?;
                    let widened = existing.disjoin(&condition);
                    if *existing == widened {
                        false
                    } else {
                        *existing = widened;
                        true
                    }
                } else {
                    reached.insert(next, condition);
                    true
                };
                if changed && queued.insert(next) {
                    queue.push_back(next);
                }
                Some(())
            };

            match &self.versions[current] {
                Version::Entry(key) => {
                    if include_entry {
                        merge_source(&mut sources, (*key, relation), condition, work)?;
                    }
                }
                Version::Definition {
                    sources,
                    condition: definition_condition,
                } => {
                    reserve_guard_work(work, [&condition, definition_condition])?;
                    let Some(condition) = condition.conjoin_if_compatible(definition_condition)
                    else {
                        continue;
                    };
                    for (input, offset) in sources {
                        enqueue(
                            (*input, true, relation.compose(*offset)),
                            condition.clone(),
                            work,
                        )?;
                    }
                }
                Version::Phi(inputs) => {
                    for input in inputs {
                        enqueue((*input, include_entry, relation), condition.clone(), work)?;
                    }
                }
                Version::Guarded {
                    source,
                    condition: guard,
                } => {
                    reserve_guard_work(work, [&condition, guard])?;
                    if let Some(condition) = condition.conjoin_if_compatible(guard) {
                        enqueue((*source, include_entry, relation), condition, work)?;
                    }
                }
                // Scalar source queries ignore the written region.
                Version::Overlay { below, above, .. } => {
                    enqueue((*below, include_entry, relation), condition.clone(), work)?;
                    enqueue((*above, include_entry, relation), condition, work)?;
                }
                Version::Restricted { source, .. } => {
                    enqueue((*source, include_entry, relation), condition, work)?;
                }
                Version::Imported {
                    graph,
                    root,
                    bindings,
                    branches,
                } => {
                    for (key, imported_relation, imported_condition) in
                        dependency_dag_external_sources(graph, *root, initial_relation, work)?
                    {
                        reserve_guard_work(work, [&imported_condition])?;
                        let imported_condition = imported_condition.remapped(branches);
                        reserve_guard_work(work, [&condition, &imported_condition])?;
                        let Some(condition) = condition.conjoin_if_compatible(&imported_condition)
                        else {
                            continue;
                        };
                        for (source, binding_relation) in bindings.get(&key).into_iter().flatten() {
                            enqueue(
                                (
                                    *source,
                                    true,
                                    relation
                                        .compose(*binding_relation)
                                        .compose(imported_relation),
                                ),
                                condition.clone(),
                                work,
                            )?;
                        }
                    }
                }
                Version::Projected { source, .. } => {
                    enqueue((*source, true, relation), condition, work)?;
                }
                Version::Replicated {
                    source,
                    replication,
                    ..
                } => {
                    // Scalar source queries cannot represent periodic positions.
                    // Exact positional consumers retain the structural operation.
                    enqueue(
                        (*source, true, replication.forget_position(relation)),
                        condition,
                        work,
                    )?;
                }
            }
        }
        let sources = Rc::new(sources);
        cache.summaries.insert(cache_key, sources.clone());
        Some(sources)
    }
}

fn dependency_dag_external_sources<K>(
    graph: &DependencyDag<K>,
    root: Option<usize>,
    initial_relation: PositionRelation,
    work: &mut usize,
) -> Option<Vec<(K, PositionRelation, PathCondition)>>
where
    K: Copy + Eq + Hash,
{
    let Some(root) = root else {
        return Some(Vec::new());
    };
    let mut incoming: HashMap<usize, Vec<&DependencyDagEdge>> = HashMap::default();
    for edge in &graph.edges {
        incoming.entry(edge.destination).or_default().push(edge);
    }
    let mut reached = HashMap::default();
    let start = (root, initial_relation);
    reached.insert(start, PathCondition::default());
    let mut queue = VecDeque::from([start]);
    let mut queued = [start].into_iter().collect::<HashSet<_>>();
    let mut sources: HashMap<(K, PositionRelation), PathCondition> = HashMap::default();
    while let Some(state @ (node, relation)) = queue.pop_front() {
        #[cfg(test)]
        SOURCE_WALK_VISITS.set(SOURCE_WALK_VISITS.get() + 1);
        queued.remove(&state);
        let condition = reached[&state].clone();
        if let DependencyDagNode::External(key) = graph.nodes[node] {
            merge_source(&mut sources, (key, relation), condition, work)?;
            continue;
        }
        let relation = if let DependencyDagNode::Replicated { replication } = graph.nodes[node] {
            replication.forget_position(relation)
        } else {
            relation
        };
        for edge in incoming.get(&node).into_iter().flatten() {
            reserve_guard_work(work, [&condition, &edge.condition])?;
            let Some(next_condition) = condition.conjoin_if_compatible(&edge.condition) else {
                continue;
            };
            let next = (edge.source, relation.compose(edge.relation));
            let changed = if let Some(existing) = reached.get_mut(&next) {
                reserve_guard_work(work, [&*existing, &next_condition])?;
                let merged = existing.disjoin(&next_condition);
                if *existing == merged {
                    false
                } else {
                    *existing = merged;
                    true
                }
            } else {
                reached.insert(next, next_condition);
                true
            };
            if changed && queued.insert(next) {
                queue.push_back(next);
            }
        }
    }
    Some(
        sources
            .into_iter()
            .map(|((key, relation), condition)| (key, relation, condition))
            .collect(),
    )
}

/// The positions of `extent` outside `region`, as disjoint boxes.
/// Positions definitely written, and positions possibly written only.
type Regions = (Vec<PositionDomain>, Vec<PositionDomain>);

/// How a version supplies a region, for `written_regions`.
enum WrittenParts {
    /// The inputs of a join without a write history, each with the region.
    Join(Vec<(VersionId, PositionDomain)>),
    Logged {
        /// Positions that still hold the key's entry value.
        entry: Vec<PositionDomain>,
        /// Positions supplied by the version below the history.
        below: Vec<(VersionId, PositionDomain)>,
        writes: Vec<log::Piece>,
    },
    Leaf,
}

impl WrittenParts {
    fn dependencies(&self) -> impl Iterator<Item = (VersionId, PositionDomain)> + '_ {
        let parts: &[(VersionId, PositionDomain)] = match self {
            Self::Join(inputs) => inputs,
            Self::Logged { below, .. } => below,
            Self::Leaf => &[],
        };
        parts.iter().copied()
    }
}

pub(super) fn complement(extent: PositionDomain, region: PositionDomain) -> Vec<PositionDomain> {
    let array_end = extent.array_start.saturating_add(extent.array_length);
    let packed_end = extent.packed_start.saturating_add(extent.packed_length);
    let start = region.array_start.max(extent.array_start);
    let end = region
        .array_start
        .saturating_add(region.array_length)
        .min(array_end);
    let low = region.packed_start.max(extent.packed_start);
    let high = region
        .packed_start
        .saturating_add(region.packed_length)
        .min(packed_end);
    if start >= end || low >= high {
        return vec![extent];
    }
    let mut pieces = Vec::new();
    let mut push =
        |array_start: usize, array_end: usize, packed_start: usize, packed_end: usize| {
            if array_start < array_end && packed_start < packed_end {
                pieces.push(PositionDomain {
                    array_start,
                    array_length: array_end - array_start,
                    packed_start,
                    packed_length: packed_end - packed_start,
                });
            }
        };
    push(extent.array_start, start, extent.packed_start, packed_end);
    push(end, array_end, extent.packed_start, packed_end);
    push(start, end, extent.packed_start, low);
    push(start, end, high, packed_end);
    pieces
}

/// Joins and source walks can accumulate quadratic guard payloads. Charge each
/// allocating operation before combining/remapping conditions, including
/// fragmented arm ranges. Rc clones and unguarded traversals need no extra work.
fn reserve_guard_work<'a>(
    work: &mut usize,
    conditions: impl IntoIterator<Item = &'a PathCondition>,
) -> Option<()> {
    let cost = conditions.into_iter().fold(0usize, |cost, condition| {
        // Allocating joins visit the constraints as well as their range data.
        cost.saturating_add(condition.branch_count())
            .saturating_add(condition.work_size())
    });
    *work = work.checked_sub(cost)?;
    Some(())
}

fn merge_source<K>(
    destination: &mut SourceMap<K>,
    key: (K, PositionRelation),
    condition: PathCondition,
    work: &mut usize,
) -> Option<()>
where
    K: Copy + Eq + Hash,
{
    if let Some(existing) = destination.get_mut(&key) {
        reserve_guard_work(work, [&*existing, &condition])?;
        *existing = existing.disjoin(&condition);
    } else {
        destination.insert(key, condition);
    }
    Some(())
}

fn merge_source_summaries<K>(
    destination: &mut SourceMap<K>,
    sources: &SourceMap<K>,
    guard: Option<&PathCondition>,
    prefix: Option<PositionRelation>,
    work: &mut usize,
) -> Option<()>
where
    K: Copy + Eq + Hash,
{
    for (&(source, relation), condition) in sources {
        let key = (
            source,
            prefix.map_or(relation, |prefix| prefix.compose(relation)),
        );
        let condition = if let Some(guard) = guard {
            reserve_guard_work(work, [condition, guard])?;
            let Some(condition) = condition.conjoin_if_compatible(guard) else {
                continue;
            };
            condition
        } else {
            condition.clone()
        };
        merge_source(destination, key, condition, work)?;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_source_queries_do_not_cache_partial_results() {
        for imported in [false, true] {
            let mut ssa = SsaStore::default();
            let mut value = ssa.read("input");
            let mut expected = PathCondition::default();
            for local in 0..64 {
                let branch = BranchId::new(1, local, 2);
                let condition = PathCondition::default().with_choice(branch, 0);
                expected = expected.with_choice(branch, 0);
                value = ssa.related_definition_guarded(
                    vec![(value, PositionRelation::default())],
                    &condition,
                );
            }
            let other = ssa.read("other");
            let root = ssa.related_definition(vec![
                (value, PositionRelation::default()),
                (other, PositionRelation::default()),
            ]);
            let (ssa, root) = if imported {
                let graph = Rc::new(ssa.dependency_dag(&[root], |_| true));
                let mut caller = SsaStore::default();
                let bindings = ["input", "other"]
                    .into_iter()
                    .map(|key| (key, vec![(caller.read(key), PositionRelation::default())]))
                    .collect::<HashMap<_, _>>();
                let root = caller.imported(
                    Rc::clone(&graph),
                    graph.roots[0],
                    Rc::new(bindings),
                    Rc::default(),
                );
                (caller, root)
            } else {
                (ssa, root)
            };
            let mut cache = SourceCache::default();
            let mut work = 1024;
            assert!(
                ssa.try_root_source_relations_guarded_cached(root, &mut cache, &mut work)
                    .is_none(),
                "imported={imported}"
            );
            assert!(cache.summaries.is_empty(), "discard partial source results");

            let mut work = 100_000;
            let mut sources = ssa
                .try_root_source_relations_guarded_cached(root, &mut cache, &mut work)
                .expect("a complete query preserves its guards and positions");
            sources.sort_unstable_by_key(|(key, _, _)| *key);
            assert_eq!(
                sources,
                [
                    ("input", PositionRelation::default(), expected),
                    (
                        "other",
                        PositionRelation::default(),
                        PathCondition::default()
                    ),
                ]
            );
        }
    }

    #[test]
    fn multi_output_invocation_imports_shared_predecessors_once() {
        for size in [64, 256, 1024] {
            let mut callee = SsaStore::default();
            let mut value = callee.definition(Vec::new());
            let roots = (0..size)
                .map(|key| {
                    let input = callee.read(key);
                    value = callee.definition(vec![value, input]);
                    value
                })
                .collect::<Vec<_>>();
            let graph = Rc::new(callee.dependency_dag(&roots, |_| true));
            let mut caller = SsaStore::default();
            let bindings = Rc::new(
                (0..size)
                    .map(|key| (key, vec![(caller.read(key), PositionRelation::default())]))
                    .collect(),
            );
            let branches = Rc::default();
            let roots = graph
                .roots
                .iter()
                .map(|root| {
                    caller.imported(
                        graph.clone(),
                        *root,
                        Rc::clone(&bindings),
                        Rc::clone(&branches),
                    )
                })
                .collect::<Vec<_>>();
            // Every output includes the preceding outputs. Both normalizing
            // all actuals and walking that prefix per root would be quadratic.
            let exported = caller
                .try_dependency_dag(&roots, |_| true, &mut (size * 32))
                .expect("one invocation must fit in a linear export budget");
            assert_eq!(exported.nodes.len(), graph.nodes.len());
            assert_eq!(exported.edges.len(), graph.edges.len());
            for index in [0, size / 2, size - 1] {
                let mut work = 0;
                let sources = dependency_dag_external_sources(
                    &exported,
                    exported.roots[index],
                    PositionRelation::whole(),
                    &mut work,
                )
                .expect("unguarded source walks need no guard budget")
                .into_iter()
                .map(|(key, _, _)| key)
                .collect::<HashSet<_>>();
                assert_eq!(sources, (0..=index).collect());
            }
        }
    }

    #[test]
    fn definition_reports_live_on_entry_source() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination = ssa.definition(vec![source]);

        let expected = ["source"].into_iter().collect::<HashSet<_>>();
        assert_eq!(ssa.root_sources(destination), expected);
    }

    #[test]
    fn related_definition_preserves_source_relation() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination = ssa.related_definition(vec![(source, PositionRelation::default())]);

        assert_eq!(
            ssa.root_source_relations(destination).get("source"),
            Some(&PositionRelation::default())
        );
    }

    #[test]
    fn replicated_source_queries_forget_only_the_repeated_axis() {
        for (replication, expected) in [
            (
                Replication::Array(2),
                PositionRelation {
                    array: None,
                    packed: Some(3),
                },
            ),
            (
                Replication::Packed(2),
                PositionRelation {
                    array: Some(1),
                    packed: None,
                },
            ),
        ] {
            let mut ssa = SsaStore::default();
            let source = ssa.read("source");
            let translated = ssa.related_definition(vec![(
                source,
                PositionRelation {
                    array: Some(1),
                    packed: Some(3),
                },
            )]);
            let repeated = ssa.replicated(
                translated,
                PositionDomain {
                    array_start: 0,
                    array_length: 8,
                    packed_start: 0,
                    packed_length: 8,
                },
                replication,
            );
            assert_eq!(
                ssa.root_source_relations(repeated).get("source"),
                Some(&expected)
            );
            let dag = ssa.dependency_dag(&[repeated], |_| true);
            let sources = dependency_dag_external_sources(
                &dag,
                dag.roots[0],
                PositionRelation::default(),
                &mut 0,
            )
            .unwrap();
            assert_eq!(sources, [("source", expected, PathCondition::default())]);
        }
    }

    #[test]
    fn positional_offsets_compose_through_definitions() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let first = ssa.related_definition(vec![(
            source,
            PositionRelation {
                array: Some(3),
                packed: Some(-2),
            },
        )]);
        let destination = ssa.related_definition(vec![(
            first,
            PositionRelation {
                array: Some(-1),
                packed: Some(5),
            },
        )]);

        assert_eq!(
            ssa.root_source_relations(destination).get("source"),
            Some(&PositionRelation {
                array: Some(2),
                packed: Some(3),
            })
        );
    }

    #[test]
    fn conflicting_offsets_only_widen_the_conflicting_axis() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination = ssa.related_definition(vec![
            (source, PositionRelation::default()),
            (
                source,
                PositionRelation {
                    array: Some(0),
                    packed: Some(1),
                },
            ),
        ]);

        assert_eq!(
            ssa.root_source_relations(destination).get("source"),
            Some(&PositionRelation {
                array: Some(0),
                packed: None,
            })
        );
    }

    #[test]
    fn whole_dependency_dominates_a_positional_path() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination = ssa.related_definition(vec![
            (source, PositionRelation::default()),
            (source, PositionRelation::whole()),
        ]);

        assert_eq!(
            ssa.root_source_relations(destination).get("source"),
            Some(&PositionRelation::whole())
        );
    }

    #[test]
    fn retained_live_on_entry_is_not_a_combinational_read() {
        let mut ssa = SsaStore::default();
        let retained = ssa.read("destination");
        let checkpoint = ssa.checkpoint();
        let assigned = ssa.definition(Vec::new());
        ssa.bind("destination", assigned);
        let branch = ssa.capture_and_rollback(checkpoint);

        ssa.merge(&[BranchState::unchanged(), branch]);

        let merged = ssa.read("destination");
        assert_ne!(merged, retained);
        assert!(ssa.root_sources(merged).is_empty());
    }

    #[test]
    fn weak_bind_retains_entry_until_a_later_explicit_read() {
        let mut ssa = SsaStore::<u8>::default();
        let replacement = ssa.definition(Vec::new());
        ssa.weak_bind(0, replacement);

        let retained = ssa.read(0);
        assert!(ssa.root_sources(retained).is_empty());

        let observed = ssa.definition(vec![retained]);
        let expected: HashSet<_> = [0].into_iter().collect();
        assert_eq!(ssa.root_sources(observed), expected);
    }

    #[test]
    fn rollback_discards_current_bindings_without_discarding_versions() {
        let mut ssa = SsaStore::default();
        let checkpoint = ssa.checkpoint();
        let source = ssa.read("source");
        let definition = ssa.definition(vec![source]);
        ssa.bind("destination", definition);

        let _ = ssa.capture_and_rollback(checkpoint);
        let restored = ssa.read("destination");

        let expected = ["source"].into_iter().collect::<HashSet<_>>();
        assert_eq!(ssa.root_sources(definition), expected);
        assert!(ssa.root_sources(restored).is_empty());
    }

    #[test]
    fn branch_state_contains_only_keys_changed_since_checkpoint() {
        let mut ssa = SsaStore::default();
        for key in 0..1_000 {
            let version = ssa.definition(Vec::new());
            ssa.bind(key, version);
        }

        let checkpoint = ssa.checkpoint();
        let version = ssa.definition(Vec::new());
        ssa.bind(500, version);
        let branch = ssa.capture_and_rollback(checkpoint);

        assert_eq!(branch.bindings.len(), 1);
        assert_eq!(branch.bindings[&500], version);
    }

    #[test]
    fn nested_rollback_preserves_the_outer_transaction() {
        let mut ssa = SsaStore::default();
        let base = ssa.definition(Vec::new());
        ssa.bind("outer", base);

        let outer_checkpoint = ssa.checkpoint();
        let outer_definition = ssa.definition(Vec::new());
        ssa.bind("outer", outer_definition);

        let inner_checkpoint = ssa.checkpoint();
        let inner_definition = ssa.definition(Vec::new());
        ssa.bind("inner", inner_definition);
        let inner_state = ssa.capture_and_rollback(inner_checkpoint);

        assert_eq!(ssa.read("outer"), outer_definition);
        assert_ne!(ssa.read("inner"), inner_definition);

        ssa.merge(&[BranchState::unchanged(), inner_state]);
        let merged_inner = ssa.read("inner");
        let outer_state = ssa.capture_and_rollback(outer_checkpoint);

        assert_eq!(ssa.read("outer"), base);
        assert_ne!(ssa.read("inner"), merged_inner);
        assert_eq!(outer_state.bindings["outer"], outer_definition);
        assert_eq!(outer_state.bindings["inner"], merged_inner);
    }

    #[test]
    fn merge_cost_tracks_sparse_bindings_not_branch_key_product() {
        let mut ssa = SsaStore::default();
        let mut states = Vec::new();
        for key in 0..10_000 {
            let version = ssa.definition(Vec::new());
            let mut bindings = HashMap::default();
            bindings.insert(key, version);
            states.push(BranchState { bindings });
        }

        ssa.merge(&states);

        assert_eq!(ssa.current.len(), states.len());
    }

    #[test]
    fn repeated_transfer_closes_dependencies_across_runtime_iterations() {
        let mut ssa = SsaStore::default();
        let checkpoint = ssa.checkpoint();

        let previous_middle = ssa.read("middle");
        let last = ssa.definition(vec![previous_middle]);
        ssa.weak_bind("last", last);

        let first = ssa.read("first");
        let middle = ssa.definition(vec![first]);
        ssa.weak_bind("middle", middle);

        let iteration = ssa.capture_and_rollback(checkpoint);
        ssa.close_repeated_transfer(&iteration, checkpoint, false, |_| None);

        let last = ssa.read("last");
        let sources = ssa.root_sources(last);
        assert!(sources.contains("first"));
    }

    #[test]
    fn repeated_transfer_shares_import_walks_without_enumerating_shift_paths() {
        const STAGES: usize = 18;
        const CALLS: usize = 64;
        let mut callee = SsaStore::default();
        let mut value = callee.read(0);
        for shift in 0..STAGES {
            value = callee.related_definition(vec![
                (value, PositionRelation::default()),
                (
                    value,
                    PositionRelation {
                        array: Some(0),
                        packed: Some(1isize << shift),
                    },
                ),
            ]);
        }
        let unrelated = callee.read(1);
        let constant = callee.definition(Vec::new());
        let graph = Rc::new(
            callee.dependency_dag(&[value, unrelated, constant], |key| [0, 1].contains(key)),
        );

        let mut caller = SsaStore::default();
        let actuals = (0..CALLS).map(|key| caller.read(key)).collect::<Vec<_>>();
        let unrelated = caller.read(CALLS);
        let checkpoint = caller.checkpoint();
        for (index, &actual) in actuals.iter().enumerate() {
            let output = caller.imported(
                graph.clone(),
                graph.roots[0],
                [
                    (0, vec![(actual, PositionRelation::default())]),
                    (1, vec![(unrelated, PositionRelation::default())]),
                ]
                .into_iter()
                .collect::<HashMap<_, _>>()
                .into(),
                Rc::default(),
            );
            caller.bind(CALLS + 1 + index, output);
        }
        // Neither an absent root nor a constant root reads any actual,
        // even when other outputs of the same summary do.
        for (index, root) in [None, graph.roots[2]].into_iter().enumerate() {
            let output = caller.imported(
                graph.clone(),
                root,
                [(1, vec![(unrelated, PositionRelation::default())])]
                    .into_iter()
                    .collect::<HashMap<_, _>>()
                    .into(),
                Rc::default(),
            );
            caller.bind(CALLS * 2 + 1 + index, output);
        }
        let iteration = caller.capture_and_rollback(checkpoint);
        ITERATION_IMPORT_VISITS.set(0);
        let before = caller.versions.len();
        caller.close_repeated_transfer(&iteration, checkpoint, false, |_| None);
        assert!(caller.versions.len() - before < CALLS * (STAGES + 8));
        assert!(
            ITERATION_IMPORT_VISITS.get() <= (CALLS + 1) * graph.nodes.len() + graph.edges.len(),
            "imports must index edges once and visit each selected node once per invocation: {}",
            ITERATION_IMPORT_VISITS.get(),
        );
        for index in 0..CALLS {
            let output = caller.read(CALLS + 1 + index);
            assert_eq!(
                caller.root_source_keys_guarded(output),
                vec![(index, PathCondition::default())]
            );
        }
        for index in 0..2 {
            let output = caller.read(CALLS * 2 + 1 + index);
            assert!(caller.root_sources(output).is_empty());
        }
    }

    #[test]
    fn opposite_arms_of_one_branch_are_incompatible() {
        let branch = BranchId::new(1, 0, 2);
        let true_path = PathCondition::default().with_choice(branch, 0);
        let false_path = PathCondition::default().with_choice(branch, 1);

        assert!(true_path.conjoin_if_compatible(&false_path).is_none());
    }

    #[test]
    fn arms_of_distinct_branches_are_compatible() {
        let first = PathCondition::default().with_choice(BranchId::new(1, 0, 2), 0);
        let second = PathCondition::default().with_choice(BranchId::new(1, 1, 2), 1);

        let combined = first
            .conjoin_if_compatible(&second)
            .expect("distinct branches can execute on the same path");
        assert!(first.covers(&combined));
        assert!(second.covers(&combined));
    }

    #[test]
    fn large_contiguous_arm_sets_remain_compact() {
        let branch = BranchId::new(1, 0, 1_000_001);
        let lower = PathCondition::default().with_choice_range(branch, 0, 500_000);
        let upper = PathCondition::default().with_choice_range(branch, 500_000, 1_000_001);

        assert_eq!(lower.disjoin(&upper), PathCondition::default());
        assert!(lower.conjoin_if_compatible(&upper).is_none());
    }

    #[test]
    fn fragmented_arm_joins_charge_accumulated_ranges() {
        let branch = BranchId::new(1, 0, 129);
        for stride in [1, 2] {
            let conditions = (0..64)
                .map(|arm| PathCondition::default().with_choice(branch, arm * stride))
                .collect::<Vec<_>>();
            assert_eq!(
                PathCondition::try_disjoin_all(&conditions, &mut 512).is_some(),
                stride == 1
            );
            // Retrying with enough work must retain exactly the allowed arms,
            // including the gaps that made the smaller budget insufficient.
            let joined = PathCondition::try_disjoin_all(&conditions, &mut 8192).unwrap();
            for arm in 0..branch.arms() {
                let choice = PathCondition::default().with_choice(branch, arm);
                assert_eq!(
                    joined.conjoin_if_compatible(&choice).is_some(),
                    arm < 64 * stride && arm % stride == 0
                );
            }
        }
    }

    #[test]
    fn fragmented_guards_bound_dependency_exports_and_imports() {
        let branch = BranchId::new(1, 0, 129);
        for stride in [1, 2] {
            let conditions = (0..64)
                .map(|arm| PathCondition::default().with_choice(branch, arm * stride))
                .collect::<Vec<_>>();
            let condition = PathCondition::try_disjoin_all(&conditions, &mut 8192).unwrap();
            let mut callee = SsaStore::default();
            let input = callee.read("input");
            let output = callee.definition_guarded(vec![input], &condition);
            assert_eq!(
                callee
                    .try_dependency_dag(&[output], |_| true, &mut 64)
                    .is_some(),
                stride == 1
            );
            let graph = Rc::new(callee.dependency_dag(&[output], |_| true));

            let mut caller = SsaStore::default();
            let actual = caller.read("actual");
            let imported = caller.imported(
                graph.clone(),
                graph.roots[0],
                [("input", vec![(actual, PositionRelation::default())])]
                    .into_iter()
                    .collect::<HashMap<_, _>>()
                    .into(),
                [(branch, BranchId::new(2, 0, branch.arms()))]
                    .into_iter()
                    .collect::<HashMap<_, _>>()
                    .into(),
            );
            for limit in [64, 256] {
                assert_eq!(
                    caller
                        .try_dependency_dag(&[imported], |_| true, &mut { limit })
                        .is_some(),
                    stride == 1 || limit == 256
                );
            }
        }
    }

    #[test]
    fn sequential_branch_joins_do_not_enumerate_path_combinations() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let mut value = source;
        for local in 0..128 {
            let branch = BranchId::new(1, local, 2);
            let left = ssa.definition_guarded(
                vec![value],
                &PathCondition::default().with_choice(branch, 0),
            );
            let right = ssa.definition_guarded(
                vec![value],
                &PathCondition::default().with_choice(branch, 1),
            );
            value = ssa.phi(vec![left, right]);
        }

        assert_eq!(
            ssa.root_source_relations_guarded(value),
            vec![(
                "source",
                PositionRelation::whole(),
                PathCondition::default()
            )]
        );
    }

    #[test]
    fn root_source_walk_does_not_use_the_native_stack() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let mut version = ssa.definition(vec![source]);
        for _ in 0..100_000 {
            version = ssa.definition(vec![version]);
        }
        let mut work = 0;
        assert_eq!(
            ssa.try_root_source_keys_guarded(version, &mut work),
            Some(vec![("source", PathCondition::default())])
        );
    }
    #[test]
    fn exact_condition_union_preserves_correlations() {
        let a = BranchId::new(1, 0, 2);
        let b = BranchId::new(1, 1, 2);
        let left = PathCondition::default().with_choice(a, 0).with_choice(b, 0);
        let right = PathCondition::default().with_choice(a, 1).with_choice(b, 0);
        assert_eq!(
            left.disjoin_exact(&right),
            Some(PathCondition::default().with_choice(b, 0))
        );
        let correlated = PathCondition::default().with_choice(a, 1).with_choice(b, 1);
        assert_eq!(left.disjoin_exact(&correlated), None);
        assert_eq!(
            left.disjoin_exact(&PathCondition::default()),
            Some(PathCondition::default())
        );
    }

    #[test]
    fn whole_source_query_does_not_enumerate_imported_shift_paths() {
        let mut callee = SsaStore::default();
        let mut value = callee.read("input");
        for shift in 0..32 {
            value = callee.related_definition(vec![
                (value, PositionRelation::default()),
                (
                    value,
                    PositionRelation {
                        array: Some(0),
                        packed: Some(1isize << shift),
                    },
                ),
            ]);
        }
        let graph = Rc::new(callee.dependency_dag(&[value], |key| *key == "input"));
        let mut caller = SsaStore::default();
        let input = caller.read("actual");
        let root = caller.imported(
            graph.clone(),
            graph.roots[0],
            [("input", vec![(input, PositionRelation::default())])]
                .into_iter()
                .collect::<HashMap<_, _>>()
                .into(),
            Rc::default(),
        );
        reset_source_walk_visits();
        assert_eq!(
            caller.root_source_keys_guarded(root),
            vec![("actual", PathCondition::default())]
        );
        assert!(source_walk_visits() < 100);
    }

    #[test]
    fn imported_long_chain_uses_an_incoming_edge_index() {
        let mut callee = SsaStore::default();
        let mut value = callee.read("input");
        for _ in 0..20_000 {
            value = callee.definition(vec![value]);
        }
        let graph = Rc::new(callee.dependency_dag(&[value], |key| *key == "input"));
        let mut caller = SsaStore::default();
        let input = caller.read("actual");
        let root = caller.imported(
            graph.clone(),
            graph.roots[0],
            [("input", vec![(input, PositionRelation::default())])]
                .into_iter()
                .collect::<HashMap<_, _>>()
                .into(),
            Rc::default(),
        );
        let result = caller.dependency_dag(&[root], |key| *key == "actual");
        // The callee's external identity is an alias of the actual input.
        assert_eq!(result.nodes.len(), graph.nodes.len());
        assert_eq!(result.edges.len(), graph.edges.len());
        assert!(result.roots[0].is_some());
    }
}
