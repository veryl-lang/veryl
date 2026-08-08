//! IR-independent statement-ordered SSA state.

use crate::{HashMap, HashSet};
use std::hash::Hash;

pub(super) type VersionId = usize;

#[derive(Clone)]
enum Version<K> {
    Entry(K),
    Definition(Vec<VersionId>),
    Phi(Vec<VersionId>),
}

#[derive(Clone)]
pub(super) struct Snapshot<K> {
    current: HashMap<K, VersionId>,
}

pub(super) struct SsaStore<K> {
    versions: Vec<Version<K>>,
    entries: HashMap<K, VersionId>,
    current: HashMap<K, VersionId>,
}

impl<K> Default for SsaStore<K> {
    fn default() -> Self {
        Self {
            versions: Vec::new(),
            entries: HashMap::default(),
            current: HashMap::default(),
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
            let version = self.entry(key);
            self.current.insert(key, version);
            version
        }
    }

    pub(super) fn definition(&mut self, mut sources: Vec<VersionId>) -> VersionId {
        sources.sort_unstable();
        sources.dedup();
        let version = self.versions.len();
        self.versions.push(Version::Definition(sources));
        version
    }

    pub(super) fn bind(&mut self, key: K, version: VersionId) {
        self.current.insert(key, version);
    }

    pub(super) fn snapshot(&self) -> Snapshot<K> {
        Snapshot {
            current: self.current.clone(),
        }
    }

    pub(super) fn restore(&mut self, snapshot: &Snapshot<K>) {
        self.current.clone_from(&snapshot.current);
    }

    pub(super) fn merge(&mut self, base: &Snapshot<K>, states: &[Snapshot<K>]) {
        let mut keys: HashSet<K> = base.current.keys().copied().collect();
        for state in states {
            keys.extend(state.current.keys().copied());
        }
        let mut merged = HashMap::default();
        for key in keys {
            let fallback = base
                .current
                .get(&key)
                .copied()
                .unwrap_or_else(|| self.entry(key));
            let inputs = states
                .iter()
                .map(|state| state.current.get(&key).copied().unwrap_or(fallback))
                .collect();
            merged.insert(key, self.phi(inputs));
        }
        self.current = merged;
    }

    pub(super) fn root_sources(&self, version: VersionId) -> HashSet<K> {
        let mut sources = HashSet::default();
        let mut visited = HashSet::default();
        match &self.versions[version] {
            // A final LiveOnEntry value is retained state, not a combinational
            // read. Entry versions reached through an explicit definition are.
            Version::Entry(_) => {}
            Version::Definition(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, true, &mut sources, &mut visited);
                }
            }
            Version::Phi(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, false, &mut sources, &mut visited);
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

    fn collect_sources(
        &self,
        version: VersionId,
        include_entry: bool,
        sources: &mut HashSet<K>,
        visited: &mut HashSet<(VersionId, bool)>,
    ) {
        if !visited.insert((version, include_entry)) {
            return;
        }
        match &self.versions[version] {
            Version::Entry(key) => {
                if include_entry {
                    sources.insert(*key);
                }
            }
            Version::Definition(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, true, sources, visited);
                }
            }
            Version::Phi(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, include_entry, sources, visited);
                }
            }
        }
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
    fn retained_live_on_entry_is_not_a_combinational_read() {
        let mut ssa = SsaStore::default();
        let base = ssa.snapshot();
        let retained = ssa.read("destination");
        let assigned = ssa.definition(Vec::new());
        ssa.bind("destination", assigned);
        let branch = ssa.snapshot();

        ssa.merge(&base, &[base.clone(), branch]);

        let merged = ssa.read("destination");
        assert_ne!(merged, retained);
        assert!(ssa.root_sources(merged).is_empty());
    }

    #[test]
    fn restore_discards_current_bindings_without_discarding_versions() {
        let mut ssa = SsaStore::default();
        let base = ssa.snapshot();
        let source = ssa.read("source");
        let definition = ssa.definition(vec![source]);
        ssa.bind("destination", definition);

        ssa.restore(&base);
        let restored = ssa.read("destination");

        let expected = ["source"].into_iter().collect::<HashSet<_>>();
        assert_eq!(ssa.root_sources(definition), expected);
        assert!(ssa.root_sources(restored).is_empty());
    }
}
