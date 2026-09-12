//! Bottom-up finite module dependency-graph summaries.

use super::graph::DependencyGraph;
use super::model::{
    BitDependency, ModuleCombSummary, SummaryDependency, SummaryNode, SummaryNodeKind,
};
use crate::ir::{Module, VarKind};
use crate::{HashMap, HashSet};
use daggy::petgraph::Direction;
use daggy::petgraph::algo::kosaraju_scc;
use daggy::petgraph::graph::{Graph, NodeIndex};
use daggy::petgraph::visit::EdgeRef;
use std::collections::VecDeque;

#[cfg(test)]
mod tests;

// Guarded and positional boundaries cannot always be contracted. Bound the
// child structure copied into each parent before reserving or cloning it, so
// a small hierarchy cannot expand into its exponentially large instance tree.
const MODULE_SUMMARY_WORK: usize = 1_000_000;

#[cfg(test)]
thread_local! {
    static INPUT_EDGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static WALKED_EDGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static SUMMARY_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(MODULE_SUMMARY_WORK) };
}

#[cfg(test)]
pub(crate) fn with_module_summary_limit<T>(limit: usize, f: impl FnOnce() -> T) -> T {
    struct Reset(usize);
    impl Drop for Reset {
        fn drop(&mut self) {
            SUMMARY_LIMIT.set(self.0);
        }
    }
    let _reset = Reset(SUMMARY_LIMIT.replace(limit));
    f()
}

pub(super) struct ExpansionBudget {
    remaining: usize,
}

impl ExpansionBudget {
    pub(super) fn new() -> Self {
        #[cfg(test)]
        let remaining = SUMMARY_LIMIT.get();
        #[cfg(not(test))]
        let remaining = MODULE_SUMMARY_WORK;
        Self { remaining }
    }

    pub(super) fn reserve(&mut self, summary: &ModuleCombSummary) -> bool {
        let remaining = (|| {
            let remaining = self.remaining.checked_sub(summary.nodes.len())?;
            let mut remaining = remaining.checked_sub(summary.edges.len())?;
            for node in &summary.nodes {
                remaining = remaining.checked_sub(node.domains.len())?;
            }
            for edge in &summary.edges {
                remaining = remaining.checked_sub(edge.condition.work_size())?;
            }
            Some(remaining)
        })();
        // Stop retrying large summaries once the module's budget is exhausted.
        self.remaining = remaining.unwrap_or(0);
        remaining.is_some()
    }

    pub(super) fn remaining(&self) -> usize {
        self.remaining
    }

    pub(super) fn reserve_dag<K>(&mut self, graph: &super::ssa::DependencyDag<K>) -> bool {
        let cost = graph
            .domains
            .iter()
            .fold(graph.nodes.len(), |cost, domains| {
                cost.saturating_add(domains.len())
            });
        let cost = graph.edges.iter().fold(cost, |cost, edge| {
            cost.saturating_add(edge.condition.work_size().saturating_add(1))
        });
        self.reserve_work(cost)
    }

    pub(super) fn reserve_work(&mut self, cost: usize) -> bool {
        let remaining = self.remaining.checked_sub(cost);
        self.remaining = remaining.unwrap_or(0);
        remaining.is_some()
    }
}

#[cfg(test)]
pub(crate) fn reset_module_summary_work() {
    INPUT_EDGES.set(0);
    WALKED_EDGES.set(0);
}

#[cfg(test)]
pub(crate) fn module_summary_work() -> (usize, usize) {
    (INPUT_EDGES.get(), WALKED_EDGES.get())
}

pub(super) fn compute_module_summary(
    module: &Module,
    graph: &DependencyGraph,
) -> ModuleCombSummary {
    summarize_graph(graph, |node| node_kind(module, &graph[node]))
}

fn summarize_graph(
    graph: &DependencyGraph,
    kind: impl Fn(NodeIndex) -> SummaryNodeKind,
) -> ModuleCombSummary {
    #[cfg(test)]
    INPUT_EDGES.set(INPUT_EDGES.get() + graph.edge_count());
    let sources = graph
        .node_indices()
        .filter(|&node| {
            matches!(
                kind(node),
                SummaryNodeKind::Input | SummaryNodeKind::Interface
            )
        })
        .collect::<Vec<_>>();
    let destinations = graph
        .node_indices()
        .filter(|&node| {
            matches!(
                kind(node),
                SummaryNodeKind::Output | SummaryNodeKind::Interface
            )
        })
        .collect::<Vec<_>>();

    let forward = reachable(graph, sources, Direction::Outgoing);
    let backward = reachable(graph, destinations, Direction::Incoming);
    // Degree tests, cycle retention and contraction must all use this same
    // induced graph. Walking the original graph can enter a discarded cycle
    // or DAG and enumerate arbitrarily many positional paths there.
    let mut retained = Graph::new();
    let mapped = graph
        .node_indices()
        .filter(|node| forward.contains(node) && backward.contains(node))
        .map(|node| (node, retained.add_node((node, kind(node)))))
        .collect::<HashMap<_, _>>();
    for edge in graph.edge_references() {
        if let (Some(&source), Some(&destination)) =
            (mapped.get(&edge.source()), mapped.get(&edge.target()))
        {
            retained.add_edge(source, destination, edge.weight());
        }
    }
    let mut cyclic = HashSet::default();
    for scc in kosaraju_scc(&retained) {
        if scc.len() > 1
            || scc
                .first()
                .is_some_and(|&node| retained.edges(node).any(|edge| edge.target() == node))
        {
            cyclic.extend(scc);
        }
    }
    let kept = retained
        .node_indices()
        .filter(|node| {
            let incoming = retained.edges_directed(*node, Direction::Incoming).count();
            let outgoing = retained.edges_directed(*node, Direction::Outgoing).count();
            let transparent = retained.edges(*node).next().is_some_and(|edge| {
                edge.weight().kind == BitDependency::identity()
                    && edge.weight().condition.is_unconditional()
            });
            // An identity copy can repeat bounds already enforced by its
            // predecessor. Keep shifted and whole-value boundaries: those
            // operations can escape the predecessor's domain.
            let domains = &graph[retained[*node].0].domains;
            let redundant_domain = domains.is_empty()
                || (incoming == 1
                    && retained
                        .edges_directed(*node, Direction::Incoming)
                        .next()
                        .is_some_and(|edge| {
                            edge.weight().kind == BitDependency::identity()
                                && *domains == graph[retained[edge.source()].0].domains
                        }));
            retained[*node].1 != SummaryNodeKind::Internal
                || cyclic.contains(node)
                || !redundant_domain
                || incoming != 1
                || outgoing != 1
                // Keep dependency operations and guards as graph structure.
                // In particular, accumulating independent guards along a
                // chain would repeatedly copy a growing condition vector.
                || !transparent
        })
        .collect::<Vec<_>>();
    let indices = kept
        .iter()
        .enumerate()
        .map(|(summary, graph)| (*graph, summary))
        .collect::<HashMap<_, _>>();

    let nodes = kept
        .iter()
        .map(|&node| SummaryNode {
            region: graph[retained[node].0].region,
            domains: graph[retained[node].0].domains.clone(),
            kind: retained[node].1,
        })
        .collect();
    let mut edges = Vec::new();
    #[cfg(test)]
    let mut visited = HashSet::default();
    // An omitted node is acyclic, has one incoming edge and has one
    // unconditional identity edge leaving it. Thus these chains cannot fork,
    // merge or cycle, and each retained edge is visited at most once in total.
    // No offset closure, path-condition accumulation or fixed point is needed.
    for &source in &kept {
        for edge in retained.edges(source) {
            #[cfg(test)]
            {
                assert!(visited.insert(edge.id()), "summary chains must be disjoint");
                WALKED_EDGES.set(WALKED_EDGES.get() + 1);
            }
            let mut node = edge.target();
            while !indices.contains_key(&node) {
                let next = retained
                    .edges(node)
                    .next()
                    .expect("an omitted series node has one outgoing edge");
                debug_assert_eq!(next.weight().kind, BitDependency::identity());
                debug_assert!(next.weight().condition.is_unconditional());
                #[cfg(test)]
                {
                    assert!(visited.insert(next.id()), "summary chains must be disjoint");
                    WALKED_EDGES.set(WALKED_EDGES.get() + 1);
                }
                node = next.target();
            }
            edges.push(SummaryDependency {
                source: indices[&source],
                destination: indices[&node],
                kind: edge.weight().kind,
                condition: edge.weight().condition.clone(),
            });
        }
    }
    edges.sort_unstable_by_key(|edge| {
        (
            edge.source,
            edge.destination,
            edge.kind,
            edge.condition.clone(),
        )
    });
    edges.dedup_by(|left, right| {
        left.source == right.source
            && left.destination == right.destination
            && left.kind == right.kind
            && left.condition == right.condition
    });

    ModuleCombSummary {
        nodes,
        edges,
        complete: true,
    }
}

fn reachable(
    graph: &DependencyGraph,
    seeds: Vec<NodeIndex>,
    direction: Direction,
) -> HashSet<NodeIndex> {
    let mut reached = seeds.iter().copied().collect::<HashSet<_>>();
    let mut queue = VecDeque::from(seeds);
    while let Some(node) = queue.pop_front() {
        for edge in graph.edges_directed(node, direction) {
            let next = match direction {
                Direction::Outgoing => edge.target(),
                Direction::Incoming => edge.source(),
            };
            if reached.insert(next) {
                queue.push_back(next);
            }
        }
    }
    reached
}

fn node_kind(module: &Module, node: &super::graph::GraphNode) -> SummaryNodeKind {
    let Some(key) = node.diagnostic else {
        return SummaryNodeKind::Internal;
    };
    if module.interface_members.contains_key(&key.0) {
        return SummaryNodeKind::Interface;
    }
    match module.variables.get(&key.0).map(|variable| variable.kind) {
        Some(VarKind::Input) => SummaryNodeKind::Input,
        Some(VarKind::Output) => SummaryNodeKind::Output,
        _ => SummaryNodeKind::Internal,
    }
}
