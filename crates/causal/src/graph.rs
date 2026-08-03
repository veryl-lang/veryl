//! Deterministic causal graphs with explicit uncertainty provenance.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    Value,
    Control,
    Address,
    /// A conservative dependency introduced by an unresolved effect or alias.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncompleteReason {
    DynamicRegion,
    ExternalComponent,
    HierarchicalReference,
    InoutPort,
    RecursiveCall,
    RuntimeLoop,
    TimedOrEventEffect,
    UnsupportedSyntax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Edge<N> {
    pub from: N,
    pub to: N,
    pub kind: EdgeKind,
}

/// Immutable result of one independently buildable unit (procedure/module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalGraph<N> {
    edges: Vec<Edge<N>>,
    successors: BTreeMap<N, Vec<(N, EdgeKind)>>,
    incomplete: BTreeSet<IncompleteReason>,
}

impl<N: Copy + Ord> CausalGraph<N> {
    #[must_use]
    pub fn from_parts(
        edges: impl IntoIterator<Item = Edge<N>>,
        incomplete: impl IntoIterator<Item = IncompleteReason>,
    ) -> Self {
        let mut edges = edges.into_iter().collect::<Vec<_>>();
        edges.sort_unstable();
        edges.dedup();
        let mut successors = BTreeMap::<N, Vec<(N, EdgeKind)>>::new();
        for edge in &edges {
            successors
                .entry(edge.from)
                .or_default()
                .push((edge.to, edge.kind));
        }
        Self {
            edges,
            successors,
            incomplete: incomplete.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn edges(&self) -> &[Edge<N>] {
        &self.edges
    }

    #[must_use]
    pub fn incomplete(&self) -> &BTreeSet<IncompleteReason> {
        &self.incomplete
    }

    /// Reachability with uncertainty tracked separately from a proven path.
    #[must_use]
    pub fn reaches(&self, from: N, to: N) -> Reachability {
        let mut stack = vec![(from, false)];
        let mut visited = BTreeSet::new();
        let mut unknown_path = false;
        while let Some((node, used_unknown)) = stack.pop() {
            if !visited.insert((node, used_unknown)) {
                continue;
            }
            if node == to && node != from {
                if used_unknown {
                    unknown_path = true;
                } else {
                    return Reachability::Proven;
                }
            }
            for &(next, kind) in self.successors.get(&node).map_or(&[][..], Vec::as_slice) {
                stack.push((next, used_unknown || kind == EdgeKind::Unknown));
            }
        }
        if unknown_path {
            Reachability::Unknown
        } else {
            Reachability::Disproven
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    Proven,
    Disproven,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definite_paths_win_over_conservative_paths() {
        let graph = CausalGraph::from_parts(
            [
                Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::Unknown,
                },
                Edge {
                    from: 0,
                    to: 2,
                    kind: EdgeKind::Value,
                },
                Edge {
                    from: 2,
                    to: 1,
                    kind: EdgeKind::Control,
                },
            ],
            [IncompleteReason::ExternalComponent],
        );
        assert_eq!(graph.reaches(0, 1), Reachability::Proven);
        assert_eq!(graph.incomplete().len(), 1);
    }
}
