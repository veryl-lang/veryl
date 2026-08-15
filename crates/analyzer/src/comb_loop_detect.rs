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

mod graph;
mod hierarchy;
mod model;
mod procedure;
mod region;
mod ssa;
mod summary;

#[cfg(test)]
pub(crate) use procedure::{
    function_barrier_evaluation_count, function_evaluation_count,
    function_result_region_probe_count, function_result_version_count,
    function_summary_graph_node_count, module_context_entries, reset_function_evaluation_count,
    reset_module_context_entries,
};

use graph::{
    DependencyGraph, GraphDependency, GraphNode, add_dependency_edge, add_region_dependency,
    check_graph, ensure_node, node_regions_overlap_with_dependency,
};
use hierarchy::{module_postorder, walk_insts};
use model::{BitDependency, ModuleCombSummary, SummaryNodeKind, SummaryRegion};
use region::{
    ArraySpan, BitPartition, IdxKey, NodeKey, PackedSpan, dst_writes, signed_difference,
    translate_position, var_reads,
};
use ssa::{BranchId, DependencyDagNode, PathCondition};
use summary::compute_module_summary;

use crate::AnalyzerError;
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    AssignDestination, Component, Declaration, Expression, Factor, InstDeclaration, Ir, Module, Op,
    Signature, Statement, SystemFunctionKind, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction};
use daggy::petgraph::graph::NodeIndex;

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

    for module in module_postorder(ir) {
        let (graph, _bit_part, module_complete) = build_module_graph(module, &summaries);
        check_graph(module, &graph, &mut errors);
        let mut summary = compute_module_summary(module, &graph);
        summary.complete = module_complete;
        summaries.insert(module.signature.clone(), summary);
        complete &= module_complete;
    }

    (errors, complete)
}

/// Split only at observed access endpoints. Runtime and storage depend on the
/// number of accesses, never on the highest referenced bit position.
fn atomic_ranges(spans: &[PackedSpan], endpoints: Option<&HashSet<usize>>) -> Vec<PackedSpan> {
    let mut events = Vec::with_capacity(spans.len() * 2 + endpoints.map_or(0, HashSet::len));
    for span in spans {
        events.push((span.start, 1isize));
        events.push((span.end(), -1isize));
    }
    if let Some(endpoints) = endpoints {
        events.extend(endpoints.iter().map(|endpoint| (*endpoint, 0)));
    }
    events.sort_unstable_by_key(|event| event.0);

    let mut atoms = Vec::new();
    let mut active = 0isize;
    let mut index = 0;
    while index < events.len() {
        let position = events[index].0;
        while index < events.len() && events[index].0 == position {
            active += events[index].1;
            index += 1;
        }
        if active > 0
            && let Some(next) = events.get(index).map(|event| event.0)
            && let Some(atom) = PackedSpan::new(position, next - position)
        {
            atoms.push(atom);
        }
    }
    atoms
}

fn build_bit_partition(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    ctx: &mut Context,
) -> BitPartition {
    let mut accesses: HashMap<IdxKey, Vec<PackedSpan>> = HashMap::default();

    for declaration in &module.declarations {
        if let Declaration::Comb(comb) = declaration {
            collect_statement_spans(&comb.statements, &mut accesses, ctx);
        }
    }

    // Inst input expressions are not represented by procedure statements.
    for inst in walk_insts(module) {
        for inp in &inst.inputs {
            for expr in &inp.exprs {
                collect_expr_spans(expr, &mut accesses, ctx);
            }
        }
        for out in &inst.outputs {
            for dst in &out.dst {
                if let Some((idx, packed)) = eval_dst_span(dst, &module.variables, ctx) {
                    accesses
                        .entry((
                            dst.id,
                            ArraySpan {
                                start: idx,
                                length: 1,
                            },
                        ))
                        .or_default()
                        .push(packed);
                }
            }
        }
    }

    collect_instance_summary_spans(module, summaries, &mut accesses, ctx);

    // Function-local regions are not represented by the caller's aggregate
    // reference table. They still need atoms because calls are lowered into
    // the same SSA version graph as their caller.
    for function in module.functions.values() {
        for body in &function.functions {
            for (path, id) in &body.arg_map {
                let r#type = module
                    .variables
                    .get(id)
                    .map(|variable| &variable.r#type)
                    .or_else(|| {
                        function
                            .args
                            .iter()
                            .flat_map(|argument| &argument.members)
                            .find_map(|(member, comptime, _)| {
                                (member == path).then_some(&comptime.r#type)
                            })
                    });
                if let Some(r#type) = r#type {
                    add_whole_type_access(&mut accesses, *id, r#type);
                }
            }
            if let Some(id) = body.ret {
                let r#type = module
                    .variables
                    .get(&id)
                    .map(|variable| &variable.r#type)
                    .unwrap_or(&function.r#type.r#type);
                add_whole_type_access(&mut accesses, id, r#type);
            }
            collect_statement_spans(&body.statements, &mut accesses, ctx);
        }
    }

    // SSA evaluation carries positional transfers on dependency edges. The
    // storage partition therefore needs only syntactically observed access
    // boundaries. Closing boundaries over the transfer graph can generate all
    // subset sums of independent shifts and silently devolve into bit-level
    // expansion.
    let endpoints = HashMap::default();
    let ranges = split_array_spans(accesses, &endpoints);

    BitPartition::new(ranges)
}

fn add_whole_type_access(
    accesses: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    id: VarId,
    r#type: &crate::ir::Type,
) {
    let Some(array_length) = r#type.array.total() else {
        return;
    };
    let Some(packed) = r#type.total_width().and_then(PackedSpan::whole) else {
        return;
    };
    accesses
        .entry((
            id,
            ArraySpan {
                start: 0,
                length: array_length,
            },
        ))
        .or_default()
        .push(packed);
}

fn collect_instance_summary_spans(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    accesses: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    ctx: &mut Context,
) {
    for inst in walk_insts(module) {
        let Component::Module(child) = inst.component.as_ref() else {
            continue;
        };
        let Some(summary) = summaries.get(&child.signature) else {
            continue;
        };
        for node in &summary.nodes {
            let direction = match node.kind {
                SummaryNodeKind::Input | SummaryNodeKind::Interface => Direction::Input,
                SummaryNodeKind::Output => Direction::Output,
                SummaryNodeKind::Internal => continue,
            };
            if let Some((parent, array, packed)) =
                summary_parent_access(inst, child, node.region, direction, ctx)
            {
                accesses.entry((parent, array)).or_default().push(packed);
            }
        }
    }
}

fn summary_parent_access(
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
    direction: Direction,
    ctx: &mut Context,
) -> Option<(VarId, ArraySpan, PackedSpan)> {
    let variable = child
        .variables
        .get(&region.id)
        .or_else(|| child.interface_members.get(&region.id))?;
    if let Some((parent, index, select)) = instance_port_region_actual(inst, region.id, direction) {
        return translated_summary_access(region, variable, parent, index, select, ctx)
            .map(|(array, packed, _)| (parent, array, packed));
    }
    let binding = inst
        .interface_bindings
        .iter()
        .find(|binding| binding.child == region.id)?;
    translated_summary_access(
        region,
        variable,
        binding.parent,
        &binding.index,
        &binding.select,
        ctx,
    )
    .map(|(array, packed, _)| (binding.parent, array, packed))
}

fn split_array_spans(
    accesses_by_index: HashMap<IdxKey, Vec<PackedSpan>>,
    endpoints: &HashMap<VarId, HashSet<usize>>,
) -> HashMap<IdxKey, Vec<PackedSpan>> {
    let mut accesses: HashMap<VarId, Vec<(ArraySpan, PackedSpan)>> = HashMap::default();
    for ((id, span), packed_spans) in accesses_by_index {
        for packed in packed_spans {
            accesses.entry(id).or_default().push((span, packed));
        }
    }

    let mut ranges = HashMap::default();
    for (id, accesses) in accesses {
        let mut events = Vec::with_capacity(accesses.len() * 2);
        for (span, packed) in accesses {
            if span.length == 0 {
                continue;
            }
            let Some(end) = span.end() else {
                continue;
            };
            events.push((span.start, true, packed));
            events.push((end, false, packed));
        }
        events.sort_unstable_by_key(|(position, starts, packed)| {
            (*position, *starts, packed.start, packed.length)
        });

        let mut active: HashMap<PackedSpan, usize> = HashMap::default();
        let mut previous = events.first().map(|event| event.0);
        let mut cursor = 0;
        while cursor < events.len() {
            let position = events[cursor].0;
            if let Some(previous) = previous
                && previous < position
                && !active.is_empty()
            {
                let split = ArraySpan {
                    start: previous,
                    length: position - previous,
                };
                let split_spans = active.keys().copied().collect::<Vec<_>>();
                let parts = atomic_ranges(&split_spans, endpoints.get(&id));
                if !parts.is_empty() {
                    ranges.insert((id, split), parts);
                }
            }
            while cursor < events.len() && events[cursor].0 == position {
                let (_, starts, packed) = events[cursor];
                if starts {
                    *active.entry(packed).or_default() += 1;
                } else if let std::collections::hash_map::Entry::Occupied(mut entry) =
                    active.entry(packed)
                {
                    *entry.get_mut() -= 1;
                    if *entry.get() == 0 {
                        entry.remove();
                    }
                }
                cursor += 1;
            }
            previous = Some(position);
        }
    }
    ranges
}

fn collect_expr_spans(
    expr: &Expression,
    out: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    ctx: &mut Context,
) {
    match expr {
        Expression::Term(t) => collect_factor_spans(t, out, ctx),
        Expression::Unary(_, e, _) => collect_expr_spans(e, out, ctx),
        Expression::Binary(a, _, b, _) => {
            collect_expr_spans(a, out, ctx);
            collect_expr_spans(b, out, ctx);
        }
        Expression::Ternary(a, b, c, _) => {
            collect_expr_spans(a, out, ctx);
            collect_expr_spans(b, out, ctx);
            collect_expr_spans(c, out, ctx);
        }
        Expression::Concatenation(parts, _) => {
            for (a, b) in parts {
                collect_expr_spans(a, out, ctx);
                if let Some(b) = b {
                    collect_expr_spans(b, out, ctx);
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            for (_, e) in fields {
                collect_expr_spans(e, out, ctx);
            }
        }
        Expression::ArrayLiteral(items, _) => {
            for item in items {
                match item {
                    crate::ir::ArrayLiteralItem::Value(value, repeat) => {
                        collect_expr_spans(value, out, ctx);
                        if let Some(repeat) = repeat {
                            collect_expr_spans(repeat, out, ctx);
                        }
                    }
                    crate::ir::ArrayLiteralItem::Defaul(value) => {
                        collect_expr_spans(value, out, ctx);
                    }
                }
            }
        }
    }
}

fn collect_factor_spans(
    factor: &Factor,
    out: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    ctx: &mut Context,
) {
    match factor {
        Factor::Variable(id, index, select, _) => {
            for (idx, packed) in var_reads(*id, index, select, ctx) {
                out.entry((*id, idx)).or_default().push(packed);
            }
        }
        Factor::FunctionCall(call) => {
            for input in call.inputs.values() {
                collect_expr_spans(input, out, ctx);
            }
        }
        Factor::SystemFunctionCall(call) => match &call.kind {
            SystemFunctionKind::Onehot(input)
            | SystemFunctionKind::Signed(input)
            | SystemFunctionKind::Unsigned(input)
            | SystemFunctionKind::Readmemh(input, _) => {
                collect_expr_spans(&input.0, out, ctx);
            }
            SystemFunctionKind::Bits(_)
            | SystemFunctionKind::Size(_)
            | SystemFunctionKind::Clog2(_)
            | SystemFunctionKind::Display(_)
            | SystemFunctionKind::Write(_)
            | SystemFunctionKind::Assert { .. }
            | SystemFunctionKind::Finish => {}
        },
        _ => {}
    }
}

fn collect_statement_spans(
    statements: &[Statement],
    out: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    ctx: &mut Context,
) {
    for statement in statements {
        match statement {
            Statement::Assign(assign) => {
                collect_expr_spans(&assign.expr, out, ctx);
                for destination in &assign.dst {
                    for (index, packed) in dst_writes(destination, ctx) {
                        out.entry((destination.id, index)).or_default().push(packed);
                    }
                }
            }
            Statement::If(statement) => {
                collect_expr_spans(&statement.cond, out, ctx);
                collect_statement_spans(&statement.true_side, out, ctx);
                collect_statement_spans(&statement.false_side, out, ctx);
            }
            Statement::Case(statement) => {
                collect_expr_spans(&statement.case_target, out, ctx);
                for arm in &statement.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            crate::ir::CasePattern::Eq(expression) => {
                                collect_expr_spans(expression, out, ctx);
                            }
                            crate::ir::CasePattern::Range { lo, hi, .. } => {
                                collect_expr_spans(lo, out, ctx);
                                collect_expr_spans(hi, out, ctx);
                            }
                        }
                    }
                    collect_statement_spans(&arm.body, out, ctx);
                }
                collect_statement_spans(&statement.default, out, ctx);
            }
            Statement::For(statement) => {
                collect_statement_spans(&statement.body, out, ctx);
            }
            Statement::FunctionCall(call) => {
                for input in call.inputs.values() {
                    collect_expr_spans(input, out, ctx);
                }
                for outputs in call.outputs.values() {
                    for destination in outputs {
                        for (index, packed) in dst_writes(destination, ctx) {
                            out.entry((destination.id, index)).or_default().push(packed);
                        }
                    }
                }
            }
            Statement::SystemFunctionCall(_)
            | Statement::IfReset(_)
            | Statement::TbMethodCall(_)
            | Statement::Break
            | Statement::Unsupported(_)
            | Statement::Null => {}
        }
    }
}

/// None if the index is dynamic.
fn eval_dst_span(
    dst: &AssignDestination,
    parent_vars: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Option<(usize, PackedSpan)> {
    let v = parent_vars.get(&dst.id)?;
    let idx_path = dst.index.eval_value(ctx)?;
    let flat = v.r#type.array.calc_index(&idx_path)?;
    let span = if let Some((high, low)) = dst.select.eval_value(ctx, &v.r#type, false) {
        PackedSpan::from_select(high, low)?
    } else {
        let width = v.total_width()?;
        PackedSpan::whole(width)?
    };
    Some((flat, span))
}

fn build_module_graph(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
) -> (DependencyGraph, BitPartition, bool) {
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.variables.extend(module.interface_members.clone());
    ctx.functions = module.functions.clone();
    let bit_part = build_bit_partition(module, summaries, &mut ctx);
    let mut builder = ModuleGraphBuilder::new(module, &bit_part, ctx);

    for (declaration_index, declaration) in module.declarations.iter().enumerate() {
        let Declaration::Comb(comb) = declaration else {
            continue;
        };
        let analysis = procedure::analyze(
            &bit_part,
            &comb.statements,
            declaration_index + 1,
            &mut builder.procedure_context,
            &mut builder.function_summaries,
        );
        if !analysis.status.is_complete() {
            builder.complete = false;
        }
        if analysis.status.is_barrier() {
            continue;
        }
        builder.add_procedure_graph(module, analysis);
    }

    for inst in walk_insts(module) {
        match inst.component.as_ref() {
            Component::Module(child) => {
                let Some(summary) = summaries.get(&child.signature) else {
                    builder.complete = false;
                    continue;
                };
                builder.complete &= summary.complete;
                builder.add_instance_feedthrough(module, inst, child, summary);
            }
            // SV black box: under-detect.
            Component::SystemVerilog(_) => builder.complete = false,
            // Interface signals are already lifted into the parent.
            Component::Interface(_) => {}
        }
    }

    let (graph, complete) = builder.finish();
    (graph, bit_part, complete)
}

struct ModuleGraphBuilder<'a> {
    bit_part: &'a BitPartition,
    graph: DependencyGraph,
    node_map: HashMap<NodeKey, NodeIndex>,
    ctx: Context,
    procedure_context: procedure::ProcedureContext,
    function_summaries: procedure::FunctionSummaries<'a>,
    complete: bool,
}

impl<'a> ModuleGraphBuilder<'a> {
    fn new(module: &'a Module, bit_part: &'a BitPartition, ctx: Context) -> Self {
        Self {
            bit_part,
            graph: DependencyGraph::new(),
            node_map: HashMap::default(),
            ctx,
            procedure_context: procedure::ProcedureContext::new(module),
            function_summaries: procedure::FunctionSummaries::new(module, bit_part),
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
        let destinations = analysis
            .destinations
            .into_iter()
            .filter_map(|(key, root)| {
                (is_module_scope_var(key.0, &module.variables)
                    && !is_inout(key.0, &module.variables))
                .then_some((key, root))
            })
            .collect::<Vec<_>>();
        let Some(internal_region) = destinations.iter().find_map(|(key, _)| {
            ensure_node(&mut self.graph, &mut self.node_map, self.bit_part, *key)
                .map(|node| self.graph[node].region)
        }) else {
            return;
        };

        let mapped = analysis
            .graph
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| match node {
                DependencyDagNode::External(key)
                    if is_module_scope_var(key.0, &module.variables)
                        && !is_inout(key.0, &module.variables) =>
                {
                    ensure_node(&mut self.graph, &mut self.node_map, self.bit_part, *key)
                }
                DependencyDagNode::External(_) => None,
                DependencyDagNode::Internal => Some(self.graph.add_node(GraphNode {
                    // Internal nodes carry no variable identity. The region is
                    // only a coordinate carrier; exact edge relations retain
                    // the positional semantics.
                    region: internal_region,
                    domains: analysis.graph.domains[index].clone(),
                    diagnostic: None,
                })),
            })
            .collect::<Vec<_>>();

        for edge in analysis.graph.edges {
            let (Some(source), Some(destination)) = (mapped[edge.source], mapped[edge.destination])
            else {
                continue;
            };
            add_dependency_edge(
                &mut self.graph,
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
        for (destination, root) in destinations {
            let (Some(root), Some(destination)) = (
                root.and_then(|root| mapped[root]),
                ensure_node(
                    &mut self.graph,
                    &mut self.node_map,
                    self.bit_part,
                    destination,
                ),
            ) else {
                continue;
            };
            add_dependency_edge(
                &mut self.graph,
                root,
                destination,
                GraphDependency::unconditional(BitDependency {
                    array: Some(0),
                    packed: Some(0),
                }),
            );
        }
    }

    fn add_instance_feedthrough(
        &mut self,
        module: &'a Module,
        inst: &InstDeclaration,
        child: &Module,
        summary: &ModuleCombSummary,
    ) {
        let bit_part = self.bit_part;
        let graph = &mut self.graph;
        let node_map = &mut self.node_map;
        let parent_vars = &module.variables;
        let ctx = &mut self.ctx;
        let procedure_context = &mut self.procedure_context;
        let function_summaries = &mut self.function_summaries;
        let mut complete = true;
        let mut input_reads: HashMap<VarId, Vec<procedure::RegionSource>> = HashMap::default();
        for inp in &inst.inputs {
            if !is_pure_input_or_output(inp.id, &child.variables, Direction::Input) {
                continue;
            }
            let mut reads = Vec::new();
            for expression in &inp.exprs {
                let (sources, dependencies, actual_complete) = analyze_instance_actual(
                    bit_part,
                    expression,
                    ctx,
                    procedure_context,
                    function_summaries,
                );
                complete &= actual_complete;
                reads.extend(sources);
                for dependency in dependencies {
                    add_region_dependency(
                        graph,
                        node_map,
                        bit_part,
                        dependency.source,
                        dependency.destination,
                        GraphDependency::unconditional(dependency.kind),
                    );
                }
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
                collect_dst_node_keys(dst, bit_part, &mut destination_keys, parent_vars, ctx);
                let (selector_reads, dependencies, selector_complete) =
                    analyze_instance_destination(
                        bit_part,
                        dst,
                        ctx,
                        procedure_context,
                        function_summaries,
                    );
                complete &= selector_complete;
                for dependency in dependencies {
                    add_region_dependency(
                        graph,
                        node_map,
                        bit_part,
                        dependency.source,
                        dependency.destination,
                        GraphDependency::unconditional(dependency.kind),
                    );
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
        for (index, node) in summary.nodes.iter().enumerate() {
            let (mapping, endpoint_mapping) = match node.kind {
                SummaryNodeKind::Input => {
                    let preserve_position = summary
                        .edges
                        .iter()
                        .any(|edge| edge.source == index && edge.kind.has_position());
                    let mapping = map_instance_source_region(
                        inst,
                        child,
                        node.region,
                        preserve_position,
                        input_reads.get(&node.region.id).map(Vec::as_slice),
                        bit_part,
                        ctx,
                        procedure_context,
                        function_summaries,
                    );
                    let resolved =
                        resolve_instance_mapping(graph, node_map, bit_part, mapping.clone());
                    (resolved, Some(mapping))
                }
                SummaryNodeKind::Output => {
                    let mapping = instance_region_mapping(
                        inst,
                        child,
                        node.region,
                        Direction::Output,
                        output_dsts.get(&node.region.id).map(Vec::as_slice),
                        bit_part,
                        ctx,
                    );
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
                        }],
                    },
                    None,
                ),
            };
            mapped_nodes.push(mapping);
            endpoint_mappings.push(endpoint_mapping);
        }

        for edge in &summary.edges {
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
                                inst,
                                child,
                                source_region,
                                true,
                                input_reads
                                    .get(&summary.nodes[edge.source].region.id)
                                    .map(Vec::as_slice),
                                bit_part,
                                ctx,
                                procedure_context,
                                function_summaries,
                            );
                            let sources =
                                resolve_instance_mapping(graph, node_map, bit_part, sources);
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
        self.complete &= complete;
    }
}

#[allow(clippy::too_many_arguments)]
fn map_instance_source_region<'a>(
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
    preserve_position: bool,
    allowed: Option<&[procedure::RegionSource]>,
    bit_part: &'a BitPartition,
    ctx: &mut Context,
    procedure_context: &mut procedure::ProcedureContext,
    function_summaries: &mut procedure::FunctionSummaries<'a>,
) -> InstanceRegionMapping {
    let parent_sources = instance_region_mapping(
        inst,
        child,
        region,
        Direction::Input,
        allowed,
        bit_part,
        ctx,
    );
    if !preserve_position
        || parent_sources
            .nodes
            .iter()
            .any(|source| source.offset.is_some())
    {
        return parent_sources;
    }
    let Some(input) = inst.inputs.iter().find(|input| input.id == region.id) else {
        return parent_sources;
    };
    let Some(expression) = input.single() else {
        return parent_sources;
    };
    let Some(variable) = child.variables.get(&region.id) else {
        return parent_sources;
    };
    let Some(width) = variable.total_width() else {
        return parent_sources;
    };
    let mut mapping = analyze_instance_actual_region(
        bit_part,
        expression,
        region,
        width,
        procedure_context,
        function_summaries,
    );
    mapping.nodes.retain(|source| {
        allowed.is_some_and(|allowed| allowed.iter().any(|item| item.key == source.key))
    });
    mapping
}

#[derive(Clone)]
struct InstanceRegionMapping {
    nodes: Vec<MappedNode>,
}

#[derive(Clone)]
struct MappedNode {
    key: NodeKey,
    offset: Option<(isize, isize)>,
    condition: PathCondition,
}

struct ResolvedInstanceRegionMapping {
    nodes: Vec<ResolvedMappedNode>,
}

struct ResolvedMappedNode {
    node: NodeIndex,
    offset: Option<(isize, isize)>,
    condition: PathCondition,
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
        && let Some((parent, index, select)) =
            instance_port_region_actual(inst, region.id, direction)
    {
        return map_summary_region(region, variable, parent, index, select, bit_part, ctx);
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
            binding.parent,
            &binding.index,
            &binding.select,
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
            })
            .collect(),
    }
}

fn instance_port_region_actual(
    inst: &InstDeclaration,
    child: VarId,
    direction: Direction,
) -> Option<(VarId, &crate::ir::VarIndex, &VarSelect)> {
    match direction {
        Direction::Input => {
            let input = inst.inputs.iter().find(|input| input.id == child)?;
            let Expression::Term(factor) = input.single()? else {
                return None;
            };
            let Factor::Variable(parent, index, select, _) = factor.as_ref() else {
                return None;
            };
            Some((*parent, index, select))
        }
        Direction::Output => {
            let output = inst.outputs.iter().find(|output| output.id == child)?;
            let [destination] = output.dst.as_slice() else {
                return None;
            };
            Some((destination.id, &destination.index, &destination.select))
        }
        Direction::Inout | Direction::Interface | Direction::Modport | Direction::Import => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn map_summary_region(
    region: SummaryRegion,
    child: &Variable,
    parent: VarId,
    index: &crate::ir::VarIndex,
    select: &VarSelect,
    bit_part: &BitPartition,
    ctx: &mut Context,
) -> InstanceRegionMapping {
    let mut keys = Vec::new();
    let offset = if let Some((array, packed, offset)) =
        translated_summary_access(region, child, parent, index, select, ctx)
    {
        keys.extend(bit_part.overlapping_access(parent, array, packed));
        Some(offset)
    } else {
        for (array, packed) in var_reads(parent, index, select, ctx) {
            keys.extend(bit_part.overlapping_access(parent, array, packed));
        }
        None
    };
    keys.sort_unstable();
    keys.dedup();
    InstanceRegionMapping {
        nodes: keys
            .into_iter()
            .map(|key| MappedNode {
                key,
                offset,
                condition: PathCondition::default(),
            })
            .collect(),
    }
}

fn translated_summary_access(
    region: SummaryRegion,
    child: &Variable,
    parent: VarId,
    index: &crate::ir::VarIndex,
    select: &VarSelect,
    ctx: &mut Context,
) -> Option<(ArraySpan, PackedSpan, (isize, isize))> {
    let accesses = var_reads(parent, index, select, ctx);
    let [(parent_array, parent_packed)] = accesses.as_slice() else {
        return None;
    };
    if !index
        .0
        .iter()
        .all(|expression| expression.comptime().is_const)
        || !select.is_const_with_range()
        || child.r#type.array.total() != Some(parent_array.length)
        || child.total_width() != Some(parent_packed.length)
    {
        return None;
    }
    let start = region.array.start.checked_add(parent_array.start)?;
    let array = (region.array.end()? <= parent_array.length).then_some(ArraySpan {
        start,
        length: region.array.length,
    })?;
    let packed = region
        .packed
        .translated(0, parent_packed.start)?
        .intersection(*parent_packed)?;
    let offset = (
        signed_difference(parent_array.start, 0)?,
        signed_difference(parent_packed.start, 0)?,
    );
    Some((array, packed, offset))
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
            if graph[source.node].diagnostic.is_some()
                && graph[destination.node].diagnostic.is_some()
                && !node_regions_overlap_with_dependency(
                    &graph[source.node],
                    &graph[destination.node],
                    kind,
                )
            {
                continue;
            }
            add_dependency_edge(
                graph,
                source.node,
                destination.node,
                GraphDependency {
                    kind,
                    condition: edge_condition,
                },
            );
        }
    }
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
    Vec<procedure::Dependency>,
    bool,
) {
    let mut analysis = InstanceActualAnalysis::new(bit_part, ctx, procedure_context, summaries);
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
    Vec<procedure::Dependency>,
    bool,
) {
    let mut analysis = InstanceActualAnalysis::new(bit_part, ctx, procedure_context, summaries);
    for expression in destination
        .index
        .0
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

fn analyze_instance_actual_region<'a>(
    bit_part: &'a BitPartition,
    expression: &Expression,
    region: SummaryRegion,
    context_width: usize,
    procedure_context: &mut procedure::ProcedureContext,
    summaries: &mut procedure::FunctionSummaries<'a>,
) -> InstanceRegionMapping {
    let mut analysis = procedure::ExpressionAnalysis::new(bit_part, procedure_context, summaries);
    let sources = analysis.eval_region(expression, region.array, region.packed, context_width);
    let mapping = InstanceRegionMapping {
        nodes: sources
            .into_iter()
            .map(|source| MappedNode {
                key: source.key,
                offset: source.offset,
                condition: source.condition,
            })
            .collect(),
    };
    analysis.restore(procedure_context);
    mapping
}

struct InstanceActualAnalysis<'a, 's, 'c> {
    bit_part: &'a BitPartition,
    ctx: &'c mut Context,
    procedure_context: &'c mut procedure::ProcedureContext,
    summaries: Option<&'s mut procedure::FunctionSummaries<'a>>,
    procedure: Option<procedure::ExpressionAnalysis<'a, 's>>,
    reads: Vec<procedure::RegionSource>,
}

impl<'a, 's, 'c> InstanceActualAnalysis<'a, 's, 'c> {
    fn new(
        bit_part: &'a BitPartition,
        ctx: &'c mut Context,
        procedure_context: &'c mut procedure::ProcedureContext,
        summaries: &'s mut procedure::FunctionSummaries<'a>,
    ) -> Self {
        Self {
            bit_part,
            ctx,
            procedure_context,
            summaries: Some(summaries),
            procedure: None,
            reads: Vec::new(),
        }
    }

    fn finish(
        mut self,
    ) -> (
        Vec<procedure::RegionSource>,
        Vec<procedure::Dependency>,
        bool,
    ) {
        self.reads
            .sort_unstable_by_key(|source| (source.key, source.condition.clone()));
        self.reads
            .dedup_by(|left, right| left.key == right.key && left.condition == right.condition);
        let complete = self
            .procedure
            .as_ref()
            .is_none_or(procedure::ExpressionAnalysis::is_complete);
        let dependencies = if let Some(mut procedure) = self.procedure.take() {
            let dependencies = procedure.dependencies();
            procedure.restore(self.procedure_context);
            dependencies
        } else {
            Vec::new()
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
                    let procedure = procedure::ExpressionAnalysis::new(
                        self.bit_part,
                        self.procedure_context,
                        summaries,
                    );
                    self.procedure = Some(procedure);
                    self.eval(expression);
                }
                Factor::Variable(_, index, select, _) => {
                    for expression in index.0.iter().chain(select.0.iter()) {
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
                    | SystemFunctionKind::Size(_)
                    | SystemFunctionKind::Clog2(_)
                    | SystemFunctionKind::Display(_)
                    | SystemFunctionKind::Write(_)
                    | SystemFunctionKind::Assert { .. }
                    | SystemFunctionKind::Finish => {}
                },
                _ => {}
            },
            Expression::Unary(_, operand, _) => self.eval(operand),
            Expression::Binary(left, op, right, _) => {
                self.eval(left);
                let evaluate_right = match op {
                    Op::LogicAnd => constant_truth(left, self.ctx) != Some(false),
                    Op::LogicOr => constant_truth(left, self.ctx) != Some(true),
                    _ => true,
                };
                if evaluate_right {
                    self.eval(right);
                }
            }
            Expression::Ternary(_, _, _, _) => {
                let summaries = self.summaries.take().expect("initialized once");
                let procedure = procedure::ExpressionAnalysis::new(
                    self.bit_part,
                    self.procedure_context,
                    summaries,
                );
                self.procedure = Some(procedure);
                self.eval(expression);
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

fn constant_truth(expression: &Expression, ctx: &mut Context) -> Option<bool> {
    expression
        .eval_value(ctx)
        .and_then(|value| value.to_usize())
        .map(|value| value != 0)
}

fn collect_factor_node_keys(
    factor: &Factor,
    bit_part: &BitPartition,
    out: &mut Vec<NodeKey>,
    ctx: &mut Context,
) {
    match factor {
        Factor::Variable(id, index, select, _) => {
            for (idx, span) in var_reads(*id, index, select, ctx) {
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
    parent_vars: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) {
    let Some((idx, packed)) = eval_dst_span(dst, parent_vars, ctx) else {
        return;
    };
    let span = ArraySpan {
        start: idx,
        length: 1,
    };
    for r in bit_part.overlapping((dst.id, span), packed) {
        out.push((dst.id, span, r));
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
    fn packed_partition_storage_depends_on_endpoints_not_declared_width() {
        let distant = 1_000_000_000;
        let spans = [
            PackedSpan {
                start: 0,
                length: 1,
            },
            PackedSpan {
                start: distant,
                length: 1,
            },
        ];

        assert_eq!(atomic_ranges(&spans, None), spans);
    }
}
