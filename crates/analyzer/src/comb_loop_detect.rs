//! Combinational loop detection on the analyzer IR (issue #931).
//!
//! Builds a sparse per-module region dependency graph from independently
//! analyzable Memory SSA procedure summaries, then reports SCCs. Module
//! instance feedthrough is summarized bottom-up in topo order.
//!
//! Under-detect by design: opaque constructs (SystemVerilog black
//! boxes, `inout` ports, recursive functions) add no edges; the
//! simulator's `analyze_dependency` is the backup safety net.

use crate::AnalyzerError;
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, Component, Declaration, Expression, Factor,
    InstDeclaration, Ir, Module, Op, SystemFunctionKind, VarIndex, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction};
use petgraph::Direction::Incoming;
use petgraph::Graph;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
#[cfg(not(target_family = "wasm"))]
use rayon::prelude::*;
use std::collections::VecDeque;
use std::collections::{BTreeMap, BTreeSet};
use veryl_causal::graph::{EdgeKind, IncompleteReason};
use veryl_causal::procedure::ProcedureSummary;
use veryl_causal::region::{Region, Span};
use veryl_parser::resource_table::StrId;

/// One concrete unpacked-array element. Bit-precision lives in masks.
type IdxKey = (VarId, usize);

/// `(VarId, array_idx, range_idx)`. `range_idx` indexes the variable's
/// `BitPartition`, so bit-disjoint reads/writes form disjoint nodes.
type NodeKey = (VarId, usize, usize);

#[derive(Clone, Copy)]
enum SummaryDirection {
    Forward,
    Reverse,
}

struct SummaryWalk {
    direction: SummaryDirection,
    endpoints: Vec<(NodeIndex, NodeKey)>,
    next_endpoint: usize,
    current_key: Option<NodeKey>,
    visited: HashSet<(NodeIndex, bool)>,
    stack: Vec<(NodeIndex, bool)>,
    dependencies: BTreeMap<(Region<VarId>, SummaryOutput), bool>,
}

impl SummaryWalk {
    fn new(direction: SummaryDirection, endpoints: Vec<(NodeIndex, NodeKey)>) -> Self {
        Self {
            direction,
            endpoints,
            next_endpoint: 0,
            current_key: None,
            visited: HashSet::default(),
            stack: Vec::new(),
            dependencies: BTreeMap::new(),
        }
    }

    /// Advance a bounded amount of graph work. Running forward and reverse
    /// walks in equal quanta lets the cheaper direction finish without an
    /// endpoint-count heuristic committing to an adversarially expensive side.
    fn advance(
        &mut self,
        graph: &Graph<NodeKey, bool>,
        live_nodes: &[bool],
        module: &Module,
        bit_part: &BitPartition,
        input_ids: &HashSet<VarId>,
        output_ids: &HashSet<VarId>,
        budget: usize,
    ) -> bool {
        let mut work = 0usize;
        while work < budget {
            if self.stack.is_empty() {
                let Some(&(endpoint, key)) = self.endpoints.get(self.next_endpoint) else {
                    return true;
                };
                self.next_endpoint += 1;
                self.current_key = Some(key);
                self.visited.clear();
                self.stack.push((endpoint, true));
            }
            let Some((node, path_aligned)) = self.stack.pop() else {
                continue;
            };
            work += 1;
            if !self.visited.insert((node, path_aligned)) {
                continue;
            }
            let endpoint_key = self.current_key.expect("active summary endpoint");
            let node_key = graph[node];
            let pair = match self.direction {
                SummaryDirection::Forward if output_ids.contains(&node_key.0) => {
                    Some((endpoint_key, node_key))
                }
                SummaryDirection::Reverse if input_ids.contains(&node_key.0) => {
                    Some((node_key, endpoint_key))
                }
                SummaryDirection::Forward | SummaryDirection::Reverse => None,
            };
            if let Some((input_key, output_key)) = pair {
                for input in node_key_regions(input_key, module, bit_part) {
                    for output in node_key_summary_outputs(output_key, module, bit_part) {
                        self.dependencies
                            .entry((input, output))
                            .and_modify(|aligned| *aligned &= path_aligned)
                            .or_insert(path_aligned);
                    }
                }
            }
            match self.direction {
                SummaryDirection::Forward => {
                    self.stack.extend(
                        graph
                            .edges(node)
                            .filter(|edge| live_nodes[edge.target().index()])
                            .map(|edge| (edge.target(), path_aligned && *edge.weight())),
                    );
                }
                SummaryDirection::Reverse => {
                    self.stack.extend(
                        graph
                            .edges_directed(node, Incoming)
                            .filter(|edge| live_nodes[edge.source().index()])
                            .map(|edge| (edge.source(), path_aligned && *edge.weight())),
                    );
                }
            }
        }
        self.stack.is_empty() && self.next_endpoint == self.endpoints.len()
    }
}

fn live_summary_nodes(
    graph: &Graph<NodeKey, bool>,
    inputs: &[(NodeIndex, NodeKey)],
    outputs: &[(NodeIndex, NodeKey)],
) -> Vec<bool> {
    let mut reachable_from_input = vec![false; graph.node_count()];
    let mut stack = inputs.iter().map(|&(node, _)| node).collect::<Vec<_>>();
    while let Some(node) = stack.pop() {
        if std::mem::replace(&mut reachable_from_input[node.index()], true) {
            continue;
        }
        stack.extend(graph.edges(node).map(|edge| edge.target()));
    }

    let mut reaches_output = vec![false; graph.node_count()];
    stack.extend(outputs.iter().map(|&(node, _)| node));
    while let Some(node) = stack.pop() {
        if std::mem::replace(&mut reaches_output[node.index()], true) {
            continue;
        }
        stack.extend(
            graph
                .edges_directed(node, Incoming)
                .map(|edge| edge.source()),
        );
    }

    reachable_from_input
        .into_iter()
        .zip(reaches_output)
        .map(|(from_input, to_output)| from_input && to_output)
        .collect()
}

/// Sparse alias node for a dynamically selected region of one object.  This
/// deliberately occupies no real array element or bit-partition slot.
const UNKNOWN_REGION_INDEX: usize = usize::MAX;
const DENSE_REGION_INDEX: usize = usize::MAX - 1;
const PERIODIC_REGION_INDEX: usize = usize::MAX - 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PeriodicRegion {
    object: VarId,
    output: Span,
    repetitions: usize,
    stride: usize,
}

#[derive(Clone, Copy, Debug)]
struct PeriodicTransferRegion {
    input: Region<VarId>,
    output: PeriodicRegion,
    aligned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SummaryOutput {
    Region(Region<VarId>),
    Periodic(PeriodicRegion),
}

/// Per `IdxKey`, atomic bit-range masks. Two bits are in the same range
/// iff they appear in the same set of per-decl masks.
#[derive(Default)]
struct BitPartition {
    ranges: HashMap<VarId, BTreeMap<usize, Vec<Span>>>,
    /// Symbolic full-element interiors of large exact spans. Their graph-node
    /// count depends on observed spans, not on unpacked array length.
    dense_regions: Vec<Region<VarId>>,
    periodic_regions: Vec<PeriodicRegion>,
    /// Stable identities for unresolved regions. They are graph nodes, not
    /// array elements, so their count depends on accesses rather than width.
    wildcards: Vec<Region<VarId>>,
}

impl BitPartition {
    /// Empty slice means the variable's bits are untouched.
    fn ranges_of(&self, key: IdxKey) -> &[Span] {
        self.ranges
            .get(&key.0)
            .and_then(|elements| elements.get(&key.1))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn wildcard_keys(&self, region: Region<VarId>) -> Vec<NodeKey> {
        self.wildcards
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(index, wildcard)| {
                if !region.may_alias(wildcard) {
                    return None;
                }
                match wildcard {
                    Region::UnknownRegion { object, .. } | Region::UnknownObject(object) => {
                        Some((object, UNKNOWN_REGION_INDEX, index))
                    }
                    Region::Exact { .. } | Region::UnknownAll => None,
                }
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModuleCombDependency {
    input: Region<VarId>,
    output: SummaryOutput,
    aligned: bool,
}

/// Region-preserving combinational feedthrough across one module boundary.
#[derive(Clone, Debug, Default)]
struct ModuleCombSummary {
    dependencies: Vec<ModuleCombDependency>,
}

/// Compatibility entry point: emit only proven loop diagnostics. Tools which
/// need to surface analysis coverage must call [`check_detailed`].
pub fn check(ir: &Ir) -> Vec<AnalyzerError> {
    check_detailed(ir).errors
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncompleteCombAnalysis {
    pub module: String,
    pub reasons: BTreeSet<IncompleteReason>,
}

#[derive(Debug, Default)]
pub struct CombAnalysisResult {
    pub errors: Vec<AnalyzerError>,
    pub incomplete: Vec<IncompleteCombAnalysis>,
}

/// Run loop detection while retaining uncertainty which must not be promoted
/// to a hard combinational-loop diagnostic.
pub fn check_detailed(ir: &Ir) -> CombAnalysisResult {
    let mut errors = Vec::new();
    let mut incomplete = Vec::new();
    let mut summaries: HashMap<usize, ModuleCombSummary> = HashMap::default();
    let mut visiting_specializations = HashSet::default();

    let (order, recursion_affected_modules) = topo_order_modules(ir);

    for &idx in &order {
        if let Component::Module(module) = &ir.components[idx] {
            // Unevaluable generic params do not have a concrete module shape.
            if module.suppress_unassigned {
                incomplete.push(IncompleteCombAnalysis {
                    module: module.name.to_string(),
                    reasons: [IncompleteReason::UnevaluatedGeneric].into(),
                });
                continue;
            }
            ensure_instance_summaries(
                module,
                &mut summaries,
                &mut visiting_specializations,
                &mut errors,
                &mut incomplete,
            );
            let (graph, mut reasons, _bit_part) = build_module_graph(module, &summaries);
            if recursion_affected_modules.contains(&idx) {
                reasons.insert(IncompleteReason::RecursiveCall);
            }
            check_graph(module, &graph, &mut errors);
            if !reasons.is_empty() {
                incomplete.push(IncompleteCombAnalysis {
                    module: module.name.to_string(),
                    reasons,
                });
            }
        }
    }

    CombAnalysisResult { errors, incomplete }
}

fn component_summary_key(inst: &InstDeclaration) -> usize {
    std::sync::Arc::as_ptr(&inst.component) as *const () as usize
}

/// Analyze the concrete module carried by each instance. Generic parameter
/// overrides are already elaborated in this `Arc<Component>` and therefore
/// cannot share the declaration-default summary keyed only by module name.
fn ensure_instance_summaries(
    module: &Module,
    summaries: &mut HashMap<usize, ModuleCombSummary>,
    visiting: &mut HashSet<usize>,
    errors: &mut Vec<AnalyzerError>,
    incomplete: &mut Vec<IncompleteCombAnalysis>,
) {
    for inst in walk_insts(module) {
        let Component::Module(child) = inst.component.as_ref() else {
            continue;
        };
        let key = component_summary_key(inst);
        if summaries.contains_key(&key) {
            continue;
        }
        if !visiting.insert(key) {
            incomplete.push(IncompleteCombAnalysis {
                module: child.name.to_string(),
                reasons: [IncompleteReason::RecursiveCall].into(),
            });
            continue;
        }
        ensure_instance_summaries(child, summaries, visiting, errors, incomplete);
        let (graph, reasons, bit_part) = build_module_graph(child, summaries);
        check_graph(child, &graph, errors);
        let summary = compute_module_summary(child, &graph, &bit_part);
        summaries.insert(key, summary);
        if !reasons.is_empty() {
            incomplete.push(IncompleteCombAnalysis {
                module: child.name.to_string(),
                reasons,
            });
        }
        visiting.remove(&key);
    }
}

/// Children before parents. Modules in, or transitively depending on, an
/// instantiation cycle are appended in input order because no bottom-up order
/// exists for them (`infinite_recursion` is reported separately).
fn topo_order_modules(ir: &Ir) -> (Vec<usize>, HashSet<usize>) {
    let mut name_to_idx: HashMap<StrId, usize> = HashMap::default();
    for (i, c) in ir.components.iter().enumerate() {
        if let Component::Module(m) = c {
            name_to_idx.insert(m.name, i);
        }
    }

    let n = ir.components.len();
    let mut deps: Vec<HashSet<usize>> = vec![HashSet::default(); n];
    let mut rev_deps: Vec<HashSet<usize>> = vec![HashSet::default(); n];

    for (i, c) in ir.components.iter().enumerate() {
        if let Component::Module(m) = c {
            for inst in walk_insts(m) {
                if let Component::Module(child) = inst.component.as_ref()
                    && let Some(&child_idx) = name_to_idx.get(&child.name)
                {
                    deps[i].insert(child_idx);
                    rev_deps[child_idx].insert(i);
                }
            }
        }
    }

    let is_module = ir
        .components
        .iter()
        .map(|component| matches!(component, Component::Module(_)))
        .collect::<Vec<_>>();
    order_from_dependencies(&is_module, &deps, &rev_deps)
}

fn order_from_dependencies(
    is_module: &[bool],
    deps: &[HashSet<usize>],
    rev_deps: &[HashSet<usize>],
) -> (Vec<usize>, HashSet<usize>) {
    let n = is_module.len();
    let mut indeg: Vec<usize> = deps.iter().map(HashSet::len).collect();
    let mut q: VecDeque<usize> = VecDeque::new();
    for (i, _) in indeg.iter().enumerate().take(n) {
        if is_module[i] && indeg[i] == 0 {
            q.push_back(i);
        }
    }
    let mut order: Vec<usize> = Vec::new();
    while let Some(i) = q.pop_front() {
        order.push(i);
        for &p in &rev_deps[i] {
            indeg[p] -= 1;
            if indeg[p] == 0 {
                q.push_back(p);
            }
        }
    }
    let ordered = order.iter().copied().collect::<HashSet<_>>();
    let recursion_affected = (0..n)
        .filter(|i| is_module[*i] && !ordered.contains(i))
        .collect::<HashSet<_>>();
    order.extend(recursion_affected.iter().copied());
    // HashSet iteration is intentionally not part of summary identity.
    order[ordered.len()..].sort_unstable();
    (order, recursion_affected)
}

fn walk_insts(module: &Module) -> impl Iterator<Item = &InstDeclaration> {
    module.declarations.iter().filter_map(|d| match d {
        Declaration::Inst(inst) => Some(inst.as_ref()),
        Declaration::Comb(_)
        | Declaration::Ff(_)
        | Declaration::External(_)
        | Declaration::Initial(_)
        | Declaration::Final(_)
        | Declaration::Unsupported(_)
        | Declaration::Null => None,
    })
}

/// Group bits into atomic ranges by signature: bits with the same set
/// of containing masks form one range. Bits in zero masks are dropped.
fn atomic_ranges(masks: &[Span], width: usize) -> Vec<Span> {
    // Every mask is already a compact interval. Sweep its endpoints instead
    // of constructing width-sized BigUints or comparing every interval with
    // every mask. A finer split at each transition is causally equivalent to
    // merging non-contiguous intervals with the same signature.
    let mut events = BTreeMap::<usize, isize>::new();
    for mask in masks {
        let Some(end) = mask.end().map(|end| end.min(width)) else {
            continue;
        };
        let start = mask.start.min(width);
        if start >= end {
            continue;
        }
        *events.entry(start).or_default() += 1;
        *events.entry(end).or_default() -= 1;
    }

    let mut active = 0isize;
    let mut previous = 0usize;
    let mut ranges = Vec::new();
    for (endpoint, delta) in events {
        if active > 0 && previous < endpoint {
            ranges.push(Span {
                start: previous,
                length: endpoint - previous,
            });
        }
        active += delta;
        previous = endpoint;
    }
    ranges
}

fn build_bit_partition(
    module: &Module,
    memory_ssa_regions: &[Region<VarId>],
    periodic_regions: &[PeriodicRegion],
) -> BitPartition {
    let mut masks: HashMap<VarId, BTreeMap<usize, Vec<Span>>> = HashMap::default();

    // A shifted copy can propagate every observed endpoint through the copy
    // relation. Feed those sparse spans into the shared partition so distinct
    // causal atoms do not collapse into a spurious self-edge.
    let dense_regions = collect_region_masks(memory_ssa_regions, module, &mut masks);
    collect_periodic_masks(periodic_regions, module, &mut masks);

    let mut ranges: HashMap<VarId, BTreeMap<usize, Vec<Span>>> = HashMap::default();
    for (object, elements) in masks {
        let width = module
            .variables
            .get(&object)
            .and_then(|variable| variable.total_width())
            .unwrap_or(1);
        for (element, mut element_masks) in elements {
            element_masks.sort_unstable();
            element_masks.dedup();
            let parts = atomic_ranges(&element_masks, width);
            if !parts.is_empty() {
                ranges.entry(object).or_default().insert(element, parts);
            }
        }
    }

    let wildcards = memory_ssa_regions
        .iter()
        .copied()
        .filter(|region| {
            matches!(
                region,
                Region::UnknownRegion { .. } | Region::UnknownObject(_)
            )
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    BitPartition {
        ranges,
        dense_regions,
        periodic_regions: periodic_regions.to_vec(),
        wildcards,
    }
}

fn collect_periodic_masks(
    periodic_regions: &[PeriodicRegion],
    module: &Module,
    masks: &mut HashMap<VarId, BTreeMap<usize, Vec<Span>>>,
) {
    for &periodic in periodic_regions {
        if periodic.repetitions == 0 || periodic.output.length == 0 {
            continue;
        }
        let Some(width) = module
            .variables
            .get(&periodic.object)
            .and_then(Variable::total_width)
        else {
            continue;
        };
        if width == 0 {
            continue;
        }
        let Some(last_start) = periodic
            .stride
            .checked_mul(periodic.repetitions - 1)
            .and_then(|offset| periodic.output.start.checked_add(offset))
        else {
            continue;
        };
        let Some(pattern_end) = last_start.checked_add(periodic.output.length) else {
            continue;
        };
        let first_element = periodic.output.start / width;
        masks
            .entry(periodic.object)
            .or_default()
            .entry(first_element)
            .or_default();
        let elements = masks.entry(periodic.object).or_default();
        let materialized = elements
            .range(first_element..=((pattern_end - 1) / width))
            .map(|(&element, _)| element)
            .collect::<Vec<_>>();
        for element in materialized {
            let Some(element_start) = element.checked_mul(width) else {
                continue;
            };
            let query = Span {
                start: element_start,
                length: width,
            };
            for overlap in periodic_span_intersections(periodic, query) {
                elements
                    .get_mut(&element)
                    .expect("materialized periodic element")
                    .push(Span {
                        start: overlap.start - element_start,
                        length: overlap.length,
                    });
            }
        }
    }
}

fn propagate_periodic_partition_regions(
    regions: &[Region<VarId>],
    transfers: &[PeriodicTransferRegion],
    periodic_regions: &mut Vec<PeriodicRegion>,
) {
    for transfer in transfers.iter().filter(|transfer| transfer.aligned) {
        let Region::Exact {
            object: input_object,
            span: input_span,
        } = transfer.input
        else {
            continue;
        };
        for &region in regions {
            let Region::Exact {
                object,
                span: region_span,
            } = region
            else {
                continue;
            };
            if object != input_object {
                continue;
            }
            let Some(overlap) = region_span.intersection(input_span) else {
                continue;
            };
            let Some(offset) = overlap.start.checked_sub(input_span.start) else {
                continue;
            };
            let Some(start) = transfer.output.output.start.checked_add(offset) else {
                continue;
            };
            periodic_regions.push(PeriodicRegion {
                object: transfer.output.object,
                output: Span {
                    start,
                    length: overlap.length,
                },
                repetitions: transfer.output.repetitions,
                stride: transfer.output.stride,
            });
        }
    }
}

fn periodic_span_intersections(periodic: PeriodicRegion, query: Span) -> Vec<Span> {
    let Some(query_end) = query.end() else {
        return Vec::new();
    };
    let Some(phase_end) = periodic.output.end() else {
        return Vec::new();
    };
    if periodic.repetitions == 0 || periodic.stride == 0 {
        return periodic.output.intersection(query).into_iter().collect();
    }
    if periodic.output.length == periodic.stride {
        let Some(length) = periodic.stride.checked_mul(periodic.repetitions) else {
            return Vec::new();
        };
        return (Span {
            start: periodic.output.start,
            length,
        })
        .intersection(query)
        .into_iter()
        .collect();
    }
    let first = if query.start < phase_end {
        0
    } else {
        (query.start - phase_end) / periodic.stride + 1
    };
    let end = if query_end <= periodic.output.start {
        0
    } else {
        query_end
            .saturating_sub(periodic.output.start)
            .saturating_add(periodic.stride - 1)
            / periodic.stride
    }
    .min(periodic.repetitions);
    (first..end)
        .filter_map(|copy| {
            let start = periodic
                .output
                .start
                .checked_add(copy.checked_mul(periodic.stride)?)?;
            (Span {
                start,
                length: periodic.output.length,
            })
            .intersection(query)
        })
        .collect()
}

fn collect_region_masks(
    regions: &[Region<VarId>],
    module: &Module,
    masks: &mut HashMap<VarId, BTreeMap<usize, Vec<Span>>>,
) -> Vec<Region<VarId>> {
    // Full-element interiors are homogeneous. Materialize only their boundary
    // representatives and any sparse element already observed by another
    // region. This keeps whole-array work independent of the declared number
    // of unpacked elements while preserving every relevant intersection.
    let mut full_interiors: HashMap<VarId, Vec<(usize, usize)>> = HashMap::default();
    for &region in regions {
        let Region::Exact { object, span } = region else {
            continue;
        };
        let Some(width) = module
            .variables
            .get(&object)
            .and_then(Variable::total_width)
        else {
            continue;
        };
        let Some(end) = span.end() else {
            continue;
        };
        if width == 0 || span.start >= end {
            continue;
        }

        let first = span.start / width;
        let last = (end - 1) / width;
        let first_end = end.min((first + 1).saturating_mul(width));
        push_element_mask(
            masks,
            object,
            first,
            Span {
                start: span.start % width,
                length: first_end - span.start,
            },
        );
        if last != first {
            push_element_mask(
                masks,
                object,
                last,
                Span {
                    start: 0,
                    length: end - last * width,
                },
            );
        }

        let interior_start = first.saturating_add(1);
        if interior_start < last {
            full_interiors
                .entry(object)
                .or_default()
                .push((interior_start, last));
        }
    }

    let mut dense_regions = Vec::new();
    for (object, mut intervals) in full_interiors {
        intervals.sort_unstable();
        intervals.dedup();

        let Some(width) = module
            .variables
            .get(&object)
            .and_then(Variable::total_width)
        else {
            continue;
        };
        let full_mask = Span {
            start: 0,
            length: width,
        };
        let elements = masks.entry(object).or_default();
        for (start, end) in intervals {
            dense_regions.push(Region::Exact {
                object,
                span: Span {
                    start: start.saturating_mul(width),
                    length: end.saturating_sub(start).saturating_mul(width),
                },
            });
            // One representative retains dependencies which touch only the
            // whole interval. Other exact accesses have already inserted
            // their own element keys and are filled by the range query.
            elements.entry(start).or_default();
            let materialized = elements
                .range(start..end)
                .map(|(&element, _)| element)
                .collect::<Vec<_>>();
            for element in materialized {
                elements
                    .get_mut(&element)
                    .expect("materialized element")
                    .push(full_mask);
            }
        }
    }
    dense_regions.sort_unstable();
    dense_regions.dedup();
    dense_regions
}

fn push_element_mask(
    masks: &mut HashMap<VarId, BTreeMap<usize, Vec<Span>>>,
    object: VarId,
    element: usize,
    mask: Span,
) {
    masks
        .entry(object)
        .or_default()
        .entry(element)
        .or_default()
        .push(mask);
}

fn build_module_graph(
    module: &Module,
    summaries: &HashMap<usize, ModuleCombSummary>,
) -> (
    Graph<NodeKey, bool>,
    BTreeSet<IncompleteReason>,
    BitPartition,
) {
    // Procedural combinational declarations use statement-ordered region
    // MemorySSA. Keep instance declarations on the existing bottom-up module
    // summary path until the same region vocabulary crosses module boundaries.
    let analyze_declaration = |declaration: &Declaration| match declaration {
        Declaration::Comb(comb) => Some(crate::comb_memory_ssa::analyze(module, comb)),
        Declaration::Ff(_)
        | Declaration::Inst(_)
        | Declaration::External(_)
        | Declaration::Initial(_)
        | Declaration::Final(_)
        | Declaration::Unsupported(_)
        | Declaration::Null => None,
    };
    #[cfg(not(target_family = "wasm"))]
    let mut procedure_summaries = module
        .declarations
        .par_iter()
        .filter_map(analyze_declaration)
        .collect::<Vec<_>>();
    #[cfg(target_family = "wasm")]
    let mut procedure_summaries = module
        .declarations
        .iter()
        .filter_map(analyze_declaration)
        .collect::<Vec<_>>();

    let instance_inputs = walk_insts(module)
        .flat_map(|inst| inst.inputs.iter().map(|input| &input.expr))
        .collect::<Vec<_>>();
    #[cfg(not(target_family = "wasm"))]
    procedure_summaries.extend(
        instance_inputs
            .par_iter()
            .map(|expression| {
                crate::comb_memory_ssa::analyze_observer_expression(module, expression)
            })
            .collect::<Vec<_>>(),
    );
    #[cfg(target_family = "wasm")]
    procedure_summaries.extend(
        instance_inputs.iter().map(|expression| {
            crate::comb_memory_ssa::analyze_observer_expression(module, expression)
        }),
    );

    let mut partition_regions = procedure_summaries
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .flat_map(|summary| {
            summary.dependencies.iter().flat_map(|dependency| {
                [
                    sparse_input_region(summary, dependency.input, dependency.output),
                    sparse_output_region(summary, dependency.output),
                ]
            })
        })
        .collect::<Vec<_>>();
    partition_regions.extend(
        procedure_summaries
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .flat_map(|summary| &summary.periodic_dependencies)
            .map(|dependency| dependency.input),
    );
    partition_regions = propagate_aligned_regions(partition_regions, &procedure_summaries);
    let mut periodic_transfers = procedure_summaries
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .flat_map(|summary| &summary.periodic_dependencies)
        .map(|dependency| PeriodicTransferRegion {
            input: dependency.input,
            output: PeriodicRegion {
                object: dependency.output_object,
                output: dependency.output,
                repetitions: dependency.repetitions,
                stride: dependency.destination_stride,
            },
            aligned: dependency.aligned,
        })
        .collect::<Vec<_>>();
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.functions = module.functions.clone();
    let (instance_regions, instance_periodic_transfers) =
        collect_instance_summary_regions(module, summaries, &mut ctx);
    partition_regions.extend(instance_regions);
    periodic_transfers.extend(instance_periodic_transfers);
    let mut periodic_regions = periodic_transfers
        .iter()
        .map(|transfer| transfer.output)
        .collect::<Vec<_>>();
    propagate_periodic_partition_regions(
        &partition_regions,
        &periodic_transfers,
        &mut periodic_regions,
    );
    periodic_regions.sort_unstable();
    periodic_regions.dedup();
    let bit_part = build_bit_partition(module, &partition_regions, &periodic_regions);

    let mut graph: Graph<NodeKey, bool> = Graph::new();
    let mut node_map: HashMap<NodeKey, NodeIndex> = HashMap::default();
    let mut incomplete = BTreeSet::new();

    if module
        .variables
        .values()
        .any(|variable| matches!(variable.kind, crate::ir::VarKind::Inout))
    {
        incomplete.insert(IncompleteReason::InoutPort);
    }

    for declaration in &module.declarations {
        match declaration {
            Declaration::External(_) => {
                incomplete.insert(IncompleteReason::ExternalComponent);
            }
            Declaration::Unsupported(_) => {
                incomplete.insert(IncompleteReason::MalformedModel);
            }
            Declaration::Comb(_)
            | Declaration::Ff(_)
            | Declaration::Inst(_)
            | Declaration::Initial(_)
            | Declaration::Final(_)
            | Declaration::Null => {}
        }
    }
    for result in &procedure_summaries {
        match result {
            Ok(summary) => {
                incomplete.extend(summary.incomplete.iter().copied());
                add_memory_ssa_edges(module, summary, &bit_part, &mut graph, &mut node_map);
                add_periodic_memory_ssa_edges(
                    module,
                    summary,
                    &bit_part,
                    &mut graph,
                    &mut node_map,
                );
            }
            Err(error) => {
                incomplete.insert(IncompleteReason::MalformedModel);
                log::debug!(
                    "failed to build comb MemorySSA for {}: {error}",
                    module.name
                );
            }
        }
    }

    for inst in walk_insts(module) {
        match inst.component.as_ref() {
            Component::Module(child) => {
                if child
                    .variables
                    .values()
                    .any(|variable| matches!(variable.kind, crate::ir::VarKind::Inout))
                {
                    incomplete.insert(IncompleteReason::InoutPort);
                }
                add_inst_output_address_edges(
                    inst,
                    &bit_part,
                    &mut graph,
                    &mut node_map,
                    &module.variables,
                    &mut ctx,
                );
                let Some(summary) = summaries.get(&component_summary_key(inst)) else {
                    continue;
                };
                add_inst_feedthrough_edges(
                    inst,
                    child,
                    summary,
                    &bit_part,
                    &mut graph,
                    &mut node_map,
                    &module.variables,
                    &mut incomplete,
                    &mut ctx,
                );
            }
            // SV black box: under-detect.
            Component::SystemVerilog(_) => {
                incomplete.insert(IncompleteReason::ExternalComponent);
            }
            // Interface signals are already lifted into the parent.
            Component::Interface(_) => {}
        }
    }

    (graph, incomplete, bit_part)
}

fn propagate_aligned_regions(
    regions: Vec<Region<VarId>>,
    summaries: &[Result<ProcedureSummary<VarId>, veryl_causal::procedure::ProcedureError>],
) -> Vec<Region<VarId>> {
    #[derive(Clone, Copy)]
    struct Transfer {
        from: Span,
        to_object: VarId,
        to: Span,
    }

    struct TransferIndex {
        transfers: Vec<Transfer>,
        leaf_base: usize,
        max_end: Vec<usize>,
    }

    impl TransferIndex {
        fn new(mut transfers: Vec<Transfer>) -> Self {
            transfers.sort_unstable_by_key(|transfer| transfer.from.start);
            let leaf_base = transfers.len().next_power_of_two().max(1);
            let mut max_end = vec![0; leaf_base * 2];
            for (index, transfer) in transfers.iter().enumerate() {
                max_end[leaf_base + index] = transfer.from.end().unwrap_or(usize::MAX);
            }
            for index in (1..leaf_base).rev() {
                max_end[index] = max_end[index * 2].max(max_end[index * 2 + 1]);
            }
            Self {
                transfers,
                leaf_base,
                max_end,
            }
        }

        fn for_each_overlap(&self, span: Span, mut visit: impl FnMut(Transfer)) {
            let Some(end) = span.end() else {
                return;
            };
            let high = self
                .transfers
                .partition_point(|transfer| transfer.from.start < end);
            self.visit_overlaps(1, 0, self.leaf_base, high, span.start, &mut visit);
        }

        fn visit_overlaps(
            &self,
            node: usize,
            low: usize,
            high: usize,
            query_high: usize,
            query_low: usize,
            visit: &mut impl FnMut(Transfer),
        ) {
            if low >= query_high || self.max_end[node] <= query_low {
                return;
            }
            if high - low == 1 {
                if let Some(&transfer) = self.transfers.get(low) {
                    visit(transfer);
                }
                return;
            }
            let middle = low + (high - low) / 2;
            self.visit_overlaps(node * 2, low, middle, query_high, query_low, visit);
            self.visit_overlaps(node * 2 + 1, middle, high, query_high, query_low, visit);
        }
    }

    let raw_transfers = summaries
        .iter()
        .filter_map(|summary| summary.as_ref().ok())
        .flat_map(|summary| &summary.dependencies)
        .filter_map(|dependency| match (dependency.input, dependency.output) {
            (
                Region::Exact {
                    object: input_object,
                    span: input_span,
                },
                Region::Exact {
                    object: output_object,
                    span: output_span,
                },
            ) if dependency.aligned && input_span.length == output_span.length => {
                Some(((input_object, input_span), (output_object, output_span)))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut transfers = HashMap::<VarId, Vec<Transfer>>::default();
    for (left, right) in raw_transfers {
        for (from, to) in [(left, right), (right, left)] {
            transfers.entry(from.0).or_default().push(Transfer {
                from: from.1,
                to_object: to.0,
                to: to.1,
            });
        }
    }
    let transfers = transfers
        .into_iter()
        .map(|(object, transfers)| (object, TransferIndex::new(transfers)))
        .collect::<HashMap<_, _>>();
    let mut known = regions.into_iter().collect::<BTreeSet<_>>();
    let mut pending = known
        .iter()
        .copied()
        .collect::<std::collections::VecDeque<_>>();
    while let Some(region) = pending.pop_front() {
        let Region::Exact { object, span } = region else {
            continue;
        };
        let Some(object_transfers) = transfers.get(&object) else {
            continue;
        };
        object_transfers.for_each_overlap(span, |transfer| {
            let Some(overlap) = span.intersection(transfer.from) else {
                return;
            };
            let Some(offset) = overlap.start.checked_sub(transfer.from.start) else {
                return;
            };
            let Some(start) = transfer.to.start.checked_add(offset) else {
                return;
            };
            let mapped = Region::Exact {
                object: transfer.to_object,
                span: Span {
                    start,
                    length: overlap.length,
                },
            };
            if known.insert(mapped) {
                pending.push_back(mapped);
            }
        });
    }
    known.into_iter().collect()
}

fn add_memory_ssa_edges(
    module: &Module,
    summary: &ProcedureSummary<VarId>,
    bit_part: &BitPartition,
    graph: &mut Graph<NodeKey, bool>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
) {
    // Incomplete effects are represented by Unknown edges/regions in the
    // causal model. They must not erase independent, proven dependencies from
    // the same procedure: doing so lets one opaque read hide an unrelated
    // scalar loop. Only proven (non-Unknown) edges become hard diagnostics.
    for dependency in &summary.dependencies {
        if dependency.kind == EdgeKind::Unknown {
            continue;
        }
        let input = sparse_input_region(summary, dependency.input, dependency.output);
        let output = sparse_output_region(summary, dependency.output);
        let preserve_alignment =
            dependency.aligned && exact_regions_have_equal_length(input, output);
        let pairs = if preserve_alignment {
            aligned_region_node_pairs(input, output, &module.variables, bit_part)
        } else {
            let sources = region_node_keys(input, &module.variables, bit_part);
            let destinations = region_node_keys(output, &module.variables, bit_part);
            sources
                .into_iter()
                .flat_map(|source| {
                    destinations
                        .iter()
                        .copied()
                        .map(move |destination| (source, destination))
                })
                .collect()
        };
        for (source, destination) in pairs {
            if !is_module_scope_var(source.0, &module.variables) {
                continue;
            }
            if !is_module_scope_var(destination.0, &module.variables) {
                continue;
            }
            let source = ensure_node(graph, node_map, source);
            let destination = ensure_node(graph, node_map, destination);
            graph.add_edge(source, destination, preserve_alignment);
        }
    }
}

fn add_periodic_memory_ssa_edges(
    module: &Module,
    summary: &ProcedureSummary<VarId>,
    bit_part: &BitPartition,
    graph: &mut Graph<NodeKey, bool>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
) {
    for dependency in &summary.periodic_dependencies {
        if dependency.kind == EdgeKind::Unknown {
            continue;
        }
        let input = restore_uncertain_region(&summary.uncertain_input_regions, dependency.input);
        let periodic = PeriodicRegion {
            object: dependency.output_object,
            output: dependency.output,
            repetitions: dependency.repetitions,
            stride: dependency.destination_stride,
        };
        let pairs = if dependency.aligned {
            aligned_periodic_node_pairs(input, periodic, &module.variables, bit_part)
        } else {
            let destinations = periodic_region_node_keys(periodic, &module.variables, bit_part);
            region_node_keys(input, &module.variables, bit_part)
                .into_iter()
                .flat_map(|source| {
                    destinations
                        .iter()
                        .copied()
                        .map(move |destination| (source, destination))
                })
                .collect()
        };
        for (source, destination) in pairs {
            if !is_module_scope_var(source.0, &module.variables) {
                continue;
            }
            if !is_module_scope_var(destination.0, &module.variables) {
                continue;
            }
            let source = ensure_node(graph, node_map, source);
            let destination = ensure_node(graph, node_map, destination);
            graph.add_edge(source, destination, dependency.aligned);
        }
    }
}

fn aligned_periodic_node_pairs(
    input: Region<VarId>,
    periodic: PeriodicRegion,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<(NodeKey, NodeKey)> {
    let Region::Exact {
        span: input_span, ..
    } = input
    else {
        return Vec::new();
    };
    let mut pairs = BTreeSet::new();
    for source in region_node_keys(input, variables, bit_part) {
        for source_region in node_key_regions_for_variables(source, variables, bit_part) {
            let Region::Exact {
                span: source_span, ..
            } = source_region
            else {
                continue;
            };
            let Some(overlap) = source_span.intersection(input_span) else {
                continue;
            };
            let Some(offset) = overlap.start.checked_sub(input_span.start) else {
                continue;
            };
            let Some(output_start) = periodic.output.start.checked_add(offset) else {
                continue;
            };
            let mapped = PeriodicRegion {
                object: periodic.object,
                output: Span {
                    start: output_start,
                    length: overlap.length,
                },
                repetitions: periodic.repetitions,
                stride: periodic.stride,
            };
            pairs.extend(
                periodic_region_node_keys(mapped, variables, bit_part)
                    .into_iter()
                    .map(|destination| (source, destination)),
            );
        }
    }
    pairs.into_iter().collect()
}

fn aligned_region_node_pairs(
    input: Region<VarId>,
    output: Region<VarId>,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<(NodeKey, NodeKey)> {
    let (
        Region::Exact {
            object: _,
            span: input_span,
        },
        Region::Exact {
            object: output_object,
            span: output_span,
        },
    ) = (input, output)
    else {
        let sources = region_node_keys(input, variables, bit_part);
        let destinations = region_node_keys(output, variables, bit_part);
        return sources
            .into_iter()
            .flat_map(|source| {
                destinations
                    .iter()
                    .copied()
                    .map(move |destination| (source, destination))
            })
            .collect();
    };
    if input_span.length != output_span.length {
        return Vec::new();
    }
    let mut pairs = BTreeSet::new();
    for source in region_node_keys(input, variables, bit_part) {
        for source_region in node_key_regions_for_variables(source, variables, bit_part) {
            let Region::Exact {
                span: source_span, ..
            } = source_region
            else {
                continue;
            };
            let Some(overlap) = source_span.intersection(input_span) else {
                continue;
            };
            let Some(offset) = overlap.start.checked_sub(input_span.start) else {
                continue;
            };
            let Some(mapped_start) = output_span.start.checked_add(offset) else {
                continue;
            };
            let mapped = Region::Exact {
                object: output_object,
                span: Span {
                    start: mapped_start,
                    length: overlap.length,
                },
            };
            pairs.extend(
                region_node_keys(mapped, variables, bit_part)
                    .into_iter()
                    .map(|destination| (source, destination)),
            );
        }
    }
    pairs.into_iter().collect()
}

fn sparse_input_region(
    summary: &ProcedureSummary<VarId>,
    input: Region<VarId>,
    output: Region<VarId>,
) -> Region<VarId> {
    if summary
        .uncertain_input_dependencies
        .contains(&(input, output))
    {
        restore_uncertain_region(&summary.uncertain_input_regions, input)
    } else {
        input
    }
}

/// Memory SSA expands an unresolved write over its sparse atom endpoints
/// so kill/phi semantics stay exact. Do not turn the resulting large exact
/// spans back into per-element graph nodes. Restore the narrowest original
/// uncertain region, retaining any statically known prefix.
fn sparse_output_region(summary: &ProcedureSummary<VarId>, output: Region<VarId>) -> Region<VarId> {
    restore_uncertain_region(&summary.uncertain_write_regions, output)
}

fn restore_uncertain_region(
    uncertain: &BTreeSet<Region<VarId>>,
    exact: Region<VarId>,
) -> Region<VarId> {
    let Region::Exact { object, span } = exact else {
        return exact;
    };
    uncertain
        .iter()
        .copied()
        .filter(|region| match region {
            Region::UnknownRegion {
                object: candidate,
                span: candidate_span,
            } => *candidate == object && candidate_span.intersection(span) == Some(span),
            Region::UnknownObject(candidate) => *candidate == object,
            Region::Exact { .. } | Region::UnknownAll => false,
        })
        .min_by_key(|region| match region {
            Region::UnknownRegion { span, .. } => span.length,
            Region::UnknownObject(_) => usize::MAX,
            Region::Exact { .. } | Region::UnknownAll => 0,
        })
        .unwrap_or(exact)
}

fn sparse_exact_node_keys(
    object: VarId,
    span: Span,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<NodeKey> {
    let Some(width) = variables.get(&object).and_then(Variable::total_width) else {
        return Vec::new();
    };
    if width == 0 {
        return Vec::new();
    }
    if span.end().is_none() {
        return Vec::new();
    }
    let Some(elements) = bit_part.ranges.get(&object) else {
        return Vec::new();
    };
    let Some(end) = span.end() else {
        return Vec::new();
    };
    if span.length == 0 {
        return Vec::new();
    }
    let first_element = span.start / width;
    let last_element = (end - 1) / width;
    let mut keys = Vec::new();
    for (&element, ranges) in elements.range(first_element..=last_element) {
        let Some(element_start) = element.checked_mul(width) else {
            continue;
        };
        let Some(overlap) = (Span {
            start: element_start,
            length: width,
        })
        .intersection(span) else {
            continue;
        };
        let local_start = overlap.start - element_start;
        let local_end = local_start + overlap.length;
        let selected = Span {
            start: local_start,
            length: local_end - local_start,
        };
        keys.extend(
            ranges
                .iter()
                .enumerate()
                .filter(|(_, range)| range.intersection(selected).is_some())
                .map(|(range, _)| (object, element, range)),
        );
    }
    keys
}

fn dense_region_node_keys(object: VarId, span: Span, bit_part: &BitPartition) -> Vec<NodeKey> {
    bit_part
        .dense_regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| match region {
            Region::Exact {
                object: candidate,
                span: candidate_span,
            } if *candidate == object
                && span.intersection(*candidate_span) == Some(*candidate_span) =>
            {
                Some((object, DENSE_REGION_INDEX, index))
            }
            Region::Exact { .. }
            | Region::UnknownRegion { .. }
            | Region::UnknownObject(_)
            | Region::UnknownAll => None,
        })
        .collect()
}

fn periodic_region_node_keys(
    periodic: PeriodicRegion,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<NodeKey> {
    let mut keys = bit_part
        .periodic_regions
        .iter()
        .position(|candidate| *candidate == periodic)
        .map(|index| vec![(periodic.object, PERIODIC_REGION_INDEX, index)])
        .unwrap_or_default();
    let Some(width) = variables
        .get(&periodic.object)
        .and_then(Variable::total_width)
    else {
        return keys;
    };
    let Some(last_start) = periodic
        .stride
        .checked_mul(periodic.repetitions.saturating_sub(1))
        .and_then(|offset| periodic.output.start.checked_add(offset))
    else {
        return keys;
    };
    let Some(end) = last_start.checked_add(periodic.output.length) else {
        return keys;
    };
    let Some(elements) = bit_part.ranges.get(&periodic.object) else {
        return keys;
    };
    for (&element, ranges) in elements.range((periodic.output.start / width)..=((end - 1) / width))
    {
        let Some(element_start) = element.checked_mul(width) else {
            continue;
        };
        for (range_index, range) in ranges.iter().enumerate() {
            let Some(start) = element_start.checked_add(range.start) else {
                continue;
            };
            let query = Span {
                start,
                length: range.length,
            };
            if !periodic_span_intersections(periodic, query).is_empty() {
                keys.push((periodic.object, element, range_index));
            }
        }
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Convert the MemorySSA engine's flattened, half-open bit region back to the
/// graph `(array element, bit partition)` coordinates. The loop is
/// proportional to touched elements, never to the declared array size.
fn region_node_keys(
    region: Region<VarId>,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<NodeKey> {
    let mut keys = match region {
        Region::Exact { object, span } => {
            let mut keys = sparse_exact_node_keys(object, span, variables, bit_part);
            keys.extend(dense_region_node_keys(object, span, bit_part));
            keys
        }
        Region::UnknownRegion { object, span } => {
            sparse_exact_node_keys(object, span, variables, bit_part)
        }
        Region::UnknownObject(object) => bit_part
            .ranges
            .get(&object)
            .into_iter()
            .flat_map(|elements| elements.iter())
            .flat_map(|(&element, ranges)| {
                (0..ranges.len()).map(move |range| (object, element, range))
            })
            .collect(),
        Region::UnknownAll => return Vec::new(),
    };
    keys.extend(bit_part.wildcard_keys(region));
    keys.sort_unstable();
    keys.dedup();
    keys
}

fn collect_instance_summary_regions(
    module: &Module,
    summaries: &HashMap<usize, ModuleCombSummary>,
    ctx: &mut Context,
) -> (Vec<Region<VarId>>, Vec<PeriodicTransferRegion>) {
    let mut regions = Vec::new();
    let mut periodic_transfers = Vec::new();
    for inst in walk_insts(module) {
        let Component::Module(child) = inst.component.as_ref() else {
            continue;
        };
        for output in &inst.outputs {
            regions.extend(collect_destination_regions(
                &output.dst,
                &module.variables,
                ctx,
            ));
            regions.extend(collect_destination_address_regions(
                &output.dst,
                &module.variables,
                ctx,
            ));
        }
        let Some(summary) = summaries.get(&component_summary_key(inst)) else {
            continue;
        };
        for dependency in &summary.dependencies {
            let Some(child_input) = region_object(dependency.input) else {
                continue;
            };
            let Some(child_output) = summary_output_object(dependency.output) else {
                continue;
            };
            let Some(input) = inst.inputs.iter().find(|input| input.id == child_input) else {
                continue;
            };
            let Some(output) = inst.outputs.iter().find(|output| output.id == child_output) else {
                continue;
            };
            let parent_inputs = map_child_input_region(
                dependency.input,
                child,
                &input.expr,
                &module.variables,
                ctx,
            );
            regions.extend(parent_inputs.iter().copied());
            match dependency.output {
                SummaryOutput::Region(region) => regions.extend(map_child_output_region(
                    region,
                    child,
                    &output.dst,
                    &module.variables,
                    ctx,
                )),
                SummaryOutput::Periodic(periodic) => {
                    let parent_outputs = map_child_periodic_output(
                        periodic,
                        child,
                        &output.dst,
                        &module.variables,
                        ctx,
                    )
                    .unwrap_or_default();
                    for &input in &parent_inputs {
                        for &output in &parent_outputs {
                            periodic_transfers.push(PeriodicTransferRegion {
                                input,
                                output,
                                aligned: dependency.aligned,
                            });
                        }
                    }
                }
            }
        }
    }
    (regions, periodic_transfers)
}

fn add_inst_output_address_edges(
    inst: &InstDeclaration,
    bit_part: &BitPartition,
    graph: &mut Graph<NodeKey, bool>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    parent_vars: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) {
    for output in &inst.outputs {
        for destination in &output.dst {
            let destination_region = variable_access_region(
                destination.id,
                &destination.index,
                &destination.select,
                parent_vars,
                ctx,
            );
            let address_regions = collect_destination_address_regions(
                std::slice::from_ref(destination),
                parent_vars,
                ctx,
            );
            for address_region in address_regions {
                for source in region_node_keys(address_region, parent_vars, bit_part) {
                    for destination in region_node_keys(destination_region, parent_vars, bit_part) {
                        let source = ensure_node(graph, node_map, source);
                        let destination = ensure_node(graph, node_map, destination);
                        graph.add_edge(source, destination, false);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_inst_feedthrough_edges(
    inst: &InstDeclaration,
    child: &Module,
    summary: &ModuleCombSummary,
    bit_part: &BitPartition,
    graph: &mut Graph<NodeKey, bool>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    parent_vars: &HashMap<VarId, Variable>,
    incomplete: &mut BTreeSet<IncompleteReason>,
    ctx: &mut Context,
) {
    for dependency in &summary.dependencies {
        let Some(child_input) = region_object(dependency.input) else {
            continue;
        };
        let Some(child_output) = summary_output_object(dependency.output) else {
            continue;
        };
        if !is_pure_input_or_output(child_input, &child.variables, Direction::Input)
            || !is_pure_input_or_output(child_output, &child.variables, Direction::Output)
        {
            continue;
        }
        let Some(input) = inst.inputs.iter().find(|input| input.id == child_input) else {
            continue;
        };
        let Some(output) = inst.outputs.iter().find(|output| output.id == child_output) else {
            continue;
        };
        if expression_has_hierarchical_reference(&input.expr) {
            incomplete.insert(IncompleteReason::HierarchicalReference);
        }
        if let SummaryOutput::Periodic(periodic) = dependency.output {
            let parent_inputs =
                map_child_input_region(dependency.input, child, &input.expr, parent_vars, ctx);
            let Some(parent_outputs) =
                map_child_periodic_output(periodic, child, &output.dst, parent_vars, ctx)
            else {
                continue;
            };
            for input_region in parent_inputs {
                for &parent_output in &parent_outputs {
                    let pairs = if dependency.aligned {
                        aligned_periodic_node_pairs(
                            input_region,
                            parent_output,
                            parent_vars,
                            bit_part,
                        )
                    } else {
                        let destinations =
                            periodic_region_node_keys(parent_output, parent_vars, bit_part);
                        region_node_keys(input_region, parent_vars, bit_part)
                            .into_iter()
                            .flat_map(|source| {
                                destinations
                                    .iter()
                                    .copied()
                                    .map(move |destination| (source, destination))
                            })
                            .collect()
                    };
                    for (source, destination) in pairs {
                        let source = ensure_node(graph, node_map, source);
                        let destination = ensure_node(graph, node_map, destination);
                        graph.add_edge(source, destination, dependency.aligned);
                    }
                }
            }
            continue;
        }
        let SummaryOutput::Region(dependency_output) = dependency.output else {
            continue;
        };
        if dependency.aligned
            && let (
                Region::Exact {
                    span: child_input_span,
                    ..
                },
                Region::Exact {
                    span: child_output_span,
                    ..
                },
            ) = (dependency.input, dependency_output)
            && child_input_span.length == child_output_span.length
            && let Some(input_type) = child
                .variables
                .get(&child_input)
                .map(|variable| &variable.r#type)
            && let Some(parent_inputs) = crate::comb_memory_ssa::map_expression_span_positioned_as(
                ctx,
                &input.expr,
                input_type,
                child_input_span,
            )
            && let Some(parent_outputs) =
                map_destinations_span_positioned(&output.dst, child_output_span, parent_vars, ctx)
        {
            for (source, destination, aligned) in aligned_instance_region_pairs(
                &parent_inputs,
                &parent_outputs,
                child_input_span,
                child_output_span,
                parent_vars,
                bit_part,
            ) {
                let source = ensure_node(graph, node_map, source);
                let destination = ensure_node(graph, node_map, destination);
                graph.add_edge(source, destination, aligned);
            }
            continue;
        }
        let parent_input_regions =
            map_child_input_region(dependency.input, child, &input.expr, parent_vars, ctx);
        let parent_output_regions =
            map_child_output_region(dependency_output, child, &output.dst, parent_vars, ctx);
        let pairs = parent_input_regions
            .iter()
            .copied()
            .flat_map(|input_region| {
                let destinations = parent_output_regions
                    .iter()
                    .copied()
                    .flat_map(|output_region| {
                        region_node_keys(output_region, parent_vars, bit_part)
                    })
                    .collect::<Vec<_>>();
                region_node_keys(input_region, parent_vars, bit_part)
                    .into_iter()
                    .flat_map(move |source| {
                        destinations
                            .clone()
                            .into_iter()
                            .map(move |destination| (source, destination))
                    })
            })
            .collect::<Vec<_>>();
        for (source, destination) in pairs {
            let source = ensure_node(graph, node_map, source);
            let destination = ensure_node(graph, node_map, destination);
            graph.add_edge(source, destination, false);
        }
    }
}

fn aligned_instance_region_pairs(
    inputs: &[crate::comb_memory_ssa::PositionedRegion],
    outputs: &[crate::comb_memory_ssa::PositionedRegion],
    child_input: Span,
    child_output: Span,
    parent_vars: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<(NodeKey, NodeKey, bool)> {
    let mut pairs = BTreeSet::new();
    for input in inputs {
        let Some(input_start) = input.expression.start.checked_sub(child_input.start) else {
            continue;
        };
        let input_relative = Span {
            start: input_start,
            length: input.expression.length,
        };
        let Region::Exact {
            object: input_object,
            span: input_region,
        } = input.region
        else {
            continue;
        };
        for output in outputs {
            let Some(output_start) = output.expression.start.checked_sub(child_output.start) else {
                continue;
            };
            let output_relative = Span {
                start: output_start,
                length: output.expression.length,
            };
            let Some(overlap) = input_relative.intersection(output_relative) else {
                continue;
            };
            let Region::Exact {
                object: output_object,
                span: output_region,
            } = output.region
            else {
                continue;
            };
            let Some(output_offset) = overlap.start.checked_sub(output_relative.start) else {
                continue;
            };
            let output_region = Region::Exact {
                object: output_object,
                span: Span {
                    start: output_region.start + output_offset,
                    length: overlap.length,
                },
            };
            if input.aligned {
                let Some(input_offset) = overlap.start.checked_sub(input_relative.start) else {
                    continue;
                };
                let input_region = Region::Exact {
                    object: input_object,
                    span: Span {
                        start: input_region.start + input_offset,
                        length: overlap.length,
                    },
                };
                pairs.extend(
                    aligned_region_node_pairs(input_region, output_region, parent_vars, bit_part)
                        .into_iter()
                        .map(|(source, destination)| (source, destination, true)),
                );
            } else {
                for source in region_node_keys(input.region, parent_vars, bit_part) {
                    for destination in region_node_keys(output_region, parent_vars, bit_part) {
                        pairs.insert((source, destination, false));
                    }
                }
            }
        }
    }
    pairs.into_iter().collect()
}

fn exact_regions_have_equal_length(left: Region<VarId>, right: Region<VarId>) -> bool {
    matches!(
        (left, right),
        (
            Region::Exact { span: left, .. },
            Region::Exact { span: right, .. }
        ) if left.length == right.length
    )
}

fn region_object(region: Region<VarId>) -> Option<VarId> {
    match region {
        Region::Exact { object, .. }
        | Region::UnknownRegion { object, .. }
        | Region::UnknownObject(object) => Some(object),
        Region::UnknownAll => None,
    }
}

fn summary_output_object(output: SummaryOutput) -> Option<VarId> {
    match output {
        SummaryOutput::Region(region) => region_object(region),
        SummaryOutput::Periodic(periodic) => Some(periodic.object),
    }
}

fn child_region_span(region: Region<VarId>, child: &Module) -> Option<(Span, bool)> {
    match region {
        Region::Exact { span, .. } => Some((span, false)),
        Region::UnknownRegion { span, .. } => Some((span, true)),
        Region::UnknownObject(object) => {
            let variable = child.variables.get(&object)?;
            let length = variable
                .total_width()?
                .checked_mul(variable.r#type.total_array().unwrap_or(1))?;
            Some((Span { start: 0, length }, true))
        }
        Region::UnknownAll => None,
    }
}

fn retain_uncertainty(regions: Vec<Region<VarId>>, uncertain: bool) -> Vec<Region<VarId>> {
    if !uncertain {
        return regions;
    }
    regions
        .into_iter()
        .map(|region| match region {
            Region::Exact { object, span } => Region::UnknownRegion { object, span },
            region => region,
        })
        .collect()
}

fn map_child_input_region(
    region: Region<VarId>,
    child: &Module,
    actual: &Expression,
    parent_vars: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Vec<Region<VarId>> {
    let Some((span, uncertain)) = child_region_span(region, child) else {
        return Vec::new();
    };
    let mapped = region_object(region)
        .and_then(|object| child.variables.get(&object))
        .and_then(|variable| {
            crate::comb_memory_ssa::map_expression_span_positioned_as(
                ctx,
                actual,
                &variable.r#type,
                span,
            )
        })
        .map(|positioned| {
            positioned
                .into_iter()
                .map(|positioned| positioned.region)
                .collect()
        })
        .or_else(|| map_expression_span_to_regions(actual, span, parent_vars, ctx))
        .unwrap_or_else(|| collect_expression_regions(actual, parent_vars, ctx));
    retain_uncertainty(mapped, uncertain)
}

fn map_child_output_region(
    region: Region<VarId>,
    child: &Module,
    actual: &[AssignDestination],
    parent_vars: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Vec<Region<VarId>> {
    let Some((span, uncertain)) = child_region_span(region, child) else {
        return Vec::new();
    };
    let mapped = map_destinations_span_to_regions(actual, span, parent_vars, ctx)
        .unwrap_or_else(|| collect_destination_regions(actual, parent_vars, ctx));
    retain_uncertainty(mapped, uncertain)
}

fn map_child_periodic_output(
    periodic: PeriodicRegion,
    child: &Module,
    destinations: &[AssignDestination],
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Option<Vec<PeriodicRegion>> {
    let variable = child.variables.get(&periodic.object)?;
    let length = variable
        .total_width()?
        .checked_mul(variable.r#type.total_array().unwrap_or(1))?;
    let mapped =
        map_destinations_span_positioned(destinations, Span { start: 0, length }, variables, ctx)?;
    if mapped.len() != 1 {
        return None;
    }
    let positioned = mapped[0];
    let Region::Exact {
        object,
        span: destination,
    } = positioned.region
    else {
        return None;
    };
    if !positioned.aligned
        || positioned.expression.start != 0
        || positioned.expression.length != length
        || destination.length != length
    {
        return None;
    }
    Some(vec![PeriodicRegion {
        object,
        output: Span {
            start: destination.start.checked_add(periodic.output.start)?,
            length: periodic.output.length,
        },
        repetitions: periodic.repetitions,
        stride: periodic.stride,
    }])
}

fn expression_has_hierarchical_reference(expression: &Expression) -> bool {
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::HierVariable(_) => true,
            Factor::Variable(_, index, select, _) => {
                index
                    .0
                    .iter()
                    .chain(select.0.iter())
                    .any(expression_has_hierarchical_reference)
                    || select
                        .1
                        .as_ref()
                        .is_some_and(|(_, width)| expression_has_hierarchical_reference(width))
            }
            Factor::SystemFunctionCall(call) => match &call.kind {
                SystemFunctionKind::Bits(input)
                | SystemFunctionKind::Size(input)
                | SystemFunctionKind::Clog2(input)
                | SystemFunctionKind::Onehot(input)
                | SystemFunctionKind::Signed(input)
                | SystemFunctionKind::Unsigned(input) => {
                    expression_has_hierarchical_reference(&input.0)
                }
                SystemFunctionKind::Readmemh(input, _) => {
                    expression_has_hierarchical_reference(&input.0)
                }
                SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => inputs
                    .iter()
                    .any(|input| expression_has_hierarchical_reference(&input.0)),
                SystemFunctionKind::Assert { cond, args, .. } => {
                    expression_has_hierarchical_reference(&cond.0)
                        || args
                            .iter()
                            .any(|input| expression_has_hierarchical_reference(&input.0))
                }
                SystemFunctionKind::Finish => false,
            },
            Factor::FunctionCall(call) => call
                .inputs
                .values()
                .any(expression_has_hierarchical_reference),
            Factor::Value(_) | Factor::Anonymous(_) | Factor::Unknown(_) => false,
        },
        Expression::Unary(_, operand, _) => expression_has_hierarchical_reference(operand),
        Expression::Binary(left, _, right, _) => {
            expression_has_hierarchical_reference(left)
                || expression_has_hierarchical_reference(right)
        }
        Expression::Ternary(condition, left, right, _) => {
            expression_has_hierarchical_reference(condition)
                || expression_has_hierarchical_reference(left)
                || expression_has_hierarchical_reference(right)
        }
        Expression::Concatenation(parts, _) => parts.iter().any(|(part, repeat)| {
            expression_has_hierarchical_reference(part)
                || repeat
                    .as_ref()
                    .is_some_and(expression_has_hierarchical_reference)
        }),
        Expression::ArrayLiteral(items, _) => items.iter().any(|item| match item {
            ArrayLiteralItem::Value(value, repeat) => {
                expression_has_hierarchical_reference(value)
                    || repeat
                        .as_ref()
                        .is_some_and(|repeat| expression_has_hierarchical_reference(repeat))
            }
            ArrayLiteralItem::Defaul(value) => expression_has_hierarchical_reference(value),
        }),
        Expression::StructConstructor(_, fields, _) => fields
            .iter()
            .any(|(_, value)| expression_has_hierarchical_reference(value)),
    }
}

fn map_expression_span_to_regions(
    expression: &Expression,
    requested: Span,
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Option<Vec<Region<VarId>>> {
    let expression_type = &expression.comptime().r#type;
    let expression_length = expression_type
        .total_width()?
        .checked_mul(expression_type.total_array().unwrap_or(1))?;
    if requested.end()? > expression_length {
        return None;
    }
    if let Some(mapped) = crate::comb_memory_ssa::map_expression_span(ctx, expression, requested) {
        return Some(mapped);
    }
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Variable(id, index, select, _) => {
                let Region::Exact { object, span } =
                    exact_variable_region(*id, index, select, variables, ctx)?
                else {
                    return None;
                };
                if requested.end()? > span.length {
                    return None;
                }
                Some(vec![Region::Exact {
                    object,
                    span: Span {
                        start: span.start.checked_add(requested.start)?,
                        length: requested.length,
                    },
                }])
            }
            Factor::Value(_) => Some(Vec::new()),
            Factor::SystemFunctionCall(call) => match &call.kind {
                SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                    map_expression_span_to_regions(&input.0, requested, variables, ctx)
                }
                _ => None,
            },
            _ => None,
        },
        Expression::Concatenation(parts, _) => {
            let mut low = 0usize;
            let mut mapped = Vec::new();
            for (part, repeat) in parts.iter().rev() {
                if repeat.is_some() {
                    return None;
                }
                let width = part.comptime().r#type.total_width()?;
                let part_span = Span {
                    start: low,
                    length: width,
                };
                if let Some(overlap) = requested.intersection(part_span) {
                    mapped.extend(map_expression_span_to_regions(
                        part,
                        Span {
                            start: overlap.start.checked_sub(low)?,
                            length: overlap.length,
                        },
                        variables,
                        ctx,
                    )?);
                }
                low = low.checked_add(width)?;
            }
            Some(mapped)
        }
        Expression::Unary(op, operand, _)
            if matches!(op, Op::BitNot | Op::Add)
                && operand.comptime().r#type.total_width()?
                    == expression.comptime().r#type.total_width()? =>
        {
            map_expression_span_to_regions(operand, requested, variables, ctx)
        }
        Expression::Binary(left, op, right, _)
            if matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor)
                && left.comptime().r#type.total_width()?
                    == expression.comptime().r#type.total_width()?
                && right.comptime().r#type.total_width()?
                    == expression.comptime().r#type.total_width()? =>
        {
            let mut mapped = map_expression_span_to_regions(left, requested, variables, ctx)?;
            mapped.extend(map_expression_span_to_regions(
                right, requested, variables, ctx,
            )?);
            Some(mapped)
        }
        _ => None,
    }
}

fn map_destinations_span_to_regions(
    destinations: &[AssignDestination],
    requested: Span,
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Option<Vec<Region<VarId>>> {
    let mut low = 0usize;
    let mut mapped = Vec::new();
    for destination in destinations.iter().rev() {
        let Region::Exact { object, span } = exact_variable_region(
            destination.id,
            &destination.index,
            &destination.select,
            variables,
            ctx,
        )?
        else {
            return None;
        };
        let destination_span = Span {
            start: low,
            length: span.length,
        };
        if let Some(overlap) = requested.intersection(destination_span) {
            mapped.push(Region::Exact {
                object,
                span: Span {
                    start: span.start.checked_add(overlap.start.checked_sub(low)?)?,
                    length: overlap.length,
                },
            });
        }
        low = low.checked_add(span.length)?;
    }
    (requested.end()? <= low).then_some(mapped)
}

fn map_destinations_span_positioned(
    destinations: &[AssignDestination],
    requested: Span,
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Option<Vec<crate::comb_memory_ssa::PositionedRegion>> {
    let mut low = 0usize;
    let mut mapped = Vec::new();
    for destination in destinations.iter().rev() {
        let Region::Exact { object, span } = exact_variable_region(
            destination.id,
            &destination.index,
            &destination.select,
            variables,
            ctx,
        )?
        else {
            return None;
        };
        let destination_span = Span {
            start: low,
            length: span.length,
        };
        if let Some(overlap) = requested.intersection(destination_span) {
            mapped.push(crate::comb_memory_ssa::PositionedRegion {
                region: Region::Exact {
                    object,
                    span: Span {
                        start: span.start.checked_add(overlap.start.checked_sub(low)?)?,
                        length: overlap.length,
                    },
                },
                expression: overlap,
                aligned: true,
                kind: EdgeKind::Value,
                repetitions: 1,
                expression_stride: 0,
            });
        }
        low = low.checked_add(span.length)?;
    }
    (requested.end()? <= low).then_some(mapped)
}

/// Conservative fallback for expression forms without a positional bit map.
///
/// This is deliberately exhaustive over the analyzer IR. A new expression or
/// factor variant must choose its causal operands here instead of silently
/// turning a legal instance connection into an unsupported procedure.
fn collect_expression_regions(
    expression: &Expression,
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Vec<Region<VarId>> {
    fn collect(
        expression: &Expression,
        variables: &HashMap<VarId, Variable>,
        ctx: &mut Context,
        regions: &mut Vec<Region<VarId>>,
    ) {
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::Variable(id, index, select, _) => {
                    for address in index.0.iter().chain(select.0.iter()) {
                        collect(address, variables, ctx, regions);
                    }
                    if let Some((_, width)) = &select.1 {
                        collect(width, variables, ctx, regions);
                    }
                    regions.push(variable_access_region(*id, index, select, variables, ctx));
                }
                Factor::SystemFunctionCall(call) => match &call.kind {
                    SystemFunctionKind::Bits(_)
                    | SystemFunctionKind::Size(_)
                    | SystemFunctionKind::Clog2(_)
                    | SystemFunctionKind::Finish => {}
                    SystemFunctionKind::Onehot(input)
                    | SystemFunctionKind::Signed(input)
                    | SystemFunctionKind::Unsigned(input) => {
                        collect(&input.0, variables, ctx, regions);
                    }
                    SystemFunctionKind::Readmemh(input, output) => {
                        collect(&input.0, variables, ctx, regions);
                        regions.extend(collect_destination_regions(&output.0, variables, ctx));
                    }
                    SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => {
                        for input in inputs {
                            collect(&input.0, variables, ctx, regions);
                        }
                    }
                    SystemFunctionKind::Assert { cond, args, .. } => {
                        collect(&cond.0, variables, ctx, regions);
                        for input in args {
                            collect(&input.0, variables, ctx, regions);
                        }
                    }
                },
                Factor::FunctionCall(call) => {
                    // A malformed/recursive function which could not produce
                    // a positional summary still retains its syntactic input
                    // dependencies without inventing an unknown object.
                    for input in call.inputs.values() {
                        collect(input, variables, ctx, regions);
                    }
                }
                Factor::HierVariable(_)
                | Factor::Value(_)
                | Factor::Anonymous(_)
                | Factor::Unknown(_) => {}
            },
            Expression::Unary(_, operand, _) => collect(operand, variables, ctx, regions),
            Expression::Binary(left, op, right, _) => {
                collect(left, variables, ctx, regions);
                let rhs_reachable = match op {
                    Op::LogicAnd | Op::LogicOr => {
                        let left_value = left.clone().eval_value(ctx);
                        !matches!(
                            (op, left_value.as_ref()),
                            (Op::LogicAnd, Some(value))
                                if !value.is_xz() && !value.is_positive()
                        ) && !matches!(
                            (op, left_value.as_ref()),
                            (Op::LogicOr, Some(value)) if !value.is_xz() && value.is_positive()
                        )
                    }
                    _ => true,
                };
                if rhs_reachable {
                    collect(right, variables, ctx, regions);
                }
            }
            Expression::Ternary(condition, left, right, _) => {
                collect(condition, variables, ctx, regions);
                match condition.clone().eval_value(ctx) {
                    Some(value) if !value.is_xz() && value.is_positive() => {
                        collect(left, variables, ctx, regions);
                    }
                    Some(value) if !value.is_xz() => collect(right, variables, ctx, regions),
                    _ => {
                        collect(left, variables, ctx, regions);
                        collect(right, variables, ctx, regions);
                    }
                }
            }
            Expression::Concatenation(parts, _) => {
                for (part, repeat) in parts {
                    collect(part, variables, ctx, regions);
                    if let Some(repeat) = repeat {
                        collect(repeat, variables, ctx, regions);
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(value, repeat) => {
                            collect(value, variables, ctx, regions);
                            if let Some(repeat) = repeat {
                                collect(repeat, variables, ctx, regions);
                            }
                        }
                        ArrayLiteralItem::Defaul(value) => {
                            collect(value, variables, ctx, regions);
                        }
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, value) in fields {
                    collect(value, variables, ctx, regions);
                }
            }
        }
    }

    let mut regions = Vec::new();
    collect(expression, variables, ctx, &mut regions);
    regions.sort_unstable();
    regions.dedup();
    regions
}

fn collect_destination_regions(
    destinations: &[AssignDestination],
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Vec<Region<VarId>> {
    let mut regions = destinations
        .iter()
        .map(|destination| {
            variable_access_region(
                destination.id,
                &destination.index,
                &destination.select,
                variables,
                ctx,
            )
        })
        .collect::<Vec<_>>();
    regions.sort_unstable();
    regions.dedup();
    regions
}

fn collect_destination_address_regions(
    destinations: &[AssignDestination],
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Vec<Region<VarId>> {
    let mut regions = Vec::new();
    for destination in destinations {
        for expression in destination
            .index
            .0
            .iter()
            .chain(destination.select.0.iter())
        {
            regions.extend(collect_expression_regions(expression, variables, ctx));
        }
        if let Some((_, expression)) = &destination.select.1 {
            regions.extend(collect_expression_regions(expression, variables, ctx));
        }
    }
    regions.sort_unstable();
    regions.dedup();
    regions
}

fn variable_access_region(
    id: VarId,
    index: &VarIndex,
    select: &VarSelect,
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Region<VarId> {
    exact_variable_region(id, index, select, variables, ctx).unwrap_or(Region::UnknownObject(id))
}

fn exact_variable_region(
    id: VarId,
    index: &VarIndex,
    select: &VarSelect,
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Option<Region<VarId>> {
    if !index.is_const() || !select.is_const_with_range() {
        return None;
    }
    let variable = variables.get(&id)?;
    let width = variable.total_width()?;
    if index.0.is_empty() && select.0.is_empty() && select.1.is_none() {
        return Some(Region::Exact {
            object: id,
            span: Span {
                start: 0,
                length: variable.r#type.total_array()?.checked_mul(width)?,
            },
        });
    }
    let index_path = index.eval_value(ctx)?;
    let array_index = variable.r#type.array.calc_index(&index_path)?;
    let (high, low) = select.eval_value(ctx, &variable.r#type, false)?;
    Some(Region::Exact {
        object: id,
        span: Span {
            start: array_index.checked_mul(width)?.checked_add(low)?,
            length: high.checked_sub(low)?.checked_add(1)?,
        },
    })
}

fn is_pure_input_or_output(id: VarId, vars: &HashMap<VarId, Variable>, want: Direction) -> bool {
    let Some(v) = vars.get(&id) else { return false };
    use crate::ir::VarKind;
    let actual = match v.kind {
        VarKind::Input => Direction::Input,
        VarKind::Output => Direction::Output,
        _ => return false,
    };
    actual == want
}

fn check_graph(module: &Module, graph: &Graph<NodeKey, bool>, errors: &mut Vec<AnalyzerError>) {
    let sccs = strongly_connected_components(graph);
    let mut reported: HashSet<Vec<NodeKey>> = HashSet::default();
    for scc in sccs {
        let is_loop = scc.len() > 1 || (scc.len() == 1 && has_self_edge(graph, scc[0]));
        if !is_loop {
            continue;
        }
        let mut keys: Vec<NodeKey> = scc.iter().map(|n| graph[*n]).collect();
        keys.sort();
        if !reported.insert(keys.clone()) {
            continue;
        }
        if let Some(error) = build_error(module, &keys) {
            errors.push(error);
        }
    }
}

fn strongly_connected_components(graph: &Graph<NodeKey, bool>) -> Vec<Vec<NodeIndex>> {
    let node_bound = graph
        .node_indices()
        .map(NodeIndex::index)
        .max()
        .map_or(0, |index| index + 1);

    // Kosaraju's two passes are both iterative. Combinational dependency
    // chains can be tens of thousands of nodes deep, so even a linear-time
    // recursive SCC implementation can exhaust the compiler's call stack.
    let mut visited = vec![false; node_bound];
    let mut finish_order = Vec::with_capacity(graph.node_count());
    for start in graph.node_indices() {
        if visited[start.index()] {
            continue;
        }

        let mut stack = vec![(start, false)];
        while let Some((node, exiting)) = stack.pop() {
            if exiting {
                finish_order.push(node);
                continue;
            }
            if visited[node.index()] {
                continue;
            }
            visited[node.index()] = true;
            stack.push((node, true));
            for successor in graph.neighbors(node) {
                if !visited[successor.index()] {
                    stack.push((successor, false));
                }
            }
        }
    }

    visited.fill(false);
    let mut components = Vec::new();
    for start in finish_order.into_iter().rev() {
        if visited[start.index()] {
            continue;
        }

        visited[start.index()] = true;
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            component.push(node);
            for predecessor in graph.neighbors_directed(node, Incoming) {
                if !visited[predecessor.index()] {
                    visited[predecessor.index()] = true;
                    stack.push(predecessor);
                }
            }
        }
        components.push(component);
    }
    components
}

fn ensure_node(
    graph: &mut Graph<NodeKey, bool>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    key: NodeKey,
) -> NodeIndex {
    *node_map.entry(key).or_insert_with(|| graph.add_node(key))
}

fn has_self_edge(graph: &Graph<NodeKey, bool>, node: NodeIndex) -> bool {
    graph
        .edges(node)
        .any(|e| e.source() == node && e.target() == node)
}

fn build_error(module: &Module, keys: &[NodeKey]) -> Option<AnalyzerError> {
    let mut tokens: Vec<veryl_parser::token_range::TokenRange> = Vec::new();
    let mut identifier: Option<String> = None;
    let mut seen_var: HashSet<VarId> = HashSet::default();
    for (id, _idx, _range) in keys {
        if !seen_var.insert(*id) {
            continue;
        }
        let variable = module.variables.get(id);
        if let Some(var) = variable
            && identifier.is_none()
        {
            identifier = Some(var.path.to_string());
        }
        if let Some(toks) = module.assign_tokens.get(id)
            && !toks.is_empty()
        {
            tokens.extend(toks.iter().copied());
        } else if let Some(var) = variable {
            // Large dynamic assignments intentionally do not allocate the
            // legacy per-element AssignTable, so no assignment-token vector
            // exists. The declaration still provides a stable diagnostic
            // anchor for a loop proven by the sparse causal graph.
            tokens.push(var.token);
        }
    }
    {
        let mut seen: HashSet<_> = HashSet::default();
        tokens.retain(|t| seen.insert(*t));
    }
    let primary = *tokens.first()?;
    let participants: Vec<_> = tokens.iter().skip(1).copied().collect();
    Some(AnalyzerError::combinational_loop(
        identifier.as_deref().unwrap_or("?"),
        &primary,
        &participants,
    ))
}

fn is_module_scope_var(id: VarId, variables: &HashMap<VarId, Variable>) -> bool {
    match variables.get(&id) {
        Some(v) => matches!(v.affiliation, Affiliation::Module | Affiliation::Interface),
        None => true,
    }
}

fn compute_module_summary(
    module: &Module,
    graph: &Graph<NodeKey, bool>,
    bit_part: &BitPartition,
) -> ModuleCombSummary {
    use crate::ir::VarKind;

    let mut input_ids: HashSet<VarId> = HashSet::default();
    let mut output_ids: HashSet<VarId> = HashSet::default();
    for v in module.variables.values() {
        match v.kind {
            VarKind::Input => {
                input_ids.insert(v.id);
            }
            VarKind::Output => {
                output_ids.insert(v.id);
            }
            _ => {}
        }
    }

    let input_nodes = graph
        .node_indices()
        .filter_map(|node| {
            let key = graph[node];
            input_ids.contains(&key.0).then_some((node, key))
        })
        .collect::<Vec<_>>();
    let output_nodes = graph
        .node_indices()
        .filter_map(|node| {
            let key = graph[node];
            output_ids.contains(&key.0).then_some((node, key))
        })
        .collect::<Vec<_>>();
    if input_nodes.is_empty() || output_nodes.is_empty() {
        return ModuleCombSummary::default();
    }
    let live_nodes = live_summary_nodes(graph, &input_nodes, &output_nodes);
    let input_nodes = input_nodes
        .into_iter()
        .filter(|(node, _)| live_nodes[node.index()])
        .collect::<Vec<_>>();
    let output_nodes = output_nodes
        .into_iter()
        .filter(|(node, _)| live_nodes[node.index()])
        .collect::<Vec<_>>();
    if input_nodes.is_empty() || output_nodes.is_empty() {
        return ModuleCombSummary::default();
    }

    const QUANTUM: usize = 512;
    let winning_walks = if graph.node_count() < QUANTUM {
        let (direction, endpoints) = if input_nodes.len() <= output_nodes.len() {
            (SummaryDirection::Forward, input_nodes)
        } else {
            (SummaryDirection::Reverse, output_nodes)
        };
        let mut walk = SummaryWalk::new(direction, endpoints);
        while !walk.advance(
            graph,
            &live_nodes,
            module,
            bit_part,
            &input_ids,
            &output_ids,
            QUANTUM,
        ) {}
        vec![walk]
    } else {
        #[cfg(not(target_family = "wasm"))]
        let lane_count = rayon::current_num_threads();
        #[cfg(target_family = "wasm")]
        let lane_count = 1usize;
        let make_walks = |direction, endpoints: Vec<(NodeIndex, NodeKey)>| {
            let lanes = lane_count.min(endpoints.len()).max(1);
            let mut partitions = vec![Vec::new(); lanes];
            for (index, endpoint) in endpoints.into_iter().enumerate() {
                partitions[index % lanes].push(endpoint);
            }
            partitions
                .into_iter()
                .map(|endpoints| SummaryWalk::new(direction, endpoints))
                .collect::<Vec<_>>()
        };
        let mut forward = make_walks(SummaryDirection::Forward, input_nodes);
        let mut reverse = make_walks(SummaryDirection::Reverse, output_nodes);
        loop {
            let advance_side = |walks: &mut [SummaryWalk]| {
                #[cfg(not(target_family = "wasm"))]
                let complete = walks
                    .par_iter_mut()
                    .map(|walk| {
                        walk.advance(
                            graph,
                            &live_nodes,
                            module,
                            bit_part,
                            &input_ids,
                            &output_ids,
                            QUANTUM,
                        )
                    })
                    .collect::<Vec<_>>();
                #[cfg(target_family = "wasm")]
                let complete = walks
                    .iter_mut()
                    .map(|walk| {
                        walk.advance(
                            graph,
                            &live_nodes,
                            module,
                            bit_part,
                            &input_ids,
                            &output_ids,
                            QUANTUM,
                        )
                    })
                    .collect::<Vec<_>>();
                complete.into_iter().all(|complete| complete)
            };
            #[cfg(not(target_family = "wasm"))]
            let (forward_complete, reverse_complete) =
                rayon::join(|| advance_side(&mut forward), || advance_side(&mut reverse));
            #[cfg(target_family = "wasm")]
            let (forward_complete, reverse_complete) =
                (advance_side(&mut forward), advance_side(&mut reverse));
            if forward_complete {
                break forward;
            }
            if reverse_complete {
                break reverse;
            }
        }
    };
    let mut dependencies = BTreeMap::new();
    for walk in winning_walks {
        for (pair, aligned) in walk.dependencies {
            dependencies
                .entry(pair)
                .and_modify(|existing| *existing &= aligned)
                .or_insert(aligned);
        }
    }
    ModuleCombSummary {
        dependencies: dependencies
            .into_iter()
            .map(|((input, output), aligned)| ModuleCombDependency {
                input,
                output,
                aligned,
            })
            .collect(),
    }
}

fn node_key_regions(key: NodeKey, module: &Module, bit_part: &BitPartition) -> Vec<Region<VarId>> {
    node_key_regions_for_variables(key, &module.variables, bit_part)
}

fn node_key_regions_for_variables(
    key: NodeKey,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<Region<VarId>> {
    if key.1 == PERIODIC_REGION_INDEX {
        return Vec::new();
    }
    if key.1 == UNKNOWN_REGION_INDEX {
        return bit_part.wildcards.get(key.2).copied().into_iter().collect();
    }
    if key.1 == DENSE_REGION_INDEX {
        return bit_part
            .dense_regions
            .get(key.2)
            .copied()
            .into_iter()
            .collect();
    }
    let Some(width) = variables.get(&key.0).and_then(Variable::total_width) else {
        return Vec::new();
    };
    let Some(span) = bit_part.ranges_of((key.0, key.1)).get(key.2) else {
        return Vec::new();
    };
    let Some(start) = key
        .1
        .checked_mul(width)
        .and_then(|base| base.checked_add(span.start))
    else {
        return Vec::new();
    };
    vec![Region::Exact {
        object: key.0,
        span: Span {
            start,
            length: span.length,
        },
    }]
}

fn node_key_summary_outputs(
    key: NodeKey,
    module: &Module,
    bit_part: &BitPartition,
) -> Vec<SummaryOutput> {
    if key.1 == PERIODIC_REGION_INDEX {
        return bit_part
            .periodic_regions
            .get(key.2)
            .copied()
            .map(SummaryOutput::Periodic)
            .into_iter()
            .collect();
    }
    node_key_regions(key, module, bit_part)
        .into_iter()
        .map(SummaryOutput::Region)
        .collect()
}

#[cfg(test)]
mod memory_ssa_tests {
    use super::*;

    #[test]
    fn recursive_module_does_not_taint_independent_modules() {
        // Why this case exists: one recursive hierarchy makes only its SCC and
        // transitive parents incomplete. Marking an independent hierarchy as
        // RecursiveCall needlessly discards a valid bottom-up summary.
        fn set(values: &[usize]) -> HashSet<usize> {
            values.iter().copied().collect()
        }

        let is_module = [true, true, true, true];
        let deps = [set(&[0]), HashSet::default(), set(&[0]), set(&[1])];
        let rev_deps = [
            set(&[0, 2]),
            set(&[3]),
            HashSet::default(),
            HashSet::default(),
        ];

        let (order, affected) = order_from_dependencies(&is_module, &deps, &rev_deps);
        assert_eq!(order, vec![1, 3, 0, 2]);
        assert_eq!(affected, set(&[0, 2]));
    }

    #[test]
    fn scc_handles_a_deep_valid_chain_without_recursion() {
        // Why this case exists: a valid 25,600-node acyclic dependency chain
        // overflowed the process stack in petgraph's recursive Tarjan DFS.
        // SCC discovery must use heap-backed worklists regardless of graph
        // depth; every node in this chain is its own component.
        let count = 25_600usize;
        let mut graph = Graph::<NodeKey, bool>::new();
        let nodes = (0..count)
            .map(|index| graph.add_node((VarId::from_raw(index as u32), 0, 0)))
            .collect::<Vec<_>>();
        for pair in nodes.windows(2) {
            graph.add_edge(pair[0], pair[1], false);
        }

        let components = strongly_connected_components(&graph);
        assert_eq!(components.len(), count);
        assert!(components.iter().all(|component| component.len() == 1));
    }

    #[test]
    fn atomic_ranges_split_at_sparse_mask_transitions() {
        // Why this case exists: sparse endpoints on a huge packed object must
        // cost O(accesses), not O(declared width).
        let width = 1_000_000;
        let low = Span {
            start: 8,
            length: 8,
        };
        let high = Span {
            start: width - 16,
            length: 8,
        };
        assert_eq!(atomic_ranges(&[low, high], width), vec![low, high]);
    }

    #[test]
    fn atomic_ranges_preserve_shared_and_disjoint_signatures() {
        // Why this case exists: an event sweep may use a finer partition than
        // signature merging, but it must split at both overlap boundaries.
        let first = Span {
            start: 0,
            length: 128,
        };
        let second = Span {
            start: 64,
            length: 128,
        };
        let ranges = atomic_ranges(&[first, second], 192);
        assert_eq!(
            ranges,
            vec![
                Span {
                    start: 0,
                    length: 64,
                },
                Span {
                    start: 64,
                    length: 64,
                },
                Span {
                    start: 128,
                    length: 64,
                },
            ]
        );
    }

    #[test]
    fn atomic_ranges_keep_adjacent_signature_transitions() {
        // Why this case exists: two masks ending and starting at the same bit
        // leave coverage nonzero, but still require distinct causal atoms.
        assert_eq!(
            atomic_ranges(
                &[
                    Span {
                        start: 0,
                        length: 64,
                    },
                    Span {
                        start: 64,
                        length: 64,
                    },
                ],
                128,
            ),
            vec![
                Span {
                    start: 0,
                    length: 64,
                },
                Span {
                    start: 64,
                    length: 64,
                },
            ]
        );
    }
}
