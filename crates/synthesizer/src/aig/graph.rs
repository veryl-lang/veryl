//! And-Inverter Graph (AIG) intermediate representation for logic
//! optimization. The gate-level IR from `conv` is converted to this
//! form, optimized with structural hash-cons and rewriting, then mapped
//! back to primitives that `postpass` can fuse into compound cells.
//!
//! Each node in the AIG is either a constant, a primary input (from
//! module ports / FF Q outputs), or a 2-input AND whose operands are
//! signed edges. Edges carry a polarity bit so an inversion is just a
//! XOR of the LSB — no separate Not node is ever materialised.

use std::collections::HashMap;

use crate::ir::NetId;

/// Signed reference to an AIG node. LSB is polarity (1 = negated).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AigEdge(u32);

impl AigEdge {
    pub const CONST0: AigEdge = AigEdge(0);
    pub const CONST1: AigEdge = AigEdge(1);

    #[inline]
    pub fn new(node: u32, negated: bool) -> Self {
        AigEdge((node << 1) | (negated as u32))
    }

    #[inline]
    pub fn node(self) -> u32 {
        self.0 >> 1
    }

    #[inline]
    pub fn is_negated(self) -> bool {
        (self.0 & 1) == 1
    }

    #[inline]
    pub fn negate(self) -> Self {
        AigEdge(self.0 ^ 1)
    }

    #[inline]
    pub fn negate_if(self, cond: bool) -> Self {
        AigEdge(self.0 ^ (cond as u32))
    }

    #[inline]
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// The kind of AIG node. Index 0 is always `Const` representing constant
/// zero; `AigEdge::CONST0` points at it with polarity 0 and
/// `AigEdge::CONST1` with polarity 1.
#[derive(Clone, Debug)]
pub enum AigNode {
    /// The single constant node (node 0).
    Const,
    /// A primary input to the AIG. `origin` maps back to a cell-IR net
    /// when we re-emit gates (either a module input port or an FF Q).
    Input { origin: NetId },
    /// 2-input AND of signed edges.
    And { fanin0: AigEdge, fanin1: AigEdge },
}

/// Sink of the AIG — an edge that some downstream construct (port,
/// FF D, etc.) wants to observe.
#[derive(Clone, Debug)]
pub struct AigSink {
    /// The cell-IR net (port output or FF D input) this edge drives.
    pub target: NetId,
    /// The AIG edge whose value should be written to `target`.
    pub edge: AigEdge,
}

pub struct AigModule {
    pub nodes: Vec<AigNode>,
    /// One entry per combinational output: module output port bits plus
    /// every FF's D input. Mapping back to the gate IR fills these in.
    pub sinks: Vec<AigSink>,
    /// Structural hash-cons table: `(canonical_fanin0, canonical_fanin1)`
    /// → node index. Inputs are stored with the smaller raw edge first
    /// so `AND(a, b)` and `AND(b, a)` collide.
    hash_cons: HashMap<(AigEdge, AigEdge), u32>,
    /// Cache from cell-IR net → AIG edge for the combinational signals
    /// we've already lowered. Lets the aigifier convert each cell once.
    pub net_edge: HashMap<NetId, AigEdge>,
}

impl Default for AigModule {
    fn default() -> Self {
        let mut m = AigModule {
            nodes: Vec::new(),
            sinks: Vec::new(),
            hash_cons: HashMap::new(),
            net_edge: HashMap::new(),
        };
        // Node 0 is always the constant.
        m.nodes.push(AigNode::Const);
        m
    }
}

impl AigModule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate (or dedup) a primary-input node for `origin` and return
    /// its edge. Called once per input port bit / FF Q.
    pub fn add_input(&mut self, origin: NetId) -> AigEdge {
        if let Some(&e) = self.net_edge.get(&origin) {
            return e;
        }
        let idx = self.nodes.len() as u32;
        self.nodes.push(AigNode::Input { origin });
        let edge = AigEdge::new(idx, false);
        self.net_edge.insert(origin, edge);
        edge
    }

    /// Hash-consed 2-input AND. Handles the trivial identities locally
    /// so later optimizations don't need to revisit them:
    ///   AND(x, 0) = 0
    ///   AND(x, 1) = x
    ///   AND(x, x) = x
    ///   AND(x, !x) = 0
    pub fn mk_and(&mut self, mut a: AigEdge, mut b: AigEdge) -> AigEdge {
        // Absorbing / unit / reflexive constants.
        if a == AigEdge::CONST0 || b == AigEdge::CONST0 {
            return AigEdge::CONST0;
        }
        if a == AigEdge::CONST1 {
            return b;
        }
        if b == AigEdge::CONST1 {
            return a;
        }
        if a == b {
            return a;
        }
        if a == b.negate() {
            return AigEdge::CONST0;
        }

        // Canonicalize: smaller raw first.
        if a.0 > b.0 {
            std::mem::swap(&mut a, &mut b);
        }

        if let Some(&idx) = self.hash_cons.get(&(a, b)) {
            return AigEdge::new(idx, false);
        }
        let idx = self.nodes.len() as u32;
        self.nodes.push(AigNode::And {
            fanin0: a,
            fanin1: b,
        });
        self.hash_cons.insert((a, b), idx);
        AigEdge::new(idx, false)
    }

    /// OR via De Morgan: `a | b = !(!a & !b)`.
    pub fn mk_or(&mut self, a: AigEdge, b: AigEdge) -> AigEdge {
        self.mk_and(a.negate(), b.negate()).negate()
    }

    /// XOR via two ANDs: `a ^ b = (a & !b) | (!a & b)`.
    pub fn mk_xor(&mut self, a: AigEdge, b: AigEdge) -> AigEdge {
        let n1 = self.mk_and(a, b.negate());
        let n2 = self.mk_and(a.negate(), b);
        self.mk_or(n1, n2)
    }

    /// `s ? d1 : d0` — decomposes to `(s & d1) | (!s & d0)`.
    pub fn mk_mux(&mut self, s: AigEdge, d0: AigEdge, d1: AigEdge) -> AigEdge {
        let t = self.mk_and(s, d1);
        let e = self.mk_and(s.negate(), d0);
        self.mk_or(t, e)
    }

    /// Add a sink that the lowered form should drive.
    pub fn add_sink(&mut self, target: NetId, edge: AigEdge) {
        self.sinks.push(AigSink { target, edge });
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn and_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| matches!(n, AigNode::And { .. }))
            .count()
    }
}
