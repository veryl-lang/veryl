//! Combinational loop detection on the analyzer IR (issue #931).
//!
//! Builds a per-module dependency graph from statement-ordered SSA summaries,
//! then reports SCCs.
//! Module instance feedthrough is summarized bottom-up in topo order.
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
    ArrayLiteralItem, AssignDestination, CaseStatement, Component, Declaration, Expression, Factor,
    ForBound, ForRange, ForStatement, FunctionCall, IfStatement, InstDeclaration, Ir, Module, Op,
    Statement, SystemFunctionKind, VarIndex, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction};
use crate::value::{Value, ValueBigUint};
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;
use std::collections::VecDeque;
use veryl_parser::resource_table::StrId;

/// One array element. Bit precision lives in the sparse partition masks.
type IdxKey = (VarId, usize);

/// `(VarId, array_idx, range_idx)`. `range_idx` indexes the variable's
/// `BitPartition`, so bit-disjoint reads/writes form disjoint nodes.
type NodeKey = (VarId, usize, usize);

/// Unpacked elements not separated by a constant-index access are represented
/// by one split piece, regardless of the declared array length.
const SPLIT_REMAINDER_INDEX: usize = usize::MAX;

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

    fn overlapping_access(&self, id: VarId, index: usize, mask: &BigUint) -> Vec<NodeKey> {
        if index != SPLIT_REMAINDER_INDEX {
            return self
                .overlapping((id, index), mask)
                .into_iter()
                .map(|range| (id, index, range))
                .collect();
        }

        let zero = BigUint::default();
        let mut keys = self
            .ranges
            .iter()
            .filter(|((object, _), _)| *object == id)
            .flat_map(|((_, split_index), ranges)| {
                ranges
                    .iter()
                    .enumerate()
                    .filter(|(_, range)| (*range & mask) != zero)
                    .map(|(range, _)| (id, *split_index, range))
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
    let mut masks: HashMap<(VarId, usize), Vec<BigUint>> = HashMap::default();

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
                    masks.entry((dst.id, idx)).or_default().push(mask);
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

    let mut ranges: HashMap<(VarId, usize), Vec<BigUint>> = HashMap::default();
    for (key, mut ms) in masks {
        let width = module
            .variables
            .get(&key.0)
            .and_then(|v| v.total_width())
            .unwrap_or(1);
        // Small values are cheap to scalarize and doing so lets an SSA value
        // cross a function boundary without collapsing independently used
        // bits into one version. Wide values remain split only at observed
        // access boundaries.
        if width <= 64 {
            ms.extend((0..width).map(|bit| BigUint::from(1u32) << bit));
        }
        let parts = atomic_ranges(&ms, width);
        if !parts.is_empty() {
            ranges.insert(key, parts);
        }
    }

    BitPartition { ranges }
}

fn collect_expr_masks(
    expr: &Expression,
    out: &mut HashMap<(VarId, usize), Vec<BigUint>>,
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
    out: &mut HashMap<(VarId, usize), Vec<BigUint>>,
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
    out: &mut HashMap<(VarId, usize), Vec<BigUint>>,
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
        for (source, destination) in SsaProcedure::analyze(module, &bit_part, &comb.statements) {
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
    for r in bit_part.overlapping((dst.id, idx), &mask) {
        out.push((dst.id, idx, r));
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

// Minimal statement-ordered SSA used by the loop detector.

type VersionId = usize;

#[derive(Clone)]
enum SsaVersion {
    Entry(NodeKey),
    Definition(Vec<VersionId>),
    Phi(Vec<VersionId>),
}

struct SsaProcedure<'a> {
    bit_part: &'a BitPartition,
    ctx: Context,
    versions: Vec<SsaVersion>,
    entries: HashMap<NodeKey, VersionId>,
    state: HashMap<NodeKey, VersionId>,
    written: HashSet<NodeKey>,
}

impl<'a> SsaProcedure<'a> {
    fn analyze(
        module: &'a Module,
        bit_part: &'a BitPartition,
        statements: &[Statement],
    ) -> Vec<(NodeKey, NodeKey)> {
        let mut ctx = Context::default();
        ctx.variables = module.variables.clone();
        ctx.functions = module.functions.clone();
        let mut this = Self {
            bit_part,
            ctx,
            versions: Vec::new(),
            entries: HashMap::default(),
            state: HashMap::default(),
            written: HashSet::default(),
        };
        this.eval_block(statements, &[]);

        let mut dependencies = Vec::new();
        let destinations: Vec<_> = this.written.iter().copied().collect();
        for destination in destinations {
            let version = this.current_version(destination);
            let mut sources = HashSet::default();
            let mut visited = HashSet::default();
            this.collect_root_sources(version, &mut sources, &mut visited);
            dependencies.extend(sources.into_iter().map(|source| (source, destination)));
        }
        dependencies
    }

    fn entry_version(&mut self, key: NodeKey) -> VersionId {
        if let Some(version) = self.entries.get(&key) {
            return *version;
        }
        let version = self.versions.len();
        self.versions.push(SsaVersion::Entry(key));
        self.entries.insert(key, version);
        version
    }

    fn current_version(&mut self, key: NodeKey) -> VersionId {
        if let Some(version) = self.state.get(&key) {
            *version
        } else {
            let version = self.entry_version(key);
            self.state.insert(key, version);
            version
        }
    }

    fn definition(&mut self, mut sources: Vec<VersionId>) -> VersionId {
        sources.sort_unstable();
        sources.dedup();
        let version = self.versions.len();
        self.versions.push(SsaVersion::Definition(sources));
        version
    }

    fn phi(&mut self, mut inputs: Vec<VersionId>) -> VersionId {
        inputs.sort_unstable();
        inputs.dedup();
        if inputs.len() == 1 {
            return inputs[0];
        }
        let version = self.versions.len();
        self.versions.push(SsaVersion::Phi(inputs));
        version
    }

    fn collect_root_sources(
        &self,
        version: VersionId,
        sources: &mut HashSet<NodeKey>,
        visited: &mut HashSet<(VersionId, bool)>,
    ) {
        match &self.versions[version] {
            // A final LiveOnEntry value is retained state, not a combinational
            // read. Entry versions reached through an explicit definition are.
            SsaVersion::Entry(_) => {}
            SsaVersion::Definition(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, true, sources, visited);
                }
            }
            SsaVersion::Phi(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, false, sources, visited);
                }
            }
        }
    }

    fn collect_sources(
        &self,
        version: VersionId,
        include_entry: bool,
        sources: &mut HashSet<NodeKey>,
        visited: &mut HashSet<(VersionId, bool)>,
    ) {
        if !visited.insert((version, include_entry)) {
            return;
        }
        match &self.versions[version] {
            SsaVersion::Entry(key) => {
                if include_entry {
                    sources.insert(*key);
                }
            }
            SsaVersion::Definition(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, true, sources, visited);
                }
            }
            SsaVersion::Phi(inputs) => {
                for input in inputs {
                    self.collect_sources(*input, include_entry, sources, visited);
                }
            }
        }
    }

    fn read_keys(&mut self, id: VarId, index: &VarIndex, select: &VarSelect) -> Vec<NodeKey> {
        let mut keys = Vec::new();
        for (idx, mask) in var_reads(id, index, select, &mut self.ctx) {
            keys.extend(self.bit_part.overlapping_access(id, idx, &mask));
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn write_keys(&mut self, destination: &AssignDestination) -> Vec<NodeKey> {
        let mut keys = Vec::new();
        for (idx, mask) in dst_writes(destination, &mut self.ctx) {
            keys.extend(self.bit_part.overlapping_access(destination.id, idx, &mask));
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn read_variable(&mut self, id: VarId, index: &VarIndex, select: &VarSelect) -> Vec<VersionId> {
        self.read_keys(id, index, select)
            .into_iter()
            .map(|key| self.current_version(key))
            .collect()
    }

    fn write_destination(
        &mut self,
        destination: &AssignDestination,
        sources: &[VersionId],
        controls: &[VersionId],
    ) {
        let mut dependencies = sources.to_vec();
        dependencies.extend_from_slice(controls);
        for expression in destination
            .index
            .0
            .iter()
            .chain(destination.select.0.iter())
        {
            dependencies.extend(self.eval_expr(expression));
        }
        if let Some((_, expression)) = &destination.select.1 {
            dependencies.extend(self.eval_expr(expression));
        }
        for key in self.write_keys(destination) {
            let version = self.definition(dependencies.clone());
            self.state.insert(key, version);
            self.written.insert(key);
        }
    }

    fn destination_width(&mut self, destination: &AssignDestination) -> Option<usize> {
        let variable = self.ctx.variables.get(&destination.id)?.clone();
        let (high, low) = destination
            .select
            .eval_value(&mut self.ctx, &variable.r#type, false)?;
        high.checked_sub(low)?.checked_add(1)
    }

    fn key_mask(&self, key: NodeKey) -> Option<&BigUint> {
        self.bit_part.ranges_of((key.0, key.1)).get(key.2)
    }

    fn write_assignment_destination(
        &mut self,
        destination: &AssignDestination,
        expression: &Expression,
        expression_offset: usize,
        expression_context_width: usize,
        controls: &[VersionId],
    ) {
        let variable = self.ctx.variables.get(&destination.id).cloned();
        let selected = if destination.select.is_const_with_range() {
            variable.as_ref().and_then(|variable| {
                destination
                    .select
                    .eval_value(&mut self.ctx, &variable.r#type, false)
            })
        } else {
            None
        };
        let keys = self.write_keys(destination);
        for key in keys {
            let mut dependencies = controls.to_vec();
            for selector in destination
                .index
                .0
                .iter()
                .chain(destination.select.0.iter())
            {
                dependencies.extend(self.eval_expr(selector));
            }
            if let Some((_, selector)) = &destination.select.1 {
                dependencies.extend(self.eval_expr(selector));
            }
            if let (Some((_, low)), Some(key_mask)) = (selected, self.key_mask(key).cloned()) {
                let requested = (key_mask >> low) << expression_offset;
                dependencies.extend(self.eval_expr_requested(
                    expression,
                    &requested,
                    expression_context_width,
                ));
            } else {
                dependencies.extend(self.eval_expr(expression));
            }
            let version = self.definition(dependencies);
            self.state.insert(key, version);
            self.written.insert(key);
        }
    }

    /// Returns true when control leaves the current block through `break`.
    fn eval_block(&mut self, statements: &[Statement], controls: &[VersionId]) -> bool {
        for statement in statements {
            if self.eval_statement(statement, controls) {
                return true;
            }
        }
        false
    }

    fn eval_statement(&mut self, statement: &Statement, controls: &[VersionId]) -> bool {
        match statement {
            Statement::Assign(assign) => {
                let widths: Vec<_> = assign
                    .dst
                    .iter()
                    .map(|destination| self.destination_width(destination))
                    .collect();
                if widths.iter().all(Option::is_some) {
                    let total_width = widths.iter().flatten().sum();
                    let mut offset = total_width;
                    for (destination, width) in assign.dst.iter().zip(widths.into_iter()) {
                        let width = width.expect("checked above");
                        offset -= width;
                        self.write_assignment_destination(
                            destination,
                            &assign.expr,
                            offset,
                            total_width,
                            controls,
                        );
                    }
                } else {
                    let sources = self.eval_expr(&assign.expr);
                    for destination in &assign.dst {
                        self.write_destination(destination, &sources, controls);
                    }
                }
                false
            }
            Statement::If(statement) => {
                self.eval_if(statement, controls);
                false
            }
            Statement::Case(statement) => {
                self.eval_case(statement, controls);
                false
            }
            Statement::For(statement) => {
                self.eval_for(statement, controls);
                false
            }
            Statement::FunctionCall(call) => {
                self.eval_call(call, controls);
                false
            }
            Statement::SystemFunctionCall(call) => {
                self.eval_system_call(call, controls, false);
                false
            }
            Statement::Break => true,
            Statement::IfReset(_)
            | Statement::TbMethodCall(_)
            | Statement::Unsupported(_)
            | Statement::Null => false,
        }
    }

    fn eval_if(&mut self, statement: &IfStatement, controls: &[VersionId]) {
        let condition = self.eval_expr(&statement.cond);
        let mut nested_controls = controls.to_vec();
        nested_controls.extend_from_slice(&condition);
        let saved_state = self.state.clone();
        let saved_written = self.written.clone();

        self.eval_block(&statement.true_side, &nested_controls);
        let true_state = self.state.clone();
        let true_written = self.written.clone();

        self.state = saved_state.clone();
        self.written = saved_written;
        self.eval_block(&statement.false_side, &nested_controls);
        let false_state = self.state.clone();
        let false_written = self.written.clone();

        self.merge_states(&saved_state, &[true_state, false_state]);
        self.written = true_written;
        self.written.extend(false_written);
    }

    fn eval_case(&mut self, statement: &CaseStatement, controls: &[VersionId]) {
        let mut condition = self.eval_expr(&statement.case_target);
        for arm in &statement.arms {
            for pattern in &arm.patterns {
                match pattern {
                    crate::ir::CasePattern::Eq(expression) => {
                        condition.extend(self.eval_expr(expression));
                    }
                    crate::ir::CasePattern::Range { lo, hi, .. } => {
                        condition.extend(self.eval_expr(lo));
                        condition.extend(self.eval_expr(hi));
                    }
                }
            }
        }
        let mut nested_controls = controls.to_vec();
        nested_controls.extend(condition);
        let saved_state = self.state.clone();
        let saved_written = self.written.clone();
        let mut states = Vec::with_capacity(statement.arms.len() + 1);
        let mut written = saved_written.clone();
        for arm in &statement.arms {
            self.state = saved_state.clone();
            self.written = saved_written.clone();
            self.eval_block(&arm.body, &nested_controls);
            states.push(self.state.clone());
            written.extend(self.written.iter().copied());
        }
        self.state = saved_state.clone();
        self.written = saved_written;
        self.eval_block(&statement.default, &nested_controls);
        states.push(self.state.clone());
        written.extend(self.written.iter().copied());
        self.merge_states(&saved_state, &states);
        self.written = written;
    }

    fn merge_states(
        &mut self,
        base: &HashMap<NodeKey, VersionId>,
        states: &[HashMap<NodeKey, VersionId>],
    ) {
        let mut keys: HashSet<NodeKey> = base.keys().copied().collect();
        for state in states {
            keys.extend(state.keys().copied());
        }
        let mut merged = HashMap::default();
        for key in keys {
            let fallback = base
                .get(&key)
                .copied()
                .unwrap_or_else(|| self.entry_version(key));
            let inputs = states
                .iter()
                .map(|state| state.get(&key).copied().unwrap_or(fallback))
                .collect();
            merged.insert(key, self.phi(inputs));
        }
        self.state = merged;
    }

    fn eval_for(&mut self, statement: &ForStatement, controls: &[VersionId]) {
        let mut range_controls = controls.to_vec();
        let bounds = match &statement.range {
            ForRange::Forward { start, end, .. }
            | ForRange::Reverse { start, end, .. }
            | ForRange::Stepped { start, end, .. } => [start, end],
        };
        for bound in bounds {
            if let ForBound::Expression(expression) = bound {
                range_controls.extend(self.eval_expr(expression));
            }
        }

        if let Some(iterations) = statement.range.eval_iter(&mut self.ctx) {
            for value in iterations {
                if let Some(variable) = self.ctx.variables.get_mut(&statement.var_id)
                    && let Some(width) = statement.var_type.total_width()
                {
                    variable.set_value(
                        &[],
                        Value::new(value as u64, width, statement.var_type.signed),
                        None,
                    );
                }
                if self.eval_block(&statement.body, &range_controls) {
                    break;
                }
            }
            return;
        }

        // Runtime loops have a zero-trip path. One symbolic body traversal is
        // enough to expose all explicit reads; the exit phi keeps LiveOnEntry
        // separate so retained state does not become a loop edge.
        let saved_state = self.state.clone();
        let saved_written = self.written.clone();
        self.eval_block(&statement.body, &range_controls);
        let body_state = self.state.clone();
        let body_written = self.written.clone();
        self.merge_states(&saved_state, &[saved_state.clone(), body_state]);
        self.written = saved_written;
        self.written.extend(body_written);
    }

    fn eval_expr_requested(
        &mut self,
        expression: &Expression,
        requested: &BigUint,
        context_width: usize,
    ) -> Vec<VersionId> {
        let expression_width = expression
            .comptime()
            .r#type
            .total_width()
            .unwrap_or(context_width);
        let low_mask = requested & ValueBigUint::gen_mask(expression_width);
        let mut reads = self.eval_expr_bits(expression, &low_mask);
        if context_width > expression_width
            && expression.comptime().r#type.signed
            && (requested >> expression_width) != BigUint::default()
            && expression_width != 0
        {
            reads.extend(
                self.eval_expr_bits(expression, &(BigUint::from(1u32) << (expression_width - 1))),
            );
        }
        reads.sort_unstable();
        reads.dedup();
        reads
    }

    fn eval_expr_bits(&mut self, expression: &Expression, requested: &BigUint) -> Vec<VersionId> {
        if *requested == BigUint::default() {
            return Vec::new();
        }
        match expression {
            Expression::Term(factor) => match factor.as_ref() {
                Factor::Variable(id, index, select, _) => {
                    let variable = self.ctx.variables.get(id).cloned();
                    let selected = if select.is_const_with_range() {
                        variable.as_ref().and_then(|variable| {
                            select.eval_value(&mut self.ctx, &variable.r#type, false)
                        })
                    } else {
                        None
                    };
                    if let Some((_, low)) = selected {
                        let source_mask = requested << low;
                        let mut reads = Vec::new();
                        for (idx, mask) in var_reads(*id, index, select, &mut self.ctx) {
                            let source_mask = &source_mask & mask;
                            for key in self.bit_part.overlapping_access(*id, idx, &source_mask) {
                                reads.push(self.current_version(key));
                            }
                        }
                        reads
                    } else {
                        self.read_variable(*id, index, select)
                    }
                }
                Factor::SystemFunctionCall(call) => match &call.kind {
                    SystemFunctionKind::Signed(input) | SystemFunctionKind::Unsigned(input) => {
                        self.eval_expr_bits(&input.0, requested)
                    }
                    _ => self.eval_system_call(call, &[], true),
                },
                Factor::FunctionCall(call) => self.eval_call(call, &[]),
                Factor::HierVariable(_)
                | Factor::Value(_)
                | Factor::Anonymous(_)
                | Factor::Unknown(_) => Vec::new(),
            },
            Expression::Unary(op, operand, _) => match op {
                Op::BitNot | Op::Add | Op::Sub => self.eval_expr_bits(operand, requested),
                _ => self.eval_expr(operand),
            },
            Expression::Binary(left, op, right, _) => match op {
                Op::LogicShiftL | Op::ArithShiftL => {
                    let shift = right
                        .eval_value(&mut self.ctx)
                        .and_then(|value| value.to_usize());
                    let mut reads = self.eval_expr(right);
                    if let Some(shift) = shift {
                        reads.extend(self.eval_expr_bits(left, &(requested >> shift)));
                    } else {
                        reads.extend(self.eval_expr(left));
                    }
                    reads
                }
                Op::LogicShiftR | Op::ArithShiftR => {
                    let shift = right
                        .eval_value(&mut self.ctx)
                        .and_then(|value| value.to_usize());
                    let mut reads = self.eval_expr(right);
                    if let Some(shift) = shift {
                        let width = left.comptime().r#type.total_width().unwrap_or(0);
                        let shifted = requested << shift;
                        reads.extend(
                            self.eval_expr_bits(left, &(&shifted & ValueBigUint::gen_mask(width))),
                        );
                        if *op == Op::ArithShiftR
                            && left.comptime().r#type.signed
                            && width != 0
                            && (&shifted >> width) != BigUint::default()
                        {
                            reads.extend(
                                self.eval_expr_bits(left, &(BigUint::from(1u32) << (width - 1))),
                            );
                        }
                    } else {
                        reads.extend(self.eval_expr(left));
                    }
                    reads
                }
                Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitXnor => {
                    let mut reads = self.eval_expr_bits(left, requested);
                    reads.extend(self.eval_expr_bits(right, requested));
                    reads
                }
                _ => self.eval_expr(expression),
            },
            Expression::Ternary(condition, left, right, _) => {
                let mut reads = self.eval_expr(condition);
                reads.extend(self.eval_expr_bits(left, requested));
                reads.extend(self.eval_expr_bits(right, requested));
                reads
            }
            Expression::Concatenation(parts, _) => {
                if parts.iter().any(|(_, repeat)| repeat.is_some()) {
                    return self.eval_expr(expression);
                }
                let mut low = 0usize;
                let mut reads = Vec::new();
                for (part, _) in parts.iter().rev() {
                    let Some(width) = part.comptime().r#type.total_width() else {
                        return self.eval_expr(expression);
                    };
                    let local = (requested >> low) & ValueBigUint::gen_mask(width);
                    reads.extend(self.eval_expr_bits(part, &local));
                    low = low.saturating_add(width);
                }
                reads
            }
            Expression::ArrayLiteral(_, _) | Expression::StructConstructor(_, _, _) => {
                self.eval_expr(expression)
            }
        }
    }

    fn eval_expr(&mut self, expression: &Expression) -> Vec<VersionId> {
        let mut reads = Vec::new();
        match expression {
            Expression::Term(factor) => self.eval_factor(factor, &mut reads),
            Expression::Unary(_, expression, _) => reads.extend(self.eval_expr(expression)),
            Expression::Binary(left, _, right, _) => {
                reads.extend(self.eval_expr(left));
                reads.extend(self.eval_expr(right));
            }
            Expression::Ternary(condition, left, right, _) => {
                reads.extend(self.eval_expr(condition));
                reads.extend(self.eval_expr(left));
                reads.extend(self.eval_expr(right));
            }
            Expression::Concatenation(parts, _) => {
                for (part, repeat) in parts {
                    reads.extend(self.eval_expr(part));
                    if let Some(repeat) = repeat {
                        reads.extend(self.eval_expr(repeat));
                    }
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        ArrayLiteralItem::Value(value, repeat) => {
                            reads.extend(self.eval_expr(value));
                            if let Some(repeat) = repeat {
                                reads.extend(self.eval_expr(repeat));
                            }
                        }
                        ArrayLiteralItem::Defaul(value) => reads.extend(self.eval_expr(value)),
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, value) in fields {
                    reads.extend(self.eval_expr(value));
                }
            }
        }
        reads.sort_unstable();
        reads.dedup();
        reads
    }

    fn eval_factor(&mut self, factor: &Factor, reads: &mut Vec<VersionId>) {
        match factor {
            Factor::Variable(id, index, select, _) => {
                for expression in index.0.iter().chain(select.0.iter()) {
                    reads.extend(self.eval_expr(expression));
                }
                if let Some((_, expression)) = &select.1 {
                    reads.extend(self.eval_expr(expression));
                }
                reads.extend(self.read_variable(*id, index, select));
            }
            Factor::FunctionCall(call) => reads.extend(self.eval_call(call, &[])),
            Factor::SystemFunctionCall(call) => {
                reads.extend(self.eval_system_call(call, &[], true));
            }
            Factor::HierVariable(_)
            | Factor::Value(_)
            | Factor::Anonymous(_)
            | Factor::Unknown(_) => {}
        }
    }

    fn eval_call(&mut self, call: &FunctionCall, controls: &[VersionId]) -> Vec<VersionId> {
        let body = self.ctx.functions.get(&call.id).and_then(|function| {
            if let Some(index) = &call.index {
                function.get_function(index)
            } else {
                function.get_function(&[])
            }
        });
        let Some(body) = body else {
            let mut sources = Vec::new();
            for input in call.inputs.values() {
                sources.extend(self.eval_expr(input));
            }
            for outputs in call.outputs.values() {
                for destination in outputs {
                    self.write_destination(destination, &sources, controls);
                }
            }
            return sources;
        };
        let mut actual_sources = Vec::new();

        for (path, actual) in &call.inputs {
            actual_sources.extend(self.eval_expr(actual));
            let Some(&formal) = body.arg_map.get(path) else {
                continue;
            };
            for key in self.keys_for_id(formal) {
                let sources = self.eval_actual_for_formal_key(actual, key);
                let version = self.definition(sources);
                self.state.insert(key, version);
            }
        }

        self.eval_block(&body.statements, controls);

        for (path, destinations) in &call.outputs {
            let Some(&formal) = body.arg_map.get(path) else {
                continue;
            };
            let widths: Vec<_> = destinations
                .iter()
                .map(|destination| self.destination_width(destination))
                .collect();
            if widths.iter().all(Option::is_some) {
                let total_width = widths.iter().flatten().sum();
                let mut offset = total_width;
                for (destination, width) in destinations.iter().zip(widths.into_iter()) {
                    let width = width.expect("checked above");
                    offset -= width;
                    self.write_formal_output(destination, formal, offset, total_width, controls);
                }
            } else {
                let sources = self.current_versions_for_id(formal);
                for destination in destinations {
                    self.write_destination(destination, &sources, controls);
                }
            }
        }

        let mut result = body
            .ret
            .map(|ret| self.current_versions_for_id(ret))
            .unwrap_or_default();
        if statements_have_unknown(&body.statements) {
            result.extend(actual_sources);
        }
        result.sort_unstable();
        result.dedup();
        result
    }

    fn keys_for_id(&self, id: VarId) -> Vec<NodeKey> {
        let mut keys = self
            .bit_part
            .ranges
            .iter()
            .filter(|((object, _), _)| *object == id)
            .flat_map(|((_, index), ranges)| {
                (0..ranges.len()).map(move |range| (id, *index, range))
            })
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    fn current_versions_for_id(&mut self, id: VarId) -> Vec<VersionId> {
        self.keys_for_id(id)
            .into_iter()
            .map(|key| self.current_version(key))
            .collect()
    }

    fn eval_actual_for_formal_key(
        &mut self,
        actual: &Expression,
        formal_key: NodeKey,
    ) -> Vec<VersionId> {
        let Some(mask) = self.key_mask(formal_key).cloned() else {
            return self.eval_expr(actual);
        };
        if let Expression::Term(factor) = actual
            && let Factor::Variable(id, index, select, _) = factor.as_ref()
            && index.0.is_empty()
            && select.is_empty()
        {
            return self
                .bit_part
                .overlapping((*id, formal_key.1), &mask)
                .into_iter()
                .map(|range| self.current_version((*id, formal_key.1, range)))
                .collect();
        }
        self.eval_expr_bits(actual, &mask)
    }

    fn write_formal_output(
        &mut self,
        destination: &AssignDestination,
        formal: VarId,
        formal_offset: usize,
        formal_width: usize,
        controls: &[VersionId],
    ) {
        let variable = self.ctx.variables.get(&destination.id).cloned();
        let selected = if destination.select.is_const_with_range() {
            variable.as_ref().and_then(|variable| {
                destination
                    .select
                    .eval_value(&mut self.ctx, &variable.r#type, false)
            })
        } else {
            None
        };
        for key in self.write_keys(destination) {
            let mut sources = controls.to_vec();
            if let (Some((_, low)), Some(mask)) = (selected, self.key_mask(key).cloned()) {
                let requested =
                    ((mask >> low) << formal_offset) & ValueBigUint::gen_mask(formal_width);
                for formal_key in self.keys_for_id(formal) {
                    if formal_key.1 != 0 {
                        continue;
                    }
                    let Some(formal_mask) = self.key_mask(formal_key) else {
                        continue;
                    };
                    if (formal_mask & &requested) != BigUint::default() {
                        sources.push(self.current_version(formal_key));
                    }
                }
            } else {
                sources.extend(self.current_versions_for_id(formal));
            }
            let version = self.definition(sources);
            self.state.insert(key, version);
            self.written.insert(key);
        }
    }

    fn eval_system_call(
        &mut self,
        call: &crate::ir::SystemFunctionCall,
        controls: &[VersionId],
        _value_position: bool,
    ) -> Vec<VersionId> {
        match &call.kind {
            SystemFunctionKind::Bits(_)
            | SystemFunctionKind::Size(_)
            | SystemFunctionKind::Clog2(_)
            | SystemFunctionKind::Finish => Vec::new(),
            SystemFunctionKind::Onehot(input)
            | SystemFunctionKind::Signed(input)
            | SystemFunctionKind::Unsigned(input) => self.eval_expr(&input.0),
            SystemFunctionKind::Readmemh(input, output) => {
                let sources = self.eval_expr(&input.0);
                for destination in &output.0 {
                    self.write_destination(destination, &sources, controls);
                }
                Vec::new()
            }
            SystemFunctionKind::Display(_) | SystemFunctionKind::Write(_) => Vec::new(),
            SystemFunctionKind::Assert { .. } => Vec::new(),
        }
    }
}

fn statements_have_unknown(statements: &[Statement]) -> bool {
    statements.iter().any(|statement| match statement {
        Statement::Assign(assign) => expression_has_unknown(&assign.expr),
        Statement::If(statement) => {
            expression_has_unknown(&statement.cond)
                || statements_have_unknown(&statement.true_side)
                || statements_have_unknown(&statement.false_side)
        }
        Statement::Case(statement) => {
            expression_has_unknown(&statement.case_target)
                || statement
                    .arms
                    .iter()
                    .any(|arm| statements_have_unknown(&arm.body))
                || statements_have_unknown(&statement.default)
        }
        Statement::For(statement) => statements_have_unknown(&statement.body),
        _ => false,
    })
}

fn expression_has_unknown(expression: &Expression) -> bool {
    match expression {
        Expression::Term(factor) => matches!(factor.as_ref(), Factor::Unknown(_)),
        Expression::Unary(_, expression, _) => expression_has_unknown(expression),
        Expression::Binary(left, _, right, _) => {
            expression_has_unknown(left) || expression_has_unknown(right)
        }
        Expression::Ternary(condition, left, right, _) => {
            expression_has_unknown(condition)
                || expression_has_unknown(left)
                || expression_has_unknown(right)
        }
        Expression::Concatenation(parts, _) => parts.iter().any(|(expression, repeat)| {
            expression_has_unknown(expression)
                || repeat.as_ref().is_some_and(expression_has_unknown)
        }),
        Expression::ArrayLiteral(items, _) => items.iter().any(|item| match item {
            ArrayLiteralItem::Value(expression, repeat) => {
                expression_has_unknown(expression)
                    || repeat
                        .as_ref()
                        .is_some_and(|expression| expression_has_unknown(expression.as_ref()))
            }
            ArrayLiteralItem::Defaul(expression) => expression_has_unknown(expression),
        }),
        Expression::StructConstructor(_, fields, _) => fields
            .iter()
            .any(|(_, expression)| expression_has_unknown(expression)),
    }
}

/// Mirrors the masking logic of `AssignDestination::eval_assign`.
fn dst_writes(dst: &AssignDestination, ctx: &mut Context) -> Vec<(usize, BigUint)> {
    let Some(variable) = ctx.get_variable_info(dst.id) else {
        return Vec::new();
    };
    let is_index_const = dst.index.is_const();
    let is_select_const = dst.select.is_const();

    let mask = if !is_select_const {
        conservative_select_mask(&dst.select, &variable.r#type, ctx)
    } else {
        let Some((beg, end)) = dst.select.eval_value(ctx, &variable.r#type, false) else {
            return Vec::new();
        };
        ValueBigUint::gen_mask_range(beg, end)
    };

    if variable.r#type.total_array().unwrap_or(2) > 1 && (!is_index_const || dst.index.0.is_empty())
    {
        return vec![(SPLIT_REMAINDER_INDEX, mask)];
    }

    let range = if !is_index_const {
        variable.r#type.array.calc_range(&[])
    } else {
        let Some(index) = dst.index.eval_value(ctx) else {
            return Vec::new();
        };
        variable.r#type.array.calc_range(&index)
    };

    let mut out = Vec::new();
    if let Some((beg, end)) = range {
        for i in beg..=end {
            out.push((i, mask.clone()));
        }
    }
    out
}

fn var_reads(
    id: VarId,
    index: &VarIndex,
    select: &VarSelect,
    ctx: &mut Context,
) -> Vec<(usize, BigUint)> {
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
    if variable.r#type.total_array().unwrap_or(2) > 1 && (!index.is_const() || index.0.is_empty()) {
        return vec![(SPLIT_REMAINDER_INDEX, mask)];
    }
    if index.is_const()
        && let Some(idx_path) = index.eval_value(ctx)
        && let Some(flat) = variable.r#type.array.calc_index(&idx_path)
    {
        return vec![(flat, mask)];
    }
    vec![(SPLIT_REMAINDER_INDEX, mask)]
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
