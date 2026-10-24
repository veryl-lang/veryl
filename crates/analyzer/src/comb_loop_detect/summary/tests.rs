use super::*;
use crate::comb_loop_detect::graph::{GraphDependency, GraphNode};
use crate::comb_loop_detect::model::SummaryRegion;
use crate::comb_loop_detect::region::{ArraySpan, PackedSpan};
use crate::comb_loop_detect::ssa::{BranchId, PathCondition, PositionDomain};
use crate::ir::VarId;

fn node(graph: &mut DependencyGraph) -> NodeIndex {
    let id = VarId::from_raw(graph.node_count() as u32);
    graph.add_node(GraphNode {
        region: SummaryRegion {
            id,
            array: ArraySpan {
                start: 0,
                length: 1,
            },
            packed: PackedSpan::new(0, 16).unwrap(),
        },
        domains: Vec::new(),
        diagnostic: None,
    })
}

fn wire(graph: &mut DependencyGraph, source: NodeIndex, destination: NodeIndex) {
    graph.add_edge(
        source,
        destination,
        GraphDependency::unconditional(BitDependency::identity()),
    );
}

fn summary(graph: &DependencyGraph, input: NodeIndex, output: NodeIndex) -> ModuleCombSummary {
    reset_module_summary_work();
    let result = summarize_graph(graph, |node| {
        if node == input {
            SummaryNodeKind::Input
        } else if node == output {
            SummaryNodeKind::Output
        } else {
            SummaryNodeKind::Internal
        }
    });
    let (input_edges, walked_edges) = module_summary_work();
    assert_eq!(input_edges, graph.edge_count());
    assert!(
        walked_edges <= input_edges,
        "{walked_edges} > {input_edges}"
    );
    assert!(result.nodes.len() <= graph.node_count());
    assert!(result.edges.len() <= graph.edge_count());
    assert!(result.complete);
    result
}

#[test]
fn discarded_cycles_and_exponential_dags_cannot_be_entered_from_a_series_node() {
    for cycle in [false, true] {
        let mut graph = DependencyGraph::new();
        let input = node(&mut graph);
        let middle = node(&mut graph);
        let output = node(&mut graph);
        wire(&mut graph, input, middle);
        wire(&mut graph, middle, output);
        let first = node(&mut graph);
        wire(&mut graph, middle, first);
        let mut previous = first;
        for bit in 0..32 {
            let next = node(&mut graph);
            wire(&mut graph, previous, next);
            graph.add_edge(
                previous,
                next,
                GraphDependency::unconditional(BitDependency {
                    array: Some(0),
                    packed: Some(1isize << bit),
                }),
            );
            previous = next;
        }
        if cycle {
            wire(&mut graph, previous, first);
        }
        let result = summary(&graph, input, output);
        assert_eq!(result.nodes.len(), 2);
        assert_eq!(result.edges.len(), 1);
        assert_eq!(result.edges[0].kind, BitDependency::identity());
        assert_eq!(module_summary_work().1, 2);
    }
}

#[test]
fn connected_diamonds_keep_exponentially_many_paths_as_a_linear_graph() {
    let mut graph = DependencyGraph::new();
    let input = node(&mut graph);
    let mut previous = input;
    for index in 0..2048 {
        let left = node(&mut graph);
        let right = node(&mut graph);
        let join = node(&mut graph);
        for (arm, destination) in [(0, left), (1, right)] {
            graph.add_edge(
                previous,
                destination,
                GraphDependency {
                    kind: BitDependency {
                        array: Some(0),
                        packed: Some(arm as isize),
                    },
                    condition: PathCondition::default()
                        .with_choice(BranchId::new(0, index, 2), arm),
                },
            );
            wire(&mut graph, destination, join);
        }
        previous = join;
    }
    let result = summary(&graph, input, previous);
    assert_eq!(module_summary_work().1, graph.edge_count());
    assert!(
        result
            .edges
            .iter()
            .all(|edge| edge.condition.branch_count() <= 1)
    );
}

#[test]
fn long_guarded_chains_do_not_accumulate_quadratic_condition_payloads() {
    const LENGTH: usize = 10_000;
    let mut graph = DependencyGraph::new();
    let input = node(&mut graph);
    let mut previous = input;
    for index in 0..LENGTH {
        let guarded = node(&mut graph);
        let next = node(&mut graph);
        graph.add_edge(
            previous,
            guarded,
            GraphDependency {
                kind: BitDependency::identity(),
                condition: PathCondition::default().with_choice(BranchId::new(0, index, 2), 0),
            },
        );
        wire(&mut graph, guarded, next);
        previous = next;
    }
    let result = summary(&graph, input, previous);
    assert_eq!(result.nodes.len(), LENGTH + 1);
    assert_eq!(result.edges.len(), LENGTH);
    assert!(
        result
            .edges
            .iter()
            .all(|edge| edge.condition.branch_count() == 1)
    );
    assert_eq!(module_summary_work().1, graph.edge_count());
}

#[test]
fn positional_operations_are_not_composed_through_overflowing_prefixes() {
    let mut graph = DependencyGraph::new();
    let input = node(&mut graph);
    let mut previous = input;
    let offsets = [isize::MAX, 1, -isize::MAX, -1];
    for offset in offsets {
        let next = node(&mut graph);
        graph.add_edge(
            previous,
            next,
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(offset),
            }),
        );
        previous = next;
    }
    let result = summary(&graph, input, previous);
    assert_eq!(result.nodes.len(), offsets.len() + 1);
    assert_eq!(result.edges.len(), offsets.len());
    assert_eq!(
        result
            .edges
            .iter()
            .map(|edge| edge.kind.packed.unwrap())
            .collect::<Vec<_>>(),
        offsets
    );
}

#[test]
fn domain_boundaries_are_kept_even_on_identity_chains() {
    let mut graph = DependencyGraph::new();
    let input = node(&mut graph);
    let middle = node(&mut graph);
    let output = node(&mut graph);
    graph[middle].domains.push(PositionDomain {
        array_start: 0,
        array_length: 1,
        packed_start: 2,
        packed_length: 3,
    });
    wire(&mut graph, input, middle);
    wire(&mut graph, middle, output);
    let result = summary(&graph, input, output);
    assert_eq!(result.nodes.len(), 3);
    assert_eq!(result.nodes[1].domains, graph[middle].domains);
}

#[test]
fn identical_domain_boundaries_contract_without_losing_guards() {
    let mut graph = DependencyGraph::new();
    let input = node(&mut graph);
    let domain = PositionDomain {
        array_start: 0,
        array_length: 1,
        packed_start: 0,
        packed_length: 16,
    };
    graph[input].domains.push(domain);
    let condition = PathCondition::default().with_choice(BranchId::new(0, 0, 2), 1);
    let mut previous = input;
    for index in 0..1024 {
        let next = node(&mut graph);
        graph[next].domains.push(domain);
        graph.add_edge(
            previous,
            next,
            GraphDependency {
                kind: BitDependency::identity(),
                condition: if index == 0 {
                    condition.clone()
                } else {
                    PathCondition::default()
                },
            },
        );
        previous = next;
    }
    let result = summary(&graph, input, previous);
    assert_eq!(result.nodes.len(), 2);
    assert_eq!(result.edges.len(), 1);
    assert_eq!(result.edges[0].kind, BitDependency::identity());
    assert_eq!(result.edges[0].condition, condition);
    assert_eq!(module_summary_work().1, graph.edge_count());
}

#[test]
fn equal_bounds_still_clip_shifted_and_whole_dependencies() {
    for kind in [
        BitDependency {
            array: Some(1),
            packed: Some(0),
        },
        BitDependency {
            array: Some(0),
            packed: Some(1),
        },
        BitDependency {
            array: None,
            packed: None,
        },
    ] {
        let mut graph = DependencyGraph::new();
        let input = node(&mut graph);
        let middle = node(&mut graph);
        let output = node(&mut graph);
        let domain = PositionDomain {
            array_start: 0,
            array_length: 1,
            packed_start: 0,
            packed_length: 16,
        };
        graph[input].domains.push(domain);
        graph[middle].domains.push(domain);
        graph.add_edge(input, middle, GraphDependency::unconditional(kind));
        wire(&mut graph, middle, output);
        let result = summary(&graph, input, output);
        // The output is unbounded, so removing the middle would let the
        // preceding operation escape the input's array or packed bounds.
        assert_eq!(result.nodes.len(), 3);
        assert_eq!(result.nodes[1].domains, vec![domain]);
        assert_eq!(result.edges[0].kind, kind);
    }
}

type Adjacency = Vec<Vec<(usize, BitDependency, PathCondition)>>;

// Independently expand a small bounded bit graph for each branch valuation.
// Comparing reachability also checks that pruning does not lose a live route
// or join mutually exclusive guards while contracting a chain.
fn bit_reachability(
    graph: &Adjacency,
    input: Option<usize>,
    output: Option<usize>,
    valuation: &PathCondition,
) -> [u8; 3] {
    let mut result = [0; 3];
    let (Some(input), Some(output)) = (input, output) else {
        return result;
    };
    for (source_bit, result) in result.iter_mut().enumerate() {
        let mut visited = HashSet::from_iter([(input, source_bit)]);
        let mut queue = VecDeque::from([(input, source_bit)]);
        while let Some((source, bit)) = queue.pop_front() {
            if source == output {
                *result |= 1 << bit;
            }
            for (target, dependency, condition) in &graph[source] {
                if condition.conjoin_if_compatible(valuation).is_none() {
                    continue;
                }
                for next_bit in 0..3 {
                    if dependency
                        .packed
                        .is_none_or(|offset| bit as isize + offset == next_bit as isize)
                        && visited.insert((*target, next_bit))
                    {
                        queue.push_back((*target, next_bit));
                    }
                }
            }
        }
    }
    result
}

#[test]
fn generated_small_graphs_preserve_bit_reachability_for_every_branch_valuation() {
    let candidates = [
        (0, 1),
        (0, 2),
        (0, 3),
        (1, 1),
        (1, 2),
        (1, 3),
        (2, 1),
        (2, 2),
        (2, 3),
        (3, 1),
        (3, 2),
        (3, 3),
    ];
    for mask in 0..1usize << candidates.len() {
        for flavor in 0..3 {
            let mut graph = DependencyGraph::new();
            let nodes = (0..4).map(|_| node(&mut graph)).collect::<Vec<_>>();
            let mut original: Adjacency = vec![Vec::new(); 4];
            for (index, &(source, destination)) in candidates.iter().enumerate() {
                if mask & (1 << index) == 0 {
                    continue;
                }
                let dependency = match flavor {
                    0 => BitDependency::identity(),
                    _ => BitDependency {
                        array: Some(0),
                        packed: match index % 4 {
                            0 => None,
                            value => Some(value as isize - 2),
                        },
                    },
                };
                let condition = if flavor == 2 && index % 3 != 0 {
                    PathCondition::default()
                        .with_choice(BranchId::new(0, index % 2, 2), index / 2 % 2)
                } else {
                    PathCondition::default()
                };
                graph.add_edge(
                    nodes[source],
                    nodes[destination],
                    GraphDependency {
                        kind: dependency,
                        condition: condition.clone(),
                    },
                );
                original[source].push((destination, dependency, condition));
            }
            let result = summary(&graph, nodes[0], nodes[3]);
            let mut summarized: Adjacency = vec![Vec::new(); result.nodes.len()];
            for edge in &result.edges {
                summarized[edge.source].push((edge.destination, edge.kind, edge.condition.clone()));
            }
            let input = result
                .nodes
                .iter()
                .position(|node| node.kind == SummaryNodeKind::Input);
            let output = result
                .nodes
                .iter()
                .position(|node| node.kind == SummaryNodeKind::Output);
            for arms in 0..4 {
                let valuation = PathCondition::default()
                    .with_choice(BranchId::new(0, 0, 2), arms & 1)
                    .with_choice(BranchId::new(0, 1, 2), arms >> 1);
                assert_eq!(
                    bit_reachability(&original, Some(0), Some(3), &valuation),
                    bit_reachability(&summarized, input, output, &valuation),
                    "mask={mask:#x}, flavor={flavor}, arms={arms}",
                );
            }
        }
    }
}
