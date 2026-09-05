//! Lazy source diagnostics for the constrained dependency graph.

use super::build_module_graph_with_trace;
use super::graph::{DependencyGraph, has_compatible_cycle, strongly_connected_components};
use super::model::{ModuleCombSummary, SummaryRegion};
use super::region::{ArraySpan, BitPartition, NodeKey};
use crate::ir::{Component, Declaration, Module, Signature, VarId, VarPath, Variable};
use crate::symbol::SymbolId;
use crate::{AnalyzerError, HashMap, HashSet};
use daggy::petgraph::Direction;
use daggy::petgraph::graph::{EdgeIndex, NodeIndex};
use daggy::petgraph::visit::EdgeRef;
use std::{collections::VecDeque, rc::Rc};
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SummaryEdgeCause {
    pub(super) inst_declaration: usize,
    pub(super) child: Signature,
    pub(super) child_source: SummaryRegion,
    pub(super) child_destination: SummaryRegion,
}

#[cfg(test)]
thread_local! {
    static DIAGNOSTIC_REPLAYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DIAGNOSTIC_PROVENANCE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DIAGNOSTIC_INSTANCE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_diagnostic_replay_count() {
    DIAGNOSTIC_REPLAYS.set(0);
}
#[cfg(test)]
pub(crate) fn diagnostic_replay_count() -> usize {
    DIAGNOSTIC_REPLAYS.get()
}
#[cfg(test)]
pub(crate) fn reset_diagnostic_provenance_build_count() {
    DIAGNOSTIC_PROVENANCE_BUILDS.set(0);
}
#[cfg(test)]
pub(crate) fn diagnostic_provenance_build_count() -> usize {
    DIAGNOSTIC_PROVENANCE_BUILDS.get()
}
#[cfg(test)]
pub(crate) fn reset_diagnostic_instance_probe_count() {
    DIAGNOSTIC_INSTANCE_PROBES.set(0);
}
#[cfg(test)]
pub(crate) fn diagnostic_instance_probe_count() -> usize {
    DIAGNOSTIC_INSTANCE_PROBES.get()
}

pub(super) type DiagnosticReplayCache = HashMap<Signature, Rc<DependencyGraph>>;

#[allow(clippy::too_many_arguments)]
pub(super) fn check_graph(
    module: &Module,
    graph: &DependencyGraph,
    bit_part: &BitPartition,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    replays: &mut DiagnosticReplayCache,
    errors: &mut Vec<AnalyzerError>,
    reported: &mut HashSet<(SymbolId, Vec<VarPath>)>,
) {
    debug_assert!(
        super::graph::unconstrained_subgraph_is_acyclic(graph),
        "unconstrained dependency nodes must be introduced as a DAG"
    );
    for scc in strongly_connected_components(graph) {
        if !has_compatible_cycle(graph, &scc) {
            continue;
        }
        let mut keys = scc
            .iter()
            .filter_map(|node| graph[*node].diagnostic)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        if keys.is_empty() {
            continue;
        }
        let mut paths = keys
            .iter()
            .filter_map(|key| {
                module
                    .variables
                    .get(&key.0)
                    .or_else(|| module.interface_members.get(&key.0))
                    .map(|var| var.path.clone())
            })
            .collect::<Vec<_>>();
        paths.sort_unstable();
        paths.dedup();
        if !reported.insert((module.signature.symbol, paths)) {
            continue;
        }
        let cycle = dependency_cycle(graph, &scc);
        let cycle_keys = cycle_nodes(graph, &cycle);
        let mut tokens = diagnostic_tokens(module, &cycle_keys);
        if tokens.is_empty() {
            tokens = diagnostic_tokens(module, &keys);
        }
        let Some(&primary) = tokens.first() else {
            continue;
        };
        let mut provenance = diagnostic_provenance(module, summaries, &cycle, replays);
        provenance.retain(|token| !tokens.contains(token));
        let identifier = module
            .variables
            .get(&keys[0].0)
            .map(|var| var.path.to_string());
        errors.push(AnalyzerError::combinational_loop(
            identifier.as_deref().unwrap_or("?"),
            &format_cycle(module, bit_part, &cycle_keys),
            &primary,
            &tokens[1..],
            &provenance,
        ));
    }
}

fn sorted_edges(
    graph: &DependencyGraph,
    node: NodeIndex,
    members: &HashSet<NodeIndex>,
) -> Vec<EdgeIndex> {
    let mut edges = graph
        .edges(node)
        .filter(|edge| members.contains(&edge.target()))
        .map(|edge| edge.id())
        .collect::<Vec<_>>();
    edges.sort_unstable_by_key(|edge| {
        let (_, target) = graph.edge_endpoints(*edge).unwrap();
        (
            graph[target].diagnostic,
            graph[target].region,
            graph[*edge].kind,
            target.index(),
            edge.index(),
        )
    });
    edges
}

fn dependency_cycle(graph: &DependencyGraph, scc: &[NodeIndex]) -> Vec<EdgeIndex> {
    if let Some(path) = super::graph::diagnostic_cycle(graph, scc) {
        return path;
    }
    let members = scc.iter().copied().collect::<HashSet<_>>();
    let start = *scc
        .iter()
        .filter(|node| graph[**node].diagnostic.is_some())
        .min_by_key(|node| graph[**node].diagnostic)
        .unwrap();
    for edge in sorted_edges(graph, start, &members) {
        let (_, next) = graph.edge_endpoints(edge).unwrap();
        if next == start
            && graph[edge]
                .kind
                .exact_offset()
                .is_none_or(|offset| offset == (0, 0))
        {
            return vec![edge];
        }
        if next == start {
            continue;
        }
        let mut queue = VecDeque::from([next]);
        let mut visited = HashSet::from_iter([next]);
        let mut predecessor = HashMap::default();
        while let Some(node) = queue.pop_front() {
            if node == start {
                let mut path = Vec::new();
                let mut current = start;
                while current != next {
                    let edge = predecessor[&current];
                    path.push(edge);
                    current = graph.edge_endpoints(edge).unwrap().0;
                }
                path.push(edge);
                path.reverse();
                return path;
            }
            for edge in sorted_edges(graph, node, &members) {
                let (_, next) = graph.edge_endpoints(edge).unwrap();
                if visited.insert(next) {
                    predecessor.insert(next, edge);
                    queue.push_back(next);
                }
            }
        }
    }
    sorted_edges(graph, start, &members)
}

fn cycle_nodes(graph: &DependencyGraph, path: &[EdgeIndex]) -> Vec<NodeKey> {
    let Some(first) = path.first() else {
        return Vec::new();
    };
    let mut keys = std::iter::once(graph.edge_endpoints(*first).unwrap().0)
        .chain(
            path.iter()
                .map(|edge| graph.edge_endpoints(*edge).unwrap().1),
        )
        .filter_map(|node| graph[node].diagnostic)
        .collect::<Vec<_>>();
    if let Some(&first) = keys.first()
        && keys.last() != Some(&first)
    {
        keys.push(first);
    }
    keys
}

fn replay(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    cache: &mut DiagnosticReplayCache,
) -> Option<Rc<DependencyGraph>> {
    if let Some(graph) = cache.get(&module.signature) {
        return Some(Rc::clone(graph));
    }
    #[cfg(test)]
    DIAGNOSTIC_REPLAYS.set(DIAGNOSTIC_REPLAYS.get() + 1);
    let (graph, _, _) = build_module_graph_with_trace(module, summaries, true).ok()?;
    let graph = Rc::new(graph);
    cache.insert(module.signature.clone(), Rc::clone(&graph));
    Some(graph)
}

fn grouped_causes(
    graph: &DependencyGraph,
    edges: &[EdgeIndex],
) -> Vec<(EdgeIndex, SummaryEdgeCause)> {
    let mut causes: Vec<(EdgeIndex, SummaryEdgeCause)> = Vec::new();
    let mut indices: HashMap<usize, usize> = HashMap::default();
    for edge in edges {
        for cause in graph.summary_causes.get(edge).into_iter().flatten() {
            if let Some(&index) = indices.get(&cause.inst_declaration) {
                causes[index].1.child_destination = cause.child_destination;
            } else {
                indices.insert(cause.inst_declaration, causes.len());
                causes.push((*edge, cause.clone()));
            }
        }
    }
    causes
}

fn diagnostic_provenance(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    cycle: &[EdgeIndex],
    cache: &mut DiagnosticReplayCache,
) -> Vec<TokenRange> {
    #[cfg(test)]
    DIAGNOSTIC_PROVENANCE_BUILDS.set(DIAGNOSTIC_PROVENANCE_BUILDS.get() + 1);
    if !module.declarations.iter().any(|decl| matches!(decl, Declaration::Inst(inst) if matches!(inst.component.as_ref(), Component::Module(_)))) {
        return Vec::new();
    }
    let Some(graph) = replay(module, summaries, cache) else {
        return Vec::new();
    };
    let Some((_, cause)) = grouped_causes(&graph, cycle).into_iter().next() else {
        return Vec::new();
    };
    enum Step<'a> {
        Expand(&'a Module, SummaryEdgeCause),
        Site(TokenRange),
    }
    let mut pending = vec![Step::Expand(module, cause)];
    let mut expanded = HashSet::default();
    let mut witnesses = Vec::new();
    while let Some(step) = pending.pop() {
        let (parent, cause) = match step {
            Step::Expand(parent, cause) => (parent, cause),
            Step::Site(token) => {
                witnesses.push(token);
                continue;
            }
        };
        #[cfg(test)]
        DIAGNOSTIC_INSTANCE_PROBES.set(DIAGNOSTIC_INSTANCE_PROBES.get() + 1);
        let Some(Declaration::Inst(inst)) = parent.declarations.get(cause.inst_declaration) else {
            continue;
        };
        let Component::Module(child) = inst.component.as_ref() else {
            continue;
        };
        if child.signature != cause.child
            || !expanded.insert((cause.child, cause.child_source, cause.child_destination))
        {
            continue;
        }
        let Some(graph) = replay(child, summaries, cache) else {
            continue;
        };
        let sources = graph
            .node_indices()
            .filter(|node| {
                graph[*node].diagnostic.is_some()
                    && overlaps(graph[*node].region, cause.child_source)
            })
            .collect::<Vec<_>>();
        let destinations = graph
            .node_indices()
            .filter(|node| {
                graph[*node].diagnostic.is_some()
                    && overlaps(graph[*node].region, cause.child_destination)
            })
            .collect::<Vec<_>>();
        let forward = reachable(&graph, sources, Direction::Outgoing);
        let backward = reachable(&graph, destinations, Direction::Incoming);
        let nodes = forward
            .intersection(&backward)
            .copied()
            .collect::<HashSet<_>>();
        let mut edges = graph
            .edge_references()
            .filter(|edge| nodes.contains(&edge.source()) && nodes.contains(&edge.target()))
            .map(|edge| edge.id())
            .collect::<Vec<_>>();
        // Order source sites and nested expansions by dependency order, even
        // when declarations are written in reverse order in the source.
        let mut indegree: HashMap<NodeIndex, usize> = nodes.iter().map(|node| (*node, 0)).collect();
        for edge in &edges {
            *indegree
                .get_mut(&graph.edge_endpoints(*edge).unwrap().1)
                .unwrap() += 1;
        }
        let mut starts = nodes
            .iter()
            .copied()
            .filter(|node| indegree[node] == 0)
            .collect::<Vec<_>>();
        starts.sort_unstable();
        let mut queue = starts
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        let mut rank = HashMap::default();
        while let Some(node) = queue.pop_first() {
            rank.insert(node, rank.len());
            for edge in graph
                .edges(node)
                .filter(|edge| nodes.contains(&edge.target()))
            {
                let degree = indegree.get_mut(&edge.target()).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    queue.insert(edge.target());
                }
            }
        }
        let mut residual = nodes
            .iter()
            .copied()
            .filter(|node| !rank.contains_key(node))
            .collect::<Vec<_>>();
        residual.sort_unstable();
        for node in residual {
            rank.insert(node, rank.len());
        }
        let data_exists = graph.sites.iter().any(|(node, site)| {
            nodes.contains(node) && site.data_inputs.iter().any(|input| nodes.contains(input))
        });
        let mut steps = Vec::new();
        for (node, site) in &graph.sites {
            if nodes.contains(node)
                && (!data_exists || site.data_inputs.iter().any(|input| nodes.contains(input)))
            {
                steps.push((rank[node], Step::Site(site.token)));
            }
        }
        edges.sort_unstable_by_key(|edge| {
            (rank[&graph.edge_endpoints(*edge).unwrap().0], edge.index())
        });
        for (edge, cause) in grouped_causes(&graph, &edges) {
            let order = rank[&graph.edge_endpoints(edge).unwrap().0];
            steps.push((order, Step::Expand(child, cause)));
        }
        steps.sort_by_key(|(order, _)| *order);
        pending.extend(steps.into_iter().rev().map(|(_, step)| step));
    }
    let mut seen = HashSet::default();
    witnesses.retain(|token| seen.insert(*token));
    witnesses
}

fn overlaps(left: SummaryRegion, right: SummaryRegion) -> bool {
    left.id == right.id && left.array.overlaps(right.array) && left.packed.overlaps(right.packed)
}

fn reachable(
    graph: &DependencyGraph,
    starts: Vec<NodeIndex>,
    direction: Direction,
) -> HashSet<NodeIndex> {
    let mut seen = HashSet::from_iter(starts.iter().copied());
    let mut queue = VecDeque::from(starts);
    while let Some(node) = queue.pop_front() {
        for neighbor in graph.neighbors_directed(node, direction) {
            if seen.insert(neighbor) {
                queue.push_back(neighbor);
            }
        }
    }
    seen
}

fn diagnostic_tokens(
    module: &Module,
    keys: &[NodeKey],
) -> Vec<veryl_parser::token_range::TokenRange> {
    let mut tokens = Vec::new();
    let mut seen_var: HashSet<VarId> = HashSet::default();
    for (id, _, _) in keys {
        if !seen_var.insert(*id) {
            continue;
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
        tokens.retain(|t| seen.insert(*t));
    }
    tokens
}

fn format_cycle(module: &Module, bit_part: &BitPartition, keys: &[NodeKey]) -> String {
    let mut names = Vec::new();
    // `dependency_cycle` repeats the first node at the end. Render each
    // region once per adjacent run, then close the human-readable cycle.
    for key in keys.iter().take(keys.len().saturating_sub(1)) {
        let name = format_cycle_node(module, bit_part, *key);
        if names.last() != Some(&name) {
            names.push(name);
        }
    }
    if let Some(first) = names.first().cloned()
        && (names.len() == 1 || names.last() != Some(&first))
    {
        names.push(first);
    }
    names.join(" -> ")
}

fn format_cycle_node(module: &Module, bit_part: &BitPartition, key: NodeKey) -> String {
    let (id, array, range) = key;
    let variable = module
        .variables
        .get(&id)
        .or_else(|| module.interface_members.get(&id));
    let mut name = variable.map_or_else(|| id.to_string(), |v| v.path.to_string());

    let Some(variable) = variable else {
        return name;
    };
    if variable.r#type.total_array() != Some(array.length) || array.start != 0 {
        if let Some(indices) = array_prefix_indices(&variable.r#type.array, array) {
            name = format_array_path(variable, &indices);
        } else if array.length == 1 {
            name.push_str(&format!("[flat {}]", array.start));
        } else if let Some(end) = array.end().and_then(|end| end.checked_sub(1)) {
            let flat = if variable.r#type.array.dims() > 1 {
                "flat "
            } else {
                ""
            };
            name.push_str(&format!("[{flat}{}..={end}]", array.start));
        }
    }

    if let Some(packed) = bit_part.ranges_of((id, array)).get(range)
        && (variable.r#type.total_width() != Some(packed.length) || packed.start != 0)
    {
        if packed.length == 1 {
            name.push_str(&format!("[{}]", packed.start));
        } else {
            name.push_str(&format!("[{}:{}]", packed.end() - 1, packed.start));
        }
    }
    name
}

fn format_array_path(variable: &Variable, indices: &[usize]) -> String {
    if indices.len() > variable.array_path_offsets.len() || variable.path.0.is_empty() {
        let mut name = variable.path.to_string();
        for index in indices {
            name.push_str(&format!("[{index}]"));
        }
        return name;
    }

    let mut selections = vec![Vec::new(); variable.path.0.len()];
    for (&index, &offset) in indices.iter().zip(&variable.array_path_offsets) {
        let Some(owner) = variable.path.0.len().checked_sub(offset + 1) else {
            let mut name = variable.path.to_string();
            for index in indices {
                name.push_str(&format!("[{index}]"));
            }
            return name;
        };
        selections[owner].push(index);
    }

    let mut name = String::new();
    for (position, segment) in variable.path.0.iter().enumerate() {
        if position != 0 {
            name.push('.');
        }
        name.push_str(&segment.to_string());
        for index in &selections[position] {
            name.push_str(&format!("[{index}]"));
        }
    }
    name
}

fn array_prefix_indices(shape: &crate::ir::Shape, span: ArraySpan) -> Option<Vec<usize>> {
    let dimensions: Vec<usize> = shape.iter().copied().collect::<Option<_>>()?;
    let total = shape.total()?;
    if span.length == 0 || dimensions.contains(&0) || span.end().is_none_or(|end| end > total) {
        return None;
    }

    // A flat span spells as leading array indices when it covers one complete,
    // aligned suffix of the declared shape. Prefer the longest prefix so unit
    // dimensions remain explicit, as they are for a single-element span.
    let mut suffix_length = 1usize;
    let mut prefix_dimensions = None;
    for prefix in (0..=dimensions.len()).rev() {
        if suffix_length == span.length && span.start.is_multiple_of(suffix_length) {
            prefix_dimensions = Some(prefix);
            break;
        }
        if prefix != 0 {
            suffix_length = suffix_length.checked_mul(dimensions[prefix - 1])?;
        }
    }

    let mut indices = unflatten_array_index(shape, span.start)?;
    indices.truncate(prefix_dimensions?);
    Some(indices)
}

fn unflatten_array_index(shape: &crate::ir::Shape, flat: usize) -> Option<Vec<usize>> {
    let dimensions: Vec<usize> = shape.iter().copied().collect::<Option<_>>()?;
    if flat >= shape.total()? || dimensions.contains(&0) {
        return None;
    }

    let mut flat = flat;
    let mut indices = vec![0; dimensions.len()];
    for (index, dimension) in dimensions.iter().enumerate().rev() {
        indices[index] = flat % dimension;
        flat /= dimension;
    }
    Some(indices)
}
