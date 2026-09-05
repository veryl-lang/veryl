//! IR-independent statement-ordered SSA state.

mod dag;
mod repeated;

use crate::{HashMap, HashSet};
use std::collections::VecDeque;
use std::hash::Hash;
use std::rc::Rc;
use veryl_parser::token_range::TokenRange;

#[cfg(test)]
thread_local! {
    static SOURCE_WALK_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ITERATION_IMPORT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
        stride: isize,
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
    pub(super) fn disjoin_all<'a>(conditions: impl IntoIterator<Item = &'a Self>) -> Self {
        let mut conditions = conditions.into_iter();
        let Some(first) = conditions.next() else {
            return Self::default();
        };
        let mut combined = first.clone();
        for condition in conditions {
            combined = combined.disjoin(condition);
        }
        combined
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

#[derive(Clone)]
pub(super) enum DependencyDagNode<K> {
    External(K),
    Internal,
    /// Zero or more positive packed translations within this node's domain.
    /// Kept as an operation in the DAG; only the circuit graph adds a self edge.
    Replicated {
        stride: isize,
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
    #[cfg(test)]
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
    entries: HashMap<K, VersionId>,
    current: HashMap<K, VersionId>,
    undo: Vec<Undo<K>>,
    checkpoints: Vec<usize>,
    sites: HashMap<VersionId, DefinitionSite<VersionId>>,
}

impl<K> Default for SsaStore<K> {
    fn default() -> Self {
        Self {
            versions: Vec::new(),
            entries: HashMap::default(),
            current: HashMap::default(),
            undo: Vec::new(),
            checkpoints: Vec::new(),
            sites: HashMap::default(),
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
        bindings: HashMap<K, Vec<(VersionId, PositionRelation)>>,
        branches: HashMap<BranchId, BranchId>,
    ) -> VersionId {
        let version = self.versions.len();
        self.versions.push(Version::Imported {
            graph,
            root,
            bindings: Rc::new(bindings),
            branches: Rc::new(branches),
        });
        version
    }

    pub(super) fn projected(&mut self, source: VersionId, domain: PositionDomain) -> VersionId {
        let version = self.versions.len();
        self.versions.push(Version::Projected { source, domain });
        version
    }

    pub(super) fn replicated(
        &mut self,
        source: VersionId,
        domain: PositionDomain,
        stride: isize,
    ) -> VersionId {
        assert!(stride > 0, "replication must advance the packed position");
        let version = self.versions.len();
        self.versions.push(Version::Replicated {
            source,
            domain,
            stride,
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
                | Version::Replicated { .. } => return true,
                Version::Definition { sources, .. } => {
                    queue.extend(sources.iter().map(|(source, _)| *source));
                }
                Version::Phi(inputs) => queue.extend(inputs.iter().copied()),
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
            let version = self.phi(inputs);
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
    pub(super) fn close_repeated_transfer(
        &mut self,
        single_iteration: &BranchState<K>,
        iteration_checkpoint: Checkpoint,
        may_skip: bool,
        domain: impl Fn(K) -> Option<PositionDomain>,
    ) {
        repeated::close(
            self,
            single_iteration,
            iteration_checkpoint,
            may_skip,
            domain,
        );
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

    pub(super) fn root_source_relations_guarded(
        &self,
        version: VersionId,
    ) -> Vec<(K, PositionRelation, PathCondition)> {
        self.root_source_relations_guarded_cached(version, &mut SourceCache::default())
    }

    /// Whole-value reads need source identities and guards, not every possible
    /// sum of shifts through an imported DAG. Forget positions before walking.
    pub(super) fn root_source_keys_guarded(&self, version: VersionId) -> Vec<(K, PathCondition)> {
        let mut cache = SourceCache {
            ignore_position: true,
            ..SourceCache::default()
        };
        self.root_source_relations_guarded_cached(version, &mut cache)
            .into_iter()
            .map(|(key, _, condition)| (key, condition))
            .collect()
    }

    pub(super) fn root_source_relations_guarded_cached(
        &self,
        version: VersionId,
        cache: &mut SourceCache<K>,
    ) -> Vec<(K, PositionRelation, PathCondition)> {
        // SSA versions form a DAG. Summarize each (version, relation) once and
        // combine branch alternatives at the join instead of re-walking the
        // same suffix for every feasible path.
        let sources = self.source_summary(version, false, cache);
        sources
            .iter()
            .map(|(&(source, relation), condition)| (source, relation, condition.clone()))
            .collect()
    }

    pub(super) fn dependency_dag(
        &self,
        roots: &[VersionId],
        allowed: &HashSet<K>,
    ) -> DependencyDag<K>
    where
        K: Ord,
    {
        self.try_dependency_dag(roots, allowed, usize::MAX)
            .expect("unlimited dependency export")
    }

    pub(super) fn try_dependency_dag(
        &self,
        roots: &[VersionId],
        allowed: &HashSet<K>,
        mut work: usize,
    ) -> Option<DependencyDag<K>>
    where
        K: Ord,
    {
        let mut states = HashSet::default();
        let mut queue = VecDeque::new();
        for &root in roots {
            if states.insert((root, false)) {
                queue.push_back((root, false));
            }
        }
        while let Some((version, include_entry)) = queue.pop_front() {
            work = work.checked_sub(1)?;
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
                Version::Imported { bindings, .. } => {
                    for sources in bindings.values() {
                        for (source, _) in sources {
                            enqueue((*source, true));
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
        type ImportKey<K> = (
            usize,
            Option<usize>,
            Vec<(K, Vec<(usize, PositionRelation)>)>,
            Vec<(BranchId, BranchId)>,
        );
        let mut imports: HashMap<ImportKey<K>, Option<usize>> = HashMap::default();

        let mut ordered = states.into_iter().collect::<Vec<_>>();
        ordered.sort_unstable();
        for state @ (version, include_entry) in ordered {
            work = work.checked_sub(1)?;
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
                    (include_entry && allowed.contains(key)).then(|| builder.external(*key))
                }
                Version::Definition { sources, condition } => {
                    work = work.checked_sub(
                        sources
                            .len()
                            .saturating_mul(condition.branch_count().saturating_add(1)),
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
                Version::Imported {
                    graph,
                    root,
                    bindings,
                    branches,
                } => {
                    let mut mapped_bindings = bindings
                        .iter()
                        .map(|(&key, sources)| {
                            let mut sources = sources
                                .iter()
                                .filter_map(|(source, relation)| {
                                    mapped[&(*source, true)].map(|source| (source, *relation))
                                })
                                .collect::<Vec<_>>();
                            sources.sort_unstable();
                            sources.dedup();
                            (key, sources)
                        })
                        .collect::<Vec<_>>();
                    mapped_bindings.sort_unstable_by_key(|(key, _)| *key);
                    let mut mapped_branches = branches
                        .iter()
                        .map(|(&source, &destination)| (source, destination))
                        .collect::<Vec<_>>();
                    mapped_branches.sort_unstable();
                    let key = (
                        Rc::as_ptr(graph) as usize,
                        *root,
                        mapped_bindings.clone(),
                        mapped_branches,
                    );
                    if let Some(node) = imports.get(&key) {
                        *node
                    } else {
                        let node = inline_dependency_dag(
                            graph,
                            *root,
                            &mapped_bindings.into_iter().collect(),
                            branches,
                            &mut builder,
                            &mut work,
                        )?;
                        imports.insert(key, node);
                        node
                    }
                }
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
                    stride,
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
                    Some(builder.replicated(inputs, vec![*domain], site, *stride))
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
    ) -> Rc<SourceMap<K>> {
        let cache_key = (version, include_entry);
        if let Some(sources) = cache.summaries.get(&cache_key) {
            return sources.clone();
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
                merge_source_summaries(&mut sources, cached, Some(&condition), Some(relation));
                continue;
            }

            let mut enqueue = |next: (VersionId, bool, PositionRelation),
                               condition: PathCondition| {
                let changed = if let Some(existing) = reached.get_mut(&next) {
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
            };

            match &self.versions[current] {
                Version::Entry(key) => {
                    if include_entry {
                        merge_source(&mut sources, (*key, relation), condition);
                    }
                }
                Version::Definition {
                    sources,
                    condition: definition_condition,
                } => {
                    let Some(condition) = condition.conjoin_if_compatible(definition_condition)
                    else {
                        continue;
                    };
                    for (input, offset) in sources {
                        enqueue((*input, true, relation.compose(*offset)), condition.clone());
                    }
                }
                Version::Phi(inputs) => {
                    for input in inputs {
                        enqueue((*input, include_entry, relation), condition.clone());
                    }
                }
                Version::Imported {
                    graph,
                    root,
                    bindings,
                    branches,
                } => {
                    for (key, imported_relation, imported_condition) in
                        dependency_dag_external_sources(graph, *root, initial_relation)
                    {
                        let imported_condition = imported_condition.remapped(branches);
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
                            );
                        }
                    }
                }
                Version::Projected { source, .. } => {
                    enqueue((*source, true, relation), condition);
                }
                Version::Replicated { source, .. } => {
                    // Scalar source queries cannot represent periodic positions.
                    // Exact positional consumers retain the structural operation.
                    enqueue(
                        (
                            *source,
                            true,
                            PositionRelation {
                                packed: None,
                                ..relation
                            },
                        ),
                        condition,
                    );
                }
            }
        }
        let sources = Rc::new(sources);
        cache.summaries.insert(cache_key, sources.clone());
        sources
    }
}

fn dependency_dag_external_sources<K>(
    graph: &DependencyDag<K>,
    root: Option<usize>,
    initial_relation: PositionRelation,
) -> Vec<(K, PositionRelation, PathCondition)>
where
    K: Copy + Eq + Hash,
{
    let Some(root) = root else {
        return Vec::new();
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
            merge_source(&mut sources, (key, relation), condition);
            continue;
        }
        let relation = if matches!(graph.nodes[node], DependencyDagNode::Replicated { .. }) {
            PositionRelation {
                packed: None,
                ..relation
            }
        } else {
            relation
        };
        for edge in incoming.get(&node).into_iter().flatten() {
            let Some(next_condition) = condition.conjoin_if_compatible(&edge.condition) else {
                continue;
            };
            let next = (edge.source, relation.compose(edge.relation));
            let changed = if let Some(existing) = reached.get_mut(&next) {
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
    sources
        .into_iter()
        .map(|((key, relation), condition)| (key, relation, condition))
        .collect()
}

fn inline_dependency_dag<K>(
    graph: &DependencyDag<K>,
    root: Option<usize>,
    bindings: &HashMap<K, Vec<(usize, PositionRelation)>>,
    branches: &HashMap<BranchId, BranchId>,
    builder: &mut dag::Builder<K>,
    work: &mut usize,
) -> Option<Option<usize>>
where
    K: Copy + Eq + Hash,
{
    let Some(root) = root else {
        return Some(None);
    };
    // Charge the existing child before allocating an import. Every distinct
    // invocation can require distinct guards; cap that expansion separately
    // from positional cycle search, including branch-condition payloads.
    let cost = graph.edges.iter().fold(graph.nodes.len(), |cost, edge| {
        cost.saturating_add(edge.condition.branch_count().saturating_add(1))
    });
    *work = work.checked_sub(cost)?;
    let mut incoming = vec![Vec::new(); graph.nodes.len()];
    for edge in &graph.edges {
        incoming[edge.destination].push(edge);
    }
    let mut retained = HashSet::default();
    let mut queue = VecDeque::from([root]);
    retained.insert(root);
    while let Some(node) = queue.pop_front() {
        for edge in &incoming[node] {
            if retained.insert(edge.source) {
                queue.push_back(edge.source);
            }
        }
    }

    // Exported nodes are topologically ordered. Intern each child after its
    // inputs are mapped so equivalent conversions and imported subgraphs use
    // the same parent nodes, including when the actual bindings differ.
    let mut mapped: HashMap<usize, usize> = HashMap::default();
    for (child, child_node) in graph.nodes.iter().enumerate() {
        if !retained.contains(&child) {
            continue;
        }
        let mut inputs = incoming[child]
            .iter()
            .map(|edge| {
                debug_assert!(edge.source < child);
                (
                    mapped[&edge.source],
                    edge.relation,
                    edge.condition.remapped(branches),
                )
            })
            .collect::<Vec<_>>();
        if let DependencyDagNode::External(key) = child_node {
            inputs.extend(
                bindings
                    .get(key)
                    .into_iter()
                    .flatten()
                    .map(|&(source, relation)| (source, relation, PathCondition::default())),
            );
        }
        let site = graph.sites.get(&child).map(|site| DefinitionSite {
            token: site.token,
            data_inputs: site
                .data_inputs
                .iter()
                .filter_map(|input| mapped.get(input).copied())
                .collect(),
        });
        let node = if let DependencyDagNode::Replicated { stride } = child_node {
            builder.replicated(inputs, graph.domains[child].clone(), site, *stride)
        } else {
            builder.internal(inputs, graph.domains[child].clone(), site)
        };
        mapped.insert(child, node);
    }
    Some(mapped.get(&root).copied())
}

fn merge_source<K>(
    destination: &mut SourceMap<K>,
    key: (K, PositionRelation),
    condition: PathCondition,
) where
    K: Copy + Eq + Hash,
{
    destination
        .entry(key)
        .and_modify(|existing| *existing = existing.disjoin(&condition))
        .or_insert(condition);
}

fn merge_source_summaries<K>(
    destination: &mut SourceMap<K>,
    sources: &SourceMap<K>,
    guard: Option<&PathCondition>,
    prefix: Option<PositionRelation>,
) where
    K: Copy + Eq + Hash,
{
    for (&(source, relation), condition) in sources {
        let key = (
            source,
            prefix.map_or(relation, |prefix| prefix.compose(relation)),
        );
        let condition = if let Some(guard) = guard {
            let Some(condition) = condition.conjoin_if_compatible(guard) else {
                continue;
            };
            condition
        } else {
            condition.clone()
        };
        merge_source(destination, key, condition);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            callee.dependency_dag(&[value, unrelated, constant], &[0, 1].into_iter().collect()),
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
                .collect(),
                HashMap::default(),
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
                    .collect(),
                HashMap::default(),
            );
            caller.bind(CALLS * 2 + 1 + index, output);
        }
        let iteration = caller.capture_and_rollback(checkpoint);
        ITERATION_IMPORT_VISITS.set(0);
        let before = caller.versions.len();
        caller.close_repeated_transfer(&iteration, checkpoint, false, |_| None);
        assert!(caller.versions.len() - before < CALLS * (STAGES + 8));
        assert!(
            ITERATION_IMPORT_VISITS.get() <= graph.nodes.len() + graph.edges.len(),
            "shared imports must index edges once and visit each selected node once: {}",
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
        assert_eq!(ssa.root_sources(version), ["source"].into_iter().collect());
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
        let graph = Rc::new(callee.dependency_dag(&[value], &["input"].into_iter().collect()));
        let mut caller = SsaStore::default();
        let input = caller.read("actual");
        let root = caller.imported(
            graph.clone(),
            graph.roots[0],
            [("input", vec![(input, PositionRelation::default())])]
                .into_iter()
                .collect(),
            HashMap::default(),
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
        let graph = Rc::new(callee.dependency_dag(&[value], &["input"].into_iter().collect()));
        let mut caller = SsaStore::default();
        let input = caller.read("actual");
        let root = caller.imported(
            graph.clone(),
            graph.roots[0],
            [("input", vec![(input, PositionRelation::default())])]
                .into_iter()
                .collect(),
            HashMap::default(),
        );
        let result = caller.dependency_dag(&[root], &["actual"].into_iter().collect());
        // The callee's external identity is an alias of the actual input.
        assert_eq!(result.nodes.len(), graph.nodes.len());
        assert_eq!(result.edges.len(), graph.edges.len());
        assert!(result.roots[0].is_some());
    }
}
