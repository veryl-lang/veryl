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

    let mut contraction = Contraction::new(&retained, graph, &cyclic);
    contraction.run();
    contraction.into_summary()
}

/// Dependencies between the retained nodes of a summary.
///
/// Internal acyclic nodes are eliminated while that does not enlarge the
/// graph: an elimination replaces every pair of an incoming and an outgoing
/// edge by one edge with the composed relation and the conjoined condition.
/// A node whose domain is not implied by its inputs is merged into a
/// single-output predecessor instead, by pulling its domain back into the
/// predecessor's coordinates. Both steps preserve the represented paths:
/// composition and conjunction are exact for translations, and otherwise
/// only admit more positions. Disjunction of conditions on parallel edges
/// with the same relation is exact.
struct Contraction<'a> {
    nodes: Vec<Option<ContractedNode>>,
    outgoing: Vec<HashMap<(usize, BitDependency), super::ssa::PathCondition>>,
    incoming: Vec<HashSet<(usize, BitDependency)>>,
    cyclic: Vec<bool>,
    graph: &'a DependencyGraph,
}

struct ContractedNode {
    graph: NodeIndex,
    kind: SummaryNodeKind,
    domains: Vec<super::ssa::PositionDomain>,
}

type AxisBounds = (isize, isize);

// Nodes of a condition that a contraction may produce. Longer conjunctions
// stay as graph structure, so a chain of independent guards is not copied
// into ever longer conditions.
pub(super) const SUMMARY_CONDITION_NODES: usize = 32;

/// `first` followed by `second`, unless a translation overflows.
fn compose_exact(first: BitDependency, second: BitDependency) -> Option<BitDependency> {
    use super::position::{Axis, Link};
    let composed = first.compose(second);
    for axis in Axis::BOTH {
        if let Link::Map(map) = second.link(axis) {
            let read = if map.crossed { axis.other() } else { axis };
            if matches!(first.link(read), Link::Map(_)) && composed.link(axis) == Link::Unlinked {
                return None;
            }
        }
    }
    Some(composed)
}

fn domain_bounds(domain: &super::ssa::PositionDomain) -> Option<[AxisBounds; 2]> {
    let range = |start: usize, length: usize| {
        let start = isize::try_from(start).ok()?;
        Some((start, start.checked_add_unsigned(length)?))
    };
    Some([
        range(domain.array_start, domain.array_length)?,
        range(domain.packed_start, domain.packed_length)?,
    ])
}

fn bounds_domain(bounds: [AxisBounds; 2]) -> Option<super::ssa::PositionDomain> {
    let [array, packed] = bounds;
    if array.0 >= array.1 || packed.0 >= packed.1 {
        return None;
    }
    Some(super::ssa::PositionDomain {
        array_start: usize::try_from(array.0).ok()?,
        array_length: usize::try_from(array.1 - array.0).ok()?,
        packed_start: usize::try_from(packed.0).ok()?,
        packed_length: usize::try_from(packed.1 - packed.0).ok()?,
    })
}

/// Hull of the destination positions of a source box, `None` if unbounded
/// or beyond `isize`.
fn image(relation: BitDependency, source: [AxisBounds; 2]) -> Option<Option<[AxisBounds; 2]>> {
    use super::position::{Axis, Link};
    let mut result = [(0, 0); 2];
    for (index, axis) in Axis::BOTH.into_iter().enumerate() {
        match relation.link(axis) {
            Link::Never => return Some(None),
            Link::Unlinked | Link::Strided { .. } => return None,
            Link::Map(map) => {
                let read = usize::from(index == 0) ^ usize::from(!map.crossed);
                let (start, end) = source[read];
                let Some((first, last)) = map.source_parameters(start, end).ok()? else {
                    return Some(None);
                };
                result[index] = map.destination_hull(first, last).ok()?;
            }
        }
    }
    Some(Some(result))
}

/// The source box whose positions reach `destination`, within `source`:
/// `Some(None)` when no position does, `None` when it is not a box or
/// is beyond `isize`.
#[allow(clippy::option_option)]
fn preimage(
    relation: BitDependency,
    destination: [AxisBounds; 2],
    mut source: [AxisBounds; 2],
) -> Option<Option<[AxisBounds; 2]>> {
    use super::position::{Axis, Link};
    for (index, axis) in Axis::BOTH.into_iter().enumerate() {
        let map = match relation.link(axis) {
            Link::Map(map) => map,
            Link::Never => return Some(None),
            Link::Unlinked | Link::Strided { .. } => return None,
        };
        let read = usize::from(index == 0) ^ usize::from(!map.crossed);
        let (start, end) = destination[index];
        let Some((first, last)) = map.destination_parameters(start, end).ok()? else {
            return Some(None);
        };
        if first == isize::MIN && last == isize::MAX {
            continue;
        }
        let hull = map.source_hull(first, last).ok()?;
        source[read] = (source[read].0.max(hull.0), source[read].1.min(hull.1));
        if source[read].0 >= source[read].1 {
            return Some(None);
        }
    }
    Some(Some(source))
}

impl<'a> Contraction<'a> {
    fn new(
        retained: &Graph<(NodeIndex, SummaryNodeKind), &super::graph::GraphDependency>,
        graph: &'a DependencyGraph,
        cyclic: &HashSet<NodeIndex>,
    ) -> Self {
        let count = retained.node_count();
        let mut contraction = Self {
            nodes: retained
                .node_indices()
                .map(|node| {
                    let (original, kind) = retained[node];
                    Some(ContractedNode {
                        graph: original,
                        kind,
                        domains: graph[original].domains.clone(),
                    })
                })
                .collect(),
            outgoing: vec![HashMap::default(); count],
            incoming: vec![HashSet::default(); count],
            cyclic: retained
                .node_indices()
                .map(|node| cyclic.contains(&node))
                .collect(),
            graph,
        };
        for edge in retained.edge_references() {
            #[cfg(test)]
            WALKED_EDGES.set(WALKED_EDGES.get() + 1);
            contraction.add_edge(
                edge.source().index(),
                edge.target().index(),
                edge.weight().kind,
                edge.weight().condition,
            );
        }
        contraction
    }

    fn add_edge(
        &mut self,
        source: usize,
        destination: usize,
        relation: BitDependency,
        condition: super::ssa::PathCondition,
    ) {
        if relation.is_empty() {
            return;
        }
        let key = (destination, relation);
        let condition = match self.outgoing[source].get(&key) {
            Some(existing) => existing.disjoin(&condition),
            None => condition,
        };
        self.outgoing[source].insert(key, condition);
        self.incoming[destination].insert((source, relation));
    }

    fn remove_node(&mut self, node: usize) {
        for (source, relation) in std::mem::take(&mut self.incoming[node]) {
            self.outgoing[source].remove(&(node, relation));
        }
        for ((destination, relation), _) in std::mem::take(&mut self.outgoing[node]) {
            self.incoming[destination].remove(&(node, relation));
        }
        self.nodes[node] = None;
    }

    fn eliminable(&self, node: usize) -> bool {
        !self.cyclic[node]
            && self.nodes[node]
                .as_ref()
                .is_some_and(|data| data.kind == SummaryNodeKind::Internal)
    }

    fn boxes(&self, node: usize) -> Option<Vec<[AxisBounds; 2]>> {
        let data = self.nodes[node].as_ref()?;
        if data.domains.is_empty() {
            return None;
        }
        data.domains.iter().map(domain_bounds).collect()
    }

    /// The node's domain removes no position: every input already lies
    /// inside it, or every successor reapplies it through an exact map.
    fn domain_is_redundant(&self, node: usize) -> bool {
        let Some(domains) = self.boxes(node) else {
            return true;
        };
        let downstream = || {
            let [domain] = domains.as_slice() else {
                return false;
            };
            self.outgoing[node].keys().all(|&(destination, relation)| {
                // Monotone maps that read both source axes send a position
                // outside the domain outside the image of the domain.
                let mut read = [false; 2];
                let exact = super::position::Axis::BOTH.into_iter().all(|axis| {
                    match relation.link(axis) {
                        super::position::Link::Map(map) if map.step != 0 => {
                            let source = if map.crossed { axis.other() } else { axis };
                            read[usize::from(source == super::position::Axis::Packed)] = true;
                            true
                        }
                        _ => false,
                    }
                }) && read == [true, true];
                let Some(successors) = self.boxes(destination) else {
                    return false;
                };
                exact
                    && matches!(image(relation, *domain), Some(Some(image)) if successors.iter().all(|successor| {
                        (0..2).all(|axis| image[axis].0 <= successor[axis].0 && successor[axis].1 <= image[axis].1)
                    }))
            })
        };
        if downstream() {
            return true;
        }
        self.incoming[node].iter().all(|&(source, relation)| {
            let Some(sources) = self.boxes(source) else {
                return false;
            };
            sources
                .into_iter()
                .all(|source| match image(relation, source) {
                    None => false,
                    Some(None) => true,
                    Some(Some(image)) => domains.iter().any(|domain| {
                        (0..2).all(|axis| {
                            domain[axis].0 <= image[axis].0 && image[axis].1 <= domain[axis].1
                        })
                    }),
                })
        })
    }

    fn eliminate(&mut self, node: usize) -> bool {
        let inputs = self.incoming[node].len();
        let outputs = self.outgoing[node].len();
        if inputs.saturating_mul(outputs) > inputs + outputs || !self.domain_is_redundant(node) {
            return false;
        }
        let incoming = self.incoming[node]
            .iter()
            .map(|&(source, relation)| (source, relation, self.outgoing[source][&(node, relation)]))
            .collect::<Vec<_>>();
        let outgoing = self.outgoing[node]
            .iter()
            .map(|(&(destination, relation), &condition)| (destination, relation, condition))
            .collect::<Vec<_>>();
        // The replacement must not enlarge the summary.
        let mut replacement: HashMap<(usize, usize, BitDependency), super::ssa::PathCondition> =
            HashMap::default();
        for &(source, first, before) in &incoming {
            for &(destination, second, after) in &outgoing {
                let Some(condition) = before.conjoin_if_compatible(&after) else {
                    continue;
                };
                let Some(relation) = compose_exact(first, second) else {
                    return false;
                };
                if relation.is_empty() {
                    continue;
                }
                let key = (source, destination, relation);
                let condition = match replacement.get(&key) {
                    Some(existing) => existing.disjoin(&condition),
                    None => condition,
                };
                replacement.insert(key, condition);
            }
        }
        if replacement.len() > inputs + outputs {
            return false;
        }
        for (&(source, destination, relation), condition) in &replacement {
            let merged = match self.outgoing[source].get(&(destination, relation)) {
                Some(existing) => existing.disjoin(condition),
                None => *condition,
            };
            if merged.work_size() > SUMMARY_CONDITION_NODES {
                return false;
            }
        }
        self.remove_node(node);
        let mut replacement = replacement.into_iter().collect::<Vec<_>>();
        replacement.sort_unstable_by_key(|((source, destination, relation), condition)| {
            (*source, *destination, *relation, *condition)
        });
        for ((source, destination, relation), condition) in replacement {
            self.add_edge(source, destination, relation, condition);
        }
        true
    }

    /// Merge a node with one input and one output into its predecessor when
    /// the predecessor has no other output.
    fn merge_into_predecessor(&mut self, node: usize) -> bool {
        if self.incoming[node].len() != 1 || self.outgoing[node].len() != 1 {
            return false;
        }
        let (source, first) = *self.incoming[node].iter().next().expect("one input");
        let (&(destination, second), &after) =
            self.outgoing[node].iter().next().expect("one output");
        if source == destination || !self.eliminable(source) || self.outgoing[source].len() != 1 {
            return false;
        }
        let Some(domains) = self.boxes(node) else {
            return false;
        };
        // An unbounded predecessor still holds only non-negative positions.
        let source_domains = self
            .boxes(source)
            .unwrap_or_else(|| vec![[(0, isize::MAX), (0, isize::MAX)]]);
        let ([domain], [source_domain]) = (domains.as_slice(), source_domains.as_slice()) else {
            return false;
        };
        let pulled = match preimage(first, *domain, *source_domain) {
            Some(Some(pulled)) => bounds_domain(pulled),
            // No position of the predecessor reaches this node.
            Some(None) => {
                self.remove_node(node);
                return true;
            }
            None => return false,
        };
        let Some(pulled) = pulled else {
            self.remove_node(node);
            return true;
        };
        let before = self.outgoing[source][&(node, first)];
        let Some(condition) = before.conjoin_if_compatible(&after) else {
            return false;
        };
        let Some(relation) = compose_exact(first, second) else {
            return false;
        };
        if condition.work_size() > SUMMARY_CONDITION_NODES {
            return false;
        }
        self.remove_node(node);
        self.nodes[source]
            .as_mut()
            .expect("a predecessor in a summary chain is retained")
            .domains = vec![pulled];
        self.add_edge(source, destination, relation, condition);
        true
    }

    fn run(&mut self) {
        loop {
            let mut changed = false;
            for node in 0..self.nodes.len() {
                if self.eliminable(node)
                    && (self.eliminate(node) || self.merge_into_predecessor(node))
                {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn into_summary(self) -> ModuleCombSummary {
        let alive = (0..self.nodes.len())
            .filter(|&node| self.nodes[node].is_some())
            .collect::<Vec<_>>();
        let indices = alive
            .iter()
            .enumerate()
            .map(|(summary, &node)| (node, summary))
            .collect::<HashMap<_, _>>();
        let nodes = alive
            .iter()
            .map(|&node| {
                let data = self.nodes[node].as_ref().expect("alive");
                SummaryNode {
                    region: self.graph[data.graph].region,
                    domains: data.domains.clone(),
                    kind: data.kind,
                }
            })
            .collect();
        let mut edges = Vec::new();
        for &source in &alive {
            for (&(destination, relation), condition) in &self.outgoing[source] {
                edges.push(SummaryDependency {
                    source: indices[&source],
                    destination: indices[&destination],
                    kind: relation,
                    condition: *condition,
                });
            }
        }
        edges.sort_unstable_by_key(|edge| {
            (edge.source, edge.destination, edge.kind, edge.condition)
        });
        ModuleCombSummary {
            nodes,
            edges,
            complete: true,
        }
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
