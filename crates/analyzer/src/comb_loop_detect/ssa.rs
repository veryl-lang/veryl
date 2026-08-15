//! IR-independent statement-ordered SSA state.

use crate::{HashMap, HashSet};
use std::collections::VecDeque;
use std::hash::Hash;
use std::rc::Rc;

pub(super) type VersionId = usize;

type SourceMap<K> = HashMap<(K, PositionRelation), PathCondition>;

pub(super) struct SourceCache<K> {
    summaries: HashMap<(VersionId, bool), Rc<SourceMap<K>>>,
    allowed: Option<HashSet<K>>,
}

impl<K> Default for SourceCache<K> {
    fn default() -> Self {
        Self {
            summaries: HashMap::default(),
            allowed: None,
        }
    }
}

impl<K> SourceCache<K>
where
    K: Eq + Hash,
{
    pub(super) fn restricted(allowed: impl IntoIterator<Item = K>) -> Self {
        Self {
            summaries: HashMap::default(),
            allowed: Some(allowed.into_iter().collect()),
        }
    }
}

#[derive(Clone)]
enum Version<K> {
    Entry(K),
    Definition {
        positional: Vec<(VersionId, PositionRelation)>,
        whole: Vec<VersionId>,
        condition: PathCondition,
    },
    Phi(Vec<VersionId>),
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
    allowed: Vec<usize>,
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
    pub(super) fn with_choice(&self, branch: BranchId, arm: usize) -> Self {
        let mut constraints = self.constraints.as_ref().clone();
        let constraint = BranchConstraint {
            branch,
            allowed: vec![arm],
        };
        match constraints.binary_search_by_key(&branch, |constraint| constraint.branch) {
            Ok(index) => constraints[index] = constraint,
            Err(index) => constraints.insert(index, constraint),
        }
        Self {
            constraints: Rc::new(constraints),
        }
    }

    pub(super) fn intersection<'a>(conditions: impl IntoIterator<Item = &'a Self>) -> Self {
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

    pub(super) fn union_if_compatible(&self, other: &Self) -> Option<Self> {
        let mut constraints = Vec::with_capacity(self.constraints.len() + other.constraints.len());
        let mut left = self.constraints.iter().peekable();
        let mut right = other.constraints.iter().peekable();
        loop {
            match (left.peek(), right.peek()) {
                (Some(a), Some(b)) if a.branch == b.branch => {
                    let allowed = a
                        .allowed
                        .iter()
                        .copied()
                        .filter(|arm| b.allowed.binary_search(arm).is_ok())
                        .collect::<Vec<_>>();
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

    pub(super) fn is_subset_of(&self, other: &Self) -> bool {
        self.constraints.iter().all(|constraint| {
            other
                .constraints
                .binary_search_by_key(&constraint.branch, |other| other.branch)
                .ok()
                .is_some_and(|index| {
                    other.constraints[index]
                        .allowed
                        .iter()
                        .all(|arm| constraint.allowed.binary_search(arm).is_ok())
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

    pub(super) fn disjoin(&self, other: &Self) -> Self {
        let mut constraints = Vec::new();
        for constraint in self.constraints.iter() {
            let Ok(index) = other
                .constraints
                .binary_search_by_key(&constraint.branch, |other| other.branch)
            else {
                continue;
            };
            let mut allowed = constraint.allowed.clone();
            allowed.extend_from_slice(&other.constraints[index].allowed);
            allowed.sort_unstable();
            allowed.dedup();
            if allowed.len() != constraint.branch.arms {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct PositionRelation {
    pub(super) array: Option<isize>,
    pub(super) packed: Option<isize>,
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

    pub(super) fn reversed(self) -> Self {
        Self {
            array: self.array.and_then(isize::checked_neg),
            packed: self.packed.and_then(isize::checked_neg),
        }
    }

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
    left?.checked_add(right?)
}

#[derive(Clone, Copy)]
pub(super) struct Checkpoint {
    undo_start: usize,
    depth: usize,
}

pub(super) struct BranchState<K> {
    bindings: HashMap<K, VersionId>,
}

impl<K> BranchState<K> {
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
}

impl<K> Default for SsaStore<K> {
    fn default() -> Self {
        Self {
            versions: Vec::new(),
            entries: HashMap::default(),
            current: HashMap::default(),
            undo: Vec::new(),
            checkpoints: Vec::new(),
        }
    }
}

impl<K> SsaStore<K>
where
    K: Copy + Eq + Hash,
{
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
        mut sources: Vec<VersionId>,
        condition: &PathCondition,
    ) -> VersionId {
        sources.sort_unstable();
        sources.dedup();
        let version = self.versions.len();
        self.versions.push(Version::Definition {
            positional: Vec::new(),
            whole: sources,
            condition: condition.clone(),
        });
        version
    }

    pub(super) fn positional_definition(
        &mut self,
        positional: Vec<(VersionId, PositionRelation)>,
        whole: Vec<VersionId>,
    ) -> VersionId {
        self.positional_definition_guarded(positional, whole, &PathCondition::default())
    }

    pub(super) fn positional_definition_guarded(
        &mut self,
        mut positional: Vec<(VersionId, PositionRelation)>,
        mut whole: Vec<VersionId>,
        condition: &PathCondition,
    ) -> VersionId {
        positional.sort_unstable();
        let mut merged: Vec<(VersionId, PositionRelation)> = Vec::with_capacity(positional.len());
        for (source, relation) in positional {
            if let Some((previous_source, previous_relation)) = merged.last_mut()
                && *previous_source == source
            {
                *previous_relation = previous_relation.union(relation);
            } else {
                merged.push((source, relation));
            }
        }
        let mut positional = merged;
        whole.sort_unstable();
        whole.dedup();
        positional.retain(|(source, _)| whole.binary_search(source).is_err());
        let version = self.versions.len();
        self.versions.push(Version::Definition {
            positional,
            whole,
            condition: condition.clone(),
        });
        version
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
        let start = (version, include_entry, PositionRelation::default());
        let mut reached = HashMap::default();
        reached.insert(start, PathCondition::default());
        let mut queued = HashSet::default();
        queued.insert(start);
        let mut queue = VecDeque::from([start]);

        while let Some(state @ (current, include_entry, relation)) = queue.pop_front() {
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
                    if include_entry
                        && cache
                            .allowed
                            .as_ref()
                            .is_none_or(|allowed| allowed.contains(key))
                    {
                        merge_source(&mut sources, (*key, relation), condition);
                    }
                }
                Version::Definition {
                    positional,
                    whole,
                    condition: definition_condition,
                } => {
                    let Some(condition) = condition.union_if_compatible(definition_condition)
                    else {
                        continue;
                    };
                    for (input, offset) in positional {
                        enqueue((*input, true, relation.compose(*offset)), condition.clone());
                    }
                    for input in whole {
                        enqueue((*input, true, PositionRelation::whole()), condition.clone());
                    }
                }
                Version::Phi(inputs) => {
                    for input in inputs {
                        enqueue((*input, include_entry, relation), condition.clone());
                    }
                }
            }
        }
        let sources = Rc::new(sources);
        cache.summaries.insert(cache_key, sources.clone());
        sources
    }
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
            let Some(condition) = condition.union_if_compatible(guard) else {
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
    fn positional_definition_preserves_source_relation() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination =
            ssa.positional_definition(vec![(source, PositionRelation::default())], Vec::new());

        assert_eq!(
            ssa.root_source_relations(destination).get("source"),
            Some(&PositionRelation::default())
        );
    }

    #[test]
    fn positional_offsets_compose_through_definitions() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let first = ssa.positional_definition(
            vec![(
                source,
                PositionRelation {
                    array: Some(3),
                    packed: Some(-2),
                },
            )],
            Vec::new(),
        );
        let destination = ssa.positional_definition(
            vec![(
                first,
                PositionRelation {
                    array: Some(-1),
                    packed: Some(5),
                },
            )],
            Vec::new(),
        );

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
        let destination = ssa.positional_definition(
            vec![
                (source, PositionRelation::default()),
                (
                    source,
                    PositionRelation {
                        array: Some(0),
                        packed: Some(1),
                    },
                ),
            ],
            Vec::new(),
        );

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
        let destination =
            ssa.positional_definition(vec![(source, PositionRelation::default())], vec![source]);

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
    fn opposite_arms_of_one_branch_are_incompatible() {
        let branch = BranchId::new(1, 0, 2);
        let true_path = PathCondition::default().with_choice(branch, 0);
        let false_path = PathCondition::default().with_choice(branch, 1);

        assert!(true_path.union_if_compatible(&false_path).is_none());
    }

    #[test]
    fn arms_of_distinct_branches_are_compatible() {
        let first = PathCondition::default().with_choice(BranchId::new(1, 0, 2), 0);
        let second = PathCondition::default().with_choice(BranchId::new(1, 1, 2), 1);

        let combined = first
            .union_if_compatible(&second)
            .expect("distinct branches can execute on the same path");
        assert!(first.is_subset_of(&combined));
        assert!(second.is_subset_of(&combined));
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
}
