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

/// Index each child once; ancestor traversal is shared by all roots of a call.
struct ImportIndex {
    incoming: Vec<Vec<usize>>,
}

impl ImportIndex {
    fn try_new<K>(graph: &DependencyDag<K>, work: &mut usize) -> Option<Self> {
        *work = work.checked_sub(graph.nodes.len().saturating_add(graph.edges.len()))?;
        let mut incoming = vec![Vec::new(); graph.nodes.len()];
        for (index, edge) in graph.edges.iter().enumerate() {
            #[cfg(test)]
            ITERATION_IMPORT_VISITS.set(ITERATION_IMPORT_VISITS.get() + 1);
            incoming[edge.destination].push(index);
        }
        Some(Self { incoming })
    }
}

#[derive(Default)]
struct TransferBuilder {
    graph: TransferGraph,
    versions: HashMap<VersionId, NodeIndex>,
    pending: VecDeque<VersionId>,
}

impl TransferBuilder {
    fn version<K>(
        &mut self,
        ssa: &SsaStore<K>,
        version: VersionId,
        start: usize,
        work: &mut usize,
    ) -> Option<NodeIndex> {
        if let Some(&node) = self.versions.get(&version) {
            return Some(node);
        }
        let range = ssa
            .repeated_versions
            .partition_point(|range| range.end <= version);
        if version >= start
            && ssa
                .repeated_versions
                .get(range)
                .is_some_and(|range| range.start <= version)
        {
            // An inner loop has already converted its transfer (including any
            // imports) to ordinary SSA. Charge every later copy before adding
            // its node, edges or domains. Pre-loop inputs are only references;
            // directly written SSA remains independent of this expansion cap.
            let payload = match &ssa.versions[version] {
                Version::Definition { sources, .. } => sources.len(),
                Version::Phi(inputs) => inputs.len(),
                Version::Guarded { .. } => 1,
                Version::Projected { .. } | Version::Replicated { .. } => 2,
                Version::Entry(_) | Version::Imported { .. } => 0,
            };
            *work = work.checked_sub(payload.saturating_add(1))?;
        }
        let node = self.graph.add_node(TransferNode::default());
        self.versions.insert(version, node);
        self.pending.push_back(version);
        Some(node)
    }

    fn copy_iteration<K: Copy + Eq + Hash>(
        &mut self,
        ssa: &SsaStore<K>,
        start: usize,
        import_work: &mut usize,
    ) -> Option<()> {
        let mut imports = HashMap::default();
        let mut invocations: HashMap<_, HashMap<usize, NodeIndex>> = HashMap::default();
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
                        let source = self.version(ssa, source, start, import_work)?;
                        self.graph.add_edge(source, node, relation);
                    }
                }
                Version::Phi(inputs) => {
                    for &source in inputs {
                        let source = self.version(ssa, source, start, import_work)?;
                        self.graph
                            .add_edge(source, node, PositionRelation::default());
                    }
                }
                Version::Guarded { source, .. } => {
                    let source = self.version(ssa, *source, start, import_work)?;
                    self.graph
                        .add_edge(source, node, PositionRelation::default());
                }
                Version::Projected { source, domain } => {
                    self.graph[node].domains.push(*domain);
                    let source = self.version(ssa, *source, start, import_work)?;
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
                    let source = self.version(ssa, *source, start, import_work)?;
                    self.graph
                        .add_edge(source, node, PositionRelation::default());
                }
                Version::Imported {
                    graph,
                    root: Some(root),
                    bindings,
                    ..
                } => {
                    // Charge the root edge and all imported storage before
                    // allocation. These copies precede DAG-export budgets.
                    *import_work = import_work.checked_sub(1)?;
                    let index = match imports.entry(Rc::as_ptr(graph)) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(ImportIndex::try_new(graph, import_work)?)
                        }
                    };
                    // Guards are discarded for runtime iterations, but actual
                    // bindings still distinguish calls. Keep one mapped child
                    // per invocation instead of copying every output prefix.
                    let mapped = invocations
                        .entry((Rc::as_ptr(graph), Rc::as_ptr(bindings)))
                        .or_default();
                    let mut pending = VecDeque::from([*root]);
                    let mut retained = Vec::new();
                    while let Some(child) = pending.pop_front() {
                        if mapped.contains_key(&child) {
                            continue;
                        }
                        *import_work = import_work.checked_sub(
                            graph.domains[child]
                                .len()
                                .saturating_add(index.incoming[child].len())
                                .saturating_add(1),
                        )?;
                        #[cfg(test)]
                        ITERATION_IMPORT_VISITS.set(ITERATION_IMPORT_VISITS.get() + 1);
                        let copied = self.graph.add_node(TransferNode {
                            input: None,
                            domains: graph.domains[child].clone(),
                            replication: match graph.nodes[child] {
                                DependencyDagNode::Replicated { stride } => Some(stride),
                                _ => None,
                            },
                        });
                        mapped.insert(child, copied);
                        retained.push(child);
                        pending.extend(
                            index.incoming[child]
                                .iter()
                                .map(|&edge| graph.edges[edge].source),
                        );
                        if let DependencyDagNode::External(key) = graph.nodes[child] {
                            let sources = bindings.get(&key).map(Vec::as_slice).unwrap_or_default();
                            *import_work = import_work.checked_sub(sources.len())?;
                            for &(source, relation) in sources {
                                let source = self.version(ssa, source, start, import_work)?;
                                self.graph.add_edge(source, copied, relation);
                            }
                        }
                    }
                    for child in retained {
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
        Some(())
    }
}

pub(super) fn try_close<K: Copy + Eq + Hash>(
    ssa: &mut SsaStore<K>,
    iteration: &BranchState<K>,
    checkpoint: Checkpoint,
    may_skip: bool,
    import_work: &mut usize,
    domain: impl Fn(K) -> Option<PositionDomain>,
) -> Option<()> {
    let mut builder = TransferBuilder::default();
    let outputs = iteration
        .bindings
        .iter()
        .map(|(&key, &output)| {
            let entry = ssa.read(key);
            let input = builder.version(ssa, entry, checkpoint.version_start, import_work)?;
            let value = builder.version(ssa, output, checkpoint.version_start, import_work)?;
            let domains = domain(key).into_iter().collect::<Vec<_>>();
            let root = builder.graph.add_node(TransferNode {
                input: None,
                domains: domains.clone(),
                replication: None,
            });
            builder
                .graph
                .add_edge(value, root, PositionRelation::default());
            Some((key, entry, input, root, domains))
        })
        .collect::<Option<Vec<_>>>()?;
    builder.copy_iteration(ssa, checkpoint.version_start, import_work)?;

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

    let generated_start = ssa.versions.len();
    let mapped = condense(ssa, &builder.graph);
    for (key, entry, _, root, domains) in outputs {
        let mut output = mapped[root.index()];
        if may_skip {
            // Skipping the loop retains the entry value without reading it.
            // A phi keeps bare retained state out of dependency DAG roots,
            // while preserving definitions established before the loop.
            output = ssa.phi(vec![output, entry]);
            output = project(ssa, output, &domains);
        }
        ssa.bind(key, output);
    }
    if generated_start < ssa.versions.len() {
        ssa.repeated_versions
            .push(generated_start..ssa.versions.len());
    }
    Some(())
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
    fn nested_repeated_transfer_stops_before_materializing_an_over_budget_copy() {
        let mut ssa = SsaStore::default();
        let input = ssa.read("input");
        let initial = ssa.definition(Vec::new());
        ssa.bind("output", initial);
        let domain = PositionDomain {
            array_start: 0,
            array_length: 1,
            packed_start: 0,
            packed_length: 8,
        };
        let outer = ssa.checkpoint();
        let inner = ssa.checkpoint();
        let mut value = input;
        for _ in 0..32 {
            value = ssa.related_definition(vec![(value, PositionRelation::default())]);
            value = ssa.projected(value, domain);
        }
        ssa.bind("output", value);
        let iteration = ssa.capture_and_rollback(inner);
        let mut work = 0;
        ssa.try_close_repeated_transfer(&iteration, inner, true, &mut work, |_| Some(domain))
            .expect("directly written SSA does not consume the copy budget");

        let iteration = ssa.capture_and_rollback(outer);
        let before = ssa.versions.len();
        let mut work = 64;
        assert!(
            ssa.try_close_repeated_transfer(&iteration, outer, true, &mut work, |_| Some(domain))
                .is_none()
        );
        assert_eq!(ssa.versions.len(), before, "reject before SSA condensation");
        assert_eq!(ssa.read("output"), initial, "no partial output binding");

        let mut work = 1024;
        ssa.try_close_repeated_transfer(&iteration, outer, true, &mut work, |_| Some(domain))
            .expect("a bounded nested transfer still preserves its dependencies");
        let output = ssa.read("output");
        assert_eq!(
            ssa.root_source_relations(output),
            HashMap::from_iter([("input", PositionRelation::default())])
        );
    }

    #[test]
    fn repeated_transfer_shares_prefixes_across_all_outputs_of_each_call() {
        for size in [64, 256, 1024] {
            let mut callee = SsaStore::default();
            let mut value = callee.read(0);
            let roots = (0..size)
                .map(|_| {
                    value = callee.definition(vec![value]);
                    value
                })
                .collect::<Vec<_>>();
            let graph = Rc::new(callee.dependency_dag(&roots, |_| true));
            let mut ssa = SsaStore::default();
            let actuals = [ssa.read(0), ssa.read(1)];
            let start = ssa.versions.len();
            let mut builder = TransferBuilder::default();
            let branches = Rc::default();
            let mut outputs = Vec::new();
            let mut work = size * 16;
            for actual in actuals {
                let bindings = Rc::new(HashMap::from_iter([(
                    0,
                    vec![(actual, PositionRelation::default())],
                )]));
                for &root in &graph.roots {
                    let output =
                        ssa.imported(graph.clone(), root, bindings.clone(), Rc::clone(&branches));
                    outputs.push(builder.version(&ssa, output, start, &mut work).unwrap());
                }
            }
            builder
                .copy_iteration(&ssa, start, &mut work)
                .expect("shared imports must fit in a linear budget");
            assert!(builder.graph.node_count() <= size * 4 + 4);
            assert!(builder.graph.edge_count() <= size * 4 + 4);
            let mapped = condense(&mut ssa, &builder.graph);
            for actual in 0..actuals.len() {
                for index in [0, size / 2, size - 1] {
                    let output = outputs[actual * size + index];
                    assert_eq!(
                        ssa.root_sources(mapped[output.index()]),
                        HashSet::from_iter([actual])
                    );
                }
            }
        }
    }

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
                    let dag = callee.dependency_dag(&[result], |_| false);
                    let root = dag.roots[0];
                    ssa.imported(
                        Rc::new(dag),
                        root,
                        HashMap::from_iter([(
                            "source",
                            vec![(input, PositionRelation::default())],
                        )])
                        .into(),
                        Rc::default(),
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
                let dag = ssa.dependency_dag(&[value], |_| false);
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
            let dag = ssa.dependency_dag(&roots, |key| *key < KEYS);
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
