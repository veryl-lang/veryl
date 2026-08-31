//! IR-independent statement-ordered SSA state.

use crate::{HashMap, HashSet};
use std::hash::Hash;

pub(super) type VersionId = usize;

#[derive(Clone)]
enum Version<K> {
    Entry(K),
    Definition {
        positional: Vec<(VersionId, PositionRelation)>,
        whole: Vec<VersionId>,
    },
    Phi(Vec<VersionId>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct PositionRelation {
    pub(super) array: Option<isize>,
    pub(super) packed: Option<isize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PositionOverflow;

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

    pub(super) fn compose(self, other: Self) -> Result<Self, PositionOverflow> {
        Ok(Self {
            array: compose_axis(self.array, other.array)?,
            packed: compose_axis(self.packed, other.packed)?,
        })
    }

    pub(super) fn reversed(self) -> Result<Self, PositionOverflow> {
        Ok(Self {
            array: reverse_axis(self.array)?,
            packed: reverse_axis(self.packed)?,
        })
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

fn compose_axis(
    left: Option<isize>,
    right: Option<isize>,
) -> Result<Option<isize>, PositionOverflow> {
    match (left, right) {
        (Some(left), Some(right)) => left.checked_add(right).map(Some).ok_or(PositionOverflow),
        _ => Ok(None),
    }
}

fn reverse_axis(value: Option<isize>) -> Result<Option<isize>, PositionOverflow> {
    value
        .map(|value| value.checked_neg().ok_or(PositionOverflow))
        .transpose()
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

    pub(super) fn definition(&mut self, mut sources: Vec<VersionId>) -> VersionId {
        sources.sort_unstable();
        sources.dedup();
        let version = self.versions.len();
        self.versions.push(Version::Definition {
            positional: Vec::new(),
            whole: sources,
        });
        version
    }

    pub(super) fn positional_definition(
        &mut self,
        mut positional: Vec<(VersionId, PositionRelation)>,
        mut whole: Vec<VersionId>,
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
        self.versions
            .push(Version::Definition { positional, whole });
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

    pub(super) fn merge(&mut self, states: &[BranchState<K>]) {
        let mut inputs_by_key: HashMap<K, (Vec<VersionId>, usize)> = HashMap::default();
        for state in states {
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
            if bound_branches < states.len() {
                inputs.push(fallback);
            }
            let version = self.phi(inputs);
            self.bind(key, version);
        }
    }

    pub(super) fn root_sources(&self, version: VersionId) -> Result<HashSet<K>, PositionOverflow> {
        Ok(self.root_source_relations(version)?.into_keys().collect())
    }

    pub(super) fn root_source_relations(
        &self,
        version: VersionId,
    ) -> Result<HashMap<K, PositionRelation>, PositionOverflow> {
        let mut sources: HashMap<K, PositionRelation> = HashMap::default();
        let mut visited = HashSet::default();
        let mut pending = Vec::new();
        match &self.versions[version] {
            // A final LiveOnEntry value is retained state, not a combinational
            // read. Entry versions reached through an explicit definition are.
            Version::Entry(_) => {}
            Version::Definition { positional, whole } => {
                pending.extend(
                    positional
                        .iter()
                        .map(|(input, relation)| (*input, true, *relation)),
                );
                pending.extend(
                    whole
                        .iter()
                        .map(|input| (*input, true, PositionRelation::whole())),
                );
            }
            Version::Phi(inputs) => {
                pending.extend(
                    inputs
                        .iter()
                        .map(|input| (*input, false, PositionRelation::default())),
                );
            }
        }

        while let Some((version, include_entry, relation)) = pending.pop() {
            if !visited.insert((version, include_entry, relation)) {
                continue;
            }
            match &self.versions[version] {
                Version::Entry(key) => {
                    if include_entry {
                        sources
                            .entry(*key)
                            .and_modify(|existing| *existing = existing.union(relation))
                            .or_insert(relation);
                    }
                }
                Version::Definition { positional, whole } => {
                    for (input, inner) in positional {
                        pending.push((*input, true, relation.compose(*inner)?));
                    }
                    pending.extend(
                        whole
                            .iter()
                            .map(|input| (*input, true, PositionRelation::whole())),
                    );
                }
                Version::Phi(inputs) => {
                    pending.extend(inputs.iter().map(|input| (*input, include_entry, relation)));
                }
            }
        }
        Ok(sources)
    }

    /// Returns every SSA version on a path from `version` to `source`.
    ///
    /// This is intentionally separate from `root_source_relations`: normal
    /// dependency analysis only needs the roots, while diagnostics may replay
    /// the defining chain after a loop has been found. Keeping this walk lazy
    /// avoids retaining a provenance vector for every dependency edge.
    pub(super) fn source_witness_versions(&self, version: VersionId, source: K) -> Vec<VersionId> {
        type State = (VersionId, bool);

        let start = (version, false);
        let mut pending = vec![start];
        let mut visited = HashSet::default();
        let mut parents: HashMap<State, Vec<State>> = HashMap::default();
        let mut matches = Vec::new();

        while let Some(state @ (version, include_entry)) = pending.pop() {
            if !visited.insert(state) {
                continue;
            }
            match &self.versions[version] {
                Version::Entry(key) => {
                    if include_entry && *key == source {
                        matches.push(state);
                    }
                }
                Version::Definition { positional, whole } => {
                    for input in positional.iter().map(|(input, _)| input).chain(whole) {
                        let child = (*input, true);
                        parents.entry(child).or_default().push(state);
                        pending.push(child);
                    }
                }
                Version::Phi(inputs) => {
                    for input in inputs {
                        let child = (*input, include_entry);
                        parents.entry(child).or_default().push(state);
                        pending.push(child);
                    }
                }
            }
        }

        // Walk back from every matching entry. This retains definitions on
        // all valid witness branches without cloning a growing path at every
        // SSA node.
        let mut witness_states = HashSet::default();
        let mut witness = HashSet::default();
        let mut pending = matches;
        while let Some(state) = pending.pop() {
            if !witness_states.insert(state) {
                continue;
            }
            witness.insert(state.0);
            pending.extend(parents.get(&state).into_iter().flatten().copied());
        }
        let mut witness = witness.into_iter().collect::<Vec<_>>();
        witness.sort_unstable();
        witness
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
        assert_eq!(ssa.root_sources(destination).unwrap(), expected);
    }

    #[test]
    fn root_source_walk_does_not_use_the_native_stack() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let mut version = ssa.definition(vec![source]);
        for _ in 0..100_000 {
            version = ssa.definition(vec![version]);
        }

        let expected = ["source"].into_iter().collect::<HashSet<_>>();
        assert_eq!(ssa.root_sources(version).unwrap(), expected);
    }

    #[test]
    fn witness_walk_ignores_an_unobserved_entry() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");

        assert!(ssa.source_witness_versions(source, "source").is_empty());
    }

    #[test]
    fn witness_walk_excludes_unrelated_phi_branches() {
        let mut ssa = SsaStore::default();
        let source_entry = ssa.read("source");
        let source_definition = ssa.definition(vec![source_entry]);
        let other_entry = ssa.read("other");
        let other_definition = ssa.definition(vec![other_entry]);
        let phi = ssa.phi(vec![source_definition, other_definition]);
        let destination = ssa.definition(vec![phi]);

        let witness = ssa.source_witness_versions(destination, "source");
        assert!(witness.contains(&source_entry));
        assert!(witness.contains(&source_definition));
        assert!(witness.contains(&phi));
        assert!(witness.contains(&destination));
        assert!(!witness.contains(&other_entry));
        assert!(!witness.contains(&other_definition));
    }

    #[test]
    fn witness_walk_does_not_use_the_native_stack() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let mut version = ssa.definition(vec![source]);
        for _ in 0..100_000 {
            version = ssa.definition(vec![version]);
        }

        let witness = ssa.source_witness_versions(version, "source");
        assert!(witness.contains(&source));
        assert!(witness.contains(&version));
    }

    #[test]
    fn positional_definition_preserves_source_relation() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination =
            ssa.positional_definition(vec![(source, PositionRelation::default())], Vec::new());

        assert_eq!(
            ssa.root_source_relations(destination)
                .unwrap()
                .get("source"),
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
            ssa.root_source_relations(destination)
                .unwrap()
                .get("source"),
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
            ssa.root_source_relations(destination)
                .unwrap()
                .get("source"),
            Some(&PositionRelation {
                array: Some(0),
                packed: None,
            })
        );
    }

    #[test]
    fn positional_offset_overflow_is_reported() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let first = ssa.positional_definition(
            vec![(
                source,
                PositionRelation {
                    array: Some(isize::MAX),
                    packed: Some(0),
                },
            )],
            Vec::new(),
        );
        let destination = ssa.positional_definition(
            vec![(
                first,
                PositionRelation {
                    array: Some(1),
                    packed: Some(0),
                },
            )],
            Vec::new(),
        );

        assert_eq!(
            ssa.root_source_relations(destination),
            Err(PositionOverflow)
        );
    }

    #[test]
    fn whole_dependency_dominates_a_positional_path() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination =
            ssa.positional_definition(vec![(source, PositionRelation::default())], vec![source]);

        assert_eq!(
            ssa.root_source_relations(destination)
                .unwrap()
                .get("source"),
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
        assert!(ssa.root_sources(merged).unwrap().is_empty());
    }

    #[test]
    fn weak_bind_retains_entry_until_a_later_explicit_read() {
        let mut ssa = SsaStore::<u8>::default();
        let replacement = ssa.definition(Vec::new());
        ssa.weak_bind(0, replacement);

        let retained = ssa.read(0);
        assert!(ssa.root_sources(retained).unwrap().is_empty());

        let observed = ssa.definition(vec![retained]);
        let expected: HashSet<_> = [0].into_iter().collect();
        assert_eq!(ssa.root_sources(observed).unwrap(), expected);
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
        assert_eq!(ssa.root_sources(definition).unwrap(), expected);
        assert!(ssa.root_sources(restored).unwrap().is_empty());
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
}
