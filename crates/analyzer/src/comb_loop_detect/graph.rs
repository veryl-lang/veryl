//! Dependency graph storage, edge normalization, and cycle detection.

mod guarded;
mod relation;

use super::diagnostics::SummaryEdgeCause;
use super::model::{BitDependency, SummaryRegion};
use super::position::{Axis, Link, Overflow};
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
use std::rc::Rc;

#[derive(Clone, Debug)]
pub(super) struct GraphDependency {
    pub(super) kind: BitDependency,
    pub(super) condition: PathCondition,
}

/// The arm a branch takes on one instance of the loops around it: the
/// instance of the position an edge reaches, by `instance` from it. Arms of
/// one branch on one instance exclude one another; on different instances
/// they do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct InstanceArm {
    pub(super) branch: InstanceBranch,
    pub(super) arm: usize,
    pub(super) arms: usize,
    pub(super) instance: crate::comb_loop_detect::position::Map,
}

impl InstanceArm {
    /// The arm as a choice of the branch on one instance, which is a
    /// branch of its own: the arms of different instances are independent.
    fn choice(&self, instance: isize) -> Option<PathCondition> {
        // Procedures number their branches from small namespaces, so the
        // top of the range is free for the branches of instances.
        let instance = u32::try_from(instance).ok()?;
        let index = u32::try_from(self.branch.index).ok()?;
        let procedure = usize::MAX.checked_sub(self.branch.namespace)?;
        let local = (usize::try_from(index).ok()? << 32) | usize::try_from(instance).ok()?;
        Some(PathCondition::default().with_choice(
            crate::comb_loop_detect::ssa::BranchId::new(procedure, local, self.arms),
            self.arm,
        ))
    }
}

/// A branch of a procedure in a scope of tables: the procedure's namespace
/// and the branch's index among those of its scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct InstanceBranch {
    pub(super) namespace: usize,
    pub(super) index: usize,
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
    /// Nodes that each iteration of a loop reads and writes, such as the
    /// tables of instances: every recurrence of the loop passes through one.
    pub(super) recurrences: HashSet<NodeIndex>,
    /// The arms each edge takes on the instance of the position it
    /// reaches, besides its condition.
    pub(super) instance_arms: HashMap<EdgeIndex, Rc<[InstanceArm]>>,
}

impl DependencyGraph {
    pub(super) fn new() -> Self {
        Self {
            graph: Graph::new(),
            edges: HashMap::default(),
            sites: HashMap::default(),
            summary_causes: HashMap::default(),
            active_summary: None,
            recurrences: HashSet::default(),
            instance_arms: HashMap::default(),
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
    add_dependency_edge_with_arms(graph, source, destination, dependency, &[]);
}

/// `add_dependency_edge` for an edge taken only on `arms`. An edge merged
/// with one taken otherwise is taken on either.
pub(super) fn add_dependency_edge_with_arms(
    graph: &mut DependencyGraph,
    source: NodeIndex,
    destination: NodeIndex,
    dependency: GraphDependency,
    arms: &[InstanceArm],
) {
    let key = (source, destination, dependency.kind);
    let edge = if let Some(&existing) = graph.edges.get(&key) {
        let weight = graph
            .edge_weight_mut(existing)
            .expect("an edge found in the graph must remain present");
        weight.condition = weight.condition.disjoin(&dependency.condition);
        if graph.instance_arms.get(&existing).map(|kept| &**kept) != Some(arms) {
            graph.instance_arms.remove(&existing);
        }
        existing
    } else {
        let edge = graph.add_edge(source, destination, dependency);
        graph.edges.insert(key, edge);
        if !arms.is_empty() {
            graph.instance_arms.insert(edge, arms.into());
        }
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

pub(super) fn node_regions_overlap_with_dependency(
    source: &GraphNode,
    destination: &GraphNode,
    dependency: BitDependency,
) -> bool {
    let range = |start: usize, length: usize| {
        let start = isize::try_from(start).ok()?;
        Some((start, start.checked_add_unsigned(length)?))
    };
    let (
        Some(source_array),
        Some(source_packed),
        Some(destination_array),
        Some(destination_packed),
    ) = (
        range(source.region.array.start, source.region.array.length),
        range(source.region.packed.start, source.region.packed.length),
        range(
            destination.region.array.start,
            destination.region.array.length,
        ),
        range(
            destination.region.packed.start,
            destination.region.packed.length,
        ),
    )
    else {
        return false;
    };
    let source = [source_array, source_packed];
    dependency.may_reach(Axis::Array, source, destination_array)
        && dependency.may_reach(Axis::Packed, source, destination_packed)
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

// Count both transitions and dominance comparisons: a bound on queued states
// alone still permits quadratic work in the per-node antichains. Exhaustion
// means incomplete analysis, never an invented cycle or a proof of absence.
const CYCLE_SEARCH_WORK: usize = 1_000_000;

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

struct SearchBudget {
    remaining: usize,
    exhausted: bool,
}

impl SearchBudget {
    fn new() -> Self {
        Self {
            remaining: CYCLE_SEARCH_WORK,
            exhausted: false,
        }
    }

    // Composition distributes over both unions, then normalization compares
    // the resulting pieces. Charge before allocating that Cartesian product.
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

    // Comparing or joining two guards looks each constraint of one up in
    // the other, whose constraints are sorted by branch.
    fn spend_guard_comparison(&mut self, left: &PathCondition, right: &PathCondition) -> bool {
        let (left, right) = (left.work_size(), right.work_size());
        let lookup = usize::BITS - left.max(right).leading_zeros();
        self.spend(
            left.saturating_add(right)
                .saturating_mul(usize::try_from(lookup).unwrap_or(usize::MAX).max(1))
                .max(1),
        )
    }

    /// The result of a relation operation, or `None` when its arithmetic
    /// overflowed. The search then stops as incomplete, as when it runs out
    /// of work.
    fn checked<T>(&mut self, result: Result<T, Overflow>) -> Option<T> {
        let value = result.ok();
        self.exhausted |= value.is_none();
        value
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
    insert_bounded_cycle_state(states, relation, condition, covers, size, None, budget)
}

type MayCover<R> = dyn Fn(&R, &R) -> bool;

/// `insert_cycle_state` where `may_cover(outer, inner)`, a check charged as
/// one unit, is false only when `outer` cannot cover `inner`.
fn insert_bounded_cycle_state<R: Clone + Eq>(
    states: &mut Vec<(R, PathCondition)>,
    relation: &R,
    condition: PathCondition,
    covers: impl Fn(&R, &R) -> bool,
    size: impl Fn(&R) -> usize,
    may_cover: Option<&MayCover<R>>,
    budget: &mut SearchBudget,
) -> Option<PathCondition> {
    let mut condition = condition;
    // The prefilter is charged one unit per state when given.
    let filter_work = usize::from(may_cover.is_some());
    let may_cover =
        |outer: &R, inner: &R| may_cover.is_none_or(|may_cover| may_cover(outer, inner));
    let relation_size = size(relation);
    let comparison_work = |r: &R| size(r).saturating_mul(relation_size).saturating_add(1);
    // Different translations usually cannot dominate or merge. Charge guard
    // comparisons only after the positional check admits them, and stop
    // charging a scan as soon as its result is known.
    for (r, c) in states.iter() {
        if !budget.spend(filter_work) {
            return None;
        }
        if !may_cover(r, relation) {
            continue;
        }
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
            if !budget.spend(filter_work) {
                return None;
            }
            if !may_cover(r, relation) || !may_cover(relation, r) {
                continue;
            }
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
        if !budget.spend(filter_work) {
            return None;
        }
        if !may_cover(relation, r) {
            index += 1;
            continue;
        }
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

enum Insertion {
    Inserted(PathCondition),
    Covered,
    Exhausted,
}

/// The armed states of one node, grouped by the hulls of their anchor and
/// current positions: a state covers another only where its hulls contain
/// the other's, so only those groups are compared. A group whose hulls are
/// single positions is covered only by its own and by wider ones. Each
/// state keeps the serial it was queued with.
#[derive(Default)]
struct ArmedStates {
    groups: Vec<(Hulls, Vec<ArmedState>)>,
    index: HashMap<Hulls, usize>,
    /// The groups whose hulls are not single positions.
    wide: Vec<usize>,
}

type Hulls = [Option<(isize, isize)>; 4];

/// A state, the path condition it holds on and its serial.
type ArmedState = (ArmedRelation, PathCondition, u64);

fn single_positions(hulls: &Hulls) -> bool {
    hulls
        .iter()
        .all(|range| range.is_some_and(|(start, end)| end.checked_sub(start) == Some(1)))
}

impl ArmedStates {
    /// The groups that may hold a state related to one with `hulls`: as an
    /// outer state when `outer`, else as an inner one.
    fn related(&self, hulls: &Hulls, outer: bool) -> Vec<usize> {
        let own = self.index.get(hulls).copied();
        if single_positions(hulls) {
            let mut groups = own.into_iter().collect::<Vec<_>>();
            if outer {
                groups.extend(
                    self.wide
                        .iter()
                        .copied()
                        .filter(|&group| Some(group) != own),
                );
            }
            return groups;
        }
        (0..self.groups.len()).collect()
    }

    /// `insert_cycle_state` over the groups that can cover or be covered.
    fn insert(
        &mut self,
        relation: &ArmedRelation,
        condition: PathCondition,
        serial: u64,
        live: &mut HashSet<u64>,
        budget: &mut SearchBudget,
    ) -> Insertion {
        let hulls = relation.relation.hulls();
        let mut condition = condition;
        let relation_size = relation.size();
        let comparison_work =
            |r: &ArmedRelation| r.size().saturating_mul(relation_size).saturating_add(1);
        for group in self.related(&hulls, true) {
            let (group_hulls, states) = &self.groups[group];
            if !budget.spend(1) {
                return Insertion::Exhausted;
            }
            if !PositionRelationSet::hulls_contain(group_hulls, &hulls) {
                continue;
            }
            for (r, c, _) in states {
                if !budget.spend(comparison_work(r)) {
                    return Insertion::Exhausted;
                }
                if r.covers(relation) {
                    if !budget.spend_guard_comparison(c, &condition) {
                        return Insertion::Exhausted;
                    }
                    if c.covers(&condition) {
                        return Insertion::Covered;
                    }
                }
            }
        }
        let own = match self.index.get(&hulls) {
            Some(&own) => own,
            None => {
                self.groups.push((hulls, Vec::new()));
                let own = self.groups.len() - 1;
                self.index.insert(hulls, own);
                if !single_positions(&hulls) {
                    self.wide.push(own);
                }
                own
            }
        };
        // Exact unions of guards on the same relation.
        loop {
            let mut merge = None;
            for (index, (r, c, _)) in self.groups[own].1.iter().enumerate() {
                if !budget.spend(comparison_work(r)) {
                    return Insertion::Exhausted;
                }
                if r == relation {
                    if !budget.spend_guard_comparison(c, &condition) {
                        return Insertion::Exhausted;
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
            let (_, _, removed) = self.groups[own].1.swap_remove(index);
            live.remove(&removed);
            condition = merged;
        }
        for group in self.related(&hulls, false) {
            if !budget.spend(1) {
                return Insertion::Exhausted;
            }
            let (group_hulls, states) = &mut self.groups[group];
            if !PositionRelationSet::hulls_contain(&hulls, group_hulls) {
                continue;
            }
            let mut index = 0;
            while index < states.len() {
                let (r, c, removed) = &states[index];
                if !budget.spend(comparison_work(r)) {
                    return Insertion::Exhausted;
                }
                if relation.covers(r) {
                    if !budget.spend_guard_comparison(&condition, c) {
                        return Insertion::Exhausted;
                    }
                    if condition.covers(c) {
                        live.remove(removed);
                        states.swap_remove(index);
                        continue;
                    }
                }
                index += 1;
            }
        }
        if !budget.spend(relation_size.saturating_add(1)) {
            return Insertion::Exhausted;
        }
        self.groups[own]
            .1
            .push((relation.clone(), condition.clone(), serial));
        live.insert(serial);
        Insertion::Inserted(condition)
    }
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

pub(super) fn compatible_cycle(graph: &DependencyGraph, scc: &[NodeIndex]) -> Option<bool> {
    // Arms taken on instances only exclude walks: a component with no walk
    // that ignores them has none, and only one with such a walk is searched
    // again with them.
    let armed = scc.iter().any(|&node| {
        graph
            .edges(node)
            .any(|edge| graph.instance_arms.contains_key(&edge.id()))
    });
    let mut found = None;
    // A walk the search without arms closes.
    let mut walk: Option<Vec<EdgeIndex>> = None;
    for arms in [false, true] {
        if arms && !armed {
            break;
        }
        // The walk the search without arms closed is followed first with
        // them: when it closes, it is a closed walk that takes its arms
        // consistently.
        if arms
            && let Some(path) = walk.as_ref()
            && let Some(&first) = path.first()
            && let Some((start, _)) = graph.edge_endpoints(first)
        {
            let edges = path.iter().copied().collect::<HashSet<_>>();
            let mut budget = SearchBudget::new();
            if has_compatible_cycle_through(graph, scc, &mut budget, true, Some((start, &edges))) {
                found = Some(true);
                break;
            }
        }
        let mut budget = SearchBudget::new();
        let closes = has_compatible_cycle_with_budget(graph, scc, &mut budget, arms);
        // Count the decision separately from optional diagnostic path
        // recovery. Measurement must not change the budget or the
        // production search path.
        #[cfg(test)]
        DECISION_WORK.set(
            DECISION_WORK
                .get()
                .saturating_add(CYCLE_SEARCH_WORK - budget.remaining),
        );
        found = (closes || !budget.exhausted).then_some(closes);
        if found != Some(true) {
            break;
        }
        // A closed walk that takes no arm closes whatever the arms are.
        if !arms && armed {
            walk = diagnostic_cycle(graph, scc);
            if walk.as_ref().is_some_and(|path| {
                path.iter()
                    .all(|edge| !graph.instance_arms.contains_key(edge))
            }) {
                break;
            }
        }
    }
    found
}

#[cfg(test)]
fn has_compatible_cycle(graph: &DependencyGraph, scc: &[NodeIndex]) -> bool {
    compatible_cycle(graph, scc).expect("small reference graphs must be decided completely")
}

// Correctness argument for the graph-relative cycle decision:
//
// Interpret a graph state as `(node, array_position, packed_position)`. An edge
// with `Some(k)` maps a coordinate to `coordinate + k`; `None` relates every
// source coordinate to every coordinate in the destination domain. Interpret a
// `PathCondition` as its stored Cartesian set of branch choices. Correlations
// discarded before graph construction are deliberately not reintroduced here.
//
// For a fixed anchor, every valuation of a queued state's condition admits a
// real path that has not revisited the anchor, with exactly the state's binary
// position relation. This holds initially by `identity`, is preserved by
// `then_dependency`, and by exact unions of guards on the same relation.
// Reverse reachability removes no path
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
// and those finite exact displacements. Thus only finitely many normalized
// relation states are reachable. Branches and arms are finite as well, and a
// dominated state is never queued again. Every endpoint and intermediate
// offset operation is a construction invariant required to be representable
// in `isize`; overflow is not interpreted as a dependency relation.
fn has_compatible_cycle_with_budget(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    budget: &mut SearchBudget,
    arms: bool,
) -> bool {
    has_compatible_cycle_through(graph, scc, budget, arms, None)
}

/// `has_compatible_cycle_with_budget` over the closed walks from one anchor
/// along `walk`'s edges alone when given.
fn has_compatible_cycle_through(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
    budget: &mut SearchBudget,
    arms: bool,
    walk: Option<(NodeIndex, &HashSet<EdgeIndex>)>,
) -> bool {
    let allowed = |edge: EdgeIndex| walk.is_none_or(|(_, edges)| edges.contains(&edge));
    if scc.is_empty()
        || (scc.len() == 1 && !graph.edges(scc[0]).any(|edge| edge.target() == scc[0]))
    {
        return false;
    }
    // Only an arm some other arm of its branch may meet on an instance can
    // exclude a walk; the others are not followed.
    let contested = if arms {
        match Contested::of(graph, scc, &allowed, budget) {
            Some(contested) => contested,
            None => return false,
        }
    } else {
        Contested::default()
    };
    let instance_arms = |edge: EdgeIndex| contested.arms.get(&edge);
    let mut nodes: HashSet<_> = scc.iter().copied().collect();
    if walk.is_none() && has_zero_dependency_cycle(graph, scc, budget, arms) {
        return true;
    }
    // Prefer finite self-edge anchors: their translations go straight to the
    // closed-walk solver instead of being enumerated inside a first-return
    // path. Among them, broad domains keep wide shifts out of the internal
    // search state. Correctness does not depend on the anchor order.
    let mut starts = match walk {
        Some((start, _)) => vec![start],
        None => scc.to_vec(),
    };
    // A recurrence node first: its iterations close as first returns
    // instead of being repeated inside the paths of other anchors.
    starts.sort_by_cached_key(|&node| {
        std::cmp::Reverse((
            graph.recurrences.contains(&node),
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
        if !budget.spend_product(graph[start].domains.len().max(1), 1) {
            return false;
        }
        let initial = ArmedRelation {
            relation: PositionRelationSet::identity(&graph[start].domains),
            arms: Vec::new(),
        };
        let mut cycles = HashSet::default();
        // Each queued state with its serial; one a later state replaced is
        // no longer live and is skipped.
        let mut queue = VecDeque::from([(start, PathCondition::default(), initial, 0u64)]);
        let mut reached: HashMap<NodeIndex, ArmedStates> = HashMap::default();
        let mut live = HashSet::default();
        let mut serials = 1u64;
        while let Some((node, condition, armed, serial)) = queue.pop_front() {
            if !budget.spend(1) {
                return false;
            }
            if node != start && !live.contains(&serial) {
                continue;
            }
            let relation = &armed.relation;
            for edge in graph.edges(node) {
                if !budget.spend(1) {
                    return false;
                }
                let next = edge.target();
                if !returnable.contains(&next) || !allowed(edge.id()) {
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
                if !budget.spend_product(relation.piece_count(), graph[next].domains.len().max(1)) {
                    return false;
                }
                let next_relation =
                    relation.then_dependency(edge.weight().kind, &graph[next].domains);
                let Some(next_relation) = budget.checked(next_relation) else {
                    return false;
                };
                if next_relation.is_empty() {
                    continue;
                }
                if !budget.spend(armed.size()) {
                    return false;
                }
                let edge_arms = instance_arms(edge.id()).map_or(&[][..], |arms| &arms[..]);
                let arms = armed.through(
                    edge.weight().kind,
                    &graph[next].domains,
                    edge_arms,
                    &next_relation,
                    false,
                );
                let Some(arms) = budget.checked(arms) else {
                    return false;
                };
                let Some(arms) = arms else {
                    continue;
                };
                // An arm taken on one known instance is a choice like any
                // other, which every walk through it shares.
                let mut next_condition = next_condition;
                if let Some(edge_arms) = instance_arms(edge.id())
                    && let Some(hull) = next_relation.array_hull()
                {
                    let mut excluded = false;
                    for arm in edge_arms.iter() {
                        let Some(choice) = single_instance(edge_arms, arm, hull)
                            .filter(|&instance| contested.contains(arm.branch, instance))
                            .and_then(|instance| arm.choice(instance))
                        else {
                            continue;
                        };
                        match next_condition.conjoin_if_compatible(&choice) {
                            Some(condition) => next_condition = condition,
                            None => {
                                excluded = true;
                                break;
                            }
                        }
                    }
                    if excluded {
                        continue;
                    }
                }
                // So is an arm taken earlier whose instance the positions the
                // path reaches now give.
                let mut arms = arms;
                if let Some(hull) = next_relation.array_hull() {
                    let mut excluded = false;
                    arms.retain(|arm| {
                        let instance = (!excluded)
                            .then(|| arm.current.single_anchor_reaching(hull))
                            .flatten();
                        // On an instance no other arm meets, it excludes none.
                        if instance
                            .is_some_and(|instance| !contested.contains(arm.branch, instance))
                        {
                            return false;
                        }
                        let Some(choice) = instance.and_then(|instance| {
                            InstanceArm {
                                branch: arm.branch,
                                arm: arm.arm,
                                arms: arm.arms,
                                instance: crate::comb_loop_detect::position::Map::translation(0),
                            }
                            .choice(instance)
                        }) else {
                            return true;
                        };
                        match next_condition.conjoin_if_compatible(&choice) {
                            Some(condition) => {
                                next_condition = condition;
                                false
                            }
                            None => {
                                excluded = true;
                                true
                            }
                        }
                    });
                    if excluded {
                        continue;
                    }
                }
                if next == start {
                    let Some(closes) = budget.checked(next_relation.intersects_identity()) else {
                        return false;
                    };
                    // Back at the anchor's own array position, the arms are
                    // taken on the instances of that position. A return
                    // there alone is excluded with them.
                    let back = armed.through(
                        edge.weight().kind,
                        &graph[next].domains,
                        edge_arms,
                        &next_relation,
                        true,
                    );
                    let Some(back) = budget.checked(back) else {
                        return false;
                    };
                    // Arms whose instances the anchor's position gives.
                    let conflicting = next_relation
                        .anchor_array_hull()
                        .zip(next_relation.array_hull())
                        .map(|(anchor, current)| (anchor.0.max(current.0), anchor.1.min(current.1)))
                        .filter(|hull| hull.0 < hull.1)
                        .is_some_and(|hull| {
                            let mut condition = next_condition.clone();
                            arms.iter().any(|arm| {
                                let Some(choice) = arm
                                    .current
                                    .single_anchor_reaching(hull)
                                    .and_then(|instance| {
                                        InstanceArm {
                                            branch: arm.branch,
                                            arm: arm.arm,
                                            arms: arm.arms,
                                            instance:
                                                crate::comb_loop_detect::position::Map::translation(
                                                    0,
                                                ),
                                        }
                                        .choice(instance)
                                    })
                                else {
                                    return false;
                                };
                                match condition.conjoin_if_compatible(&choice) {
                                    Some(next) => {
                                        condition = next;
                                        false
                                    }
                                    None => true,
                                }
                            })
                        });
                    let next_relation = if back.is_none() || conflicting {
                        let returns = next_relation.without_array_diagonal();
                        if returns.is_empty() {
                            continue;
                        }
                        returns
                    } else if closes {
                        return true;
                    } else {
                        next_relation
                    };
                    // A return kept apart from the others keeps only its
                    // condition: each arm it took on one of several
                    // instances becomes a choice on each of them in turn,
                    // as a later return may take that instance otherwise.
                    let Some(conditions) = arm_choices(&arms, next_condition, &contested, budget)
                    else {
                        return false;
                    };
                    let before = cycles.len();
                    for condition in conditions {
                        cycles.insert(GuardedCycle {
                            relation: next_relation.clone(),
                            condition,
                        });
                    }
                    // Whether the count reached a power of two it had not.
                    let after = cycles.len();
                    let inserted = after > before
                        && (1usize << (usize::BITS - 1 - after.leading_zeros())) > before;
                    // Check geometrically growing prefixes without waiting
                    // for every first-return path through the other loops.
                    if inserted
                        && !queue.is_empty()
                        && cycles.len() >= 2
                        && try_cycle_witness(&cycles, budget)
                    {
                        return true;
                    }
                    continue;
                }
                let next_armed = ArmedRelation {
                    relation: next_relation,
                    arms,
                };
                let states = reached.entry(next).or_default();
                match states.insert(&next_armed, next_condition, serials, &mut live, budget) {
                    Insertion::Inserted(condition) => {
                        queue.push_back((next, condition, next_armed, serials));
                        serials += 1;
                    }
                    Insertion::Covered => {}
                    Insertion::Exhausted => return false,
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

/// The arms of a component that another arm of their branch may meet on an
/// instance: the instances where the arms of its edges into tables differ,
/// and the edges with such arms.
#[derive(Default)]
struct Contested {
    instances: HashSet<(InstanceBranch, isize)>,
    /// Branches on instances no bounded positions give.
    unbounded: HashSet<InstanceBranch>,
    arms: HashMap<EdgeIndex, Rc<[InstanceArm]>>,
}

impl Contested {
    fn of(
        graph: &DependencyGraph,
        scc: &[NodeIndex],
        allowed: &dyn Fn(EdgeIndex) -> bool,
        budget: &mut SearchBudget,
    ) -> Option<Self> {
        let members = scc.iter().copied().collect::<HashSet<_>>();
        let mut taken: HashMap<(InstanceBranch, isize), usize> = HashMap::default();
        let mut contested = Self::default();
        let mut armed = Vec::new();
        for &node in scc {
            for edge in graph.edges(node) {
                if !members.contains(&edge.target()) || !allowed(edge.id()) {
                    continue;
                }
                let Some(arms) = graph.instance_arms.get(&edge.id()) else {
                    continue;
                };
                armed.push((edge.id(), arms.clone()));
                let domains = &graph[edge.target()].domains;
                for arm in arms.iter() {
                    if domains.is_empty() {
                        contested.unbounded.insert(arm.branch);
                    }
                    for domain in domains {
                        // Instances beyond `isize` are not known; the
                        // search stops as incomplete rather than leave the
                        // arm uncontested.
                        let parameters = isize::try_from(domain.array_start)
                            .ok()
                            .and_then(|start| {
                                let length = isize::try_from(domain.array_length).ok()?;
                                Some((start, start.checked_add(length)?))
                            })
                            .ok_or(Overflow)
                            .and_then(|(start, end)| arm.instance.source_parameters(start, end));
                        let parameters = budget.checked(parameters)?;
                        let Some((first, last)) = parameters else {
                            continue;
                        };
                        for t in first..=last {
                            if !budget.spend(1) {
                                return None;
                            }
                            let instance = arm
                                .instance
                                .step
                                .checked_mul(t)
                                .and_then(|offset| arm.instance.base.checked_add(offset))
                                .ok_or(Overflow);
                            let instance = budget.checked(instance)?;
                            match taken.entry((arm.branch, instance)) {
                                std::collections::hash_map::Entry::Vacant(entry) => {
                                    entry.insert(arm.arm);
                                }
                                std::collections::hash_map::Entry::Occupied(entry) => {
                                    if *entry.get() != arm.arm {
                                        contested.instances.insert((arm.branch, instance));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let branches = contested
            .instances
            .iter()
            .map(|(branch, _)| *branch)
            .chain(contested.unbounded.iter().copied())
            .collect::<HashSet<_>>();
        for (edge, arms) in armed {
            let kept = arms
                .iter()
                .filter(|arm| branches.contains(&arm.branch))
                .copied()
                .collect::<Vec<_>>();
            if !kept.is_empty() {
                contested.arms.insert(edge, kept.into());
            }
        }
        Some(contested)
    }

    fn contains(&self, branch: InstanceBranch, instance: isize) -> bool {
        self.unbounded.contains(&branch) || self.instances.contains(&(branch, instance))
    }
}

/// `condition` with each arm of `arms` taken on one of the instances it
/// may be on, for each such choice that is compatible. An arm on instances
/// no bounded range holds stays out of the condition. `None` when the work
/// is exhausted.
fn arm_choices(
    arms: &[TakenArm],
    condition: PathCondition,
    contested: &Contested,
    budget: &mut SearchBudget,
) -> Option<Vec<PathCondition>> {
    let mut conditions = vec![condition];
    for arm in arms {
        let Some((low, high)) = arm
            .current
            .anchor_array_hull()
            .or_else(|| arm.anchor.array_hull())
        else {
            continue;
        };
        let mut next = Vec::new();
        for condition in &conditions {
            for instance in low..high {
                if !budget.spend(1) {
                    return None;
                }
                if !contested.contains(arm.branch, instance) {
                    next.push(condition.clone());
                    continue;
                }
                let choice = InstanceArm {
                    branch: arm.branch,
                    arm: arm.arm,
                    arms: arm.arms,
                    instance: crate::comb_loop_detect::position::Map::translation(0),
                }
                .choice(instance);
                match choice {
                    Some(choice) => next.extend(condition.conjoin_if_compatible(&choice)),
                    // An instance no choice names keeps the condition.
                    None => next.push(condition.clone()),
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        conditions = next;
    }
    Some(conditions)
}

/// The one instance the arms of `arm`'s branch and arm among `arms` give
/// the positions of `hull`, `None` when they give several or none. Arms
/// of one write differ only by the positions they map.
fn single_instance(arms: &[InstanceArm], arm: &InstanceArm, hull: (isize, isize)) -> Option<isize> {
    let mut found = None;
    for other in arms
        .iter()
        .filter(|other| other.branch == arm.branch && other.arm == arm.arm)
    {
        let map = other.instance;
        let Ok(Some((first, last))) = map.source_parameters(hull.0, hull.1) else {
            continue;
        };
        for t in [first, last] {
            let instance = map.base.checked_add(map.step.checked_mul(t)?)?;
            if found.is_some_and(|found| found != instance) {
                return None;
            }
            found = Some(instance);
        }
    }
    found
}

/// A relation from the anchor with the arms its path took on instances.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ArmedRelation {
    relation: PositionRelationSet,
    arms: Vec<TakenArm>,
}

/// An arm a path took on an instance: the relation from the instance to the
/// current position, and from the anchor to the instance.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TakenArm {
    branch: InstanceBranch,
    arm: usize,
    arms: usize,
    current: PositionRelationSet,
    anchor: PositionRelationSet,
}

impl TakenArm {
    /// Whether this arm and `other` take one branch differently on the
    /// instance `current` relates to the current position or `anchor`
    /// relates to the anchor position, for every position the path takes.
    fn excludes(
        &self,
        other: &InstanceArm,
        current: &PositionRelationSet,
        anchor: &PositionRelationSet,
    ) -> bool {
        self.branch == other.branch
            && self.arm != other.arm
            && ((!self.current.is_empty() && self.current.array_within_function(current))
                || (!self.anchor.is_empty() && self.anchor.array_within_function(anchor)))
    }
}

impl ArmedRelation {
    /// Every path of `other` is one of `self`'s: a wider relation that took
    /// no other arm.
    fn covers(&self, other: &Self) -> bool {
        self.relation.piecewise_covers(&other.relation)
            && self.arms.iter().all(|arm| {
                other.arms.iter().any(|narrower| {
                    narrower.branch == arm.branch
                        && narrower.arm == arm.arm
                        && arm.current.piecewise_covers(&narrower.current)
                        && arm.anchor.piecewise_covers(&narrower.anchor)
                })
            })
    }

    fn size(&self) -> usize {
        self.arms
            .iter()
            .fold(self.relation.piece_count(), |size, arm| {
                size.saturating_add(arm.current.piece_count())
                    .saturating_add(arm.anchor.piece_count())
            })
    }

    /// The arms taken after an edge of `dependency` into `destination` that
    /// takes `arms` on the instances of the positions it reaches, where the
    /// path relates the anchor as `relation`, back at the anchor when
    /// `closing`. `None` when one of them excludes an arm already taken on
    /// the same instance.
    fn through(
        &self,
        dependency: BitDependency,
        destination: &[PositionDomain],
        arms: &[InstanceArm],
        relation: &PositionRelationSet,
        closing: bool,
    ) -> Result<Option<Vec<TakenArm>>, Overflow> {
        let mut taken = Vec::with_capacity(self.arms.len() + arms.len());
        for arm in &self.arms {
            let current = arm.current.then_dependency(dependency, destination)?;
            // An arm related to no instance by either relation excludes none.
            if !current.array_is_determined() && !arm.anchor.array_is_determined() {
                continue;
            }
            taken.push(TakenArm {
                current,
                ..arm.clone()
            });
        }
        for arm in arms {
            let instance = BitDependency {
                array: Link::Map(arm.instance),
                packed: Link::Unlinked,
            };
            // Each instance with the positions holding it.
            let current = match arm.instance.inverse() {
                Some(positions) => PositionRelationSet::identity(&[]).then_dependency(
                    BitDependency {
                        array: Link::Map(positions),
                        packed: Link::Unlinked,
                    },
                    destination,
                )?,
                None => PositionRelationSet::default(),
            };
            // The instance of each anchor position. A closing path is back
            // at the anchor position.
            let anchor = if closing {
                PositionRelationSet::identity(&[]).then_dependency(instance, &[])?
            } else {
                relation.then_dependency(instance, &[])?
            };
            if taken
                .iter()
                .any(|other| other.excludes(arm, &current, &anchor))
            {
                return Ok(None);
            }
            if !current.array_is_determined() && !anchor.array_is_determined() {
                continue;
            }
            let entry = TakenArm {
                branch: arm.branch,
                arm: arm.arm,
                arms: arm.arms,
                current,
                anchor,
            };
            if !taken.contains(&entry) {
                taken.push(entry);
            }
        }
        Ok(Some(taken))
    }
}

/// Recover a feasible first-return path for source diagnostics. Parent indices
/// keep long paths linear in storage; positions and guards match the decision
/// walk. Arms taken on instances are not followed: the path shown is one of
/// the component's, which the decision has already found to close.
pub(super) fn diagnostic_cycle(
    graph: &DependencyGraph,
    scc: &[NodeIndex],
) -> Option<Vec<EdgeIndex>> {
    let mut budget = SearchBudget::new();
    let members = scc.iter().copied().collect::<HashSet<_>>();
    let mut anchors = scc
        .iter()
        .copied()
        .filter(|node| graph[*node].diagnostic.is_some())
        .collect::<Vec<_>>();
    anchors.sort_unstable_by_key(|node| graph[*node].diagnostic);
    for start in anchors {
        let relation = PositionRelationSet::identity(&graph[start].domains);
        let mut states = vec![(
            start,
            PathCondition::default(),
            relation,
            None::<(usize, EdgeIndex)>,
        )];
        let mut queue = VecDeque::from([0]);
        let mut reached: HashMap<NodeIndex, Vec<(PositionRelationSet, PathCondition)>> =
            HashMap::default();
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
                if !budget.spend_product(relation.piece_count(), graph[next].domains.len().max(1)) {
                    return None;
                }
                let relation = relation.then_dependency(graph[edge].kind, &graph[next].domains);
                let relation = budget.checked(relation)?;
                if relation.is_empty() {
                    continue;
                }
                if next == start {
                    if budget.checked(relation.intersects_identity())? {
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
                let previous = reached.entry(next).or_default();
                if !budget.spend(previous.len().saturating_mul(2)) {
                    return None;
                }
                if previous
                    .iter()
                    .any(|(r, c)| r.piecewise_covers(&relation) && c.covers(&condition))
                {
                    continue;
                }
                previous.retain(|(r, c)| !relation.piecewise_covers(r) || !condition.covers(c));
                previous.push((relation.clone(), condition.clone()));
                queue.push_back(states.len());
                states.push((next, condition, relation, Some((index, edge))));
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
    arms: bool,
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
        if has_zero_dependency_cycle_in_component(graph, &component, &nodes, budget, arms) {
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
    arms: bool,
) -> bool {
    for &start in scc {
        let initial = (initial_feasible_positions(&graph[start]), Vec::new());
        let mut queue = VecDeque::from([(start, PathCondition::default(), initial)]);
        type Taken = (Vec<FeasiblePosition>, Vec<InstanceArm>);
        let mut reached: HashMap<NodeIndex, Vec<(Taken, PathCondition)>> = HashMap::default();
        while let Some((node, condition, (feasible, taken))) = queue.pop_front() {
            if !budget.spend(reached.get(&node).map_or(1, |states| states.len() + 1)) {
                return false;
            }
            if node != start
                && !reached[&node]
                    .iter()
                    .any(|((r, t), c)| *r == feasible && *t == taken && *c == condition)
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
                // Positions stay the anchor's, so arms of one branch exclude
                // one another where they take instances alike.
                let mut taken = taken.clone();
                if let Some(arms) = graph.instance_arms.get(&edge.id()).filter(|_| arms) {
                    let excluded = arms.iter().any(|arm| {
                        taken.iter().any(|other| {
                            other.branch == arm.branch
                                && other.arm != arm.arm
                                && other.instance == arm.instance
                        })
                    });
                    if excluded {
                        continue;
                    }
                    for arm in arms.iter() {
                        if !taken.contains(arm) {
                            taken.push(*arm);
                        }
                    }
                    // One set of arms, whatever order a path took them in.
                    taken.sort_unstable();
                }
                if next == start {
                    return true;
                }
                let state = (feasible, taken);
                if let Some(condition) = insert_cycle_state(
                    reached.entry(next).or_default(),
                    &state,
                    next_condition,
                    |(left, left_arms): &Taken, (right, right_arms): &Taken| {
                        left == right && left_arms.iter().all(|arm| right_arms.contains(arm))
                    },
                    |(feasible, taken): &Taken| feasible.len() + taken.len(),
                    budget,
                ) {
                    queue.push_back((next, condition, state));
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
        array: inverse_translated_axis(
            domain.array_start,
            domain.array_length,
            dependency.array.translation_offset(),
        )?,
        packed: inverse_translated_axis(
            domain.packed_start,
            domain.packed_length,
            dependency.packed.translation_offset(),
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
    dependency.array == Link::from_offset(Some(0))
        && dependency.packed == Link::from_offset(Some(0))
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
                array: Link::from_offset(Some(-4)),
                packed: Link::from_offset(Some(-4)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(0)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                    array: Link::from_offset(Some(0)),
                    packed: Link::from_offset(Some(3)),
                },
                condition: PathCondition::default(),
            },
        );
        graph.add_edge(
            b,
            a,
            GraphDependency {
                kind: BitDependency {
                    array: Link::from_offset(Some(0)),
                    packed: Link::from_offset(Some(-1)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                array: Link::from_offset(Some(0)),
                packed: Link::from_offset(Some(packed)),
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
                    array: Link::from_offset(Some(0)),
                    packed: Link::from_offset(Some(1)),
                },
                condition.clone(),
            ),
            (
                BitDependency {
                    array: Link::from_offset(Some(0)),
                    packed: Link::from_offset(Some(-7)),
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
                    array: Link::from_offset(Some(1)),
                    packed: Link::from_offset(Some(0)),
                },
                condition.clone(),
            ),
            (
                BitDependency {
                    array: Link::from_offset(Some(0)),
                    packed: Link::from_offset(Some(1)),
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
                    array: Link::from_offset(Some(1)),
                    packed: Link::from_offset(Some(0)),
                },
                condition.clone(),
            ),
            (
                BitDependency {
                    array: Link::from_offset(Some(0)),
                    packed: Link::from_offset(Some(1)),
                },
                condition.clone(),
            ),
            (
                BitDependency {
                    array: Link::from_offset(Some(-1)),
                    packed: Link::from_offset(Some(-1)),
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
                    array: Link::IDENTITY,
                    packed: Link::from_offset(packed),
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
                        let mapped = match dependency.packed.translation_offset() {
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
                    array: Link::from_offset(
                        (raw_array % 4 != 0).then_some(raw_array as isize % 5 - 2),
                    ),
                    packed: Link::from_offset(
                        (raw_packed % 4 != 0).then_some(raw_packed as isize % 5 - 2),
                    ),
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
                            let destination_arrays = mapped_positions(
                                source_array,
                                dependency.array.translation_offset(),
                                array_width,
                            );
                            let destination_packeds = mapped_positions(
                                source_packed,
                                dependency.packed.translation_offset(),
                                packed_width,
                            );
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

    /// Compare the decision with the expanded graphs of random graphs whose
    /// edges take links from `link`. Exact when `exact`, else only sound.
    fn compare_with_expanded_graphs(
        link: impl Fn(&mut dyn FnMut() -> u32) -> Link,
        exact: bool,
    ) -> (usize, usize) {
        use daggy::petgraph::algo::is_cyclic_directed;

        let mut state = 0x1357_9bdf_u32;
        let mut random = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state >> 8
        };
        let (mut matched, mut over) = (0, 0);
        for case in 0..6_000 {
            let node_count = 1 + random() as usize % 3;
            let array_width = 1 + random() as usize % 4;
            let packed_width = 1 + random() as usize % 4;
            let array = ArraySpan {
                start: 0,
                length: array_width,
            };
            let mut graph = DependencyGraph::new();
            let nodes = (0..node_count)
                .map(|id| {
                    let id = VarId::from_raw(id as u32);
                    graph.add_node(GraphNode {
                        region: SummaryRegion {
                            id,
                            array,
                            packed: PackedSpan::new(0, packed_width).unwrap(),
                        },
                        domains: vec![PositionDomain {
                            array_start: 0,
                            array_length: array_width,
                            packed_start: 0,
                            packed_length: packed_width,
                        }],
                        diagnostic: Some((id, array, 0)),
                    })
                })
                .collect::<Vec<_>>();
            let mut edges = Vec::new();
            for _ in 0..1 + random() as usize % (node_count * 3) {
                let source = random() as usize % node_count;
                let destination = random() as usize % node_count;
                let dependency = BitDependency {
                    array: link(&mut random),
                    packed: link(&mut random),
                };
                if dependency.is_empty() {
                    continue;
                }
                edges.push((source, destination, dependency));
                add_dependency_edge(
                    &mut graph,
                    nodes[source],
                    nodes[destination],
                    GraphDependency::unconditional(dependency),
                );
            }
            let symbolic = tarjan_scc(&graph.graph)
                .iter()
                .any(|scc| has_compatible_cycle(&graph, scc));
            let mut expanded = Graph::<(), ()>::new();
            let positions = (0..node_count)
                .map(|_| {
                    (0..array_width * packed_width)
                        .map(|_| expanded.add_node(()))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            for &(source, destination, dependency) in &edges {
                for from in 0..array_width * packed_width {
                    for to in 0..array_width * packed_width {
                        let point = |index: usize| {
                            (
                                (index / packed_width) as isize,
                                (index % packed_width) as isize,
                            )
                        };
                        if dependency.relates(point(from), point(to)) {
                            expanded.add_edge(
                                positions[source][from],
                                positions[destination][to],
                                (),
                            );
                        }
                    }
                }
            }
            let concrete = is_cyclic_directed(&expanded);
            assert!(
                symbolic || !concrete,
                "case {case}: missed a cycle in [{array_width}, {packed_width}] with {edges:?}"
            );
            assert!(
                !exact || symbolic == concrete,
                "case {case}: invented a cycle in [{array_width}, {packed_width}] with {edges:?}"
            );
            if symbolic == concrete {
                matched += 1;
            } else {
                over += 1;
            }
        }
        (matched, over)
    }

    #[test]
    fn affine_cycle_detection_never_misses_an_expanded_cycle() {
        use crate::comb_loop_detect::position::Map;
        let link = |random: &mut dyn FnMut() -> u32| match random() % 7 {
            0 => Link::Unlinked,
            1 => Link::strided(2 + random() as isize % 2, random() as isize % 3),
            2 | 3 => Link::translation(random() as isize % 5 - 2),
            _ => Map::scaled(
                random().is_multiple_of(3),
                [-2isize, -1, 1, 2, 3][random() as usize % 5],
                random() as isize % 7 - 3,
                1 + random() as isize % 3,
            ),
        };
        let (matched, over) = compare_with_expanded_graphs(link, false);
        eprintln!("affine cycle detection: {matched} exact, {over} conservative");
    }

    #[test]
    fn strided_cycle_detection_matches_expanded_graphs() {
        // Strided and unlinked coordinates with translations and strides
        // lose no precision.
        use crate::comb_loop_detect::position::Map;
        let link = |random: &mut dyn FnMut() -> u32| match random() % 5 {
            0 => Link::Unlinked,
            1 | 2 => Link::strided(2 + random() as isize % 2, random() as isize % 3),
            3 => Map::scaled(false, 2, random() as isize % 3, 1),
            _ => Link::translation(random() as isize % 5 - 2),
        };
        compare_with_expanded_graphs(link, true);
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
                    array: Link::from_offset(Some(0)),
                    packed: Link::from_offset(Some(shift)),
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
            true
        ));
        assert!(budget.exhausted);
        assert_eq!(compatible_cycle(&graph, &[node]), Some(false));
        for shift in 1..=32 {
            add_dependency_edge(
                &mut graph,
                node,
                node,
                GraphDependency::unconditional(BitDependency {
                    array: Link::from_offset(Some(0)),
                    packed: Link::from_offset(Some(-shift)),
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
            true
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
            true
        ));
        assert!(budget.exhausted);
        assert_eq!(compatible_cycle(&graph, &[node]), Some(true));
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
                    array: Link::from_offset(Some(offset)),
                    packed: Link::from_offset(Some(1 - offset)),
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
                true
            ));
            assert!(budget.exhausted);
        }
        assert_eq!(compatible_cycle(&graph, &[node]), Some(false));
    }
}
