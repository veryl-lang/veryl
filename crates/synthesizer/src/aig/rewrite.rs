//! AIG structural rewrite pass. For each AND node we enumerate ≤4-cuts,
//! compute each cut's truth table, look it up in the NPN4 library
//! ([`crate::aig::npn4`]), and — when the library's minimum-size AIG has
//! fewer ANDs than the current cone — rebuild the cone using the library
//! pattern. The rewrite runs in topological order; hash-consing in the
//! destination AIG makes sharing opportunistic.
//!
//! This is a light-weight "one-shot rewrite" pass: we don't do MFFC
//! reference counting or multi-round greedy application. The gain is
//! only the difference in local AND count; downstream dead nodes will be
//! swept later by the aig_to_cells reachability pass.

use std::collections::{HashMap, HashSet};

use crate::aig::graph::{AigEdge, AigModule, AigNode};
use crate::aig::npn4::{self, AigPattern, PatEdge, Tt4, VAR_TT};

const MAX_CUT_LEAVES: usize = 4;
const CUTS_PER_NODE: usize = 8;

/// Priority cut for an AIG node. `leaves` are sorted ascending node
/// indices (AIG nodes, not edges — polarity does not participate in
/// the cut). `cone_size` is the internal AND count of the cone rooted
/// at the owning node with these leaves.
#[derive(Clone, Debug)]
struct Cut {
    leaves: Vec<u32>,
    cone_size: u32,
}

/// Enumerate priority cuts for every AIG node. A node's cuts are
/// produced by merging pairs of cuts from its fanins plus the trivial
/// self-cut. Dominated cuts (same leaves, larger cone) are pruned; we
/// keep at most [`CUTS_PER_NODE`] cuts per node ordered by smaller leaf
/// count then smaller cone.
fn enumerate_cuts(aig: &AigModule) -> Vec<Vec<Cut>> {
    let n = aig.nodes.len();
    let mut cuts: Vec<Vec<Cut>> = vec![Vec::new(); n];
    for (i, node) in aig.nodes.iter().enumerate() {
        match node {
            AigNode::Const | AigNode::Input { .. } => {
                cuts[i].push(Cut {
                    leaves: vec![i as u32],
                    cone_size: 0,
                });
            }
            AigNode::And { fanin0, fanin1 } => {
                let a = fanin0.node() as usize;
                let b = fanin1.node() as usize;
                let mut own: Vec<Cut> = Vec::new();
                for ca in &cuts[a] {
                    for cb in &cuts[b] {
                        if let Some(m) = merge_cuts(ca, cb) {
                            own.push(m);
                        }
                    }
                }
                // Trivial self-cut.
                own.push(Cut {
                    leaves: vec![i as u32],
                    cone_size: 0,
                });
                // Deduplicate by leaf set, keeping the smallest cone_size.
                own.sort_by(|a, b| a.leaves.cmp(&b.leaves).then(a.cone_size.cmp(&b.cone_size)));
                own.dedup_by(|a, b| a.leaves == b.leaves);
                // Prioritise: smaller leaf count first, then smaller cone.
                own.sort_by(|a, b| {
                    a.leaves
                        .len()
                        .cmp(&b.leaves.len())
                        .then(a.cone_size.cmp(&b.cone_size))
                });
                own.truncate(CUTS_PER_NODE);
                cuts[i] = own;
            }
        }
    }
    cuts
}

/// Sorted-merge union of two cuts. Returns `None` if the union would
/// exceed [`MAX_CUT_LEAVES`].
fn merge_cuts(a: &Cut, b: &Cut) -> Option<Cut> {
    let mut out: Vec<u32> = Vec::with_capacity(MAX_CUT_LEAVES);
    let (mut i, mut j) = (0, 0);
    let (la, lb) = (a.leaves.len(), b.leaves.len());
    while i < la || j < lb {
        let take = match (i < la, j < lb) {
            (true, true) => {
                let av = a.leaves[i];
                let bv = b.leaves[j];
                match av.cmp(&bv) {
                    std::cmp::Ordering::Less => {
                        i += 1;
                        av
                    }
                    std::cmp::Ordering::Greater => {
                        j += 1;
                        bv
                    }
                    std::cmp::Ordering::Equal => {
                        i += 1;
                        j += 1;
                        av
                    }
                }
            }
            (true, false) => {
                let v = a.leaves[i];
                i += 1;
                v
            }
            (false, true) => {
                let v = b.leaves[j];
                j += 1;
                v
            }
            (false, false) => break,
        };
        if out.len() >= MAX_CUT_LEAVES {
            return None;
        }
        out.push(take);
    }
    Some(Cut {
        leaves: out,
        cone_size: a.cone_size + b.cone_size + 1,
    })
}

/// Compute the cut's truth table by evaluating the cone rooted at
/// `root` with each leaf assigned `VAR_TT[leaf_position]`. Returns
/// `None` if the cut is trivial (leaves = {root}) or has >4 leaves.
fn compute_cut_tt(aig: &AigModule, root: u32, cut: &Cut) -> Option<Tt4> {
    if cut.leaves.len() > 4 {
        return None;
    }
    if cut.leaves.len() == 1 && cut.leaves[0] == root {
        return None;
    }
    let mut leaf_tt: HashMap<u32, Tt4> = HashMap::with_capacity(cut.leaves.len());
    for (i, &n) in cut.leaves.iter().enumerate() {
        leaf_tt.insert(n, VAR_TT[i]);
    }
    let mut memo: HashMap<u32, Tt4> = HashMap::new();
    Some(eval_tt(aig, root, &leaf_tt, &mut memo))
}

fn eval_tt(
    aig: &AigModule,
    node: u32,
    leaf_tt: &HashMap<u32, Tt4>,
    memo: &mut HashMap<u32, Tt4>,
) -> Tt4 {
    if let Some(&t) = leaf_tt.get(&node) {
        return t;
    }
    if let Some(&t) = memo.get(&node) {
        return t;
    }
    let tt = match &aig.nodes[node as usize] {
        AigNode::Const => 0,
        AigNode::Input { .. } => {
            // Primary inputs not mentioned by the cut shouldn't appear
            // inside the cone (the cut enumeration ensures leaves cover
            // every live path). Treat any stray as 0 defensively.
            0
        }
        AigNode::And { fanin0, fanin1 } => {
            let a_raw = eval_tt(aig, fanin0.node(), leaf_tt, memo);
            let a = if fanin0.is_negated() { !a_raw } else { a_raw };
            let b_raw = eval_tt(aig, fanin1.node(), leaf_tt, memo);
            let b = if fanin1.is_negated() { !b_raw } else { b_raw };
            a & b
        }
    };
    memo.insert(node, tt);
    tt
}

/// Apply structural rewriting: build a new AIG in which combinational
/// subgraphs are replaced with smaller library-found equivalents where
/// profitable. Ports, FFs, and primary inputs are preserved.
pub fn rewrite(aig: &AigModule) -> AigModule {
    let cuts = enumerate_cuts(aig);

    let mut new_aig = AigModule::new();
    let mut new_edge: Vec<Option<AigEdge>> = vec![None; aig.nodes.len()];

    for (idx, node) in aig.nodes.iter().enumerate() {
        let e = match node {
            AigNode::Const => AigEdge::CONST0,
            AigNode::Input { origin } => new_aig.add_input(*origin),
            AigNode::And { fanin0, fanin1 } => {
                match try_library_rewrite(&mut new_aig, aig, idx as u32, &cuts[idx], &new_edge) {
                    Some(e) => e,
                    None => {
                        let e0 = new_edge[fanin0.node() as usize]
                            .expect("fanin edge emitted before AND (topo order)")
                            .negate_if(fanin0.is_negated());
                        let e1 = new_edge[fanin1.node() as usize]
                            .expect("fanin edge emitted before AND (topo order)")
                            .negate_if(fanin1.is_negated());
                        new_aig.mk_and(e0, e1)
                    }
                }
            }
        };
        new_edge[idx] = Some(e);
    }

    // Rewire sinks.
    for sink in &aig.sinks {
        let src = new_edge[sink.edge.node() as usize]
            .expect("sink node emitted")
            .negate_if(sink.edge.is_negated());
        new_aig.add_sink(sink.target, src);
    }

    // Strip dead nodes: rebuild the AIG keeping only nodes reachable
    // from some sink. Dead ANDs left behind by rewrite (original cone
    // nodes whose sole consumer was the rewritten root) go away here.
    compact(&new_aig)
}

/// Reachability-based dead code elimination. Walks from every sink's
/// edge backwards, copying live nodes into a fresh AIG in topological
/// order.
fn compact(aig: &AigModule) -> AigModule {
    // Mark reachable from sinks.
    let mut live: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = aig.sinks.iter().map(|s| s.edge.node()).collect();
    while let Some(idx) = stack.pop() {
        if !live.insert(idx) {
            continue;
        }
        if let AigNode::And { fanin0, fanin1 } = aig.nodes[idx as usize] {
            stack.push(fanin0.node());
            stack.push(fanin1.node());
        }
    }

    let mut out = AigModule::new();
    let mut new_edge: Vec<Option<AigEdge>> = vec![None; aig.nodes.len()];
    for (idx, node) in aig.nodes.iter().enumerate() {
        if !live.contains(&(idx as u32)) {
            continue;
        }
        let e = match node {
            AigNode::Const => AigEdge::CONST0,
            AigNode::Input { origin } => out.add_input(*origin),
            AigNode::And { fanin0, fanin1 } => {
                let e0 = new_edge[fanin0.node() as usize]
                    .expect("fanin live before AND in topo order")
                    .negate_if(fanin0.is_negated());
                let e1 = new_edge[fanin1.node() as usize]
                    .expect("fanin live before AND in topo order")
                    .negate_if(fanin1.is_negated());
                out.mk_and(e0, e1)
            }
        };
        new_edge[idx] = Some(e);
    }
    for sink in &aig.sinks {
        let src = new_edge[sink.edge.node() as usize]
            .expect("sink node live")
            .negate_if(sink.edge.is_negated());
        out.add_sink(sink.target, src);
    }
    out
}

/// Try every cut for `root` and return the library-built edge if any
/// cut has a strictly smaller pattern than the cone.
fn try_library_rewrite(
    new_aig: &mut AigModule,
    aig: &AigModule,
    root: u32,
    cuts: &[Cut],
    new_edge: &[Option<AigEdge>],
) -> Option<AigEdge> {
    let mut best: Option<(u32, AigEdge)> = None; // (pat_size, output_edge)
    for cut in cuts {
        if cut.leaves.len() < 2 || cut.leaves.len() > 4 {
            continue;
        }
        // Trivial cut (the root itself) — no rewrite available.
        if cut.leaves.len() == 1 && cut.leaves[0] == root {
            continue;
        }
        let tt = match compute_cut_tt(aig, root, cut) {
            Some(t) => t,
            None => continue,
        };
        let (canonical, t) = npn4::npn_canonical(tt);
        let pat = match npn4::lookup_canonical(canonical) {
            Some(p) => p,
            None => continue,
        };
        let pat_size = pat.size() as u32;
        if pat_size >= cut.cone_size {
            continue; // no improvement
        }
        // Resolve cut leaves as new-AIG edges.
        let mut leaf_edges: [AigEdge; 4] = [AigEdge::CONST0; 4];
        for (i, &leaf) in cut.leaves.iter().enumerate() {
            leaf_edges[i] = new_edge[leaf as usize].expect("leaf emitted");
        }
        // If cut has < 4 leaves, pad by repeating the first. The library
        // pattern for the canonical TT might still reference x2/x3; in
        // that case the duplicated leaf keeps semantics correct (the
        // canonical TT itself encoded the "don't-care" variable).
        for i in cut.leaves.len()..4 {
            leaf_edges[i] = leaf_edges[0];
        }
        // Build var_edges: canonical x_i ← leaf_edges[t.perm[i]] XOR t.in_neg_bit(i).
        let mut var_edges: [AigEdge; 4] = [AigEdge::CONST0; 4];
        for (i, ve) in var_edges.iter_mut().enumerate() {
            let leaf_pos = t.perm[i] as usize;
            let le = leaf_edges[leaf_pos];
            let neg = (t.in_neg >> i) & 1 != 0;
            *ve = le.negate_if(neg);
        }
        // Instantiate the canonical pattern.
        let out_edge = instantiate_pattern(new_aig, pat, &var_edges).negate_if(t.out_neg);

        match best {
            Some((bs, _)) if bs <= pat_size => {}
            _ => {
                best = Some((pat_size, out_edge));
            }
        }
    }
    best.map(|(_, e)| e)
}

/// Instantiate `pat` in `new_aig` by substituting its input variables
/// with `var_edges[0..4]`. Returns the edge representing the pattern's
/// output.
fn instantiate_pattern(
    new_aig: &mut AigModule,
    pat: &AigPattern,
    var_edges: &[AigEdge; 4],
) -> AigEdge {
    let mut node_edges: Vec<AigEdge> = Vec::with_capacity(4 + pat.ands.len());
    node_edges.extend_from_slice(var_edges);
    for &(a, b) in &pat.ands {
        let ea = resolve_pat_edge(&node_edges, a);
        let eb = resolve_pat_edge(&node_edges, b);
        node_edges.push(new_aig.mk_and(ea, eb));
    }
    resolve_pat_edge(&node_edges, pat.output)
}

#[inline]
fn resolve_pat_edge(node_edges: &[AigEdge], pe: PatEdge) -> AigEdge {
    let base = node_edges[pe.0 as usize];
    base.negate_if(pe.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aig::graph::AigModule;
    use crate::ir::NetId;

    /// Evaluate an AIG edge given the values of primary inputs.
    fn eval(aig: &AigModule, edge: AigEdge, inputs: &HashMap<NetId, bool>) -> bool {
        let base = match &aig.nodes[edge.node() as usize] {
            AigNode::Const => false,
            AigNode::Input { origin } => *inputs.get(origin).expect("input value missing"),
            AigNode::And { fanin0, fanin1 } => {
                let a = eval(aig, *fanin0, inputs);
                let b = eval(aig, *fanin1, inputs);
                a && b
            }
        };
        base ^ edge.is_negated()
    }

    fn sinks_equal(orig: &AigModule, rewritten: &AigModule, input_ids: &[NetId]) -> bool {
        let n = input_ids.len();
        for m in 0..(1u64 << n) {
            let mut inputs = HashMap::new();
            for (i, &id) in input_ids.iter().enumerate() {
                inputs.insert(id, ((m >> i) & 1) == 1);
            }
            for (so, sr) in orig.sinks.iter().zip(rewritten.sinks.iter()) {
                if eval(orig, so.edge, &inputs) != eval(rewritten, sr.edge, &inputs) {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn rewrite_preserves_and2() {
        let mut a = AigModule::new();
        let ea = a.add_input(10);
        let eb = a.add_input(11);
        let and = a.mk_and(ea, eb);
        a.add_sink(20, and);

        let b = rewrite(&a);
        assert_eq!(b.and_count(), 1);
        assert_eq!(b.sinks.len(), 1);
    }

    #[test]
    fn rewrite_cut_enumeration_has_paths() {
        let mut a = AigModule::new();
        let ea = a.add_input(10);
        let eb = a.add_input(11);
        let ec = a.add_input(12);
        let ab = a.mk_and(ea, eb);
        let abc = a.mk_and(ab, ec);
        a.add_sink(20, abc);

        let cuts = enumerate_cuts(&a);
        let root_cuts = &cuts[abc.node() as usize];
        let has_abc_cut = root_cuts.iter().any(|c| c.leaves.len() == 3);
        assert!(has_abc_cut, "no {{a,b,c}} cut found at root");
    }

    #[test]
    fn rewrite_preserves_xor2_semantics() {
        // xor = (a & !b) | (!a & b), 3 ANDs.
        let mut a = AigModule::new();
        let ea = a.add_input(10);
        let eb = a.add_input(11);
        let l = a.mk_and(ea, eb.negate());
        let r = a.mk_and(ea.negate(), eb);
        let xor = a.mk_or(l, r);
        a.add_sink(20, xor);

        let b = rewrite(&a);
        assert!(
            sinks_equal(&a, &b, &[10, 11]),
            "xor rewrite changed semantics"
        );
    }

    #[test]
    fn rewrite_preserves_and3_semantics() {
        let mut a = AigModule::new();
        let ea = a.add_input(10);
        let eb = a.add_input(11);
        let ec = a.add_input(12);
        let ab = a.mk_and(ea, eb);
        let abc = a.mk_and(ab, ec);
        a.add_sink(20, abc);

        let b = rewrite(&a);
        assert!(sinks_equal(&a, &b, &[10, 11, 12]));
    }

    #[test]
    fn rewrite_preserves_mux2_semantics() {
        // mux = (s & b) | (!s & a), 3 ANDs.
        let mut a = AigModule::new();
        let es = a.add_input(10);
        let ea = a.add_input(11);
        let eb = a.add_input(12);
        let t = a.mk_and(es, eb);
        let e = a.mk_and(es.negate(), ea);
        let mux = a.mk_or(t, e);
        a.add_sink(20, mux);

        let b = rewrite(&a);
        assert!(sinks_equal(&a, &b, &[10, 11, 12]));
    }

    #[test]
    fn rewrite_reduces_redundant_and() {
        // (a & b) & (a & c) = a & b & c — 3 distinct ANDs collapse to 2.
        let mut a = AigModule::new();
        let ea = a.add_input(10);
        let eb = a.add_input(11);
        let ec = a.add_input(12);
        let ab = a.mk_and(ea, eb);
        let ac = a.mk_and(ea, ec);
        let root = a.mk_and(ab, ac);
        a.add_sink(20, root);

        let b = rewrite(&a);
        assert!(
            b.and_count() <= 2,
            "rewrite produced {} ANDs, expected ≤ 2",
            b.and_count()
        );
        assert!(sinks_equal(&a, &b, &[10, 11, 12]));
    }
}
