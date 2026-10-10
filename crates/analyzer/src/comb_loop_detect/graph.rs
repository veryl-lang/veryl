//! Dependency graph storage, edge normalization, and cycle detection.

mod guarded;
mod relation;

use super::steps::Steps;
use relation::{PathPieces, PieceStates};

use super::diagnostics::SummaryEdgeCause;
use super::model::{AxisBounds, bounds_domain, domain_bounds, image};
use super::model::{BitDependency, SummaryRegion};
#[cfg(test)]
use super::region::translate_position;
use super::region::{BitPartition, NodeKey};
use super::ssa::DefinitionSite;
use super::ssa::{PathCondition, PositionDomain};
#[cfg(test)]
use crate::ir::VarId;
use crate::{HashMap, HashSet};
use daggy::petgraph::Direction;
use daggy::petgraph::Graph;
use daggy::petgraph::algo::kosaraju_scc;
#[cfg(test)]
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::{EdgeIndex, NodeIndex};
use daggy::petgraph::visit::EdgeRef;
#[cfg(test)]
use guarded::compatible_cycle_displacements_cancel;
use guarded::{GuardedCycle, guarded_cycle_displacements_cancel};
use relation::PositionRelationSet;
use std::collections::VecDeque;
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
    pub(super) sites: HashMap<NodeIndex, DefinitionSite<NodeIndex>>,
    pub(super) summary_causes: HashMap<EdgeIndex, Vec<SummaryEdgeCause>>,
    pub(super) active_summary: Option<SummaryEdgeCause>,
    // Region-restricted entries into storage nodes, by node and region.
    pub(super) carriers: HashMap<(NodeIndex, PositionDomain), NodeIndex>,
    // In a split graph, the edge of the original graph each edge comes from.
    pub(super) origins: HashMap<EdgeIndex, EdgeIndex>,
}

impl DependencyGraph {
    pub(super) fn new() -> Self {
        Self {
            graph: Graph::new(),
            edges: HashMap::default(),
            sites: HashMap::default(),
            summary_causes: HashMap::default(),
            active_summary: None,
            carriers: HashMap::default(),
            origins: HashMap::default(),
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
    let edge = if let Some(&existing) = graph.edges.get(&key) {
        let weight = graph
            .edge_weight_mut(existing)
            .expect("an edge found in the graph must remain present");
        weight.condition = weight.condition.disjoin(&dependency.condition);
        existing
    } else {
        let edge = graph.add_edge(source, destination, dependency);
        graph.edges.insert(key, edge);
        edge
    };
    if let Some(cause) = graph.active_summary.clone() {
        graph.summary_causes.entry(edge).or_default().push(cause);
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

/// Whether `dependency` can relate a position `source` admits to one that
/// `destination` admits.
#[cfg(test)]
pub(super) fn node_regions_overlap_with_dependency(
    source: &GraphNode,
    destination: &GraphNode,
    dependency: BitDependency,
) -> bool {
    regions_overlap_with_dependency(
        (source.region, &source.domains),
        (destination.region, &destination.domains),
        dependency,
    )
}

/// As `node_regions_overlap_with_dependency`, for a node given by its region
/// and domains. A single domain admits only that box; otherwise the region
/// bounds the positions.
pub(super) fn regions_overlap_with_dependency(
    source: (SummaryRegion, &[PositionDomain]),
    destination: (SummaryRegion, &[PositionDomain]),
    dependency: BitDependency,
) -> bool {
    let range = |start: usize, length: usize| {
        let start = isize::try_from(start).ok()?;
        Some((start, start.checked_add_unsigned(length)?))
    };
    let extent = |(region, domains): (SummaryRegion, &[PositionDomain])| {
        let domain = match domains {
            [domain] => Some((
                range(domain.array_start, domain.array_length)?,
                range(domain.packed_start, domain.packed_length)?,
            )),
            _ => None,
        };
        domain.or_else(|| {
            Some((
                range(region.array.start, region.array.length)?,
                range(region.packed.start, region.packed.length)?,
            ))
        })
    };
    let (Some((source_array, source_packed)), Some((destination_array, destination_packed))) =
        (extent(source), extent(destination))
    else {
        return false;
    };
    let source = [source_array, source_packed];
    dependency.may_reach(0, source, destination_array)
        && dependency.may_reach(1, source, destination_packed)
}

/// Both passes use explicit worklists, including for long acyclic chains.
pub(super) fn strongly_connected_components(graph: &DependencyGraph) -> Vec<Vec<NodeIndex>> {
    kosaraju_scc(&**graph)
}

pub(super) fn unconstrained_subgraph_is_acyclic(graph: &DependencyGraph) -> bool {
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

#[cfg(test)]
thread_local! {
    static SEARCH_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DECISION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_cycle_search_work() {
    SEARCH_WORK.set(0);
}
#[cfg(test)]
pub(crate) fn cycle_search_work() -> usize {
    SEARCH_WORK.get()
}
#[cfg(test)]
pub(crate) fn reset_cycle_decision_work() {
    DECISION_WORK.set(0);
}
#[cfg(test)]
pub(crate) fn cycle_decision_work() -> usize {
    DECISION_WORK.get()
}

/// The steps of one search. It counts both transitions and dominance
/// comparisons: a bound on queued states alone still permits quadratic work
/// in the per-node antichains. Exhaustion means incomplete analysis, never an
/// invented cycle or a proof of absence.
struct SearchBudget {
    remaining: usize,
    exhausted: bool,
}

impl SearchBudget {
    #[cfg(test)]
    fn new() -> Self {
        Self {
            remaining: super::steps::STEP_LIMIT,
            exhausted: false,
        }
    }

    /// Search with the steps that remain for the module, keeping what the
    /// search leaves.
    fn with_steps<T>(steps: &Steps, search: impl FnOnce(&mut Self) -> T) -> T {
        steps.lend(|remaining| {
            let mut budget = Self {
                remaining: *remaining,
                exhausted: false,
            };
            let result = search(&mut budget);
            *remaining = budget.remaining;
            result
        })
    }

    // Composition distributes over both unions. Charge before allocating
    // that Cartesian product; normalization charges its own comparisons.
    fn spend_pieces(&mut self, left: usize, right: usize) -> bool {
        self.spend(left.saturating_mul(right).saturating_add(1))
    }

    // Pairwise work over a Cartesian product of feasible positions.
    fn spend_product(&mut self, left: usize, right: usize) -> bool {
        let pieces = left.saturating_mul(right);
        self.spend(pieces.saturating_pow(2).saturating_add(1))
    }

    fn spend_conditions(&mut self, left: &PathCondition, right: &PathCondition) -> bool {
        self.spend(
            left.work_size()
                .saturating_add(right.work_size())
                .saturating_add(1),
        )
    }

    fn spend_guard_comparison(&mut self, left: &PathCondition, right: &PathCondition) -> bool {
        self.spend(
            left.work_size()
                .saturating_add(right.work_size())
                .saturating_pow(2)
                .max(1),
        )
    }

    fn spend(&mut self, work: usize) -> bool {
        if self.exhausted || work > self.remaining {
            self.exhausted = true;
            return false;
        }
        self.remaining -= work;
        #[cfg(test)]
        SEARCH_WORK.set(SEARCH_WORK.get().saturating_add(work));
        true
    }
}

/// Insert a relation/guard state, merging only exact unions at the same
/// relation. A FIFO worklist lets independent diamond arms meet at their join
/// before traversing the shared suffix; obsolete queued states are skipped.
fn insert_cycle_state<R: Clone + Eq>(
    states: &mut Vec<(R, PathCondition)>,
    relation: &R,
    condition: PathCondition,
    covers: impl Fn(&R, &R) -> bool,
    size: impl Fn(&R) -> usize,
    budget: &mut SearchBudget,
) -> Option<PathCondition> {
    let mut condition = condition;
    let relation_size = size(relation);
    let comparison_work = |r: &R| size(r).saturating_mul(relation_size).saturating_add(1);
    // Different translations usually cannot dominate or merge. Charge guard
    // comparisons only after the positional check admits them, and stop
    // charging a scan as soon as its result is known.
    for (r, c) in states.iter() {
        if !budget.spend(comparison_work(r)) {
            return None;
        }
        if covers(r, relation) {
            if !budget.spend_guard_comparison(c, &condition) {
                return None;
            }
            if c.covers(&condition) {
                return None;
            }
        }
    }
    loop {
        let mut merge = None;
        for (index, (r, c)) in states.iter().enumerate() {
            if !budget.spend(comparison_work(r)) {
                return None;
            }
            if r == relation {
                if !budget.spend_guard_comparison(c, &condition) {
                    return None;
                }
                if let Some(merged) = c.disjoin_exact(&condition) {
                    merge = Some((index, merged));
                    break;
                }
            }
        }
        let Some((index, merged)) = merge else {
            break;
        };
        states.swap_remove(index);
        condition = merged;
    }
    let mut index = 0;
    while index < states.len() {
        let (r, c) = &states[index];
        if !budget.spend(comparison_work(r)) {
            return None;
        }
        if covers(relation, r) {
            if !budget.spend_guard_comparison(&condition, c) {
                return None;
            }
            if condition.covers(c) {
                states.swap_remove(index);
                continue;
            }
        }
        index += 1;
    }
    if !budget.spend(relation_size.saturating_add(1)) {
        return None;
    }
    states.push((relation.clone(), condition.clone()));
    Some(condition)
}

fn try_cycle_witness(cycles: &HashSet<GuardedCycle>, budget: &mut SearchBudget) -> bool {
    // A few first returns may already close a positional loop while other
    // paths keep generating states. Bound this optional check separately so
    // a difficult partial set cannot consume the entire decision budget.
    let allowance = (budget.remaining / 16).min(16_384);
    if allowance == 0 {
        return false;
    }
    let mut witness_budget = SearchBudget {
        remaining: allowance,
        exhausted: false,
    };
    let found = guarded_cycle_displacements_cancel(cycles, &mut witness_budget);
    // Work was charged by witness_budget; retain that cost without treating
    // its local cutoff as exhaustion of the complete search.
    budget.remaining -= allowance - witness_budget.remaining;
    found
}

/// Whether `scc` has a compatible cycle, or `None` when the steps ran out.
pub(super) fn compatible_cycle(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    steps: &Steps,
) -> Option<bool> {
    SearchBudget::with_steps(steps, |budget| {
        #[cfg(test)]
        let initial = budget.remaining;
        let found = has_compatible_cycle_with_budget(graph, scc, budget, None);
        // Count the decision separately from diagnostic path recovery.
        #[cfg(test)]
        DECISION_WORK.set(
            DECISION_WORK
                .get()
                .saturating_add(initial - budget.remaining),
        );
        (found || !budget.exhausted).then_some(found)
    })
}

#[cfg(test)]
fn has_compatible_cycle(graph: &DependencyGraph, scc: &[NodeIndex]) -> bool {
    compatible_cycle(graph, scc, &Steps::new())
        .expect("small reference graphs must be decided completely")
}

// Correctness argument for the graph-relative cycle decision:
//
// Interpret a graph state as `(node, array_position, packed_position)`. An edge
// with `Some(k)` maps a coordinate to `coordinate + k`; `None` relates every
// source coordinate to every coordinate in the destination domain. Interpret a
// `PathCondition` as its stored Cartesian set of branch choices. Correlations
// discarded before graph construction are deliberately not reintroduced here.
//
// For a fixed anchor, each node keeps a table from relation pieces to path
// conditions (`PieceStates`). A piece under a condition means: for every
// valuation of the condition, every position pair of the piece is realized by
// a real path from the anchor that has not revisited it and whose guards that
// valuation admits. This holds initially by `identity`, is preserved by
// `then_dependency` with the conjoined edge guard, by the union of pieces
// (composition distributes over unions) and by the exact disjunction of the
// conditions of one piece. Reverse reachability removes no path that can
// return to the anchor. A piece contained in another piece whose condition
// covers its own adds no continuation, so dropping it is lossless; only the
// pieces whose condition grew are expanded again. The identity-edge search is
// the same invariant specialized to zero translations.
//
// Once an anchor occurrence is fixed, cutting a closed walk at every later
// occurrence of that anchor uniquely decomposes it into first-return walks.
// The search records every such relation, or a real relation/condition state
// that covers it as above. Conversely, compatible recorded relations whose
// composition intersects identity concatenate to a real closed walk. The
// guarded-composition argument in `guarded` proves that it finds exactly such
// sequences. Trying every node as anchor therefore proves that this function
// decides whether the SCC contains a branch-compatible closed walk that
// returns to the same array and packed position, provided the budget is not
// exhausted. The public decision reports exhaustion separately as incomplete.
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
// and those finite exact displacements; merging contiguous anchor-independent
// ranges only unions such endpoints. Thus only finitely many pieces are
// reachable. Branches and arms are finite as well, so the condition of each
// piece grows only finitely often and every piece is expanded finitely many
// times. Every endpoint and intermediate
// offset operation is a construction invariant required to be representable
// in `isize`; overflow is not interpreted as a dependency relation.
fn has_compatible_cycle_with_budget(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    budget: &mut SearchBudget,
    mut cuts: Option<&mut Cuts>,
) -> bool {
    if scc.is_empty()
        || (scc.len() == 1 && !graph.edges(scc[0]).any(|edge| edge.target() == scc[0]))
    {
        return false;
    }
    let mut nodes: HashSet<_> = scc.iter().copied().collect();
    if has_zero_dependency_cycle(graph, scc, budget) {
        return true;
    }
    // Prefer finite self-edge anchors: their translations go straight to the
    // closed-walk solver instead of being enumerated inside a first-return
    // path. Among them, broad domains keep wide shifts out of the internal
    // search state. Correctness does not depend on the anchor order.
    let mut starts = scc.to_vec();
    starts.sort_by_cached_key(|&node| {
        std::cmp::Reverse((
            !graph[node].domains.is_empty(),
            graph.edges(node).any(|edge| edge.target() == node),
            domain_area(&graph[node]),
        ))
    });
    for start in starts {
        if !budget.spend(scc.len()) {
            return false;
        }
        let returnable = nodes_that_may_reach_start(graph, &nodes, start);
        if !budget.spend_pieces(graph[start].domains.len().max(1), 1) {
            return false;
        }
        let initial = PositionRelationSet::identity(&graph[start].domains, budget);
        let mut cycles = HashSet::default();
        let mut reached: HashMap<NodeIndex, PieceStates> = HashMap::default();
        let mut queue = VecDeque::from([start]);
        let mut queued = HashSet::from_iter([start]);
        let mut first = true;
        while let Some(node) = queue.pop_front() {
            queued.remove(&node);
            if !budget.spend(1) {
                return false;
            }
            let states = if std::mem::take(&mut first) {
                vec![(PathCondition::default(), initial.clone())]
            } else {
                reached
                    .get_mut(&node)
                    .expect("queued nodes have states")
                    .take_delta(budget)
            };
            for (condition, relation) in states {
                // Parallel edges into one node under one guard continue as a
                // single union state. Composition distributes over unions.
                let mut successors: HashMap<(NodeIndex, PathCondition), Vec<PositionRelationSet>> =
                    HashMap::default();
                let mut order = Vec::new();
                for edge in graph.edges(node) {
                    if !budget.spend(1) {
                        return false;
                    }
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
                    if !budget
                        .spend_pieces(relation.piece_count(), graph[next].domains.len().max(1))
                    {
                        return false;
                    }
                    let next_relation =
                        relation.then_dependency(edge.weight().kind, &graph[next].domains, budget);
                    if next_relation.is_empty() {
                        continue;
                    }
                    if let Some(cuts) = cuts.as_deref_mut() {
                        record_cuts(cuts, start, next, &next_relation);
                    }
                    let successor = successors
                        .entry((next, next_condition.clone()))
                        .or_default();
                    if successor.is_empty() {
                        order.push((next, next_condition));
                    }
                    successor.push(next_relation);
                }
                for successor in order {
                    let relations = successors.remove(&successor).expect("ordered successor");
                    let (next, next_condition) = successor;
                    let next_relation = if relations.len() == 1 {
                        relations.into_iter().next().expect("one relation")
                    } else {
                        let pieces = relations.iter().map(PositionRelationSet::piece_count).sum();
                        if !budget.spend(pieces) {
                            return false;
                        }
                        PositionRelationSet::union_all(relations, budget)
                    };
                    if next == start {
                        if next_relation.intersects_identity() {
                            return true;
                        }
                        let mut inserted = false;
                        for relation in next_relation.split_translations() {
                            inserted |= cycles.insert(GuardedCycle {
                                relation,
                                condition: next_condition.clone(),
                            });
                        }
                        // Check geometrically growing prefixes without waiting
                        // for every first-return path through the other loops.
                        if inserted
                            && !queue.is_empty()
                            && cycles.len() >= 2
                            && cycles.len().is_power_of_two()
                            && try_cycle_witness(&cycles, budget)
                        {
                            return true;
                        }
                        continue;
                    }
                    let Some(changed) = reached.entry(next).or_default().insert(
                        &next_relation,
                        &next_condition,
                        budget,
                    ) else {
                        return false;
                    };
                    if changed && queued.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
        if guarded_cycle_displacements_cancel(&cycles, budget) {
            return true;
        }
        if budget.exhausted {
            return false;
        }
        // All feasible closed walks through this anchor have been rejected.
        // Any remaining cycle avoids it, so later anchors need not enumerate
        // those same walks again (including loops inside expression arms).
        nodes.remove(&start);
    }
    false
}

/// Boundaries per node, on each axis, at which a report divides it.
pub(super) type Cuts = HashMap<NodeIndex, [Vec<isize>; 2]>;

/// The boundaries that the decision for a cyclic component observes: the
/// ranges of the start positions and of the positions each walk reaches.
/// Dividing nodes there separates the positions the search distinguished,
/// so a report can name the elements and bits of a cycle. The search takes
/// the module's steps, so the boundaries follow its work, not the number of
/// positions.
pub(super) fn cycle_cuts(graph: &DependencyGraph, scc: &[NodeIndex], steps: &Steps) -> Cuts {
    let mut cuts = Cuts::default();
    SearchBudget::with_steps(steps, |budget| {
        has_compatible_cycle_with_budget(graph, scc, budget, Some(&mut cuts))
    });
    cuts
}

fn record_cuts(cuts: &mut Cuts, start: NodeIndex, node: NodeIndex, relation: &PositionRelationSet) {
    for [anchor, current] in relation.piece_bounds() {
        for (node, bounds) in [(start, anchor), (node, current)] {
            let entry = cuts.entry(node).or_default();
            for axis in 0..2 {
                if let Some((low, high)) = bounds[axis] {
                    entry[axis].extend([low, high]);
                }
            }
        }
    }
}

fn node_box(node: &GraphNode) -> Option<[AxisBounds; 2]> {
    match node.domains.as_slice() {
        [domain] => domain_bounds(domain),
        _ => None,
    }
}

/// The nodes that replace one node of the graph being split.
enum Pieces {
    /// Outside the split components.
    Dropped,
    Whole(NodeIndex, Option<[AxisBounds; 2]>),
    /// A grid at `cuts`, one node per cell in array-major order.
    Grid {
        cuts: [Vec<isize>; 2],
        cells: Vec<(NodeIndex, [AxisBounds; 2])>,
    },
}

impl Pieces {
    fn nodes(&self) -> Vec<NodeIndex> {
        match self {
            Self::Dropped => Vec::new(),
            Self::Whole(node, _) => vec![*node],
            Self::Grid { cells, .. } => cells.iter().map(|(node, _)| *node).collect(),
        }
    }

    /// The pieces that may contain a position of `reached`, or all of them
    /// when it is unbounded.
    fn candidates(
        &self,
        reached: Option<[AxisBounds; 2]>,
    ) -> Vec<(NodeIndex, Option<[AxisBounds; 2]>)> {
        match self {
            Self::Dropped => Vec::new(),
            Self::Whole(node, bounds) => vec![(*node, *bounds)],
            Self::Grid { cuts, cells } => {
                let Some(reached) = reached else {
                    return cells
                        .iter()
                        .map(|&(node, cell)| (node, Some(cell)))
                        .collect();
                };
                // Cell `k` spans `cuts[k]..cuts[k + 1]`.
                let range = |axis: usize| {
                    let cuts = &cuts[axis];
                    let (start, end) = reached[axis];
                    let first = cuts.partition_point(|&cut| cut <= start).saturating_sub(1);
                    let last = cuts.partition_point(|&cut| cut < end).min(cuts.len() - 1);
                    first..last
                };
                let width = cuts[1].len() - 1;
                let mut result = Vec::new();
                for array in range(0) {
                    for packed in range(1) {
                        let (node, cell) = cells[array * width + packed];
                        result.push((node, Some(cell)));
                    }
                }
                result
            }
        }
    }
}

/// Divide the nodes of `nodes` at `cuts`, keeping only those nodes and the
/// edges among them. A node is replaced by disjoint pieces of its domain, and
/// an edge is kept between pieces whose positions it can relate, so the
/// represented position pairs and walks are unchanged. `None` when no node
/// is divided, or when the module's steps do not pay for the division.
pub(super) fn split_at_cuts(
    graph: &DependencyGraph,
    nodes: &HashSet<NodeIndex>,
    cuts: &Cuts,
    steps: &Steps,
) -> Option<DependencyGraph> {
    let mut grids: Vec<Option<[Vec<isize>; 2]>> = Vec::with_capacity(graph.node_count());
    let mut changed = false;
    for node in graph.node_indices() {
        let bounds = node_box(&graph[node]).filter(|_| nodes.contains(&node));
        let (Some(bounds), Some(found)) = (bounds, cuts.get(&node)) else {
            grids.push(None);
            continue;
        };
        let grid = [0, 1].map(|axis| {
            let (low, high) = bounds[axis];
            let mut axis_cuts = found[axis]
                .iter()
                .copied()
                .filter(|&cut| low < cut && cut < high)
                .chain([low, high])
                .collect::<Vec<_>>();
            axis_cuts.sort_unstable();
            axis_cuts.dedup();
            axis_cuts
        });
        if grid[0].len() == 2 && grid[1].len() == 2 {
            grids.push(None);
            continue;
        }
        changed = true;
        grids.push(Some(grid));
    }
    if !changed {
        return None;
    }

    let mut split = DependencyGraph::new();
    let mut mapped: Vec<Pieces> = Vec::with_capacity(graph.node_count());
    for node in graph.node_indices() {
        let original = &graph[node];
        if !nodes.contains(&node) {
            mapped.push(Pieces::Dropped);
            continue;
        }
        let Some(grid) = grids[node.index()].take() else {
            mapped.push(Pieces::Whole(
                split.add_node(original.clone()),
                node_box(original),
            ));
            continue;
        };
        let mut cells = Vec::new();
        for array in grid[0].windows(2) {
            for packed in grid[1].windows(2) {
                let cell = [(array[0], array[1]), (packed[0], packed[1])];
                let domain = bounds_domain(cell)
                    .expect("cells between distinct cuts of a domain are non-empty domains");
                let region = if original.diagnostic.is_some() {
                    SummaryRegion {
                        array: super::region::ArraySpan {
                            start: domain.array_start,
                            length: domain.array_length,
                        },
                        packed: super::region::PackedSpan::new(
                            domain.packed_start,
                            domain.packed_length,
                        )
                        .unwrap_or(original.region.packed),
                        ..original.region
                    }
                } else {
                    original.region
                };
                if !steps.take(1) {
                    return None;
                }
                let piece = split.add_node(GraphNode {
                    region,
                    domains: vec![domain],
                    diagnostic: original.diagnostic,
                });
                cells.push((piece, cell));
            }
        }
        mapped.push(Pieces::Grid { cuts: grid, cells });
    }
    for edge in graph.edge_references() {
        let relation = edge.weight().kind;
        for (source, source_box) in mapped[edge.source().index()].candidates(None) {
            let reached = match source_box.map(|source_box| image(relation, source_box)) {
                Some(Some(None)) => continue,
                Some(Some(Some(reached))) => Some(reached),
                Some(None) | None => None,
            };
            for (destination, destination_box) in mapped[edge.target().index()].candidates(reached)
            {
                if !steps.take(1) {
                    return None;
                }
                let reaches = match (source_box, destination_box) {
                    (Some(source_box), Some(destination_box)) => {
                        relation.may_reach(0, source_box, destination_box[0])
                            && relation.may_reach(1, source_box, destination_box[1])
                    }
                    _ => true,
                };
                if !reaches {
                    continue;
                }
                split.active_summary = None;
                add_dependency_edge(&mut split, source, destination, edge.weight().clone());
                let new_edge = split.edges[&(source, destination, relation)];
                split.origins.insert(new_edge, edge.id());
                if let Some(causes) = graph.summary_causes.get(&edge.id()) {
                    split
                        .summary_causes
                        .entry(new_edge)
                        .or_default()
                        .extend(causes.iter().cloned());
                }
            }
        }
    }
    for (node, site) in &graph.sites {
        let inputs = site
            .data_inputs
            .iter()
            .flat_map(|input| mapped[input.index()].nodes())
            .collect::<Vec<_>>();
        for piece in mapped[node.index()].nodes() {
            split.sites.insert(
                piece,
                DefinitionSite {
                    token: site.token,
                    data_inputs: inputs.clone(),
                },
            );
        }
    }
    Some(split)
}

/// Per axis, bounds of the positions at the first node of `path` that the
/// path can lead back to themselves (`None` when unbounded), or `None` when
/// it leads back to none.
pub(super) fn cycle_anchor_bounds(
    graph: &DependencyGraph,
    path: &[EdgeIndex],
    steps: &Steps,
) -> Option<[Option<AxisBounds>; 2]> {
    let (start, _) = graph.edge_endpoints(*path.first()?)?;
    SearchBudget::with_steps(steps, |budget| {
        let domains = &graph[start].domains;
        if !budget.spend_pieces(domains.len().max(1), 1) {
            return None;
        }
        let mut relation = PositionRelationSet::identity(domains, budget);
        for &edge in path {
            let (_, next) = graph.edge_endpoints(edge)?;
            if !budget.spend_pieces(relation.piece_count(), graph[next].domains.len().max(1)) {
                return None;
            }
            relation = relation.then_dependency(graph[edge].kind, &graph[next].domains, budget);
            if relation.is_empty() {
                return None;
            }
        }
        relation.identity_anchor_bounds()
    })
}

/// Recover a feasible first-return path for source diagnostics. Parent indices
/// keep long paths linear in storage; positions and guards match the decision walk.
pub(super) fn diagnostic_cycle(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    steps: &Steps,
) -> Option<Vec<EdgeIndex>> {
    SearchBudget::with_steps(steps, |budget| {
        diagnostic_cycle_with_budget(graph, scc, budget)
    })
}

/// A component of a divided graph and a cycle through it.
pub(super) type ReportedCycle = (Vec<NodeIndex>, Vec<EdgeIndex>);

/// The cycles to report for a cyclic component: the component divided where
/// its decision distinguished positions, and in each part of it the first
/// path found back to the same positions. All of it takes the module's steps;
/// empty when no path is found, and the component is then reported
/// undivided.
pub(super) fn report_cycles(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    steps: &Steps,
) -> (Option<DependencyGraph>, Vec<ReportedCycle>) {
    let nodes = scc.iter().copied().collect::<HashSet<_>>();
    let cuts = cycle_cuts(graph, scc, steps);
    let split = split_at_cuts(graph, &nodes, &cuts, steps);
    let found = split.as_ref().map_or_else(Vec::new, |split| {
        SearchBudget::with_steps(steps, |budget| {
            let mut found = Vec::new();
            // A cycle is reported once per set of variables it passes through,
            // so a part whose variables already have a path is not searched.
            let mut variables_found = HashSet::default();
            for part in kosaraju_scc(&**split) {
                let cyclic =
                    part.len() > 1 || split.edges(part[0]).any(|edge| edge.target() == part[0]);
                if !cyclic {
                    continue;
                }
                let mut variables = part
                    .iter()
                    .filter_map(|node| split[*node].diagnostic.map(|key| key.0))
                    .collect::<Vec<_>>();
                variables.sort_unstable();
                variables.dedup();
                if variables_found.contains(&variables) {
                    continue;
                }
                if let Some(path) = diagnostic_cycle_with_budget(split, &part, budget) {
                    variables_found.insert(variables);
                    found.push((part, path));
                }
                if budget.exhausted {
                    break;
                }
            }
            found
        })
    });
    (split, found)
}

fn diagnostic_cycle_with_budget(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    budget: &mut SearchBudget,
) -> Option<Vec<EdgeIndex>> {
    let members = scc.iter().copied().collect::<HashSet<_>>();
    let mut anchors = scc
        .iter()
        .copied()
        .filter(|node| graph[*node].diagnostic.is_some())
        .collect::<Vec<_>>();
    anchors.sort_unstable_by_key(|node| graph[*node].diagnostic);
    for start in anchors {
        let relation = PositionRelationSet::identity(&graph[start].domains, budget);
        let mut states = vec![(
            start,
            PathCondition::default(),
            relation,
            None::<(usize, EdgeIndex)>,
        )];
        let mut queue = VecDeque::from([0]);
        let mut reached: HashMap<NodeIndex, PathPieces> = HashMap::default();
        while let Some(index) = queue.pop_front() {
            let (node, condition, relation, _) = states[index].clone();
            let mut edges = graph
                .edges(node)
                .filter(|edge| members.contains(&edge.target()))
                .map(|edge| edge.id())
                .collect::<Vec<_>>();
            edges.sort_unstable_by_key(|edge| {
                let (_, next) = graph.edge_endpoints(*edge).unwrap();
                (
                    graph[next].diagnostic,
                    graph[next].region,
                    graph[*edge].kind,
                    edge.index(),
                )
            });
            for edge in edges {
                if !budget.spend(1) {
                    return None;
                }
                let (_, next) = graph.edge_endpoints(edge).unwrap();
                let Some(condition) = condition.conjoin_if_compatible(&graph[edge].condition)
                else {
                    continue;
                };
                if !budget.spend_pieces(relation.piece_count(), graph[next].domains.len().max(1)) {
                    return None;
                }
                let relation =
                    relation.then_dependency(graph[edge].kind, &graph[next].domains, budget);
                if relation.is_empty() {
                    continue;
                }
                if next == start {
                    if relation.intersects_identity() {
                        let mut path = vec![edge];
                        let mut cursor = index;
                        while let Some((parent, edge)) = states[cursor].3 {
                            path.push(edge);
                            cursor = parent;
                        }
                        path.reverse();
                        return Some(path);
                    }
                    continue;
                }
                // Continue each piece separately, so that pieces reached
                // along other paths are recognized by lookup instead of by
                // comparing whole relations.
                for piece in relation.into_pieces() {
                    let recorded = reached
                        .entry(next)
                        .or_default()
                        .insert(&piece, &condition, budget)?;
                    if recorded {
                        queue.push_back(states.len());
                        states.push((next, condition.clone(), piece, Some((index, edge))));
                    }
                }
            }
        }
    }
    None
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
    budget: &mut SearchBudget,
) -> bool {
    // Identity paths can cycle only within an SCC of the identity-only
    // subgraph. Starting at every node of an acyclic identity chain would
    // spend quadratic work before the positional search reaches its shifts.
    if !budget.spend(scc.len()) {
        return false;
    }
    let mut identity = Graph::new();
    let mapped = scc
        .iter()
        .map(|&node| (node, identity.add_node(node)))
        .collect::<HashMap<_, _>>();
    for &node in scc {
        for edge in graph.edges(node) {
            if !budget.spend(1) {
                return false;
            }
            if dependency_is_identity(edge.weight().kind)
                && let Some(&destination) = mapped.get(&edge.target())
            {
                identity.add_edge(mapped[&node], destination, ());
            }
        }
    }
    if !budget.spend(identity.node_count().saturating_add(identity.edge_count())) {
        return false;
    }
    for component in kosaraju_scc(&identity) {
        if component.len() == 1
            && !identity
                .edges(component[0])
                .any(|edge| edge.target() == component[0])
        {
            continue;
        }
        let component = component
            .into_iter()
            .map(|node| identity[node])
            .collect::<Vec<_>>();
        let nodes = component.iter().copied().collect();
        if has_zero_dependency_cycle_in_component(graph, &component, &nodes, budget) {
            return true;
        }
        if budget.exhausted {
            return false;
        }
    }
    false
}

fn has_zero_dependency_cycle_in_component(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    nodes: &HashSet<NodeIndex>,
    budget: &mut SearchBudget,
) -> bool {
    for &start in scc {
        let initial = initial_feasible_positions(&graph[start]);
        let mut queue = VecDeque::from([(start, PathCondition::default(), initial)]);
        let mut reached: HashMap<NodeIndex, Vec<(Vec<FeasiblePosition>, PathCondition)>> =
            HashMap::default();
        while let Some((node, condition, feasible)) = queue.pop_front() {
            if !budget.spend(reached.get(&node).map_or(1, |states| states.len() + 1)) {
                return false;
            }
            if node != start
                && !reached[&node]
                    .iter()
                    .any(|(r, c)| *r == feasible && *c == condition)
            {
                continue;
            }
            for edge in graph.edges(node) {
                if !budget.spend(1) {
                    return false;
                }
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
                if !budget.spend_product(feasible.len(), graph[next].domains.len().max(1)) {
                    return false;
                }
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
                if let Some(condition) = insert_cycle_state(
                    reached.entry(next).or_default(),
                    &feasible,
                    next_condition,
                    PartialEq::eq,
                    Vec::len,
                    budget,
                ) {
                    queue.push_back((next, condition, feasible));
                }
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

        assert!(compatible_cycle_displacements_cancel(
            &cycles,
            &mut SearchBudget::new()
        ));
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

        assert!(!compatible_cycle_displacements_cancel(
            &cycles,
            &mut SearchBudget::new()
        ));
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

        assert!(compatible_cycle_displacements_cancel(
            &cycles,
            &mut SearchBudget::new()
        ));
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

    #[test]
    fn scc_walk_does_not_use_the_native_stack() {
        const COUNT: usize = 100_000;
        let mut graph = DependencyGraph::new();
        let mut previous = None;
        for start in 0..COUNT {
            let current = graph.add_node(test_node(0, ArraySpan { start, length: 1 }));
            if let Some(previous) = previous {
                add_dependency_edge(
                    &mut graph,
                    previous,
                    current,
                    GraphDependency::unconditional(BitDependency::WHOLE),
                );
            }
            previous = Some(current);
        }
        assert_eq!(strongly_connected_components(&graph).len(), COUNT);
    }
    #[test]
    fn first_return_composition_obeys_search_limit() {
        let mut graph = DependencyGraph::new();
        let node = graph.add_node(test_node(
            0,
            ArraySpan {
                start: 0,
                length: 128,
            },
        ));
        for shift in 1..=32 {
            add_dependency_edge(
                &mut graph,
                node,
                node,
                GraphDependency::unconditional(BitDependency {
                    array: Some(0),
                    packed: Some(shift),
                }),
            );
        }
        let mut budget = SearchBudget {
            remaining: 100,
            exhausted: false,
        };
        assert!(!has_compatible_cycle_with_budget(
            &graph,
            &[node],
            &mut budget,
            None,
        ));
        assert!(budget.exhausted);
        assert_eq!(
            compatible_cycle(&graph, &[node], &Steps::new()),
            Some(false)
        );
        for shift in 1..=32 {
            add_dependency_edge(
                &mut graph,
                node,
                node,
                GraphDependency::unconditional(BitDependency {
                    array: Some(0),
                    packed: Some(-shift),
                }),
            );
        }
        let mut budget = SearchBudget {
            remaining: 1_000,
            exhausted: false,
        };
        assert!(has_compatible_cycle_with_budget(
            &graph,
            &[node],
            &mut budget,
            None,
        ));
        assert!(!budget.exhausted);
        let mut budget = SearchBudget {
            remaining: 100,
            exhausted: false,
        };
        assert!(!has_compatible_cycle_with_budget(
            &graph,
            &[node],
            &mut budget,
            None,
        ));
        assert!(budget.exhausted);
        assert_eq!(compatible_cycle(&graph, &[node], &Steps::new()), Some(true));
    }

    #[test]
    fn cancellation_prefilters_stop_without_claiming_acyclicity() {
        let mut graph = DependencyGraph::new();
        let node = graph.add_node(test_node(
            0,
            ArraySpan {
                start: 0,
                length: 128,
            },
        ));
        // Every edge increases array + packed by one, but neither individual
        // axis has a common sign. Proving absence requires the transition and
        // cancellation prefilters, including their pair/triple comparisons.
        for offset in -12..=12 {
            add_dependency_edge(
                &mut graph,
                node,
                node,
                GraphDependency::unconditional(BitDependency {
                    array: Some(offset),
                    packed: Some(1 - offset),
                }),
            );
        }
        for limit in [1_000, 3_000] {
            let mut budget = SearchBudget {
                remaining: limit,
                exhausted: false,
            };
            assert!(!has_compatible_cycle_with_budget(
                &graph,
                &[node],
                &mut budget,
                None,
            ));
            assert!(budget.exhausted);
        }
        assert_eq!(
            compatible_cycle(&graph, &[node], &Steps::new()),
            Some(false)
        );
    }
}
