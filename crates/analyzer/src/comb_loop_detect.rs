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
    AssignDestination, AssignStatement, CaseStatement, Component, Declaration, Expression, Factor,
    ForBound, ForRange, ForStatement, FunctionCall, IfStatement, InstDeclaration, Ir, Module, Op,
    Statement, SystemFunctionKind, VarIndex, VarSelect, Variable,
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

/// `FfTable` / `per_decl_refs` granularity. Bit-precision lives in masks.
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
            // Unevaluable generic params -> empty per_decl_refs.
            if module.suppress_unassigned {
                incomplete.push(IncompleteCombAnalysis {
                    module: module.name.to_string(),
                    reasons: [IncompleteReason::UnsupportedSyntax].into(),
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
        _ => None,
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

/// Arrays larger than this are under-detected: a dynamic write (`arr[i] = ...`
/// with no foldable index) fans out to every element, so the per-element graph
/// / bit-partition expansion below is O(elements). A memory this large is not a
/// realistic combinational-loop participant, so adding no edges stays sound.
const OVERSIZED_ARRAY: usize = 1 << 16;

fn oversized_array(id: VarId, variables: &HashMap<VarId, Variable>) -> bool {
    variables
        .get(&id)
        .and_then(|v| v.r#type.total_array())
        .is_some_and(|n| n > OVERSIZED_ARRAY)
}

fn build_bit_partition(
    module: &Module,
    ctx: &mut Context,
    memory_ssa_regions: &[Region<VarId>],
) -> BitPartition {
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

    // Per-reference masks. Per-decl aggregates alone would collapse
    // bit-disjoint reads/writes of the same var into one atomic range
    // (e.g. `b = a[0]; c = a[1];` aggregates a's mask to {0,1}).
    for ((src_id, src_idx), entry) in &module.ff_table.table {
        for (_, assign_target, src_read_mask, _) in &entry.refered {
            if *src_read_mask != BigUint::default() {
                masks
                    .entry((*src_id, *src_idx))
                    .or_default()
                    .push(src_read_mask.clone());
            }
            if let Some((dst_id, dst_idx_opt, lhs_mask)) = assign_target
                && *lhs_mask != BigUint::default()
            {
                if let Some(dst_idx) = dst_idx_opt {
                    masks
                        .entry((*dst_id, *dst_idx))
                        .or_default()
                        .push(lhs_mask.clone());
                } else if let Some(var) = module.variables.get(dst_id)
                    && let Some(total) = var.r#type.total_array()
                    && total <= OVERSIZED_ARRAY
                {
                    for i in 0..total {
                        masks
                            .entry((*dst_id, i))
                            .or_default()
                            .push(lhs_mask.clone());
                    }
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

    // MemorySSA can discover finer positional boundaries than the legacy
    // read/write masks.  A shifted copy, for example, propagates every
    // observed endpoint through the copy relation.  Feed those sparse spans
    // back into the shared partition so distinct causal atoms do not collapse
    // into a spurious self-edge.
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
) -> (Graph<NodeKey, ()>, BTreeSet<IncompleteReason>, BitPartition) {
    let ff_table = &module.ff_table;
    let union_writes = compute_union_writes(&module.per_decl_refs);
    let writes_per_decl = compute_writes_per_decl(&module.per_decl_refs);
    let undom_per_decl = compute_undominated_per_decl(module);

    // Procedural combinational declarations use statement-ordered region
    // MemorySSA. Keep instance declarations on the existing bottom-up module
    // summary path until the same region vocabulary crosses module boundaries.
    let analyze_declaration = |declaration: &Declaration| match declaration {
        Declaration::Comb(comb) => Some(crate::comb_memory_ssa::analyze(module, comb)),
        _ => None,
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
        .flat_map(|summary| &summary.dependencies)
        .flat_map(|dependency| [dependency.input, dependency.output])
        .collect::<Vec<_>>();
    let mut ctx = Context::default();
    ctx.variables = module.variables.clone();
    ctx.functions = module.functions.clone();
    partition_regions.extend(collect_instance_summary_regions(
        module, summaries, &mut ctx,
    ));
    let bit_part = build_bit_partition(module, &mut ctx, &partition_regions);

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
                incomplete.insert(IncompleteReason::UnsupportedSyntax);
            }
            _ => {}
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
                incomplete.insert(IncompleteReason::UnsupportedSyntax);
                log::debug!(
                    "failed to build comb MemorySSA for {}: {error}",
                    module.name
                );
            }
        }
    }

    for ((src_id, src_idx), entry) in &ff_table.table {
        if entry.is_ff {
            continue;
        }
        if !is_module_scope_var(*src_id, &module.variables) {
            continue;
        }
        // Under-detect oversized arrays (see `OVERSIZED_ARRAY`).
        if oversized_array(*src_id, &module.variables) {
            continue;
        }
        let src_id_idx = (*src_id, *src_idx);

        for (reader_decl, assign_target, src_read_mask, from_ff) in &entry.refered {
            if *from_ff {
                continue;
            }
            if matches!(
                module.declarations.get(*reader_decl),
                Some(Declaration::Comb(_))
            ) {
                continue;
            }
            // `decl_read_mask == 0` also filters out reads gathered by
            // `gather_ff` but missing from `eval_assign` (notably inst
            // input expressions, which `add_inst_feedthrough_edges` handles).
            let decl_read_mask = lookup_read_mask(&module.per_decl_refs, *reader_decl, src_id_idx);
            if decl_read_mask == BigUint::default() {
                continue;
            }
            let read_mask = if *src_read_mask != BigUint::default() {
                &decl_read_mask & src_read_mask
            } else {
                decl_read_mask
            };
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

            // Per-statement LHS mask preferred over per-decl aggregate.
            // Otherwise `t1[0] = 0; t1[1] = src;` would route src into both
            // bits.
            let dst_with_masks: Vec<((VarId, usize), BigUint)> = match assign_target {
                Some((dst_id, Some(dst_idx), lhs_mask)) => {
                    let mask = if *lhs_mask != BigUint::default() {
                        lhs_mask.clone()
                    } else {
                        lookup_write_mask(&module.per_decl_refs, *reader_decl, (*dst_id, *dst_idx))
                    };
                    if mask != BigUint::default() {
                        vec![((*dst_id, *dst_idx), mask)]
                    } else {
                        vec![]
                    }
                }
                // Under-detect oversized arrays (see `OVERSIZED_ARRAY`).
                Some((dst_id, None, _)) if oversized_array(*dst_id, &module.variables) => vec![],
                Some((dst_id, None, lhs_mask)) => writes_per_decl
                    .get(reader_decl)
                    .map(|w| {
                        w.iter()
                            .filter(|(id, _, _)| id == dst_id)
                            .map(|(id, idx, decl_mask)| {
                                let mask = if *lhs_mask != BigUint::default() {
                                    lhs_mask & decl_mask
                                } else {
                                    decl_mask.clone()
                                };
                                ((*id, *idx), mask)
                            })
                            .filter(|(_, m)| m != &BigUint::default())
                            .collect()
                    })
                    .unwrap_or_default(),
                None => writes_per_decl
                    .get(reader_decl)
                    .map(|w| {
                        w.iter()
                            .filter(|(id, _, _)| *id == *src_id)
                            .map(|(id, idx, m)| ((*id, *idx), m.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            };

            for (dst_id_idx, write_mask) in dst_with_masks {
                if write_mask == BigUint::default() {
                    continue;
                }
                if !is_module_scope_var(dst_id_idx.0, &module.variables) {
                    continue;
                }
                // Same `(VarId, idx)`: only undominated reads can close a cycle through
                // this declaration (`a = 0; a = a + 1` must not). Disjoint bits still form
                // real multi-bit cycles (`o_y[1] = o_y[2]; o_y[2] = o_y[1]`), so read/write
                // masks need not overlap — the bit-partition ranges stop `a[1] = a[0]` self-edges.
                //
                // A condition read (`assign_target` is None, e.g. `if yw[i]`) is
                // recorded against EVERY same-variable write, so on a feed-forward
                // array chain it wires `yw[i]` to every `yw[j]` — a false cross-index
                // cycle. Extend the same undominated filter to it: a condition read
                // dominated by an earlier write to the same element cannot close a
                // loop; a real (undominated) condition loop is still detected.
                let mut effective_read = effective_read.clone();
                if src_id_idx == dst_id_idx || assign_target.is_none() {
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

fn add_memory_ssa_edges(
    module: &Module,
    summary: &ProcedureSummary<VarId>,
    bit_part: &BitPartition,
    graph: &mut Graph<NodeKey, ()>,
    node_map: &mut HashMap<NodeKey, NodeIndex>,
) {
    // Unknown control/effects invalidate a whole procedure. Dynamic regions
    // are narrower: they invalidate only their owning objects, so a dynamic
    // memory write cannot hide an unrelated scalar loop.
    if summary.unknown_all
        || summary
            .incomplete
            .iter()
            .any(|reason| *reason != IncompleteReason::DynamicRegion)
    {
        return;
    }
    for dependency in &summary.dependencies {
        if dependency.kind == EdgeKind::Unknown {
            continue;
        }
        let sources = region_node_keys(dependency.input, &module.variables, bit_part);
        let destinations = region_node_keys(dependency.output, &module.variables, bit_part);
        for source in &sources {
            if !is_module_scope_var(source.0, &module.variables) {
                continue;
            }
            for destination in &destinations {
                if !is_module_scope_var(destination.0, &module.variables) {
                    continue;
                }
                let source = ensure_node(graph, node_map, *source);
                let destination = ensure_node(graph, node_map, *destination);
                graph.add_edge(source, destination, ());
            }
        }
    }
}

/// Convert the MemorySSA engine's flattened, half-open bit region back to the
/// legacy graph's `(array element, bit partition)` coordinates. The loop is
/// proportional to touched elements, never to the declared array size.
fn region_node_keys(
    region: Region<VarId>,
    variables: &HashMap<VarId, Variable>,
    bit_part: &BitPartition,
) -> Vec<NodeKey> {
    let (object, span) = match region {
        Region::Exact { object, span } => (object, span),
        Region::UnknownObject(object) => {
            return vec![(object, UNKNOWN_REGION_INDEX, UNKNOWN_REGION_INDEX)];
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
            if let Some(mapped) =
                map_expression_span_to_regions(&input.expr, input_span, &module.variables, ctx)
            {
                regions.extend(mapped);
            }
            if let Some(mapped) =
                map_destinations_span_to_regions(&output.dst, output_span, &module.variables, ctx)
            {
                regions.extend(mapped);
            }
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
        let Some(parent_input_regions) =
            map_expression_span_to_regions(&input.expr, input_span, parent_vars, ctx)
        else {
            incomplete.insert(IncompleteReason::UnsupportedSyntax);
            continue;
        };
        let Some(parent_output_regions) =
            map_destinations_span_to_regions(&output.dst, output_span, parent_vars, ctx)
        else {
            incomplete.insert(IncompleteReason::UnsupportedSyntax);
            continue;
        };
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

fn map_expression_span_to_regions(
    expression: &Expression,
    requested: Span,
    variables: &HashMap<VarId, Variable>,
    ctx: &mut Context,
) -> Option<Vec<Region<VarId>>> {
    if requested.end()? > expression.comptime().r#type.total_width()? {
        return None;
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
        Statement::Case(c) => walk_case(c, state, ctx),
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

fn walk_case(stmt: &CaseStatement, state: &mut DominanceState, ctx: &mut Context) {
    walk_expr(&stmt.case_target, state, ctx);
    for arm in &stmt.arms {
        for p in &arm.patterns {
            match p {
                crate::ir::CasePattern::Eq(e) => walk_expr(e, state, ctx),
                crate::ir::CasePattern::Range { lo, hi, .. } => {
                    walk_expr(lo, state, ctx);
                    walk_expr(hi, state, ctx);
                }
            }
        }
    }

    let saved_defs = state.defs.clone();
    let saved_undom = state.undom.clone();

    let mut branch_states: Vec<DominanceState> = Vec::with_capacity(stmt.arms.len() + 1);
    for arm in &stmt.arms {
        let mut s = DominanceState {
            defs: saved_defs.clone(),
            undom: saved_undom.clone(),
        };
        walk_block(&arm.body, &mut s, ctx);
        branch_states.push(s);
    }
    // Empty default behaves as the saved state, modeling "no arm matched".
    let mut default_state = DominanceState {
        defs: saved_defs,
        undom: saved_undom,
    };
    walk_block(&stmt.default, &mut default_state, ctx);
    branch_states.push(default_state);

    // defs = intersection across branches; undom = union.
    let mut keys: HashSet<IdxKey> = HashSet::default();
    for b in &branch_states {
        for k in b.defs.keys() {
            keys.insert(*k);
        }
    }
    let mut merged_defs: HashMap<IdxKey, BigUint> = HashMap::default();
    for key in keys {
        let zero = BigUint::default();
        let mut acc: Option<BigUint> = None;
        for b in &branch_states {
            let v = b.defs.get(&key).unwrap_or(&zero).clone();
            acc = Some(match acc {
                Some(a) => a & v,
                None => v,
            });
        }
        if let Some(merged) = acc
            && merged != zero
        {
            merged_defs.insert(key, merged);
        }
    }
    state.defs = merged_defs;

    let mut merged_undom: HashMap<IdxKey, BigUint> = HashMap::default();
    for b in branch_states {
        for (key, mask) in b.undom {
            *merged_undom.entry(key).or_default() |= &mask;
        }
    }
    state.undom = merged_undom;
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
        if end.saturating_sub(beg) > OVERSIZED_ARRAY {
            return out;
        }
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
    // Legacy dominance bridge only: exact comb dependencies are handled by
    // region MemorySSA. Never expand a dynamic access to a huge array here.
    let total = variable.r#type.total_array().unwrap_or(1);
    if total > OVERSIZED_ARRAY {
        return Vec::new();
    }
    (0..total).map(|i| (i, mask.clone())).collect()
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
