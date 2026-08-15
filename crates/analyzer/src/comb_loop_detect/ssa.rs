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
    pub(super) array: Option<i128>,
    pub(super) packed: Option<i128>,
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
            array: self.array.and_then(i128::checked_neg),
            packed: self.packed.and_then(i128::checked_neg),
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

fn compose_axis(left: Option<i128>, right: Option<i128>) -> Option<i128> {
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

    pub(super) fn root_sources(&self, version: VersionId) -> HashSet<K> {
        self.root_source_relations(version).into_keys().collect()
    }

    pub(super) fn root_source_relations(&self, version: VersionId) -> HashMap<K, PositionRelation> {
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
                    pending.extend(
                        positional
                            .iter()
                            .map(|(input, inner)| (*input, true, relation.compose(*inner))),
                    );
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
        sources
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
}
