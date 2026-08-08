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
    function_result_region_probe_count, function_result_version_count,
    reset_function_evaluation_count,
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

/// `feedthrough[child_in_id] = { child_out_ids reachable purely combinationally }`.
/// Port-level only -- the parent keeps bit precision via `BitPartition`.
#[derive(Clone, Debug, Default)]
struct ModuleCombSummary {
    feedthrough: HashMap<VarId, HashSet<VarId>>,
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
            let graph = build_module_graph(module, &summaries);
            check_graph(module, &graph, &mut errors);
            let summary = compute_module_summary(module, &graph);
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

fn build_bit_partition(module: &Module, ctx: &mut Context) -> BitPartition {
    let mut accesses: HashMap<IdxKey, Vec<PackedSpan>> = HashMap::default();

    for declaration in &module.declarations {
        if let Declaration::Comb(comb) = declaration {
            collect_statement_spans(&comb.statements, &mut accesses, ctx);
        }
    }

    // Inst input expressions are not represented by procedure statements.
    for inst in walk_insts(module) {
        for inp in &inst.inputs {
            collect_expr_spans(&inp.expr, &mut accesses, ctx);
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

    BitPartition { ranges }
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
        let mut boundaries = Vec::with_capacity(accesses.len() * 2);
        for (span, _) in &accesses {
            if span.length == 0 {
                continue;
            }
            boundaries.push(span.start);
            if let Some(end) = span.end() {
                boundaries.push(end);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        for boundary in boundaries.windows(2) {
            let split = ArraySpan {
                start: boundary[0],
                length: boundary[1] - boundary[0],
            };
            let split_spans = accesses
                .iter()
                .filter(|(access, _)| access.overlaps(split))
                .map(|(_, span)| *span)
                .collect::<Vec<_>>();
            if split_spans.is_empty() {
                continue;
            }
            let parts = atomic_ranges(&split_spans, endpoints.get(&id));
            if !parts.is_empty() {
                ranges.insert((id, split), parts);
            }
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
        Expression::ArrayLiteral(_, _) => {}
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
) -> Graph<NodeKey, ()> {
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.functions = module.functions.clone();
    let bit_part = build_bit_partition(module, &mut ctx);

    let mut graph: Graph<NodeKey, ()> = Graph::new();
    let mut node_map: HashMap<NodeKey, NodeIndex> = HashMap::default();

    for declaration in &module.declarations {
        let Declaration::Comb(comb) = declaration else {
            continue;
        };
        for (source, destination) in procedure::analyze(module, &bit_part, &comb.statements) {
            if !is_module_scope_var(source.0, &module.variables)
                || !is_module_scope_var(destination.0, &module.variables)
            {
                continue;
            }
            let source = ensure_node(&mut graph, &mut node_map, source);
            let destination = ensure_node(&mut graph, &mut node_map, destination);
            graph.add_edge(source, destination, ());
        }
    }

    for inst in walk_insts(module) {
        match inst.component.as_ref() {
            Component::Module(child) => {
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
                    &mut ctx,
                );
            }
            // SV black box: under-detect.
            Component::SystemVerilog(_) => {}
            // Interface signals are already lifted into the parent.
            Component::Interface(_) => {}
        }
    }

    graph
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
    ctx: &mut Context,
) {
    add_sparse_whole_port_copy_edges(inst, child, bit_part, graph, node_map, parent_vars);

    let mut input_reads: HashMap<VarId, Vec<NodeKey>> = HashMap::default();
    for inp in &inst.inputs {
        if !is_pure_input_or_output(inp.id, &child.variables, Direction::Input) {
            continue;
        }
        let mut reads = Vec::new();
        collect_expr_node_keys(&inp.expr, bit_part, &mut reads, ctx);
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
        if !keys.is_empty() {
            output_dsts.insert(out.id, keys);
        }
    }

    for (child_in_id, out_set) in &summary.feedthrough {
        let Some(parent_reads) = input_reads.get(child_in_id) else {
            continue;
        };
        for child_out_id in out_set {
            let Some(parent_dsts) = output_dsts.get(child_out_id) else {
                continue;
            };
            for r in parent_reads {
                for d in parent_dsts {
                    if r == d {
                        continue;
                    }
                    let s = ensure_node(graph, node_map, *r);
                    let t = ensure_node(graph, node_map, *d);
                    graph.add_edge(s, t, ());
                }
            }
        }
    }
}

fn add_sparse_whole_port_copy_edges(
    inst: &InstDeclaration,
    child: &Module,
    bit_part: &BitPartition,
    graph: &mut Graph<NodeKey, ()>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    parent_vars: &HashMap<VarId, Variable>,
) {
    for declaration in &child.declarations {
        let Declaration::Comb(comb) = declaration else {
            continue;
        };
        let [Statement::Assign(assign)] = comb.statements.as_slice() else {
            continue;
        };
        let [destination] = assign.dst.as_slice() else {
            continue;
        };
        if !destination.index.0.is_empty()
            || !destination.select.is_empty()
            || !is_pure_input_or_output(destination.id, &child.variables, Direction::Output)
        {
            continue;
        }
        let Expression::Term(factor) = &assign.expr else {
            continue;
        };
        let Factor::Variable(input_id, input_index, input_select, _) = factor.as_ref() else {
            continue;
        };
        if !input_index.0.is_empty()
            || !input_select.is_empty()
            || !is_pure_input_or_output(*input_id, &child.variables, Direction::Input)
        {
            continue;
        }

        let Some(input) = inst.inputs.iter().find(|input| input.id == *input_id) else {
            continue;
        };
        let Expression::Term(input_factor) = &input.expr else {
            continue;
        };
        let Factor::Variable(parent_input, parent_input_index, parent_input_select, _) =
            input_factor.as_ref()
        else {
            continue;
        };
        if !parent_input_index.0.is_empty() || !parent_input_select.is_empty() {
            continue;
        }

        let Some(output) = inst
            .outputs
            .iter()
            .find(|output| output.id == destination.id)
        else {
            continue;
        };
        let [parent_destination] = output.dst.as_slice() else {
            continue;
        };
        if !parent_destination.index.0.is_empty() || !parent_destination.select.is_empty() {
            continue;
        }
        let parent_output = parent_destination.id;

        let Some(child_input) = child.variables.get(input_id) else {
            continue;
        };
        let Some(child_output) = child.variables.get(&destination.id) else {
            continue;
        };
        let Some(parent_input_variable) = parent_vars.get(parent_input) else {
            continue;
        };
        let Some(parent_output_variable) = parent_vars.get(&parent_output) else {
            continue;
        };
        if child_input.total_width() != child_output.total_width()
            || child_input.r#type.total_array() != child_output.r#type.total_array()
            || parent_input_variable.total_width() != parent_output_variable.total_width()
            || parent_input_variable.r#type.total_array()
                != parent_output_variable.r#type.total_array()
        {
            continue;
        }

        for ((object, index), ranges) in &bit_part.ranges {
            if *object != parent_output {
                continue;
            }
            for (destination_range, span) in ranges.iter().enumerate() {
                let destination_key = (parent_output, *index, destination_range);
                for source_range in bit_part.overlapping((*parent_input, *index), *span) {
                    let source_key = (*parent_input, *index, source_range);
                    let source = ensure_node(graph, node_map, source_key);
                    let destination = ensure_node(graph, node_map, destination_key);
                    graph.add_edge(source, destination, ());
                }
            }
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

fn collect_expr_node_keys(
    expr: &Expression,
    bit_part: &BitPartition,
    out: &mut Vec<NodeKey>,
    ctx: &mut Context,
) {
    match expr {
        Expression::Term(t) => collect_factor_node_keys(t, bit_part, out, ctx),
        Expression::Unary(_, e, _) => collect_expr_node_keys(e, bit_part, out, ctx),
        Expression::Binary(a, _, b, _) => {
            collect_expr_node_keys(a, bit_part, out, ctx);
            collect_expr_node_keys(b, bit_part, out, ctx);
        }
        Expression::Ternary(a, b, c, _) => {
            collect_expr_node_keys(a, bit_part, out, ctx);
            collect_expr_node_keys(b, bit_part, out, ctx);
            collect_expr_node_keys(c, bit_part, out, ctx);
        }
        Expression::Concatenation(parts, _) => {
            for (a, b) in parts {
                collect_expr_node_keys(a, bit_part, out, ctx);
                if let Some(b) = b {
                    collect_expr_node_keys(b, bit_part, out, ctx);
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            for (_, e) in fields {
                collect_expr_node_keys(e, bit_part, out, ctx);
            }
        }
        Expression::ArrayLiteral(_, _) => {}
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

fn compute_module_summary(module: &Module, graph: &Graph<NodeKey, ()>) -> ModuleCombSummary {
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

    let mut feedthrough: HashMap<VarId, HashSet<VarId>> = HashMap::default();
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
                feedthrough.entry(key.0).or_default().insert(nk.0);
            }
            for e in graph.edges(n) {
                stack.push(e.target());
            }
        }
    }
    ModuleCombSummary { feedthrough }
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
