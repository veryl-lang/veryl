//! Bottom-up finite module dependency-graph summaries.

use super::graph::DependencyGraph;
use super::model::{ModuleCombSummary, SummaryDependency, SummaryNode, SummaryNodeKind};
use crate::ir::{Module, VarKind};
use crate::{HashMap, HashSet};
use daggy::petgraph::Direction;
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;
use std::collections::VecDeque;

pub(super) fn compute_module_summary(
    module: &Module,
    graph: &DependencyGraph,
) -> ModuleCombSummary {
    let sources = graph
        .node_indices()
        .filter(|&node| {
            matches!(
                node_kind(module, &graph[node]),
                SummaryNodeKind::Input | SummaryNodeKind::Interface
            )
        })
        .collect::<Vec<_>>();
    let destinations = graph
        .node_indices()
        .filter(|&node| {
            matches!(
                node_kind(module, &graph[node]),
                SummaryNodeKind::Output | SummaryNodeKind::Interface
            )
        })
        .collect::<Vec<_>>();

    let forward = reachable(graph, sources, Direction::Outgoing);
    let backward = reachable(graph, destinations, Direction::Incoming);
    let retained = graph
        .node_indices()
        .filter(|node| forward.contains(node) && backward.contains(node))
        .collect::<HashSet<_>>();
    let mut cyclic = HashSet::default();
    for scc in tarjan_scc(&**graph) {
        if scc.len() > 1
            || scc
                .first()
                .is_some_and(|&node| graph.edges(node).any(|edge| edge.target() == node))
        {
            cyclic.extend(scc);
        }
    }
    let kept = graph
        .node_indices()
        .filter(|node| {
            let incoming = graph
                .edges_directed(*node, Direction::Incoming)
                .filter(|edge| retained.contains(&edge.source()))
                .count();
            let outgoing = graph
                .edges_directed(*node, Direction::Outgoing)
                .filter(|edge| retained.contains(&edge.target()))
                .count();
            retained.contains(node)
                && (node_kind(module, &graph[*node]) != SummaryNodeKind::Internal
                    || cyclic.contains(node)
                    || !graph[*node].domains.is_empty()
                    // Collapse only a linear series node. Eliminating a
                    // branch or join would enumerate path combinations and
                    // can make a compact dependency DAG exponential.
                    || incoming != 1
                    || outgoing != 1)
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
            region: graph[node].region,
            domains: graph[node].domains.clone(),
            kind: node_kind(module, &graph[node]),
        })
        .collect();
    let mut edges = Vec::new();
    for &source in &kept {
        let mut reached = HashMap::default();
        let mut queued = HashSet::default();
        let mut queue = VecDeque::new();
        for edge in graph.edges(source) {
            enqueue_if_changed(
                &mut reached,
                &mut queued,
                &mut queue,
                (edge.target(), edge.weight().kind),
                edge.weight().condition.clone(),
            );
        }
        while let Some(state @ (node, dependency)) = queue.pop_front() {
            queued.remove(&state);
            let condition = reached[&state].clone();
            if let Some(&destination) = indices.get(&node) {
                edges.push(SummaryDependency {
                    source: indices[&source],
                    destination,
                    kind: dependency,
                    condition,
                });
                continue;
            }
            for edge in graph.edges(node) {
                let Some(condition) = condition.union_if_compatible(&edge.weight().condition)
                else {
                    continue;
                };
                enqueue_if_changed(
                    &mut reached,
                    &mut queued,
                    &mut queue,
                    (edge.target(), dependency.compose(edge.weight().kind)),
                    condition,
                );
            }
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

fn enqueue_if_changed(
    reached: &mut HashMap<(NodeIndex, super::model::BitDependency), super::ssa::PathCondition>,
    queued: &mut HashSet<(NodeIndex, super::model::BitDependency)>,
    queue: &mut VecDeque<(NodeIndex, super::model::BitDependency)>,
    state: (NodeIndex, super::model::BitDependency),
    condition: super::ssa::PathCondition,
) {
    let changed = if let Some(existing) = reached.get_mut(&state) {
        let merged = existing.disjoin(&condition);
        if *existing == merged {
            false
        } else {
            *existing = merged;
            true
        }
    } else {
        reached.insert(state, condition);
        true
    };
    if changed && queued.insert(state) {
        queue.push_back(state);
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
