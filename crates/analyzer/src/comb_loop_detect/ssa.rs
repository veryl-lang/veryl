//! IR-independent statement-ordered SSA state.

use crate::{HashMap, HashSet};
use std::hash::Hash;

pub(super) type VersionId = usize;

#[derive(Clone)]
enum Version<K> {
    Entry(K),
    Definition {
        positional: Vec<(VersionId, PositionOffset)>,
        whole: Vec<VersionId>,
    },
    Phi(Vec<VersionId>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct PositionOffset {
    pub(super) array: i128,
    pub(super) packed: i128,
}

impl PositionOffset {
    pub(super) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            array: self.array.checked_add(other.array)?,
            packed: self.packed.checked_add(other.packed)?,
        })
    }

    pub(super) fn checked_neg(self) -> Option<Self> {
        Some(Self {
            array: self.array.checked_neg()?,
            packed: self.packed.checked_neg()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum SourceKind {
    Offset(PositionOffset),
    Whole,
}

impl SourceKind {
    fn compose(self, inner: Self) -> Self {
        match (self, inner) {
            (Self::Offset(outer), Self::Offset(inner)) => outer
                .checked_add(inner)
                .map(Self::Offset)
                .unwrap_or(Self::Whole),
            _ => Self::Whole,
        }
    }

    fn union(self, other: Self) -> Self {
        if self == other { self } else { Self::Whole }
    }
}

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
        mut positional: Vec<(VersionId, PositionOffset)>,
        mut whole: Vec<VersionId>,
    ) -> VersionId {
        positional.sort_unstable();
        positional.dedup();
        whole.sort_unstable();
        whole.dedup();
        let mut conflicting = Vec::new();
        for pair in positional.windows(2) {
            if pair[0].0 == pair[1].0 && pair[0].1 != pair[1].1 {
                conflicting.push(pair[0].0);
            }
        }
        whole.extend(conflicting);
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
        self.root_source_kinds(version).into_keys().collect()
    }

    pub(super) fn root_source_kinds(&self, version: VersionId) -> HashMap<K, SourceKind> {
        let mut sources: HashMap<K, SourceKind> = HashMap::default();
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
                        .map(|(input, offset)| (*input, true, SourceKind::Offset(*offset))),
                );
                pending.extend(whole.iter().map(|input| (*input, true, SourceKind::Whole)));
            }
            Version::Phi(inputs) => {
                pending.extend(
                    inputs.iter().map(|input| {
                        (*input, false, SourceKind::Offset(PositionOffset::default()))
                    }),
                );
            }
        }

        while let Some((version, include_entry, kind)) = pending.pop() {
            if !visited.insert((version, include_entry, kind)) {
                continue;
            }
            match &self.versions[version] {
                Version::Entry(key) => {
                    if include_entry {
                        sources
                            .entry(*key)
                            .and_modify(|existing| *existing = existing.union(kind))
                            .or_insert(kind);
                    }
                }
                Version::Definition { positional, whole } => {
                    pending.extend(positional.iter().map(|(input, offset)| {
                        (*input, true, kind.compose(SourceKind::Offset(*offset)))
                    }));
                    pending.extend(whole.iter().map(|input| (*input, true, SourceKind::Whole)));
                }
                Version::Phi(inputs) => {
                    pending.extend(inputs.iter().map(|input| (*input, include_entry, kind)));
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
    fn positional_definition_preserves_source_kind() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination =
            ssa.positional_definition(vec![(source, PositionOffset::default())], Vec::new());

        assert_eq!(
            ssa.root_source_kinds(destination).get("source"),
            Some(&SourceKind::Offset(PositionOffset::default()))
        );
    }

    #[test]
    fn positional_offsets_compose_through_definitions() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let first = ssa.positional_definition(
            vec![(
                source,
                PositionOffset {
                    array: 3,
                    packed: -2,
                },
            )],
            Vec::new(),
        );
        let destination = ssa.positional_definition(
            vec![(
                first,
                PositionOffset {
                    array: -1,
                    packed: 5,
                },
            )],
            Vec::new(),
        );

        assert_eq!(
            ssa.root_source_kinds(destination).get("source"),
            Some(&SourceKind::Offset(PositionOffset {
                array: 2,
                packed: 3,
            }))
        );
    }

    #[test]
    fn conflicting_positional_offsets_become_whole_dependencies() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination = ssa.positional_definition(
            vec![
                (source, PositionOffset::default()),
                (
                    source,
                    PositionOffset {
                        array: 0,
                        packed: 1,
                    },
                ),
            ],
            Vec::new(),
        );

        assert_eq!(
            ssa.root_source_kinds(destination).get("source"),
            Some(&SourceKind::Whole)
        );
    }

    #[test]
    fn whole_dependency_dominates_a_positional_path() {
        let mut ssa = SsaStore::default();
        let source = ssa.read("source");
        let destination =
            ssa.positional_definition(vec![(source, PositionOffset::default())], vec![source]);

        assert_eq!(
            ssa.root_source_kinds(destination).get("source"),
            Some(&SourceKind::Whole)
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
