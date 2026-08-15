//! Bottom-up module feedthrough summary construction.

use super::graph::DependencyGraph;
use super::model::{BitDependency, ModuleCombSummary, SummaryDependency, SummaryRegion};
use super::region::{BitPartition, NodeKey};
use super::ssa::PathCondition;
use crate::ir::{Module, VarId, VarKind};
use crate::{HashMap, HashSet};
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;
use std::collections::VecDeque;

type ReachabilityState = (NodeIndex, BitDependency);

pub(super) fn compute_module_summary(
    module: &Module,
    graph: &DependencyGraph,
    bit_part: &BitPartition,
) -> ModuleCombSummary {
    let (source_ids, destination_ids) = summary_endpoint_ids(module);
    let mut feedthrough: HashMap<SummaryRegion, Vec<SummaryDependency>> = HashMap::default();
    let mut reached = HashMap::default();
    let mut queued = HashSet::default();
    let mut queue = VecDeque::new();

    for source_node in graph.node_indices() {
        let source_key = graph[source_node];
        if !source_ids.contains(&source_key.0) {
            continue;
        }
        let Some(source) = summary_region(source_key, bit_part) else {
            continue;
        };

        reached.clear();
        queued.clear();
        queue.clear();
        for edge in graph.edges(source_node) {
            let state = (edge.target(), edge.weight().kind);
            enqueue_if_changed(
                &mut reached,
                &mut queued,
                &mut queue,
                state,
                edge.weight().condition.clone(),
            );
        }

        while let Some(state @ (node, dependency)) = queue.pop_front() {
            queued.remove(&state);
            let condition = reached[&state].clone();
            for edge in graph.edges(node) {
                let Some(condition) = condition.union_if_compatible(&edge.weight().condition)
                else {
                    continue;
                };
                let state = (edge.target(), dependency.compose(edge.weight().kind));
                enqueue_if_changed(&mut reached, &mut queued, &mut queue, state, condition);
            }
        }

        let mut destinations = Vec::new();
        for (&(node, kind), condition) in &reached {
            let key = graph[node];
            if destination_ids.contains(&key.0)
                && let Some(destination) = summary_region(key, bit_part)
            {
                destinations.push(SummaryDependency {
                    destination,
                    kind,
                    condition: condition.clone(),
                });
            }
        }
        feedthrough.insert(source, coalesce_destinations(destinations));
    }

    ModuleCombSummary {
        feedthrough,
        complete: true,
    }
}

fn summary_endpoint_ids(module: &Module) -> (HashSet<VarId>, HashSet<VarId>) {
    let mut sources = HashSet::default();
    let mut destinations = HashSet::default();
    for variable in module.variables.values() {
        match variable.kind {
            VarKind::Input => {
                sources.insert(variable.id);
            }
            VarKind::Output => {
                destinations.insert(variable.id);
            }
            _ => {}
        }
    }
    for &id in module.interface_members.keys() {
        sources.insert(id);
        destinations.insert(id);
    }
    (sources, destinations)
}

fn enqueue_if_changed(
    reached: &mut HashMap<ReachabilityState, PathCondition>,
    queued: &mut HashSet<ReachabilityState>,
    queue: &mut VecDeque<ReachabilityState>,
    state: ReachabilityState,
    condition: PathCondition,
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

fn coalesce_destinations(mut destinations: Vec<SummaryDependency>) -> Vec<SummaryDependency> {
    destinations
        .sort_unstable_by_key(|dependency| (dependency.destination, dependency.condition.clone()));
    let mut merged: Vec<SummaryDependency> = Vec::with_capacity(destinations.len());
    for dependency in destinations {
        let Some(previous) = merged.last_mut() else {
            merged.push(dependency);
            continue;
        };
        if previous.destination == dependency.destination
            && previous.condition == dependency.condition
        {
            previous.kind = previous.kind.union(dependency.kind);
            continue;
        }
        let adjacent = previous.condition == dependency.condition
            && previous.destination.id == dependency.destination.id
            && previous.destination.packed == dependency.destination.packed
            && previous.kind.packed == dependency.kind.packed
            && previous.destination.array.end() == Some(dependency.destination.array.start);
        if adjacent
            && let Some(length) = previous
                .destination
                .array
                .length
                .checked_add(dependency.destination.array.length)
        {
            previous.destination.array.length = length;
            previous.kind = previous.kind.union(dependency.kind);
        } else {
            merged.push(dependency);
        }
    }
    merged
}

fn summary_region(key: NodeKey, bit_part: &BitPartition) -> Option<SummaryRegion> {
    let packed = bit_part.ranges_of((key.0, key.1)).get(key.2).copied()?;
    Some(SummaryRegion {
        id: key.0,
        array: key.1,
        packed,
    })
}
