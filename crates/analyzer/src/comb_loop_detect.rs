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
    ensure_node, node_regions_overlap_with_dependency,
};
#[cfg(test)]
pub(crate) use graph::{cycle_search_work, reset_cycle_search_work, with_cycle_search_limit};
use hierarchy::{module_postorder, walk_insts};
use model::{BitDependency, ModuleCombSummary, SummaryNodeKind, SummaryRegion};
use region::{
    ArraySpan, BitPartition, IdxKey, NodeKey, PackedSpan, dst_writes, signed_difference,
    translate_position, var_reads,
};
use ssa::{BranchId, DependencyDagNode, PathCondition};
#[cfg(test)]
pub(crate) use ssa::{
    import_binding_visits, reset_import_binding_visits, reset_source_walk_visits,
    source_walk_visits,
};
use summary::{ExpansionBudget, compute_module_summary};
#[cfg(test)]
pub(crate) use summary::{
    module_summary_work, reset_module_summary_work, with_module_summary_limit,
};

#[cfg(test)]
pub(crate) use procedure::{with_procedure_guard_limit, with_procedure_import_limit};

use crate::AnalyzerError;
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    AssignDestination, Component, Declaration, Expression, Factor, FunctionCall,
    InstActualFragment, InstDeclaration, InstInterfaceBinding, Ir, Module, Op, Signature,
    Statement, SystemFunctionKind, VarSelect, Variable,
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

    let mut diagnostic_replays = DiagnosticReplayCache::default();
    let mut reported = HashSet::default();
    for module in module_postorder(ir) {
        let (graph, bit_part, module_complete) = match build_module_graph(module, &summaries) {
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
            &bit_part,
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
    let mut calls = Vec::new();

    for declaration in &module.declarations {
        if let Declaration::Comb(comb) = declaration {
            collect_statement_spans(&comb.statements, &mut accesses, &mut calls, ctx);
        }
    }

    // Inst input expressions are not represented by procedure statements.
    for inst in walk_insts(module) {
        for inp in &inst.inputs {
            for expr in &inp.exprs {
                collect_expr_spans(expr, &mut accesses, &mut calls, ctx);
            }
        }
        for out in &inst.outputs {
            for dst in &out.dst {
                for expression in dst
                    .index
                    .0
                    .iter()
                    .chain(dst.select.0.iter())
                    .chain(dst.select.1.iter().map(|(_, x)| x))
                {
                    collect_expr_spans(expression, &mut accesses, &mut calls, ctx);
                }
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
            if function.array.is_empty() {
                collect_statement_spans(&body.statements, &mut accesses, &mut calls, ctx);
            }
        }
    }

    // Hydrate only receivers that are actually called. A queue avoids growing
    // the Rust stack with the function-call graph; each specialization is
    // visited once even when it is called repeatedly.
    let mut visited = HashSet::default();
    while let Some(call) = calls.pop() {
        let receiver = call
            .receiver_index
            .0
            .iter()
            .map(|x| {
                x.comptime()
                    .is_const
                    .then(|| x.comptime().get_value().ok().and_then(|v| v.to_usize()))
                    .flatten()
                    .ok_or_else(|| x.token_range())
            })
            .collect::<Vec<_>>();
        if !visited.insert((call.id, receiver)) {
            continue;
        }
        if let Some(body) = module
            .functions
            .get(&call.id)
            .and_then(|f| f.get_function_for_index(&call.receiver_index))
        {
            collect_statement_spans(&body.statements, &mut accesses, &mut calls, ctx);
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
    // Use the same instance order and budget as graph construction. Skipped
    // summaries must not expand the parent's partition before that check.
    let mut budget = ExpansionBudget::new();
    for inst in walk_insts(module) {
        let Component::Module(child) = inst.component.as_ref() else {
            continue;
        };
        let Some(summary) = summaries.get(&child.signature) else {
            continue;
        };
        if !budget.reserve(summary) {
            continue;
        }
        for node in &summary.nodes {
            let direction = match node.kind {
                SummaryNodeKind::Input | SummaryNodeKind::Interface => Direction::Input,
                SummaryNodeKind::Output => Direction::Output,
                SummaryNodeKind::Internal => continue,
            };
            for (parent, array, packed) in
                summary_parent_accesses(inst, child, node.region, direction, ctx)
            {
                accesses.entry((parent, array)).or_default().push(packed);
            }
        }
    }
}

fn summary_parent_accesses(
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
    direction: Direction,
    ctx: &mut Context,
) -> Vec<(VarId, ArraySpan, PackedSpan)> {
    let Some(variable) = child
        .variables
        .get(&region.id)
        .or_else(|| child.interface_members.get(&region.id))
    else {
        return Vec::new();
    };
    if let Some(binding) = inst
        .interface_bindings
        .iter()
        .find(|binding| binding.child == region.id)
        && let Some(accesses) = translated_interface_binding_accesses(region, variable, binding)
    {
        return accesses
            .into_iter()
            .map(|access| (access.parent, access.array, access.packed))
            .collect();
    }
    if direction == Direction::Output
        && let Some(output) = inst.outputs.iter().find(|output| output.id == region.id)
    {
        let accesses = if let Some(actual) = &output.range_dst {
            translated_contiguous_actual_accesses(region, variable, actual)
        } else {
            translated_fragment_accesses(region, variable, &output.dst, ctx)
        };
        if let Some(accesses) = accesses {
            return accesses
                .into_iter()
                .map(|access| (access.parent, access.array, access.packed))
                .collect();
        }
    }
    if direction == Direction::Input
        && let Some(input) = inst.inputs.iter().find(|input| input.id == region.id)
        && let Some(actual) = &input.range_src
        && let Some(accesses) = translated_contiguous_actual_accesses(region, variable, actual)
    {
        return accesses
            .into_iter()
            .map(|access| (access.parent, access.array, access.packed))
            .collect();
    }
    if let Some((parent, index, select)) = instance_port_region_actual(inst, region.id, direction) {
        return translated_summary_access(region, variable, parent, index, select, ctx)
            .map(|(array, packed, _)| vec![(parent, array, packed)])
            .unwrap_or_default();
    }
    Vec::new()
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

/// Field boundaries of a struct-literal write, as spans on the destination.
///
/// The destination of `x = T'{a: p, b: q}` is one whole variable, so without
/// these the bit partition gives it a single node and `p` and `q` become
/// interchangeable. The read side (`eval_expr_requested`) already answers per
/// field, and `write_assignment_destination` already slices per destination
/// key, so supplying the boundaries is all that is needed for both to line up.
fn collect_struct_field_bounds(
    expr: &Expression,
    dst: PackedSpan,
    id: VarId,
    index: ArraySpan,
    out: &mut HashMap<IdxKey, Vec<PackedSpan>>,
) {
    let Expression::StructConstructor(r#type, fields, _) = expr else {
        return;
    };
    let mut low = dst.start;
    // `fields` is in declaration order whatever order the literal named them
    // in, and the first declared member is the most significant.
    for (name, _) in fields.iter().rev() {
        let Some(width) = r#type.get_member_type(*name).and_then(|m| m.total_width()) else {
            return;
        };
        let Some(span) = PackedSpan::new(low, width) else {
            return;
        };
        out.entry((id, index)).or_default().push(span);
        let Some(next) = low.checked_add(width) else {
            return;
        };
        low = next;
    }
}

fn collect_expr_spans(
    expr: &Expression,
    out: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    calls: &mut Vec<FunctionCall>,
    ctx: &mut Context,
) {
    match expr {
        Expression::Term(t) => collect_factor_spans(t, out, calls, ctx),
        Expression::Unary(_, e, _) => collect_expr_spans(e, out, calls, ctx),
        Expression::Binary(a, _, b, _) => {
            collect_expr_spans(a, out, calls, ctx);
            collect_expr_spans(b, out, calls, ctx);
        }
        Expression::Ternary(a, b, c, _) => {
            collect_expr_spans(a, out, calls, ctx);
            collect_expr_spans(b, out, calls, ctx);
            collect_expr_spans(c, out, calls, ctx);
        }
        Expression::Concatenation(parts, _) => {
            for (a, b) in parts {
                collect_expr_spans(a, out, calls, ctx);
                if let Some(b) = b {
                    collect_expr_spans(b, out, calls, ctx);
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            for (_, e) in fields {
                collect_expr_spans(e, out, calls, ctx);
            }
        }
        Expression::ArrayLiteral(items, _) => {
            for item in items {
                match item {
                    crate::ir::ArrayLiteralItem::Value(value, repeat) => {
                        collect_expr_spans(value, out, calls, ctx);
                        if let Some(repeat) = repeat {
                            collect_expr_spans(repeat, out, calls, ctx);
                        }
                    }
                    crate::ir::ArrayLiteralItem::Defaul(value) => {
                        collect_expr_spans(value, out, calls, ctx);
                    }
                }
            }
        }
    }
}

fn collect_call_spans(
    call: &FunctionCall,
    out: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    calls: &mut Vec<FunctionCall>,
    ctx: &mut Context,
) {
    for expression in call.receiver_index.0.iter().chain(call.inputs.values()) {
        collect_expr_spans(expression, out, calls, ctx);
    }
    for destinations in call.outputs.values() {
        for dst in destinations {
            for expression in dst
                .index
                .0
                .iter()
                .chain(dst.select.0.iter())
                .chain(dst.select.1.iter().map(|(_, x)| x))
            {
                collect_expr_spans(expression, out, calls, ctx);
            }
            for (index, packed) in dst_writes(dst, ctx) {
                out.entry((dst.id, index)).or_default().push(packed);
            }
        }
    }
    calls.push(call.clone());
}

fn collect_factor_spans(
    factor: &Factor,
    out: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    calls: &mut Vec<FunctionCall>,
    ctx: &mut Context,
) {
    match factor {
        Factor::Variable(id, index, select, _) => {
            for (idx, packed) in var_reads(*id, index, select, ctx) {
                out.entry((*id, idx)).or_default().push(packed);
            }
        }
        Factor::FunctionCall(call) => collect_call_spans(call, out, calls, ctx),
        Factor::SystemFunctionCall(call) => match &call.kind {
            SystemFunctionKind::Onehot(input)
            | SystemFunctionKind::Signed(input)
            | SystemFunctionKind::Unsigned(input)
            | SystemFunctionKind::Readmemh(input, _) => {
                collect_expr_spans(&input.0, out, calls, ctx);
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
    calls: &mut Vec<FunctionCall>,
    ctx: &mut Context,
) {
    for statement in statements {
        match statement {
            Statement::Assign(assign) => {
                collect_expr_spans(&assign.expr, out, calls, ctx);
                for destination in &assign.dst {
                    for (index, packed) in dst_writes(destination, ctx) {
                        out.entry((destination.id, index)).or_default().push(packed);
                        collect_struct_field_bounds(
                            &assign.expr,
                            packed,
                            destination.id,
                            index,
                            out,
                        );
                    }
                }
            }
            Statement::If(statement) => {
                collect_expr_spans(&statement.cond, out, calls, ctx);
                collect_statement_spans(&statement.true_side, out, calls, ctx);
                collect_statement_spans(&statement.false_side, out, calls, ctx);
            }
            Statement::Case(statement) => {
                collect_expr_spans(&statement.case_target, out, calls, ctx);
                for arm in &statement.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            crate::ir::CasePattern::Eq(expression) => {
                                collect_expr_spans(expression, out, calls, ctx);
                            }
                            crate::ir::CasePattern::Range { lo, hi, .. } => {
                                collect_expr_spans(lo, out, calls, ctx);
                                collect_expr_spans(hi, out, calls, ctx);
                            }
                        }
                    }
                    collect_statement_spans(&arm.body, out, calls, ctx);
                }
                collect_statement_spans(&statement.default, out, calls, ctx);
            }
            Statement::For(statement) => {
                collect_statement_spans(&statement.body, out, calls, ctx);
            }
            Statement::FunctionCall(call) => collect_call_spans(call, out, calls, ctx),
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
) -> Result<(DependencyGraph, BitPartition, bool), Box<AnalyzerError>> {
    build_module_graph_with_trace(module, summaries, TraceKind::None)
}

fn build_module_graph_with_trace(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    tracing: TraceKind,
) -> Result<(DependencyGraph, BitPartition, bool), Box<AnalyzerError>> {
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.variables.extend(module.interface_members.clone());
    ctx.functions = module.functions.clone();
    let bit_part = build_bit_partition(module, summaries, &mut ctx);
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
    if let Some(token) = oversized.or_else(|| {
        bit_part.position_overflow().map(|id| {
            module
                .variables
                .get(&id)
                .or_else(|| module.interface_members.get(&id))
                .map_or(module.token, |variable| variable.token)
        })
    }) {
        return Err(Box::new(
            AnalyzerError::combinational_loop_position_overflow(&token),
        ));
    }

    let mut builder = ModuleGraphBuilder::new(module, &bit_part, ctx);
    builder.function_summaries.tracing = tracing == TraceKind::Sources;
    builder.trace_instances = tracing != TraceKind::None;

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
    Ok((graph, bit_part, complete))
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
    fn new(module: &'a Module, bit_part: &'a BitPartition, ctx: Context) -> Self {
        Self {
            bit_part,
            graph: DependencyGraph::new(),
            node_map: HashMap::default(),
            ctx,
            procedure_context: procedure::ProcedureContext::new(module),
            function_summaries: procedure::FunctionSummaries::new(module, bit_part),
            summary_budget: ExpansionBudget::new(),
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
        let parent_vars = &module.variables;
        let ctx = &mut self.ctx;
        let procedure_context = &mut self.procedure_context;
        let function_summaries = &mut self.function_summaries;
        let mut actuals = InstanceActuals::default();
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
                if let Some(dependencies) = dependencies {
                    add_procedure_graph(graph, node_map, bit_part, module, dependencies);
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
        let positioned_sources = summary
            .edges
            .iter()
            .filter(|edge| edge.kind.has_position())
            .map(|edge| edge.source)
            .collect::<HashSet<_>>();
        let mut mapped_nodes = Vec::with_capacity(summary.nodes.len());
        let mut endpoint_mappings = Vec::with_capacity(summary.nodes.len());
        for (index, node) in summary.nodes.iter().enumerate() {
            let (mapping, endpoint_mapping) = match node.kind {
                SummaryNodeKind::Input => {
                    let mapping = map_instance_source_region(
                        graph,
                        node_map,
                        inst,
                        child,
                        node.region,
                        positioned_sources.contains(&index),
                        input_reads.get(&node.region.id).map(Vec::as_slice),
                        bit_part,
                        ctx,
                        &mut actuals,
                        &mut self.summary_budget,
                    );
                    (mapping, None)
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
                                true,
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
        if let DependencyDagNode::Replicated { stride } = node
            && let Some(node) = mapped[index]
        {
            // A bounded positive translation represents every copy without
            // expanding bits, repetitions or paths through function imports.
            add_dependency_edge(
                graph,
                node,
                node,
                GraphDependency::unconditional(BitDependency {
                    array: Some(0),
                    packed: Some(*stride),
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
    preserve_position: bool,
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
    if !preserve_position
        || parent_sources
            .nodes
            .iter()
            .any(|source| source.offset.is_some())
    {
        return resolve_instance_mapping(graph, node_map, bit_part, parent_sources);
    }
    let Some(input) = inst.inputs.iter().find(|input| input.id == region.id) else {
        return resolve_instance_mapping(graph, node_map, bit_part, parent_sources);
    };
    let Some(_) = input.single() else {
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
    exhausted: bool,
}

impl InstanceActuals {
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
        ResolvedInstanceRegionMapping {
            nodes: root
                .into_iter()
                .map(|node| ResolvedMappedNode {
                    node,
                    offset: Some((0, 0)),
                    condition: PathCondition::default(),
                })
                .collect(),
        }
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
            let expression = inst
                .inputs
                .iter()
                .find(|input| input.id == first.id)
                .and_then(|input| input.single())
                .expect("deferred inputs have one actual");
            let width = child.variables[&first.id]
                .total_width()
                .expect("deferred inputs have known widths");
            let mut analysis =
                procedure::ExpressionAnalysis::new(bit_part, procedure_context, summaries);
            let dag = analysis.eval_regions(
                expression,
                regions,
                width,
                &child.variables[&first.id].r#type,
                budget.remaining(),
            );
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
    if let (Some(variable), Some(binding)) = (
        variable,
        inst.interface_bindings
            .iter()
            .find(|binding| binding.child == region.id),
    ) && let Some(mapping) =
        map_summary_region_to_interface_binding(region, variable, binding, bit_part)
    {
        return mapping;
    }

    if direction == Direction::Output
        && let (Some(variable), Some(output)) = (
            variable,
            inst.outputs.iter().find(|output| output.id == region.id),
        )
    {
        let mapping = if let Some(actual) = &output.range_dst {
            map_summary_region_to_contiguous_actual(region, variable, actual, bit_part)
        } else {
            map_summary_region_to_fragments(region, variable, &output.dst, bit_part, ctx)
        };
        if let Some(mapping) = mapping {
            return mapping;
        }
    }

    if direction == Direction::Input
        && let (Some(variable), Some(input)) = (
            variable,
            inst.inputs.iter().find(|input| input.id == region.id),
        )
        && let Some(actual) = &input.range_src
        && let Some(mapping) =
            map_summary_region_to_contiguous_actual(region, variable, actual, bit_part)
    {
        return mapping;
    }

    if let Some(variable) = variable
        && let Some((parent, index, select)) =
            instance_port_region_actual(inst, region.id, direction)
    {
        return map_summary_region(region, variable, parent, index, select, bit_part, ctx);
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

#[derive(Clone, Copy)]
struct ActualFragment {
    parent: VarId,
    child_array: ArraySpan,
    child_packed: PackedSpan,
    parent_array: ArraySpan,
    parent_packed: PackedSpan,
}

#[derive(Clone, Copy)]
struct TranslatedFragmentAccess {
    parent: VarId,
    array: ArraySpan,
    packed: PackedSpan,
    offset: (isize, isize),
}

fn actual_fragments(
    child: &Variable,
    actual: &[AssignDestination],
    ctx: &mut Context,
) -> Option<Vec<ActualFragment>> {
    let child_array_length = child.r#type.array.total()?;
    let child_packed_width = child.total_width()?;
    let child_packed = PackedSpan::whole(child_packed_width)?;
    let accesses = actual
        .iter()
        .map(|destination| {
            if !destination.index.is_const() || !destination.select.is_const_with_range() {
                return None;
            }
            let spans = var_reads(destination.id, &destination.index, &destination.select, ctx);
            let [(parent_array, parent_packed)] = spans.as_slice() else {
                return None;
            };
            Some((destination.id, *parent_array, *parent_packed))
        })
        .collect::<Option<Vec<_>>>()?;
    let array_length = accesses.iter().try_fold(0usize, |total, (_, array, _)| {
        total.checked_add(array.length)
    })?;
    if array_length == child_array_length
        && accesses
            .iter()
            .all(|(_, _, packed)| packed.length == child_packed_width)
    {
        let mut child_start = 0usize;
        return accesses
            .into_iter()
            .map(|(parent, parent_array, parent_packed)| {
                let child_array = ArraySpan {
                    start: child_start,
                    length: parent_array.length,
                };
                child_start = child_start.checked_add(parent_array.length)?;
                Some(ActualFragment {
                    parent,
                    child_array,
                    child_packed,
                    parent_array,
                    parent_packed,
                })
            })
            .collect();
    }

    let packed_width = accesses.iter().try_fold(0usize, |total, (_, _, packed)| {
        total.checked_add(packed.length)
    })?;
    if child_array_length != 1
        || packed_width != child_packed_width
        || accesses.iter().any(|(_, array, _)| array.length != 1)
    {
        return None;
    }

    let mut child_start = child_packed_width;
    accesses
        .into_iter()
        .map(|(parent, parent_array, parent_packed)| {
            child_start = child_start.checked_sub(parent_packed.length)?;
            Some(ActualFragment {
                parent,
                child_array: ArraySpan {
                    start: 0,
                    length: 1,
                },
                child_packed: PackedSpan::new(child_start, parent_packed.length)?,
                parent_array,
                parent_packed,
            })
        })
        .collect()
}

fn contiguous_actual_fragment(
    child: &Variable,
    actual: &InstActualFragment,
) -> Option<ActualFragment> {
    let child_array_length = child.r#type.array.total()?;
    let child_packed_width = child.total_width()?;
    if child_array_length != actual.parent_array_length
        || child_packed_width != actual.parent_packed_length
    {
        return None;
    }
    Some(ActualFragment {
        parent: actual.parent,
        child_array: ArraySpan {
            start: 0,
            length: child_array_length,
        },
        child_packed: PackedSpan::whole(child_packed_width)?,
        parent_array: ArraySpan {
            start: actual.parent_array_start,
            length: actual.parent_array_length,
        },
        parent_packed: PackedSpan::new(actual.parent_packed_start, actual.parent_packed_length)?,
    })
}

fn translate_actual_fragments(
    region: SummaryRegion,
    fragments: impl IntoIterator<Item = ActualFragment>,
) -> Option<Vec<TranslatedFragmentAccess>> {
    fragments
        .into_iter()
        .filter_map(|fragment| {
            let array = region.array.intersection(fragment.child_array)?;
            let packed = region.packed.intersection(fragment.child_packed)?;
            Some((fragment, array, packed))
        })
        .map(|(fragment, child_array, child_packed)| {
            let array =
                child_array.translated(fragment.child_array.start, fragment.parent_array.start)?;
            let packed = child_packed
                .translated(fragment.child_packed.start, fragment.parent_packed.start)?;
            Some(TranslatedFragmentAccess {
                parent: fragment.parent,
                array,
                packed,
                offset: (
                    signed_difference(fragment.parent_array.start, fragment.child_array.start)?,
                    signed_difference(fragment.parent_packed.start, fragment.child_packed.start)?,
                ),
            })
        })
        .collect()
}

fn translated_fragment_accesses(
    region: SummaryRegion,
    child: &Variable,
    actual: &[AssignDestination],
    ctx: &mut Context,
) -> Option<Vec<TranslatedFragmentAccess>> {
    let fragments = actual_fragments(child, actual, ctx)?;
    translate_actual_fragments(region, fragments)
}

fn translated_interface_binding_accesses(
    region: SummaryRegion,
    child: &Variable,
    binding: &InstInterfaceBinding,
) -> Option<Vec<TranslatedFragmentAccess>> {
    translate_actual_fragments(
        region,
        [contiguous_actual_fragment(child, &binding.actual)?],
    )
}

fn translated_contiguous_actual_accesses(
    region: SummaryRegion,
    child: &Variable,
    actual: &InstActualFragment,
) -> Option<Vec<TranslatedFragmentAccess>> {
    translate_actual_fragments(region, [contiguous_actual_fragment(child, actual)?])
}

fn map_translated_fragment_accesses(
    accesses: Vec<TranslatedFragmentAccess>,
    bit_part: &BitPartition,
) -> InstanceRegionMapping {
    let mut nodes = Vec::new();
    for access in accesses {
        nodes.extend(
            bit_part
                .overlapping_access(access.parent, access.array, access.packed)
                .into_iter()
                .map(|key| MappedNode {
                    key,
                    offset: Some(access.offset),
                    condition: PathCondition::default(),
                }),
        );
    }
    InstanceRegionMapping { nodes }
}

fn map_summary_region_to_fragments(
    region: SummaryRegion,
    child: &Variable,
    actual: &[AssignDestination],
    bit_part: &BitPartition,
    ctx: &mut Context,
) -> Option<InstanceRegionMapping> {
    let accesses = translated_fragment_accesses(region, child, actual, ctx)?;
    Some(map_translated_fragment_accesses(accesses, bit_part))
}

fn map_summary_region_to_interface_binding(
    region: SummaryRegion,
    child: &Variable,
    binding: &InstInterfaceBinding,
    bit_part: &BitPartition,
) -> Option<InstanceRegionMapping> {
    let accesses = translated_interface_binding_accesses(region, child, binding)?;
    Some(map_translated_fragment_accesses(accesses, bit_part))
}

fn map_summary_region_to_contiguous_actual(
    region: SummaryRegion,
    child: &Variable,
    actual: &InstActualFragment,
    bit_part: &BitPartition,
) -> Option<InstanceRegionMapping> {
    let accesses = translated_contiguous_actual_accesses(region, child, actual)?;
    Some(map_translated_fragment_accesses(accesses, bit_part))
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
    #[test]
    fn array_partition_sweep_keeps_an_access_active_until_its_own_end() {
        let id = VarId::from_raw(0);
        let packed = PackedSpan {
            start: 0,
            length: 1,
        };
        let mut accesses = HashMap::default();
        accesses.insert(
            (
                id,
                ArraySpan {
                    start: 0,
                    length: 2,
                },
            ),
            vec![packed],
        );
        accesses.insert(
            (
                id,
                ArraySpan {
                    start: 1,
                    length: 2,
                },
            ),
            vec![packed],
        );

        let ranges = split_array_spans(accesses, &HashMap::default());
        for start in 0..3 {
            assert_eq!(
                ranges
                    .get(&(id, ArraySpan { start, length: 1 }))
                    .map(Vec::as_slice),
                Some([packed].as_slice())
            );
        }
    }
    #[test]
    fn disjoint_array_point_queries_do_not_scan_every_partition() {
        const COUNT: usize = 16_384;

        let id = VarId::from_raw(0);
        let packed = PackedSpan {
            start: 0,
            length: 32,
        };
        let mut accesses = HashMap::default();
        for start in 0..COUNT {
            accesses.insert((id, ArraySpan { start, length: 1 }), vec![packed]);
        }

        let ranges = split_array_spans(accesses, &HashMap::default());
        let partition = BitPartition::new(ranges);
        assert_eq!(partition.array_spans(id).len(), COUNT);
        for start in 0..COUNT {
            assert_eq!(
                partition.overlapping_access(id, ArraySpan { start, length: 1 }, packed),
                vec![(id, ArraySpan { start, length: 1 }, 0)]
            );
        }
    }
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
