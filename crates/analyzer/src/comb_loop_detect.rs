//! Combinational loop detection on the analyzer IR (issue #931).
//!
//! Builds a per-module dependency graph from statement-ordered SSA summaries,
//! then reports SCCs.
//! Module instance feedthrough is summarized bottom-up in topo order.
//!
//! Under-detect by design: opaque constructs (SystemVerilog black
//! boxes, `inout` ports, recursive functions) add no edges; the
//! simulator's `analyze_dependency` is the backup safety net.

mod procedure;
mod region;
mod ssa;

#[cfg(test)]
pub(crate) use procedure::{
    function_barrier_evaluation_count, function_evaluation_count,
    function_result_region_probe_count, function_result_version_count, module_context_entries,
    reset_function_evaluation_count, reset_module_context_entries,
};

use region::{ArraySpan, BitPartition, IdxKey, NodeKey, PackedSpan, dst_writes, var_reads};

use crate::AnalyzerError;
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    AssignDestination, Component, Declaration, Expression, Factor, FunctionCall, InstDeclaration,
    Ir, Module, Op, Statement, SystemFunctionKind, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction};
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;
use std::collections::VecDeque;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SummaryRegion {
    id: VarId,
    array: ArraySpan,
    packed: PackedSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BitDependency {
    /// `None` means that every source coordinate on this axis may affect the
    /// destination region. `Some(C)` preserves `source + C = destination`.
    array: Option<i128>,
    packed: Option<i128>,
}

impl BitDependency {
    const WHOLE: Self = Self {
        array: None,
        packed: None,
    };

    fn exact_offset(self) -> Option<(i128, i128)> {
        self.array.zip(self.packed)
    }

    fn has_position(self) -> bool {
        self.array.is_some() || self.packed.is_some()
    }

    fn compose(self, next: Self) -> Self {
        Self {
            array: self
                .array
                .zip(next.array)
                .and_then(|(left, right)| left.checked_add(right)),
            packed: self
                .packed
                .zip(next.packed)
                .and_then(|(left, right)| left.checked_add(right)),
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            array: (self.array == other.array).then_some(self.array).flatten(),
            packed: (self.packed == other.packed)
                .then_some(self.packed)
                .flatten(),
        }
    }
}

/// Sparse region-to-region reachability across a module boundary. Endpoints
/// include ordinary ports and interface members captured by imported modport
/// functions.
#[derive(Clone, Debug, Default)]
struct ModuleCombSummary {
    feedthrough: HashMap<SummaryRegion, HashMap<SummaryRegion, BitDependency>>,
}

pub fn check(ir: &Ir) -> Vec<AnalyzerError> {
    let mut errors = Vec::new();
    let mut summaries: HashMap<StrId, ModuleCombSummary> = HashMap::default();

    let order = topo_order_modules(ir);

    for &idx in &order {
        if let Component::Module(module) = &ir.components[idx] {
            // Unevaluable generic parameters do not have a stable procedure.
            if module.suppress_unassigned {
                continue;
            }
            let (graph, bit_part) = build_module_graph(module, &summaries);
            check_graph(module, &graph, &mut errors);
            let summary = compute_module_summary(module, &graph, &bit_part);
            summaries.insert(module.name, summary);
        }
    }

    errors
}

/// Children before parents. Falls back to input order on cycle
/// (`infinite_recursion` is reported separately).
fn topo_order_modules(ir: &Ir) -> Vec<usize> {
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
        return (0..n)
            .filter(|i| matches!(ir.components.get(*i), Some(Component::Module(_))))
            .collect();
    }
    order
}

fn walk_insts(module: &Module) -> impl Iterator<Item = &InstDeclaration> {
    module.declarations.iter().filter_map(|d| match d {
        Declaration::Inst(inst) => Some(inst.as_ref()),
        _ => None,
    })
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

#[derive(Clone, Copy, Debug)]
struct PackedTransfer {
    left_id: VarId,
    left: PackedSpan,
    right_id: VarId,
    right: PackedSpan,
}

#[derive(Clone, Copy, Debug)]
struct PackedTransferEdge {
    index: usize,
    reverse: bool,
    source: PackedSpan,
    destination_id: VarId,
    destination: PackedSpan,
}

fn add_transfer(
    transfers: &mut Vec<PackedTransfer>,
    left_id: VarId,
    left: PackedSpan,
    right_id: VarId,
    right: PackedSpan,
) {
    if left.length == right.length {
        transfers.push(PackedTransfer {
            left_id,
            left,
            right_id,
            right,
        });
    }
}

fn propagate_packed_endpoints(
    accesses: &HashMap<IdxKey, Vec<PackedSpan>>,
    transfers: &[PackedTransfer],
) -> HashMap<VarId, HashSet<usize>> {
    let mut adjacency: HashMap<VarId, Vec<PackedTransferEdge>> = HashMap::default();
    for (index, transfer) in transfers.iter().enumerate() {
        adjacency
            .entry(transfer.left_id)
            .or_default()
            .push(PackedTransferEdge {
                index,
                reverse: false,
                source: transfer.left,
                destination_id: transfer.right_id,
                destination: transfer.right,
            });
        adjacency
            .entry(transfer.right_id)
            .or_default()
            .push(PackedTransferEdge {
                index,
                reverse: true,
                source: transfer.right,
                destination_id: transfer.left_id,
                destination: transfer.left,
            });
    }

    let mut seeds = HashSet::default();
    for ((id, _), spans) in accesses {
        for span in spans {
            seeds.insert((*id, span.start));
            seeds.insert((*id, span.end()));
        }
    }
    for transfer in transfers {
        for (id, point) in [
            (transfer.left_id, transfer.left.start),
            (transfer.left_id, transfer.left.end()),
            (transfer.right_id, transfer.right.start),
            (transfer.right_id, transfer.right.end()),
        ] {
            seeds.insert((id, point));
        }
    }

    // Each observed endpoint crosses each directed relation once. Process all
    // points at the same depth before consuming a direction so converging copy
    // paths retain every arrival. Reusing a direction around an offset cycle
    // would materialize periodic repetitions as one boundary per vector bit.
    let mut endpoints: HashMap<VarId, HashSet<usize>> = HashMap::default();
    for seed in seeds {
        let mut frontier = vec![seed];
        let mut visited = [seed].into_iter().collect::<HashSet<_>>();
        let mut used_directions = HashSet::default();
        while !frontier.is_empty() {
            let mut next = Vec::new();
            let mut used_this_round = HashSet::default();
            for (id, point) in frontier {
                endpoints.entry(id).or_default().insert(point);
                let Some(edges) = adjacency.get(&id) else {
                    continue;
                };
                for edge in edges {
                    let direction = (edge.index, edge.reverse);
                    if point < edge.source.start
                        || point > edge.source.end()
                        || used_directions.contains(&direction)
                    {
                        continue;
                    }
                    let Some(mapped) = point
                        .checked_sub(edge.source.start)
                        .and_then(|offset| edge.destination.start.checked_add(offset))
                    else {
                        continue;
                    };
                    used_this_round.insert(direction);
                    if visited.insert((edge.destination_id, mapped)) {
                        next.push((edge.destination_id, mapped));
                    }
                }
            }
            used_directions.extend(used_this_round);
            frontier = next;
        }
    }
    endpoints
}

fn build_bit_partition(
    module: &Module,
    summaries: &HashMap<StrId, ModuleCombSummary>,
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
            collect_statement_spans(&body.statements, &mut accesses, ctx);
        }
    }

    let transfers = collect_packed_transfers(module, ctx);
    let endpoints = propagate_packed_endpoints(&accesses, &transfers);
    let ranges = split_array_spans(accesses, &endpoints);

    BitPartition::new(ranges)
}

fn collect_instance_summary_spans(
    module: &Module,
    summaries: &HashMap<StrId, ModuleCombSummary>,
    accesses: &mut HashMap<IdxKey, Vec<PackedSpan>>,
    ctx: &mut Context,
) {
    for inst in walk_insts(module) {
        let Component::Module(child) = inst.component.as_ref() else {
            continue;
        };
        let Some(summary) = summaries.get(&child.name) else {
            continue;
        };
        for (source, destinations) in &summary.feedthrough {
            if let Some((parent, array, packed)) =
                summary_parent_access(inst, child, *source, Direction::Input, ctx)
            {
                accesses.entry((parent, array)).or_default().push(packed);
            }
            for destination in destinations.keys() {
                if let Some((parent, array, packed)) =
                    summary_parent_access(inst, child, *destination, Direction::Output, ctx)
                {
                    accesses.entry((parent, array)).or_default().push(packed);
                }
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

fn variable_packed_span(id: VarId, select: &VarSelect, ctx: &mut Context) -> Option<PackedSpan> {
    if !select.is_const_with_range() {
        return None;
    }
    let variable = ctx.variables.get(&id)?.clone();
    let (high, low) = select.eval_value(ctx, &variable.r#type, false)?;
    PackedSpan::from_select(high, low)
}

fn collect_packed_transfers(module: &Module, ctx: &mut Context) -> Vec<PackedTransfer> {
    let mut transfers = Vec::new();
    for declaration in &module.declarations {
        if let Declaration::Comb(comb) = declaration {
            collect_statement_transfers(&comb.statements, ctx, &mut transfers);
        }
    }
    for function in module.functions.values() {
        for body in &function.functions {
            collect_statement_transfers(&body.statements, ctx, &mut transfers);
        }
    }
    transfers.sort_unstable_by_key(|transfer| {
        (
            transfer.left_id,
            transfer.left,
            transfer.right_id,
            transfer.right,
        )
    });
    transfers.dedup_by_key(|transfer| {
        (
            transfer.left_id,
            transfer.left,
            transfer.right_id,
            transfer.right,
        )
    });
    transfers
}

fn collect_statement_transfers(
    statements: &[Statement],
    ctx: &mut Context,
    transfers: &mut Vec<PackedTransfer>,
) {
    for statement in statements {
        match statement {
            Statement::Assign(assign) => {
                collect_expression_calls(&assign.expr, ctx, transfers);
                let destinations = assign
                    .dst
                    .iter()
                    .map(|destination| {
                        variable_packed_span(destination.id, &destination.select, ctx)
                            .map(|span| (destination.id, span))
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(destinations) = destinations else {
                    continue;
                };
                let total_width = destinations.iter().map(|(_, span)| span.length).sum();
                let mut offset = total_width;
                for (id, destination) in destinations {
                    offset -= destination.length;
                    let expression = PackedSpan {
                        start: offset,
                        length: destination.length,
                    };
                    collect_expression_transfers(
                        &assign.expr,
                        expression,
                        id,
                        destination,
                        ctx,
                        transfers,
                    );
                }
            }
            Statement::If(statement) => {
                collect_expression_calls(&statement.cond, ctx, transfers);
                collect_statement_transfers(&statement.true_side, ctx, transfers);
                collect_statement_transfers(&statement.false_side, ctx, transfers);
            }
            Statement::Case(statement) => {
                collect_expression_calls(&statement.case_target, ctx, transfers);
                for arm in &statement.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            crate::ir::CasePattern::Eq(expression) => {
                                collect_expression_calls(expression, ctx, transfers);
                            }
                            crate::ir::CasePattern::Range { lo, hi, .. } => {
                                collect_expression_calls(lo, ctx, transfers);
                                collect_expression_calls(hi, ctx, transfers);
                            }
                        }
                    }
                    collect_statement_transfers(&arm.body, ctx, transfers);
                }
                collect_statement_transfers(&statement.default, ctx, transfers);
            }
            Statement::For(statement) => {
                collect_statement_transfers(&statement.body, ctx, transfers);
            }
            Statement::FunctionCall(call) => collect_call_transfers(call, ctx, transfers),
            Statement::SystemFunctionCall(_)
            | Statement::IfReset(_)
            | Statement::TbMethodCall(_)
            | Statement::Break
            | Statement::Unsupported(_)
            | Statement::Null => {}
        }
    }
}

fn collect_expression_calls(
    expression: &Expression,
    ctx: &mut Context,
    transfers: &mut Vec<PackedTransfer>,
) {
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::FunctionCall(call) => collect_call_transfers(call, ctx, transfers),
            Factor::SystemFunctionCall(call) => match &call.kind {
                SystemFunctionKind::Onehot(input)
                | SystemFunctionKind::Signed(input)
                | SystemFunctionKind::Unsigned(input) => {
                    collect_expression_calls(&input.0, ctx, transfers);
                }
                SystemFunctionKind::Readmemh(input, _) => {
                    collect_expression_calls(&input.0, ctx, transfers);
                }
                _ => {}
            },
            _ => {}
        },
        Expression::Unary(_, operand, _) => collect_expression_calls(operand, ctx, transfers),
        Expression::Binary(left, _, right, _) => {
            collect_expression_calls(left, ctx, transfers);
            collect_expression_calls(right, ctx, transfers);
        }
        Expression::Ternary(condition, left, right, _) => {
            collect_expression_calls(condition, ctx, transfers);
            collect_expression_calls(left, ctx, transfers);
            collect_expression_calls(right, ctx, transfers);
        }
        Expression::Concatenation(parts, _) => {
            for (part, repeat) in parts {
                collect_expression_calls(part, ctx, transfers);
                if let Some(repeat) = repeat {
                    collect_expression_calls(repeat, ctx, transfers);
                }
            }
        }
        Expression::ArrayLiteral(items, _) => {
            for item in items {
                match item {
                    crate::ir::ArrayLiteralItem::Value(value, repeat) => {
                        collect_expression_calls(value, ctx, transfers);
                        if let Some(repeat) = repeat {
                            collect_expression_calls(repeat, ctx, transfers);
                        }
                    }
                    crate::ir::ArrayLiteralItem::Defaul(value) => {
                        collect_expression_calls(value, ctx, transfers);
                    }
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            for (_, value) in fields {
                collect_expression_calls(value, ctx, transfers);
            }
        }
    }
}

fn collect_call_transfers(
    call: &FunctionCall,
    ctx: &mut Context,
    transfers: &mut Vec<PackedTransfer>,
) {
    let body = ctx.functions.get(&call.id).and_then(|function| {
        if let Some(index) = &call.index {
            function.get_function(index)
        } else {
            function.get_function(&[])
        }
    });
    let Some(body) = body else {
        for input in call.inputs.values() {
            collect_expression_calls(input, ctx, transfers);
        }
        return;
    };

    for (path, actual) in &call.inputs {
        collect_expression_calls(actual, ctx, transfers);
        let Some(&formal) = body.arg_map.get(path) else {
            continue;
        };
        let Some(width) = ctx.variables.get(&formal).and_then(Variable::total_width) else {
            continue;
        };
        let Some(span) = PackedSpan::whole(width) else {
            continue;
        };
        collect_expression_transfers(actual, span, formal, span, ctx, transfers);
    }

    for (path, destinations) in &call.outputs {
        let Some(&formal) = body.arg_map.get(path) else {
            continue;
        };
        let destination_spans = destinations
            .iter()
            .map(|destination| {
                variable_packed_span(destination.id, &destination.select, ctx)
                    .map(|span| (destination.id, span))
            })
            .collect::<Option<Vec<_>>>();
        let Some(destination_spans) = destination_spans else {
            continue;
        };
        let total_width = destination_spans.iter().map(|(_, span)| span.length).sum();
        let mut offset = total_width;
        for (destination_id, destination) in destination_spans {
            offset -= destination.length;
            add_transfer(
                transfers,
                formal,
                PackedSpan {
                    start: offset,
                    length: destination.length,
                },
                destination_id,
                destination,
            );
        }
    }
}

fn collect_expression_transfers(
    expression: &Expression,
    requested: PackedSpan,
    target_id: VarId,
    target: PackedSpan,
    ctx: &mut Context,
    transfers: &mut Vec<PackedTransfer>,
) {
    if requested.length != target.length {
        return;
    }
    match expression {
        Expression::Term(factor) => match factor.as_ref() {
            Factor::Variable(id, _, select, _) => {
                let Some(selected) = variable_packed_span(*id, select, ctx) else {
                    return;
                };
                let Some(expression_width) = PackedSpan::whole(selected.length) else {
                    return;
                };
                let Some(valid) = requested.intersection(expression_width) else {
                    return;
                };
                let Some(source) = valid.translated(0, selected.start) else {
                    return;
                };
                let Some(target) = valid.translated(requested.start, target.start) else {
                    return;
                };
                add_transfer(transfers, *id, source, target_id, target);
            }
            Factor::FunctionCall(call) => {
                collect_call_transfers(call, ctx, transfers);
                let body = ctx.functions.get(&call.id).and_then(|function| {
                    if let Some(index) = &call.index {
                        function.get_function(index)
                    } else {
                        function.get_function(&[])
                    }
                });
                if let Some(ret) = body.and_then(|body| body.ret)
                    && let Some(width) = ctx.variables.get(&ret).and_then(Variable::total_width)
                    && let Some(valid) =
                        PackedSpan::whole(width).and_then(|width| requested.intersection(width))
                    && let Some(target) = valid.translated(requested.start, target.start)
                {
                    add_transfer(transfers, ret, valid, target_id, target);
                }
            }
            Factor::SystemFunctionCall(call) => match &call.kind {
                SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                    collect_expression_transfers(
                        &input.0, requested, target_id, target, ctx, transfers,
                    );
                }
                _ => collect_expression_calls(expression, ctx, transfers),
            },
            _ => {}
        },
        Expression::Unary(Op::BitNot | Op::Add | Op::Sub, operand, _) => {
            collect_expression_transfers(operand, requested, target_id, target, ctx, transfers);
        }
        Expression::Binary(left, Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor, right, _) => {
            collect_expression_transfers(left, requested, target_id, target, ctx, transfers);
            collect_expression_transfers(right, requested, target_id, target, ctx, transfers);
        }
        Expression::Binary(left, op, right, _)
            if matches!(
                op,
                Op::LogicShiftL | Op::ArithShiftL | Op::LogicShiftR | Op::ArithShiftR
            ) =>
        {
            let shift = right.eval_value(ctx).and_then(|value| value.to_usize());
            let width = left.comptime().r#type.total_width();
            if let (Some(shift), Some(width)) = (shift, width) {
                let mapped = if matches!(op, Op::LogicShiftL | Op::ArithShiftL) {
                    PackedSpan::new(shift, width)
                        .and_then(|window| requested.intersection(window))
                        .and_then(|valid| Some((valid, valid.translated(shift, 0)?)))
                } else {
                    width
                        .checked_sub(shift)
                        .and_then(PackedSpan::whole)
                        .and_then(|window| requested.intersection(window))
                        .and_then(|valid| Some((valid, valid.translated(0, shift)?)))
                };
                if let Some((valid, source)) = mapped
                    && let Some(target) = valid.translated(requested.start, target.start)
                {
                    collect_expression_transfers(left, source, target_id, target, ctx, transfers);
                }
            }
            collect_expression_calls(right, ctx, transfers);
        }
        Expression::Ternary(_, left, right, _) => {
            collect_expression_transfers(left, requested, target_id, target, ctx, transfers);
            collect_expression_transfers(right, requested, target_id, target, ctx, transfers);
        }
        Expression::Concatenation(parts, _) if parts.iter().all(|(_, repeat)| repeat.is_none()) => {
            let mut low = 0usize;
            for (part, _) in parts.iter().rev() {
                let Some(width) = part.comptime().r#type.total_width() else {
                    return;
                };
                let Some(window) = PackedSpan::new(low, width) else {
                    return;
                };
                if let Some(overlap) = requested.intersection(window)
                    && let Some(local) = overlap.translated(low, 0)
                    && let Some(target) = overlap.translated(requested.start, target.start)
                {
                    collect_expression_transfers(part, local, target_id, target, ctx, transfers);
                }
                low = low.saturating_add(width);
            }
        }
        _ => collect_expression_calls(expression, ctx, transfers),
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
    summaries: &HashMap<StrId, ModuleCombSummary>,
) -> (Graph<NodeKey, BitDependency>, BitPartition) {
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.variables.extend(module.interface_members.clone());
    ctx.functions = module.functions.clone();
    let bit_part = build_bit_partition(module, summaries, &mut ctx);

    let mut graph: Graph<NodeKey, BitDependency> = Graph::new();
    let mut node_map: HashMap<NodeKey, NodeIndex> = HashMap::default();
    let mut function_summaries = procedure::FunctionSummaries::new(module, &bit_part);
    let mut procedure_context = procedure::ProcedureContext::new(module);

    for declaration in &module.declarations {
        let Declaration::Comb(comb) = declaration else {
            continue;
        };
        for dependency in
            procedure::analyze(&bit_part, &comb.statements, &mut procedure_context)
        {
            let source = dependency.source;
            let destination = dependency.destination;
            if !is_module_scope_var(source.0, &module.variables)
                || !is_module_scope_var(destination.0, &module.variables)
            {
                continue;
            }
            let source = ensure_node(&mut graph, &mut node_map, source);
            let destination = ensure_node(&mut graph, &mut node_map, destination);
            graph.add_edge(source, destination, dependency.kind);
        }
    }

    for inst in walk_insts(module) {
        match inst.component.as_ref() {
            Component::Module(child) => {
                let Some(summary) = summaries.get(&child.name) else {
                    continue;
                };
                add_inst_feedthrough_edges(
                    module,
                    inst,
                    child,
                    summary,
                    &bit_part,
                    &mut graph,
                    &mut node_map,
                    &module.variables,
                    &mut ctx,
                    &mut procedure_context,
                    &mut function_summaries,
                );
            }
            // SV black box: under-detect.
            Component::SystemVerilog(_) => {}
            // Interface signals are already lifted into the parent.
            Component::Interface(_) => {}
        }
    }

    (graph, bit_part)
}

#[allow(clippy::too_many_arguments)]
fn add_inst_feedthrough_edges<'a>(
    module: &'a Module,
    inst: &InstDeclaration,
    child: &Module,
    summary: &ModuleCombSummary,
    bit_part: &'a BitPartition,
    graph: &mut Graph<NodeKey, BitDependency>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    parent_vars: &HashMap<VarId, Variable>,
    ctx: &mut Context,
    procedure_context: &mut procedure::ProcedureContext,
    function_summaries: &mut procedure::FunctionSummaries<'a>,
) {
    let mut input_reads: HashMap<VarId, Vec<NodeKey>> = HashMap::default();
    for inp in &inst.inputs {
        if !is_pure_input_or_output(inp.id, &child.variables, Direction::Input) {
            continue;
        }
        let mut reads = Vec::new();
        for expr in &inp.exprs {
            let (sources, dependencies) =
                analyze_instance_actual(bit_part, expr, ctx, procedure_context, function_summaries);
            reads.extend(sources);
            for dependency in dependencies {
                let source = ensure_node(graph, node_map, dependency.source);
                let destination = ensure_node(graph, node_map, dependency.destination);
                graph.add_edge(source, destination, dependency.kind);
            }
        }
        reads.sort_unstable();
        reads.dedup();
        if !reads.is_empty() {
            input_reads.insert(inp.id, reads);
        }
    }

    let mut output_dsts: HashMap<VarId, Vec<NodeKey>> = HashMap::default();
    for out in &inst.outputs {
        if !is_pure_input_or_output(out.id, &child.variables, Direction::Output) {
            continue;
        }
        let mut keys = Vec::new();
        for dst in &out.dst {
            collect_dst_node_keys(dst, bit_part, &mut keys, parent_vars, ctx);
        }
        keys.sort_unstable();
        keys.dedup();
        if !keys.is_empty() {
            output_dsts.insert(out.id, keys);
        }
    }

    for (child_source, destination_set) in &summary.feedthrough {
        for (child_destination, dependency) in destination_set {
            let parent_destinations = instance_region_mapping(
                inst,
                child,
                *child_destination,
                *dependency,
                Direction::Output,
                output_dsts.get(&child_destination.id).map(Vec::as_slice),
                bit_part,
                ctx,
            );

            if let Some((array, packed)) = dependency.exact_offset() {
                let mut fallback_destinations = Vec::new();
                for destination in parent_destinations.nodes {
                    match child_source_region_for_destination(
                        *child_source,
                        *child_destination,
                        array,
                        packed,
                        destination,
                        bit_part,
                    ) {
                        RegionProjection::Exact(source_region) => {
                            let parent_sources = map_instance_source_region(
                                module,
                                inst,
                                child,
                                source_region,
                                *dependency,
                                input_reads.get(&child_source.id).map(Vec::as_slice),
                                bit_part,
                                ctx,
                                function_summaries,
                            );
                            add_mapped_dependency_edges(
                                graph,
                                node_map,
                                bit_part,
                                &parent_sources,
                                &InstanceRegionMapping {
                                    nodes: vec![destination],
                                },
                                *dependency,
                            );
                        }
                        RegionProjection::Disjoint => {}
                        RegionProjection::Unknown => fallback_destinations.push(destination),
                    }
                }
                if fallback_destinations.is_empty() {
                    continue;
                }
                let parent_sources = map_instance_source_region(
                    module,
                    inst,
                    child,
                    *child_source,
                    *dependency,
                    input_reads.get(&child_source.id).map(Vec::as_slice),
                    bit_part,
                    ctx,
                    function_summaries,
                );
                add_mapped_dependency_edges(
                    graph,
                    node_map,
                    bit_part,
                    &parent_sources,
                    &InstanceRegionMapping {
                        nodes: fallback_destinations,
                    },
                    *dependency,
                );
                continue;
            }

            let parent_sources = map_instance_source_region(
                module,
                inst,
                child,
                *child_source,
                *dependency,
                input_reads.get(&child_source.id).map(Vec::as_slice),
                bit_part,
                ctx,
                function_summaries,
            );
            add_mapped_dependency_edges(
                graph,
                node_map,
                bit_part,
                &parent_sources,
                &parent_destinations,
                *dependency,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn map_instance_source_region<'a>(
    module: &'a Module,
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
    dependency: BitDependency,
    allowed: Option<&[NodeKey]>,
    bit_part: &'a BitPartition,
    ctx: &mut Context,
    function_summaries: &mut procedure::FunctionSummaries<'a>,
) -> InstanceRegionMapping {
    let parent_sources = instance_region_mapping(
        inst,
        child,
        region,
        dependency,
        Direction::Input,
        allowed,
        bit_part,
        ctx,
    );
    if !dependency.has_position()
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
        module,
        bit_part,
        expression,
        region,
        width,
        function_summaries,
    );
    mapping
        .nodes
        .retain(|source| allowed.is_some_and(|allowed| allowed.binary_search(&source.key).is_ok()));
    mapping
}

struct InstanceRegionMapping {
    nodes: Vec<MappedNode>,
}

#[derive(Clone, Copy)]
struct MappedNode {
    key: NodeKey,
    offset: Option<(i128, i128)>,
}

enum RegionProjection {
    Exact(SummaryRegion),
    Disjoint,
    Unknown,
}

fn child_source_region_for_destination(
    child_source: SummaryRegion,
    child_destination: SummaryRegion,
    dependency_array: i128,
    dependency_packed: i128,
    destination: MappedNode,
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

fn translate_array_span(span: ArraySpan, offset: i128) -> Option<ArraySpan> {
    let start = translate_position(span.start, offset)?;
    (span.length != 0 && start.checked_add(span.length).is_some()).then_some(ArraySpan {
        start,
        length: span.length,
    })
}

fn translate_packed_span(span: PackedSpan, offset: i128) -> Option<PackedSpan> {
    PackedSpan::new(translate_position(span.start, offset)?, span.length)
}

fn translate_position(position: usize, offset: i128) -> Option<usize> {
    let position = i128::try_from(position).ok()?;
    usize::try_from(position.checked_add(offset)?).ok()
}

#[allow(clippy::too_many_arguments)]
fn instance_region_mapping(
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
    dependency: BitDependency,
    direction: Direction,
    fallback: Option<&[NodeKey]>,
    bit_part: &BitPartition,
    ctx: &mut Context,
) -> InstanceRegionMapping {
    let variable = child
        .variables
        .get(&region.id)
        .or_else(|| child.interface_members.get(&region.id));
    if let Some(variable) = variable
        && (direction == Direction::Input || dependency.has_position())
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
            .copied()
            .map(|key| MappedNode { key, offset: None })
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
            .map(|key| MappedNode { key, offset })
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
) -> Option<(ArraySpan, PackedSpan, (i128, i128))> {
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

fn add_mapped_dependency_edges(
    graph: &mut Graph<NodeKey, BitDependency>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    bit_part: &BitPartition,
    sources: &InstanceRegionMapping,
    destinations: &InstanceRegionMapping,
    dependency: BitDependency,
) {
    for source in &sources.nodes {
        for destination in &destinations.nodes {
            let kind = if let (
                Some((source_array, source_packed)),
                Some((destination_array, destination_packed)),
            ) = (source.offset, destination.offset)
            {
                BitDependency {
                    array: dependency.array.and_then(|array| {
                        array
                            .checked_add(destination_array)?
                            .checked_sub(source_array)
                    }),
                    packed: dependency.packed.and_then(|packed| {
                        packed
                            .checked_add(destination_packed)?
                            .checked_sub(source_packed)
                    }),
                }
            } else {
                BitDependency::WHOLE
            };
            if !node_regions_overlap_with_dependency(source.key, destination.key, kind, bit_part) {
                continue;
            }
            let source = ensure_node(graph, node_map, source.key);
            let destination = ensure_node(graph, node_map, destination.key);
            graph.add_edge(source, destination, kind);
        }
    }
}

fn node_regions_overlap_with_dependency(
    source: NodeKey,
    destination: NodeKey,
    dependency: BitDependency,
    bit_part: &BitPartition,
) -> bool {
    let Some(source_packed) = bit_part.ranges_of((source.0, source.1)).get(source.2) else {
        return false;
    };
    let Some(destination_packed) = bit_part
        .ranges_of((destination.0, destination.1))
        .get(destination.2)
    else {
        return false;
    };
    dependency.array.is_none_or(|array| {
        spans_overlap_with_offset(
            source.1.start,
            source.1.length,
            destination.1.start,
            destination.1.length,
            array,
        )
    }) && dependency.packed.is_none_or(|packed| {
        spans_overlap_with_offset(
            source_packed.start,
            source_packed.length,
            destination_packed.start,
            destination_packed.length,
            packed,
        )
    })
}

fn spans_overlap_with_offset(
    source_start: usize,
    source_length: usize,
    destination_start: usize,
    destination_length: usize,
    offset: i128,
) -> bool {
    let Some(source_start) = i128::try_from(source_start)
        .ok()
        .and_then(|start| start.checked_add(offset))
    else {
        return false;
    };
    let Some(source_end) = i128::try_from(source_length)
        .ok()
        .and_then(|length| source_start.checked_add(length))
    else {
        return false;
    };
    let Some(destination_start) = i128::try_from(destination_start).ok() else {
        return false;
    };
    let Some(destination_end) = i128::try_from(destination_length)
        .ok()
        .and_then(|length| destination_start.checked_add(length))
    else {
        return false;
    };
    source_start < destination_end && destination_start < source_end
}

fn signed_difference(destination: usize, source: usize) -> Option<i128> {
    i128::try_from(destination)
        .ok()?
        .checked_sub(i128::try_from(source).ok()?)
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
) -> (Vec<NodeKey>, Vec<procedure::Dependency>) {
    let mut analysis = InstanceActualAnalysis {
        bit_part,
        ctx,
        procedure_context,
        summaries: Some(summaries),
        procedure: None,
        reads: Vec::new(),
    };
    analysis.eval(expression);
    analysis.reads.sort_unstable();
    analysis.reads.dedup();
    let dependencies = if let Some(mut procedure) = analysis.procedure.take() {
        let dependencies = procedure.dependencies();
        procedure.restore(analysis.procedure_context);
        dependencies
    } else {
        Vec::new()
    };
    (analysis.reads, dependencies)
}

fn analyze_instance_actual_region<'a>(
    module: &'a Module,
    bit_part: &'a BitPartition,
    expression: &Expression,
    region: SummaryRegion,
    context_width: usize,
    summaries: &mut procedure::FunctionSummaries<'a>,
) -> InstanceRegionMapping {
    let mut analysis = procedure::ExpressionAnalysis::new(module, bit_part, summaries);
    InstanceRegionMapping {
        nodes: analysis
            .eval_region(expression, region.array, region.packed, context_width)
            .into_iter()
            .map(|source| MappedNode {
                key: source.key,
                offset: source.offset,
            })
            .collect(),
    }
}

struct InstanceActualAnalysis<'a, 's, 'c> {
    bit_part: &'a BitPartition,
    ctx: &'c mut Context,
    procedure_context: &'c mut procedure::ProcedureContext,
    summaries: Option<&'s mut procedure::FunctionSummaries<'a>>,
    procedure: Option<procedure::ExpressionAnalysis<'a, 's>>,
    reads: Vec<NodeKey>,
}

impl InstanceActualAnalysis<'_, '_, '_> {
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
                    collect_factor_node_keys(factor, self.bit_part, &mut self.reads, self.ctx);
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
            Expression::Ternary(condition, left, right, _) => {
                self.eval(condition);
                match constant_truth(condition, self.ctx) {
                    Some(true) => self.eval(left),
                    Some(false) => self.eval(right),
                    None => {
                        self.eval(left);
                        self.eval(right);
                    }
                }
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

fn check_graph(
    module: &Module,
    graph: &Graph<NodeKey, BitDependency>,
    errors: &mut Vec<AnalyzerError>,
) {
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
    graph: &mut Graph<NodeKey, BitDependency>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    key: NodeKey,
) -> NodeIndex {
    *node_map.entry(key).or_insert_with(|| graph.add_node(key))
}

fn has_self_edge(graph: &Graph<NodeKey, BitDependency>, node: NodeIndex) -> bool {
    let mut offsets = Vec::new();
    for edge in graph
        .edges(node)
        .filter(|edge| edge.source() == node && edge.target() == node)
    {
        if let Some((array, packed)) = edge.weight().exact_offset() {
            if array == 0 && packed == 0 {
                return true;
            }
            offsets.push((array, packed));
        } else {
            return true;
        }
    }
    if offsets.len() <= 1 {
        return false;
    }
    // A closed walk cannot return to its starting position when every edge
    // strictly moves the same coordinate in one direction. Other mixtures are
    // kept conservatively: opposing transfers can compose into a zero offset.
    let increases_one_coordinate = [
        offsets.iter().all(|(array, _)| *array > 0),
        offsets.iter().all(|(array, _)| *array < 0),
        offsets.iter().all(|(_, packed)| *packed > 0),
        offsets.iter().all(|(_, packed)| *packed < 0),
    ]
    .into_iter()
    .any(|monotone| monotone);
    !increases_one_coordinate
}

fn build_error(module: &Module, keys: &[NodeKey]) -> Option<AnalyzerError> {
    let mut tokens: Vec<veryl_parser::token_range::TokenRange> = Vec::new();
    let mut identifier: Option<String> = None;
    let mut seen_var: HashSet<VarId> = HashSet::default();
    for (id, _idx, _range) in keys {
        if !seen_var.insert(*id) {
            continue;
        }
        if let Some(var) = module.variables.get(id)
            && identifier.is_none()
        {
            identifier = Some(var.path.to_string());
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
    graph: &Graph<NodeKey, BitDependency>,
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
    let interface_ids = module
        .interface_members
        .keys()
        .copied()
        .collect::<HashSet<_>>();
    let mut source_ids = input_ids;
    source_ids.extend(interface_ids.iter().copied());
    let mut destination_ids = output_ids;
    destination_ids.extend(interface_ids);

    let mut feedthrough: HashMap<SummaryRegion, HashMap<SummaryRegion, BitDependency>> =
        HashMap::default();
    let mut reached: HashMap<NodeIndex, BitDependency> = HashMap::default();
    let mut queue: VecDeque<NodeIndex> = VecDeque::new();
    for ni in graph.node_indices() {
        let key = graph[ni];
        if !source_ids.contains(&key.0) {
            continue;
        }
        let Some(source) = summary_region(key, bit_part) else {
            continue;
        };
        let mut destinations = Vec::new();
        reached.clear();
        queue.clear();
        for edge in graph.edges(ni) {
            reached
                .entry(edge.target())
                .and_modify(|dependency| *dependency = dependency.union(*edge.weight()))
                .or_insert(*edge.weight());
            queue.push_back(edge.target());
        }
        while let Some(n) = queue.pop_front() {
            let dependency = reached[&n];
            let nk = graph[n];
            if destination_ids.contains(&nk.0)
                && let Some(destination) = summary_region(nk, bit_part)
            {
                destinations.push((destination, dependency));
            }
            for e in graph.edges(n) {
                let next = dependency.compose(*e.weight());
                let changed = match reached.entry(e.target()) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        let merged = entry.get().union(next);
                        if *entry.get() == merged {
                            false
                        } else {
                            entry.insert(merged);
                            true
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(next);
                        true
                    }
                };
                if changed {
                    queue.push_back(e.target());
                }
            }
        }
        let destination_set = feedthrough.entry(source).or_default();
        for (destination, dependency) in coalesce_summary_destinations(destinations) {
            destination_set
                .entry(destination)
                .and_modify(|existing| *existing = existing.union(dependency))
                .or_insert(dependency);
        }
    }
    ModuleCombSummary { feedthrough }
}

fn coalesce_summary_destinations(
    mut destinations: Vec<(SummaryRegion, BitDependency)>,
) -> Vec<(SummaryRegion, BitDependency)> {
    destinations.sort_unstable_by_key(|(destination, _)| *destination);
    let mut merged: Vec<(SummaryRegion, BitDependency)> = Vec::with_capacity(destinations.len());
    for (destination, dependency) in destinations {
        let Some((previous, previous_dependency)) = merged.last_mut() else {
            merged.push((destination, dependency));
            continue;
        };
        if *previous == destination {
            *previous_dependency = previous_dependency.union(dependency);
            continue;
        }
        let adjacent = previous.id == destination.id
            && previous.packed == destination.packed
            && previous_dependency.packed == dependency.packed
            && previous.array.end() == Some(destination.array.start);
        if adjacent
            && let Some(length) = previous.array.length.checked_add(destination.array.length)
        {
            previous.array.length = length;
            *previous_dependency = previous_dependency.union(dependency);
        } else {
            merged.push((destination, dependency));
        }
    }
    merged
}

fn summary_region(key: NodeKey, bit_part: &BitPartition) -> Option<SummaryRegion> {
    let packed = bit_part.ranges_of((key.0, key.1)).get(key.2).copied()?;
    Some(SummaryRegion {
        id: key.0,
        array: key.1,
        packed,
    })
}

#[cfg(test)]
mod region_tests {
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
    fn shifted_transfer_cycle_does_not_enumerate_declared_width() {
        let width = 1_000_000_000;
        let id = VarId::from_raw(0);
        let mut accesses = HashMap::default();
        accesses.insert(
            (
                id,
                ArraySpan {
                    start: 0,
                    length: 1,
                },
            ),
            vec![PackedSpan {
                start: 0,
                length: width,
            }],
        );
        let transfers = [PackedTransfer {
            left_id: id,
            left: PackedSpan {
                start: 0,
                length: width - 1,
            },
            right_id: id,
            right: PackedSpan {
                start: 1,
                length: width - 1,
            },
        }];

        let endpoints = propagate_packed_endpoints(&accesses, &transfers);
        assert_eq!(
            endpoints[&id],
            [0, 1, 2, width - 2, width - 1, width]
                .into_iter()
                .collect::<HashSet<_>>()
        );
    }

    #[test]
    fn endpoint_paths_do_not_hide_distinct_arrivals_at_a_shared_transfer() {
        let x = VarId::from_raw(0);
        let y = VarId::from_raw(1);
        let z = VarId::from_raw(2);
        let w = VarId::from_raw(3);
        let q = VarId::from_raw(4);
        let mut accesses = HashMap::default();
        accesses.insert(
            (
                x,
                ArraySpan {
                    start: 0,
                    length: 1,
                },
            ),
            vec![PackedSpan {
                start: 5,
                length: 1,
            }],
        );
        let aligned = PackedSpan {
            start: 0,
            length: 10,
        };
        let shifted = PackedSpan {
            start: 1,
            length: 10,
        };
        let transfers = [
            PackedTransfer {
                left_id: x,
                left: aligned,
                right_id: y,
                right: aligned,
            },
            PackedTransfer {
                left_id: x,
                left: aligned,
                right_id: z,
                right: shifted,
            },
            PackedTransfer {
                left_id: y,
                left: aligned,
                right_id: w,
                right: aligned,
            },
            PackedTransfer {
                left_id: z,
                left: shifted,
                right_id: w,
                right: shifted,
            },
            PackedTransfer {
                left_id: w,
                left: PackedSpan {
                    start: 0,
                    length: 11,
                },
                right_id: q,
                right: PackedSpan {
                    start: 0,
                    length: 11,
                },
            },
        ];

        let endpoints = propagate_packed_endpoints(&accesses, &transfers);
        let expected = [5, 6, 7].into_iter().collect::<HashSet<_>>();
        assert!(endpoints[&q].is_superset(&expected));
    }
}
