//! Dependency graph storage, edge normalization, and cycle detection.

mod guarded;
mod relation;

use super::model::{BitDependency, SummaryRegion};
#[cfg(test)]
use super::region::translate_position;
use super::region::{BitPartition, NodeKey};
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
use relation::PositionRelationSet;
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
    let Some(source_end) = source_start.checked_add(source_length) else {
        return false;
    };
    let Some(destination_end) = destination_start.checked_add(destination_length) else {
        return false;
    };
    if offset >= 0 {
        let offset = offset.unsigned_abs();
        let (Some(source_start), Some(source_end)) = (
            source_start.checked_add(offset),
            source_end.checked_add(offset),
        ) else {
            return false;
        };
        source_start < destination_end && destination_start < source_end
    } else {
        // `source + offset` overlaps `destination` iff `source` overlaps
        // `destination - offset`. Shift the destination in the non-negative
        // direction so a valid source suffix is not lost when source_start +
        // offset would be negative.
        let offset = offset.unsigned_abs();
        let (Some(destination_start), Some(destination_end)) = (
            destination_start.checked_add(offset),
            destination_end.checked_add(offset),
        ) else {
            return false;
        };
        source_start < destination_end && destination_start < source_end
    }
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

// Correctness argument for the graph-relative cycle decision:
//
// Interpret a graph state as `(node, array_position, packed_position)`. An edge
// with `Some(k)` maps a coordinate to `coordinate + k`; `None` relates every
// source coordinate to every coordinate in the destination domain. Interpret a
// `PathCondition` as its stored Cartesian set of branch choices. Correlations
// discarded before graph construction are deliberately not reintroduced here.
//
// For a fixed anchor, each stack state denotes one real path that has not
// revisited the anchor: its condition is the conjunction of the edge
// conditions, and its `PositionRelationSet` is exactly the relation from the
// anchor position to the current position. This holds initially by `identity`
// and is preserved by `then_dependency`. Reverse reachability removes no path
// that can return to the anchor. If an existing state has a superset relation
// under a weaker condition, every continuation of the new state is also a
// continuation of the existing state: relation composition is monotone and
// every valuation admitted by the new condition is admitted by the existing
// one. The dominance pruning is therefore lossless. The identity-edge search
// is the same invariant specialized to zero translations.
//
// Once an anchor occurrence is fixed, cutting a closed walk at every later
// occurrence of that anchor uniquely decomposes it into first-return walks.
// The search records every such relation, or a real relation/condition state
// that covers it as above. Conversely, compatible recorded relations whose
// composition intersects identity concatenate to a real closed walk. The
// guarded-composition argument in `guarded` proves that it finds exactly such
// sequences. Trying every node as anchor therefore proves that this function
// returns true exactly when the SCC contains a branch-compatible closed walk
// that returns to the same array and packed position.
//
// Termination assumes the builder invariant asserted by
// `unconstrained_subgraph_is_acyclic` in debug builds. An empty-domain-only
// path has bounded length, so every repeatable path visits a finite domain. An exact
// first-return relation cannot contain a `None` dependency: once an axis is
// `Unlinked`, later composition never links it again. Its finite-domain visit
// therefore restricts its starting positions to finite ranges. In any feasible
// exact word, the cumulative displacement is the difference between positions
// in its first and last finite guards, so only finitely many displacements and
// interval endpoints are reachable. A mixed word can be rotated to begin with
// an `Unlinked` relation; its endpoints come only from finite domain boundaries
// and those finite exact displacements. Thus only finitely many normalized
// relation states are reachable. Branches and arms are finite as well, and a
// dominated state is never queued again. Every endpoint and intermediate
// offset operation is a construction invariant required to be representable
// in `isize`; overflow is not interpreted as a dependency relation.
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
        let initial = PositionRelationSet::identity(&graph[start].domains);
        let mut cycles = HashSet::default();
        let mut stack = vec![(start, PathCondition::default(), initial)];
        let mut reached: HashMap<NodeIndex, Vec<(PositionRelationSet, PathCondition)>> =
            HashMap::default();
        while let Some((node, condition, relation)) = stack.pop() {
            for edge in graph.edges(node) {
                let next = edge.target();
                if !returnable.contains(&next) {
                    continue;
                }
                let Some(next_condition) =
                    condition.conjoin_if_compatible(&edge.weight().condition)
                else {
                    continue;
                };
                // Retain the full binary relation from the anchor position to
                // the current position. Unlike a single optional offset, this
                // preserves the reachable current range after a WHOLE edge.
                let next_relation =
                    relation.then_dependency(edge.weight().kind, &graph[next].domains);
                if next_relation.is_empty() {
                    continue;
                }
                if next == start {
                    if next_relation.intersects_identity() {
                        return true;
                    }
                    cycles.insert(GuardedCycle {
                        relation: next_relation,
                        condition: next_condition,
                    });
                    continue;
                }
                let states = reached.entry(next).or_default();
                if states
                    .iter()
                    .any(|(existing_relation, existing_condition)| {
                        existing_relation.piecewise_covers(&next_relation)
                            && existing_condition.covers(&next_condition)
                    })
                {
                    continue;
                }
                states.retain(|(existing_relation, existing_condition)| {
                    !next_relation.piecewise_covers(existing_relation)
                        || !next_condition.covers(existing_condition)
                });
                states.push((next_relation.clone(), next_condition.clone()));
                stack.push((next, next_condition, next_relation));
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
                if !dependency_is_identity(edge.weight().kind) {
                    continue;
                }
                let next = edge.target();
                if !nodes.contains(&next) {
                    continue;
                }
                let Some(next_condition) =
                    condition.conjoin_if_compatible(&edge.weight().condition)
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
                    existing.covers(&next_condition) && *existing_feasible == feasible
                }) {
                    continue;
                }
                states.retain(|(existing, existing_feasible)| {
                    !next_condition.covers(existing) || *existing_feasible != feasible
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
    let start = isize::try_from(start)
        .expect("position domain start must fit in isize")
        .checked_sub(offset)
        .expect("translated position start must fit in isize");
    let end = start
        .checked_add_unsigned(length)
        .expect("translated position end must fit in isize");
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

fn dependency_is_identity(dependency: BitDependency) -> bool {
    dependency.array == Some(0) && dependency.packed == Some(0)
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
    fn negative_offset_can_map_a_source_suffix_into_the_destination() {
        let source = test_node(
            0,
            ArraySpan {
                start: 0,
                length: 8,
            },
        );
        let destination = test_node(
            1,
            ArraySpan {
                start: 0,
                length: 4,
            },
        );

        assert!(node_regions_overlap_with_dependency(
            &source,
            &destination,
            BitDependency {
                array: Some(-4),
                packed: Some(-4),
            },
        ));
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
    fn whole_dependency_retains_the_reachable_current_range() {
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let mut graph = DependencyGraph::new();
        let mut node = |id| {
            let id = VarId::from_raw(id);
            graph.add_node(GraphNode {
                region: SummaryRegion {
                    id,
                    array,
                    packed: PackedSpan::new(0, 5).unwrap(),
                },
                domains: vec![PositionDomain {
                    array_start: 0,
                    array_length: 1,
                    packed_start: 0,
                    packed_length: 5,
                }],
                diagnostic: Some((id, array, 0)),
            })
        };
        let a = node(0);
        let b = node(1);
        let c = node(2);
        let d = node(3);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        // The exact prefix is feasible only from a[0]. WHOLE can then reach
        // any d bit, but the final +2 edge can return only to a[2..5], never
        // to the original a[0].
        graph.add_edge(a, b, edge(3));
        graph.add_edge(b, c, edge(1));
        graph.add_edge(c, d, GraphDependency::unconditional(BitDependency::WHOLE));
        graph.add_edge(d, a, edge(2));

        assert!(!has_compatible_cycle(&graph, &[a, b, c, d]));
    }

    #[test]
    fn whole_dependency_composes_with_a_later_translation() {
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
        let low = node(1, 0, 1);
        let high = node(2, 2, 1);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        // One first-return path maps anchor[0] to anchor[2] through WHOLE;
        // the other maps anchor[2] back to anchor[0]. Neither closes alone.
        graph.add_edge(anchor, low, edge(0));
        graph.add_edge(
            low,
            high,
            GraphDependency::unconditional(BitDependency::WHOLE),
        );
        graph.add_edge(high, anchor, edge(0));
        graph.add_edge(anchor, anchor, edge(-2));

        assert!(has_compatible_cycle(&graph, &[anchor, low, high]));
    }

    #[test]
    fn whole_dependency_and_repeated_shift_are_sparse_at_scale() {
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
        let low = node(1, 0, 1);
        let high = node(2, width - 1, 1);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        // WHOLE maps anchor[0] to anchor[width - 1]. Repeating the -1
        // translation closes the walk without enumerating every position.
        graph.add_edge(anchor, low, edge(0));
        graph.add_edge(
            low,
            high,
            GraphDependency::unconditional(BitDependency::WHOLE),
        );
        graph.add_edge(high, anchor, edge(0));
        graph.add_edge(anchor, anchor, edge(-1));

        assert!(has_compatible_cycle(&graph, &[anchor, low, high]));
    }

    #[test]
    fn whole_dependency_and_diverging_shift_are_sparse_at_scale() {
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
        let low = node(1, 0, 1);
        let high = node(2, width - 1, 1);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        graph.add_edge(anchor, low, edge(0));
        graph.add_edge(
            low,
            high,
            GraphDependency::unconditional(BitDependency::WHOLE),
        );
        graph.add_edge(high, anchor, edge(0));
        graph.add_edge(anchor, anchor, edge(1));

        assert!(!has_compatible_cycle(&graph, &[anchor, low, high]));
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
    fn opposing_displacements_separated_by_a_gap_are_sparse_at_scale() {
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let width = 1_000_000;
        let middle = width / 2;
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
        let plus_guard = node(1, 1, middle);
        let minus_guard = node(2, middle, width - middle - 1);
        let edge = |packed| {
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(packed),
            })
        };

        // +1 starts below `middle`; -1 starts above it. Both can finish at
        // `middle`, where neither relation can start, so no word can switch
        // from one displacement direction to the other.
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
                let raw_dependency = random();
                let packed = (raw_dependency % 4 != 0).then_some(raw_dependency as isize % 7 - 3);
                let arm = match random() % 3 {
                    0 => None,
                    arm => Some(arm as usize - 1),
                };
                let dependency = BitDependency {
                    array: Some(0),
                    packed,
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
                        if !source_allowed {
                            continue;
                        }
                        let mapped = match dependency.packed {
                            Some(offset) => translate_position(position, offset)
                                .filter(|&mapped| mapped < width)
                                .into_iter()
                                .collect::<Vec<_>>(),
                            None => (0..width).collect(),
                        };
                        for mapped in mapped {
                            let destination_allowed =
                                domain_specs[*destination].is_none_or(|(start, length)| {
                                    (start..start + length).contains(&mapped)
                                });
                            if destination_allowed {
                                expanded.add_edge(
                                    expanded_nodes[*source][position],
                                    expanded_nodes[*destination][mapped],
                                    (),
                                );
                            }
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

    #[test]
    fn positional_cycle_detection_matches_two_dimensional_expansion() {
        use daggy::petgraph::algo::is_cyclic_directed;

        let mut state = 0x243f_6a88_u32;
        let mut random = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        for case in 0..20_000 {
            let node_count = 2 + random() as usize % 3;
            let array_width = 2 + random() as usize % 3;
            let packed_width = 2 + random() as usize % 3;
            let array = ArraySpan {
                start: 0,
                length: array_width,
            };
            let mut graph = DependencyGraph::new();
            let mut domain_specs = Vec::new();
            let nodes = (0..node_count)
                .map(|id| {
                    let domain = if case % 2 == 0 {
                        (0, array_width, 0, packed_width)
                    } else {
                        let array_start = random() as usize % array_width;
                        let packed_start = random() as usize % packed_width;
                        (
                            array_start,
                            1 + random() as usize % (array_width - array_start),
                            packed_start,
                            1 + random() as usize % (packed_width - packed_start),
                        )
                    };
                    domain_specs.push(domain);
                    let id = VarId::from_raw(id as u32);
                    graph.add_node(GraphNode {
                        region: SummaryRegion {
                            id,
                            array,
                            packed: PackedSpan::new(0, packed_width).unwrap(),
                        },
                        domains: vec![PositionDomain {
                            array_start: domain.0,
                            array_length: domain.1,
                            packed_start: domain.2,
                            packed_length: domain.3,
                        }],
                        diagnostic: Some((id, array, 0)),
                    })
                })
                .collect::<Vec<_>>();
            let edge_count = 1 + random() as usize % (node_count * node_count * 2);
            let branch = BranchId::new(case + 100_001, 0, 2);
            let mut edge_specs = Vec::new();
            for _ in 0..edge_count {
                let source = random() as usize % node_count;
                let destination = random() as usize % node_count;
                let raw_array = random();
                let raw_packed = random();
                let dependency = BitDependency {
                    array: (raw_array % 4 != 0).then_some(raw_array as isize % 5 - 2),
                    packed: (raw_packed % 4 != 0).then_some(raw_packed as isize % 5 - 2),
                };
                let arm = match random() % 3 {
                    0 => None,
                    arm => Some(arm as usize - 1),
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

            let symbolic = tarjan_scc(&graph.graph)
                .iter()
                .any(|scc| has_compatible_cycle(&graph, scc));
            let mut concrete = false;
            for selected_arm in 0..2 {
                let mut expanded = Graph::<(), ()>::new();
                let expanded_nodes = (0..node_count)
                    .map(|_| {
                        (0..array_width * packed_width)
                            .map(|_| expanded.add_node(()))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                for (source, destination, dependency, arm) in &edge_specs {
                    if arm.is_some_and(|arm| arm != selected_arm) {
                        continue;
                    }
                    let source_domain = domain_specs[*source];
                    let destination_domain = domain_specs[*destination];
                    for source_array in 0..array_width {
                        for source_packed in 0..packed_width {
                            if !(source_domain.0..source_domain.0 + source_domain.1)
                                .contains(&source_array)
                                || !(source_domain.2..source_domain.2 + source_domain.3)
                                    .contains(&source_packed)
                            {
                                continue;
                            }
                            let destination_arrays =
                                mapped_positions(source_array, dependency.array, array_width);
                            let destination_packeds =
                                mapped_positions(source_packed, dependency.packed, packed_width);
                            for destination_array in &destination_arrays {
                                for destination_packed in &destination_packeds {
                                    if !(destination_domain.0
                                        ..destination_domain.0 + destination_domain.1)
                                        .contains(destination_array)
                                        || !(destination_domain.2
                                            ..destination_domain.2 + destination_domain.3)
                                            .contains(destination_packed)
                                    {
                                        continue;
                                    }
                                    let source_position =
                                        source_array * packed_width + source_packed;
                                    let destination_position =
                                        destination_array * packed_width + destination_packed;
                                    expanded.add_edge(
                                        expanded_nodes[*source][source_position],
                                        expanded_nodes[*destination][destination_position],
                                        (),
                                    );
                                }
                            }
                        }
                    }
                }
                concrete |= is_cyclic_directed(&expanded);
            }
            assert_eq!(
                symbolic, concrete,
                "case {case}, shape [{array_width}, {packed_width}], domains {domain_specs:?}, edges {edge_specs:?}"
            );
        }
    }

    fn mapped_positions(position: usize, offset: Option<isize>, width: usize) -> Vec<usize> {
        match offset {
            Some(offset) => translate_position(position, offset)
                .filter(|&position| position < width)
                .into_iter()
                .collect(),
            None => (0..width).collect(),
        }
    }
}
