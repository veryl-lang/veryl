//! Combinational loop detection on the analyzer IR (issue #931).
//!
//! The analysis pipeline is split by responsibility:
//!
//! 1. discover sparse bit and array regions used by each module;
//! 2. evaluate procedures in statement order and build dependency edges;
//! 3. detect compatible cycles in the graph;
//! 4. summarize module feedthrough bottom-up for parent instances.
//!
//! Under-detect by design: opaque constructs (SystemVerilog black
//! boxes, `inout` ports, recursive functions) add no edges; the
//! simulator's `analyze_dependency` is the backup safety net.

mod diagnostics;
mod graph;
mod hierarchy;
mod model;
mod procedure;
mod region;
mod ssa;
mod steps;
mod summary;

#[cfg(test)]
pub(crate) use procedure::{
    function_barrier_evaluation_count, function_evaluation_count,
    function_result_region_probe_count, function_result_version_count,
    function_summary_graph_edge_count, function_summary_graph_node_count, module_context_entries,
    reset_function_evaluation_count, reset_module_context_entries,
    reset_traced_procedure_evaluation_count, reset_visible_source_probes,
    traced_procedure_evaluation_count, visible_source_probes, write_footprint_statement_visits,
};

use diagnostics::{DiagnosticReplayCache, TraceKind, check_graph};
#[cfg(test)]
pub(crate) use diagnostics::{
    diagnostic_instance_probe_count, diagnostic_provenance_build_count, diagnostic_replay_count,
    reset_diagnostic_instance_probe_count, reset_diagnostic_provenance_build_count,
    reset_diagnostic_replay_count,
};
use graph::{
    DependencyGraph, GraphDependency, GraphNode, add_dependency_edge, add_region_dependency,
    ensure_node, regions_overlap_with_dependency,
};
#[cfg(test)]
pub(crate) use graph::{
    cycle_decision_work, cycle_search_work, reset_cycle_decision_work, reset_cycle_search_work,
};
use hierarchy::module_postorder;
use model::{BitDependency, ModuleCombSummary, SummaryNodeKind, SummaryRegion};
use region::{
    ArraySpan, BitPartition, IdxKey, NodeKey, PackedSpan, dst_writes, signed_difference,
    translate_position, var_reads,
};
use ssa::{BranchId, DependencyDagNode, PathCondition, PositionDomain};
#[cfg(test)]
pub(crate) use ssa::{
    import_binding_visits, reset_import_binding_visits, reset_source_walk_visits,
    source_walk_visits,
};
use steps::Steps;
#[cfg(test)]
pub(crate) use steps::{FIRST_ALLOWANCE, reset_steps_taken, steps_taken, with_step_limit};
use summary::{ExpansionBudget, compute_module_summary};
#[cfg(test)]
pub(crate) use summary::{module_summary_work, reset_module_summary_work};

use crate::AnalyzerError;
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    AssignDestination, Component, Declaration, Expression, Factor, InstDeclaration, Ir,
    MemberSelectDomain, Module, Op, Signature, SystemFunctionKind, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction};
use daggy::petgraph::graph::NodeIndex;

#[cfg(test)]
thread_local! {
    static ANALYSIS_SIZE: std::cell::Cell<(usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0)) };
}

#[cfg(test)]
pub(crate) fn reset_analysis_size() {
    ANALYSIS_SIZE.set((0, 0, 0));
}

#[cfg(test)]
pub(crate) fn analysis_size() -> (usize, usize, usize) {
    ANALYSIS_SIZE.get()
}

pub fn check(ir: &Ir) -> Vec<AnalyzerError> {
    check_inner(ir).0
}

#[cfg(test)]
pub(crate) fn is_complete(ir: &Ir) -> bool {
    check_inner(ir).1
}

fn check_inner(ir: &Ir) -> (Vec<AnalyzerError>, bool) {
    let mut errors = Vec::new();
    let mut complete = true;
    let mut summaries: HashMap<Signature, ModuleCombSummary> = HashMap::default();

    let mut diagnostic_replays = DiagnosticReplayCache::default();
    let mut reported = HashSet::default();
    for module in module_postorder(ir) {
        let steps = Steps::new();
        let (graph, module_complete) = match build_module_graph(module, &summaries, &steps) {
            Ok(result) => result,
            Err(error) => {
                errors.push(*error);
                complete = false;
                summaries.insert(module.signature.clone(), ModuleCombSummary::default());
                continue;
            }
        };
        let cycles_complete = check_graph(
            module,
            &graph,
            &steps,
            &summaries,
            &mut diagnostic_replays,
            &mut errors,
            &mut reported,
        );
        let mut summary = compute_module_summary(module, &graph);
        summary.complete = module_complete && cycles_complete;
        summaries.insert(module.signature.clone(), summary);
        complete &= module_complete && cycles_complete;
    }

    (errors, complete)
}

/// One storage region per variable: its whole declared extent. Procedure SSA
/// records the region of every definition and relations carry positions, so
/// accesses need not cut storage; a node is created only when it is used.
fn build_bit_partition(module: &Module, ctx: &Context) -> BitPartition {
    let mut ranges: HashMap<IdxKey, Vec<PackedSpan>> = HashMap::default();
    let mut add = |id: VarId, r#type: &crate::ir::Type| {
        let Some(array_length) = r#type.array.total() else {
            return;
        };
        let Some(packed) = r#type.total_width().and_then(PackedSpan::whole) else {
            return;
        };
        let array = ArraySpan {
            start: 0,
            length: array_length,
        };
        ranges.entry((id, array)).or_insert_with(|| vec![packed]);
    };
    // Parameters and constants are never driven, so they need no storage.
    for (id, variable) in &ctx.variables {
        if !matches!(
            variable.kind,
            crate::ir::VarKind::Param | crate::ir::VarKind::Const
        ) {
            add(*id, &variable.r#type);
        }
    }
    // Function arguments and results without a module variable are lowered
    // into the caller's SSA and need storage of their declared type.
    for function in module.functions.values() {
        for body in &function.functions {
            for (path, id) in &body.arg_map {
                if ctx.variables.contains_key(id) {
                    continue;
                }
                let r#type = function
                    .args
                    .iter()
                    .flat_map(|argument| &argument.members)
                    .find_map(|(member, comptime, _)| (member == path).then_some(&comptime.r#type));
                if let Some(r#type) = r#type {
                    add(*id, r#type);
                }
            }
            if let Some(id) = body.ret
                && !ctx.variables.contains_key(&id)
            {
                add(id, &function.r#type.r#type);
            }
        }
    }

    #[cfg(test)]
    {
        let (atoms, nodes, edges) = ANALYSIS_SIZE.get();
        ANALYSIS_SIZE.set((
            atoms + ranges.values().map(Vec::len).sum::<usize>(),
            nodes,
            edges,
        ));
    }
    BitPartition::new(ranges)
}

fn build_module_graph(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    steps: &Steps,
) -> Result<(DependencyGraph, bool), Box<AnalyzerError>> {
    build_module_graph_with_trace(module, summaries, TraceKind::None, steps)
}

fn build_module_graph_with_trace(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    tracing: TraceKind,
    steps: &Steps,
) -> Result<(DependencyGraph, bool), Box<AnalyzerError>> {
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.variables.extend(module.interface_members.clone());
    ctx.functions = module.functions.clone();
    let limit = isize::MAX as usize;
    let oversized = module
        .variables
        .values()
        .chain(module.interface_members.values())
        .find(|variable| {
            variable
                .r#type
                .total_array()
                .is_some_and(|size| size > limit)
                || variable.total_width().is_some_and(|width| width > limit)
        })
        .map(|variable| variable.token);
    if let Some(token) = oversized {
        return Err(Box::new(
            AnalyzerError::combinational_loop_position_overflow(&token),
        ));
    }
    let bit_part = build_bit_partition(module, &ctx);
    if let Some(token) = bit_part.position_overflow().map(|id| {
        module
            .variables
            .get(&id)
            .or_else(|| module.interface_members.get(&id))
            .map_or(module.token, |variable| variable.token)
    }) {
        return Err(Box::new(
            AnalyzerError::combinational_loop_position_overflow(&token),
        ));
    }

    let mut builder = ModuleGraphBuilder::new(module, &bit_part, ctx, steps);
    builder.function_summaries.tracing = tracing == TraceKind::Sources;
    builder.trace_instances = tracing != TraceKind::None;

    let combs = module
        .declarations
        .iter()
        .enumerate()
        .filter_map(|(declaration_index, declaration)| match declaration {
            Declaration::Comb(comb) => Some((declaration_index, comb)),
            _ => None,
        })
        .collect::<Vec<_>>();
    // Procedures leave half of the steps for deciding the cycles they form.
    let analyses = steps.share(
        combs.len(),
        true,
        |stage| {
            let (declaration_index, comb) = combs[stage];
            procedure::analyze(
                &bit_part,
                &comb.statements,
                declaration_index + 1,
                &mut builder.procedure_context,
                &mut builder.function_summaries,
            )
        },
        |analysis| analysis.ran_out,
    );
    for analysis in analyses {
        if !analysis.status.is_complete() {
            builder.complete = false;
        }
        if analysis.status.is_barrier() {
            continue;
        }
        builder.add_procedure_graph(module, analysis);
    }

    for (declaration_index, declaration) in module.declarations.iter().enumerate() {
        let Declaration::Inst(inst) = declaration else {
            continue;
        };
        match inst.component.as_ref() {
            Component::Module(child) => {
                let Some(summary) = summaries.get(&child.signature) else {
                    builder.complete = false;
                    continue;
                };
                builder.complete &= summary.complete;
                builder.add_instance_feedthrough(module, declaration_index, inst, child, summary);
            }
            // SV black box: under-detect.
            Component::SystemVerilog(_) => builder.complete = false,
            // Interface signals are already lifted into the parent.
            Component::Interface(_) => {}
        }
    }

    let (graph, complete) = builder.finish();
    #[cfg(test)]
    {
        let (atoms, nodes, edges) = ANALYSIS_SIZE.get();
        ANALYSIS_SIZE.set((
            atoms,
            nodes + graph.node_count(),
            edges + graph.edge_count(),
        ));
    }
    Ok((graph, complete))
}

struct ModuleGraphBuilder<'a> {
    bit_part: &'a BitPartition,
    graph: DependencyGraph,
    node_map: HashMap<NodeKey, NodeIndex>,
    ctx: Context,
    procedure_context: procedure::ProcedureContext,
    function_summaries: procedure::FunctionSummaries<'a>,
    summary_budget: ExpansionBudget,
    trace_instances: bool,
    complete: bool,
}

impl<'a> ModuleGraphBuilder<'a> {
    fn new(module: &'a Module, bit_part: &'a BitPartition, ctx: Context, steps: &Steps) -> Self {
        Self {
            bit_part,
            graph: DependencyGraph::new(),
            node_map: HashMap::default(),
            ctx,
            procedure_context: procedure::ProcedureContext::new(module),
            function_summaries: procedure::FunctionSummaries::new(module, bit_part, steps),
            summary_budget: ExpansionBudget::new(steps),
            trace_instances: false,
            complete: !module
                .variables
                .values()
                .any(|variable| matches!(variable.kind, crate::ir::VarKind::Inout)),
        }
    }

    fn finish(self) -> (DependencyGraph, bool) {
        (self.graph, self.complete)
    }

    fn add_procedure_graph(&mut self, module: &Module, analysis: procedure::ProcedureResult) {
        add_procedure_graph(
            &mut self.graph,
            &mut self.node_map,
            self.bit_part,
            module,
            analysis,
        );
    }

    fn add_instance_feedthrough(
        &mut self,
        module: &'a Module,
        declaration_index: usize,
        inst: &InstDeclaration,
        child: &Module,
        summary: &ModuleCombSummary,
    ) {
        if !self.summary_budget.reserve(summary) {
            self.complete = false;
            return;
        }
        let bit_part = self.bit_part;
        let graph = &mut self.graph;
        let node_map = &mut self.node_map;
        let ctx = &mut self.ctx;
        let procedure_context = &mut self.procedure_context;
        let function_summaries = &mut self.function_summaries;
        let mut actuals = InstanceActuals::default();
        let outputs = OutputConnections::new(inst, child, ctx);
        let mut complete = true;
        let mut input_reads: HashMap<VarId, Vec<procedure::RegionSource>> = HashMap::default();
        for inp in &inst.inputs {
            if !is_pure_input_or_output(inp.id, &child.variables, Direction::Input) {
                continue;
            }
            let (mut reads, dependencies, actual_complete) = analyze_instance_actual(
                bit_part,
                &inp.expr,
                ctx,
                procedure_context,
                function_summaries,
            );
            complete &= actual_complete;
            if let Some(dependencies) = dependencies {
                add_procedure_graph(graph, node_map, bit_part, module, dependencies);
            }
            reads.sort_unstable_by_key(|source| {
                (source.key, source.offset, source.condition.clone())
            });
            reads.dedup_by(|left, right| {
                left.key == right.key
                    && left.offset == right.offset
                    && left.condition == right.condition
            });
            if !reads.is_empty() {
                input_reads.insert(inp.id, reads);
            }
        }

        let mut output_dsts: HashMap<VarId, Vec<procedure::RegionSource>> = HashMap::default();
        for out in &inst.outputs {
            if !is_pure_input_or_output(out.id, &child.variables, Direction::Output) {
                continue;
            }
            let mut keys = Vec::new();
            for dst in &out.dst {
                let mut destination_keys = Vec::new();
                collect_dst_node_keys(dst, bit_part, &mut destination_keys, ctx);
                let (selector_reads, dependencies, selector_complete) =
                    analyze_instance_destination(
                        bit_part,
                        dst,
                        ctx,
                        procedure_context,
                        function_summaries,
                    );
                complete &= selector_complete;
                if let Some(dependencies) = dependencies {
                    add_procedure_graph(graph, node_map, bit_part, module, dependencies);
                }
                for source in selector_reads {
                    for destination in &destination_keys {
                        add_region_dependency(
                            graph,
                            node_map,
                            bit_part,
                            source.key,
                            *destination,
                            GraphDependency {
                                kind: BitDependency::WHOLE,
                                condition: source.condition.clone(),
                            },
                        );
                    }
                }
                keys.extend(destination_keys);
            }
            keys.sort_unstable();
            keys.dedup();
            if !keys.is_empty() {
                output_dsts.insert(
                    out.id,
                    keys.into_iter()
                        .map(|key| procedure::RegionSource {
                            key,
                            offset: None,
                            condition: PathCondition::default(),
                        })
                        .collect(),
                );
            }
        }

        let summary_branches = remap_module_summary_branches(summary, inst);
        let mut mapped_nodes = Vec::with_capacity(summary.nodes.len());
        let mut endpoint_mappings = Vec::with_capacity(summary.nodes.len());
        for node in &summary.nodes {
            let (mapping, endpoint_mapping) = match node.kind {
                SummaryNodeKind::Input => {
                    let mapping = map_instance_source_region(
                        graph,
                        node_map,
                        inst,
                        child,
                        node.region,
                        input_reads.get(&node.region.id).map(Vec::as_slice),
                        bit_part,
                        ctx,
                        &mut actuals,
                        &mut self.summary_budget,
                    );
                    (mapping, None)
                }
                SummaryNodeKind::Output => {
                    let mapping = outputs.mapping(node.region, bit_part).unwrap_or_else(|| {
                        instance_region_mapping(
                            inst,
                            child,
                            node.region,
                            Direction::Output,
                            output_dsts.get(&node.region.id).map(Vec::as_slice),
                            bit_part,
                            ctx,
                        )
                    });
                    let resolved =
                        resolve_instance_mapping(graph, node_map, bit_part, mapping.clone());
                    (resolved, Some(mapping))
                }
                SummaryNodeKind::Interface => {
                    let mapping = instance_region_mapping(
                        inst,
                        child,
                        node.region,
                        Direction::Input,
                        None,
                        bit_part,
                        ctx,
                    );
                    let resolved =
                        resolve_instance_mapping(graph, node_map, bit_part, mapping.clone());
                    (resolved, Some(mapping))
                }
                SummaryNodeKind::Internal => (
                    ResolvedInstanceRegionMapping {
                        nodes: vec![ResolvedMappedNode {
                            node: graph.add_node(GraphNode {
                                region: node.region,
                                domains: node.domains.clone(),
                                diagnostic: None,
                            }),
                            offset: Some((0, 0)),
                            condition: PathCondition::default(),
                            region: None,
                        }],
                    },
                    None,
                ),
            };
            mapped_nodes.push(mapping);
            endpoint_mappings.push(endpoint_mapping);
        }

        for edge in &summary.edges {
            if self.trace_instances {
                graph.active_summary = Some(diagnostics::SummaryEdgeCause {
                    inst_declaration: declaration_index,
                    child: child.signature.clone(),
                    child_source: summary.nodes[edge.source].region,
                    child_destination: summary.nodes[edge.destination].region,
                });
            }
            let condition = edge.condition.remapped(&summary_branches);
            if summary.nodes[edge.source].kind == SummaryNodeKind::Input
                && let Some((array, packed)) = edge.kind.exact_offset()
                && let Some(destinations) = &endpoint_mappings[edge.destination]
            {
                let mut fallback_destinations = Vec::new();
                for destination in &destinations.nodes {
                    match child_source_region_for_destination(
                        summary.nodes[edge.source].region,
                        summary.nodes[edge.destination].region,
                        array,
                        packed,
                        destination,
                        bit_part,
                    ) {
                        RegionProjection::Exact(source_region) => {
                            let sources = map_instance_source_region(
                                graph,
                                node_map,
                                inst,
                                child,
                                source_region,
                                input_reads
                                    .get(&summary.nodes[edge.source].region.id)
                                    .map(Vec::as_slice),
                                bit_part,
                                ctx,
                                &mut actuals,
                                &mut self.summary_budget,
                            );
                            let destinations = resolve_instance_mapping(
                                graph,
                                node_map,
                                bit_part,
                                InstanceRegionMapping {
                                    nodes: vec![destination.clone()],
                                },
                            );
                            add_resolved_dependency_edges(
                                graph,
                                &sources,
                                &destinations,
                                edge.kind,
                                &condition,
                            );
                        }
                        RegionProjection::Disjoint => {}
                        RegionProjection::Unknown => {
                            fallback_destinations.push(destination.clone())
                        }
                    }
                }
                if fallback_destinations.is_empty() {
                    continue;
                }
                let destinations = resolve_instance_mapping(
                    graph,
                    node_map,
                    bit_part,
                    InstanceRegionMapping {
                        nodes: fallback_destinations,
                    },
                );
                add_resolved_dependency_edges(
                    graph,
                    &mapped_nodes[edge.source],
                    &destinations,
                    edge.kind,
                    &condition,
                );
                continue;
            }
            add_resolved_dependency_edges(
                graph,
                &mapped_nodes[edge.source],
                &mapped_nodes[edge.destination],
                edge.kind,
                &condition,
            );
        }
        graph.active_summary = None;
        complete &= actuals.resolve(
            graph,
            node_map,
            bit_part,
            inst,
            child,
            &input_reads,
            procedure_context,
            function_summaries,
            &mut self.summary_budget,
        );
        self.complete &= complete;
    }
}

fn add_dependency_dag(
    graph: &mut DependencyGraph,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    bit_part: &BitPartition,
    dag: ssa::DependencyDag<NodeKey>,
    internal_region: SummaryRegion,
    allowed: impl Fn(NodeKey) -> bool,
) -> Vec<Option<NodeIndex>> {
    // These are parent expression/procedure edges, even when insertion
    // was triggered by projecting an individual child summary edge.
    let active_summary = graph.active_summary.take();
    let mapped = dag
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| match node {
            DependencyDagNode::External(key) if allowed(*key) => {
                ensure_node(graph, node_map, bit_part, *key)
            }
            DependencyDagNode::External(_) => None,
            DependencyDagNode::Internal | DependencyDagNode::Replicated { .. } => {
                Some(graph.add_node(GraphNode {
                    // Internal nodes carry no variable identity. The region is
                    // only a coordinate carrier; exact edge relations retain
                    // the positional semantics.
                    region: internal_region,
                    domains: dag.domains[index].clone(),
                    diagnostic: None,
                }))
            }
        })
        .collect::<Vec<_>>();

    for (index, node) in dag.nodes.iter().enumerate() {
        if let DependencyDagNode::Replicated { replication } = node
            && let Some(node) = mapped[index]
        {
            // A bounded nonzero translation, forward or backward, represents
            // every copy without expanding bits, repetitions or paths through
            // function imports.
            let relation = replication.relation();
            add_dependency_edge(
                graph,
                node,
                node,
                GraphDependency::unconditional(BitDependency {
                    array: relation.array,
                    packed: relation.packed,
                }),
            );
        }
    }

    for (node, site) in dag.sites {
        if let Some(node) = mapped[node] {
            graph.sites.insert(
                node,
                ssa::DefinitionSite {
                    token: site.token,
                    data_inputs: site
                        .data_inputs
                        .iter()
                        .filter_map(|input| mapped[*input])
                        .collect(),
                },
            );
        }
    }
    for edge in dag.edges {
        let (Some(source), Some(destination)) = (mapped[edge.source], mapped[edge.destination])
        else {
            continue;
        };
        add_dependency_edge(
            graph,
            source,
            destination,
            GraphDependency {
                kind: BitDependency {
                    array: edge.relation.array,
                    packed: edge.relation.packed,
                },
                condition: edge.condition,
            },
        );
    }
    graph.active_summary = active_summary;
    mapped
}

fn add_procedure_graph(
    graph: &mut DependencyGraph,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    bit_part: &BitPartition,
    module: &Module,
    analysis: procedure::ProcedureResult,
) {
    let destinations = analysis
        .destinations
        .into_iter()
        .filter_map(|(key, root)| {
            (is_module_scope_var(key.0, &module.variables) && !is_inout(key.0, &module.variables))
                .then_some((key, root))
        })
        .collect::<Vec<_>>();
    let Some(internal_region) = destinations.iter().find_map(|(key, _)| {
        ensure_node(graph, node_map, bit_part, *key).map(|node| graph[node].region)
    }) else {
        return;
    };

    let mapped = add_dependency_dag(
        graph,
        node_map,
        bit_part,
        analysis.graph,
        internal_region,
        |key| is_module_scope_var(key.0, &module.variables) && !is_inout(key.0, &module.variables),
    );
    for (destination, root) in destinations {
        let (Some(root), Some(destination)) = (
            root.and_then(|root| mapped[root]),
            ensure_node(graph, node_map, bit_part, destination),
        ) else {
            continue;
        };
        add_dependency_edge(
            graph,
            root,
            destination,
            GraphDependency::unconditional(BitDependency {
                array: Some(0),
                packed: Some(0),
            }),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn map_instance_source_region(
    graph: &mut DependencyGraph,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
    allowed: Option<&[procedure::RegionSource]>,
    bit_part: &BitPartition,
    ctx: &mut Context,
    actuals: &mut InstanceActuals,
    budget: &mut ExpansionBudget,
) -> ResolvedInstanceRegionMapping {
    let parent_sources = instance_region_mapping(
        inst,
        child,
        region,
        Direction::Input,
        allowed,
        bit_part,
        ctx,
    );
    // Direct storage nodes are safe only when the requested child region
    // contains the entire mapped parent node. Otherwise a whole-value child
    // dependency would erase the actual's slice bounds before applying them.
    if parent_sources.nodes.iter().all(|source| {
        let Some((array_offset, packed_offset)) = source.offset else {
            return false;
        };
        let Some(array_start) = translate_position(region.array.start, array_offset) else {
            return false;
        };
        let Some(packed_start) = translate_position(region.packed.start, packed_offset) else {
            return false;
        };
        let array = ArraySpan {
            start: array_start,
            length: region.array.length,
        };
        let Some(packed) = PackedSpan::new(packed_start, region.packed.length) else {
            return false;
        };
        let parent_packed = bit_part.ranges_of((source.key.0, source.key.1))[source.key.2];
        array.intersection(source.key.1) == Some(source.key.1)
            && packed.intersection(parent_packed) == Some(parent_packed)
    }) {
        return resolve_instance_mapping(graph, node_map, bit_part, parent_sources);
    }
    if parent_sources
        .nodes
        .iter()
        .all(|source| source.offset.is_some())
    {
        // Connection metadata already gives the exact coordinate mapping;
        // project it directly.
        let sources = resolve_instance_mapping(graph, node_map, bit_part, parent_sources);
        return actuals.project_mapping(graph, region, &sources, budget);
    }
    let Some(_) = inst.inputs.iter().find(|input| input.id == region.id) else {
        return resolve_instance_mapping(graph, node_map, bit_part, parent_sources);
    };
    let Some(variable) = child.variables.get(&region.id) else {
        return resolve_instance_mapping(graph, node_map, bit_part, parent_sources);
    };
    let Some(_) = variable.total_width() else {
        return resolve_instance_mapping(graph, node_map, bit_part, parent_sources);
    };
    actuals.defer(graph, region, budget)
}

#[derive(Default)]
struct InstanceActuals {
    roots: HashMap<SummaryRegion, NodeIndex>,
    projections: HashMap<SummaryRegion, NodeIndex>,
    exhausted: bool,
}

impl InstanceActuals {
    fn project_mapping(
        &mut self,
        graph: &mut DependencyGraph,
        region: SummaryRegion,
        sources: &ResolvedInstanceRegionMapping,
        budget: &mut ExpansionBudget,
    ) -> ResolvedInstanceRegionMapping {
        let root = if let Some(&root) = self.projections.get(&region) {
            Some(root)
        } else if budget.reserve_work(sources.nodes.len().saturating_add(1)) {
            let root = graph.add_node(GraphNode {
                region,
                domains: vec![PositionDomain {
                    array_start: region.array.start,
                    array_length: region.array.length,
                    packed_start: region.packed.start,
                    packed_length: region.packed.length,
                }],
                diagnostic: None,
            });
            self.projections.insert(region, root);
            add_resolved_dependency_edges(
                graph,
                sources,
                &ResolvedInstanceRegionMapping::from_root(Some(root)),
                BitDependency::identity(),
                &PathCondition::default(),
            );
            Some(root)
        } else {
            self.exhausted = true;
            None
        };
        ResolvedInstanceRegionMapping::from_root(root)
    }

    fn defer(
        &mut self,
        graph: &mut DependencyGraph,
        region: SummaryRegion,
        budget: &mut ExpansionBudget,
    ) -> ResolvedInstanceRegionMapping {
        let root = if let Some(&root) = self.roots.get(&region) {
            Some(root)
        } else if budget.reserve_work(2) {
            // Resolve all projections together after the child edges have
            // supplied their requested regions. The exported root supplies
            // the bounds; this placeholder only connects its consumers.
            let root = graph.add_node(GraphNode {
                region,
                domains: Vec::new(),
                diagnostic: None,
            });
            self.roots.insert(region, root);
            Some(root)
        } else {
            self.exhausted = true;
            None
        };
        ResolvedInstanceRegionMapping::from_root(root)
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve<'a>(
        self,
        graph: &mut DependencyGraph,
        node_map: &mut HashMap<NodeKey, NodeIndex>,
        bit_part: &'a BitPartition,
        inst: &InstDeclaration,
        child: &Module,
        input_reads: &HashMap<VarId, Vec<procedure::RegionSource>>,
        procedure_context: &mut procedure::ProcedureContext,
        summaries: &mut procedure::FunctionSummaries<'a>,
        budget: &mut ExpansionBudget,
    ) -> bool {
        let mut complete = !self.exhausted;
        let mut regions = self.roots.keys().copied().collect::<Vec<_>>();
        regions.sort_unstable();
        for regions in regions.chunk_by(|left, right| left.id == right.id) {
            let first = regions[0];
            let input = inst
                .inputs
                .iter()
                .find(|input| input.id == first.id)
                .expect("deferred regions have an input actual");
            let variable = &child.variables[&first.id];
            let width = variable
                .total_width()
                .expect("deferred inputs have known widths");
            let expression = &input.expr;
            let context_type = &variable.r#type;
            let mut analysis =
                procedure::ExpressionAnalysis::new(bit_part, procedure_context, summaries);
            let dag = analysis.eval_regions(expression, regions, width, context_type);
            complete &= analysis.is_complete();
            analysis.restore(procedure_context);
            if !budget.reserve_dag(&dag) {
                complete = false;
                continue;
            }
            let allowed = input_reads
                .get(&first.id)
                .into_iter()
                .flatten()
                .map(|source| source.key)
                .collect::<HashSet<_>>();
            let roots = dag.roots.clone();
            let mapped = add_dependency_dag(graph, node_map, bit_part, dag, first, |key| {
                allowed.contains(&key)
            });
            for (region, root) in regions.iter().zip(roots) {
                if let Some(root) = root.and_then(|root| mapped[root]) {
                    add_dependency_edge(
                        graph,
                        root,
                        self.roots[region],
                        GraphDependency::unconditional(BitDependency::identity()),
                    );
                }
            }
        }
        complete
    }
}

#[derive(Clone)]
struct InstanceRegionMapping {
    nodes: Vec<MappedNode>,
}

/// Output destinations are ordered array slices or a packed concatenation.
/// Build their child coordinates once per instance, using the same IR as the
/// assignment checks and backends. Unknown or width-changing connections keep
/// the conservative mapping used for other output expressions.
struct OutputConnections(HashMap<VarId, Vec<OutputFragment>>);

struct OutputFragment {
    parent: VarId,
    child_array: ArraySpan,
    child_packed: PackedSpan,
    parent_array: ArraySpan,
    parent_packed: PackedSpan,
}

struct OutputAccess {
    parent: VarId,
    array: ArraySpan,
    packed: PackedSpan,
    offset: (isize, isize),
}

impl OutputConnections {
    fn new(inst: &InstDeclaration, child: &Module, ctx: &mut Context) -> Self {
        let connections = inst
            .outputs
            .iter()
            .filter_map(|output| {
                let variable = child.variables.get(&output.id)?;
                let fragments = Self::fragments(variable, &output.dst, ctx)?;
                Some((output.id, fragments))
            })
            .collect();
        Self(connections)
    }

    fn fragments(
        child: &Variable,
        destinations: &[AssignDestination],
        ctx: &mut Context,
    ) -> Option<Vec<OutputFragment>> {
        let child_array_length = child.r#type.array.total()?;
        let child_width = child.total_width()?;
        let child_packed = PackedSpan::whole(child_width)?;
        let accesses = destinations
            .iter()
            .map(|dst| {
                if !dst.index.is_const() || !dst.select.is_const_with_range() {
                    return None;
                }
                let spans = var_reads(
                    dst.id,
                    &dst.index,
                    &dst.select,
                    dst.comptime.member_select_domain,
                    ctx,
                );
                let [(array, packed)] = spans.as_slice() else {
                    return None;
                };
                Some((dst.id, *array, *packed))
            })
            .collect::<Option<Vec<_>>>()?;
        let array_length = accesses.iter().try_fold(0usize, |total, (_, array, _)| {
            total.checked_add(array.length)
        })?;
        if array_length == child_array_length
            && accesses
                .iter()
                .all(|(_, _, packed)| packed.length == child_width)
        {
            // Unpacked arrays connect in ascending element order. A destination
            // may cover an entire row of a multidimensional array.
            let mut start = 0;
            return Some(
                accesses
                    .into_iter()
                    .map(|(parent, parent_array, parent_packed)| {
                        let child_array = ArraySpan {
                            start,
                            length: parent_array.length,
                        };
                        start += parent_array.length;
                        OutputFragment {
                            parent,
                            child_array,
                            child_packed,
                            parent_array,
                            parent_packed,
                        }
                    })
                    .collect(),
            );
        }

        let packed_width = accesses.iter().try_fold(0usize, |total, (_, _, packed)| {
            total.checked_add(packed.length)
        })?;
        if child_array_length != 1
            || packed_width != child_width
            || accesses.iter().any(|(_, array, _)| array.length != 1)
        {
            return None;
        }

        // A packed concatenation lists its most significant fragment first.
        // Reverse it so both layouts can be searched in child coordinate order.
        let mut start = 0;
        accesses
            .into_iter()
            .rev()
            .map(|(parent, parent_array, parent_packed)| {
                let child_packed = PackedSpan::new(start, parent_packed.length)?;
                start += parent_packed.length;
                Some(OutputFragment {
                    parent,
                    child_array: ArraySpan {
                        start: 0,
                        length: 1,
                    },
                    child_packed,
                    parent_array,
                    parent_packed,
                })
            })
            .collect()
    }

    fn accesses(&self, region: SummaryRegion) -> Option<Vec<OutputAccess>> {
        let fragments = self.0.get(&region.id)?;
        // Only one axis varies between fragments. Skip preceding fragments
        // without rescanning a whole array for each child summary region.
        let first = fragments.partition_point(|fragment| {
            fragment.child_array.end().unwrap() <= region.array.start
                || fragment.child_packed.end() <= region.packed.start
        });
        fragments[first..]
            .iter()
            .take_while(|fragment| {
                fragment.child_array.start < region.array.end().unwrap()
                    && fragment.child_packed.start < region.packed.end()
            })
            .map(|fragment| {
                let array = region
                    .array
                    .intersection(fragment.child_array)?
                    .translated(fragment.child_array.start, fragment.parent_array.start)?;
                let packed = region
                    .packed
                    .intersection(fragment.child_packed)?
                    .translated(fragment.child_packed.start, fragment.parent_packed.start)?;
                Some(OutputAccess {
                    parent: fragment.parent,
                    array,
                    packed,
                    offset: (
                        signed_difference(fragment.parent_array.start, fragment.child_array.start)?,
                        signed_difference(
                            fragment.parent_packed.start,
                            fragment.child_packed.start,
                        )?,
                    ),
                })
            })
            .collect()
    }

    fn mapping(
        &self,
        region: SummaryRegion,
        bit_part: &BitPartition,
    ) -> Option<InstanceRegionMapping> {
        let nodes = self
            .accesses(region)?
            .into_iter()
            .flat_map(|actual| {
                bit_part
                    .overlapping_access(actual.parent, actual.array, actual.packed)
                    .into_iter()
                    .map(move |key| MappedNode {
                        key,
                        offset: Some(actual.offset),
                        condition: PathCondition::default(),
                        region: Some(PositionDomain::new(actual.array, actual.packed)),
                    })
            })
            .collect();
        Some(InstanceRegionMapping { nodes })
    }
}

#[derive(Clone)]
struct MappedNode {
    key: NodeKey,
    offset: Option<(isize, isize)>,
    condition: PathCondition,
    // The parent positions the actual covers within `key`, when that is
    // known. A summary dependency into this actual reaches only them.
    region: Option<PositionDomain>,
}

struct ResolvedInstanceRegionMapping {
    nodes: Vec<ResolvedMappedNode>,
}

impl ResolvedInstanceRegionMapping {
    fn from_root(root: Option<NodeIndex>) -> Self {
        Self {
            nodes: root
                .into_iter()
                .map(|node| ResolvedMappedNode {
                    node,
                    offset: Some((0, 0)),
                    condition: PathCondition::default(),
                    region: None,
                })
                .collect(),
        }
    }
}

struct ResolvedMappedNode {
    node: NodeIndex,
    offset: Option<(isize, isize)>,
    condition: PathCondition,
    region: Option<PositionDomain>,
}

fn remap_module_summary_branches(
    summary: &ModuleCombSummary,
    inst: &InstDeclaration,
) -> HashMap<BranchId, BranchId> {
    let mut branches = summary
        .edges
        .iter()
        .flat_map(|dependency| dependency.condition.branches())
        .collect::<Vec<_>>();
    branches.sort_unstable();
    branches.dedup();
    let namespace = std::ptr::from_ref(inst).addr();
    branches
        .into_iter()
        .enumerate()
        .map(|(local, branch)| (branch, BranchId::new(namespace, local, branch.arms())))
        .collect()
}

enum RegionProjection {
    Exact(SummaryRegion),
    Disjoint,
    Unknown,
}

fn child_source_region_for_destination(
    child_source: SummaryRegion,
    child_destination: SummaryRegion,
    dependency_array: isize,
    dependency_packed: isize,
    destination: &MappedNode,
    bit_part: &BitPartition,
) -> RegionProjection {
    let Some((destination_array_offset, destination_packed_offset)) = destination.offset else {
        return RegionProjection::Unknown;
    };
    let Some(parent_packed) = bit_part
        .ranges_of((destination.key.0, destination.key.1))
        .get(destination.key.2)
        .copied()
    else {
        return RegionProjection::Unknown;
    };
    let Some(destination_array_offset) = destination_array_offset.checked_neg() else {
        return RegionProjection::Unknown;
    };
    let Some(destination_packed_offset) = destination_packed_offset.checked_neg() else {
        return RegionProjection::Unknown;
    };
    let Some(child_destination_array) =
        translate_array_span(destination.key.1, destination_array_offset)
    else {
        return RegionProjection::Unknown;
    };
    let Some(child_destination_packed) =
        translate_packed_span(parent_packed, destination_packed_offset)
    else {
        return RegionProjection::Unknown;
    };
    let Some(child_destination_array) =
        child_destination_array.intersection(child_destination.array)
    else {
        return RegionProjection::Disjoint;
    };
    let Some(child_destination_packed) =
        child_destination_packed.intersection(child_destination.packed)
    else {
        return RegionProjection::Disjoint;
    };
    let Some(dependency_array) = dependency_array.checked_neg() else {
        return RegionProjection::Unknown;
    };
    let Some(dependency_packed) = dependency_packed.checked_neg() else {
        return RegionProjection::Unknown;
    };
    let Some(child_source_array) = translate_array_span(child_destination_array, dependency_array)
    else {
        return RegionProjection::Unknown;
    };
    let Some(child_source_packed) =
        translate_packed_span(child_destination_packed, dependency_packed)
    else {
        return RegionProjection::Unknown;
    };
    let Some(array) = child_source_array.intersection(child_source.array) else {
        return RegionProjection::Disjoint;
    };
    let Some(packed) = child_source_packed.intersection(child_source.packed) else {
        return RegionProjection::Disjoint;
    };
    RegionProjection::Exact(SummaryRegion {
        id: child_source.id,
        array,
        packed,
    })
}

fn translate_array_span(span: ArraySpan, offset: isize) -> Option<ArraySpan> {
    let start = translate_position(span.start, offset)?;
    (span.length != 0 && start.checked_add(span.length).is_some()).then_some(ArraySpan {
        start,
        length: span.length,
    })
}

fn translate_packed_span(span: PackedSpan, offset: isize) -> Option<PackedSpan> {
    PackedSpan::new(translate_position(span.start, offset)?, span.length)
}

#[allow(clippy::too_many_arguments)]
fn instance_region_mapping(
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
    direction: Direction,
    fallback: Option<&[procedure::RegionSource]>,
    bit_part: &BitPartition,
    ctx: &mut Context,
) -> InstanceRegionMapping {
    let variable = child
        .variables
        .get(&region.id)
        .or_else(|| child.interface_members.get(&region.id));
    if let Some(variable) = variable
        && let Some(actual) = instance_port_region_actual(inst, region.id, direction)
    {
        return map_summary_region(region, variable, actual, bit_part, ctx);
    }

    if let (Some(variable), Some(binding)) = (
        variable,
        inst.interface_bindings
            .iter()
            .find(|binding| binding.child == region.id),
    ) {
        return map_summary_region(
            region,
            variable,
            ParentAccess {
                parent: binding.parent,
                index: &binding.index,
                select: &binding.select,
                member_select_domain: None,
            },
            bit_part,
            ctx,
        );
    }

    InstanceRegionMapping {
        nodes: fallback
            .into_iter()
            .flatten()
            .map(|source| MappedNode {
                key: source.key,
                offset: None,
                condition: source.condition.clone(),
                region: None,
            })
            .collect(),
    }
}

#[derive(Clone, Copy)]
struct ParentAccess<'a> {
    parent: VarId,
    index: &'a crate::ir::VarIndex,
    select: &'a VarSelect,
    member_select_domain: Option<MemberSelectDomain>,
}

fn instance_port_region_actual(
    inst: &InstDeclaration,
    child: VarId,
    direction: Direction,
) -> Option<ParentAccess<'_>> {
    match direction {
        Direction::Input => {
            let input = inst.inputs.iter().find(|input| input.id == child)?;
            let Expression::Term(factor) = &input.expr else {
                return None;
            };
            let Factor::Variable(parent, index, select, comptime) = factor.as_ref() else {
                return None;
            };
            Some(ParentAccess {
                parent: *parent,
                index,
                select,
                member_select_domain: comptime.member_select_domain,
            })
        }
        Direction::Output => {
            let output = inst.outputs.iter().find(|output| output.id == child)?;
            let [destination] = output.dst.as_slice() else {
                return None;
            };
            Some(ParentAccess {
                parent: destination.id,
                index: &destination.index,
                select: &destination.select,
                member_select_domain: destination.comptime.member_select_domain,
            })
        }
        Direction::Inout | Direction::Interface | Direction::Modport | Direction::Import => None,
    }
}

fn map_summary_region(
    region: SummaryRegion,
    child: &Variable,
    actual: ParentAccess<'_>,
    bit_part: &BitPartition,
    ctx: &mut Context,
) -> InstanceRegionMapping {
    if let Some(access) = translated_summary_access(region, child, actual, ctx) {
        return InstanceRegionMapping {
            nodes: access
                .array
                .into_iter()
                .flat_map(|array| {
                    bit_part
                        .overlapping_access(actual.parent, array, access.packed)
                        .into_iter()
                        .map(move |key| (key, array))
                })
                .map(|(key, array)| MappedNode {
                    key,
                    offset: Some(access.offset),
                    condition: PathCondition::default(),
                    region: Some(PositionDomain::new(array, access.packed)),
                })
                .collect(),
        };
    }
    // Each region the actual may cover, on every key it overlaps.
    let mut nodes = Vec::new();
    for (array, packed) in var_reads(
        actual.parent,
        actual.index,
        actual.select,
        actual.member_select_domain,
        ctx,
    ) {
        for key in bit_part.overlapping_access(actual.parent, array, packed) {
            nodes.push(MappedNode {
                key,
                offset: None,
                condition: PathCondition::default(),
                region: Some(PositionDomain::new(array, packed)),
            });
        }
    }
    InstanceRegionMapping { nodes }
}

struct TranslatedSummaryAccess {
    array: Option<ArraySpan>,
    packed: PackedSpan,
    offset: (isize, isize),
}

fn translated_summary_access(
    region: SummaryRegion,
    child: &Variable,
    actual: ParentAccess<'_>,
    ctx: &mut Context,
) -> Option<TranslatedSummaryAccess> {
    if !actual.index.is_const() || !actual.select.is_const_with_range() {
        return None;
    }
    let parent = ctx.variables.get(&actual.parent)?.clone();
    let selected_shape = actual
        .index
        .selected_shape(
            ctx,
            &parent.r#type.array,
            actual
                .index
                .indices
                .last()
                .map(|x| x.token_range())
                .unwrap_or_default(),
        )
        .ok()?;
    if child.r#type.array.total() != selected_shape.total() {
        return None;
    }
    let accesses = var_reads(
        actual.parent,
        actual.index,
        actual.select,
        actual.member_select_domain,
        ctx,
    );
    let [(parent_array, parent_packed)] = accesses.as_slice() else {
        return None;
    };
    if child.total_width() != Some(parent_packed.length) {
        return None;
    }
    let selection = actual.index.eval_selection(ctx, &parent.r#type.array)?;
    let array = region
        .array
        .intersection(ArraySpan {
            start: selection.result_start,
            length: selection.length,
        })
        .and_then(|array| array.translated(selection.result_start, selection.source_start));
    let packed = region
        .packed
        .translated(0, parent_packed.start)?
        .intersection(*parent_packed)?;
    let offset = (
        signed_difference(selection.source_start, selection.result_start)?,
        signed_difference(parent_packed.start, 0)?,
    );
    Some(TranslatedSummaryAccess {
        array: array.and_then(|array| array.intersection(*parent_array)),
        packed,
        offset,
    })
}

fn resolve_instance_mapping(
    graph: &mut DependencyGraph,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    bit_part: &BitPartition,
    mapping: InstanceRegionMapping,
) -> ResolvedInstanceRegionMapping {
    let nodes = mapping
        .nodes
        .into_iter()
        .filter_map(|mapped| {
            let node = ensure_node(graph, node_map, bit_part, mapped.key)?;
            Some(ResolvedMappedNode {
                node,
                offset: mapped.offset,
                condition: mapped.condition,
                region: mapped.region,
            })
        })
        .collect();
    ResolvedInstanceRegionMapping { nodes }
}

fn add_resolved_dependency_edges(
    graph: &mut DependencyGraph,
    sources: &ResolvedInstanceRegionMapping,
    destinations: &ResolvedInstanceRegionMapping,
    dependency: BitDependency,
    condition: &PathCondition,
) {
    for source in &sources.nodes {
        for destination in &destinations.nodes {
            let Some(edge_condition) = condition
                .conjoin_if_compatible(&source.condition)
                .and_then(|condition| condition.conjoin_if_compatible(&destination.condition))
            else {
                continue;
            };
            let kind = if let (
                Some((source_array, source_packed)),
                Some((destination_array, destination_packed)),
            ) = (source.offset, destination.offset)
            {
                BitDependency {
                    array: dependency.array.map(|array| {
                        array
                            .checked_add(destination_array)
                            .and_then(|offset| offset.checked_sub(source_array))
                            .expect("mapped array dependency offset must fit in isize")
                    }),
                    packed: dependency.packed.map(|packed| {
                        packed
                            .checked_add(destination_packed)
                            .and_then(|offset| offset.checked_sub(source_packed))
                            .expect("mapped packed dependency offset must fit in isize")
                    }),
                }
            } else {
                BitDependency::WHOLE
            };
            // A carrier admits only the bound region of its storage node,
            // and is created only for a dependency that can reach it.
            let destination_node = &graph[destination.node];
            let destination_domains = destination.region.as_slice();
            if graph[source.node].diagnostic.is_some()
                && destination_node.diagnostic.is_some()
                && !regions_overlap_with_dependency(
                    (graph[source.node].region, &graph[source.node].domains),
                    (
                        destination_node.region,
                        if destination_domains.is_empty() {
                            &destination_node.domains
                        } else {
                            destination_domains
                        },
                    ),
                    kind,
                )
            {
                continue;
            }
            let target = destination.region.map_or(destination.node, |region| {
                region_carrier(graph, destination.node, region)
            });
            add_dependency_edge(
                graph,
                source.node,
                target,
                GraphDependency {
                    kind,
                    condition: edge_condition,
                },
            );
        }
    }
}

/// A node that admits only `region` of `node` and feeds it. Dependencies
/// into part of a storage node pass through it, so an unlinked summary
/// dependency reaches the actual's positions rather than the whole storage.
fn region_carrier(
    graph: &mut DependencyGraph,
    node: NodeIndex,
    region: PositionDomain,
) -> NodeIndex {
    if graph[node].domains.as_slice() == [region] {
        return node;
    }
    if let Some(&carrier) = graph.carriers.get(&(node, region)) {
        return carrier;
    }
    let storage = graph[node].region;
    let carrier = graph.add_node(GraphNode {
        region: storage,
        domains: vec![region],
        diagnostic: None,
    });
    let active_summary = graph.active_summary.take();
    add_dependency_edge(
        graph,
        carrier,
        node,
        GraphDependency::unconditional(BitDependency::identity()),
    );
    graph.active_summary = active_summary;
    graph.carriers.insert((node, region), carrier);
    carrier
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

fn analyze_instance_actual<'a>(
    bit_part: &'a BitPartition,
    expression: &Expression,
    ctx: &mut Context,
    procedure_context: &mut procedure::ProcedureContext,
    summaries: &mut procedure::FunctionSummaries<'a>,
) -> (
    Vec<procedure::RegionSource>,
    Option<procedure::ProcedureResult>,
    bool,
) {
    let mut analysis = InstanceActualAnalysis::new(
        bit_part,
        ctx,
        procedure_context,
        summaries,
        std::ptr::from_ref(expression).addr(),
    );
    analysis.eval(expression);
    analysis.finish()
}

fn analyze_instance_destination<'a>(
    bit_part: &'a BitPartition,
    destination: &AssignDestination,
    ctx: &mut Context,
    procedure_context: &mut procedure::ProcedureContext,
    summaries: &mut procedure::FunctionSummaries<'a>,
) -> (
    Vec<procedure::RegionSource>,
    Option<procedure::ProcedureResult>,
    bool,
) {
    let mut analysis = InstanceActualAnalysis::new(
        bit_part,
        ctx,
        procedure_context,
        summaries,
        std::ptr::from_ref(destination).addr(),
    );
    for expression in destination
        .index
        .indices
        .iter()
        .chain(destination.select.0.iter())
    {
        analysis.eval(expression);
    }
    if let Some((_, expression)) = &destination.select.1 {
        analysis.eval(expression);
    }
    analysis.finish()
}

struct InstanceActualAnalysis<'a, 's, 'c> {
    bit_part: &'a BitPartition,
    ctx: &'c mut Context,
    procedure_context: &'c mut procedure::ProcedureContext,
    summaries: Option<&'s mut procedure::FunctionSummaries<'a>>,
    procedure: Option<procedure::ExpressionAnalysis<'a, 's>>,
    reads: Vec<procedure::RegionSource>,
    namespace: usize,
}

impl<'a, 's, 'c> InstanceActualAnalysis<'a, 's, 'c> {
    fn new(
        bit_part: &'a BitPartition,
        ctx: &'c mut Context,
        procedure_context: &'c mut procedure::ProcedureContext,
        summaries: &'s mut procedure::FunctionSummaries<'a>,
        namespace: usize,
    ) -> Self {
        Self {
            bit_part,
            ctx,
            procedure_context,
            summaries: Some(summaries),
            procedure: None,
            reads: Vec::new(),
            namespace,
        }
    }

    fn finish(
        mut self,
    ) -> (
        Vec<procedure::RegionSource>,
        Option<procedure::ProcedureResult>,
        bool,
    ) {
        self.reads
            .sort_unstable_by_key(|source| (source.key, source.condition.clone()));
        self.reads
            .dedup_by(|left, right| left.key == right.key && left.condition == right.condition);
        let (dependencies, complete) = if let Some(mut procedure) = self.procedure.take() {
            let dependencies = Some(procedure.dependencies());
            let complete = procedure.is_complete();
            procedure.restore(self.procedure_context);
            (dependencies, complete)
        } else {
            (None, true)
        };
        (self.reads, dependencies, complete)
    }

    fn eval(&mut self, expression: &Expression) {
        if let Some(procedure) = &mut self.procedure {
            self.reads.extend(procedure.eval(expression));
            return;
        }
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::FunctionCall(_) => {
                    let summaries = self.summaries.take().expect("initialized once");
                    let mut procedure = procedure::ExpressionAnalysis::new(
                        self.bit_part,
                        self.procedure_context,
                        summaries,
                    );
                    procedure.use_namespace(self.namespace);
                    self.procedure = Some(procedure);
                    self.eval(expression);
                }
                Factor::Variable(_, index, select, _) => {
                    for expression in index.expressions().chain(select.0.iter()) {
                        self.eval(expression);
                    }
                    if let Some((_, expression)) = &select.1 {
                        self.eval(expression);
                    }
                    let mut reads = Vec::new();
                    collect_factor_node_keys(factor, self.bit_part, &mut reads, self.ctx);
                    self.reads
                        .extend(reads.into_iter().map(|key| procedure::RegionSource {
                            key,
                            offset: None,
                            condition: PathCondition::default(),
                        }));
                }
                Factor::SystemFunctionCall(call) => match &call.kind {
                    SystemFunctionKind::Onehot(input)
                    | SystemFunctionKind::Signed(input)
                    | SystemFunctionKind::Unsigned(input)
                    | SystemFunctionKind::Readmemh(input, _) => self.eval(&input.0),
                    SystemFunctionKind::Bits(_)
                    | SystemFunctionKind::Size(..)
                    | SystemFunctionKind::Clog2(_)
                    | SystemFunctionKind::Display(_)
                    | SystemFunctionKind::Write(_)
                    | SystemFunctionKind::Assert { .. }
                    | SystemFunctionKind::Finish => {}
                },
                _ => {}
            },
            Expression::Unary(_, operand, _) => self.eval(operand),
            Expression::Binary(_, Op::LogicAnd | Op::LogicOr, _, _)
            | Expression::Ternary(_, _, _, _) => {
                let summaries = self.summaries.take().expect("initialized once");
                let mut procedure = procedure::ExpressionAnalysis::new(
                    self.bit_part,
                    self.procedure_context,
                    summaries,
                );
                procedure.use_namespace(self.namespace);
                self.procedure = Some(procedure);
                self.eval(expression);
            }
            Expression::Binary(left, _, right, _) => {
                self.eval(left);
                self.eval(right);
            }
            Expression::Concatenation(parts, _) => {
                for (part, repeat) in parts {
                    self.eval(part);
                    if let Some(repeat) = repeat {
                        self.eval(repeat);
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        crate::ir::ArrayLiteralItem::Value(value, repeat) => {
                            self.eval(value);
                            if let Some(repeat) = repeat {
                                self.eval(repeat);
                            }
                        }
                        crate::ir::ArrayLiteralItem::Defaul(value) => self.eval(value),
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, value) in fields {
                    self.eval(value);
                }
            }
        }
    }
}

fn collect_factor_node_keys(
    factor: &Factor,
    bit_part: &BitPartition,
    out: &mut Vec<NodeKey>,
    ctx: &mut Context,
) {
    match factor {
        Factor::Variable(id, index, select, comptime) => {
            for (idx, span) in var_reads(*id, index, select, comptime.member_select_domain, ctx) {
                out.extend(bit_part.overlapping_access(*id, idx, span));
            }
        }
        Factor::FunctionCall(_) | Factor::SystemFunctionCall(_) => {
            // No caller LHS at an inst input -- under-detect.
        }
        _ => {}
    }
}

fn collect_dst_node_keys(
    dst: &AssignDestination,
    bit_part: &BitPartition,
    out: &mut Vec<NodeKey>,
    ctx: &mut Context,
) {
    for (array, packed) in dst_writes(dst, ctx) {
        out.extend(bit_part.overlapping_access(dst.id, array, packed));
    }
}

fn is_module_scope_var(id: VarId, variables: &HashMap<VarId, Variable>) -> bool {
    match variables.get(&id) {
        Some(v) => matches!(v.affiliation, Affiliation::Module | Affiliation::Interface),
        None => true,
    }
}

fn is_inout(id: VarId, variables: &HashMap<VarId, Variable>) -> bool {
    variables
        .get(&id)
        .is_some_and(|variable| matches!(variable.kind, crate::ir::VarKind::Inout))
}

#[cfg(test)]
mod partition_tests {
    use super::*;

    #[test]
    fn partition_rejects_positions_that_do_not_fit_the_relation_type() {
        let id = VarId::from_raw(0);
        let mut ranges = HashMap::default();
        ranges.insert(
            (
                id,
                ArraySpan {
                    start: isize::MAX as usize + 1,
                    length: 1,
                },
            ),
            vec![PackedSpan {
                start: 0,
                length: 1,
            }],
        );

        assert_eq!(BitPartition::new(ranges).position_overflow(), Some(id));
    }
}
