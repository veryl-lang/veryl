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
use crate::comb_loop_detect::position::{Link, greatest_common_divisor};
use daggy::petgraph::Graph;
use daggy::petgraph::algo::kosaraju_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;

#[derive(Default)]
struct TransferNode {
    input: Option<VersionId>,
    domains: Vec<PositionDomain>,
    replication: Option<Replication>,
    /// The value of a written key at the start of an iteration.
    is_iteration_input: bool,
    /// The value of a written key at the end of an iteration.
    is_output: bool,
}

/// `data` distinguishes an explicit read from a retention-only edge. A path
/// consisting only of retention edges carries the previous value as state,
/// not as a combinational dependency, exactly as `Phi` and `Guarded` do in
/// the dependency DAG export.
#[derive(Clone)]
struct TransferEdge {
    relation: PositionRelation,
    data: bool,
    /// The choices of branches outside the loop the edge holds on. Those
    /// inside it are taken on each iteration again, so no iteration keeps
    /// another's.
    condition: PathCondition,
}

impl TransferEdge {
    fn retain() -> Self {
        Self::retain_on(PathCondition::default())
    }

    fn retain_on(condition: PathCondition) -> Self {
        Self {
            relation: PositionRelation {
                array: Link::IDENTITY,
                packed: Link::IDENTITY,
            },
            data: false,
            condition,
        }
    }

    fn data(relation: PositionRelation) -> Self {
        Self::data_on(relation, PathCondition::default())
    }

    fn data_on(relation: PositionRelation, condition: PathCondition) -> Self {
        Self {
            relation,
            data: true,
            condition,
        }
    }
}

type TransferGraph = Graph<TransferNode, TransferEdge>;

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
    /// The branches made before the loop, see `RepeatedIteration`.
    outer_branches: Option<BranchId>,
}

impl TransferBuilder {
    /// The choices of `condition` on branches made before the loop.
    fn outer(&self, condition: &PathCondition) -> PathCondition {
        match self.outer_branches {
            Some(first) => condition.restricted(|branch| {
                branch.procedure == first.procedure && branch.local < first.local
            }),
            None => PathCondition::default(),
        }
    }

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
                Version::Definition { sources, condition } => {
                    let condition = self.outer(condition);
                    for &(source, relation) in sources {
                        let source = self.version(ssa, source, start, import_work)?;
                        self.graph.add_edge(
                            source,
                            node,
                            TransferEdge::data_on(relation, condition.clone()),
                        );
                    }
                }
                Version::Phi(inputs) => {
                    for &source in inputs {
                        let source = self.version(ssa, source, start, import_work)?;
                        self.graph.add_edge(source, node, TransferEdge::retain());
                    }
                }
                Version::Guarded { source, condition } => {
                    let condition = self.outer(condition);
                    let source = self.version(ssa, *source, start, import_work)?;
                    self.graph
                        .add_edge(source, node, TransferEdge::retain_on(condition));
                }
                Version::Projected {
                    source,
                    domain,
                    retains,
                } => {
                    self.graph[node].domains.push(*domain);
                    let source = self.version(ssa, *source, start, import_work)?;
                    let edge = if *retains {
                        TransferEdge::retain()
                    } else {
                        TransferEdge::data(PositionRelation::default())
                    };
                    self.graph.add_edge(source, node, edge);
                }
                Version::Replicated {
                    source,
                    domain,
                    replication,
                } => {
                    self.graph[node].domains.push(*domain);
                    self.graph[node].replication = Some(*replication);
                    let source = self.version(ssa, *source, start, import_work)?;
                    self.graph.add_edge(
                        source,
                        node,
                        TransferEdge::data(PositionRelation::default()),
                    );
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
                                DependencyDagNode::Replicated { replication } => Some(replication),
                                _ => None,
                            },
                            ..TransferNode::default()
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
                                self.graph
                                    .add_edge(source, copied, TransferEdge::data(relation));
                            }
                        }
                    }
                    for child in retained {
                        for &edge in &index.incoming[child] {
                            let edge = &graph.edges[edge];
                            self.graph.add_edge(
                                mapped[&edge.source],
                                mapped[&child],
                                TransferEdge::data(edge.relation),
                            );
                        }
                    }
                    self.graph.add_edge(
                        mapped[root],
                        node,
                        TransferEdge::data(PositionRelation::default()),
                    );
                }
                Version::Imported { root: None, .. } => {}
            }
        }
        Some(())
    }
}

/// What every iteration of a statically counted loop is known to do to one
/// key, beyond its one-iteration transfer.
#[derive(Clone, Debug, Default)]
pub(in crate::comb_loop_detect) struct TransferCoverage {
    /// Every position of the key is written by some iteration, so the loop
    /// does not retain its entry value.
    pub(in crate::comb_loop_detect) killed: bool,
    /// The only positions at which an iteration can read the entry value.
    pub(in crate::comb_loop_detect) exposed: Option<Vec<PositionDomain>>,
}

/// One abstract iteration of a runtime loop and the states recorded in it.
pub(in crate::comb_loop_detect) struct RepeatedIteration<'s, K> {
    /// The output of each written key after the iteration.
    pub(in crate::comb_loop_detect) state: &'s BranchState<K>,
    /// Versions that predate it are the iteration's inputs.
    pub(in crate::comb_loop_detect) checkpoint: Checkpoint,
    /// The loop may run zero times and retain each key's entry version.
    pub(in crate::comb_loop_detect) may_skip: bool,
    /// States recorded inside the iteration, such as return paths.
    pub(in crate::comb_loop_detect) observed: &'s mut [BranchState<K>],
    /// How many of the last `observed` states observe only the keys they
    /// bind: values the iterations produce without feeding the next.
    pub(in crate::comb_loop_detect) bound_only: usize,
    /// The first branch made in the loop, of the procedure's numbering: the
    /// earlier ones of the procedure take one arm on every iteration, so
    /// the paths through the iterations keep their choices.
    pub(in crate::comb_loop_detect) outer_branches: Option<BranchId>,
}

impl<'s, K> RepeatedIteration<'s, K> {
    pub(in crate::comb_loop_detect) fn new(
        state: &'s BranchState<K>,
        checkpoint: Checkpoint,
        may_skip: bool,
    ) -> Self {
        Self {
            state,
            checkpoint,
            may_skip,
            observed: &mut [],
            bound_only: 0,
            outer_branches: None,
        }
    }
}

pub(super) fn try_close<K: Copy + Eq + Hash>(
    ssa: &mut SsaStore<K>,
    iteration: RepeatedIteration<K>,
    import_work: &mut usize,
    domain: impl Fn(K) -> Option<PositionDomain>,
    coverage: impl Fn(K) -> TransferCoverage,
) -> Option<()> {
    let RepeatedIteration {
        state: iteration,
        checkpoint,
        may_skip,
        observed,
        bound_only,
        outer_branches,
    } = iteration;
    let bound_from = observed.len().saturating_sub(bound_only);
    let mut builder = TransferBuilder {
        outer_branches,
        ..TransferBuilder::default()
    };
    let outputs = iteration
        .bindings
        .iter()
        .map(|(&key, &output)| {
            let entry = ssa.read(key);
            let input = builder.version(ssa, entry, checkpoint.version_start, import_work)?;
            let value = builder.version(ssa, output, checkpoint.version_start, import_work)?;
            let domains = domain(key).into_iter().collect::<Vec<_>>();
            let root = builder.graph.add_node(TransferNode {
                domains: domains.clone(),
                is_output: true,
                ..TransferNode::default()
            });
            builder.graph.add_edge(value, root, TransferEdge::retain());
            Some((key, entry, input, root, domains))
        })
        .collect::<Option<Vec<_>>>()?;
    // A state observed inside the body, such as a return path, sees what any
    // number of earlier iterations left. Each of its values that the body can
    // change is an output of the transfer that does not feed the next
    // iteration. A written key it does not bind still holds its entry. Other
    // keys keep pre-loop or body-local entry values that no iteration changes.
    let entries = outputs
        .iter()
        .map(|&(key, entry, ..)| (key, entry))
        .collect::<HashMap<_, _>>();
    let mut observers = Vec::new();
    for (index, state) in observed.iter().enumerate() {
        let unbound = entries
            .iter()
            .filter(|(key, _)| index < bound_from && !state.bindings.contains_key(key))
            .map(|(&key, &entry)| (key, entry));
        for (key, version) in state
            .bindings
            .iter()
            .map(|(&key, &version)| (key, version))
            .chain(unbound)
        {
            let changed = if let Some(&entry) = entries.get(&key) {
                version == entry || version >= checkpoint.version_start
            } else {
                version >= checkpoint.version_start
                    && !matches!(ssa.versions[version], Version::Entry(_))
            };
            if !changed {
                continue;
            }
            let value = builder.version(ssa, version, checkpoint.version_start, import_work)?;
            let root = builder.graph.add_node(TransferNode {
                is_output: true,
                ..TransferNode::default()
            });
            builder.graph.add_edge(value, root, TransferEdge::retain());
            observers.push((index, key, root));
        }
    }
    builder.copy_iteration(ssa, checkpoint.version_start, import_work)?;

    let mut unrestricted = HashSet::default();
    let mut initials = HashMap::default();
    let mut sharing: HashMap<NodeIndex, usize> = HashMap::default();
    for (_, entry, input, root, domains) in &outputs {
        // Separate the immutable first-iteration input from the join that
        // also accepts prior iterations. Multiple keys may share a version;
        // their domains are alternatives, not intersecting restrictions.
        *sharing.entry(*input).or_default() += 1;
        if builder.graph[*input].input.take().is_some() {
            let initial = builder.graph.add_node(TransferNode {
                input: Some(*entry),
                ..TransferNode::default()
            });
            builder
                .graph
                .add_edge(initial, *input, TransferEdge::retain());
            initials.insert(*input, initial);
        }
        builder.graph[*input].is_iteration_input = true;
        if domains.is_empty() {
            unrestricted.insert(*input);
            builder.graph[*input].domains.clear();
        } else if !unrestricted.contains(input) {
            builder.graph[*input].domains.extend_from_slice(domains);
        }
        builder
            .graph
            .add_edge(*root, *input, TransferEdge::retain());
    }

    let generated_start = ssa.versions.len();
    let coverage = outputs
        .iter()
        .map(|(key, ..)| coverage(*key))
        .collect::<Vec<_>>();
    // Explicit reads of an entry value see only its exposed positions. The
    // retained state of the entry remains unrestricted.
    let mut views = HashMap::default();
    for ((_, entry, input, _, _), coverage) in outputs.iter().zip(&coverage) {
        if let (Some(exposed), Some(&initial), Some(1)) =
            (&coverage.exposed, initials.get(input), sharing.get(input))
        {
            let view = if exposed.is_empty() {
                ssa.phi(Vec::new())
            } else {
                ssa.projected_union(*entry, exposed)
            };
            views.insert(initial, view);
        }
    }
    let layers = condense(ssa, &builder.graph, &views);
    for (index, key, root) in observers {
        let retained = layers.retained_value(ssa, &builder.graph, root);
        let data = layers.data[root.index()];
        let value = full(ssa, retained, data);
        observed[index].bindings.insert(key, value);
    }
    for ((key, entry, _, root, _), coverage) in outputs.into_iter().zip(coverage) {
        let retained = layers.retained_value(ssa, &builder.graph, root);
        let data = layers.data[root.index()];
        let mut output = if coverage.killed && !may_skip {
            // Every position is overwritten; the entry is not retained.
            full(ssa, None, data)
        } else {
            full(ssa, retained, data)
        };
        if may_skip {
            // Zero iterations retain the entry value as state.
            output = ssa.phi(vec![output, entry]);
        }
        ssa.bind(key, output);
    }
    if generated_start < ssa.versions.len() {
        ssa.repeated_versions
            .push(generated_start..ssa.versions.len());
    }
    Some(())
}

/// Retained and data layers of every transfer node.
///
/// The retained layer is the set of initial inputs reachable through
/// retention edges only. It is emitted as a `Phi`, so it remains state in the
/// exported DAG. The data layer contains every path with at least one
/// explicit read. A retained value entering a data edge becomes an ordinary
/// dependency there, exactly as in straight-line SSA, except that an initial
/// input with a restricted view is read only at its exposed positions.
struct Layers<'v> {
    /// Each initial input with the path it is retained on.
    retained: Vec<Vec<(NodeIndex, PathCondition)>>,
    data: Vec<Option<VersionId>>,
    views: &'v HashMap<NodeIndex, VersionId>,
}

impl<'v> Layers<'v> {
    fn new(count: usize, views: &'v HashMap<NodeIndex, VersionId>) -> Self {
        Self {
            retained: vec![Vec::new(); count],
            data: vec![None; count],
            views,
        }
    }

    /// The retained state of a node.
    fn retained_value<K: Copy + Eq + Hash>(
        &self,
        ssa: &mut SsaStore<K>,
        graph: &TransferGraph,
        node: NodeIndex,
    ) -> Option<VersionId> {
        let inputs = self.retained[node.index()]
            .iter()
            .filter_map(|(initial, condition)| Some(ssa.guarded(graph[*initial].input?, condition)))
            .collect();
        join(ssa, inputs)
    }

    /// The value of a node as observed by an explicit read.
    fn read<K: Copy + Eq + Hash>(
        &self,
        ssa: &mut SsaStore<K>,
        graph: &TransferGraph,
        node: NodeIndex,
    ) -> VersionId {
        let mut inputs = self.retained[node.index()]
            .iter()
            .filter_map(|(initial, condition)| {
                let input = self.views.get(initial).copied().or(graph[*initial].input)?;
                Some(ssa.guarded(input, condition))
            })
            .collect::<Vec<_>>();
        inputs.extend(self.data[node.index()]);
        ssa.phi(inputs)
    }

    /// Retain at `node` what `source` retains, on the path `condition`.
    fn retain_from(&mut self, node: NodeIndex, source: NodeIndex, condition: &PathCondition) {
        if node == source {
            return;
        }
        let inherited = self.retained[source.index()]
            .iter()
            .filter_map(|(initial, held)| Some((*initial, held.conjoin_if_compatible(condition)?)))
            .collect::<Vec<_>>();
        let retained = &mut self.retained[node.index()];
        retained.extend(inherited);
        retained.sort_unstable();
        retained.dedup();
    }
}

/// Materialize the SCC condensation as ordinary acyclic SSA. Each graph node
/// and edge contributes only bounded work and storage, independently of the
/// declared widths or the number of paths through shared function summaries.
fn condense<'v, K: Copy + Eq + Hash>(
    ssa: &mut SsaStore<K>,
    graph: &TransferGraph,
    views: &'v HashMap<NodeIndex, VersionId>,
) -> Layers<'v> {
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
    // A cycle through retention edges only carries state around the loop.
    // Only an internal explicit read turns every value entering the
    // component into a data dependency of every member.
    let mut internal_data = vec![false; components.len()];
    let mut stable = vec![PositionRelation::default(); components.len()];
    let mut translations = vec![[AxisTranslation::Stable; 2]; components.len()];
    for node in graph.node_indices() {
        if let Some(replication) = graph[node].replication {
            // Replication changes its axis's coordinates if it participates in
            // an actual runtime recurrence. Otherwise keep it as an operation;
            // its finite repetitions are not procedural feedback.
            let component = component_of[node.index()];
            let relation = &mut stable[component];
            *relation = replication.forget_position(*relation);
            // Inside a recurrence, the repetition is one more internal
            // translation of its axis.
            let [array, packed] = &mut translations[component];
            match replication {
                Replication::Array(stride) => *array = array.with(Some(stride)),
                Replication::Packed(stride) => *packed = packed.with(Some(stride)),
            }
        }
    }
    for edge in graph.edge_references() {
        let source = component_of[edge.source().index()];
        let destination = component_of[edge.target().index()];
        if source == destination {
            cyclic[source] = true;
            internal_data[source] |= edge.weight().data;
            let relation = edge.weight().relation;
            let [array, packed] = &mut translations[source];
            *array = array.with(relation.array.translation_offset());
            *packed = packed.with(relation.packed.translation_offset());
            if relation.array != Link::IDENTITY {
                stable[source].array = Link::from_offset(None);
            }
            if relation.packed != Link::IDENTITY {
                stable[source].packed = Link::from_offset(None);
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
    let mut layers = Layers::new(graph.node_count(), views);
    while let Some(component) = queue.pop_front() {
        let nodes = &components[component];
        let recurrence = cyclic[component]
            .then(|| {
                UniformRecurrence::new(graph, nodes, &translations[component], &stable[component])
            })
            .flatten();
        if graph[nodes[0]].input.is_some() && nodes.len() == 1 {
            layers.retained[nodes[0].index()] = vec![(nodes[0], PathCondition::default())];
        } else if let Some(recurrence) = recurrence {
            recurrence.materialize(ssa, graph, &incoming[component], &mut layers);
        } else if cyclic[component] {
            let mut data_inputs = Vec::new();
            let mut sources = Vec::new();
            let mut retained = Vec::new();
            for edge in &incoming[component] {
                let condition = &edge.weight().condition;
                if internal_data[component] || edge.weight().data {
                    let value = layers.read(ssa, graph, edge.source());
                    let value = ssa.related_definition_guarded(
                        vec![(value, edge.weight().relation)],
                        condition,
                    );
                    let value = ssa.projected_union(value, &graph[edge.target()].domains);
                    sources.push((value, stable[component]));
                }
                if !edge.weight().data {
                    retained.push((edge.source(), condition.clone()));
                    if let Some(data) = layers.data[edge.source().index()] {
                        data_inputs.push(ssa.guarded(data, condition));
                    }
                }
            }
            // Retained inputs of members of an SCC with an internal read have
            // already been added to `sources`; keep them as state as well.
            if !sources.is_empty() {
                data_inputs.push(ssa.related_definition(sources));
            }
            let data_value = join(ssa, data_inputs);
            let bounds = member_bounds(graph, nodes);
            for &node in nodes {
                for (source, condition) in &retained {
                    layers.retain_from(node, *source, condition);
                }
                let domains = match bounds.get(&node) {
                    Some(Bound::Hull(domain)) => std::slice::from_ref(domain),
                    _ => graph[node].domains.as_slice(),
                };
                layers.data[node.index()] =
                    data_value.map(|value| ssa.projected_union(value, domains));
            }
        } else {
            let node = nodes[0];
            let mut data_inputs = Vec::new();
            let mut sources = Vec::new();
            for edge in &incoming[component] {
                let condition = &edge.weight().condition;
                if edge.weight().data {
                    let value = layers.read(ssa, graph, edge.source());
                    let value = ssa.guarded(value, condition);
                    sources.push((value, edge.weight().relation));
                } else {
                    layers.retain_from(node, edge.source(), condition);
                    if let Some(data) = layers.data[edge.source().index()] {
                        data_inputs.push(ssa.guarded(data, condition));
                    }
                }
            }
            if !sources.is_empty() {
                data_inputs.push(ssa.related_definition(sources));
            }
            layers.data[node.index()] = join(ssa, data_inputs).map(|value| {
                if let Some(replication) = graph[node].replication {
                    let alternatives = graph[node]
                        .domains
                        .iter()
                        .map(|&domain| ssa.replicated(value, domain, replication))
                        .collect();
                    ssa.phi(alternatives)
                } else {
                    ssa.projected_union(value, &graph[node].domains)
                }
            });
        }
        for &successor in &successors[component] {
            pending[successor] -= 1;
            if pending[successor] == 0 {
                queue.push_back(successor);
            }
        }
    }
    debug_assert!(pending.iter().all(|&count| count == 0));
    layers
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bound {
    Empty,
    Hull(PositionDomain),
    Unbounded,
}

impl Bound {
    fn of(domains: &[PositionDomain]) -> Self {
        domains
            .iter()
            .fold(Self::Empty, |bound, &domain| bound.join(Self::Hull(domain)))
    }

    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Empty, bound) | (bound, Self::Empty) => bound,
            (Self::Hull(left), Self::Hull(right)) => {
                let array_start = left.array_start.min(right.array_start);
                let packed_start = left.packed_start.min(right.packed_start);
                let array_end = (left.array_start + left.array_length)
                    .max(right.array_start + right.array_length);
                let packed_end = (left.packed_start + left.packed_length)
                    .max(right.packed_start + right.packed_length);
                Self::Hull(PositionDomain {
                    array_start,
                    array_length: array_end - array_start,
                    packed_start,
                    packed_length: packed_end - packed_start,
                })
            }
            _ => Self::Unbounded,
        }
    }
}

/// The positions at which each member of a recurrence without domains of its
/// own can hold a value. A value changes position only through an edge that
/// moves it, so a member reached only through edges that keep positions
/// holds values only where its sources do. Data entering the recurrence with
/// its positions forgotten is then confined to those positions instead of
/// reaching every position of the member.
fn member_bounds(graph: &TransferGraph, nodes: &[NodeIndex]) -> HashMap<NodeIndex, Bound> {
    let members = nodes.iter().copied().collect::<HashSet<_>>();
    let mut bounds = nodes
        .iter()
        .map(|&node| {
            let bound = if graph[node].domains.is_empty() {
                Bound::Empty
            } else {
                Bound::of(&graph[node].domains)
            };
            (node, bound)
        })
        .collect::<HashMap<_, _>>();
    loop {
        let mut changed = false;
        for &node in nodes {
            if !graph[node].domains.is_empty() {
                continue;
            }
            let mut bound = if graph[node].input.is_some() || graph[node].replication.is_some() {
                Bound::Unbounded
            } else {
                Bound::Empty
            };
            for edge in graph.edges_directed(node, daggy::petgraph::Direction::Incoming) {
                let relation = edge.weight().relation;
                let source =
                    if relation.array != Link::IDENTITY || relation.packed != Link::IDENTITY {
                        Bound::Unbounded
                    } else if members.contains(&edge.source()) {
                        bounds[&edge.source()]
                    } else if graph[edge.source()].domains.is_empty() {
                        Bound::Unbounded
                    } else {
                        Bound::of(&graph[edge.source()].domains)
                    };
                bound = bound.join(source);
            }
            if bounds[&node] != bound {
                bounds.insert(node, bound);
                changed = true;
            }
        }
        if !changed {
            return bounds;
        }
    }
}

fn full<K: Copy + Eq + Hash>(
    ssa: &mut SsaStore<K>,
    retained: Option<VersionId>,
    data: Option<VersionId>,
) -> VersionId {
    match (retained, data) {
        (Some(retained), Some(data)) => ssa.phi(vec![retained, data]),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => ssa.phi(Vec::new()),
    }
}

fn join<K: Copy + Eq + Hash>(ssa: &mut SsaStore<K>, values: Vec<VersionId>) -> Option<VersionId> {
    match values.len() {
        0 => None,
        _ => Some(ssa.phi(values)),
    }
}

type IncomingEdge<'g> = daggy::petgraph::graph::EdgeReference<'g, TransferEdge>;

// Shortest-path work for one recurrence, counted in visited members and
// edges per distinct entry.
const UNIFORM_RECURRENCE_WORK: usize = 1 << 20;

/// A recurrence whose internal translations advance along one axis in one
/// direction while the other axis is preserved.
///
/// Every internal path from an entry member `e` to a member `m` translates
/// by a non-negative multiple of `stride` (the gcd of the internal offsets).
/// Let `d` be the least translation over the paths with at least one explicit
/// read. Every such path translates by `d + k * stride` for some `k >= 0`, and
/// its intermediate positions lie between its endpoints. Replicating a source
/// within the members' bounding box and translating it by `d` therefore
/// covers every data path from `e`. Paths through retention edges only keep
/// their value as state at the same position.
struct UniformRecurrence {
    replication: Replication,
    domain: PositionDomain,
    /// The positions a translation can start from or reach: those the
    /// sources of the translating edges hold, and where they move them.
    /// `None` when no translation can take place there.
    moving: Option<PositionDomain>,
    /// Relation retained by every internal path; the replicated axis is zero.
    stable: PositionRelation,
    members: Vec<NodeIndex>,
    local: HashMap<NodeIndex, usize>,
    /// Internal edges as `(target, stride units, explicit read)`.
    adjacency: Vec<Vec<(usize, usize, bool)>>,
}

impl UniformRecurrence {
    fn new(
        graph: &TransferGraph,
        nodes: &[NodeIndex],
        translations: &[AxisTranslation; 2],
        stable: &PositionRelation,
    ) -> Option<Self> {
        let replication = recurrence_replication(translations, stable)?;
        let domain = component_domain(graph, nodes)?;
        let (stride, stable) = match replication {
            Replication::Array(stride) => (
                stride,
                PositionRelation {
                    array: Link::IDENTITY,
                    packed: stable.packed,
                },
            ),
            Replication::Packed(stride) => (
                stride,
                PositionRelation {
                    array: stable.array,
                    packed: Link::IDENTITY,
                },
            ),
        };
        let local = nodes
            .iter()
            .enumerate()
            .map(|(index, &node)| (node, index))
            .collect::<HashMap<_, _>>();
        let mut adjacency = vec![Vec::new(); nodes.len()];
        // A value moves only through a translating edge, from the positions
        // its source holds. Unbounded when a source holds every position.
        let mut moving = Some(Bound::Empty);
        for &node in nodes {
            for edge in graph.edges(node) {
                let Some(&target) = local.get(&edge.target()) else {
                    continue;
                };
                let offset = match replication {
                    Replication::Array(_) => edge.weight().relation.array,
                    Replication::Packed(_) => edge.weight().relation.packed,
                }
                .translation_offset()?;
                // Uniform translations share the stride's sign and divide by it.
                let units = usize::try_from(offset / stride).ok()?;
                adjacency[local[&node]].push((target, units, edge.weight().data));
                if units != 0 {
                    moving = moving.and_then(|bound| {
                        let domains = &graph[node].domains;
                        if domains.is_empty() {
                            return None;
                        }
                        let mut bound = bound.join(Bound::of(domains));
                        for domain in domains {
                            bound =
                                bound.join(Bound::Hull(translate(*domain, replication, offset)?));
                        }
                        Some(bound)
                    });
                }
            }
        }
        // The replicated members also move their values.
        if nodes.iter().any(|&node| graph[node].replication.is_some()) {
            moving = None;
        }
        let moving = match moving {
            None | Some(Bound::Unbounded) => Some(domain),
            Some(Bound::Empty) => None,
            Some(Bound::Hull(bound)) => intersect(domain, bound, replication),
        };
        Some(Self {
            replication,
            domain,
            moving,
            stable,
            members: nodes.to_vec(),
            local,
            adjacency,
        })
    }

    fn stride(&self) -> isize {
        match self.replication {
            Replication::Array(stride) | Replication::Packed(stride) => stride,
        }
    }

    /// Least translation units from `entry` to each member over paths with
    /// an explicit read, and whether a retention-only path exists.
    fn distances(&self, entry: usize, data: bool) -> (Vec<Option<usize>>, Vec<bool>) {
        let count = self.members.len();
        let mut best = vec![[None::<usize>; 2]; count];
        let mut heap = std::collections::BinaryHeap::new();
        best[entry][usize::from(data)] = Some(0);
        heap.push(std::cmp::Reverse((0usize, entry, data)));
        while let Some(std::cmp::Reverse((distance, node, seen))) = heap.pop() {
            if best[node][usize::from(seen)].is_some_and(|best| best < distance) {
                continue;
            }
            for &(target, units, data) in &self.adjacency[node] {
                let seen = seen || data;
                let Some(next) = distance.checked_add(units) else {
                    continue;
                };
                let slot = &mut best[target][usize::from(seen)];
                if slot.is_none_or(|best| next < best) {
                    *slot = Some(next);
                    heap.push(std::cmp::Reverse((next, target, seen)));
                }
            }
        }
        (
            best.iter().map(|states| states[1]).collect(),
            best.iter().map(|states| states[0].is_some()).collect(),
        )
    }

    fn translated(&self, units: usize) -> Option<PositionRelation> {
        let offset = isize::try_from(units).ok()?.checked_mul(self.stride())?;
        Some(match self.replication {
            Replication::Array(_) => PositionRelation {
                array: Link::from_offset(Some(offset)),
                ..self.stable
            },
            Replication::Packed(_) => PositionRelation {
                packed: Link::from_offset(Some(offset)),
                ..self.stable
            },
        })
    }

    fn materialize<K: Copy + Eq + Hash>(
        &self,
        ssa: &mut SsaStore<K>,
        graph: &TransferGraph,
        incoming: &[IncomingEdge<'_>],
        layers: &mut Layers<'_>,
    ) {
        // Only members observed outside the component need a value.
        let exits = self
            .members
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                graph[**node].is_output
                    || graph
                        .edges(**node)
                        .any(|edge| !self.local.contains_key(&edge.target()))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let mut entries: HashMap<(usize, bool), Vec<&IncomingEdge<'_>>> = HashMap::default();
        for edge in incoming {
            entries
                .entry((self.local[&edge.target()], edge.weight().data))
                .or_default()
                .push(edge);
        }
        let edges = self.adjacency.iter().map(Vec::len).sum::<usize>();
        let fits = entries
            .len()
            .checked_mul(self.members.len().saturating_add(edges))
            .is_some_and(|work| work <= UNIFORM_RECURRENCE_WORK);
        let mut retained = vec![Vec::new(); self.members.len()];
        let mut data_inputs = vec![Vec::new(); self.members.len()];
        let mut ordered = entries.into_iter().collect::<Vec<_>>();
        ordered.sort_unstable_by_key(|((entry, data), _)| (*entry, *data));
        for ((entry, entry_data), edges) in ordered {
            let (reads, retains) = if fits {
                self.distances(entry, entry_data)
            } else {
                // Without distances, every member may observe every entry.
                (
                    vec![Some(0); self.members.len()],
                    vec![!entry_data; self.members.len()],
                )
            };
            let sources = edges
                .iter()
                .map(|edge| {
                    let value = layers.read(ssa, graph, edge.source());
                    let value = ssa.related_definition_guarded(
                        vec![(value, edge.weight().relation)],
                        &edge.weight().condition,
                    );
                    (
                        ssa.projected_union(value, &graph[edge.target()].domains),
                        self.stable,
                    )
                })
                .collect::<Vec<_>>();
            let joined = ssa.related_definition(sources);
            // A value at a position no translation starts from stays there.
            let held = ssa.projected(joined, self.domain);
            let repeated = self.moving.map(|moving| {
                let moving_value = ssa.projected(joined, moving);
                ssa.replicated(moving_value, moving, self.replication)
            });
            for &member in &exits {
                if retains[member] {
                    for edge in &edges {
                        let condition = &edge.weight().condition;
                        retained[member].push((edge.source(), condition.clone()));
                        if let Some(data) = layers.data[edge.source().index()] {
                            data_inputs[member].push(ssa.guarded(data, condition));
                        }
                    }
                }
                if let Some(units) = reads[member] {
                    let relation = self
                        .translated(units)
                        .unwrap_or_else(PositionRelation::whole);
                    // A path that translates starts from a position some
                    // translation starts from; one that does not may also
                    // start anywhere else.
                    let value = match (units, repeated) {
                        (0, Some(repeated)) => ssa.phi(vec![held, repeated]),
                        (0, None) => held,
                        (_, Some(repeated)) => repeated,
                        (_, None) => continue,
                    };
                    data_inputs[member].push(ssa.related_definition(vec![(value, relation)]));
                }
            }
        }
        for (index, &node) in self.members.iter().enumerate() {
            for (source, condition) in std::mem::take(&mut retained[index]) {
                layers.retain_from(node, source, &condition);
            }
            layers.data[node.index()] = join(ssa, std::mem::take(&mut data_inputs[index]))
                .map(|value| ssa.projected_union(value, &graph[node].domains));
        }
    }
}

/// `domain` moved by `offset` along the replicated axis, `None` when it
/// leaves the nonnegative positions.
fn translate(
    mut domain: PositionDomain,
    replication: Replication,
    offset: isize,
) -> Option<PositionDomain> {
    let start = match replication {
        Replication::Array(_) => &mut domain.array_start,
        Replication::Packed(_) => &mut domain.packed_start,
    };
    *start = start.checked_add_signed(offset)?;
    Some(domain)
}

/// `domain` with its replicated axis confined to that of `bound`, `None`
/// when they share no position there.
fn intersect(
    mut domain: PositionDomain,
    bound: PositionDomain,
    replication: Replication,
) -> Option<PositionDomain> {
    let (start, length, bound_start, bound_length) = match replication {
        Replication::Array(_) => (
            &mut domain.array_start,
            &mut domain.array_length,
            bound.array_start,
            bound.array_length,
        ),
        Replication::Packed(_) => (
            &mut domain.packed_start,
            &mut domain.packed_length,
            bound.packed_start,
            bound.packed_length,
        ),
    };
    let first = (*start).max(bound_start);
    let end = (*start + *length).min(bound_start + bound_length);
    if first >= end {
        return None;
    }
    *start = first;
    *length = end - first;
    Some(domain)
}

/// Translations of the internal edges of one component along one axis.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AxisTranslation {
    Stable,
    /// Every non-zero translation is a multiple of `stride` with its sign.
    Uniform(isize),
    Unlinked,
}

impl AxisTranslation {
    fn with(self, offset: Option<isize>) -> Self {
        let Some(offset) = offset else {
            return Self::Unlinked;
        };
        match self {
            Self::Unlinked => Self::Unlinked,
            _ if offset == 0 => self,
            Self::Stable => Self::Uniform(offset),
            Self::Uniform(stride) if (stride > 0) == (offset > 0) => {
                let gcd = greatest_common_divisor(stride.unsigned_abs(), offset.unsigned_abs());
                isize::try_from(gcd)
                    .map(|gcd| Self::Uniform(if stride > 0 { gcd } else { -gcd }))
                    .unwrap_or(Self::Unlinked)
            }
            Self::Uniform(_) => Self::Unlinked,
        }
    }
}

/// A recurrence that advances along a single axis in one direction is a
/// bounded repetition of that translation. The other axis is either preserved
/// or already unlinked by `stable`, which every entering source receives.
fn recurrence_replication(
    translations: &[AxisTranslation; 2],
    _stable: &PositionRelation,
) -> Option<Replication> {
    match translations {
        [
            AxisTranslation::Uniform(stride),
            AxisTranslation::Stable | AxisTranslation::Unlinked,
        ] => Some(Replication::Array(*stride)),
        [
            AxisTranslation::Stable | AxisTranslation::Unlinked,
            AxisTranslation::Uniform(stride),
        ] => Some(Replication::Packed(*stride)),
        _ => None,
    }
}

/// The bounding box of every constrained member domain. A larger domain only
/// admits additional intermediate positions, so it remains a conservative
/// bound. Every cycle of a transfer passes through an iteration input, so
/// with constrained inputs and monotone translations along one axis, each
/// unconstrained intermediate position lies between two bounded positions.
fn component_domain(graph: &TransferGraph, nodes: &[NodeIndex]) -> Option<PositionDomain> {
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for &node in nodes {
        if graph[node].domains.is_empty() {
            if graph[node].is_iteration_input {
                return None;
            }
            continue;
        }
        for domain in &graph[node].domains {
            let array_end = domain.array_start.checked_add(domain.array_length)?;
            let packed_end = domain.packed_start.checked_add(domain.packed_length)?;
            bounds = Some(match bounds {
                None => (
                    domain.array_start,
                    array_end,
                    domain.packed_start,
                    packed_end,
                ),
                Some((array_start, array_stop, packed_start, packed_stop)) => (
                    array_start.min(domain.array_start),
                    array_stop.max(array_end),
                    packed_start.min(domain.packed_start),
                    packed_stop.max(packed_end),
                ),
            });
        }
    }
    let (array_start, array_end, packed_start, packed_end) = bounds?;
    Some(PositionDomain {
        array_start,
        array_length: array_end - array_start,
        packed_start,
        packed_length: packed_end - packed_start,
    })
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
        ssa.try_close_repeated_transfer(
            RepeatedIteration::new(&iteration, inner, true),
            &mut work,
            |_| Some(domain),
            |_| TransferCoverage::default(),
        )
        .expect("directly written SSA does not consume the copy budget");

        let iteration = ssa.capture_and_rollback(outer);
        let before = ssa.versions.len();
        let mut work = 64;
        assert!(
            ssa.try_close_repeated_transfer(
                RepeatedIteration::new(&iteration, outer, true),
                &mut work,
                |_| Some(domain),
                |_| { TransferCoverage::default() }
            )
            .is_none()
        );
        assert_eq!(ssa.versions.len(), before, "reject before SSA condensation");
        assert_eq!(ssa.read("output"), initial, "no partial output binding");

        let mut work = 1024;
        ssa.try_close_repeated_transfer(
            RepeatedIteration::new(&iteration, outer, true),
            &mut work,
            |_| Some(domain),
            |_| TransferCoverage::default(),
        )
        .expect("a bounded nested transfer still preserves its dependencies");
        let output = ssa.read("output");
        assert_eq!(
            ssa.root_source_relations(output),
            HashMap::from_iter([("input", PositionRelation::default())])
        );
    }

    #[test]
    fn repeated_transfer_observes_only_the_keys_it_can_change() {
        let mut ssa = SsaStore::default();
        let function = ssa.checkpoint();
        let seed = ssa.read("seed");
        let shared = ssa.definition(vec![seed]);
        ssa.bind("written", shared);
        ssa.bind("kept", shared);
        let body = ssa.checkpoint();
        let mut observed = [ssa.snapshot_since(function)];
        let mut work = usize::MAX;
        let previous = ssa.read("written");
        let input = ssa.read("input");
        let output = ssa.definition(vec![previous, input]);
        ssa.bind("written", output);
        let iteration = ssa.capture_and_rollback(body);
        ssa.try_close_repeated_transfer(
            RepeatedIteration {
                observed: &mut observed,
                ..RepeatedIteration::new(&iteration, body, true)
            },
            &mut work,
            |_| None,
            |_| TransferCoverage::default(),
        )
        .expect("unlimited runtime transfer construction");
        let [observed] = observed;
        // A later iteration sees what earlier ones wrote, but a key that only
        // shares the written key's entry version still holds that version.
        assert_eq!(
            ssa.root_sources(observed.bindings[&"written"]),
            HashSet::from_iter(["seed", "input"])
        );
        assert_eq!(observed.bindings[&"kept"], shared);
        ssa.capture_and_rollback(function);
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
            let views = HashMap::default();
            let layers = condense(&mut ssa, &builder.graph, &views);
            for actual in 0..actuals.len() {
                for index in [0, size / 2, size - 1] {
                    let output = outputs[actual * size + index];
                    let retained = layers.retained_value(&mut ssa, &builder.graph, output);
                    let output = full(&mut ssa, retained, layers.data[output.index()]);
                    assert_eq!(ssa.root_sources(output), HashSet::from_iter([actual]));
                }
            }
        }
    }

    #[test]
    fn repeated_transfer_retains_acyclic_replication_at_scale() {
        for width in [8, 1 << 30] {
            for replication in [Replication::Packed(2), Replication::Array(2)] {
                for imported in [false, true] {
                    let mut ssa = SsaStore::default();
                    let input = ssa.read("input");
                    let domain = match replication {
                        Replication::Packed(_) => PositionDomain {
                            array_start: 1,
                            array_length: 1,
                            packed_start: 0,
                            packed_length: width,
                        },
                        Replication::Array(_) => PositionDomain {
                            array_start: 0,
                            array_length: width,
                            packed_start: 1,
                            packed_length: 1,
                        },
                    };
                    let seed_domain = match replication {
                        Replication::Packed(_) => PositionDomain {
                            packed_length: 2,
                            ..domain
                        },
                        Replication::Array(_) => PositionDomain {
                            array_length: 2,
                            ..domain
                        },
                    };
                    let checkpoint = ssa.checkpoint();
                    let value = if imported {
                        let mut callee = SsaStore::default();
                        let source = callee.read("source");
                        let source = callee.projected(source, seed_domain);
                        let result = callee.replicated(source, domain, replication);
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
                        let source = ssa.projected(input, seed_domain);
                        ssa.replicated(source, domain, replication)
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
                            matches!(kind, DependencyDagNode::Replicated { replication: actual }
                            if *actual == replication)
                            .then_some(node)
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(replicas.len(), 1);
                    assert_eq!(dag.domains[replicas[0]], [domain]);
                }
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
                        array: Link::from_offset(Some(0)),
                        packed: Link::from_offset(Some(shift)),
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
                            array: Link::from_offset(Some(0)),
                            packed: Link::from_offset((shift == 0).then_some(0)),
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
                            array: Link::from_offset(Some(0)),
                            packed: Link::from_offset(offset),
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
                    // Skipping the loop retains the entry value as state,
                    // which is not a combinational dependency.
                    let mut expected = HashSet::default();
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
                    let in_domain = |node: usize, bit: usize| {
                        let domains = &dag.domains[node];
                        domains.is_empty()
                            || domains.iter().any(|domain| {
                                domain.packed_start <= bit
                                    && bit < domain.packed_start + domain.packed_length
                            })
                    };
                    while let Some((node, bit)) = queue.pop_front() {
                        // A replicated node repeats its translation within
                        // its domain, as its self edge in the circuit graph.
                        if let DependencyDagNode::Replicated {
                            replication: Replication::Packed(stride),
                        } = dag.nodes[node]
                            && let Some(next) = bit.checked_add_signed(stride)
                            && in_domain(node, next)
                            && reached.insert((node, next))
                        {
                            queue.push_back((node, next));
                        }
                        for edge in &outgoing[node] {
                            for next in positions(bit, edge.relation.packed.translation_offset()) {
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
