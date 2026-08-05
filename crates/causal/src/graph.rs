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
    /// The analysis input does not expose enough behavior to establish a
    /// dependency across this boundary.
    Opaque(OpaqueBoundary),
    /// The behavior is defined and available, but this analysis run did not
    /// derive its dependency relation.
    Analysis(AnalysisGap),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OpaqueBoundary {
    /// A dynamic access could not be bounded to a known object or static
    /// prefix. Bounded dynamic accesses are conservative and complete.
    UnboundedRegion,
    ExternalComponent,
    InoutPort,
    TimedOrEventEffect,
    /// Generic elaboration did not produce a concrete module shape.
    UnevaluatedGeneric,
    /// Earlier conversion retained a source construct without a causal model.
    UnsupportedConstruct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnalysisGap {
    UnresolvedHierarchy,
    RecursiveCall,
    Loop(LoopAnalysisGap),
    RegionMapping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LoopAnalysisGap {
    DynamicTripCount,
    ExpansionLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnalysisFailure {
    /// The frontend produced an IR shape which violates the causal adapter's
    /// invariants. This is an analyzer defect, not analysis incompleteness.
    MalformedModel,
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
        // Why both reasons are present: completeness is one predicate, while
        // clients still need to distinguish missing input behavior from a
        // dependency the current analysis declined to derive.
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
            [
                IncompleteReason::Opaque(OpaqueBoundary::ExternalComponent),
                IncompleteReason::Analysis(AnalysisGap::Loop(LoopAnalysisGap::ExpansionLimit)),
            ],
        );
        assert_eq!(graph.reaches(0, 1), Reachability::Proven);
        assert_eq!(graph.incomplete().len(), 2);
        assert!(
            graph
                .incomplete()
                .contains(&IncompleteReason::Opaque(OpaqueBoundary::ExternalComponent))
        );
        assert!(
            graph
                .incomplete()
                .contains(&IncompleteReason::Analysis(AnalysisGap::Loop(
                    LoopAnalysisGap::ExpansionLimit
                )))
        );
    }
}
