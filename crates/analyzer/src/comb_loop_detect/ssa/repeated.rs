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
use daggy::petgraph::Direction;
use daggy::petgraph::Graph;
use daggy::petgraph::algo::kosaraju_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;
use std::collections::BTreeSet;

#[derive(Default)]
struct TransferNode {
    input: Option<VersionId>,
    domains: Vec<PositionDomain>,
    replication: Option<Replication>,
    /// The node computes a value from its sources rather than keeping one
    /// of them.
    computes: bool,
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
                Version::Overlay { .. } => 3,
                Version::Restricted { .. } => 2,
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
        work: &mut usize,
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
                    self.graph[node].computes = true;
                    for &(source, relation) in sources {
                        let source = self.version(ssa, source, start, work)?;
                        self.graph.add_edge(source, node, relation);
                    }
                }
                Version::Phi(inputs) => {
                    for &source in inputs {
                        let source = self.version(ssa, source, start, work)?;
                        self.graph
                            .add_edge(source, node, PositionRelation::default());
                    }
                }
                Version::Guarded { source, .. } => {
                    let source = self.version(ssa, *source, start, work)?;
                    self.graph
                        .add_edge(source, node, PositionRelation::default());
                }
                Version::Overlay {
                    below,
                    retained,
                    above,
                } => {
                    let below = self.version(ssa, *below, start, work)?;
                    let below = match retained {
                        Some(domains) => {
                            // Only the retained positions keep the value below.
                            let restricted = self.graph.add_node(TransferNode {
                                domains: domains.to_vec(),
                                ..TransferNode::default()
                            });
                            self.graph
                                .add_edge(below, restricted, PositionRelation::default());
                            restricted
                        }
                        None => below,
                    };
                    self.graph
                        .add_edge(below, node, PositionRelation::default());
                    let above = self.version(ssa, *above, start, work)?;
                    self.graph
                        .add_edge(above, node, PositionRelation::default());
                }
                Version::Restricted { source, domain } => {
                    self.graph[node].domains.push(*domain);
                    let source = self.version(ssa, *source, start, work)?;
                    self.graph
                        .add_edge(source, node, PositionRelation::default());
                }
                Version::Projected { source, domain } => {
                    self.graph[node].domains.push(*domain);
                    let source = self.version(ssa, *source, start, work)?;
                    self.graph
                        .add_edge(source, node, PositionRelation::default());
                }
                Version::Replicated {
                    source,
                    domain,
                    replication,
                } => {
                    self.graph[node].domains.push(*domain);
                    self.graph[node].replication = Some(*replication);
                    self.graph[node].computes = true;
                    let source = self.version(ssa, *source, start, work)?;
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
                    *work = work.checked_sub(1)?;
                    self.graph[node].computes = true;
                    let index = match imports.entry(Rc::as_ptr(graph)) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(ImportIndex::try_new(graph, work)?)
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
                        *work = work.checked_sub(
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
                            computes: true,
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
                            *work = work.checked_sub(sources.len())?;
                            for &(source, relation) in sources {
                                let source = self.version(ssa, source, start, work)?;
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
    work: &mut usize,
    domain: impl Fn(K) -> Option<PositionDomain>,
) -> Option<()> {
    let mut builder = TransferBuilder::default();
    let start = checkpoint.version_start;
    let outputs = iteration
        .bindings
        .iter()
        .map(|(&key, &output)| {
            let entry = ssa.read(key);
            let input = builder.version(ssa, entry, start, work)?;
            let value = builder.version(ssa, output, start, work)?;
            let domains = domain(key).into_iter().collect::<Vec<_>>();
            let root = builder.graph.add_node(TransferNode {
                domains: domains.clone(),
                ..TransferNode::default()
            });
            builder
                .graph
                .add_edge(value, root, PositionRelation::default());
            Some((key, entry, input, root, domains))
        })
        .collect::<Option<Vec<_>>>()?;
    builder.copy_iteration(ssa, start, work)?;

    let mut unrestricted = HashSet::default();
    for (_, entry, input, root, domains) in &outputs {
        // Separate the immutable first-iteration input from the join that
        // also accepts prior iterations. Multiple keys may share a version;
        // their domains are alternatives, not intersecting restrictions.
        if builder.graph[*input].input.take().is_some() {
            let initial = builder.graph.add_node(TransferNode {
                input: Some(*entry),
                ..TransferNode::default()
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

    let refined = refine(&builder.graph, work)?;
    let generated_start = ssa.versions.len();
    let mapped = condense(ssa, &refined.graph);
    for (key, entry, _, root, domains) in outputs {
        let output = match domains.as_slice() {
            [extent] => {
                // The value after the loop: each cell that an iteration may
                // change, written over the value before the loop. Cells that
                // only keep that value stay as they were, with their history.
                let mut output = entry;
                for &(part, cell) in &refined.parts[root.index()] {
                    if refined.keeps_entry(part, entry, work)? {
                        continue;
                    }
                    let region = cell.unwrap_or(*extent);
                    output = ssa.overlay(output, mapped[part.index()], region, *extent, !may_skip);
                }
                output
            }
            _ => {
                let parts = refined.parts[root.index()]
                    .iter()
                    .map(|(part, _)| mapped[part.index()])
                    .collect::<Vec<_>>();
                let mut output = ssa.phi(parts);
                if may_skip {
                    // Skipping the loop keeps the value from before it.
                    let entry = restrict(ssa, entry, &domains);
                    output = ssa.phi(vec![output, entry]);
                }
                output
            }
        };
        ssa.bind(key, output);
    }
    if generated_start < ssa.versions.len() {
        ssa.repeated_versions
            .push(generated_start..ssa.versions.len());
    }
    Some(())
}

/// A transfer graph whose nodes are divided into position cells.
struct Refined {
    graph: TransferGraph,
    /// Per node of the original graph, its cells: the node of each and the
    /// box of positions it holds, `None` when the node stays whole.
    parts: Vec<Vec<(NodeIndex, Option<PositionDomain>)>>,
}

impl Refined {
    /// Whether `part` only keeps `entry`, the value before the loop: every
    /// way into it keeps values along identity edges from that value.
    fn keeps_entry(&self, part: NodeIndex, entry: VersionId, work: &mut usize) -> Option<bool> {
        let mut visited = HashSet::from_iter([part]);
        let mut stack = vec![part];
        while let Some(node) = stack.pop() {
            *work = work.checked_sub(1)?;
            let weight = &self.graph[node];
            if let Some(input) = weight.input {
                if input != entry {
                    return Some(false);
                }
                continue;
            }
            if weight.computes || weight.replication.is_some() {
                return Some(false);
            }
            let mut incoming = self
                .graph
                .edges_directed(node, Direction::Incoming)
                .peekable();
            if incoming.peek().is_none() {
                return Some(false);
            }
            for edge in incoming {
                *work = work.checked_sub(1)?;
                if *edge.weight() != PositionRelation::default() {
                    return Some(false);
                }
                if visited.insert(edge.source()) {
                    stack.push(edge.source());
                }
            }
        }
        Some(true)
    }
}

#[cfg(test)]
thread_local! {
    static REFINE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Divide the nodes of `graph` into cells of positions before condensing it.
///
/// A recurrence condenses into one value, which loses which positions of its
/// nodes reach which. Cells keep them apart: the boundaries of each node's
/// domains are carried, within its recurrence, along every edge that keeps
/// positions on an axis, and each node is divided at the boundaries it
/// receives. A cell connects to a cell of a successor only when the edge can
/// bring one of its positions there. A recurrence then joins only cells that
/// feed one another, however the loop body was written, while a cell's value
/// is still the union of its node's at those positions.
fn refine(graph: &TransferGraph, work: &mut usize) -> Option<Refined> {
    let components = kosaraju_scc(graph);
    let mut component_of = vec![0; graph.node_count()];
    for (index, nodes) in components.iter().enumerate() {
        for node in nodes {
            component_of[node.index()] = index;
        }
    }
    let divisible = |node: NodeIndex| graph[node].replication.is_none();
    let mut cuts: Vec<[BTreeSet<usize>; 2]> = vec![Default::default(); graph.node_count()];
    // Boundaries a node received but has not passed on yet. Each boundary
    // crosses each edge at most once, and every crossing is charged.
    let mut fresh: Vec<[Vec<usize>; 2]> = vec![Default::default(); graph.node_count()];
    let mut pending = Vec::new();
    for node in graph.node_indices().filter(|&node| divisible(node)) {
        for domain in &graph[node].domains {
            for (axis, bounds) in [
                (
                    0,
                    [
                        domain.array_start,
                        domain.array_start.saturating_add(domain.array_length),
                    ],
                ),
                (
                    1,
                    [
                        domain.packed_start,
                        domain.packed_start.saturating_add(domain.packed_length),
                    ],
                ),
            ] {
                for bound in bounds {
                    *work = work.checked_sub(1)?;
                    if cuts[node.index()][axis].insert(bound) {
                        fresh[node.index()][axis].push(bound);
                    }
                }
            }
        }
        if !graph[node].domains.is_empty() {
            pending.push(node);
        }
    }
    while let Some(node) = pending.pop() {
        let new = std::mem::take(&mut fresh[node.index()]);
        let neighbours = graph
            .edges_directed(node, Direction::Outgoing)
            .map(|edge| (edge.target(), *edge.weight()))
            .chain(
                graph
                    .edges_directed(node, Direction::Incoming)
                    .map(|edge| (edge.source(), *edge.weight())),
            );
        for (other, relation) in neighbours {
            *work = work.checked_sub(1)?;
            #[cfg(test)]
            REFINE_VISITS.set(REFINE_VISITS.get() + 1);
            if component_of[other.index()] != component_of[node.index()] || !divisible(other) {
                continue;
            }
            let was_idle = fresh[other.index()].iter().all(Vec::is_empty);
            for (axis, offset) in [relation.array, relation.packed].into_iter().enumerate() {
                if offset != Some(0) {
                    continue;
                }
                *work = work.checked_sub(new[axis].len())?;
                #[cfg(test)]
                REFINE_VISITS.set(REFINE_VISITS.get() + new[axis].len());
                for &cut in &new[axis] {
                    if cuts[other.index()][axis].insert(cut) {
                        fresh[other.index()][axis].push(cut);
                    }
                }
            }
            if was_idle && fresh[other.index()].iter().any(|cuts| !cuts.is_empty()) {
                pending.push(other);
            }
        }
    }

    let mut refined = TransferGraph::new();
    let mut parts = Vec::with_capacity(graph.node_count());
    for node in graph.node_indices() {
        let weight = &graph[node];
        let [array, packed] = &cuts[node.index()];
        let cells = if !divisible(node) || (array.is_empty() && packed.is_empty()) {
            vec![None]
        } else {
            let intervals = |cuts: &BTreeSet<usize>| {
                let mut bounds = vec![0];
                bounds.extend(cuts.iter().copied().filter(|&cut| cut > 0));
                bounds.push(isize::MAX as usize);
                bounds.dedup();
                bounds
                    .windows(2)
                    .map(|pair| (pair[0], pair[1]))
                    .collect::<Vec<_>>()
            };
            let (arrays, packeds) = (intervals(array), intervals(packed));
            // Each cell is compared with each domain of the node, and again
            // when its domains are cut.
            *work = work.checked_sub(
                arrays
                    .len()
                    .saturating_mul(packeds.len())
                    .saturating_mul(weight.domains.len().max(1).saturating_mul(2)),
            )?;
            let mut cells = Vec::new();
            for &(array_start, array_end) in &arrays {
                for &(packed_start, packed_end) in &packeds {
                    let cell = PositionDomain {
                        array_start,
                        array_length: array_end - array_start,
                        packed_start,
                        packed_length: packed_end - packed_start,
                    };
                    // A node with domains holds only their positions.
                    if weight.domains.is_empty()
                        || weight
                            .domains
                            .iter()
                            .any(|domain| log::intersection(*domain, cell).is_some())
                    {
                        cells.push(Some(cell));
                    }
                }
            }
            cells
        };
        let node_parts = cells
            .into_iter()
            .map(|cell| {
                let domains = match cell {
                    None => weight.domains.clone(),
                    Some(cell) if weight.domains.is_empty() => vec![cell],
                    Some(cell) => weight
                        .domains
                        .iter()
                        .filter_map(|domain| log::intersection(*domain, cell))
                        .collect(),
                };
                let part = refined.add_node(TransferNode {
                    input: weight.input,
                    domains,
                    replication: weight.replication,
                    computes: weight.computes,
                });
                (part, cell)
            })
            .collect::<Vec<_>>();
        parts.push(node_parts);
    }
    for edge in graph.edge_references() {
        let relation = *edge.weight();
        let (sources, targets) = (&parts[edge.source().index()], &parts[edge.target().index()]);
        *work = work.checked_sub(sources.len().saturating_mul(targets.len()))?;
        for &(source, source_cell) in sources {
            for &(target, target_cell) in targets {
                if may_relate(source_cell, relation, target_cell) {
                    refined.add_edge(source, target, relation);
                }
            }
        }
    }
    Some(Refined {
        graph: refined,
        parts,
    })
}

/// Whether `relation` can bring a position of `source` into `target`; a
/// missing cell holds every position.
fn may_relate(
    source: Option<PositionDomain>,
    relation: PositionRelation,
    target: Option<PositionDomain>,
) -> bool {
    let (Some(source), Some(target)) = (source, target) else {
        return true;
    };
    let axis =
        |start: usize, length: usize, offset: Option<isize>, other: usize, other_length: usize| {
            let Some(offset) = offset else {
                return true;
            };
            let (Ok(start), Ok(other)) = (isize::try_from(start), isize::try_from(other)) else {
                return true;
            };
            let (Some(low), Some(high), Some(other_high)) = (
                start.checked_add(offset),
                start
                    .checked_add_unsigned(length)
                    .and_then(|end| end.checked_add(offset)),
                other.checked_add_unsigned(other_length),
            ) else {
                return true;
            };
            low < other_high && other < high
        };
    axis(
        source.array_start,
        source.array_length,
        relation.array,
        target.array_start,
        target.array_length,
    ) && axis(
        source.packed_start,
        source.packed_length,
        relation.packed,
        target.packed_start,
        target.packed_length,
    )
}

/// `value` restricted to `domains`, as retention.
fn restrict<K: Copy + Eq + Hash>(
    ssa: &mut SsaStore<K>,
    value: VersionId,
    domains: &[PositionDomain],
) -> VersionId {
    if domains.is_empty() {
        return value;
    }
    let alternatives = domains
        .iter()
        .map(|&domain| ssa.restricted(value, domain))
        .collect();
    ssa.phi(alternatives)
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
        if let Some(replication) = graph[node].replication {
            // Replication changes its axis's coordinates if it participates in
            // an actual runtime recurrence. Otherwise keep it as an operation;
            // its finite repetitions are not procedural feedback.
            let relation = &mut stable[component_of[node.index()]];
            *relation = replication.forget_position(*relation);
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
            // A recurrence that only keeps values, along identity edges,
            // holds one of its entering values; it computes nothing.
            let keeps = stable[component] == PositionRelation::default()
                && nodes.iter().all(|node| !graph[*node].computes);
            let sources = incoming[component]
                .iter()
                .map(|edge| {
                    let source = mapped[edge.source().index()];
                    let value = if keeps && *edge.weight() == PositionRelation::default() {
                        source
                    } else {
                        ssa.related_definition(vec![(source, *edge.weight())])
                    };
                    let value = if keeps {
                        restrict(ssa, value, &graph[edge.target()].domains)
                    } else {
                        project(ssa, value, &graph[edge.target()].domains)
                    };
                    (value, stable[component])
                })
                .collect::<Vec<_>>();
            let joined = if keeps {
                ssa.phi(sources.into_iter().map(|(value, _)| value).collect())
            } else {
                ssa.related_definition(sources)
            };
            // Discarding internal path restrictions is conservative; keeping
            // the entry and exit projections still bounds the affected bits.
            for &node in nodes {
                mapped[node.index()] = if keeps {
                    restrict(ssa, joined, &graph[node].domains)
                } else {
                    project(ssa, joined, &graph[node].domains)
                };
            }
        } else {
            let node = nodes[0];
            // A node that only keeps values along identity edges joins them.
            let keeps = !graph[node].computes
                && incoming[component]
                    .iter()
                    .all(|edge| *edge.weight() == PositionRelation::default());
            let value = graph[node].input.unwrap_or_else(|| {
                let sources = incoming[component]
                    .iter()
                    .map(|edge| (mapped[edge.source().index()], *edge.weight()));
                if keeps {
                    ssa.phi(sources.map(|(source, _)| source).collect())
                } else {
                    ssa.related_definition(sources.collect())
                }
            });
            mapped[node.index()] = if let Some(replication) = graph[node].replication {
                let alternatives = graph[node]
                    .domains
                    .iter()
                    .map(|&domain| ssa.replicated(value, domain, replication))
                    .collect();
                ssa.phi(alternatives)
            } else if keeps {
                restrict(ssa, value, &graph[node].domains)
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
    fn dense_transfer_boundaries_are_charged_per_crossing() {
        // Every output reads every input along identity edges, and each keeps
        // its own bit, so all nodes share one recurrence and many boundaries.
        for keys in [16usize, 64] {
            let mut ssa = SsaStore::default();
            let extent = PositionDomain {
                array_start: 0,
                array_length: 1,
                packed_start: 0,
                packed_length: keys,
            };
            let entries = (0..keys).map(|key| ssa.read(key)).collect::<Vec<_>>();
            let checkpoint = ssa.checkpoint();
            for key in 0..keys {
                let sources = entries
                    .iter()
                    .map(|&entry| (entry, PositionRelation::default()))
                    .collect();
                let value = ssa.related_definition(sources);
                let bit = PositionDomain {
                    packed_start: key,
                    packed_length: 1,
                    ..extent
                };
                let value = ssa.projected(value, bit);
                ssa.bind(key, value);
            }
            let iteration = ssa.capture_and_rollback(checkpoint);
            REFINE_VISITS.set(0);
            let budget = usize::MAX / 2;
            let mut work = budget;
            ssa.try_close_repeated_transfer(&iteration, checkpoint, true, &mut work, |_| {
                Some(extent)
            })
            .expect("an unlimited budget closes the transfer");
            let visits = REFINE_VISITS.get();
            assert!(
                visits <= budget - work,
                "{keys} keys: {visits} boundary visits for {} charged steps",
                budget - work
            );
        }
    }

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
        // Directly written SSA is not copied; dividing it into cells is
        // charged by its size.
        let mut work = 1024;
        ssa.try_close_repeated_transfer(&iteration, inner, true, &mut work, |_| Some(domain))
            .expect("directly written SSA is closed within a budget of its size");

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
                    // Skipping the loop keeps each key's entry value, which
                    // is retention rather than a dependency on it.
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
