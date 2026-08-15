//! Dependency graph storage, edge normalization, and cycle detection.

use super::model::BitDependency;
use super::region::{BitPartition, NodeKey, translate_position};
use super::ssa::PathCondition;
use crate::ir::{Module, VarId};
use crate::{AnalyzerError, HashMap, HashSet};
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::{EdgeIndex, NodeIndex};
use daggy::petgraph::visit::EdgeRef;
use std::ops::{Deref, DerefMut};

#[derive(Clone, Debug)]
pub(super) struct GraphDependency {
    pub(super) kind: BitDependency,
    pub(super) condition: PathCondition,
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
    graph: Graph<NodeKey, GraphDependency>,
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
    type Target = Graph<NodeKey, GraphDependency>;

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
    source: NodeKey,
    destination: NodeKey,
    dependency: GraphDependency,
) {
    let source = ensure_node(graph, node_map, source);
    let destination = ensure_node(graph, node_map, destination);
    add_dependency_edge(graph, source, destination, dependency);
}

pub(super) fn ensure_node(
    graph: &mut DependencyGraph,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    key: NodeKey,
) -> NodeIndex {
    *node_map.entry(key).or_insert_with(|| graph.add_node(key))
}

pub(super) fn node_regions_overlap_with_dependency(
    source: NodeKey,
    destination: NodeKey,
    dependency: BitDependency,
    bit_part: &BitPartition,
) -> bool {
    let Some(source_packed) = bit_part.ranges_of((source.0, source.1)).get(source.2) else {
        return false;
    };
    let Some(destination_packed) = bit_part
        .ranges_of((destination.0, destination.1))
        .get(destination.2)
    else {
        return false;
    };
    dependency.array.is_none_or(|array| {
        spans_overlap_with_offset(
            source.1.start,
            source.1.length,
            destination.1.start,
            destination.1.length,
            array,
        )
    }) && dependency.packed.is_none_or(|packed| {
        spans_overlap_with_offset(
            source_packed.start,
            source_packed.length,
            destination_packed.start,
            destination_packed.length,
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
    let sccs = tarjan_scc(&graph.graph);
    let mut reported: HashSet<Vec<NodeKey>> = HashSet::default();
    for scc in sccs {
        if !has_compatible_cycle(graph, &scc) {
            continue;
        }
        let mut keys: Vec<NodeKey> = scc.iter().map(|node| graph[*node]).collect();
        keys.sort();
        if !reported.insert(keys.clone()) {
            continue;
        }
        if let Some(error) = build_error(module, &keys) {
            errors.push(error);
        }
    }
}

fn has_compatible_cycle(graph: &DependencyGraph, scc: &[NodeIndex]) -> bool {
    let nodes: HashSet<_> = scc.iter().copied().collect();
    let mut cycle_displacements = HashSet::default();
    for &start in scc {
        let identity = BitDependency {
            array: Some(0),
            packed: Some(0),
        };
        let mut stack = vec![(start, PathCondition::default(), identity, 0b1111)];
        let mut reached: HashMap<NodeIndex, Vec<(PathCondition, u8)>> = HashMap::default();
        while let Some((node, condition, dependency, directions)) = stack.pop() {
            for edge in graph.edges(node) {
                let next = edge.target();
                if !nodes.contains(&next) {
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
                let next_directions = directions & dependency_direction_mask(edge.weight().kind);
                if next == start {
                    if dependency_may_return_to_same_position(next_dependency) {
                        return true;
                    }
                    if next_dependency.exact_offset().is_none() {
                        return true;
                    }
                    cycle_displacements.insert((next_dependency, next_condition));
                    continue;
                }
                let states = reached.entry(next).or_default();
                if states
                    .iter()
                    .any(|(existing_condition, existing_directions)| {
                        existing_condition.is_subset_of(&next_condition)
                            && existing_directions & next_directions == *existing_directions
                    })
                {
                    continue;
                }
                states.retain(|(existing_condition, existing_directions)| {
                    !(next_condition.is_subset_of(existing_condition)
                        && next_directions & *existing_directions == next_directions)
                });
                states.push((next_condition.clone(), next_directions));
                stack.push((next, next_condition, next_dependency, next_directions));
            }
        }
    }
    compatible_cycle_displacements_cancel(&cycle_displacements)
}

fn dependency_may_return_to_same_position(dependency: BitDependency) -> bool {
    dependency.array.is_none_or(|offset| offset == 0)
        && dependency.packed.is_none_or(|offset| offset == 0)
}

fn dependency_direction_mask(dependency: BitDependency) -> u8 {
    let Some((array, packed)) = dependency.exact_offset() else {
        return 0;
    };
    u8::from(array > 0)
        | (u8::from(array < 0) << 1)
        | (u8::from(packed > 0) << 2)
        | (u8::from(packed < 0) << 3)
}

fn compatible_cycle_displacements_cancel(cycles: &HashSet<(BitDependency, PathCondition)>) -> bool {
    // A closed walk is a non-negative combination of its coarse cycles. In
    // two positional dimensions, a zero displacement needs at most three
    // cycle vectors. Keep the associated path conditions so mutually
    // exclusive cycles are never combined.
    let exact: Vec<_> = cycles
        .iter()
        .filter_map(|(dependency, condition)| {
            dependency.exact_offset().map(|offset| (offset, condition))
        })
        .collect();

    for left in 0..exact.len() {
        for right in (left + 1)..exact.len() {
            let Some(condition) = exact[left].1.union_if_compatible(exact[right].1) else {
                continue;
            };
            if opposite_collinear(exact[left].0, exact[right].0) {
                return true;
            }
            for third in (right + 1)..exact.len() {
                if condition.union_if_compatible(exact[third].1).is_some()
                    && origin_is_in_positive_cone(exact[left].0, exact[right].0, exact[third].0)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn opposite_collinear(left: (isize, isize), right: (isize, isize)) -> bool {
    let Some(cross) = cross_product(left, right) else {
        return true;
    };
    let Some(dot) = dot_product(left, right) else {
        return true;
    };
    cross == 0 && dot < 0
}

fn origin_is_in_positive_cone(a: (isize, isize), b: (isize, isize), c: (isize, isize)) -> bool {
    let [Some(first), Some(second), Some(third)] = [
        cross_product(b, c),
        cross_product(c, a),
        cross_product(a, b),
    ] else {
        return true;
    };
    let coefficients = [first, second, third];
    let has_positive = coefficients.iter().any(|&coefficient| coefficient > 0);
    let has_negative = coefficients.iter().any(|&coefficient| coefficient < 0);
    (has_positive || has_negative) && !(has_positive && has_negative)
}

fn cross_product(left: (isize, isize), right: (isize, isize)) -> Option<isize> {
    left.0
        .checked_mul(right.1)?
        .checked_sub(left.1.checked_mul(right.0)?)
}

fn dot_product(left: (isize, isize), right: (isize, isize)) -> Option<isize> {
    left.0
        .checked_mul(right.0)?
        .checked_add(left.1.checked_mul(right.1)?)
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
    use super::super::region::ArraySpan;
    use super::super::ssa::BranchId;
    use super::*;

    #[test]
    fn coalesces_alternative_conditions_for_the_same_dependency() {
        let region = ArraySpan {
            start: 0,
            length: 1,
        };
        let mut graph = DependencyGraph::new();
        let source = graph.add_node((VarId::from_raw(0), region, 0));
        let destination = graph.add_node((VarId::from_raw(1), region, 0));
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
        let a = graph.add_node((VarId::from_raw(0), region, 0));
        let b = graph.add_node((VarId::from_raw(1), region, 0));
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
    fn nonzero_composed_offset_is_not_a_cycle() {
        let region = ArraySpan {
            start: 0,
            length: 16,
        };
        let mut graph = DependencyGraph::new();
        let a = graph.add_node((VarId::from_raw(0), region, 0));
        let b = graph.add_node((VarId::from_raw(1), region, 0));
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
}
