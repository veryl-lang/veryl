use crate::backend::inst::{
    ReuseOutcome, port_alias_enabled, try_compile_inst_chunks, try_reuse_or_claim,
};
use crate::ir::context::{Context, Conv, ScopeContext};
use crate::ir::expression::{ExpressionContext, build_dynamic_bit_select};
use crate::ir::external::{ProtoExternalComponent, ProtoExternalConnect};
use crate::ir::module::{BitRange, gather_bit_aware_outputs, ranges_overlap};
use crate::ir::opt::multi_write_analysis::analyze_multi_write;
use crate::ir::opt::multi_write_analysis::collect_dyn_indexed_vars;
use crate::ir::opt::version_split;
use crate::ir::partial_index::partial_index_base;
use crate::ir::statement::{
    ProtoAssignStatement, array_literal_element_exprs, const_array_element_exprs, msb_first_window,
    size_literal_rhs,
};
use crate::ir::variable::{
    ModuleVariableMeta, VarOffset, align_up_64, create_variable_meta, ff_cacheline_pad_enabled,
};
use crate::ir::{Event, ProtoExpression, ProtoStatement, Value};
use crate::simulator_error::SimulatorError;
use crate::{HashMap, HashSet};
use std::collections::VecDeque;
use std::sync::Arc;
use veryl_analyzer::ir as air;
use veryl_parser::token_range::TokenRange;

/// Read of an `always_ff` reset net, as the `if_reset` condition.  Built as a
/// plain factor so port aliasing and selects are applied like any other read.
fn build_reset_condition(
    context: &mut Context,
    reset: &air::FfReset,
) -> Result<ProtoExpression, SimulatorError> {
    let factor = air::Factor::Variable(
        reset.id,
        reset.index.clone(),
        reset.select.clone(),
        reset.comptime.clone(),
    );
    Conv::conv(context, &air::Expression::Term(Box::new(factor)))
}

/// The declared type kind of a reset net.
fn reset_kind(context: &mut Context, id: air::VarId) -> Option<air::TypeKind> {
    context
        .scope()
        .analyzer_context
        .variables
        .get(&id)
        .map(|x| x.r#type.kind.clone())
}

/// `(active low, synchronous)` a polarity-agnostic `reset` net takes from the
/// instance ports it is wired to.
///
/// Those ports were lowered against their own declarations, so a block reading
/// the same net has to match them or the two ends of one wire run at opposite
/// polarities.  A port of THIS module is excluded: the emitted SystemVerilog
/// resolves it from `reset_type`, and so must the simulator.
fn inst_declared_reset_kind(context: &mut Context, id: air::VarId) -> Option<(bool, bool)> {
    let scope = context.scope();
    let is_port = scope
        .analyzer_context
        .variables
        .get(&id)
        .is_some_and(|v| scope.analyzer_context.port_types.contains_key(&v.path));
    if is_port {
        return None;
    }
    scope.inst_reset_kind.get(&id).copied().flatten()
}

/// True when the reset net asserts LOW.
fn reset_active_low(context: &mut Context, id: air::VarId) -> bool {
    match reset_kind(context, id) {
        Some(air::TypeKind::ResetAsyncHigh) | Some(air::TypeKind::ResetSyncHigh) => false,
        Some(air::TypeKind::ResetAsyncLow) | Some(air::TypeKind::ResetSyncLow) => true,
        _ => inst_declared_reset_kind(context, id)
            .map(|(active_low, _)| active_low)
            .unwrap_or(!context.config.abstract_reset_active_high),
    }
}

/// True when the reset asserts as an event of its own rather than being
/// sampled at a clock edge.
fn reset_is_async(context: &mut Context, id: air::VarId) -> bool {
    match reset_kind(context, id) {
        Some(air::TypeKind::ResetAsyncHigh) | Some(air::TypeKind::ResetAsyncLow) => true,
        Some(air::TypeKind::ResetSyncHigh) | Some(air::TypeKind::ResetSyncLow) => false,
        _ => inst_declared_reset_kind(context, id)
            .map(|(_, sync)| !sync)
            .unwrap_or(!context.config.abstract_reset_sync),
    }
}

/// Stable topological sort of comb statements using Kahn's algorithm (BFS/FIFO).
///
/// Dependency edges come in two classes:
///
/// - SEMANTIC edges the final order must respect for sequential
///   correctness: single-writer RAW, (same-block) prior-writer binding,
///   WAR, and WAW.
/// - BEST-EFFORT edges that only improve settle locality: a reader with no
///   same-block prior writer (split-driver nets, cross-block reads) is
///   placed after every overlapping writer so it sees settled values in
///   one pass.
///
/// Offset- and statement-granularity conflation (whole `if` statements,
/// dynamic indices, shared inlined-function scratch) can fabricate a cycle
/// out of edges no wire of the circuit closes.  The sort does NOT answer
/// one by dropping edges: a synthesisable design has an order that needs a
/// single settle pass, so a cycle here means the statements are tracked
/// more coarsely than the circuit, and the cure is to split them.  The
/// cycle is reported to the caller, which splits and asks again.
pub(crate) fn stable_topo_sort(statements: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    stable_topo_sort_impl(statements, None).0
}

/// Result of the block-aware sort: the schedule, an exact required-pass
/// hint (`Some(1)` when one settle pass provably suffices), and whether
/// the sort fell back to source order (the caller then runs its
/// combinational-loop diagnostic).
pub(crate) type SortOutcome = (Vec<ProtoStatement>, Option<usize>, bool);

/// Block-aware variant: `blocks[i]` is the source block (always_comb /
/// assign declaration) statement `i` was flattened from.  A reader binds
/// its OWN block's prior version (sequential reassignment) and reads other
/// blocks' vars SETTLED.  The one-pass hint is exact on strict success: a
/// same-block prior version is re-produced THIS pass (a block's statements
/// execute as a unit), and every other read is ordered after its writers.
pub(crate) fn stable_topo_sort_with_blocks(
    statements: Vec<ProtoStatement>,
    blocks: &[usize],
) -> SortOutcome {
    stable_topo_sort_impl(statements, Some(blocks))
}

fn stable_topo_sort_impl(statements: Vec<ProtoStatement>, blocks: Option<&[usize]>) -> SortOutcome {
    let n = statements.len();
    if n <= 1 {
        return (statements, Some(1), false);
    }

    // writer_ranges values are in ascending statement order; the edge
    // rules below rely on it.
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_reads: Vec<Vec<(VarOffset, BitRange)>> = Vec::with_capacity(n);
    let mut writer_ranges: HashMap<VarOffset, Vec<(usize, BitRange)>> = HashMap::default();
    {
        let mut ins = vec![];
        let mut bit_outs = vec![];
        for (i, s) in statements.iter().enumerate() {
            ins.clear();
            let mut outs = vec![];
            s.gather_variable_offsets(&mut ins, &mut outs);
            stmt_outputs.push(outs);
            let mut reads = vec![];
            s.gather_reads_with_ranges(&mut reads);
            stmt_reads.push(reads);
            bit_outs.clear();
            gather_bit_aware_outputs(s, &mut bit_outs);
            for &(off, br) in &bit_outs {
                writer_ranges.entry(off).or_default().push((i, br));
            }
        }
    }

    // Build a map: variable → set of statement indices that write it.
    // Dedup per stmt so repeated writes (e.g. multiple case arms touching
    // the same var) don't inflate the downstream in-degree count.
    let mut writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        let mut unique_outs: HashSet<VarOffset> = HashSet::default();
        for &key in outs {
            if unique_outs.insert(key) {
                writers.entry(key).or_default().push(i);
            }
        }
    }

    // SPLIT-DRIVER nets: multi-writer vars whose writers carry static,
    // pairwise-disjoint ranges (per-bit generate assigns).  A later writer
    // never clobbers an earlier one, so the prior-binding and WAR rules
    // would only manufacture false ordering constraints for them.
    let mut split_driver: HashSet<VarOffset> = HashSet::default();
    'next_var: for (key, ws) in &writers {
        if ws.len() < 2 {
            continue;
        }
        let Some(ranges) = writer_ranges.get(key) else {
            continue;
        };
        // Only writes from DIFFERENT statements can clobber each other.  One
        // statement can name the same range more than once — the gather
        // reports it per branch it writes it in — and comparing those against
        // each other would read a single driver as several.
        for (i, (pa, a)) in ranges.iter().enumerate() {
            if a.is_none() {
                continue 'next_var;
            }
            for (pb, b) in ranges.iter().skip(i + 1) {
                if pa != pb && ranges_overlap(*a, *b) {
                    continue 'next_var;
                }
            }
        }
        split_driver.insert(*key);
    }

    // --- Edge construction ---------------------------------------------
    let mut adj_sem: Vec<HashSet<usize>> = vec![HashSet::default(); n];
    let mut adj_opt: Vec<HashSet<usize>> = vec![HashSet::default(); n];
    // Why each edge exists.  Only populated under `VERYL_PASS_DIAG`: reading a
    // cycle without it means cross-referencing a second dump in a different
    // numbering space.
    let diag = std::env::var("VERYL_PASS_DIAG").is_ok();
    let mut edge_cause: HashMap<(usize, usize), &'static str> = HashMap::default();
    macro_rules! cause {
        ($from:expr, $to:expr, $why:expr) => {
            if diag {
                edge_cause.entry(($from, $to)).or_insert($why);
            }
        };
    }
    // Cross-block prior bindings are indistinguishable without block
    // identity, and void the one-pass hint.
    let mut hint_blocked = blocks.is_none();

    // Without block info every prior writer binds, as before.
    let same_block = |w: usize, r: usize| -> bool { blocks.map(|b| b[w] == b[r]).unwrap_or(true) };
    // Bits of `key` a statement writes on EVERY execution path, as merged
    // (lo, hi) spans (full width = (0, usize::MAX)); an If contributes the
    // INTERSECTION of its branches (a lowered case guarantees a bit only
    // when every arm incl. the default writes it).
    fn merge_spans(mut v: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
        v.sort_unstable();
        let mut m: Vec<(usize, usize)> = Vec::new();
        for (lo, hi) in v {
            match m.last_mut() {
                Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                _ => m.push((lo, hi)),
            }
        }
        m
    }
    fn guaranteed_write_spans(stmt: &ProtoStatement, key: &VarOffset) -> Vec<(usize, usize)> {
        fn intersect(a: &[(usize, usize)], b: &[(usize, usize)]) -> Vec<(usize, usize)> {
            let mut out = Vec::new();
            let (mut i, mut j) = (0, 0);
            while i < a.len() && j < b.len() {
                let lo = a[i].0.max(b[j].0);
                let hi = a[i].1.min(b[j].1);
                if lo <= hi {
                    out.push((lo, hi));
                }
                if a[i].1 < b[j].1 {
                    i += 1;
                } else {
                    j += 1;
                }
            }
            out
        }
        match stmt {
            ProtoStatement::Assign(a) if a.dst == *key && a.dynamic_select.is_none() => {
                match a.select {
                    None => vec![(0, usize::MAX)],
                    Some((hi, lo)) => vec![(lo, hi)],
                }
            }
            ProtoStatement::If(x) => {
                let t = merge_spans(
                    x.true_side
                        .iter()
                        .flat_map(|s| guaranteed_write_spans(s, key))
                        .collect(),
                );
                let f = merge_spans(
                    x.false_side
                        .iter()
                        .flat_map(|s| guaranteed_write_spans(s, key))
                        .collect(),
                );
                intersect(&t, &f)
            }
            ProtoStatement::Case(c) => {
                // Guaranteed only if EVERY arm and the default writes the bit
                // (exactly one branch runs) — intersect all branches.
                let mut spans: Option<Vec<(usize, usize)>> = None;
                for branch in c
                    .arms
                    .iter()
                    .map(|arm| &arm.body)
                    .chain(std::iter::once(&c.default))
                {
                    let b = merge_spans(
                        branch
                            .iter()
                            .flat_map(|s| guaranteed_write_spans(s, key))
                            .collect(),
                    );
                    spans = Some(match spans {
                        None => b,
                        Some(acc) => intersect(&acc, &b),
                    });
                }
                spans.unwrap_or_default()
            }
            ProtoStatement::SequentialBlock(body) => merge_spans(
                body.iter()
                    .flat_map(|s| guaranteed_write_spans(s, key))
                    .collect(),
            ),
            _ => Vec::new(),
        }
    }
    // A prior-bound read stays stable across passes iff every bit a LATER
    // overlapping writer can leave in the slot (the only cross-pass leak
    // channel) is re-produced unconditionally by the reader's own block
    // before the read (conditional producers reach here via
    // uncovered-branch latch designs — that diagnostic is a warning).
    let prior_unconditional_cover = |reader_idx: usize, key: &VarOffset, rr: BitRange| -> bool {
        let wranges = &writer_ranges[key];
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut last_p = usize::MAX;
        for (p, _) in wranges {
            if *p >= reader_idx || !same_block(*p, reader_idx) || *p == last_p {
                continue;
            }
            last_p = *p;
            spans.extend(guaranteed_write_spans(&statements[*p], key));
        }
        let merged = merge_spans(spans);
        for (p, wr) in wranges {
            if *p <= reader_idx || !ranges_overlap(*wr, rr) {
                continue;
            }
            let (hi, lo) = match wr {
                None => (usize::MAX, 0),
                Some((hi, lo)) => (*hi, *lo),
            };
            if !merged.iter().any(|(m_lo, m_hi)| *m_lo <= lo && hi <= *m_hi) {
                return false;
            }
        }
        true
    };

    // RAW, range-filtered: a read of `x[A]` does not depend on a writer
    // of `x[B]` (packed per-bit drivers, pipeline-phase vectors).
    let mut relevant: Vec<usize> = Vec::new();
    for (reader_idx, reads) in stmt_reads.iter().enumerate() {
        for (key, rr) in reads {
            let Some(wranges) = writer_ranges.get(key) else {
                continue;
            };
            relevant.clear();
            relevant.extend(
                wranges
                    .iter()
                    .filter(|(_, wr)| ranges_overlap(*wr, *rr))
                    .map(|(p, _)| *p),
            );
            relevant.dedup();
            if relevant.is_empty() {
                continue;
            }
            if relevant.len() == 1 {
                let writer_idx = relevant[0];
                if writer_idx == reader_idx {
                    // Self-reference only.
                } else if writer_idx < reader_idx
                    || writers.get(key).is_some_and(|ws| ws.len() == 1)
                {
                    adj_sem[writer_idx].insert(reader_idx);
                    cause!(writer_idx, reader_idx, "raw");
                } else {
                    // Sole overlapping writer comes later: a settled-value
                    // read, so the edge is best-effort rather than semantic.
                    adj_opt[writer_idx].insert(reader_idx);
                    cause!(writer_idx, reader_idx, "raw-settled");
                }
            } else if relevant.binary_search(&reader_idx).is_ok() {
                // The reader itself writes overlapping bits (shared inlined
                // scratch, carry chains): the value is produced
                // intra-statement, so it stays in source order.  KNOWN
                // LIMITATION: a read its own write does not cover also
                // depends on the other writers, but edges here close
                // sem/opt cycles across shared-scratch statement clusters
                // and collapse real designs to source order.
            } else if let Some(&writer_idx) = (!split_driver.contains(key))
                .then(|| {
                    relevant
                        .iter()
                        .rev()
                        .find(|&&w| w < reader_idx && same_block(w, reader_idx))
                })
                .flatten()
            {
                adj_sem[writer_idx].insert(reader_idx);
                cause!(writer_idx, reader_idx, "prior-binding");
                if relevant.last().is_some_and(|&w| w > reader_idx)
                    && !prior_unconditional_cover(reader_idx, key, *rr)
                {
                    if !hint_blocked && std::env::var("VERYL_PASS_DIAG").is_ok() {
                        log::info!(
                            "pass_diag: hint blocked: uncovered prior binding reader #{reader_idx} var {key:?} range {rr:?}"
                        );
                    }
                    hint_blocked = true;
                }
            } else {
                // Split-driver net, cross-block read, or same-block
                // read-before-write: wait for every overlapping writer so
                // the read sees settled values.
                for &writer_idx in &relevant {
                    adj_opt[writer_idx].insert(reader_idx);
                    cause!(writer_idx, reader_idx, "settled-group");
                }
            }
        }
    }

    // WAR: a reader between two overlapping writes must precede the later
    // write, else the re-write clobbers the value it read.  Scoped to a
    // SAME-block prior (matching the RAW binding) — a cross-block reader
    // orders after every writer, so a WAR edge would only close a false
    // cycle.
    for (reader_idx, reads) in stmt_reads.iter().enumerate() {
        for (key, rr) in reads {
            if split_driver.contains(key) {
                continue;
            }
            let Some(wranges) = writer_ranges.get(key) else {
                continue;
            };
            let has_prior = wranges.iter().any(|(p, wr)| {
                *p < reader_idx && ranges_overlap(*wr, *rr) && same_block(*p, reader_idx)
            });
            if !has_prior {
                continue;
            }
            if let Some(&(next_writer, _)) = wranges
                .iter()
                .find(|(p, wr)| *p > reader_idx && ranges_overlap(*wr, *rr))
            {
                adj_sem[reader_idx].insert(next_writer);
                cause!(reader_idx, next_writer, "war");
            }
        }
    }

    // WAW: chain consecutive writers of a reassigned var so overlapping
    // writes keep source order.  Skip only when next reaches prev over
    // SEMANTIC edges (a genuine cycle); a best-effort path does not skip, so
    // write order is kept instead of being silently inverted.
    {
        let mut stack: Vec<usize> = Vec::new();
        let mut visited: HashSet<usize> = HashSet::default();
        for (key, writer_indices) in &writers {
            if split_driver.contains(key) {
                continue;
            }
            for pair in writer_indices.windows(2) {
                let (prev, next) = (pair[0], pair[1]);
                let mut reachable = false;
                stack.clear();
                stack.push(next);
                visited.clear();
                while let Some(node) = stack.pop() {
                    if node == prev {
                        reachable = true;
                        break;
                    }
                    if visited.insert(node) {
                        stack.extend(adj_sem[node].iter().copied());
                    }
                }
                if !reachable {
                    adj_sem[prev].insert(next);
                    cause!(prev, next, "waw");
                } else {
                    // Two overlapping writes whose order the sort cannot
                    // guarantee: never claim an exact one-pass schedule.
                    if !hint_blocked && std::env::var("VERYL_PASS_DIAG").is_ok() {
                        log::info!(
                            "pass_diag: hint blocked: WAW skip prev #{prev} next #{next} var {key:?}"
                        );
                    }
                    hint_blocked = true;
                }
            }
        }
    }

    // Best-effort edges duplicated in the semantic class are redundant;
    // dropping them must actually remove the constraint.
    for u in 0..n {
        if !adj_sem[u].is_empty() && !adj_opt[u].is_empty() {
            let sem = &adj_sem[u];
            adj_opt[u].retain(|v| !sem.contains(v));
        }
    }

    // --- Kahn ------------------------------------------------------------
    // ONE attempt, and every edge constrains it: a graph that does not
    // linearize means the statements are tracked more coarsely than the
    // circuit wires them, and the caller answers that by splitting them
    // finer -- not by dropping edges for an order that needs extra passes.
    let order = {
        let mut in_degree: Vec<usize> = vec![0; n];
        for succs in adj_sem.iter() {
            for &v in succs {
                in_degree[v] += 1;
            }
        }
        for succs in adj_opt.iter() {
            for &v in succs {
                in_degree[v] += 1;
            }
        }
        let mut queue: VecDeque<usize> = VecDeque::new();
        for (i, &deg) in in_degree.iter().enumerate() {
            if deg == 0 {
                queue.push_back(i);
            }
        }
        let mut sorted_indices: Vec<usize> = Vec::with_capacity(n);
        let mut successors: Vec<usize> = Vec::new();
        while let Some(idx) = queue.pop_front() {
            sorted_indices.push(idx);
            successors.clear();
            successors.extend(adj_sem[idx].iter().copied());
            successors.extend(adj_opt[idx].iter().copied());
            // Index order for determinism.
            successors.sort_unstable();
            successors.dedup();
            for &succ in &successors {
                in_degree[succ] -= 1;
                if in_degree[succ] == 0 {
                    queue.push_back(succ);
                }
            }
        }
        (sorted_indices.len() == n).then_some(sorted_indices)
    };

    let Some(sorted_indices) = order else {
        if std::env::var("VERYL_PASS_DIAG").is_ok() {
            use daggy::petgraph::Graph;
            use daggy::petgraph::algo::tarjan_scc;
            let mut g: Graph<(), ()> = Graph::new();
            let nodes: Vec<_> = (0..n).map(|_| g.add_node(())).collect();
            for (u, succs) in adj_sem.iter().enumerate() {
                for &v in succs {
                    g.add_edge(nodes[u], nodes[v], ());
                }
            }
            for (u, succs) in adj_opt.iter().enumerate() {
                for &v in succs {
                    g.add_edge(nodes[u], nodes[v], ());
                }
            }
            let mut scc_id: Vec<usize> = vec![usize::MAX; n];
            let mut nontrivial = 0usize;
            for (i, scc) in tarjan_scc(&g).into_iter().enumerate() {
                if scc.len() > 1 {
                    nontrivial += 1;
                    for node in scc {
                        scc_id[node.index()] = i;
                    }
                }
            }
            log::info!(
                "pass_diag: stable_topo_sort n={n}: cycle in {nontrivial} SCC(s); \
                 reporting it for the caller to split",
            );
            trace_sort_cycles(&statements, &adj_sem, &adj_opt, &scc_id, &edge_cause);
        }
        return (statements, None, true);
    };

    // Reconstruct statement list in sorted order.
    let mut result: Vec<Option<ProtoStatement>> = statements.into_iter().map(Some).collect();
    let sorted: Vec<ProtoStatement> = sorted_indices
        .into_iter()
        .map(|i| result[i].take().unwrap())
        .collect();
    (sorted, (!hint_blocked).then_some(1), false)
}

/// `VERYL_PASS_DIAG=1` diagnostic: BFS one shortest cycle per nontrivial
/// SCC and print it with source locations, edge class and cause, and the
/// variable ranges that connect each step.
fn trace_sort_cycles(
    statements: &[ProtoStatement],
    adj_sem: &[HashSet<usize>],
    adj_opt: &[HashSet<usize>],
    scc_id: &[usize],
    edge_cause: &HashMap<(usize, usize), &'static str>,
) {
    let n = statements.len();
    // Every nontrivial SCC, not just the first: on a design with several, the
    // one that costs a settle pass is rarely the one that happens to come
    // first in statement order.
    let mut ids: Vec<usize> = Vec::new();
    for &id in scc_id.iter() {
        if id != usize::MAX && !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return;
    }
    for (k, id) in ids.iter().enumerate() {
        log::info!(
            "pass_diag: SCC {k} (id {id}) has {} members",
            scc_id.iter().filter(|&&x| x == *id).count()
        );
    }

    const MAX_TRACED: usize = 4;
    for (k, id) in ids.iter().enumerate().take(MAX_TRACED) {
        let members: Vec<usize> = (0..n).filter(|&i| scc_id[i] == *id).collect();
        log::info!("pass_diag: cycle inside SCC {k}:");
        let mset: HashSet<usize> = members.iter().cloned().collect();
        let start = members[0];
        let mut parent: HashMap<usize, usize> = HashMap::default();
        let mut bfsq: VecDeque<usize> = VecDeque::new();
        bfsq.push_back(start);
        let mut closer: Option<usize> = None;
        'bfs: while let Some(u) = bfsq.pop_front() {
            for &v in adj_sem[u].iter().chain(adj_opt[u].iter()) {
                if !mset.contains(&v) {
                    continue;
                }
                if v == start {
                    closer = Some(u);
                    break 'bfs;
                }
                if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(v) {
                    e.insert(u);
                    bfsq.push_back(v);
                }
            }
        }
        let Some(last) = closer else {
            return;
        };
        let mut path = vec![start];
        let mut cur = last;
        let mut rev = vec![];
        while cur != start {
            rev.push(cur);
            cur = parent[&cur];
        }
        rev.reverse();
        path.extend(rev);
        for (i, &m) in path.iter().enumerate() {
            let nxt = path.get(i + 1).copied().unwrap_or(start);
            let kind = if adj_sem[m].contains(&nxt) {
                "sem"
            } else {
                "opt"
            };
            let kind_of = |st: &ProtoStatement| match st {
                ProtoStatement::Assign(_) => "Assign",
                ProtoStatement::AssignDynamic(_) => "AssignDynamic",
                ProtoStatement::If(_) => "If",
                ProtoStatement::Case(_) => "Case",
                ProtoStatement::For(_) => "For",
                ProtoStatement::SequentialBlock(_) => "SeqBlock",
                ProtoStatement::CompiledBlock(_) => "CompiledBlock",
                ProtoStatement::SystemFunctionCall(_) => "SysFn",
                _ => "?",
            };
            let desc = match statements[m].token() {
                Some(t) => {
                    let src = t.beg.source.to_string();
                    let file = src.rsplit('/').next().unwrap_or(&src).to_string();
                    format!("{}@{file}:{}", kind_of(&statements[m]), t.beg.line)
                }
                None => format!("{}@generated", kind_of(&statements[m])),
            };
            // The pair of RANGES is what decides whether the edge is real: the
            // writer's bits on `m` against the reader's bits on `nxt`.
            let mut w_bits = vec![];
            gather_bit_aware_outputs(&statements[m], &mut w_bits);
            let mut r_bits = vec![];
            statements[nxt].gather_reads_with_ranges(&mut r_bits);
            let mut via: Vec<String> = Vec::new();
            for (woff, wr) in &w_bits {
                for (roff, rr) in &r_bits {
                    if woff == roff && ranges_overlap(*wr, *rr) {
                        let e = format!("{woff:?} w{wr:?} r{rr:?}");
                        if !via.contains(&e) {
                            via.push(e);
                        }
                    }
                }
            }
            log::info!(
                "  cycle[{i}] #{m} {desc} -[{kind}/{}]-> #{nxt} via {}",
                edge_cause.get(&(m, nxt)).copied().unwrap_or("?"),
                via.join(", ")
            );
            // The node's OWN reads, so a cycle is readable without a second
            // dump in a different numbering space.
            {
                let mut r = vec![];
                statements[m].gather_reads_with_ranges(&mut r);
                r.sort_unstable_by_key(|(o, br)| (o.is_ff(), o.raw(), *br));
                r.dedup();
                let mut w = vec![];
                gather_bit_aware_outputs(&statements[m], &mut w);
                w.sort_unstable_by_key(|(o, br)| (o.is_ff(), o.raw(), *br));
                w.dedup();
                let fmt = |v: &[(VarOffset, BitRange)]| -> String {
                    v.iter()
                        .take(12)
                        .map(|(o, br)| match br {
                            Some((hi, lo)) => format!("{o:?}[{hi}:{lo}]"),
                            None => format!("{o:?}"),
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                log::info!(
                    "        #{m} writes {} | reads {}{}",
                    fmt(&w),
                    fmt(&r),
                    if r.len() > 12 { " ..." } else { "" }
                );
                // Per-arm reads: a node that writes ONE variable from many
                // arms carries the UNION of the arms' reads, and a cycle
                // through that union is false whenever no single arm closes it.
                if let ProtoStatement::Case(c) = &statements[m] {
                    for (a, arm) in c.arms.iter().enumerate() {
                        let mut ar = vec![];
                        arm.cond.gather_variable_offsets(&mut ar);
                        let mut reads: Vec<(VarOffset, BitRange)> =
                            ar.into_iter().map(|o| (o, None)).collect();
                        for st in &arm.body {
                            st.gather_reads_with_ranges(&mut reads);
                        }
                        reads.sort_unstable_by_key(|(o, br)| (o.is_ff(), o.raw(), *br));
                        reads.dedup();
                        log::info!("          arm{a} reads {}", fmt(&reads));
                    }
                }
            }
        }
    }
}

pub struct ProtoDeclaration {
    pub event_statements: HashMap<Event, Vec<ProtoStatement>>,
    pub comb_statements: Vec<ProtoStatement>,
    /// Post-comb functions: child comb-only JIT functions for pre-event eval.
    pub post_comb_fns: Vec<ProtoStatement>,
    pub child_modules: Vec<ModuleVariableMeta>,
    /// Clock-typed non-port variables discovered inside child instances.
    /// Bubbled up through `Inst` declarations so the top module sees every
    /// derived clock across the hierarchy, not just its own locals.
    pub derived_clock_candidates: Vec<(air::VarId, VarOffset, usize)>,
    /// User-defined component instances declared at this level.
    pub external_components: Vec<ProtoExternalComponent>,
}

impl Conv<&air::Declaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::Declaration) -> Result<Self, SimulatorError> {
        match src {
            air::Declaration::Comb(x) => {
                let mut comb_statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    comb_statements.extend(stmts);
                }
                #[allow(unused_mut)]
                let mut comb_statements = if comb_statements.len() > 1 {
                    vec![ProtoStatement::SequentialBlock(comb_statements)]
                } else {
                    comb_statements
                };
                // Version-split runs here, before chunk compilation and the
                // cross-test caches, so every module's always_comb is seen
                // while still a flat SequentialBlock (at top level the DUT's
                // blocks end up nested inside CompiledBlocks).
                // Skipped when a statement in the block reads a whole memory:
                // the pass's select chains over such reads buy nothing.
                #[cfg(not(target_family = "wasm"))]
                if version_split::pass_enabled(context.config.use_4state)
                    && !crate::ir::deps::stmts_infeasible(&comb_statements)
                {
                    // Fresh comb offsets for rename temps come from the same
                    // allocator as function locals; instance-reuse records
                    // the post-conv size, so cache replay stays consistent.
                    let use_4state = context.config.use_4state;
                    let comb_total = &mut context.comb_total_bytes;
                    let mut alloc = |width: usize| -> isize {
                        let nb = crate::ir::variable::native_bytes(width);
                        let off = *comb_total as isize;
                        *comb_total += crate::ir::variable::value_size(nb, use_4state);
                        off
                    };
                    let stats = version_split::run(&mut comb_statements, &mut alloc);
                    version_split::accumulate(&stats);
                }
                Ok(ProtoDeclaration {
                    event_statements: HashMap::default(),
                    comb_statements,
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    derived_clock_candidates: vec![],
                    external_components: vec![],
                })
            }
            air::Declaration::Ff(x) => {
                // Converted before the body so statements its own conversion
                // defers (an inlined function inside an index) stay separable
                // from the body's.
                let reset_cond = match &x.reset {
                    Some(reset) => {
                        let cond = build_reset_condition(context, reset)?;
                        let pending = std::mem::take(&mut context.pending_statements);
                        Some((cond, pending))
                    }
                    None => None,
                };

                let mut statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    statements.extend(stmts);
                }

                let clock_event = Event::Clock(x.clock.id);
                let mut event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();

                if let Some(reset) = &x.reset {
                    let (cond, mut clock_stmts) = reset_cond.unwrap();
                    let head = statements.remove(0);
                    let (mut true_side, mut false_side) = head.split_if_reset().unwrap();
                    // Statements after `if_reset` run on both reset and clock
                    // edges, so append to both branches instead of dropping them.
                    true_side.extend(statements.iter().cloned());
                    false_side.extend(statements);

                    // The `negedge rst_n` arm of the sensitivity list: an async
                    // reset must also reach a block whose clock is stopped, and
                    // no clock-edge test can express that.  Reaching a block
                    // through both arms in one step runs its reset branch twice,
                    // as it does in SystemVerilog.  A sync reset has no such arm.
                    if reset_is_async(context, reset.id) {
                        event_statements.insert(Event::Reset(reset.id), true_side.clone());
                    }

                    // The `if (rst)` of the emitted body: a reset produced by
                    // `assign`, by a cast, or by another `always_ff` has no
                    // assertion event and is reached only this way.
                    //
                    // Polarity rides in the branch ORDER so the condition stays
                    // a bare 1-bit read: active-low takes the clock branch when
                    // the net reads 1.
                    let (true_side, false_side) = if reset_active_low(context, reset.id) {
                        (false_side, true_side)
                    } else {
                        (true_side, false_side)
                    };
                    clock_stmts.push(ProtoStatement::If(crate::ir::ProtoIfStatement {
                        cond: Some(cond),
                        true_side,
                        false_side,
                    }));
                    event_statements.insert(clock_event, clock_stmts);
                } else {
                    event_statements.insert(clock_event, statements);
                }

                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    derived_clock_candidates: vec![],
                    external_components: vec![],
                })
            }
            air::Declaration::Inst(x) => Conv::conv(context, x.as_ref()),
            air::Declaration::Initial(x) => {
                context.in_initial = true;
                let mut initial_statements = vec![];
                let mut conv_err = None;
                for stmt in &x.statements {
                    match Conv::conv(context, stmt) {
                        Ok(stmts) => {
                            let stmts: Vec<ProtoStatement> = stmts;
                            initial_statements.extend(stmts);
                        }
                        Err(e) => {
                            conv_err = Some(e);
                            break;
                        }
                    }
                }
                context.in_initial = false;
                if let Some(e) = conv_err {
                    return Err(e);
                }
                let mut event_statements = HashMap::default();
                event_statements.insert(Event::Initial, initial_statements);
                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    derived_clock_candidates: vec![],
                    external_components: vec![],
                })
            }
            air::Declaration::Final(x) => {
                let mut final_statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    final_statements.extend(stmts);
                }
                let mut event_statements = HashMap::default();
                event_statements.insert(Event::Final, final_statements);
                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    derived_clock_candidates: vec![],
                    external_components: vec![],
                })
            }
            air::Declaration::Unsupported(token) => {
                Err(SimulatorError::unsupported_description(token))
            }
            air::Declaration::External(x) => {
                let mut connects = vec![];
                for c in &x.connects {
                    let expr: ProtoExpression = Conv::conv(context, &c.expr)?;
                    // Only a plain (unindexed, unselected) variable
                    // connection can serve as a component output.
                    let output = c.output.as_ref().and_then(|dst| {
                        (dst.index.0.is_empty()
                            && dst.select.0.is_empty()
                            && dst.select.1.is_none())
                        .then_some(dst.id)
                    });
                    // Clock/reset events fire on the connected variable;
                    // input-only (modport) connections carry it in the
                    // expression rather than `output`. Hierarchical
                    // references have no VarId here; their event key is
                    // recovered after hier resolution in module conv.
                    let event_var = output.or_else(|| {
                        if !(c.is_clock || c.is_reset) {
                            return None;
                        }
                        if let air::Expression::Term(term) = &c.expr
                            && let air::Factor::Variable(var_id, _, _, _) = term.as_ref()
                        {
                            Some(*var_id)
                        } else {
                            None
                        }
                    });
                    connects.push(ProtoExternalConnect {
                        port: c.port,
                        expr,
                        output,
                        input: c.input,
                        group: c.group,
                        member: c.member,
                        event_var,
                        is_clock: c.is_clock,
                        is_reset: c.is_reset,
                        width: c.width,
                        token: c.token,
                    });
                }
                Ok(ProtoDeclaration {
                    event_statements: HashMap::default(),
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    derived_clock_candidates: vec![],
                    external_components: vec![ProtoExternalComponent {
                        name: x.name,
                        component: x.component,
                        params: x.params.clone(),
                        connects,
                        is_var_form: x.is_var_form,
                        token: x.token,
                    }],
                })
            }
            air::Declaration::Null => Ok(ProtoDeclaration {
                event_statements: HashMap::default(),
                comb_statements: vec![],
                post_comb_fns: vec![],
                child_modules: vec![],
                derived_clock_candidates: vec![],
                external_components: vec![],
            }),
        }
    }
}

impl Conv<&air::InstDeclaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::InstDeclaration) -> Result<Self, SimulatorError> {
        let air::Component::Module(child_module) = src.component.as_ref() else {
            // `$sv::` blackbox instances reach this conv path with a
            // SystemVerilog component — the simulator cannot model them.
            return Err(SimulatorError::unsupported_description(&src.token));
        };

        let mut child_analyzer_context = veryl_analyzer::conv::Context::default();
        child_analyzer_context.variables = child_module.variables.clone();
        child_analyzer_context.functions = child_module.functions.clone();
        let mut child_ff_table = child_module.ff_table.clone();
        if context.config.disable_ff_opt {
            child_ff_table.force_all_ff();
        }

        // Comb-to-FF hoist: clone child declarations and mutate them —
        // move comb-side `let` writes into the consuming FF block, then
        // rebuild the FfTable on the hoisted form.
        let mut hoisted_child_decls = child_module.declarations.clone();
        {
            let plans = veryl_analyzer::ir::comb_to_ff_hoist::plan_hoists(
                &hoisted_child_decls,
                &child_ff_table,
                &child_module.variables,
            );
            veryl_analyzer::ir::comb_to_ff_hoist::apply_hoists(
                &mut hoisted_child_decls,
                &plans,
                &child_module.variables,
            );
            child_ff_table = air::FfTable::default();
            for (i, x) in hoisted_child_decls.iter().enumerate() {
                x.gather_ff(&mut child_analyzer_context, &mut child_ff_table, i);
            }
            child_ff_table.update_is_ff(&hoisted_child_decls, &mut child_analyzer_context);
            if context.config.disable_ff_opt {
                child_ff_table.force_all_ff();
            }
        }
        let child_decls: &[air::Declaration] = &hoisted_child_decls;

        let mut ff_start = context.ff_total_bytes as isize;
        if ff_cacheline_pad_enabled() {
            ff_start = align_up_64(ff_start);
            context.ff_total_bytes = ff_start as usize;
        }
        let comb_start = context.comb_total_bytes as isize;

        // Analyzer-IR pre-pass to identify multi-RMW FFs.  Same as
        // ProtoModule::conv but for child module.
        let multi_rmw_set = analyze_multi_write(
            child_decls,
            &mut child_analyzer_context,
            context.config.disable_ff_opt,
        );
        let dyn_indexed = collect_dyn_indexed_vars(child_decls);

        let (mut child_variable_meta, child_ff_count, child_comb_count) = create_variable_meta(
            &child_module.variables,
            &child_ff_table,
            &multi_rmw_set,
            &dyn_indexed,
            context.config.use_4state,
            ff_start,
            comb_start,
        )?;

        context.ff_total_bytes += child_ff_count;
        context.comb_total_bytes += child_comb_count;

        // Alias child input/output port storage to the parent slot it's wired
        // to, eliminating the inter-module copy at settle_comb.  De-aliased only
        // for the reuse-target DUT boundary — see `port_alias_enabled`.
        let component_key = Arc::as_ptr(&src.component);
        let alias_enabled = port_alias_enabled(
            component_key,
            child_ff_count,
            child_comb_count,
            context.in_reuse_dut,
            context.test_top_id,
            context.config.dut_reuse,
        );
        let mut aliased_input_ids: HashSet<air::VarId> = HashSet::default();
        if alias_enabled {
            for input in &src.inputs {
                let Some(air::Expression::Term(factor)) = input.single() else {
                    continue;
                };
                let air::Factor::Variable(parent_id, idx, sel, _) = factor.as_ref() else {
                    continue;
                };
                if !idx.0.is_empty() || !sel.is_empty() {
                    continue;
                }
                // Reading parent storage directly during an event would
                // race the parent's own NBA commit; reject when the
                // parent has any always_ff writer.
                let parent_scope = context.scope();
                let parent_has_ff_writer = parent_scope
                    .ff_table
                    .table
                    .iter()
                    .any(|((vid, _), entry)| *vid == *parent_id && entry.assigned.is_some());
                if parent_has_ff_writer {
                    continue;
                }
                let parent_meta = match parent_scope.variable_meta.get(parent_id) {
                    Some(m) => m.clone(),
                    None => continue,
                };
                let child_meta = match child_variable_meta.get(&input.id) {
                    Some(m) => m,
                    None => continue,
                };
                if parent_meta.elements.len() != child_meta.elements.len()
                    || parent_meta.width != child_meta.width
                    || parent_meta.native_bytes != child_meta.native_bytes
                {
                    continue;
                }
                let entry = child_variable_meta.get_mut(&input.id).unwrap();
                for (i, parent_elem) in parent_meta.elements.iter().enumerate() {
                    entry.elements[i].current = parent_elem.current;
                    entry.elements[i].next_offset = parent_elem.next_offset;
                }
                // Drop initial_values so fill_buffers_recursive doesn't
                // overwrite parent storage with the child port's default.
                entry.initial_values.clear();
                aliased_input_ids.insert(input.id);
            }
        }

        // Output ports are aliased comb→comb only.  An FF on either side
        // has a separate next-slot write that would not reach the comb
        // slot the alias targets.
        let mut aliased_output_ids: HashSet<air::VarId> = HashSet::default();
        let mut already_aliased_parent_ids: HashSet<air::VarId> = HashSet::default();
        if alias_enabled {
            for output in &src.outputs {
                // A concat destructure (multiple dsts) maps each parent var
                // to a slice of the child port — no whole-var alias exists.
                let [parent_dst] = output.dst.as_slice() else {
                    continue;
                };
                if !parent_dst.index.0.is_empty() || !parent_dst.select.is_empty() {
                    continue;
                }
                let child_output_is_ff = child_ff_table
                    .table
                    .iter()
                    .any(|((vid, _), entry)| *vid == output.id && entry.assigned.is_some());
                if child_output_is_ff {
                    continue;
                }
                let parent_scope = context.scope();
                let parent_has_ff_writer = parent_scope
                    .ff_table
                    .table
                    .iter()
                    .any(|((vid, _), entry)| *vid == parent_dst.id && entry.assigned.is_some());
                if parent_has_ff_writer {
                    continue;
                }
                let parent_meta = match parent_scope.variable_meta.get(&parent_dst.id) {
                    Some(m) => m.clone(),
                    None => continue,
                };
                if parent_meta.elements.iter().any(|e| e.is_ff()) {
                    continue;
                }
                let child_meta = match child_variable_meta.get(&output.id) {
                    Some(m) => m,
                    None => continue,
                };
                if parent_meta.elements.len() != child_meta.elements.len()
                    || parent_meta.width != child_meta.width
                    || parent_meta.native_bytes != child_meta.native_bytes
                {
                    continue;
                }
                if already_aliased_parent_ids.contains(&parent_dst.id) {
                    continue;
                }
                let entry = child_variable_meta.get_mut(&output.id).unwrap();
                for (i, parent_elem) in parent_meta.elements.iter().enumerate() {
                    entry.elements[i].current = parent_elem.current;
                    entry.elements[i].next_offset = parent_elem.next_offset;
                }
                entry.initial_values.clear();
                aliased_output_ids.insert(output.id);
                already_aliased_parent_ids.insert(parent_dst.id);
            }
        }

        let mut all_event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        let mut all_comb_statements: Vec<ProtoStatement> = vec![];
        let mut all_post_comb_fns: Vec<ProtoStatement> = vec![];
        let mut all_child_modules: Vec<ModuleVariableMeta> = vec![];
        let mut all_derived_clock_candidates: Vec<(air::VarId, VarOffset, usize)> = vec![];

        // Cross-test DUT reuse: if this component was already converted (earlier
        // test/instance), restore its subtree relocated to this instance's
        // offsets, skipping IR assembly AND codegen.  `child_variable_meta` and
        // the port copies / event remap below are still rebuilt fresh.
        let (reuse_hit, claim_guard) = match try_reuse_or_claim(
            component_key,
            alias_enabled,
            ff_start,
            comb_start,
            context.config.dut_reuse,
        ) {
            ReuseOutcome::Hit(r) => (Some(r), None),
            ReuseOutcome::Compute(g) => (None, Some(g)),
            ReuseOutcome::Disabled => (None, None),
        };
        if let Some(reuse) = reuse_hit {
            // Re-key the cached subtree's grandchild derived-clock event ids
            // (minted near u32::MAX) to fresh ones, else they collide with ids
            // this context mints for its own parts.  Small per-scope ids pass through.
            let threshold = air::VarId::from_raw(u32::MAX / 2);
            let mut id_map: HashMap<air::VarId, air::VarId> = HashMap::default();
            let mut intern = |vid: air::VarId, ctx: &mut Context| {
                if vid > threshold {
                    id_map
                        .entry(vid)
                        .or_insert_with(|| ctx.alloc_internal_event_id());
                }
            };
            for ev in reuse.event_statements.keys() {
                if let Event::Clock(v) | Event::Reset(v) = ev {
                    intern(*v, context);
                }
            }
            for (v, _, _) in &reuse.derived_clock_candidates {
                intern(*v, context);
            }
            let rekey = |ev: Event| -> Event {
                match ev {
                    Event::Clock(v) => Event::Clock(id_map.get(&v).copied().unwrap_or(v)),
                    Event::Reset(v) => Event::Reset(id_map.get(&v).copied().unwrap_or(v)),
                    other => other,
                }
            };
            all_event_statements = reuse
                .event_statements
                .into_iter()
                .map(|(ev, stmts)| (rekey(ev), stmts))
                .collect();
            all_comb_statements = reuse.comb_statements;
            all_post_comb_fns = reuse.post_comb_fns;
            all_child_modules = reuse.child_modules;
            all_derived_clock_candidates = reuse
                .derived_clock_candidates
                .into_iter()
                .map(|(v, off, nb)| (id_map.get(&v).copied().unwrap_or(v), off, nb))
                .collect();
            // Reserve the full region the reference conv consumed (declared
            // vars + function-local / temporary allocations + the whole nested
            // subtree).  `create_variable_meta` above only advanced by this
            // component's declared-var counts; the skipped child-decl loop is
            // where the rest was allocated.
            context.ff_total_bytes = ff_start as usize + reuse.ff_size;
            context.comb_total_bytes = comb_start as usize + reuse.comb_size;
        } else {
            let child_scope = ScopeContext {
                variable_meta: child_variable_meta.clone(),
                analyzer_context: child_analyzer_context,
                ff_table: child_ff_table.clone(),
                inst_reset_kind: crate::ir::module::collect_inst_reset_kinds(
                    &child_module.declarations,
                ),
                func_offset_index: None,
            };
            context.scope_contexts.push(child_scope);

            // If this component is the de-aliased DUT boundary, mark its whole
            // subtree so descendants stay aliased (only the topmost boundary is
            // de-aliased; internals relocate uniformly with the DUT).
            let prev_in_reuse_dut = context.in_reuse_dut;
            context.in_reuse_dut = prev_in_reuse_dut || !alias_enabled;

            for decl in child_decls {
                let mut proto_decl: ProtoDeclaration = Conv::conv(context, decl)?;

                for (event, mut stmts) in proto_decl.event_statements {
                    all_event_statements
                        .entry(event)
                        .and_modify(|v| v.append(&mut stmts))
                        .or_insert(stmts);
                }
                // Move, not clone: `proto_decl` is dropped after this iteration.
                all_comb_statements.append(&mut proto_decl.comb_statements);
                all_post_comb_fns.extend(proto_decl.post_comb_fns);
                all_child_modules.extend(proto_decl.child_modules);
                all_derived_clock_candidates.extend(proto_decl.derived_clock_candidates);
            }

            // A nested test module's initial/final statements merge into the
            // parent, where the same instance names may mean other instances;
            // resolve them against its own child tree while its scope is
            // current.
            crate::ir::hier_ref::resolve_hier_refs(
                context,
                &mut all_event_statements,
                &all_child_modules,
            )?;

            context.in_reuse_dut = prev_in_reuse_dut;
            context.scope_contexts.pop();

            try_compile_inst_chunks(
                context,
                src,
                ff_start,
                comb_start,
                alias_enabled,
                &mut all_event_statements,
                &mut all_comb_statements,
            );

            // Publish this component's whole internal subtree (statements +
            // child-module metas + nested derived-clock candidates) for later
            // instances/tests via the single-flight claim guard.  Dropping the
            // guard unfulfilled (on an earlier `?` error) releases the claim.
            // `all_derived_clock_candidates` here holds only the NESTED
            // (grandchild) candidates; this component's own derived clocks are
            // added later by the always-run event remap and stay per-instance.
            if let Some(guard) = claim_guard {
                let ff_size = context.ff_total_bytes - ff_start as usize;
                let comb_size = context.comb_total_bytes - comb_start as usize;
                guard.store(
                    ff_start,
                    comb_start,
                    ff_size,
                    comb_size,
                    &all_event_statements,
                    &all_comb_statements,
                    &all_post_comb_fns,
                    &all_child_modules,
                    &all_derived_clock_candidates,
                );
            }
        }

        // Input ports: parent expr → child port var
        for input in &src.inputs {
            if aliased_input_ids.contains(&input.id) {
                continue;
            }
            let child_meta = child_variable_meta.get(&input.id).unwrap();

            // One expression per port element: an unpacked-array slice, wired
            // element-wise.
            if input.exprs.len() != 1 {
                if input.exprs.len() != child_meta.elements.len() {
                    return Err(SimulatorError::unsupported_description(&src.token));
                }
                for (child_element, expr) in child_meta.elements.iter().zip(&input.exprs) {
                    let mut proto_expr: ProtoExpression = Conv::conv(context, expr)?;
                    size_literal_rhs(&mut proto_expr, None, None, child_meta.width);
                    all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                        dst: child_element.current,
                        dst_width: child_meta.width,
                        select: None,
                        dynamic_select: None,
                        rhs_select: None,
                        expr: proto_expr,
                        dst_ff_current_offset: 0, // not FF
                        token: TokenRange::default(),
                    }));
                }
                continue;
            }
            let input_expr = &input.exprs[0];

            // Array port fed by a const array (`inst u: Sub (i: pk::TBL)`).
            if let Some(exprs) = const_array_element_exprs(input_expr, child_meta.elements.len()) {
                for (child_element, expr) in child_meta.elements.iter().zip(exprs) {
                    all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                        dst: child_element.current,
                        dst_width: child_meta.width,
                        select: None,
                        dynamic_select: None,
                        rhs_select: None,
                        expr,
                        dst_ff_current_offset: 0, // not FF
                        token: TokenRange::default(),
                    }));
                }
                continue;
            }

            // Array port fed by an array literal (`inst u: Sub (p: '{default: '0})`).
            if let Some(exprs) = array_literal_element_exprs(
                context,
                input_expr,
                &child_meta.r#type,
                child_meta.elements.len(),
            ) {
                for (child_element, mut expr) in child_meta.elements.iter().zip(exprs) {
                    size_literal_rhs(&mut expr, None, None, child_meta.width);
                    all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                        dst: child_element.current,
                        dst_width: child_meta.width,
                        select: None,
                        dynamic_select: None,
                        rhs_select: None,
                        expr,
                        dst_ff_current_offset: 0, // not FF
                        token: TokenRange::default(),
                    }));
                }
                continue;
            }

            // Array port fed by a bare or constant partial-index variable
            // (e.g. `w_q[i]` from `logic [N, M]`): expand per-element.
            if child_meta.elements.len() > 1
                && let air::Expression::Term(factor) = input_expr
                && let air::Factor::Variable(parent_id, index, select, _) = factor.as_ref()
                && select.is_empty()
            {
                let parent_scope = context.scope();
                if let Some(parent_meta) = parent_scope.variable_meta.get(parent_id).cloned() {
                    let base_index = if index.0.is_empty() {
                        Some(0)
                    } else if let Some(idx_vals) =
                        index.eval_value(&mut parent_scope.analyzer_context)
                    {
                        partial_index_base(
                            parent_meta.r#type.array.as_slice(),
                            &idx_vals,
                            child_meta.elements.len(),
                            parent_meta.elements.len(),
                        )
                    } else {
                        None
                    };

                    if let Some(base) = base_index {
                        for i in 0..child_meta.elements.len() {
                            let child_element = &child_meta.elements[i];
                            let parent_element = &parent_meta.elements[base + i];
                            let parent_expr = ProtoExpression::Variable {
                                var_offset: parent_element.current,
                                select: None,
                                dynamic_select: None,
                                width: child_meta.width,
                                var_full_width: child_meta.width,
                                expr_context: ExpressionContext {
                                    width: child_meta.width,
                                    signed: false,
                                },
                            };
                            all_comb_statements.push(ProtoStatement::Assign(
                                ProtoAssignStatement {
                                    dst: child_element.current,
                                    dst_width: child_meta.width,
                                    select: None,
                                    dynamic_select: None,
                                    rhs_select: None,
                                    expr: parent_expr,
                                    dst_ff_current_offset: 0, // not FF
                                    token: TokenRange::default(),
                                },
                            ));
                        }
                        continue;
                    }
                }
            }

            let mut proto_expr: ProtoExpression = Conv::conv(context, input_expr)?;
            // No assignment statement here to size the literal.
            size_literal_rhs(&mut proto_expr, None, None, child_meta.width);
            let element = &child_meta.elements[0];
            all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                dst: element.current,
                dst_width: child_meta.width,
                select: None,
                dynamic_select: None,
                rhs_select: None,
                expr: proto_expr.clone(),
                dst_ff_current_offset: 0, // not FF
                token: TokenRange::default(),
            }));
        }

        // Output ports: child port var → parent dst.  A concat destructure
        // (`o: {a, b}`) slices the port value MSB-first within the concat's
        // TOTAL width — truncated / zero-extended like `assign {a, b} = o`.
        for output in &src.outputs {
            if aliased_output_ids.contains(&output.id) {
                continue;
            }
            let child_meta = child_variable_meta.get(&output.id).unwrap();
            let multi_dst = output.dst.len() > 1;
            // One dst per array element (`o: arr[2*n+:2]`) is element-wise
            // wiring, not a concat destructure of a single packed port value.
            let element_wise = multi_dst && child_meta.elements.len() == output.dst.len();
            let concat_dst = multi_dst && !element_wise;

            // Field widths for the windows below, which slice a single
            // packed child port value.
            let mut dst_widths: Vec<usize> = Vec::with_capacity(output.dst.len());
            if concat_dst {
                if child_meta.elements.len() != 1 {
                    return Err(SimulatorError::unsupported_description(
                        &output.dst[0].token,
                    ));
                }
                for parent_dst in &output.dst {
                    // A dynamic bit-select field has no constant window.
                    if !parent_dst.select.is_empty() && !parent_dst.select.is_const() {
                        return Err(SimulatorError::unsupported_description(&parent_dst.token));
                    }
                    let parent_scope = context.scope();
                    let width = if parent_dst.select.is_empty() {
                        parent_scope
                            .variable_meta
                            .get(&parent_dst.id)
                            .ok_or_else(|| {
                                SimulatorError::unsupported_description(&parent_dst.token)
                            })?
                            .width
                    } else {
                        let (beg, end) = parent_dst
                            .select
                            .eval_value(
                                &mut parent_scope.analyzer_context,
                                &parent_dst.comptime.r#type,
                                false,
                            )
                            .ok_or_else(|| {
                                SimulatorError::unsupported_description(&parent_dst.token)
                            })?;
                        beg - end + 1
                    };
                    dst_widths.push(width);
                }
            }
            let mut remaining: usize = dst_widths.iter().sum();

            for (dst_idx, parent_dst) in output.dst.iter().enumerate() {
                let (
                    parent_index,
                    parent_select,
                    parent_width,
                    parent_need_dynamic,
                    parent_width_shape,
                    parent_kind_width,
                ) = {
                    let parent_scope = context.scope();
                    let parent_meta = parent_scope.variable_meta.get(&parent_dst.id).unwrap();
                    let parent_index = parent_dst
                        .index
                        .eval_value(&mut parent_scope.analyzer_context)
                        .unwrap();

                    let select = if !parent_dst.select.is_empty() {
                        parent_dst.select.eval_value(
                            &mut parent_scope.analyzer_context,
                            &parent_dst.comptime.r#type,
                            false,
                        )
                    } else {
                        None
                    };
                    let need_dynamic =
                        !parent_dst.select.is_empty() && !parent_dst.select.is_const();
                    let select = if need_dynamic { None } else { select };
                    let width = parent_meta.width;
                    let width_shape = parent_meta.r#type.width().clone();
                    let kind_width = parent_meta.r#type.kind.width().unwrap_or(1);
                    (
                        parent_index,
                        select,
                        width,
                        need_dynamic,
                        width_shape,
                        kind_width,
                    )
                };

                let parent_dynamic_select = if parent_need_dynamic {
                    Some(build_dynamic_bit_select(
                        context,
                        &parent_width_shape,
                        &parent_dst.select,
                        parent_kind_width,
                    )?)
                } else {
                    None
                };

                // MSB-first window, clamped to the port width: bits above
                // it read zero (SV zero-extension of the RHS).
                let (rhs_select, field_above_port) = if concat_dst {
                    let (msb, lsb) = msb_first_window(&mut remaining, dst_widths[dst_idx]);
                    if lsb >= child_meta.width {
                        (None, true)
                    } else {
                        (Some((msb.min(child_meta.width - 1), lsb)), false)
                    }
                } else {
                    (None, false)
                };

                let parent_scope = context.scope();
                let parent_meta = parent_scope.variable_meta.get(&parent_dst.id).unwrap();

                // Parent elements to connect: full index → 1, empty index →
                // whole array, partial constant prefix → contiguous slice.
                let parent_element_indices: Vec<usize> =
                    if let Some(idx) = parent_meta.r#type.array.calc_index(&parent_index) {
                        vec![idx]
                    } else if parent_index.is_empty() && !parent_meta.r#type.array.is_empty() {
                        (0..parent_meta.elements.len()).collect()
                    } else if let Some(base) = partial_index_base(
                        parent_meta.r#type.array.as_slice(),
                        &parent_index,
                        child_meta.elements.len(),
                        parent_meta.elements.len(),
                    ) {
                        (base..base + child_meta.elements.len()).collect()
                    } else {
                        return Err(SimulatorError::unsupported_description(&parent_dst.token));
                    };

                // A concat field spanning multiple parent array elements
                // has no single rhs_select window.
                if multi_dst && parent_element_indices.len() != 1 {
                    return Err(SimulatorError::unsupported_description(&parent_dst.token));
                }

                for (elem_idx, &parent_elem_idx) in parent_element_indices.iter().enumerate() {
                    // Element-wise wiring puts one parent element per dst, so
                    // the child element advances with the dst.
                    let child_elem_idx = if element_wise { dst_idx } else { elem_idx };
                    let child_element = &child_meta.elements[child_elem_idx];
                    let parent_element = &parent_meta.elements[parent_elem_idx];

                    let child_expr = if field_above_port {
                        let width = dst_widths[dst_idx];
                        ProtoExpression::Value {
                            value: Value::new(0, width, false),
                            width,
                            expr_context: ExpressionContext {
                                width,
                                signed: false,
                            },
                        }
                    } else {
                        ProtoExpression::Variable {
                            var_offset: child_element.current,
                            select: None,
                            dynamic_select: None,
                            width: child_meta.width,
                            var_full_width: child_meta.width,
                            expr_context: ExpressionContext {
                                width: child_meta.width,
                                signed: false,
                            },
                        }
                    };

                    let dst_var = if parent_element.is_ff() {
                        VarOffset::Ff(parent_element.next_offset)
                    } else {
                        VarOffset::Comb(parent_element.current_offset())
                    };

                    let stmt = ProtoStatement::Assign(ProtoAssignStatement {
                        dst: dst_var,
                        dst_width: parent_width,
                        select: parent_select,
                        dynamic_select: parent_dynamic_select.clone(),
                        rhs_select,
                        expr: child_expr,
                        dst_ff_current_offset: parent_element.current_offset(),
                        token: TokenRange::default(),
                    });

                    all_comb_statements.push(stmt);
                }
            }
        }

        // Remap child event keys (clock/reset) to parent VarIds via input port connections
        let mut child_to_parent_var: HashMap<air::VarId, air::VarId> = HashMap::default();
        for input in &src.inputs {
            if let Some(air::Expression::Term(factor)) = input.single()
                && let air::Factor::Variable(parent_var_id, _, _, _) = factor.as_ref()
            {
                child_to_parent_var.insert(input.id, *parent_var_id);
            }
        }

        let mut remapped_events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        for (event, stmts) in all_event_statements {
            let new_event = match &event {
                Event::Clock(child_id) => {
                    if let Some(parent_id) = child_to_parent_var.get(child_id) {
                        Event::Clock(*parent_id)
                    } else {
                        event.clone()
                    }
                }
                Event::Reset(child_id) => {
                    if let Some(parent_id) = child_to_parent_var.get(child_id) {
                        Event::Reset(*parent_id)
                    } else {
                        event.clone()
                    }
                }
                _ => event.clone(),
            };
            remapped_events
                .entry(new_event)
                .and_modify(|v| v.extend(stmts.clone()))
                .or_insert(stmts);
        }

        // Derived-clock candidates: clock-typed non-port vars of this
        // child, monitored for 0→1 edges by the top simulator.  Offsets
        // in `child_variable_meta` are already absolute (parent-rebased);
        // non-port vars need no alias rewriting.
        //
        // Candidate and event key are re-keyed to a globally-unique id
        // here, where the owner scope closes: VarIds are per-module-scope,
        // so a nested id can numerically equal an ancestor's input-port
        // id, and the event remap at that boundary would hijack the key
        // onto an unrelated clock.  A reset-typed internal var gets the
        // same re-key, and no schedule candidate: nothing generates an
        // assertion edge for a reset a module produces itself, so its
        // `Event::Reset` never fires — the clock-edge level test in
        // `Event::Clock` is what serves those blocks.
        let child_port_var_set: HashSet<air::VarId> =
            child_module.ports.values().copied().collect();
        for (vid, var) in &child_module.variables {
            let is_clock = var.r#type.is_clock();
            let is_reset = var.r#type.is_reset();
            if !is_clock && !is_reset {
                continue;
            }
            if child_port_var_set.contains(vid) {
                continue;
            }
            if is_reset {
                if let Some(stmts) = remapped_events.remove(&Event::Reset(*vid)) {
                    let unique_id = context.alloc_internal_event_id();
                    remapped_events.insert(Event::Reset(unique_id), stmts);
                }
                continue;
            }
            if let Some(meta) = child_variable_meta.get(vid)
                && let Some(elem) = meta.elements.first()
            {
                let unique_id = context.alloc_internal_event_id();
                if let Some(stmts) = remapped_events.remove(&Event::Clock(*vid)) {
                    remapped_events.insert(Event::Clock(unique_id), stmts);
                }
                all_derived_clock_candidates.push((unique_id, elem.current, elem.native_bytes));
            }
        }

        let child_module_meta = ModuleVariableMeta {
            name: src.name,
            hierarchy: src.hierarchy.clone(),
            variable_meta: child_variable_meta,
            children: all_child_modules,
        };

        Ok(ProtoDeclaration {
            event_statements: remapped_events,
            comb_statements: all_comb_statements,
            post_comb_fns: all_post_comb_fns,
            child_modules: vec![child_module_meta],
            derived_clock_candidates: all_derived_clock_candidates,
            external_components: vec![],
        })
    }
}
