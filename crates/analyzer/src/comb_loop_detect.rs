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
    Ir, Module, Op, Signature, Statement, SystemFunctionKind, VarPath, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction, SymbolId};
use daggy::petgraph::Graph;
use daggy::petgraph::algo::kosaraju_scc;
use daggy::petgraph::graph::{EdgeIndex, NodeIndex};
use daggy::petgraph::visit::EdgeRef;
use std::{collections::VecDeque, rc::Rc};
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SummaryRegion {
    id: VarId,
    array: ArraySpan,
    packed: PackedSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct BitDependency {
    /// `None` means that every source coordinate on this axis may affect the
    /// destination region. `Some(C)` preserves `source + C = destination`.
    array: Option<isize>,
    packed: Option<isize>,
}

impl BitDependency {
    const WHOLE: Self = Self {
        array: None,
        packed: None,
    };

    fn exact_offset(self) -> Option<(isize, isize)> {
        self.array.zip(self.packed)
    }

    fn has_position(self) -> bool {
        self.array.is_some() || self.packed.is_some()
    }

    fn compose(self, next: Self) -> Result<Self, ssa::PositionOverflow> {
        Ok(Self {
            array: self
                .array
                .zip(next.array)
                .map(|(left, right)| left.checked_add(right).ok_or(ssa::PositionOverflow))
                .transpose()?,
            packed: self
                .packed
                .zip(next.packed)
                .map(|(left, right)| left.checked_add(right).ok_or(ssa::PositionOverflow))
                .transpose()?,
        })
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
    complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct DiagnosticEdge {
    index: EdgeIndex,
    source: NodeKey,
    destination: NodeKey,
    dependency: BitDependency,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SummaryEdgeCause {
    inst_token: TokenRange,
    child: Signature,
    child_source: SummaryRegion,
    child_destination: SummaryRegion,
}

#[derive(Clone, Copy, Debug)]
struct LocalEdgeCause {
    declaration: usize,
    source: NodeKey,
    destination: NodeKey,
    dependency: BitDependency,
}

/// Populated only by a diagnostic replay after a loop has been detected. The
/// normal graph and module summaries deliberately remain provenance-free.
#[derive(Clone, Debug, Default)]
struct DiagnosticTrace {
    local: HashMap<EdgeIndex, LocalEdgeCause>,
    summaries: HashMap<EdgeIndex, Vec<SummaryEdgeCause>>,
}

#[cfg(test)]
thread_local! {
    static DIAGNOSTIC_REPLAYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_diagnostic_replay_count() {
    DIAGNOSTIC_REPLAYS.set(0);
}

#[cfg(test)]
pub(crate) fn diagnostic_replay_count() -> usize {
    DIAGNOSTIC_REPLAYS.get()
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
    // Specializations share one declaration; report each of its loops once.
    let mut reported: HashSet<(SymbolId, Vec<VarPath>)> = HashSet::default();

    for module in module_postorder(ir) {
        let (graph, bit_part, module_complete) = match build_module_graph(module, &summaries, None)
        {
            Ok(result) => result,
            Err(error) => {
                errors.push(*error);
                complete = false;
                summaries.insert(module.signature.clone(), ModuleCombSummary::default());
                continue;
            }
        };
        check_graph(
            module,
            &graph,
            &bit_part,
            &summaries,
            &mut diagnostic_replays,
            &mut errors,
            &mut reported,
        );
        let mut summary = match compute_module_summary(module, &graph, &bit_part) {
            Ok(summary) => summary,
            Err(ssa::PositionOverflow) => {
                errors.push(AnalyzerError::combinational_loop_position_overflow(
                    &module.token,
                ));
                complete = false;
                summaries.insert(module.signature.clone(), ModuleCombSummary::default());
                continue;
            }
        };
        summary.complete = module_complete;
        summaries.insert(module.signature.clone(), summary);
        complete &= module_complete;
    }

    (errors, complete)
}

/// Actual instantiated specializations in children-before-parents order.
/// Unevaluable generic templates are not stable bodies and therefore do not
/// claim the same signature as a concrete default specialization.
fn module_postorder(ir: &Ir) -> Vec<&Module> {
    fn visit<'a>(
        module: &'a Module,
        visited: &mut HashSet<Signature>,
        active: &mut HashSet<Signature>,
        order: &mut Vec<&'a Module>,
    ) {
        if module.suppress_unassigned
            || visited.contains(&module.signature)
            || !active.insert(module.signature.clone())
        {
            return;
        }
        for inst in walk_insts(module) {
            if let Component::Module(child) = inst.component.as_ref() {
                visit(child, visited, active, order);
            }
        }
        active.remove(&module.signature);
        visited.insert(module.signature.clone());
        order.push(module);
    }

    let mut visited = HashSet::default();
    let mut active = HashSet::default();
    let mut order = Vec::new();
    for component in &ir.components {
        if let Component::Module(module) = component {
            visit(module, &mut visited, &mut active, &mut order);
        }
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

/// One variable's outgoing relations, indexed for "which spans contain this
/// bit position" queries.
///
/// A variable copied around a wide datapath collects hundreds of relations
/// while any one bit position sits inside only a handful of them, so the
/// traversal below asks instead of scanning.
#[derive(Default)]
struct VarEdges {
    edges: Vec<PackedTransferEdge>,
    /// `max_end[i]` = the largest span end among `edges[..=i]`.
    max_end: Vec<usize>,
}

impl VarEdges {
    fn finish(&mut self) {
        self.edges
            .sort_unstable_by_key(|edge| (edge.source.start, edge.source.end()));
        self.max_end.clear();
        self.max_end.reserve(self.edges.len());
        let mut running = 0usize;
        for edge in &self.edges {
            running = running.max(edge.source.end());
            self.max_end.push(running);
        }
    }

    /// Every relation whose source span contains `point`, in no particular
    /// order.
    fn containing(&self, point: usize) -> impl Iterator<Item = &PackedTransferEdge> {
        let upper = self
            .edges
            .partition_point(|edge| edge.source.start <= point);
        self.edges[..upper]
            .iter()
            .enumerate()
            .rev()
            // No earlier span reaches `point` either, so the walk can stop.
            .take_while(move |(i, _)| self.max_end[*i] >= point)
            .map(|(_, edge)| edge)
            .filter(move |edge| edge.source.end() >= point)
    }
}

fn propagate_packed_endpoints(
    accesses: &HashMap<IdxKey, Vec<PackedSpan>>,
    transfers: &[PackedTransfer],
) -> HashMap<VarId, HashSet<usize>> {
    let mut adjacency: HashMap<VarId, VarEdges> = HashMap::default();
    for (index, transfer) in transfers.iter().enumerate() {
        adjacency
            .entry(transfer.left_id)
            .or_default()
            .edges
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
            .edges
            .push(PackedTransferEdge {
                index,
                reverse: true,
                source: transfer.right,
                destination_id: transfer.left_id,
                destination: transfer.left,
            });
    }
    for edges in adjacency.values_mut() {
        edges.finish();
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
    // Per-seed state, reused across seeds: a design has tens of thousands of
    // them, and rebuilding these each time costs more than the traversal.
    let mut used = vec![false; transfers.len() * 2];
    let mut used_positions: Vec<usize> = Vec::new();
    let mut used_this_round: Vec<usize> = Vec::new();
    let mut visited: HashSet<(VarId, usize)> = HashSet::default();
    let mut frontier: Vec<(VarId, usize)> = Vec::new();
    let mut next: Vec<(VarId, usize)> = Vec::new();
    for seed in seeds {
        frontier.clear();
        frontier.push(seed);
        visited.clear();
        visited.insert(seed);
        while !frontier.is_empty() {
            next.clear();
            used_this_round.clear();
            for &(id, point) in &frontier {
                endpoints.entry(id).or_default().insert(point);
                let Some(edges) = adjacency.get(&id) else {
                    continue;
                };
                for edge in edges.containing(point) {
                    let direction = edge.index * 2 + usize::from(edge.reverse);
                    if used[direction] {
                        continue;
                    }
                    let Some(mapped) = point
                        .checked_sub(edge.source.start)
                        .and_then(|offset| edge.destination.start.checked_add(offset))
                    else {
                        continue;
                    };
                    used_this_round.push(direction);
                    if visited.insert((edge.destination_id, mapped)) {
                        next.push((edge.destination_id, mapped));
                    }
                }
            }
            for &direction in &used_this_round {
                if !used[direction] {
                    used[direction] = true;
                    used_positions.push(direction);
                }
            }
            std::mem::swap(&mut frontier, &mut next);
        }
        for direction in used_positions.drain(..) {
            used[direction] = false;
        }
    }

    endpoints
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
    summaries: &HashMap<Signature, ModuleCombSummary>,
    mut diagnostic_trace: Option<&mut DiagnosticTrace>,
) -> Result<(Graph<NodeKey, BitDependency>, BitPartition, bool), Box<AnalyzerError>> {
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

    let mut graph: Graph<NodeKey, BitDependency> = Graph::new();
    let mut node_map: HashMap<NodeKey, NodeIndex> = HashMap::default();
    let mut function_summaries = procedure::FunctionSummaries::new(module, &bit_part);
    let mut procedure_context = procedure::ProcedureContext::new(module);
    let mut complete = !module
        .variables
        .values()
        .any(|variable| matches!(variable.kind, crate::ir::VarKind::Inout));

    for (declaration_index, declaration) in module.declarations.iter().enumerate() {
        let Declaration::Comb(comb) = declaration else {
            continue;
        };
        let analysis = procedure::analyze(&bit_part, &comb.statements, &mut procedure_context);
        if analysis.position_overflow {
            return Err(Box::new(
                AnalyzerError::combinational_loop_position_overflow(&module.token),
            ));
        }
        if !analysis.complete {
            complete = false;
            continue;
        }
        for dependency in analysis.dependencies {
            let source = dependency.source;
            let destination = dependency.destination;
            if !is_module_scope_var(source.0, &module.variables)
                || !is_module_scope_var(destination.0, &module.variables)
                || is_inout(source.0, &module.variables)
                || is_inout(destination.0, &module.variables)
            {
                continue;
            }
            let source_node = ensure_node(&mut graph, &mut node_map, source);
            let destination_node = ensure_node(&mut graph, &mut node_map, destination);
            let edge = graph.add_edge(source_node, destination_node, dependency.kind);
            if let Some(trace) = diagnostic_trace.as_deref_mut() {
                trace.local.insert(
                    edge,
                    LocalEdgeCause {
                        declaration: declaration_index,
                        source,
                        destination,
                        dependency: dependency.kind,
                    },
                );
            }
        }
    }

    for inst in walk_insts(module) {
        match inst.component.as_ref() {
            Component::Module(child) => {
                let Some(summary) = summaries.get(&child.signature) else {
                    complete = false;
                    continue;
                };
                complete &= summary.complete;
                let mut position_overflow = false;
                complete &= add_inst_feedthrough_edges(
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
                    &mut position_overflow,
                    diagnostic_trace.as_deref_mut(),
                );
                if position_overflow {
                    return Err(Box::new(
                        AnalyzerError::combinational_loop_position_overflow(&module.token),
                    ));
                }
            }
            // SV black box: under-detect.
            Component::SystemVerilog(_) => complete = false,
            // Interface signals are already lifted into the parent.
            Component::Interface(_) => {}
        }
    }

    Ok((graph, bit_part, complete))
}

#[allow(clippy::too_many_arguments)]
fn add_inst_feedthrough_edges<'a>(
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
    position_overflowed: &mut bool,
    mut diagnostic_trace: Option<&mut DiagnosticTrace>,
) -> bool {
    let mut complete = true;
    let mut input_reads: HashMap<VarId, Vec<NodeKey>> = HashMap::default();
    for inp in &inst.inputs {
        if !is_pure_input_or_output(inp.id, &child.variables, Direction::Input) {
            continue;
        }
        let mut reads = Vec::new();
        for expr in &inp.exprs {
            let (sources, dependencies, actual_complete, position_overflow) =
                analyze_instance_actual(bit_part, expr, ctx, procedure_context, function_summaries);
            if position_overflow {
                *position_overflowed = true;
                return false;
            }
            complete &= actual_complete;
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
            let mut destination_keys = Vec::new();
            collect_dst_node_keys(dst, bit_part, &mut destination_keys, parent_vars, ctx);
            let (selector_reads, dependencies, selector_complete, position_overflow) =
                analyze_instance_destination(
                    bit_part,
                    dst,
                    ctx,
                    procedure_context,
                    function_summaries,
                );
            if position_overflow {
                *position_overflowed = true;
                return false;
            }
            complete &= selector_complete;
            for dependency in dependencies {
                let source = ensure_node(graph, node_map, dependency.source);
                let destination = ensure_node(graph, node_map, dependency.destination);
                graph.add_edge(source, destination, dependency.kind);
            }
            for source in selector_reads {
                for destination in &destination_keys {
                    let source = ensure_node(graph, node_map, source);
                    let destination = ensure_node(graph, node_map, *destination);
                    graph.add_edge(source, destination, BitDependency::WHOLE);
                }
            }
            keys.extend(destination_keys);
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
                        RegionProjection::Exact {
                            source: source_region,
                            destination: destination_region,
                        } => {
                            let parent_sources = map_instance_source_region(
                                inst,
                                child,
                                source_region,
                                *dependency,
                                input_reads.get(&child_source.id).map(Vec::as_slice),
                                bit_part,
                                ctx,
                                procedure_context,
                                function_summaries,
                                position_overflowed,
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
                                diagnostic_trace.as_deref_mut(),
                                inst,
                                child,
                                source_region,
                                destination_region,
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
                    inst,
                    child,
                    *child_source,
                    *dependency,
                    input_reads.get(&child_source.id).map(Vec::as_slice),
                    bit_part,
                    ctx,
                    procedure_context,
                    function_summaries,
                    position_overflowed,
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
                    diagnostic_trace.as_deref_mut(),
                    inst,
                    child,
                    *child_source,
                    *child_destination,
                );
                continue;
            }

            let parent_sources = map_instance_source_region(
                inst,
                child,
                *child_source,
                *dependency,
                input_reads.get(&child_source.id).map(Vec::as_slice),
                bit_part,
                ctx,
                procedure_context,
                function_summaries,
                position_overflowed,
            );
            add_mapped_dependency_edges(
                graph,
                node_map,
                bit_part,
                &parent_sources,
                &parent_destinations,
                *dependency,
                diagnostic_trace.as_deref_mut(),
                inst,
                child,
                *child_source,
                *child_destination,
            );
        }
    }
    complete
}

#[allow(clippy::too_many_arguments)]
fn map_instance_source_region<'a>(
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
    dependency: BitDependency,
    allowed: Option<&[NodeKey]>,
    bit_part: &'a BitPartition,
    ctx: &mut Context,
    procedure_context: &mut procedure::ProcedureContext,
    function_summaries: &mut procedure::FunctionSummaries<'a>,
    position_overflowed: &mut bool,
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
    let (mut mapping, position_overflow) = analyze_instance_actual_region(
        bit_part,
        expression,
        region,
        width,
        procedure_context,
        function_summaries,
    );
    *position_overflowed |= position_overflow;
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
    offset: Option<(isize, isize)>,
}

enum RegionProjection {
    Exact {
        source: SummaryRegion,
        destination: SummaryRegion,
    },
    Disjoint,
    Unknown,
}

fn child_source_region_for_destination(
    child_source: SummaryRegion,
    child_destination: SummaryRegion,
    dependency_array: isize,
    dependency_packed: isize,
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
    RegionProjection::Exact {
        source: SummaryRegion {
            id: child_source.id,
            array,
            packed,
        },
        destination: SummaryRegion {
            id: child_destination.id,
            array: child_destination_array,
            packed: child_destination_packed,
        },
    }
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

fn translate_position(position: usize, offset: isize) -> Option<usize> {
    let position = isize::try_from(position).ok()?;
    usize::try_from(position.checked_add(offset)?).ok()
}

#[allow(clippy::too_many_arguments)]
fn instance_region_mapping(
    inst: &InstDeclaration,
    child: &Module,
    region: SummaryRegion,
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

#[allow(clippy::too_many_arguments)]
fn add_mapped_dependency_edges(
    graph: &mut Graph<NodeKey, BitDependency>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
    bit_part: &BitPartition,
    sources: &InstanceRegionMapping,
    destinations: &InstanceRegionMapping,
    dependency: BitDependency,
    mut diagnostic_trace: Option<&mut DiagnosticTrace>,
    inst: &InstDeclaration,
    child: &Module,
    child_source: SummaryRegion,
    child_destination: SummaryRegion,
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
            let source_node = ensure_node(graph, node_map, source.key);
            let destination_node = ensure_node(graph, node_map, destination.key);
            let edge = graph.add_edge(source_node, destination_node, kind);
            if let Some(trace) = diagnostic_trace.as_deref_mut() {
                trace
                    .summaries
                    .entry(edge)
                    .or_default()
                    .push(SummaryEdgeCause {
                        inst_token: inst.token,
                        child: child.signature.clone(),
                        child_source,
                        child_destination,
                    });
            }
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
    offset: isize,
) -> bool {
    let Some(source_start) = isize::try_from(source_start)
        .ok()
        .and_then(|start| start.checked_add(offset))
    else {
        return false;
    };
    let Some(source_end) = isize::try_from(source_length)
        .ok()
        .and_then(|length| source_start.checked_add(length))
    else {
        return false;
    };
    let Some(destination_start) = isize::try_from(destination_start).ok() else {
        return false;
    };
    let Some(destination_end) = isize::try_from(destination_length)
        .ok()
        .and_then(|length| destination_start.checked_add(length))
    else {
        return false;
    };
    source_start < destination_end && destination_start < source_end
}

fn signed_difference(destination: usize, source: usize) -> Option<isize> {
    isize::try_from(destination)
        .ok()?
        .checked_sub(isize::try_from(source).ok()?)
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
) -> (Vec<NodeKey>, Vec<procedure::Dependency>, bool, bool) {
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
) -> (Vec<NodeKey>, Vec<procedure::Dependency>, bool, bool) {
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
) -> (InstanceRegionMapping, bool) {
    let mut analysis = procedure::ExpressionAnalysis::new(bit_part, procedure_context, summaries);
    let mapping = InstanceRegionMapping {
        nodes: analysis
            .eval_region(expression, region.array, region.packed, context_width)
            .into_iter()
            .map(|source| MappedNode {
                key: source.key,
                offset: source.offset,
            })
            .collect(),
    };
    let position_overflow = analysis.position_overflowed();
    analysis.restore(procedure_context);
    (mapping, position_overflow)
}

struct InstanceActualAnalysis<'a, 's, 'c> {
    bit_part: &'a BitPartition,
    ctx: &'c mut Context,
    procedure_context: &'c mut procedure::ProcedureContext,
    summaries: Option<&'s mut procedure::FunctionSummaries<'a>>,
    procedure: Option<procedure::ExpressionAnalysis<'a, 's>>,
    reads: Vec<NodeKey>,
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

    fn finish(mut self) -> (Vec<NodeKey>, Vec<procedure::Dependency>, bool, bool) {
        self.reads.sort_unstable();
        self.reads.dedup();
        let complete = self
            .procedure
            .as_ref()
            .is_none_or(|analysis| analysis.is_complete());
        let mut position_overflow = self
            .procedure
            .as_ref()
            .is_some_and(|analysis| analysis.position_overflowed());
        let dependencies = if let Some(mut analysis) = self.procedure.take() {
            let dependencies = analysis.dependencies();
            position_overflow |= analysis.position_overflowed();
            analysis.restore(self.procedure_context);
            dependencies
        } else {
            Vec::new()
        };
        (self.reads, dependencies, complete, position_overflow)
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
    bit_part: &BitPartition,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    diagnostic_replays: &mut DiagnosticReplayCache,
    errors: &mut Vec<AnalyzerError>,
    reported: &mut HashSet<(SymbolId, Vec<VarPath>)>,
) {
    let sccs = strongly_connected_components(graph);
    let mut seen: HashSet<Vec<NodeKey>> = HashSet::default();
    for scc in sccs {
        let is_loop = scc.len() > 1 || (scc.len() == 1 && has_self_edge(graph, scc[0]));
        if !is_loop {
            continue;
        }
        let cycle = dependency_cycle(graph, &scc);
        let cycle_steps = diagnostic_steps(graph, &cycle);
        let mut keys: Vec<NodeKey> = scc.iter().map(|n| graph[*n]).collect();
        keys.sort();
        if !seen.insert(keys.clone()) {
            continue;
        }
        let cycle_keys = if scc.len() == 1 {
            vec![graph[scc[0]], graph[scc[0]]]
        } else {
            diagnostic_path_nodes(&cycle_steps)
        };
        let Some(error) = build_error(
            module,
            bit_part,
            summaries,
            &keys,
            &cycle_keys,
            &cycle_steps,
            diagnostic_replays,
        ) else {
            continue;
        };
        // `VarId` is fresh per specialization, so paths are the only stable
        // name. `assign_tokens` can name an id that is in neither map; such a
        // cycle has none, so report it rather than collapse it with another.
        let paths = loop_paths(module, &keys);
        if !paths.is_empty() && !reported.insert((module.signature.symbol, paths)) {
            continue;
        }
        errors.push(error);
    }
}

fn loop_paths(module: &Module, keys: &[NodeKey]) -> Vec<VarPath> {
    let mut paths: Vec<VarPath> = keys
        .iter()
        .filter_map(|(id, _, _)| {
            module
                .variables
                .get(id)
                .or_else(|| module.interface_members.get(id))
                .map(|variable| variable.path.clone())
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

fn strongly_connected_components(graph: &Graph<NodeKey, BitDependency>) -> Vec<Vec<NodeIndex>> {
    // Petgraph's Tarjan implementation uses recursive DFS. A long, otherwise
    // shallow dependency chain can therefore exhaust the native stack. The
    // Kosaraju implementation uses explicit worklists for both passes.
    kosaraju_scc(graph)
}

/// Returns the deterministic edges of one directed cycle from an SCC.
fn dependency_cycle(graph: &Graph<NodeKey, BitDependency>, scc: &[NodeIndex]) -> Vec<EdgeIndex> {
    let members: HashSet<NodeIndex> = scc.iter().copied().collect();
    if scc.len() == 1 {
        let edges = sorted_out_edges(graph, scc[0], &members);
        if let Some(edge) = edges.iter().copied().find(|edge| {
            graph[*edge]
                .exact_offset()
                .is_none_or(|(array, packed)| array == 0 && packed == 0)
        }) {
            return vec![edge];
        }
        // `has_self_edge` also accepts opposing exact offsets which compose
        // into a closed walk. Keep all of those edges available to diagnostic
        // provenance while rendering the same `node -> node` cycle as before.
        return edges;
    }

    let start = *scc
        .iter()
        .min_by_key(|node| graph[**node])
        .expect("a detected SCC is never empty");

    for edge in sorted_out_edges(graph, start, &members) {
        let (_, next) = graph
            .edge_endpoints(edge)
            .expect("an outgoing edge has endpoints");
        if next == start {
            continue;
        }
        if let Some(mut path) = shortest_path(graph, next, start, &members) {
            path.insert(0, edge);
            return path;
        }
    }

    // Every node in a non-trivial SCC has a path back to itself through a
    // different node, so this is only reachable if the graph is malformed.
    unreachable!("non-trivial SCC did not contain a directed cycle")
}

fn shortest_path(
    graph: &Graph<NodeKey, BitDependency>,
    start: NodeIndex,
    end: NodeIndex,
    members: &HashSet<NodeIndex>,
) -> Option<Vec<EdgeIndex>> {
    let mut queue = VecDeque::from([start]);
    let mut visited = HashSet::default();
    visited.insert(start);
    let mut predecessor: HashMap<NodeIndex, EdgeIndex> = HashMap::default();

    while let Some(node) = queue.pop_front() {
        if node == end {
            let mut path = Vec::new();
            let mut current = end;
            while current != start {
                let edge = *predecessor.get(&current)?;
                path.push(edge);
                let (source, _) = graph.edge_endpoints(edge)?;
                current = source;
            }
            path.reverse();
            return Some(path);
        }

        for edge in sorted_out_edges(graph, node, members) {
            let (_, next) = graph.edge_endpoints(edge)?;
            if visited.insert(next) {
                predecessor.insert(next, edge);
                queue.push_back(next);
            }
        }
    }
    None
}

fn sorted_out_edges(
    graph: &Graph<NodeKey, BitDependency>,
    node: NodeIndex,
    members: &HashSet<NodeIndex>,
) -> Vec<EdgeIndex> {
    let mut edges: Vec<EdgeIndex> = graph
        .edges(node)
        .filter(|edge| members.contains(&edge.target()))
        .map(|edge| edge.id())
        .collect();
    edges.sort_unstable_by_key(|edge| {
        let (_, target) = graph
            .edge_endpoints(*edge)
            .expect("an outgoing edge has endpoints");
        (graph[target], graph[*edge], edge.index())
    });
    edges
}

fn diagnostic_steps(
    graph: &Graph<NodeKey, BitDependency>,
    edges: &[EdgeIndex],
) -> Vec<DiagnosticEdge> {
    edges
        .iter()
        .filter_map(|edge| {
            let (source, destination) = graph.edge_endpoints(*edge)?;
            Some(DiagnosticEdge {
                index: *edge,
                source: graph[source],
                destination: graph[destination],
                dependency: graph[*edge],
            })
        })
        .collect()
}

fn diagnostic_path_nodes(path: &[DiagnosticEdge]) -> Vec<NodeKey> {
    let Some(first) = path.first() else {
        return Vec::new();
    };
    std::iter::once(first.source)
        .chain(path.iter().map(|edge| edge.destination))
        .collect()
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

fn build_error(
    module: &Module,
    bit_part: &BitPartition,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    keys: &[NodeKey],
    cycle_keys: &[NodeKey],
    cycle: &[DiagnosticEdge],
    diagnostic_replays: &mut DiagnosticReplayCache,
) -> Option<AnalyzerError> {
    let mut identifier: Option<String> = None;
    for (id, _idx, _range) in keys {
        if let Some(var) = module.variables.get(id)
            && identifier.is_none()
        {
            identifier = Some(var.path.to_string());
        }
    }
    let mut tokens = diagnostic_tokens(module, cycle_keys);
    if tokens.is_empty() {
        // A synthetic cycle can lack an assignment site of its own. Preserve
        // the previous best-effort behavior by falling back to its SCC.
        tokens = diagnostic_tokens(module, keys);
    }
    let primary = *tokens.first()?;
    let mut provenance = diagnostic_provenance(module, summaries, cycle, diagnostic_replays);
    provenance.retain(|token| !tokens.contains(token));
    let participants = if provenance.is_empty() {
        tokens.iter().skip(1).copied().collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let cycle = format_cycle(module, bit_part, cycle_keys);
    Some(AnalyzerError::combinational_loop(
        identifier.as_deref().unwrap_or("?"),
        &cycle,
        &primary,
        &participants,
        &provenance,
    ))
}

struct DiagnosticReplay {
    graph: Graph<NodeKey, BitDependency>,
    bit_part: BitPartition,
    trace: DiagnosticTrace,
}

type DiagnosticReplayCache = HashMap<Signature, Rc<DiagnosticReplay>>;

struct DiagnosticTraversal<'a> {
    summaries: &'a HashMap<Signature, ModuleCombSummary>,
    replays: &'a mut DiagnosticReplayCache,
    active: HashSet<Signature>,
    witnesses: Vec<TokenRange>,
}

fn build_diagnostic_replay(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
) -> Option<DiagnosticReplay> {
    #[cfg(test)]
    DIAGNOSTIC_REPLAYS.set(DIAGNOSTIC_REPLAYS.get() + 1);

    let mut trace = DiagnosticTrace::default();
    let (graph, bit_part, _) = build_module_graph(module, summaries, Some(&mut trace)).ok()?;
    for causes in trace.summaries.values_mut() {
        causes.sort_unstable();
        causes.dedup();
    }
    Some(DiagnosticReplay {
        graph,
        bit_part,
        trace,
    })
}

fn cached_diagnostic_replay(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    cache: &mut DiagnosticReplayCache,
) -> Option<Rc<DiagnosticReplay>> {
    if let Some(replay) = cache.get(&module.signature) {
        return Some(Rc::clone(replay));
    }
    let replay = Rc::new(build_diagnostic_replay(module, summaries)?);
    cache.insert(module.signature.clone(), Rc::clone(&replay));
    Some(replay)
}

/// Expand only the edges selected for an already-detected cycle. Module
/// summaries stay compact on the normal path; this replay recovers the
/// internal statements carrying one selected summarized dependency.
fn diagnostic_provenance(
    module: &Module,
    summaries: &HashMap<Signature, ModuleCombSummary>,
    cycle: &[DiagnosticEdge],
    replays: &mut DiagnosticReplayCache,
) -> Vec<TokenRange> {
    if !walk_insts(module).any(|inst| matches!(inst.component.as_ref(), Component::Module(_))) {
        return Vec::new();
    }
    let Some(replay) = cached_diagnostic_replay(module, summaries, replays) else {
        return Vec::new();
    };
    let mut active = HashSet::default();
    active.insert(module.signature.clone());
    let mut traversal = DiagnosticTraversal {
        summaries,
        replays,
        active,
        witnesses: Vec::new(),
    };
    // Expand the first summarized edge in the deterministic reported cycle.
    // Keep every concrete assignment on its recovered path: the deepest
    // assignment is not necessarily the one that should break the loop.
    for edge in cycle {
        if !replay_edge_matches(&replay, *edge) || !replay.trace.summaries.contains_key(&edge.index)
        {
            continue;
        }
        trace_dependency_path(
            module,
            &replay,
            std::slice::from_ref(edge),
            false,
            &mut traversal,
        );
        if !traversal.witnesses.is_empty() {
            break;
        }
    }
    let mut seen = HashSet::default();
    traversal
        .witnesses
        .into_iter()
        .filter(|token| seen.insert(*token))
        .collect()
}

fn replay_edge_matches(replay: &DiagnosticReplay, edge: DiagnosticEdge) -> bool {
    let Some((source, destination)) = replay.graph.edge_endpoints(edge.index) else {
        return false;
    };
    replay.graph[source] == edge.source
        && replay.graph[destination] == edge.destination
        && replay.graph[edge.index] == edge.dependency
}

fn local_dependency_witnesses(
    module: &Module,
    replay: &DiagnosticReplay,
    edge: DiagnosticEdge,
) -> Vec<TokenRange> {
    let Some(cause) = replay.trace.local.get(&edge.index) else {
        return Vec::new();
    };
    let Some(Declaration::Comb(comb)) = module.declarations.get(cause.declaration) else {
        return Vec::new();
    };
    let target = (cause.source, cause.destination, cause.dependency);
    let mut context = procedure::ProcedureContext::new(module);
    let traced =
        procedure::analyze_traced(&replay.bit_part, &comb.statements, &mut context, target);
    traced.get(&target).cloned().unwrap_or_default()
}

fn trace_dependency_path(
    module: &Module,
    replay: &DiagnosticReplay,
    path: &[DiagnosticEdge],
    include_local: bool,
    traversal: &mut DiagnosticTraversal,
) {
    let summaries = traversal.summaries;
    for edge in path {
        let before_local = traversal.witnesses.len();
        if include_local {
            traversal
                .witnesses
                .extend(local_dependency_witnesses(module, replay, *edge));
        }
        if traversal.witnesses.len() != before_local {
            continue;
        }

        let Some(causes) = replay.trace.summaries.get(&edge.index) else {
            continue;
        };
        for cause in causes {
            if !traversal.active.insert(cause.child.clone()) {
                continue;
            }
            let Some(child) = find_child_module(module, cause) else {
                traversal.active.remove(&cause.child);
                continue;
            };
            let Some(child_replay) = cached_diagnostic_replay(child, summaries, traversal.replays)
            else {
                traversal.active.remove(&cause.child);
                continue;
            };
            let Some(child_path) = summary_dependency_path(
                &child_replay.graph,
                &child_replay.bit_part,
                cause.child_source,
                cause.child_destination,
            ) else {
                traversal.active.remove(&cause.child);
                continue;
            };
            let before = traversal.witnesses.len();
            trace_dependency_path(child, &child_replay, &child_path, true, traversal);
            if traversal.witnesses.len() == before
                && let Some(token) = diagnostic_tokens(child, &diagnostic_path_nodes(&child_path))
                    .into_iter()
                    .next()
            {
                traversal.witnesses.push(token);
            }
            traversal.active.remove(&cause.child);
            if traversal.witnesses.len() != before {
                break;
            }
        }
    }
}

fn find_child_module<'a>(module: &'a Module, cause: &SummaryEdgeCause) -> Option<&'a Module> {
    walk_insts(module).find_map(|inst| {
        let Component::Module(child) = inst.component.as_ref() else {
            return None;
        };
        (inst.token == cause.inst_token && child.signature == cause.child).then_some(child)
    })
}

fn summary_dependency_path(
    graph: &Graph<NodeKey, BitDependency>,
    bit_part: &BitPartition,
    source: SummaryRegion,
    destination: SummaryRegion,
) -> Option<Vec<DiagnosticEdge>> {
    let mut sources = graph
        .node_indices()
        .filter(|node| node_overlaps_summary_region(graph[*node], bit_part, source))
        .collect::<Vec<_>>();
    sources.sort_unstable_by_key(|node| graph[*node]);
    let destinations = graph
        .node_indices()
        .filter(|node| node_overlaps_summary_region(graph[*node], bit_part, destination))
        .collect::<HashSet<_>>();
    let members = graph.node_indices().collect::<HashSet<_>>();

    for start in sources {
        if destinations.contains(&start) {
            for edge in sorted_out_edges(graph, start, &members) {
                let (_, next) = graph.edge_endpoints(edge)?;
                if next == start {
                    return Some(diagnostic_steps(graph, &[edge]));
                }
                if let Some(mut path) = shortest_path(graph, next, start, &members) {
                    path.insert(0, edge);
                    return Some(diagnostic_steps(graph, &path));
                }
            }
        }

        let mut queue = VecDeque::from([start]);
        let mut visited = HashSet::default();
        let mut predecessor: HashMap<NodeIndex, EdgeIndex> = HashMap::default();
        visited.insert(start);
        while let Some(node) = queue.pop_front() {
            if node != start && destinations.contains(&node) {
                let mut path = Vec::new();
                let mut current = node;
                while current != start {
                    let edge = *predecessor.get(&current)?;
                    path.push(edge);
                    let (source, _) = graph.edge_endpoints(edge)?;
                    current = source;
                }
                path.reverse();
                return Some(diagnostic_steps(graph, &path));
            }
            for edge in sorted_out_edges(graph, node, &members) {
                let (_, next) = graph.edge_endpoints(edge)?;
                if visited.insert(next) {
                    predecessor.insert(next, edge);
                    queue.push_back(next);
                }
            }
        }
    }
    None
}

fn node_overlaps_summary_region(
    key: NodeKey,
    bit_part: &BitPartition,
    region: SummaryRegion,
) -> bool {
    key.0 == region.id
        && key.1.overlaps(region.array)
        && bit_part
            .ranges_of((key.0, key.1))
            .get(key.2)
            .is_some_and(|packed| packed.overlaps(region.packed))
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

fn compute_module_summary(
    module: &Module,
    graph: &Graph<NodeKey, BitDependency>,
    bit_part: &BitPartition,
) -> Result<ModuleCombSummary, ssa::PositionOverflow> {
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
                let next = dependency.compose(*e.weight())?;
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
    Ok(ModuleCombSummary {
        feedthrough,
        complete: true,
    })
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
    fn dependency_cycle_prefers_node_keys_over_edge_insertion_order() {
        let span = ArraySpan {
            start: 0,
            length: 1,
        };
        let mut graph: Graph<NodeKey, BitDependency> = Graph::new();
        // Deliberately insert nodes and edges in an order that differs from
        // their keys. The diagnostic should still select a -> b -> a.
        let c = graph.add_node((VarId::from_raw(2), span, 0));
        let a = graph.add_node((VarId::from_raw(0), span, 0));
        let b = graph.add_node((VarId::from_raw(1), span, 0));
        graph.add_edge(a, c, BitDependency::WHOLE);
        graph.add_edge(c, a, BitDependency::WHOLE);
        graph.add_edge(a, b, BitDependency::WHOLE);
        graph.add_edge(b, a, BitDependency::WHOLE);

        let scc = strongly_connected_components(&graph).remove(0);
        let cycle = dependency_cycle(&graph, &scc);
        let cycle = diagnostic_path_nodes(&diagnostic_steps(&graph, &cycle));

        assert_eq!(cycle, vec![graph[a], graph[b], graph[a]]);
    }

    #[test]
    fn dependency_cycle_closes_a_self_edge() {
        let mut graph: Graph<NodeKey, BitDependency> = Graph::new();
        let node = graph.add_node((
            VarId::from_raw(0),
            ArraySpan {
                start: 0,
                length: 1,
            },
            0,
        ));
        let edge = graph.add_edge(node, node, BitDependency::WHOLE);

        assert_eq!(dependency_cycle(&graph, &[node]), vec![edge]);
    }

    #[test]
    fn dependency_cycle_prefers_a_closing_parallel_self_edge() {
        let mut graph: Graph<NodeKey, BitDependency> = Graph::new();
        let node = graph.add_node((
            VarId::from_raw(0),
            ArraySpan {
                start: 0,
                length: 1,
            },
            0,
        ));
        graph.add_edge(
            node,
            node,
            BitDependency {
                array: Some(0),
                packed: Some(-1),
            },
        );
        let closing = graph.add_edge(
            node,
            node,
            BitDependency {
                array: Some(0),
                packed: Some(0),
            },
        );

        assert_eq!(dependency_cycle(&graph, &[node]), vec![closing]);
    }

    #[test]
    fn scc_walk_does_not_use_the_native_stack() {
        const COUNT: usize = 100_000;

        let id = VarId::from_raw(0);
        let mut graph: Graph<NodeKey, BitDependency> = Graph::new();
        let mut previous = None;
        for start in 0..COUNT {
            let current = graph.add_node((id, ArraySpan { start, length: 1 }, 0));
            if let Some(previous) = previous {
                graph.add_edge(previous, current, BitDependency::WHOLE);
            }
            previous = Some(current);
        }

        assert_eq!(strongly_connected_components(&graph).len(), COUNT);
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

    #[test]
    fn point_query_answers_exactly_what_a_full_scan_would() {
        let id = VarId::from_raw(0);
        // A long span ahead of short ones: sorting by start alone would stop
        // the walk before reaching it.
        let spans = [(0, 64), (8, 4), (16, 4), (16, 32), (60, 8), (100, 4)];
        let mut edges = VarEdges::default();
        for (index, (start, length)) in spans.into_iter().enumerate() {
            edges.edges.push(PackedTransferEdge {
                index,
                reverse: false,
                source: PackedSpan { start, length },
                destination_id: id,
                destination: PackedSpan {
                    start: 0,
                    length: 1,
                },
            });
        }
        let scan = edges.edges.clone();
        edges.finish();

        for point in 0..=120 {
            let mut indexed: Vec<usize> = edges.containing(point).map(|edge| edge.index).collect();
            indexed.sort_unstable();
            let mut scanned: Vec<usize> = scan
                .iter()
                .filter(|edge| point >= edge.source.start && point <= edge.source.end())
                .map(|edge| edge.index)
                .collect();
            scanned.sort_unstable();
            assert_eq!(indexed, scanned, "point {point}");
        }
    }
}
