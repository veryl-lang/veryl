//! Finite structural summaries of an arbitrary number of loop iterations.
//!
//! Keep the one-iteration DAG, including imported projections, and connect
//! each written output to its next-iteration input. Condense recurrence SCCs
//! before returning to SSA: a procedural recurrence is not itself a circuit
//! feedback edge. Acyclic transfers remain positional. Within a recurrence,
//! retain an axis when every internal edge preserves its coordinate, and
//! conservatively unlink only axes which can change. No iteration counts,
//! bit positions, displacement sums, or dependency paths are enumerated.

use super::*;
use daggy::petgraph::Graph;
use daggy::petgraph::algo::kosaraju_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;

#[derive(Default)]
struct TransferNode {
    input: Option<VersionId>,
    domains: Vec<PositionDomain>,
    replication: Option<isize>,
}

type TransferGraph = Graph<TransferNode, PositionRelation>;

/// Share ancestor queries across calls, but resolve actual versions per call.
struct ImportIndex {
    incoming: Vec<Vec<usize>>,
    nodes_by_root: HashMap<usize, Vec<usize>>,
}

impl ImportIndex {
    fn new<K>(graph: &DependencyDag<K>) -> Self {
        let mut incoming = vec![Vec::new(); graph.nodes.len()];
        for (index, edge) in graph.edges.iter().enumerate() {
            #[cfg(test)]
            ITERATION_IMPORT_VISITS.set(ITERATION_IMPORT_VISITS.get() + 1);
            incoming[edge.destination].push(index);
        }
        Self {
            incoming,
            nodes_by_root: HashMap::default(),
        }
    }

    fn retain<K>(&mut self, graph: &DependencyDag<K>, root: usize) {
        self.nodes_by_root.entry(root).or_insert_with(|| {
            let mut visited = HashSet::default();
            let mut queue = VecDeque::from([root]);
            visited.insert(root);
            while let Some(node) = queue.pop_front() {
                #[cfg(test)]
                ITERATION_IMPORT_VISITS.set(ITERATION_IMPORT_VISITS.get() + 1);
                for &edge in &self.incoming[node] {
                    let source = graph.edges[edge].source;
                    if visited.insert(source) {
                        queue.push_back(source);
                    }
                }
            }
            visited.into_iter().collect()
        });
    }
}

#[derive(Default)]
struct TransferBuilder {
    graph: TransferGraph,
    versions: HashMap<VersionId, NodeIndex>,
    pending: VecDeque<VersionId>,
}

impl TransferBuilder {
    fn version(&mut self, version: VersionId) -> NodeIndex {
        *self.versions.entry(version).or_insert_with(|| {
            self.pending.push_back(version);
            self.graph.add_node(TransferNode::default())
        })
    }

    fn copy_iteration<K: Copy + Eq + Hash>(&mut self, ssa: &SsaStore<K>, start: usize) {
        let mut imports = HashMap::default();
        while let Some(version) = self.pending.pop_front() {
            let node = self.versions[&version];
            if version < start || matches!(ssa.versions[version], Version::Entry(_)) {
                self.graph[node].input = Some(version);
                continue;
            }
            // A branch can take different arms on different iterations.
            // Keep may-dependencies without reusing an arm constraint across
            // those iterations. Pre-loop input versions keep their guards.
            match &ssa.versions[version] {
                Version::Entry(_) => unreachable!("entries are handled above"),
                Version::Definition { sources, .. } => {
                    for &(source, relation) in sources {
                        let source = self.version(source);
                        self.graph.add_edge(source, node, relation);
                    }
                }
                Version::Phi(inputs) => {
                    for &source in inputs {
                        let source = self.version(source);
                        self.graph
                            .add_edge(source, node, PositionRelation::default());
                    }
                }
                Version::Projected { source, domain } => {
                    self.graph[node].domains.push(*domain);
                    let source = self.version(*source);
                    self.graph
                        .add_edge(source, node, PositionRelation::default());
                }
                Version::Replicated {
                    source,
                    domain,
                    stride,
                } => {
                    self.graph[node].domains.push(*domain);
                    self.graph[node].replication = Some(*stride);
                    let source = self.version(*source);
                    self.graph
                        .add_edge(source, node, PositionRelation::default());
                }
                Version::Imported {
                    graph,
                    root: Some(root),
                    bindings,
                    ..
                } => {
                    let index = imports
                        .entry(Rc::as_ptr(graph))
                        .or_insert_with(|| ImportIndex::new(graph));
                    index.retain(graph, *root);
                    let retained = &index.nodes_by_root[root];
                    let mut mapped = HashMap::default();
                    for &child in retained {
                        let copied = self.graph.add_node(TransferNode {
                            input: None,
                            domains: graph.domains[child].clone(),
                            replication: match graph.nodes[child] {
                                DependencyDagNode::Replicated { stride } => Some(stride),
                                _ => None,
                            },
                        });
                        mapped.insert(child, copied);
                        if let DependencyDagNode::External(key) = graph.nodes[child] {
                            for &(source, relation) in bindings.get(&key).into_iter().flatten() {
                                let source = self.version(source);
                                self.graph.add_edge(source, copied, relation);
                            }
                        }
                    }
                    for &child in retained {
                        for &edge in &index.incoming[child] {
                            let edge = &graph.edges[edge];
                            self.graph.add_edge(
                                mapped[&edge.source],
                                mapped[&child],
                                edge.relation,
                            );
                        }
                    }
                    self.graph
                        .add_edge(mapped[root], node, PositionRelation::default());
                }
                Version::Imported { root: None, .. } => {}
            }
        }
    }
}

pub(super) fn close<K: Copy + Eq + Hash>(
    ssa: &mut SsaStore<K>,
    iteration: &BranchState<K>,
    checkpoint: Checkpoint,
    may_skip: bool,
    domain: impl Fn(K) -> Option<PositionDomain>,
) {
    let mut builder = TransferBuilder::default();
    let outputs = iteration
        .bindings
        .iter()
        .map(|(&key, &output)| {
            let entry = ssa.read(key);
            let input = builder.version(entry);
            let value = builder.version(output);
            let domains = domain(key).into_iter().collect::<Vec<_>>();
            let root = builder.graph.add_node(TransferNode {
                input: None,
                domains: domains.clone(),
                replication: None,
            });
            builder
                .graph
                .add_edge(value, root, PositionRelation::default());
            (key, entry, input, root, domains)
        })
        .collect::<Vec<_>>();
    builder.copy_iteration(ssa, checkpoint.version_start);

    let mut unrestricted = HashSet::default();
    for (_, entry, input, root, domains) in &outputs {
        // Separate the immutable first-iteration input from the join that
        // also accepts prior iterations. Multiple keys may share a version;
        // their domains are alternatives, not intersecting restrictions.
        if builder.graph[*input].input.take().is_some() {
            let initial = builder.graph.add_node(TransferNode {
                input: Some(*entry),
                domains: Vec::new(),
                replication: None,
            });
            builder
                .graph
                .add_edge(initial, *input, PositionRelation::default());
        }
        if domains.is_empty() {
            unrestricted.insert(*input);
            builder.graph[*input].domains.clear();
        } else if !unrestricted.contains(input) {
            builder.graph[*input].domains.extend_from_slice(domains);
        }
        builder
            .graph
            .add_edge(*root, *input, PositionRelation::default());
    }

    let mapped = condense(ssa, &builder.graph);
    for (key, entry, _, root, domains) in outputs {
        let mut output = mapped[root.index()];
        if may_skip {
            let entry = project(ssa, entry, &domains);
            output = ssa.related_definition(vec![
                (output, PositionRelation::default()),
                (entry, PositionRelation::default()),
            ]);
        }
        ssa.bind(key, output);
    }
}

fn project<K: Copy + Eq + Hash>(
    ssa: &mut SsaStore<K>,
    value: VersionId,
    domains: &[PositionDomain],
) -> VersionId {
    if domains.is_empty() {
        return value;
    }
    let alternatives = domains
        .iter()
        .map(|&domain| ssa.projected(value, domain))
        .collect();
    ssa.phi(alternatives)
}

/// Materialize the SCC condensation as ordinary acyclic SSA. Each graph node
/// and edge contributes only bounded work and storage, independently of the
/// declared widths or the number of paths through shared function summaries.
fn condense<K: Copy + Eq + Hash>(ssa: &mut SsaStore<K>, graph: &TransferGraph) -> Vec<VersionId> {
    let components = kosaraju_scc(graph);
    let mut component_of = vec![0; graph.node_count()];
    for (index, nodes) in components.iter().enumerate() {
        for node in nodes {
            component_of[node.index()] = index;
        }
    }
    let mut incoming = vec![Vec::new(); components.len()];
    let mut successors = vec![Vec::new(); components.len()];
    let mut cyclic = components
        .iter()
        .map(|nodes| nodes.len() > 1)
        .collect::<Vec<_>>();
    let mut stable = vec![PositionRelation::default(); components.len()];
    for node in graph.node_indices() {
        if graph[node].replication.is_some() {
            // Replication changes packed coordinates if it participates in
            // an actual runtime recurrence. Otherwise keep it as an operation;
            // its finite repetitions are not procedural feedback.
            stable[component_of[node.index()]].packed = None;
        }
    }
    for edge in graph.edge_references() {
        let source = component_of[edge.source().index()];
        let destination = component_of[edge.target().index()];
        if source == destination {
            cyclic[source] = true;
            if edge.weight().array != Some(0) {
                stable[source].array = None;
            }
            if edge.weight().packed != Some(0) {
                stable[source].packed = None;
            }
        } else {
            incoming[destination].push(edge);
            successors[source].push(destination);
        }
    }
    let mut pending = incoming.iter().map(Vec::len).collect::<Vec<_>>();
    let mut queue = pending
        .iter()
        .enumerate()
        .filter_map(|(index, &count)| (count == 0).then_some(index))
        .collect::<VecDeque<_>>();
    let mut mapped = vec![0; graph.node_count()];
    while let Some(component) = queue.pop_front() {
        let nodes = &components[component];
        if cyclic[component] {
            let sources = incoming[component]
                .iter()
                .map(|edge| {
                    let value = ssa
                        .related_definition(vec![(mapped[edge.source().index()], *edge.weight())]);
                    let value = project(ssa, value, &graph[edge.target()].domains);
                    (value, stable[component])
                })
                .collect();
            let joined = ssa.related_definition(sources);
            // Discarding internal path restrictions is conservative; keeping
            // the entry and exit projections still bounds the affected bits.
            for &node in nodes {
                mapped[node.index()] = project(ssa, joined, &graph[node].domains);
            }
        } else {
            let node = nodes[0];
            let value = graph[node].input.unwrap_or_else(|| {
                ssa.related_definition(
                    incoming[component]
                        .iter()
                        .map(|edge| (mapped[edge.source().index()], *edge.weight()))
                        .collect(),
                )
            });
            mapped[node.index()] = if let Some(stride) = graph[node].replication {
                let alternatives = graph[node]
                    .domains
                    .iter()
                    .map(|&domain| ssa.replicated(value, domain, stride))
                    .collect();
                ssa.phi(alternatives)
            } else {
                project(ssa, value, &graph[node].domains)
            };
        }
        for &successor in &successors[component] {
            pending[successor] -= 1;
            if pending[successor] == 0 {
                queue.push_back(successor);
            }
        }
    }
    debug_assert!(pending.iter().all(|&count| count == 0));
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_transfer_retains_acyclic_packed_replication_at_scale() {
        for width in [8, 1 << 30] {
            for imported in [false, true] {
                let mut ssa = SsaStore::default();
                let input = ssa.read("input");
                let domain = PositionDomain {
                    array_start: 1,
                    array_length: 1,
                    packed_start: 0,
                    packed_length: width,
                };
                let checkpoint = ssa.checkpoint();
                let value = if imported {
                    let mut callee = SsaStore::default();
                    let source = callee.read("source");
                    let source = callee.projected(
                        source,
                        PositionDomain {
                            packed_length: 2,
                            ..domain
                        },
                    );
                    let result = callee.replicated(source, domain, 2);
                    let dag = callee.dependency_dag(&[result], &HashSet::default());
                    let root = dag.roots[0];
                    ssa.imported(
                        Rc::new(dag),
                        root,
                        HashMap::from_iter([(
                            "source",
                            vec![(input, PositionRelation::default())],
                        )]),
                        HashMap::default(),
                    )
                } else {
                    let source = ssa.projected(
                        input,
                        PositionDomain {
                            packed_length: 2,
                            ..domain
                        },
                    );
                    ssa.replicated(source, domain, 2)
                };
                ssa.bind("value", value);
                let iteration = ssa.capture_and_rollback(checkpoint);
                let before = ssa.versions.len();
                ssa.close_repeated_transfer(&iteration, checkpoint, false, |_| Some(domain));
                assert!(ssa.versions.len() - before < 20);
                let value = ssa.read("value");
                let dag = ssa.dependency_dag(&[value], &HashSet::default());
                assert!(dag.nodes.len() < 10);
                let replicas = dag
                    .nodes
                    .iter()
                    .enumerate()
                    .filter_map(|(node, kind)| {
                        matches!(kind, DependencyDagNode::Replicated { stride: 2 }).then_some(node)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(replicas.len(), 1);
                assert_eq!(dag.domains[replicas[0]], [domain]);
            }
        }
    }

    #[test]
    fn repeated_transfer_long_chain_stays_structural() {
        const COUNT: usize = 10_000;
        let mut ssa = SsaStore::default();
        let inputs = (0..=COUNT).map(|key| ssa.read(key)).collect::<Vec<_>>();
        let checkpoint = ssa.checkpoint();
        for key in 0..COUNT {
            let value =
                ssa.related_definition(vec![(inputs[key + 1], PositionRelation::default())]);
            ssa.bind(key, value);
        }
        let iteration = ssa.capture_and_rollback(checkpoint);
        let before = ssa.versions.len();
        ssa.close_repeated_transfer(&iteration, checkpoint, false, |_| None);
        assert!(ssa.versions.len() - before < COUNT * 4);
        let first = ssa.read(0);
        assert_eq!(ssa.root_source_keys_guarded(first).len(), COUNT);
    }

    #[test]
    fn repeated_transfer_wide_recurrence_does_not_enumerate_positions() {
        for width in [8, 1 << 30] {
            for shift in [0, 1] {
                let mut ssa = SsaStore::default();
                let input = ssa.read("input");
                ssa.bind("value", input);
                let checkpoint = ssa.checkpoint();
                let value = ssa.related_definition(vec![(
                    input,
                    PositionRelation {
                        array: Some(0),
                        packed: Some(shift),
                    },
                )]);
                ssa.bind("value", value);
                let iteration = ssa.capture_and_rollback(checkpoint);
                let before = ssa.versions.len();
                ssa.close_repeated_transfer(&iteration, checkpoint, false, |_| {
                    Some(PositionDomain {
                        array_start: 0,
                        array_length: 1,
                        packed_start: 0,
                        packed_length: width,
                    })
                });
                assert!(ssa.versions.len() - before < 16);
                let value = ssa.read("value");
                assert_eq!(
                    ssa.root_source_relations(value),
                    [(
                        "input",
                        PositionRelation {
                            array: Some(0),
                            packed: (shift == 0).then_some(0),
                        }
                    )]
                    .into_iter()
                    .collect()
                );
            }
        }
    }

    #[test]
    fn repeated_transfer_covers_small_expanded_transfers() {
        const KEYS: usize = 3;
        const WIDTH: usize = 4;
        let positions = |bit: usize, offset: Option<isize>| {
            (0..WIDTH).filter(move |&next| {
                offset.is_none_or(|offset| bit as isize + offset == next as isize)
            })
        };
        let mut random = 7u32;
        for case in 0..64 {
            let mut next_random = || {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (random >> 16) as usize
            };
            let identity = case % 4 < 2;
            let may_skip = case % 2 == 0;
            let mut ssa = SsaStore::default();
            let entries = (0..KEYS).map(|key| ssa.read(key)).collect::<Vec<_>>();
            let checkpoint = ssa.checkpoint();
            let mut transfer = Vec::new();
            for destination in 0..KEYS {
                let mut sources = Vec::new();
                for _ in 0..next_random() % 4 {
                    let source = next_random() % KEYS;
                    let offset = if identity {
                        Some(0)
                    } else {
                        [Some(-1), Some(0), Some(1), None][next_random() % 4]
                    };
                    transfer.push((source, destination, offset));
                    sources.push((
                        entries[source],
                        PositionRelation {
                            array: Some(0),
                            packed: offset,
                        },
                    ));
                }
                let value = ssa.related_definition(sources);
                ssa.bind(destination, value);
            }
            let iteration = ssa.capture_and_rollback(checkpoint);
            ssa.close_repeated_transfer(&iteration, checkpoint, may_skip, |_| {
                Some(PositionDomain {
                    array_start: 0,
                    array_length: 1,
                    packed_start: 0,
                    packed_length: WIDTH,
                })
            });
            let roots = (0..KEYS).map(|key| ssa.read(key)).collect::<Vec<_>>();
            let dag = ssa.dependency_dag(&roots, &(0..KEYS).collect());
            let mut outgoing = vec![Vec::new(); dag.nodes.len()];
            for edge in &dag.edges {
                outgoing[edge.source].push(edge);
            }

            for key in 0..KEYS {
                for bit in 0..WIDTH {
                    // Independent reference: expand the small transfer to
                    // individual bits and compute ordinary reachability.
                    let mut expected = HashSet::default();
                    if may_skip {
                        expected.insert((key, bit));
                    }
                    let mut visited = HashSet::from_iter([(key, bit)]);
                    let mut queue = VecDeque::from([(key, bit)]);
                    while let Some((current, bit)) = queue.pop_front() {
                        for &(source, destination, offset) in &transfer {
                            if source != current {
                                continue;
                            }
                            for next in positions(bit, offset) {
                                expected.insert((destination, next));
                                if visited.insert((destination, next)) {
                                    queue.push_back((destination, next));
                                }
                            }
                        }
                    }

                    let mut reached = HashSet::default();
                    let mut queue = VecDeque::new();
                    for (node, kind) in dag.nodes.iter().enumerate() {
                        if matches!(kind, DependencyDagNode::External(source) if *source == key) {
                            reached.insert((node, bit));
                            queue.push_back((node, bit));
                        }
                    }
                    while let Some((node, bit)) = queue.pop_front() {
                        for edge in &outgoing[node] {
                            for next in positions(bit, edge.relation.packed) {
                                let domains = &dag.domains[edge.destination];
                                if !domains.is_empty()
                                    && !domains.iter().any(|domain| {
                                        domain.packed_start <= next
                                            && next < domain.packed_start + domain.packed_length
                                    })
                                {
                                    continue;
                                }
                                if reached.insert((edge.destination, next)) {
                                    queue.push_back((edge.destination, next));
                                }
                            }
                        }
                    }
                    let actual = dag
                        .roots
                        .iter()
                        .enumerate()
                        .flat_map(|(key, root)| {
                            (0..WIDTH)
                                .filter_map(|bit| {
                                    root.is_some_and(|root| reached.contains(&(root, bit)))
                                        .then_some((key, bit))
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect::<HashSet<_>>();
                    assert!(
                        expected.is_subset(&actual),
                        "case={case}, input=({key}, {bit}), transfer={transfer:?}, expected={expected:?}, actual={actual:?}"
                    );
                    if identity {
                        assert_eq!(
                            expected, actual,
                            "identity transfers must retain exact bit correspondence"
                        );
                    }
                }
            }
        }
    }
}
