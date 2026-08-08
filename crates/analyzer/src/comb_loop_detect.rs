//! Combinational loop detection on the analyzer IR (issue #931).
//!
//! Builds a per-module dependency graph from statement-ordered SSA summaries,
//! then reports SCCs.
//! Module instance feedthrough is summarized bottom-up in topo order.
//!
//! Under-detect by design: opaque constructs (SystemVerilog black
//! boxes, `inout` ports, recursive functions) add no edges; the
//! simulator's `analyze_dependency` is the backup safety net.

mod ssa;

use crate::AnalyzerError;
use crate::BigUint;
use crate::HashMap;
use crate::HashSet;
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::{
    AssignDestination, Component, Declaration, Expression, Factor, InstDeclaration, Ir, Module,
    Statement, VarIndex, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction};
use crate::value::ValueBigUint;
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;
use std::collections::VecDeque;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ArraySpan {
    start: usize,
    length: usize,
}

impl ArraySpan {
    fn end(self) -> Option<usize> {
        self.start.checked_add(self.length)
    }

    fn overlaps(self, other: Self) -> bool {
        let Some(left_end) = self.end() else {
            return false;
        };
        let Some(right_end) = other.end() else {
            return false;
        };
        self.start < right_end && other.start < left_end
    }
}

/// One split unpacked-array interval. Bit precision lives in the masks.
type IdxKey = (VarId, ArraySpan);

/// `(VarId, array_idx, range_idx)`. `range_idx` indexes the variable's
/// `BitPartition`, so bit-disjoint reads/writes form disjoint nodes.
type NodeKey = (VarId, ArraySpan, usize);

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

    fn overlapping_access(&self, id: VarId, access: ArraySpan, mask: &BigUint) -> Vec<NodeKey> {
        let zero = BigUint::default();
        let mut keys = self
            .ranges
            .iter()
            .filter(|((object, _), _)| *object == id)
            .filter(|((_, split), _)| split.overlaps(access))
            .flat_map(|((_, split), ranges)| {
                ranges
                    .iter()
                    .enumerate()
                    .filter(|(_, range)| (*range & mask) != zero)
                    .map(|(range, _)| (id, *split, range))
            })
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        keys
    }
}

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

/// Group bits into atomic ranges by signature: bits with the same set
/// of containing masks form one range. Bits in zero masks are dropped.
fn atomic_ranges(masks: &[BigUint], _width: usize) -> Vec<BigUint> {
    // Refine only regions which occur in an access mask. This is independent
    // of untouched declared width: each step splits existing disjoint atoms
    // into their intersection and difference with the new mask.
    let mut atoms: Vec<BigUint> = Vec::new();
    for mask in masks {
        let mut remaining = mask.clone();
        let mut refined = Vec::with_capacity(atoms.len() + 1);
        for atom in atoms {
            let overlap = &atom & mask;
            if overlap == BigUint::default() {
                refined.push(atom);
                continue;
            }
            let difference = &atom ^ &overlap;
            if difference != BigUint::default() {
                refined.push(difference);
            }
            refined.push(overlap.clone());
            remaining ^= overlap;
        }
        if remaining != BigUint::default() {
            refined.push(remaining);
        }
        atoms = refined;
    }
    let mut ret = atoms;
    // Stable order by lowest set bit so NodeKey range_idx is deterministic.
    ret.sort_by_key(|m| m.trailing_zeros().unwrap_or(0));
    ret
}

fn build_bit_partition(module: &Module, ctx: &mut Context) -> BitPartition {
    let mut masks: HashMap<IdxKey, Vec<BigUint>> = HashMap::default();

    for declaration in &module.declarations {
        if let Declaration::Comb(comb) = declaration {
            collect_statement_masks(&comb.statements, &mut masks, ctx);
        }
    }

    // Inst input expressions: gather_ff records them but without masks.
    for inst in walk_insts(module) {
        for inp in &inst.inputs {
            collect_expr_masks(&inp.expr, &mut masks, ctx);
        }
        for out in &inst.outputs {
            for dst in &out.dst {
                if let Some((idx, mask)) = eval_dst_mask(dst, &module.variables, ctx) {
                    masks
                        .entry((
                            dst.id,
                            ArraySpan {
                                start: idx,
                                length: 1,
                            },
                        ))
                        .or_default()
                        .push(mask);
                }
            }
        }
    }

    // Function-local regions are not represented by the caller's aggregate
    // reference table. They still need atoms because calls are lowered into
    // the same SSA version graph as their caller.
    for function in module.functions.values() {
        for body in &function.functions {
            collect_statement_masks(&body.statements, &mut masks, ctx);
        }
    }

    let ranges = split_array_masks(module, masks);

    BitPartition { ranges }
}

fn split_array_masks(
    module: &Module,
    masks: HashMap<IdxKey, Vec<BigUint>>,
) -> HashMap<IdxKey, Vec<BigUint>> {
    let mut accesses: HashMap<VarId, Vec<(ArraySpan, BigUint)>> = HashMap::default();
    for ((id, span), masks) in masks {
        for mask in masks {
            accesses.entry(id).or_default().push((span, mask));
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

        let width = module
            .variables
            .get(&id)
            .and_then(Variable::total_width)
            .unwrap_or(1);
        for boundary in boundaries.windows(2) {
            let split = ArraySpan {
                start: boundary[0],
                length: boundary[1] - boundary[0],
            };
            let mut split_masks = accesses
                .iter()
                .filter(|(access, _)| access.overlaps(split))
                .map(|(_, mask)| mask.clone())
                .collect::<Vec<_>>();
            if split_masks.is_empty() {
                continue;
            }
            if width <= 64 {
                split_masks.extend((0..width).map(|bit| BigUint::from(1u32) << bit));
            }
            let parts = atomic_ranges(&split_masks, width);
            if !parts.is_empty() {
                ranges.insert((id, split), parts);
            }
        }
    }
    ranges
}

fn collect_expr_masks(
    expr: &Expression,
    out: &mut HashMap<IdxKey, Vec<BigUint>>,
    ctx: &mut Context,
) {
    match expr {
        Expression::Term(t) => collect_factor_masks(t, out, ctx),
        Expression::Unary(_, e, _) => collect_expr_masks(e, out, ctx),
        Expression::Binary(a, _, b, _) => {
            collect_expr_masks(a, out, ctx);
            collect_expr_masks(b, out, ctx);
        }
        Expression::Ternary(a, b, c, _) => {
            collect_expr_masks(a, out, ctx);
            collect_expr_masks(b, out, ctx);
            collect_expr_masks(c, out, ctx);
        }
        Expression::Concatenation(parts, _) => {
            for (a, b) in parts {
                collect_expr_masks(a, out, ctx);
                if let Some(b) = b {
                    collect_expr_masks(b, out, ctx);
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            for (_, e) in fields {
                collect_expr_masks(e, out, ctx);
            }
        }
        Expression::ArrayLiteral(_, _) => {}
    }
}

fn collect_factor_masks(
    factor: &Factor,
    out: &mut HashMap<IdxKey, Vec<BigUint>>,
    ctx: &mut Context,
) {
    match factor {
        Factor::Variable(id, index, select, _) => {
            for (idx, mask) in var_reads(*id, index, select, ctx) {
                out.entry((*id, idx)).or_default().push(mask);
            }
        }
        Factor::FunctionCall(call) => {
            for input in call.inputs.values() {
                collect_expr_masks(input, out, ctx);
            }
        }
        _ => {}
    }
}

fn collect_statement_masks(
    statements: &[Statement],
    out: &mut HashMap<IdxKey, Vec<BigUint>>,
    ctx: &mut Context,
) {
    for statement in statements {
        match statement {
            Statement::Assign(assign) => {
                collect_expr_masks(&assign.expr, out, ctx);
                for destination in &assign.dst {
                    for (index, mask) in dst_writes(destination, ctx) {
                        out.entry((destination.id, index)).or_default().push(mask);
                    }
                }
            }
            Statement::If(statement) => {
                collect_expr_masks(&statement.cond, out, ctx);
                collect_statement_masks(&statement.true_side, out, ctx);
                collect_statement_masks(&statement.false_side, out, ctx);
            }
            Statement::Case(statement) => {
                collect_expr_masks(&statement.case_target, out, ctx);
                for arm in &statement.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            crate::ir::CasePattern::Eq(expression) => {
                                collect_expr_masks(expression, out, ctx);
                            }
                            crate::ir::CasePattern::Range { lo, hi, .. } => {
                                collect_expr_masks(lo, out, ctx);
                                collect_expr_masks(hi, out, ctx);
                            }
                        }
                    }
                    collect_statement_masks(&arm.body, out, ctx);
                }
                collect_statement_masks(&statement.default, out, ctx);
            }
            Statement::For(statement) => {
                collect_statement_masks(&statement.body, out, ctx);
            }
            Statement::FunctionCall(call) => {
                for input in call.inputs.values() {
                    collect_expr_masks(input, out, ctx);
                }
                for outputs in call.outputs.values() {
                    for destination in outputs {
                        for (index, mask) in dst_writes(destination, ctx) {
                            out.entry((destination.id, index)).or_default().push(mask);
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
fn eval_dst_mask(
    dst: &AssignDestination,
    parent_vars: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Option<(usize, BigUint)> {
    let v = parent_vars.get(&dst.id)?;
    let idx_path = dst.index.eval_value(ctx)?;
    let flat = v.r#type.array.calc_index(&idx_path)?;
    let mask = if let Some((beg, end)) = dst.select.eval_value(ctx, &v.r#type, false) {
        ValueBigUint::gen_mask_range(beg, end)
    } else {
        let width = v.total_width()?;
        ValueBigUint::gen_mask(width)
    };
    Some((flat, mask))
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
        for (source, destination) in ssa::analyze(module, &bit_part, &comb.statements) {
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
            for (destination_range, mask) in ranges.iter().enumerate() {
                let destination_key = (parent_output, *index, destination_range);
                for source_range in bit_part.overlapping((*parent_input, *index), mask) {
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
            for (idx, mask) in var_reads(*id, index, select, ctx) {
                out.extend(bit_part.overlapping_access(*id, idx, &mask));
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
    let Some((idx, mask)) = eval_dst_mask(dst, parent_vars, ctx) else {
        return;
    };
    let span = ArraySpan {
        start: idx,
        length: 1,
    };
    for r in bit_part.overlapping((dst.id, span), &mask) {
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

/// Mirrors the masking logic of `AssignDestination::eval_assign`.
fn dst_writes(dst: &AssignDestination, ctx: &mut Context) -> Vec<(ArraySpan, BigUint)> {
    let Some(variable) = ctx.get_variable_info(dst.id) else {
        return Vec::new();
    };
    let is_select_const = dst.select.is_const();

    let mask = if !is_select_const {
        conservative_select_mask(&dst.select, &variable.r#type, ctx)
    } else {
        let Some((beg, end)) = dst.select.eval_value(ctx, &variable.r#type, false) else {
            return Vec::new();
        };
        ValueBigUint::gen_mask_range(beg, end)
    };

    array_access_span(&dst.index, &variable.r#type, ctx)
        .map(|span| vec![(span, mask)])
        .unwrap_or_default()
}

fn var_reads(
    id: VarId,
    index: &VarIndex,
    select: &VarSelect,
    ctx: &mut Context,
) -> Vec<(ArraySpan, BigUint)> {
    let Some(variable) = ctx.variables.get(&id).cloned() else {
        return Vec::new();
    };
    let mask = if select.is_const_with_range()
        && let Some((beg, end)) = select.eval_value(ctx, &variable.r#type, false)
    {
        ValueBigUint::gen_mask_range(beg, end)
    } else {
        conservative_select_mask(select, &variable.r#type, ctx)
    };
    array_access_span(index, &variable.r#type, ctx)
        .map(|span| vec![(span, mask)])
        .unwrap_or_default()
}

fn array_access_span(
    index: &VarIndex,
    r#type: &crate::ir::Type,
    ctx: &mut Context,
) -> Option<ArraySpan> {
    let prefix_len = index
        .0
        .iter()
        .take_while(|expression| expression.comptime().is_const)
        .count();
    let prefix = VarIndex(index.0[..prefix_len].to_vec());
    let values = prefix.eval_value(ctx)?;
    let (start, inclusive_end) = r#type.array.calc_range(&values)?;
    Some(ArraySpan {
        start,
        length: inclusive_end.checked_sub(start)?.checked_add(1)?,
    })
}

fn conservative_select_mask(
    select: &VarSelect,
    r#type: &crate::ir::Type,
    ctx: &mut Context,
) -> BigUint {
    let prefix = VarSelect(
        select
            .0
            .iter()
            .take_while(|expression| expression.comptime().is_const)
            .cloned()
            .collect(),
        None,
    );
    if let Some((beg, end)) = prefix.eval_value(ctx, r#type, false) {
        ValueBigUint::gen_mask_range(beg, end)
    } else {
        r#type
            .total_width()
            .map(ValueBigUint::gen_mask)
            .unwrap_or_default()
    }
}
