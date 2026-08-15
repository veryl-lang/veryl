//! Dependency graph storage, edge normalization, and cycle detection.

mod guarded;

use super::model::{BitDependency, SummaryRegion};
use super::region::{BitPartition, NodeKey, translate_position};
use super::ssa::{PathCondition, PositionDomain};
use crate::ir::{Module, VarId};
use crate::{AnalyzerError, HashMap, HashSet};
use daggy::petgraph::Direction;
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::{EdgeIndex, NodeIndex};
use daggy::petgraph::visit::EdgeRef;
#[cfg(test)]
use guarded::compatible_cycle_displacements_cancel;
use guarded::{GuardedCycle, guarded_cycle_displacements_cancel};
use std::ops::{Deref, DerefMut};

#[derive(Clone, Debug)]
pub(super) struct GraphDependency {
    pub(super) kind: BitDependency,
    pub(super) condition: PathCondition,
}

#[derive(Clone, Debug)]
pub(super) struct GraphNode {
    pub(super) region: SummaryRegion,
    pub(super) domains: Vec<PositionDomain>,
    /// Present only for a region belonging to the module currently being
    /// diagnosed. Instance-summary internals deliberately have no synthetic
    /// `VarId` and therefore cannot collide with real variables.
    pub(super) diagnostic: Option<NodeKey>,
}

impl GraphDependency {
    pub(super) fn unconditional(kind: BitDependency) -> Self {
        Self {
            kind,
            condition: PathCondition::default(),
        }
    }
}

pub(super) struct DependencyGraph {
    graph: Graph<GraphNode, GraphDependency>,
    edges: HashMap<(NodeIndex, NodeIndex, BitDependency), EdgeIndex>,
}

impl DependencyGraph {
    pub(super) fn new() -> Self {
        Self {
            graph: Graph::new(),
            edges: HashMap::default(),
        }
    }
}

impl Deref for DependencyGraph {
    type Target = Graph<GraphNode, GraphDependency>;

    fn deref(&self) -> &Self::Target {
        &self.graph
    }
}

impl DerefMut for DependencyGraph {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.graph
    }
}

pub(super) fn add_dependency_edge(
    graph: &mut DependencyGraph,
    source: NodeIndex,
    destination: NodeIndex,
    dependency: GraphDependency,
) {
    let key = (source, destination, dependency.kind);
    if let Some(&existing) = graph.edges.get(&key) {
        let weight = graph
            .edge_weight_mut(existing)
            .expect("an edge found in the graph must remain present");
        weight.condition = weight.condition.disjoin(&dependency.condition);
    } else {
        let edge = graph.add_edge(source, destination, dependency);
        graph.edges.insert(key, edge);
    }
}

pub(super) fn add_region_dependency(
    graph: &mut DependencyGraph,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    bit_part: &BitPartition,
    source: NodeKey,
    destination: NodeKey,
    dependency: GraphDependency,
) {
    let Some(source) = ensure_node(graph, node_map, bit_part, source) else {
        return;
    };
    let Some(destination) = ensure_node(graph, node_map, bit_part, destination) else {
        return;
    };
    add_dependency_edge(graph, source, destination, dependency);
}

pub(super) fn ensure_node(
    graph: &mut DependencyGraph,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    bit_part: &BitPartition,
    key: NodeKey,
) -> Option<NodeIndex> {
    if let Some(node) = node_map.get(&key) {
        return Some(*node);
    }
    let packed = bit_part.ranges_of((key.0, key.1)).get(key.2).copied()?;
    let node = graph.add_node(GraphNode {
        region: SummaryRegion {
            id: key.0,
            array: key.1,
            packed,
        },
        domains: vec![PositionDomain {
            array_start: key.1.start,
            array_length: key.1.length,
            packed_start: packed.start,
            packed_length: packed.length,
        }],
        diagnostic: Some(key),
    });
    node_map.insert(key, node);
    Some(node)
}

pub(super) fn node_regions_overlap_with_dependency(
    source: &GraphNode,
    destination: &GraphNode,
    dependency: BitDependency,
) -> bool {
    dependency.array.is_none_or(|array| {
        spans_overlap_with_offset(
            source.region.array.start,
            source.region.array.length,
            destination.region.array.start,
            destination.region.array.length,
            array,
        )
    }) && dependency.packed.is_none_or(|packed| {
        spans_overlap_with_offset(
            source.region.packed.start,
            source.region.packed.length,
            destination.region.packed.start,
            destination.region.packed.length,
            packed,
        )
    })
}

fn spans_overlap_with_offset(
    source_start: usize,
    source_length: usize,
    destination_start: usize,
    destination_length: usize,
    offset: isize,
) -> bool {
    let Some(source_start) = translate_position(source_start, offset) else {
        return false;
    };
    let Some(source_end) = source_start.checked_add(source_length) else {
        return false;
    };
    let Some(destination_end) = destination_start.checked_add(destination_length) else {
        return false;
    };
    source_start < destination_end && destination_start < source_end
}

pub(super) fn check_graph(
    module: &Module,
    graph: &DependencyGraph,
    errors: &mut Vec<AnalyzerError>,
) {
    debug_assert!(
        unconstrained_subgraph_is_acyclic(graph),
        "unconstrained dependency nodes must be introduced as a DAG"
    );
    let sccs = tarjan_scc(&graph.graph);
    let mut reported: HashSet<Vec<NodeKey>> = HashSet::default();
    for scc in sccs {
        if !has_compatible_cycle(graph, &scc) {
            continue;
        }
        let mut keys: Vec<NodeKey> = scc
            .iter()
            .filter_map(|node| graph[*node].diagnostic)
            .collect();
        keys.sort();
        keys.dedup();
        if keys.is_empty() {
            continue;
        }
        if !reported.insert(keys.clone()) {
            continue;
        }
        if let Some(error) = build_error(module, &keys) {
            errors.push(error);
        }
    }
}

fn unconstrained_subgraph_is_acyclic(graph: &DependencyGraph) -> bool {
    let mut induced = Graph::<(), ()>::new();
    let mapped = graph
        .node_indices()
        .filter(|&node| graph[node].domains.is_empty())
        .map(|node| (node, induced.add_node(())))
        .collect::<HashMap<_, _>>();
    for edge in graph.edge_references() {
        let (Some(&source), Some(&destination)) =
            (mapped.get(&edge.source()), mapped.get(&edge.target()))
        else {
            continue;
        };
        induced.add_edge(source, destination, ());
    }
    !daggy::petgraph::algo::is_cyclic_directed(&induced)
}

fn has_compatible_cycle(graph: &DependencyGraph, scc: &[NodeIndex]) -> bool {
    let nodes: HashSet<_> = scc.iter().copied().collect();
    if has_zero_dependency_cycle(graph, scc, &nodes) {
        return true;
    }
    // Returning to the anchor ends the first-return search, so choosing a
    // broad domain first keeps that domain out of the internal search state.
    // This is important for sparse graphs with a wide repeated shift and a
    // narrow wrap region; correctness does not depend on the order.
    let mut starts = scc.to_vec();
    starts.sort_unstable_by_key(|&node| std::cmp::Reverse(domain_area(&graph[node])));
    for start in starts {
        let returnable = nodes_that_may_reach_start(graph, &nodes, start);
        let identity = BitDependency {
            array: Some(0),
            packed: Some(0),
        };
        let initial = initial_feasible_positions(&graph[start]);
        let mut cycles = HashSet::default();
        let mut stack = vec![(start, PathCondition::default(), identity, initial)];
        let mut reached: HashMap<
            NodeIndex,
            Vec<(BitDependency, PathCondition, Vec<FeasiblePosition>)>,
        > = HashMap::default();
        while let Some((node, condition, dependency, feasible)) = stack.pop() {
            for edge in graph.edges(node) {
                let next = edge.target();
                if !returnable.contains(&next) {
                    continue;
                }
                let Some(next_condition) = condition.union_if_compatible(&edge.weight().condition)
                else {
                    continue;
                };
                // Keep every traversed edge in the start node's coordinate
                // system. A return to `start` can then be classified by the
                // composed displacement instead of individual edge signs.
                let next_dependency = dependency.compose(edge.weight().kind);
                let feasible =
                    restrict_feasible_positions(&feasible, next_dependency, &graph[next].domains);
                if feasible.is_empty() {
                    continue;
                }
                if next == start {
                    if dependency_may_return_to_same_position(next_dependency) {
                        return true;
                    }
                    if next_dependency.exact_offset().is_none() {
                        return true;
                    }
                    cycles.insert(GuardedCycle {
                        dependency: next_dependency,
                        condition: next_condition,
                        feasible,
                    });
                    continue;
                }
                let states = reached.entry(next).or_default();
                if states.iter().any(
                    |(existing_dependency, existing_condition, existing_feasible)| {
                        *existing_dependency == next_dependency
                            && existing_condition.is_subset_of(&next_condition)
                            && *existing_feasible == feasible
                    },
                ) {
                    continue;
                }
                states.retain(
                    |(existing_dependency, existing_condition, existing_feasible)| {
                        *existing_dependency != next_dependency
                            || !next_condition.is_subset_of(existing_condition)
                            || *existing_feasible != feasible
                    },
                );
                states.push((next_dependency, next_condition.clone(), feasible.clone()));
                stack.push((next, next_condition, next_dependency, feasible));
            }
        }
        if guarded_cycle_displacements_cancel(&cycles) {
            return true;
        }
    }
    false
}

fn nodes_that_may_reach_start(
    graph: &DependencyGraph,
    scc: &HashSet<NodeIndex>,
    start: NodeIndex,
) -> HashSet<NodeIndex> {
    let mut reached = HashSet::default();
    let mut stack = vec![start];
    reached.insert(start);
    while let Some(node) = stack.pop() {
        for edge in graph.edges_directed(node, Direction::Incoming) {
            let source = edge.source();
            if !scc.contains(&source)
                || !edge_has_feasible_position(graph, source, node, edge.weight().kind)
            {
                continue;
            }
            if reached.insert(source) {
                stack.push(source);
            }
        }
    }
    reached
}

fn edge_has_feasible_position(
    graph: &DependencyGraph,
    source: NodeIndex,
    destination: NodeIndex,
    dependency: BitDependency,
) -> bool {
    !restrict_feasible_positions(
        &initial_feasible_positions(&graph[source]),
        dependency,
        &graph[destination].domains,
    )
    .is_empty()
}

fn domain_area(node: &GraphNode) -> usize {
    if node.domains.is_empty() {
        return usize::MAX;
    }
    node.domains.iter().fold(0usize, |total, domain| {
        total.saturating_add(domain.array_length.saturating_mul(domain.packed_length))
    })
}

fn has_zero_dependency_cycle(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    nodes: &HashSet<NodeIndex>,
) -> bool {
    for &start in scc {
        let initial = initial_feasible_positions(&graph[start]);
        let mut stack = vec![(start, PathCondition::default(), initial)];
        let mut reached: HashMap<NodeIndex, Vec<(PathCondition, Vec<FeasiblePosition>)>> =
            HashMap::default();
        while let Some((node, condition, feasible)) = stack.pop() {
            for edge in graph.edges(node) {
                if !dependency_may_return_to_same_position(edge.weight().kind) {
                    continue;
                }
                let next = edge.target();
                if !nodes.contains(&next) {
                    continue;
                }
                let Some(next_condition) = condition.union_if_compatible(&edge.weight().condition)
                else {
                    continue;
                };
                let feasible = restrict_feasible_positions(
                    &feasible,
                    edge.weight().kind,
                    &graph[next].domains,
                );
                if feasible.is_empty() {
                    continue;
                }
                if next == start {
                    return true;
                }
                let states = reached.entry(next).or_default();
                if states.iter().any(|(existing, existing_feasible)| {
                    existing.is_subset_of(&next_condition) && *existing_feasible == feasible
                }) {
                    continue;
                }
                states.retain(|(existing, existing_feasible)| {
                    !next_condition.is_subset_of(existing) || *existing_feasible != feasible
                });
                states.push((next_condition.clone(), feasible.clone()));
                stack.push((next, next_condition, feasible));
            }
        }
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct FeasiblePosition {
    array: Option<(isize, isize)>,
    packed: Option<(isize, isize)>,
}

fn initial_feasible_positions(node: &GraphNode) -> Vec<FeasiblePosition> {
    if node.domains.is_empty() {
        return vec![FeasiblePosition {
            array: None,
            packed: None,
        }];
    }
    node.domains
        .iter()
        .filter_map(|domain| feasible_from_domain(*domain, BitDependency::identity()))
        .collect()
}

fn restrict_feasible_positions(
    current: &[FeasiblePosition],
    dependency: BitDependency,
    domains: &[PositionDomain],
) -> Vec<FeasiblePosition> {
    if domains.is_empty() {
        return current.to_vec();
    }
    let mut result = Vec::new();
    for &current in current {
        for &domain in domains {
            let Some(allowed) = feasible_from_domain(domain, dependency) else {
                continue;
            };
            let Some(array) = intersect_axis(current.array, allowed.array) else {
                continue;
            };
            let Some(packed) = intersect_axis(current.packed, allowed.packed) else {
                continue;
            };
            result.push(FeasiblePosition { array, packed });
        }
    }
    result.sort_unstable();
    result.dedup();
    result
}

fn feasible_from_domain(
    domain: PositionDomain,
    dependency: BitDependency,
) -> Option<FeasiblePosition> {
    Some(FeasiblePosition {
        array: inverse_translated_axis(domain.array_start, domain.array_length, dependency.array)?,
        packed: inverse_translated_axis(
            domain.packed_start,
            domain.packed_length,
            dependency.packed,
        )?,
    })
}

fn inverse_translated_axis(
    start: usize,
    length: usize,
    offset: Option<isize>,
) -> Option<Option<(isize, isize)>> {
    let Some(offset) = offset else {
        return Some(None);
    };
    let start = isize::try_from(start).ok()?.checked_sub(offset)?;
    let end = start.checked_add_unsigned(length)?;
    (start < end).then_some(Some((start, end)))
}

fn intersect_axis(
    left: Option<(isize, isize)>,
    right: Option<(isize, isize)>,
) -> Option<Option<(isize, isize)>> {
    match (left, right) {
        (None, right) | (right, None) => Some(right),
        (Some(left), Some(right)) => {
            let range = (left.0.max(right.0), left.1.min(right.1));
            (range.0 < range.1).then_some(Some(range))
        }
    }
}

fn dependency_may_return_to_same_position(dependency: BitDependency) -> bool {
    dependency.array.is_none_or(|offset| offset == 0)
        && dependency.packed.is_none_or(|offset| offset == 0)
}

fn build_error(module: &Module, keys: &[NodeKey]) -> Option<AnalyzerError> {
    let mut tokens: Vec<veryl_parser::token_range::TokenRange> = Vec::new();
    let mut identifier: Option<String> = None;
    let mut seen_var: HashSet<VarId> = HashSet::default();
    for (id, _idx, _range) in keys {
        if !seen_var.insert(*id) {
            continue;
        }
        if let Some(var) = module.variables.get(id)
            && identifier.is_none()
        {
            identifier = Some(var.path.to_string());
        }
        if let Some(toks) = module.assign_tokens.get(id) {
            tokens.extend(toks.iter().copied());
        } else if let Some(variable) = module.variables.get(id) {
            // Assignment coverage intentionally omits oversized arrays. Keep
            // a usable diagnostic site when the sparse graph still proves a
            // cycle through one of those variables.
            tokens.push(variable.token);
        }
    }
    {
        let mut seen: HashSet<_> = HashSet::default();
        tokens.retain(|token| seen.insert(*token));
    }
    let primary = *tokens.first()?;
    let participants: Vec<_> = tokens.iter().skip(1).copied().collect();
    Some(AnalyzerError::combinational_loop(
        identifier.as_deref().unwrap_or("?"),
        &primary,
        &participants,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::region::{ArraySpan, PackedSpan};
    use super::super::ssa::BranchId;
    use super::*;

    fn test_node(id: u32, region: ArraySpan) -> GraphNode {
        let id = VarId::from_raw(id);
        GraphNode {
            region: SummaryRegion {
                id,
                array: region,
                packed: PackedSpan::new(region.start, region.length).unwrap(),
            },
            domains: vec![PositionDomain {
                array_start: region.start,
                array_length: region.length,
                packed_start: region.start,
                packed_length: region.length,
            }],
            diagnostic: Some((id, region, 0)),
        }
    }

    #[test]
    fn coalesces_alternative_conditions_for_the_same_dependency() {
        let region = ArraySpan {
            start: 0,
            length: 1,
        };
        let mut graph = DependencyGraph::new();
        let source = graph.add_node(test_node(0, region));
        let destination = graph.add_node(test_node(1, region));
        let branch = BranchId::new(1, 0, 2);
        for arm in 0..2 {
            add_dependency_edge(
                &mut graph,
                source,
                destination,
                GraphDependency {
                    kind: BitDependency::WHOLE,
                    condition: PathCondition::default().with_choice(branch, arm),
                },
            );
        }

        assert_eq!(graph.edge_count(), 1);
        assert_eq!(
            graph.edge_weights().next().unwrap().condition,
            PathCondition::default()
        );
    }

    #[test]
    fn zero_offset_cycle_is_compatible() {
        let region = ArraySpan {
            start: 0,
            length: 1,
        };
        let mut graph = DependencyGraph::new();
        let a = graph.add_node(test_node(0, region));
        let b = graph.add_node(test_node(1, region));
        let identity = GraphDependency {
            kind: BitDependency {
                array: Some(0),
                packed: Some(0),
            },
            condition: PathCondition::default(),
        };
        graph.add_edge(a, b, identity.clone());
        graph.add_edge(b, a, identity);

        assert!(has_compatible_cycle(&graph, &[a, b]));
    }

    #[test]
    fn shifted_atoms_and_wraparound_form_a_cycle() {
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let mut graph = DependencyGraph::new();
        let mut node = |id, start, length| {
            let id = VarId::from_raw(id);
            graph.add_node(GraphNode {
                region: SummaryRegion {
                    id,
                    array,
                    packed: PackedSpan::new(start, length).unwrap(),
                },
                domains: vec![PositionDomain {
                    array_start: array.start,
                    array_length: array.length,
                    packed_start: start,
                    packed_length: length,
                }],
                diagnostic: Some((id, array, 0)),
            })
        };
        let low = node(0, 0, 1);
        let middle = node(0, 1, 6);
        let high = node(0, 7, 1);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };
        graph.add_edge(low, middle, edge(1));
        graph.add_edge(middle, middle, edge(1));
        graph.add_edge(middle, high, edge(1));
        graph.add_edge(high, low, edge(-7));

        assert!(has_compatible_cycle(&graph, &[low, middle, high]));
    }

    #[test]
    fn nonzero_composed_offset_is_not_a_cycle() {
        let region = ArraySpan {
            start: 0,
            length: 16,
        };
        let mut graph = DependencyGraph::new();
        let a = graph.add_node(test_node(0, region));
        let b = graph.add_node(test_node(1, region));
        graph.add_edge(
            a,
            b,
            GraphDependency {
                kind: BitDependency {
                    array: Some(0),
                    packed: Some(3),
                },
                condition: PathCondition::default(),
            },
        );
        graph.add_edge(
            b,
            a,
            GraphDependency {
                kind: BitDependency {
                    array: Some(0),
                    packed: Some(-1),
                },
                condition: PathCondition::default(),
            },
        );

        assert!(!has_compatible_cycle(&graph, &[a, b]));
    }

    #[test]
    fn parallel_cumulative_offsets_do_not_hide_a_zero_offset_cycle() {
        let region = ArraySpan {
            start: 0,
            length: 8,
        };
        let mut graph = DependencyGraph::new();
        let start = graph.add_node(test_node(0, region));
        let branch = graph.add_node(test_node(1, region));
        let join = graph.add_node(test_node(2, region));
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        graph.add_edge(start, branch, edge(0));
        graph.add_edge(branch, join, edge(1));
        graph.add_edge(branch, join, edge(2));
        graph.add_edge(join, start, edge(-2));

        assert!(has_compatible_cycle(&graph, &[start, branch, join]));
    }

    #[test]
    fn repeated_internal_shift_retains_a_zero_sum_walk() {
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let mut graph = DependencyGraph::new();
        let mut node = |id, start, length| {
            let id = VarId::from_raw(id);
            graph.add_node(GraphNode {
                region: SummaryRegion {
                    id,
                    array,
                    packed: PackedSpan::new(start, length).unwrap(),
                },
                domains: vec![PositionDomain {
                    array_start: 0,
                    array_length: 1,
                    packed_start: start,
                    packed_length: length,
                }],
                diagnostic: Some((id, array, 0)),
            })
        };
        let high = node(0, 5, 2);
        let low = node(1, 2, 3);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        // high[5] -> high[6] -> low[2] -> low[3] -> low[4] -> high[5]
        graph.add_edge(high, high, edge(1));
        graph.add_edge(high, low, edge(-4));
        graph.add_edge(low, low, edge(1));
        graph.add_edge(low, high, edge(1));

        assert!(has_compatible_cycle(&graph, &[high, low]));
    }

    #[test]
    fn repeated_internal_shift_is_sparse_at_scale() {
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let width = 1_000_000;
        let mut graph = DependencyGraph::new();
        let mut node = |id, start, length| {
            let id = VarId::from_raw(id);
            graph.add_node(GraphNode {
                region: SummaryRegion {
                    id,
                    array,
                    packed: PackedSpan::new(start, length).unwrap(),
                },
                domains: vec![PositionDomain {
                    array_start: 0,
                    array_length: 1,
                    packed_start: start,
                    packed_length: length,
                }],
                diagnostic: Some((id, array, 0)),
            })
        };
        let high = node(0, width - 2, 2);
        let low = node(1, 0, width - 2);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        graph.add_edge(high, high, edge(1));
        graph.add_edge(high, low, edge(-((width - 1) as isize)));
        graph.add_edge(low, low, edge(1));
        graph.add_edge(low, high, edge(1));

        assert!(has_compatible_cycle(&graph, &[high, low]));
    }

    #[test]
    fn repeated_internal_shift_with_no_return_is_sparse_at_scale() {
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let width = 1_000_000;
        let mut graph = DependencyGraph::new();
        let mut node = |id, start, length| {
            let id = VarId::from_raw(id);
            graph.add_node(GraphNode {
                region: SummaryRegion {
                    id,
                    array,
                    packed: PackedSpan::new(start, length).unwrap(),
                },
                domains: vec![PositionDomain {
                    array_start: 0,
                    array_length: 1,
                    packed_start: start,
                    packed_length: length,
                }],
                diagnostic: Some((id, array, 0)),
            })
        };
        let high = node(0, width - 2, 2);
        let low = node(1, 0, width - 2);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        graph.add_edge(high, high, edge(1));
        graph.add_edge(high, low, edge(-((width - 1) as isize)));
        graph.add_edge(low, low, edge(1));
        // This edge makes the coarse graph strongly connected, but its
        // source and destination domains are disjoint.
        graph.add_edge(low, high, edge(0));

        assert!(!has_compatible_cycle(&graph, &[high, low]));
    }

    #[test]
    fn opposing_cycle_displacements_with_disjoint_guards_do_not_close() {
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let mut graph = DependencyGraph::new();
        let mut node = |id, start, length| {
            let id = VarId::from_raw(id);
            graph.add_node(GraphNode {
                region: SummaryRegion {
                    id,
                    array,
                    packed: PackedSpan::new(0, 3).unwrap(),
                },
                domains: vec![PositionDomain {
                    array_start: 0,
                    array_length: 1,
                    packed_start: start,
                    packed_length: length,
                }],
                diagnostic: Some((id, array, 0)),
            })
        };
        let anchor = node(0, 0, 3);
        let plus_guard = node(1, 1, 1);
        let minus_guard = node(2, 1, 1);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        // The +1 walk is feasible only from anchor[0], and the -1 walk only
        // from anchor[2]. Both finish at anchor[1], where the other walk is
        // disabled, so their displacements cannot be concatenated.
        graph.add_edge(anchor, plus_guard, edge(1));
        graph.add_edge(plus_guard, anchor, edge(0));
        graph.add_edge(anchor, minus_guard, edge(-1));
        graph.add_edge(minus_guard, anchor, edge(0));

        assert!(!has_compatible_cycle(
            &graph,
            &[anchor, plus_guard, minus_guard]
        ));
    }

    #[test]
    fn guarded_opposing_displacements_close_without_enumerating_the_width() {
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let width = 1_000_000;
        let mut graph = DependencyGraph::new();
        let mut node = |id, start, length| {
            let id = VarId::from_raw(id);
            graph.add_node(GraphNode {
                region: SummaryRegion {
                    id,
                    array,
                    packed: PackedSpan::new(0, width).unwrap(),
                },
                domains: vec![PositionDomain {
                    array_start: 0,
                    array_length: 1,
                    packed_start: start,
                    packed_length: length,
                }],
                diagnostic: Some((id, array, 0)),
            })
        };
        let anchor = node(0, 0, width);
        let increment = node(1, 1, width - 1);
        let wrap = node(2, 0, 1);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        graph.add_edge(anchor, increment, edge(1));
        graph.add_edge(increment, anchor, edge(0));
        graph.add_edge(anchor, wrap, edge(-((width - 1) as isize)));
        graph.add_edge(wrap, anchor, edge(0));

        assert!(has_compatible_cycle(&graph, &[anchor, increment, wrap]));
    }

    #[test]
    fn opposing_cycle_displacements_can_close_a_repeated_walk() {
        let condition = PathCondition::default();
        let cycles = [
            (
                BitDependency {
                    array: Some(0),
                    packed: Some(1),
                },
                condition.clone(),
            ),
            (
                BitDependency {
                    array: Some(0),
                    packed: Some(-7),
                },
                condition,
            ),
        ]
        .into_iter()
        .collect();

        assert!(compatible_cycle_displacements_cancel(&cycles));
    }

    #[test]
    fn nonzero_cycle_displacements_in_one_half_plane_cannot_close() {
        let condition = PathCondition::default();
        let cycles = [
            (
                BitDependency {
                    array: Some(1),
                    packed: Some(0),
                },
                condition.clone(),
            ),
            (
                BitDependency {
                    array: Some(0),
                    packed: Some(1),
                },
                condition,
            ),
        ]
        .into_iter()
        .collect();

        assert!(!compatible_cycle_displacements_cancel(&cycles));
    }

    #[test]
    fn three_cycle_displacements_can_close_in_two_dimensions() {
        let condition = PathCondition::default();
        let cycles = [
            (
                BitDependency {
                    array: Some(1),
                    packed: Some(0),
                },
                condition.clone(),
            ),
            (
                BitDependency {
                    array: Some(0),
                    packed: Some(1),
                },
                condition.clone(),
            ),
            (
                BitDependency {
                    array: Some(-1),
                    packed: Some(-1),
                },
                condition,
            ),
        ]
        .into_iter()
        .collect();

        assert!(compatible_cycle_displacements_cancel(&cycles));
    }

    #[test]
    fn positional_cycle_detection_matches_small_expanded_graphs() {
        use daggy::petgraph::algo::is_cyclic_directed;

        let mut state = 0x9e37_79b9_u32;
        let mut random = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        for case in 0..100_000 {
            let node_count = 2 + random() as usize % 3;
            let width = 2 + random() as usize % 5;
            let array = ArraySpan {
                start: 0,
                length: 1,
            };
            let mut graph = DependencyGraph::new();
            let mut domain_specs = Vec::new();
            let nodes = (0..node_count)
                .map(|id| {
                    let domain = if case % 2 == 0 {
                        Some((0, width))
                    } else {
                        let start = random() as usize % width;
                        Some((start, 1 + random() as usize % (width - start)))
                    };
                    domain_specs.push(domain);
                    let id = VarId::from_raw(id as u32);
                    graph.add_node(GraphNode {
                        region: SummaryRegion {
                            id,
                            array,
                            packed: PackedSpan::new(0, width).unwrap(),
                        },
                        domains: domain
                            .map(|(start, length)| PositionDomain {
                                array_start: array.start,
                                array_length: array.length,
                                packed_start: start,
                                packed_length: length,
                            })
                            .into_iter()
                            .collect(),
                        diagnostic: Some((id, array, 0)),
                    })
                })
                .collect::<Vec<_>>();
            let edge_count = 1 + random() as usize % (node_count * node_count * 2);
            let mut edge_specs = Vec::new();
            let branch = BranchId::new(case + 1, 0, 2);
            for _ in 0..edge_count {
                let source = random() as usize % node_count;
                let destination = random() as usize % node_count;
                let packed = random() as isize % 7 - 3;
                let arm = match random() % 3 {
                    0 => None,
                    arm => Some(arm as usize - 1),
                };
                let dependency = BitDependency {
                    array: Some(0),
                    packed: Some(packed),
                };
                if node_regions_overlap_with_dependency(
                    &graph[nodes[source]],
                    &graph[nodes[destination]],
                    dependency,
                ) {
                    edge_specs.push((source, destination, dependency, arm));
                    add_dependency_edge(
                        &mut graph,
                        nodes[source],
                        nodes[destination],
                        GraphDependency {
                            kind: dependency,
                            condition: arm.map_or_else(PathCondition::default, |arm| {
                                PathCondition::default().with_choice(branch, arm)
                            }),
                        },
                    );
                }
            }

            let coarse = tarjan_scc(&graph.graph)
                .iter()
                .any(|scc| has_compatible_cycle(&graph, scc));
            let mut exact = false;
            for selected_arm in 0..2 {
                let mut expanded = Graph::<(), ()>::new();
                let expanded_nodes = (0..node_count)
                    .map(|_| {
                        (0..width)
                            .map(|_| expanded.add_node(()))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                for (source, destination, dependency, arm) in &edge_specs {
                    if arm.is_some_and(|arm| arm != selected_arm) {
                        continue;
                    }
                    for position in 0..width {
                        let source_allowed = domain_specs[*source].is_none_or(|(start, length)| {
                            (start..start + length).contains(&position)
                        });
                        let Some(mapped) = translate_position(position, dependency.packed.unwrap())
                        else {
                            continue;
                        };
                        let destination_allowed =
                            domain_specs[*destination].is_none_or(|(start, length)| {
                                (start..start + length).contains(&mapped)
                            });
                        if source_allowed && mapped < width && destination_allowed {
                            expanded.add_edge(
                                expanded_nodes[*source][position],
                                expanded_nodes[*destination][mapped],
                                (),
                            );
                        }
                    }
                }
                exact |= is_cyclic_directed(&expanded);
            }
            assert_eq!(
                coarse, exact,
                "case {case}, width {width}, edges {edge_specs:?}"
            );
        }
    }
}
