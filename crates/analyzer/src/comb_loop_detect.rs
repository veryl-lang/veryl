//! Combinational loop detection on the analyzer IR (issue #931).
//!
//! Builds a per-module `(VarId, array_index)` dependency graph from
//! `FfTable` and per-decl `ReferencedEntry` masks, then reports SCCs.
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
    AssignDestination, AssignStatement, Component, Declaration, Expression, Factor, ForBound,
    ForRange, ForStatement, FunctionCall, IfStatement, InstDeclaration, Ir, Module, Statement,
    VarIndex, VarSelect, Variable,
};
use crate::symbol::{Affiliation, Direction};
use crate::value::ValueBigUint;
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use daggy::petgraph::graph::NodeIndex;
use daggy::petgraph::visit::EdgeRef;
use std::collections::VecDeque;
use veryl_parser::resource_table::StrId;

/// `FfTable` / `per_decl_refs` granularity. Bit-precision lives in masks.
type IdxKey = (VarId, usize);

/// `(VarId, array_idx, range_idx)`. `range_idx` indexes the variable's
/// `BitPartition`, so bit-disjoint reads/writes form disjoint nodes.
type NodeKey = (VarId, usize, usize);

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
            // Unevaluable generic params -> empty per_decl_refs.
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
fn atomic_ranges(masks: &[BigUint], width: usize) -> Vec<BigUint> {
    let mut by_sig: HashMap<BigUint, BigUint> = HashMap::default();
    let one = BigUint::from(1u32);
    for b in 0..width {
        let mut sig = BigUint::default();
        for (i, m) in masks.iter().enumerate() {
            if m.bit(b as u64) {
                sig |= &one << i;
            }
        }
        if sig == BigUint::default() {
            continue;
        }
        let entry = by_sig.entry(sig).or_default();
        *entry |= &one << b;
    }
    let mut ret: Vec<BigUint> = by_sig.into_values().collect();
    // Stable order by lowest set bit so NodeKey range_idx is deterministic.
    ret.sort_by_key(|m| m.trailing_zeros().unwrap_or(0));
    ret
}

fn build_bit_partition(module: &Module, ctx: &mut Context) -> BitPartition {
    let mut masks: HashMap<(VarId, usize), Vec<BigUint>> = HashMap::default();

    // Intra-module reads / writes captured during eval_assign.
    for refs in module.per_decl_refs.values() {
        for (id, entry) in refs {
            for (i, m) in entry.mask_ref.iter().enumerate() {
                if *m != BigUint::default() {
                    masks.entry((*id, i)).or_default().push(m.clone());
                }
            }
            for (i, m) in entry.mask_assign.iter().enumerate() {
                if *m != BigUint::default() {
                    masks.entry((*id, i)).or_default().push(m.clone());
                }
            }
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
    let ff_table = &module.ff_table;
    let union_writes = compute_union_writes(&module.per_decl_refs);
    let writes_per_decl = compute_writes_per_decl(&module.per_decl_refs);
    let undom_per_decl = compute_undominated_per_decl(module);

    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.functions = module.functions.clone();
    let bit_part = build_bit_partition(module, &mut ctx);

    let mut graph: Graph<NodeKey, ()> = Graph::new();
    let mut node_map: HashMap<NodeKey, NodeIndex> = HashMap::default();

    for ((src_id, src_idx), entry) in &ff_table.table {
        if entry.is_ff {
            continue;
        }
        if !is_module_scope_var(*src_id, &module.variables) {
            continue;
        }
        let src_id_idx = (*src_id, *src_idx);

        for (reader_decl, assign_target, from_ff) in &entry.refered {
            if *from_ff {
                continue;
            }
            let read_mask = lookup_read_mask(&module.per_decl_refs, *reader_decl, src_id_idx);
            if read_mask == BigUint::default() {
                continue;
            }
            // Internal sources need a comb writer overlapping the read bits.
            // Input ports are driven externally so they always carry data.
            let effective_read = if is_input_port(*src_id, &module.variables) {
                read_mask.clone()
            } else {
                let Some(driven) = union_writes.get(&src_id_idx) else {
                    continue;
                };
                let overlap = &read_mask & driven;
                if overlap == BigUint::default() {
                    continue;
                }
                overlap
            };

            let dst_id_idxs: Vec<(VarId, usize)> = match assign_target {
                Some((dst_id, Some(dst_idx))) => vec![(*dst_id, *dst_idx)],
                Some((dst_id, None)) => writes_per_decl
                    .get(reader_decl)
                    .map(|w| {
                        w.iter()
                            .filter(|(id, _, _)| id == dst_id)
                            .map(|(id, idx, _)| (*id, *idx))
                            .collect()
                    })
                    .unwrap_or_default(),
                None => writes_per_decl
                    .get(reader_decl)
                    .map(|w| w.iter().map(|(id, idx, _)| (*id, *idx)).collect())
                    .unwrap_or_default(),
            };

            for dst_id_idx in dst_id_idxs {
                let write_mask = lookup_write_mask(&module.per_decl_refs, *reader_decl, dst_id_idx);
                if write_mask == BigUint::default() {
                    continue;
                }
                if !is_module_scope_var(dst_id_idx.0, &module.variables) {
                    continue;
                }
                // Same `(VarId, idx)` self-edge: both bit-overlap and
                // some-undominated-read needed (`a[1] = a[0]` and
                // `a = 0; a = a + 1` would otherwise false-positive).
                let mut effective_read = effective_read.clone();
                if src_id_idx == dst_id_idx {
                    if (&effective_read & &write_mask) == BigUint::default() {
                        continue;
                    }
                    let undom = undom_per_decl
                        .get(reader_decl)
                        .and_then(|m| m.get(&src_id_idx))
                        .cloned()
                        .unwrap_or_default();
                    let undom_read = &undom & &effective_read;
                    if undom_read == BigUint::default() {
                        continue;
                    }
                    effective_read = undom_read;
                }

                let src_ranges = bit_part.overlapping(src_id_idx, &effective_read);
                let dst_ranges = bit_part.overlapping(dst_id_idx, &write_mask);
                for sr in &src_ranges {
                    let src_node_key = (src_id_idx.0, src_id_idx.1, *sr);
                    let src_node = ensure_node(&mut graph, &mut node_map, src_node_key);
                    for dr in &dst_ranges {
                        let dst_node_key = (dst_id_idx.0, dst_id_idx.1, *dr);
                        let dst_node = ensure_node(&mut graph, &mut node_map, dst_node_key);
                        graph.add_edge(src_node, dst_node, ());
                    }
                }
            }
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
                for r in bit_part.overlapping((*id, idx), &mask) {
                    out.push((*id, idx, r));
                }
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

fn compute_union_writes(
    per_decl_refs: &HashMap<usize, HashMap<VarId, crate::ir::ReferencedEntry>>,
) -> HashMap<(VarId, usize), BigUint> {
    let mut out: HashMap<(VarId, usize), BigUint> = HashMap::default();
    for refs in per_decl_refs.values() {
        for (id, entry) in refs {
            for (i, mask) in entry.mask_assign.iter().enumerate() {
                if *mask == BigUint::default() {
                    continue;
                }
                let cur = out.entry((*id, i)).or_default();
                *cur |= mask;
            }
        }
    }
    out
}

/// `decl -> Vec<(VarId, idx, write_mask)>`. Includes inst-output dsts.
fn compute_writes_per_decl(
    per_decl_refs: &HashMap<usize, HashMap<VarId, crate::ir::ReferencedEntry>>,
) -> HashMap<usize, Vec<(VarId, usize, BigUint)>> {
    let mut out: HashMap<usize, Vec<(VarId, usize, BigUint)>> = HashMap::default();
    for (decl, refs) in per_decl_refs {
        for (id, entry) in refs {
            for (i, mask) in entry.mask_assign.iter().enumerate() {
                if *mask == BigUint::default() {
                    continue;
                }
                out.entry(*decl).or_default().push((*id, i, mask.clone()));
            }
        }
    }
    out
}

fn lookup_read_mask(
    per_decl_refs: &HashMap<usize, HashMap<VarId, crate::ir::ReferencedEntry>>,
    decl: usize,
    key: (VarId, usize),
) -> BigUint {
    per_decl_refs
        .get(&decl)
        .and_then(|m| m.get(&key.0))
        .and_then(|e| e.mask_ref.get(key.1).cloned())
        .unwrap_or_default()
}

fn lookup_write_mask(
    per_decl_refs: &HashMap<usize, HashMap<VarId, crate::ir::ReferencedEntry>>,
    decl: usize,
    key: (VarId, usize),
) -> BigUint {
    per_decl_refs
        .get(&decl)
        .and_then(|m| m.get(&key.0))
        .and_then(|e| e.mask_assign.get(key.1).cloned())
        .unwrap_or_default()
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

fn is_input_port(id: VarId, variables: &HashMap<VarId, Variable>) -> bool {
    use crate::ir::VarKind;
    matches!(variables.get(&id).map(|v| v.kind), Some(VarKind::Input))
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

// Statement-level dominance analysis.

/// `defs`: bits guaranteed-written on the current path.
/// `undom`: bits read without a covering preceding write.
#[derive(Default, Clone)]
struct DominanceState {
    defs: HashMap<IdxKey, BigUint>,
    undom: HashMap<IdxKey, BigUint>,
}

fn compute_undominated_per_decl(module: &Module) -> HashMap<usize, HashMap<IdxKey, BigUint>> {
    let mut out: HashMap<usize, HashMap<IdxKey, BigUint>> = HashMap::default();
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.functions = module.functions.clone();

    for (decl_idx, decl) in module.declarations.iter().enumerate() {
        if let Declaration::Comb(c) = decl {
            let mut state = DominanceState::default();
            walk_block(&c.statements, &mut state, &mut ctx);
            state.undom.retain(|_, m| *m != BigUint::default());
            if !state.undom.is_empty() {
                out.insert(decl_idx, state.undom);
            }
        }
    }
    out
}

fn walk_block(stmts: &[Statement], state: &mut DominanceState, ctx: &mut Context) {
    for stmt in stmts {
        walk_stmt(stmt, state, ctx);
    }
}

fn walk_stmt(stmt: &Statement, state: &mut DominanceState, ctx: &mut Context) {
    match stmt {
        Statement::Assign(a) => walk_assign(a, state, ctx),
        Statement::If(i) => walk_if(i, state, ctx),
        Statement::For(f) => walk_for(f, state, ctx),
        Statement::FunctionCall(c) => walk_function_call(c.as_ref(), state, ctx),
        // IfReset is always_ff-only; the rest have no LHS to track.
        Statement::IfReset(_)
        | Statement::SystemFunctionCall(_)
        | Statement::TbMethodCall(_)
        | Statement::Break
        | Statement::Unsupported(_)
        | Statement::Null => {}
    }
}

fn walk_assign(stmt: &AssignStatement, state: &mut DominanceState, ctx: &mut Context) {
    // RHS before LHS: otherwise `a = a + 1` sees itself as dominated.
    walk_expr(&stmt.expr, state, ctx);
    for dst in &stmt.dst {
        for (idx, mask) in dst_writes(dst, ctx) {
            let key = (dst.id, idx);
            *state.defs.entry(key).or_default() |= &mask;
        }
    }
}

fn walk_if(stmt: &IfStatement, state: &mut DominanceState, ctx: &mut Context) {
    walk_expr(&stmt.cond, state, ctx);

    let saved_defs = state.defs.clone();
    let saved_undom = state.undom.clone();

    let mut true_state = DominanceState {
        defs: saved_defs.clone(),
        undom: saved_undom.clone(),
    };
    walk_block(&stmt.true_side, &mut true_state, ctx);

    let mut false_state = DominanceState {
        defs: saved_defs,
        undom: saved_undom,
    };
    walk_block(&stmt.false_side, &mut false_state, ctx);

    // Merge: defs = intersection (only both-paths writes dominate
    // downstream); undom = union (any path's undom contributes).
    let mut keys: HashSet<IdxKey> = HashSet::default();
    for k in true_state.defs.keys().chain(false_state.defs.keys()) {
        keys.insert(*k);
    }
    let mut merged_defs: HashMap<IdxKey, BigUint> = HashMap::default();
    for key in keys {
        let zero = BigUint::default();
        let t = true_state.defs.get(&key).unwrap_or(&zero);
        let f = false_state.defs.get(&key).unwrap_or(&zero);
        let merged = t & f;
        if merged != zero {
            merged_defs.insert(key, merged);
        }
    }
    state.defs = merged_defs;

    state.undom = true_state.undom;
    for (key, mask) in false_state.undom {
        *state.undom.entry(key).or_default() |= &mask;
    }
}

fn walk_for(stmt: &ForStatement, state: &mut DominanceState, ctx: &mut Context) {
    walk_for_range(&stmt.range, state, ctx);
    // Body may run zero times: surface undom reads but don't trust
    // its writes to dominate anything afterwards.
    let saved_defs = state.defs.clone();
    walk_block(&stmt.body, state, ctx);
    state.defs = saved_defs;
}

fn walk_for_range(range: &ForRange, state: &mut DominanceState, ctx: &mut Context) {
    let bounds = match range {
        ForRange::Forward { start, end, .. }
        | ForRange::Reverse { start, end, .. }
        | ForRange::Stepped { start, end, .. } => [start, end],
    };
    for b in bounds {
        if let ForBound::Expression(e) = b {
            walk_expr(e, state, ctx);
        }
    }
}

fn walk_function_call(call: &FunctionCall, state: &mut DominanceState, ctx: &mut Context) {
    for input in call.inputs.values() {
        walk_expr(input, state, ctx);
    }
    for outputs in call.outputs.values() {
        for dst in outputs {
            for (idx, mask) in dst_writes(dst, ctx) {
                let key = (dst.id, idx);
                *state.defs.entry(key).or_default() |= &mask;
            }
        }
    }
}

fn walk_expr(expr: &Expression, state: &mut DominanceState, ctx: &mut Context) {
    match expr {
        Expression::Term(t) => walk_factor(t, state, ctx),
        Expression::Unary(_, e, _) => walk_expr(e, state, ctx),
        Expression::Binary(a, _, b, _) => {
            walk_expr(a, state, ctx);
            walk_expr(b, state, ctx);
        }
        Expression::Ternary(a, b, c, _) => {
            walk_expr(a, state, ctx);
            walk_expr(b, state, ctx);
            walk_expr(c, state, ctx);
        }
        Expression::Concatenation(parts, _) => {
            for (a, b) in parts {
                walk_expr(a, state, ctx);
                if let Some(b) = b {
                    walk_expr(b, state, ctx);
                }
            }
        }
        Expression::StructConstructor(_, fields, _) => {
            for (_, e) in fields {
                walk_expr(e, state, ctx);
            }
        }
        Expression::ArrayLiteral(_, _) => {}
    }
}

fn walk_factor(factor: &Factor, state: &mut DominanceState, ctx: &mut Context) {
    match factor {
        Factor::Variable(id, index, select, _) => {
            for (idx, mask) in var_reads(*id, index, select, ctx) {
                let key = (*id, idx);
                let dominated = state.defs.get(&key).cloned().unwrap_or_default();
                let undom_bits = &mask ^ (&mask & &dominated);
                if undom_bits != BigUint::default() {
                    *state.undom.entry(key).or_default() |= undom_bits;
                }
            }
        }
        Factor::FunctionCall(call) => walk_function_call(call, state, ctx),
        _ => {}
    }
}

/// Mirrors the masking logic of `AssignDestination::eval_assign`.
fn dst_writes(dst: &AssignDestination, ctx: &mut Context) -> Vec<(usize, BigUint)> {
    let Some(variable) = ctx.get_variable_info(dst.id) else {
        return Vec::new();
    };
    let is_index_const = dst.index.is_const();
    let is_select_const = dst.select.is_const();

    let range = if !is_index_const {
        variable.r#type.array.calc_range(&[])
    } else {
        let Some(index) = dst.index.eval_value(ctx) else {
            return Vec::new();
        };
        variable.r#type.array.calc_range(&index)
    };

    let mask = if !is_select_const {
        let Some(width) = variable.total_width() else {
            return Vec::new();
        };
        ValueBigUint::gen_mask(width)
    } else {
        let Some((beg, end)) = dst.select.eval_value(ctx, &variable.r#type, false) else {
            return Vec::new();
        };
        ValueBigUint::gen_mask_range(beg, end)
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
    let mask = if let Some((beg, end)) = select.eval_value(ctx, &variable.r#type, false) {
        ValueBigUint::gen_mask_range(beg, end)
    } else {
        let Some(width) = variable.total_width() else {
            return Vec::new();
        };
        ValueBigUint::gen_mask(width)
    };
    if let Some(idx_path) = index.eval_value(ctx)
        && let Some(flat) = variable.r#type.array.calc_index(&idx_path)
    {
        return vec![(flat, mask)];
    }
    // Dynamic index: conservatively treat every element as read.
    let total = variable.r#type.total_array().unwrap_or(1);
    (0..total).map(|i| (i, mask.clone())).collect()
}
