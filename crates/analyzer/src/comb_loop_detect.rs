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
use crate::BigUint;
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    ArrayLiteralItem, AssignDestination, Component, Declaration, Expression, Factor,
    InstDeclaration, Ir, Module, Op, SystemFunctionKind, VarIndex, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction};
use crate::value::ValueBigUint;
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;
#[cfg(not(target_family = "wasm"))]
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use veryl_causal::graph::{EdgeKind, IncompleteReason};
use veryl_causal::procedure::ProcedureSummary;
use veryl_causal::region::{Region, Span};
use veryl_parser::resource_table::StrId;

/// One concrete unpacked-array element. Bit-precision lives in masks.
type IdxKey = (VarId, usize);

/// `(VarId, array_idx, range_idx)`. `range_idx` indexes the variable's
/// `BitPartition`, so bit-disjoint reads/writes form disjoint nodes.
type NodeKey = (VarId, usize, usize);

/// Sparse alias node for a dynamically selected region of one object.  This
/// deliberately occupies no real array element or bit-partition slot.
const UNKNOWN_REGION_INDEX: usize = usize::MAX;

/// Per `IdxKey`, atomic bit-range masks. Two bits are in the same range
/// iff they appear in the same set of per-decl masks.
#[derive(Default)]
struct BitPartition {
    ranges: HashMap<IdxKey, Vec<BigUint>>,
}

impl BitPartition {
    /// Empty slice means the variable's bits are untouched.
    fn ranges_of(&self, key: IdxKey) -> &[BigUint] {
        self.ranges.get(&key).map(|v| v.as_slice()).unwrap_or(&[])
    }

    fn overlapping(&self, key: IdxKey, mask: &BigUint) -> Vec<usize> {
        let zero = BigUint::default();
        self.ranges_of(key)
            .iter()
            .enumerate()
            .filter(|(_, m)| (*m & mask) != zero)
            .map(|(i, _)| i)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModuleCombDependency {
    input: Region<VarId>,
    output: Region<VarId>,
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
    let mut summaries: HashMap<StrId, ModuleCombSummary> = HashMap::default();

    let (order, recursive_modules) = topo_order_modules(ir);

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
            let (graph, mut reasons, bit_part) = build_module_graph(module, &summaries);
            if recursive_modules {
                reasons.insert(IncompleteReason::RecursiveCall);
            }
            check_graph(module, &graph, &mut errors);
            let summary = compute_module_summary(module, &graph, &bit_part);
            summaries.insert(module.name, summary);
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

/// Children before parents. Falls back to input order on cycle
/// (`infinite_recursion` is reported separately).
fn topo_order_modules(ir: &Ir) -> (Vec<usize>, bool) {
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
                    && child_idx != i
                {
                    deps[i].insert(child_idx);
                    rev_deps[child_idx].insert(i);
                }
            }
        }
    }

    let mut indeg: Vec<usize> = deps.iter().map(|s| s.len()).collect();
    let mut q: VecDeque<usize> = VecDeque::new();
    for (i, _) in indeg.iter().enumerate().take(n) {
        if matches!(ir.components.get(i), Some(Component::Module(_))) && indeg[i] == 0 {
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
    if order.len()
        != ir
            .components
            .iter()
            .filter(|c| matches!(c, Component::Module(_)))
            .count()
    {
        // Cycle in module graph -- emit imprecise reports anyway.
        return (
            (0..n)
                .filter(|i| matches!(ir.components.get(*i), Some(Component::Module(_))))
                .collect(),
            true,
        );
    }
    (order, false)
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
fn atomic_ranges(masks: &[BigUint], width: usize) -> Vec<BigUint> {
    // Split only at mask transitions. The previous implementation visited
    // every declared bit and built a signature BigUint for it, making a sparse
    // access to a million-bit object take O(width * accesses). Here the work is
    // O(mask limbs + transitions * accesses), independent of untouched spans.
    let mut endpoints = BTreeSet::new();
    endpoints.insert(0usize);
    endpoints.insert(width);
    for mask in masks {
        let digits = mask.iter_u64_digits().collect::<Vec<_>>();
        let mut previous_bit = false;
        for word_index in 0..=digits.len() {
            let word = digits.get(word_index).copied().unwrap_or(0);
            let shifted = word.wrapping_shl(1) | u64::from(previous_bit);
            let mut transitions = word ^ shifted;
            while transitions != 0 {
                let bit = transitions.trailing_zeros() as usize;
                let endpoint = word_index.saturating_mul(64).saturating_add(bit);
                if endpoint <= width {
                    endpoints.insert(endpoint);
                }
                transitions &= transitions - 1;
            }
            previous_bit = word >> 63 != 0;
        }
    }

    let mut by_sig: HashMap<BigUint, BigUint> = HashMap::default();
    let one = BigUint::from(1u32);
    let endpoints = endpoints.into_iter().collect::<Vec<_>>();
    for window in endpoints.windows(2) {
        let start = window[0];
        let end = window[1];
        if start >= end {
            continue;
        }
        let mut sig = BigUint::default();
        for (i, m) in masks.iter().enumerate() {
            if m.bit(start as u64) {
                sig |= &one << i;
            }
        }
        if sig == BigUint::default() {
            continue;
        }
        let entry = by_sig.entry(sig).or_default();
        *entry |= ValueBigUint::gen_mask_range(end - 1, start);
    }
    let mut ret: Vec<BigUint> = by_sig.into_values().collect();
    // Stable order by lowest set bit so NodeKey range_idx is deterministic.
    ret.sort_by_key(|m| m.trailing_zeros().unwrap_or(0));
    ret
}

fn build_bit_partition(module: &Module, memory_ssa_regions: &[Region<VarId>]) -> BitPartition {
    let mut masks: HashMap<(VarId, usize), Vec<BigUint>> = HashMap::default();

    // A shifted copy can propagate every observed endpoint through the copy
    // relation. Feed those sparse spans into the shared partition so distinct
    // causal atoms do not collapse into a spurious self-edge.
    for &region in memory_ssa_regions {
        collect_region_mask(region, module, &mut masks);
    }

    let mut ranges: HashMap<(VarId, usize), Vec<BigUint>> = HashMap::default();
    for (key, ms) in masks {
        let width = module
            .variables
            .get(&key.0)
            .and_then(|v| v.total_width())
            .unwrap_or(1);
        let parts = atomic_ranges(&ms, width);
        if !parts.is_empty() {
            ranges.insert(key, parts);
        }
    }

    BitPartition { ranges }
}

fn collect_region_mask(
    region: Region<VarId>,
    module: &Module,
    masks: &mut HashMap<(VarId, usize), Vec<BigUint>>,
) {
    let Region::Exact { object, span } = region else {
        return;
    };
    let Some(width) = module
        .variables
        .get(&object)
        .and_then(Variable::total_width)
    else {
        return;
    };
    let Some(end) = span.end() else {
        return;
    };
    let mut cursor = span.start;
    while cursor < end {
        let element = cursor / width;
        let bit_start = cursor % width;
        let element_end = end.min((element + 1).saturating_mul(width));
        let bit_end = element_end - element * width;
        masks
            .entry((object, element))
            .or_default()
            .push(ValueBigUint::gen_mask_range(bit_end - 1, bit_start));
        cursor = element_end;
    }
}

fn build_module_graph(
    module: &Module,
    summaries: &HashMap<StrId, ModuleCombSummary>,
) -> (Graph<NodeKey, ()>, BTreeSet<IncompleteReason>, BitPartition) {
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
    let procedure_summaries = module
        .declarations
        .par_iter()
        .filter_map(analyze_declaration)
        .collect::<Vec<_>>();

    let mut partition_regions = procedure_summaries
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .flat_map(|summary| {
            summary.dependencies.iter().flat_map(|dependency| {
                [
                    dependency.input,
                    sparse_output_region(summary, dependency.output),
                ]
            })
        })
        .collect::<Vec<_>>();
    partition_regions = propagate_aligned_regions(partition_regions, &procedure_summaries);
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.functions = module.functions.clone();
    partition_regions.extend(collect_instance_summary_regions(
        module, summaries, &mut ctx,
    ));
    let bit_part = build_bit_partition(module, &partition_regions);

    let mut graph: Graph<NodeKey, ()> = Graph::new();
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
    #[cfg(target_family = "wasm")]
    let procedure_summaries = module
        .declarations
        .iter()
        .filter_map(analyze_declaration)
        .collect::<Vec<_>>();
    for result in &procedure_summaries {
        match result {
            Ok(summary) => {
                incomplete.extend(summary.incomplete.iter().copied());
                add_memory_ssa_edges(module, summary, &bit_part, &mut graph, &mut node_map);
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
                let Some(summary) = summaries.get(&child.name) else {
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
    let transfers = summaries
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
    let mut known = regions.into_iter().collect::<BTreeSet<_>>();
    let mut pending = known
        .iter()
        .copied()
        .collect::<std::collections::VecDeque<_>>();
    while let Some(region) = pending.pop_front() {
        let Region::Exact { object, span } = region else {
            continue;
        };
        for &(left, right) in &transfers {
            for (from, to) in [(left, right), (right, left)] {
                if object != from.0 {
                    continue;
                }
                let Some(overlap) = span.intersection(from.1) else {
                    continue;
                };
                let Some(offset) = overlap.start.checked_sub(from.1.start) else {
                    continue;
                };
                let Some(start) = to.1.start.checked_add(offset) else {
                    continue;
                };
                let mapped = Region::Exact {
                    object: to.0,
                    span: Span {
                        start,
                        length: overlap.length,
                    },
                };
                if known.insert(mapped) {
                    pending.push_back(mapped);
                }
            }
        }
    }
    known.into_iter().collect()
}

fn add_memory_ssa_edges(
    module: &Module,
    summary: &ProcedureSummary<VarId>,
    bit_part: &BitPartition,
    graph: &mut Graph<NodeKey, ()>,
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
        let output = sparse_output_region(summary, dependency.output);
        let pairs = if dependency.aligned {
            aligned_region_node_pairs(dependency.input, output, &module.variables, bit_part)
        } else {
            let sources = region_node_keys(dependency.input, &module.variables, bit_part);
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
            graph.add_edge(source, destination, ());
        }
    }
}

fn aligned_region_node_pairs(
    input: Region<VarId>,
    output: Region<VarId>,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<(NodeKey, NodeKey)> {
    let (
        Region::Exact {
            object: input_object,
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
    let Some(input_width) = variables.get(&input_object).and_then(Variable::total_width) else {
        return Vec::new();
    };
    let mut pairs = BTreeSet::new();
    for source in region_node_keys(input, variables, bit_part) {
        let Some(mask) = bit_part.ranges_of((source.0, source.1)).get(source.2) else {
            continue;
        };
        for local in mask_spans(mask, input_width) {
            let Some(global_start) = source
                .1
                .checked_mul(input_width)
                .and_then(|base| base.checked_add(local.start))
            else {
                continue;
            };
            let Some(overlap) = (Span {
                start: global_start,
                length: local.length,
            })
            .intersection(input_span) else {
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

/// Memory SSA expands an unknown-object write over its sparse atom endpoints
/// so kill/phi semantics stay exact. Do not turn the resulting large exact
/// output spans back into per-element graph nodes: the object-local alias node
/// carries the same uncertainty and aliases only regions touched elsewhere.
fn sparse_output_region(summary: &ProcedureSummary<VarId>, output: Region<VarId>) -> Region<VarId> {
    match output {
        Region::Exact { object, .. } if summary.uncertain_write_objects.contains(&object) => {
            Region::UnknownObject(object)
        }
        output => output,
    }
}

/// Convert the MemorySSA engine's flattened, half-open bit region back to the
/// graph `(array element, bit partition)` coordinates. The loop is
/// proportional to touched elements, never to the declared array size.
fn region_node_keys(
    region: Region<VarId>,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<NodeKey> {
    let (object, span) = match region {
        Region::Exact { object, span } => (object, span),
        Region::UnknownObject(object) => {
            // Alias every region of this object which is otherwise observable
            // in the sparse partition. This is proportional to touched
            // regions, not to the declared array size.
            let mut keys = bit_part
                .ranges
                .keys()
                .filter(|(candidate, _)| *candidate == object)
                .flat_map(|&(candidate, element)| {
                    (0..bit_part.ranges_of((candidate, element)).len())
                        .map(move |range| (candidate, element, range))
                })
                .collect::<Vec<_>>();
            keys.push((object, UNKNOWN_REGION_INDEX, UNKNOWN_REGION_INDEX));
            keys.sort_unstable();
            keys.dedup();
            return keys;
        }
        Region::UnknownAll => return Vec::new(),
    };
    let Some(width) = variables.get(&object).and_then(Variable::total_width) else {
        return Vec::new();
    };
    let Some(end) = span.end() else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    let mut cursor = span.start;
    while cursor < end {
        let element = cursor / width;
        let bit_start = cursor % width;
        let element_end = end.min((element + 1).saturating_mul(width));
        let bit_end = element_end - element * width;
        let mask = ValueBigUint::gen_mask_range(bit_end - 1, bit_start);
        keys.extend(
            bit_part
                .overlapping((object, element), &mask)
                .into_iter()
                .map(|range| (object, element, range)),
        );
        cursor = element_end;
    }
    keys
}

fn collect_instance_summary_regions(
    module: &Module,
    summaries: &HashMap<StrId, ModuleCombSummary>,
    ctx: &mut Context,
) -> Vec<Region<VarId>> {
    let mut regions = Vec::new();
    for inst in walk_insts(module) {
        let Component::Module(child) = inst.component.as_ref() else {
            continue;
        };
        let Some(summary) = summaries.get(&child.name) else {
            continue;
        };
        for dependency in &summary.dependencies {
            let Region::Exact {
                object: child_input,
                span: input_span,
            } = dependency.input
            else {
                continue;
            };
            let Region::Exact {
                object: child_output,
                span: output_span,
            } = dependency.output
            else {
                continue;
            };
            let Some(input) = inst.inputs.iter().find(|input| input.id == child_input) else {
                continue;
            };
            let Some(output) = inst.outputs.iter().find(|output| output.id == child_output) else {
                continue;
            };
            regions.extend(
                map_expression_span_to_regions(&input.expr, input_span, &module.variables, ctx)
                    .unwrap_or_else(|| {
                        collect_expression_regions(&input.expr, &module.variables, ctx)
                    }),
            );
            regions.extend(
                map_destinations_span_to_regions(&output.dst, output_span, &module.variables, ctx)
                    .unwrap_or_else(|| {
                        collect_destination_regions(&output.dst, &module.variables, ctx)
                    }),
            );
        }
    }
    regions
}

#[allow(clippy::too_many_arguments)]
fn add_inst_feedthrough_edges(
    inst: &InstDeclaration,
    child: &Module,
    summary: &ModuleCombSummary,
    bit_part: &BitPartition,
    graph: &mut Graph<NodeKey, ()>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    parent_vars: &HashMap<VarId, Variable>,
    incomplete: &mut BTreeSet<IncompleteReason>,
    ctx: &mut Context,
) {
    for dependency in &summary.dependencies {
        let Region::Exact {
            object: child_input,
            span: input_span,
        } = dependency.input
        else {
            continue;
        };
        let Region::Exact {
            object: child_output,
            span: output_span,
        } = dependency.output
        else {
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
        let parent_input_regions =
            map_expression_span_to_regions(&input.expr, input_span, parent_vars, ctx)
                .unwrap_or_else(|| collect_expression_regions(&input.expr, parent_vars, ctx));
        let parent_output_regions =
            map_destinations_span_to_regions(&output.dst, output_span, parent_vars, ctx)
                .unwrap_or_else(|| collect_destination_regions(&output.dst, parent_vars, ctx));
        for input_region in parent_input_regions {
            for source in region_node_keys(input_region, parent_vars, bit_part) {
                for &output_region in &parent_output_regions {
                    for destination in region_node_keys(output_region, parent_vars, bit_part) {
                        let source = ensure_node(graph, node_map, source);
                        let destination = ensure_node(graph, node_map, destination);
                        graph.add_edge(source, destination, ());
                    }
                }
            }
        }
    }
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
    if requested.end()? > expression.comptime().r#type.total_width()? {
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
            Expression::Binary(left, _, right, _) => {
                collect(left, variables, ctx, regions);
                collect(right, variables, ctx, regions);
            }
            Expression::Ternary(condition, left, right, _) => {
                collect(condition, variables, ctx, regions);
                collect(left, variables, ctx, regions);
                collect(right, variables, ctx, regions);
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

fn check_graph(module: &Module, graph: &Graph<NodeKey, ()>, errors: &mut Vec<AnalyzerError>) {
    let sccs = tarjan_scc(graph);
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

fn ensure_node(
    graph: &mut Graph<NodeKey, ()>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    key: NodeKey,
) -> NodeIndex {
    *node_map.entry(key).or_insert_with(|| graph.add_node(key))
}

fn has_self_edge(graph: &Graph<NodeKey, ()>, node: NodeIndex) -> bool {
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
    graph: &Graph<NodeKey, ()>,
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

    let mut dependencies = BTreeSet::new();
    let mut visited: HashSet<NodeIndex> = HashSet::default();
    let mut stack: Vec<NodeIndex> = Vec::new();
    for ni in graph.node_indices() {
        let key = graph[ni];
        if !input_ids.contains(&key.0) {
            continue;
        }
        visited.clear();
        stack.clear();
        stack.push(ni);
        while let Some(n) = stack.pop() {
            if !visited.insert(n) {
                continue;
            }
            let nk = graph[n];
            if output_ids.contains(&nk.0) {
                for input in node_key_regions(key, module, bit_part) {
                    for output in node_key_regions(nk, module, bit_part) {
                        dependencies.insert(ModuleCombDependency { input, output });
                    }
                }
            }
            for e in graph.edges(n) {
                stack.push(e.target());
            }
        }
    }
    ModuleCombSummary {
        dependencies: dependencies.into_iter().collect(),
    }
}

fn node_key_regions(key: NodeKey, module: &Module, bit_part: &BitPartition) -> Vec<Region<VarId>> {
    if key.1 == UNKNOWN_REGION_INDEX || key.2 == UNKNOWN_REGION_INDEX {
        return Vec::new();
    }
    let Some(width) = module.variables.get(&key.0).and_then(Variable::total_width) else {
        return Vec::new();
    };
    let Some(mask) = bit_part.ranges_of((key.0, key.1)).get(key.2) else {
        return Vec::new();
    };
    mask_spans(mask, width)
        .into_iter()
        .filter_map(|span| {
            let start = key.1.checked_mul(width)?.checked_add(span.start)?;
            Some(Region::Exact {
                object: key.0,
                span: Span {
                    start,
                    length: span.length,
                },
            })
        })
        .collect()
}

fn mask_spans(mask: &BigUint, width: usize) -> Vec<Span> {
    let digits = mask.iter_u64_digits().collect::<Vec<_>>();
    let mut spans = Vec::new();
    let mut start = None;
    for word_index in 0..=digits.len() {
        let word = digits.get(word_index).copied().unwrap_or(0);
        let previous_bit = word_index
            .checked_sub(1)
            .and_then(|index| digits.get(index))
            .is_some_and(|word| word >> 63 != 0);
        let mut transitions = word ^ (word.wrapping_shl(1) | u64::from(previous_bit));
        while transitions != 0 {
            let bit = transitions.trailing_zeros() as usize;
            let point = word_index.saturating_mul(64).saturating_add(bit).min(width);
            if mask.bit(point as u64) {
                start = Some(point);
            } else if let Some(span_start) = start.take()
                && span_start < point
            {
                spans.push(Span {
                    start: span_start,
                    length: point - span_start,
                });
            }
            transitions &= transitions - 1;
        }
    }
    if let Some(span_start) = start
        && span_start < width
    {
        spans.push(Span {
            start: span_start,
            length: width - span_start,
        });
    }
    spans
}

#[cfg(test)]
mod memory_ssa_tests {
    use super::*;

    #[test]
    fn atomic_ranges_split_at_sparse_mask_transitions() {
        let width = 1_000_000;
        let low = ValueBigUint::gen_mask_range(15, 8);
        let high = ValueBigUint::gen_mask_range(width - 9, width - 16);
        let ranges = atomic_ranges(&[low.clone(), high.clone()], width);
        assert_eq!(ranges.len(), 2);
        assert!(ranges.contains(&low));
        assert!(ranges.contains(&high));
    }

    #[test]
    fn atomic_ranges_preserve_shared_and_disjoint_signatures() {
        let first = ValueBigUint::gen_mask_range(127, 0);
        let second = ValueBigUint::gen_mask_range(191, 64);
        let ranges = atomic_ranges(&[first, second], 192);
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0], ValueBigUint::gen_mask_range(63, 0));
        assert_eq!(ranges[1], ValueBigUint::gen_mask_range(127, 64));
        assert_eq!(ranges[2], ValueBigUint::gen_mask_range(191, 128));
    }

    #[test]
    fn module_summary_masks_split_across_word_boundaries() {
        let mask = ValueBigUint::gen_mask_range(1, 0)
            | ValueBigUint::gen_mask_range(65, 63)
            | ValueBigUint::gen_mask_range(127, 127);
        assert_eq!(
            mask_spans(&mask, 192),
            vec![
                Span {
                    start: 0,
                    length: 2,
                },
                Span {
                    start: 63,
                    length: 3,
                },
                Span {
                    start: 127,
                    length: 1,
                },
            ]
        );
        assert!(mask_spans(&BigUint::default(), 192).is_empty());
    }
}
