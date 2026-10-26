//! AIG → cell-IR technology mapping. Walks the (optimised) AIG and
//! emits compound library cells — Xor2/Xnor2/Mux2/Ao2x/Aoi2x/Oa2x/Oai2x/
//! And3/Or3 — for recognised subgraphs, falling back to plain And2/Not
//! cells for the rest. This recovers the high-level cell kinds lost when
//! [`aigify`] lowered the original `GateModule` to pure ANDs, so the
//! downstream area/delay estimate benefits from compound-cell library
//! data rather than an over-counted And2 + Not chain.
//!
//! The match engine is a single top-down greedy pass. For each AND node
//! in topological order we check a fixed list of templates (most
//! reductive first). A template matches only when every internal AND it
//! would consume is currently simple (not itself a compound root) and
//! has refcount 1 — otherwise the internal ANDs are still needed by
//! other consumers and must be emitted in their own right.
//!
//! [`aigify`]: crate::aig::convert::aigify

use std::collections::HashSet;

use crate::aig::graph::{AigEdge, AigModule, AigNode};
use crate::ir::{
    Cell, CellKind, GateModule, NET_CONST0, NET_CONST1, NetDriver, NetId, NetInfo, PortDir,
};

/// Convert an AIG back to a [`GateModule`]. `original` carries ports,
/// primary-input nets, and FF structure that must be preserved verbatim;
/// only combinational cells are regenerated.
pub fn aig_to_cells_techmap(aig: &AigModule, original: &GateModule) -> GateModule {
    let mut out = GateModule {
        name: original.name,
        ports: original.ports.clone(),
        nets: Vec::new(),
        cells: Vec::new(),
        ffs: original.ffs.clone(),
    };
    out.nets = original
        .nets
        .iter()
        .map(|n| NetInfo {
            driver: match n.driver {
                NetDriver::Const(b) => NetDriver::Const(b),
                NetDriver::PortInput => NetDriver::PortInput,
                NetDriver::FfQ(idx) => NetDriver::FfQ(idx),
                _ => NetDriver::Undriven,
            },
            origin: n.origin,
        })
        .collect();
    for (i, ff) in out.ffs.iter().enumerate() {
        if (ff.q as usize) < out.nets.len() {
            out.nets[ff.q as usize].driver = NetDriver::FfQ(i);
        }
    }
    for port in &out.ports {
        if matches!(port.dir, PortDir::Input) {
            for &net in &port.nets {
                out.nets[net as usize].driver = NetDriver::PortInput;
            }
        }
    }

    let refcount = compute_refcount(aig);
    let (pos_refs, neg_refs) = compute_polarity_refs(aig);
    let live = compute_live(aig);

    // pos_net[n] and neg_net[n] cache the nets providing node n's positive
    // and negated values. Constants and inputs have their nets fixed by
    // the original GateModule; ANDs allocate fresh nets on first use.
    let mut pos_net: Vec<Option<NetId>> = vec![None; aig.nodes.len()];
    let mut neg_net: Vec<Option<NetId>> = vec![None; aig.nodes.len()];
    pos_net[0] = Some(NET_CONST0);
    neg_net[0] = Some(NET_CONST1);
    for (idx, node) in aig.nodes.iter().enumerate() {
        if let AigNode::Input { origin } = node {
            pos_net[idx] = Some(*origin);
        }
    }

    // Decide each AND node's role in one pass. Because we process
    // ascending topo order, all fanin decisions are already made when we
    // reach a node — we only need to check whether they're "simple"
    // (unconsumed And2) to form a compound here.
    let n = aig.nodes.len();
    let mut role: Vec<NodeRole> = vec![NodeRole::Undecided; n];
    for idx in 0..n {
        if !live.contains(&(idx as u32)) {
            role[idx] = NodeRole::Dead;
            continue;
        }
        if let AigNode::And { fanin0, fanin1 } = aig.nodes[idx] {
            if let Some(m) = try_match(
                aig, idx as u32, fanin0, fanin1, &refcount, &pos_refs, &neg_refs, &role,
            ) {
                for &inner in &m.inner_ands {
                    role[inner as usize] = NodeRole::Consumed;
                }
                role[idx] = NodeRole::Compound(m);
            } else {
                role[idx] = NodeRole::Simple;
            }
        }
    }

    // Second pass: emit cells in topological order.
    for idx in 0..n {
        match &role[idx] {
            NodeRole::Undecided | NodeRole::Dead | NodeRole::Consumed => continue,
            NodeRole::Simple => {
                if let AigNode::And { fanin0, fanin1 } = aig.nodes[idx] {
                    let f0 = resolve(&mut out, &mut pos_net, &mut neg_net, fanin0);
                    let f1 = resolve(&mut out, &mut pos_net, &mut neg_net, fanin1);
                    let o0 = out.nets[f0 as usize].origin;
                    let o1 = out.nets[f1 as usize].origin;
                    let net = alloc_net(&mut out, o0.or(o1));
                    let cell_idx = out.cells.len();
                    out.cells.push(Cell {
                        kind: CellKind::And2,
                        inputs: vec![f0, f1],
                        output: net,
                    });
                    out.nets[net as usize].driver = NetDriver::Cell(cell_idx);
                    pos_net[idx] = Some(net);
                }
            }
            NodeRole::Compound(m) => {
                // Clone to release borrow on role.
                let kind = m.kind;
                let inputs = m.inputs.clone();
                let input_nets: Vec<NetId> = inputs
                    .iter()
                    .map(|&e| resolve(&mut out, &mut pos_net, &mut neg_net, e))
                    .collect();
                let origin_hint = input_nets
                    .iter()
                    .filter_map(|&n| out.nets[n as usize].origin)
                    .next();
                let net = alloc_net(&mut out, origin_hint);
                let cell_idx = out.cells.len();
                out.cells.push(Cell {
                    kind,
                    inputs: input_nets,
                    output: net,
                });
                out.nets[net as usize].driver = NetDriver::Cell(cell_idx);
                // The compound's output IS the positive value of this AND
                // node unless `output_is_negated` is true — in that case
                // the cell actually computes !top_and, so we register it
                // as the neg net and lazily invert for positive consumers.
                if m.output_is_negated {
                    neg_net[idx] = Some(net);
                } else {
                    pos_net[idx] = Some(net);
                }
            }
        }
    }

    // Wire sinks: ports first, then FF Ds.
    let port_out_count: usize = original
        .ports
        .iter()
        .filter(|p| matches!(p.dir, PortDir::Output | PortDir::Inout))
        .map(|p| p.nets.len())
        .sum();
    for (i, sink) in aig.sinks.iter().enumerate() {
        let src_net = resolve(&mut out, &mut pos_net, &mut neg_net, sink.edge);
        if i < port_out_count {
            let target = sink.target;
            if src_net != target {
                let cell_idx = out.cells.len();
                out.cells.push(Cell {
                    kind: CellKind::Buf,
                    inputs: vec![src_net],
                    output: target,
                });
                out.nets[target as usize].driver = NetDriver::Cell(cell_idx);
            }
        } else {
            let ff_idx = i - port_out_count;
            out.ffs[ff_idx].d = src_net;
        }
    }
    out
}

#[derive(Debug, Clone)]
enum NodeRole {
    Undecided,
    Dead,
    Simple,
    Compound(CompoundMatch),
    Consumed,
}

#[derive(Debug, Clone)]
struct CompoundMatch {
    kind: CellKind,
    /// Input edges in the order the `kind` expects.
    inputs: Vec<AigEdge>,
    /// Inner AND nodes whose cells are skipped (absorbed into this compound).
    inner_ands: Vec<u32>,
    /// When true, the compound cell actually produces the NEGATED value
    /// of the matched top AND node — the emitter caches it in `neg_net`.
    /// Used for patterns like Xor2/Ao21 where the canonical AIG shape
    /// already has an inverted output edge.
    output_is_negated: bool,
}

/// Count references to each node: other nodes' fanins plus sinks.
fn compute_refcount(aig: &AigModule) -> Vec<u32> {
    let mut rc = vec![0u32; aig.nodes.len()];
    for node in &aig.nodes {
        if let AigNode::And { fanin0, fanin1 } = node {
            rc[fanin0.node() as usize] += 1;
            rc[fanin1.node() as usize] += 1;
        }
    }
    for sink in &aig.sinks {
        rc[sink.edge.node() as usize] += 1;
    }
    rc
}

/// Per-polarity reference counts. Tells us whether downstream consumers
/// of a node want its positive, negated, or both values — useful for
/// picking the "right" compound cell kind (Or2 vs Nor2, Xor2 vs Xnor2).
fn compute_polarity_refs(aig: &AigModule) -> (Vec<u32>, Vec<u32>) {
    let n = aig.nodes.len();
    let mut pos = vec![0u32; n];
    let mut neg = vec![0u32; n];
    let mut bump = |e: AigEdge| {
        if e.is_negated() {
            neg[e.node() as usize] += 1;
        } else {
            pos[e.node() as usize] += 1;
        }
    };
    for node in &aig.nodes {
        if let AigNode::And { fanin0, fanin1 } = node {
            bump(*fanin0);
            bump(*fanin1);
        }
    }
    for sink in &aig.sinks {
        bump(sink.edge);
    }
    (pos, neg)
}

/// Reachability from any sink.
fn compute_live(aig: &AigModule) -> HashSet<u32> {
    let mut live = HashSet::new();
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
    live
}

/// Information about an AND fanin that we've committed to absorbing.
#[derive(Clone, Copy)]
struct InnerAnd {
    node: u32,
    f0: AigEdge,
    f1: AigEdge,
    /// Whether the edge from root to this inner is negated.
    edge_negated: bool,
}

/// Classify a fanin edge. Returns `Some` when the edge points at a
/// currently-simple AND with refcount 1 — i.e., it is safe to absorb.
fn inner_of(
    aig: &AigModule,
    root: u32,
    e: AigEdge,
    refcount: &[u32],
    role: &[NodeRole],
) -> Option<InnerAnd> {
    let n = e.node();
    if n == root {
        return None;
    }
    if refcount[n as usize] != 1 {
        return None;
    }
    if !matches!(role[n as usize], NodeRole::Simple) {
        return None;
    }
    match aig.nodes[n as usize] {
        AigNode::And { fanin0, fanin1 } => Some(InnerAnd {
            node: n,
            f0: fanin0,
            f1: fanin1,
            edge_negated: e.is_negated(),
        }),
        _ => None,
    }
}

/// Attempt to match a compound pattern rooted at AND node `root` with
/// the given fanin edges. Templates are tried in order of size savings.
#[allow(clippy::too_many_arguments)]
fn try_match(
    aig: &AigModule,
    root: u32,
    fanin0: AigEdge,
    fanin1: AigEdge,
    refcount: &[u32],
    pos_refs: &[u32],
    neg_refs: &[u32],
    role: &[NodeRole],
) -> Option<CompoundMatch> {
    let in0 = inner_of(aig, root, fanin0, refcount, role);
    let in1 = inner_of(aig, root, fanin1, refcount, role);

    // 4-input family: both fanins are absorbable ANDs with both edges
    // negated. Top+ = !(A1 | A2) where A1 = a.f0 & a.f1, A2 = b.f0 & b.f1.
    // If either edge is positive the top encodes And4 (no dedicated cell)
    // so we fall through.
    if let (Some(a), Some(b)) = (in0, in1)
        && a.edge_negated
        && b.edge_negated
    {
        if let Some((x, y, top_is_xor)) = match_xor_pair(a.f0, a.f1, b.f0, b.f1) {
            return Some(pick_xor_polarity(
                root, x, y, a.node, b.node, top_is_xor, pos_refs, neg_refs,
            ));
        }
        if let Some((s, d0, d1)) = match_mux_pair(a.f0, a.f1, b.f0, b.f1) {
            return Some(CompoundMatch {
                kind: CellKind::Mux2,
                inputs: vec![s, d0, d1],
                inner_ands: vec![a.node, b.node],
                output_is_negated: true,
            });
        }
        return Some(CompoundMatch {
            kind: CellKind::Aoi22,
            inputs: vec![a.f0, a.f1, b.f0, b.f1],
            inner_ands: vec![a.node, b.node],
            output_is_negated: false,
        });
    }

    // 3-input family: exactly one fanin is an absorbable AND.
    if let Some((inner, leaf_edge)) = match (in0, in1) {
        (Some(a), None) => Some((a, fanin1)),
        (None, Some(b)) => Some((b, fanin0)),
        _ => None,
    } {
        // Derivation: top = AND(inner_edge, leaf_edge) where inner_edge has
        // polarity `inner.edge_negated` and leaf_edge has polarity
        // `leaf_edge.is_negated()`. Let IN = inner.f0 & inner.f1.
        //   (+, +) top+ = IN & leaf      → And3(f0, f1, leaf)
        //   (+, -) top+ = IN & !leaf     → no clean 1-cell match
        //   (-, +) top+ = !IN & leaf     → (!f0 | !f1) & leaf = Oa21(!f0, !f1, leaf)
        //   (-, -) top+ = !IN & !leaf    → !(IN | leaf) = Aoi21(f0, f1, leaf+)
        return Some(match (inner.edge_negated, leaf_edge.is_negated()) {
            (false, false) => CompoundMatch {
                kind: CellKind::And3,
                inputs: vec![inner.f0, inner.f1, leaf_edge],
                inner_ands: vec![inner.node],
                output_is_negated: false,
            },
            (true, false) => CompoundMatch {
                kind: CellKind::Oa21,
                inputs: vec![inner.f0.negate(), inner.f1.negate(), leaf_edge],
                inner_ands: vec![inner.node],
                output_is_negated: false,
            },
            (true, true) => CompoundMatch {
                kind: CellKind::Aoi21,
                inputs: vec![inner.f0, inner.f1, leaf_edge.negate()],
                inner_ands: vec![inner.node],
                output_is_negated: false,
            },
            _ => return None,
        });
    }

    // 2-input family: top = AND(leaf0, leaf1) with no absorbable ANDs.
    //   (+, +) plain And2, no compound.
    //   (-, -) top+ = !a & !b = Nor2; negated = Or2.
    //   (+, -) or (-, +) top+ = a & !b or !a & b — just a plain And2 with
    //     one edge negated, already handled by resolve().
    if fanin0.is_negated() && fanin1.is_negated() {
        return Some(pick_or_polarity(root, fanin0, fanin1, pos_refs, neg_refs));
    }

    None
}

/// Pick between Xor2 and Xnor2 based on how the root node is referenced
/// downstream. `top_is_xor` says whether the positive value of the top
/// AND already equals XOR (form "same") or XNOR (form "crossed").
#[allow(clippy::too_many_arguments)]
fn pick_xor_polarity(
    root: u32,
    x: AigEdge,
    y: AigEdge,
    ia: u32,
    ib: u32,
    top_is_xor: bool,
    pos_refs: &[u32],
    neg_refs: &[u32],
) -> CompoundMatch {
    let pos = pos_refs[root as usize];
    let neg = neg_refs[root as usize];
    // Prefer the cell that matches the dominant consumer polarity to
    // avoid adding a trailing Not. "positive consumer dominant" means we
    // want the cell whose output equals the top AND's positive value.
    let positive_is_xor = top_is_xor;
    let want_positive = pos >= neg;
    let emit_xor = want_positive == positive_is_xor;
    let kind = if emit_xor {
        CellKind::Xor2
    } else {
        CellKind::Xnor2
    };
    // output_is_negated = true when the emitted cell's output equals the
    // top AND's *negated* value.
    let output_is_negated = emit_xor != top_is_xor;
    CompoundMatch {
        kind,
        inputs: vec![x, y],
        inner_ands: vec![ia, ib],
        output_is_negated,
    }
}

/// Pick between Or2 and Nor2 for an AND-with-both-fanins-negated node.
/// top+ = Nor2(a, b) with a = !fanin0, b = !fanin1; top- = Or2(a, b).
fn pick_or_polarity(
    root: u32,
    f0: AigEdge,
    f1: AigEdge,
    pos_refs: &[u32],
    neg_refs: &[u32],
) -> CompoundMatch {
    let pos = pos_refs[root as usize];
    let neg = neg_refs[root as usize];
    let a = f0.negate();
    let b = f1.negate();
    if neg >= pos {
        CompoundMatch {
            kind: CellKind::Or2,
            inputs: vec![a, b],
            inner_ands: vec![],
            output_is_negated: true,
        }
    } else {
        CompoundMatch {
            kind: CellKind::Nor2,
            inputs: vec![a, b],
            inner_ands: vec![],
            output_is_negated: false,
        }
    }
}

/// Xor2 pair check. There are two canonical AIG forms for XOR(x, y):
///   Form "crossed":  AND(x, !y) and AND(!x, y)  — top AND output is XNOR
///   Form "same":     AND(x, y)  and AND(!x, !y) — top AND output is XOR
/// Returns `(X, Y, top_is_xor)` where X, Y are positive edges and
/// `top_is_xor` is true iff the top AND's positive value is XOR
/// (form "same") rather than XNOR (form "crossed").
fn match_xor_pair(
    a0: AigEdge,
    a1: AigEdge,
    b0: AigEdge,
    b1: AigEdge,
) -> Option<(AigEdge, AigEdge, bool)> {
    let combos: [[AigEdge; 4]; 4] = [
        [a0, a1, b0, b1],
        [a0, a1, b1, b0],
        [a1, a0, b0, b1],
        [a1, a0, b1, b0],
    ];
    for [p0, p1, q0, q1] in combos {
        if p0.node() == q0.node()
            && p0.is_negated() != q0.is_negated()
            && p1.node() == q1.node()
            && p1.is_negated() != q1.is_negated()
            && p0.node() != p1.node()
        {
            // Form discriminator: within inner1 = AND(p0, p1), do its two
            // fanins have opposite polarities (crossed) or the same
            // polarity (same)?
            let top_is_xor = p0.is_negated() == p1.is_negated();
            let x = if !p0.is_negated() { p0 } else { q0 };
            let y = if !p1.is_negated() { p1 } else { q1 };
            return Some((x, y, top_is_xor));
        }
    }
    None
}

/// Mux2 pair check: is this `AND(S, D1)` and `AND(!S, D0)` for some S, D0, D1?
/// Returns (S, D0, D1) as edges.
fn match_mux_pair(
    a0: AigEdge,
    a1: AigEdge,
    b0: AigEdge,
    b1: AigEdge,
) -> Option<(AigEdge, AigEdge, AigEdge)> {
    // Find the "select" candidate: one edge in {a0,a1} must share node with
    // one edge in {b0,b1} with opposite polarity; the other edges become D0, D1.
    let a_pairs = [(a0, a1), (a1, a0)];
    let b_pairs = [(b0, b1), (b1, b0)];
    for (sa, da) in a_pairs {
        for (sb, db) in b_pairs {
            if sa.node() == sb.node() && sa.is_negated() != sb.is_negated() {
                // S is the positive polarity.
                let s = if !sa.is_negated() { sa } else { sb };
                // D1 is the arm next to positive S, D0 is the arm next to !S.
                let (d1, d0) = if !sa.is_negated() { (da, db) } else { (db, da) };
                if d0.node() != d1.node() || d0.is_negated() != d1.is_negated() {
                    return Some((s, d0, d1));
                }
            }
        }
    }
    None
}

/// Allocate a fresh net in `out`.
fn alloc_net(
    out: &mut GateModule,
    origin_hint: Option<(veryl_parser::resource_table::StrId, usize)>,
) -> NetId {
    let id = out.nets.len() as NetId;
    out.nets.push(NetInfo {
        driver: NetDriver::Undriven,
        origin: origin_hint,
    });
    id
}

/// Resolve an AigEdge to a concrete net in the output module, allocating
/// an inverter on demand for negated edges (or for positive edges when
/// only the negated net was recorded by an `output_is_negated` compound).
fn resolve(
    out: &mut GateModule,
    pos_net: &mut [Option<NetId>],
    neg_net: &mut [Option<NetId>],
    edge: AigEdge,
) -> NetId {
    let idx = edge.node() as usize;
    let (want_pos, want_neg) = if edge.is_negated() {
        (neg_net, pos_net)
    } else {
        (pos_net, neg_net)
    };
    if let Some(n) = want_pos[idx] {
        return n;
    }
    // Constant 0 / 1: if the positive side is missing (shouldn't happen
    // in practice — node 0 is always seeded), treat the negation of the
    // const lane as the opposite constant net.
    if idx == 0 && !edge.is_negated() {
        return NET_CONST0;
    }
    if idx == 0 && edge.is_negated() {
        return NET_CONST1;
    }
    // Materialise from the opposite polarity via a Not cell.
    let src =
        want_neg[idx].unwrap_or_else(|| panic!("no net available for node {idx} (edge {edge:?})"));
    let origin = out.nets[src as usize].origin;
    let net = out.nets.len() as NetId;
    out.nets.push(NetInfo {
        driver: NetDriver::Undriven,
        origin,
    });
    let cell_idx = out.cells.len();
    out.cells.push(Cell {
        kind: CellKind::Not,
        inputs: vec![src],
        output: net,
    });
    out.nets[net as usize].driver = NetDriver::Cell(cell_idx);
    want_pos[idx] = Some(net);
    net
}
