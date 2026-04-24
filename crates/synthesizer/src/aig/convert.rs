//! Conversion between the gate-level cell IR ([`crate::ir::GateModule`])
//! and the AIG IR ([`crate::aig::graph::AigModule`]). The AIG is used as an
//! optimisation intermediate: hash-consed construction fuses equivalent
//! subgraphs across the whole module, and structural rewrites can
//! ride on top (see the rewrite / balance passes). Mapping back emits
//! plain `And2` / `Not` cells so the existing postpass can fuse them
//! into compound library primitives.
//!
//! Only combinational cells are touched — FFs are preserved verbatim
//! across the round-trip; their Q outputs enter the AIG as primary
//! inputs and their D inputs become sinks.

use std::collections::HashSet;

use crate::aig::graph::{AigEdge, AigModule, AigNode};
use crate::ir::{
    Cell, CellKind, FfCell, GateModule, NET_CONST0, NET_CONST1, NetDriver, NetId, NetInfo, PortDir,
};

/// Convert the combinational part of `gate` into an AIG. Inputs (module
/// ports, FF Q outputs, undriven nets) become primary inputs; every
/// combinational cell is expanded into hash-consed ANDs; module output
/// ports and FF D inputs become sinks.
pub fn aigify(gate: &GateModule) -> AigModule {
    let mut aig = AigModule::new();

    // Seed constants onto the shared const-0 node.
    aig.net_edge.insert(NET_CONST0, AigEdge::CONST0);
    aig.net_edge.insert(NET_CONST1, AigEdge::CONST1);

    // Topological walk of cells driven by `net`, lowering each into
    // hash-consed ANDs. We memoise on net so shared nets lower once and
    // the hash-cons does the heavy CSE.
    fn lower_net(aig: &mut AigModule, gate: &GateModule, net: NetId) -> AigEdge {
        if let Some(&e) = aig.net_edge.get(&net) {
            return e;
        }
        let edge = match gate.nets[net as usize].driver {
            NetDriver::Const(false) => AigEdge::CONST0,
            NetDriver::Const(true) => AigEdge::CONST1,
            NetDriver::PortInput | NetDriver::FfQ(_) | NetDriver::Undriven => {
                // Treat any non-combinational driver as a primary input
                // for the AIG. The caller wires the same NetId back when
                // re-emitting cells.
                aig.add_input(net)
            }
            NetDriver::Cell(idx) => {
                let cell = &gate.cells[idx];
                lower_cell(aig, gate, cell)
            }
        };
        aig.net_edge.insert(net, edge);
        edge
    }

    fn lower_cell(aig: &mut AigModule, gate: &GateModule, cell: &Cell) -> AigEdge {
        use CellKind::*;
        // Resolve every input up-front so each mk_* call has exclusive
        // access to the AIG's `&mut` borrow.
        let inputs: Vec<AigEdge> = cell
            .inputs
            .iter()
            .map(|&n| lower_net(aig, gate, n))
            .collect();
        match cell.kind {
            Buf => inputs[0],
            Not => inputs[0].negate(),
            And2 => aig.mk_and(inputs[0], inputs[1]),
            Or2 => aig.mk_or(inputs[0], inputs[1]),
            Nand2 => aig.mk_and(inputs[0], inputs[1]).negate(),
            Nor2 => aig.mk_or(inputs[0], inputs[1]).negate(),
            Xor2 => aig.mk_xor(inputs[0], inputs[1]),
            Xnor2 => aig.mk_xor(inputs[0], inputs[1]).negate(),
            And3 => {
                let ab = aig.mk_and(inputs[0], inputs[1]);
                aig.mk_and(ab, inputs[2])
            }
            Or3 => {
                let ab = aig.mk_or(inputs[0], inputs[1]);
                aig.mk_or(ab, inputs[2])
            }
            Nand3 => {
                let ab = aig.mk_and(inputs[0], inputs[1]);
                aig.mk_and(ab, inputs[2]).negate()
            }
            Nor3 => {
                let ab = aig.mk_or(inputs[0], inputs[1]);
                aig.mk_or(ab, inputs[2]).negate()
            }
            Ao21 => {
                let ab = aig.mk_and(inputs[0], inputs[1]);
                aig.mk_or(ab, inputs[2])
            }
            Aoi21 => {
                let ab = aig.mk_and(inputs[0], inputs[1]);
                aig.mk_or(ab, inputs[2]).negate()
            }
            Oa21 => {
                let ab = aig.mk_or(inputs[0], inputs[1]);
                aig.mk_and(ab, inputs[2])
            }
            Oai21 => {
                let ab = aig.mk_or(inputs[0], inputs[1]);
                aig.mk_and(ab, inputs[2]).negate()
            }
            Ao31 => {
                let ab = aig.mk_and(inputs[0], inputs[1]);
                let abc = aig.mk_and(ab, inputs[2]);
                aig.mk_or(abc, inputs[3])
            }
            Aoi31 => {
                let ab = aig.mk_and(inputs[0], inputs[1]);
                let abc = aig.mk_and(ab, inputs[2]);
                aig.mk_or(abc, inputs[3]).negate()
            }
            Ao22 => {
                let ab = aig.mk_and(inputs[0], inputs[1]);
                let cd = aig.mk_and(inputs[2], inputs[3]);
                aig.mk_or(ab, cd)
            }
            Aoi22 => {
                let ab = aig.mk_and(inputs[0], inputs[1]);
                let cd = aig.mk_and(inputs[2], inputs[3]);
                aig.mk_or(ab, cd).negate()
            }
            Oai22 => {
                let ab = aig.mk_or(inputs[0], inputs[1]);
                let cd = aig.mk_or(inputs[2], inputs[3]);
                aig.mk_and(ab, cd).negate()
            }
            Mux2 => aig.mk_mux(inputs[0], inputs[1], inputs[2]),
        }
    }

    // Seed sinks: every output / FF D input. These root the traversal;
    // anything not reachable from them stays out of the AIG.
    for port in &gate.ports {
        if matches!(port.dir, PortDir::Output | PortDir::Inout) {
            for &net in &port.nets {
                let edge = lower_net(&mut aig, gate, net);
                aig.add_sink(net, edge);
            }
        }
    }
    for (i, ff) in gate.ffs.iter().enumerate() {
        let edge = lower_net(&mut aig, gate, ff.d);
        // Encode the FF index into the "target" net id via a negative
        // sentinel isn't possible (NetId is unsigned); instead we keep a
        // parallel list on the caller side. Here we stash the FF's D net
        // so the re-emit side knows which FF to rewire.
        let _ = i;
        aig.add_sink(ff.d, edge);
    }

    aig
}

/// Reconstruct a [`GateModule`] from an optimised AIG while preserving
/// the original ports and FFs. All combinational cells come from ANDs
/// and inverters in the AIG — the postpass then fuses them into
/// compound library cells.
pub fn aig_to_cells(aig: &AigModule, original: &GateModule) -> GateModule {
    let mut out = GateModule {
        name: original.name,
        ports: original.ports.clone(),
        nets: Vec::new(),
        cells: Vec::new(),
        ffs: original.ffs.clone(),
    };

    // Preserve the original net table layout so port / FF references
    // remain valid. All driver fields are wiped and re-populated below.
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

    // Re-attach FF Q nets (they were set via the original module's
    // net[q].driver = FfQ(idx)). Copy intact.
    for (i, ff) in out.ffs.iter().enumerate() {
        if (ff.q as usize) < out.nets.len() {
            out.nets[ff.q as usize].driver = NetDriver::FfQ(i);
        }
    }

    // Mark undriven internal nets — they'll be filled if they appear as
    // cell outputs from the AIG emission. Nets that only show up as
    // inputs will be re-driven when we wire sinks below.
    for port in &out.ports {
        if matches!(port.dir, PortDir::Input) {
            for &net in &port.nets {
                out.nets[net as usize].driver = NetDriver::PortInput;
            }
        }
    }

    // Compute reachable nodes from all sinks. Only these need to be
    // emitted — any AIG node not referenced by some sink is dead.
    let mut reachable: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = aig.sinks.iter().map(|s| s.edge.node()).collect();
    while let Some(idx) = stack.pop() {
        if !reachable.insert(idx) {
            continue;
        }
        if let AigNode::And { fanin0, fanin1 } = &aig.nodes[idx as usize] {
            stack.push(fanin0.node());
            stack.push(fanin1.node());
        }
    }

    // For each AIG node, pick a "positive" net in the output module
    // that carries its un-negated value. `Input` nodes use the origin
    // net (a module input or FF Q); `And` nodes allocate a fresh net.
    let mut pos_net: Vec<Option<NetId>> = vec![None; aig.nodes.len()];
    let mut neg_net: Vec<Option<NetId>> = vec![None; aig.nodes.len()];
    pos_net[0] = Some(NET_CONST0);
    neg_net[0] = Some(NET_CONST1);

    for (idx, node) in aig.nodes.iter().enumerate() {
        if let AigNode::Input { origin } = node {
            pos_net[idx] = Some(*origin);
        }
    }

    // Helper: allocate a fresh net in `out`, preserving the first-input
    // origin tag when we can derive one.
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

    // Emit cells in AIG topological order. Nodes are appended in
    // creation order so walking ascending is already topological.
    for idx in 0..aig.nodes.len() {
        if !reachable.contains(&(idx as u32)) {
            continue;
        }
        if let AigNode::And { fanin0, fanin1 } = aig.nodes[idx].clone() {
            // Resolve each fanin to a net in `out`. For negated fanins
            // we materialise (and cache) an inverter cell.
            let f0_net = resolve_fanin(&mut out, &mut pos_net, &mut neg_net, fanin0);
            let f1_net = resolve_fanin(&mut out, &mut pos_net, &mut neg_net, fanin1);
            let o0 = out.nets[f0_net as usize].origin;
            let o1 = out.nets[f1_net as usize].origin;

            let out_net = alloc_net(&mut out, o0.or(o1));
            let cell_idx = out.cells.len();
            out.cells.push(Cell {
                kind: CellKind::And2,
                inputs: vec![f0_net, f1_net],
                output: out_net,
            });
            out.nets[out_net as usize].driver = NetDriver::Cell(cell_idx);
            pos_net[idx] = Some(out_net);
        }
    }

    // Wire sinks: port outputs and FF D pins read from the edge's net.
    // Sinks appear in the order they were added: ports first, FFs
    // second (see `aigify`).
    let port_out_count: usize = original
        .ports
        .iter()
        .filter(|p| matches!(p.dir, PortDir::Output | PortDir::Inout))
        .map(|p| p.nets.len())
        .sum();

    for (i, sink) in aig.sinks.iter().enumerate() {
        let src_net = resolve_fanin(&mut out, &mut pos_net, &mut neg_net, sink.edge);
        if i < port_out_count {
            let target = sink.target;
            if src_net != target {
                // Buffer target so the worklist / postpass collapses the
                // redundant alias in a later pass.
                let cell_idx = out.cells.len();
                out.cells.push(Cell {
                    kind: CellKind::Buf,
                    inputs: vec![src_net],
                    output: target,
                });
                out.nets[target as usize].driver = NetDriver::Cell(cell_idx);
            }
        } else {
            // FF D: rewire directly to the resolved net.
            let ff_idx = i - port_out_count;
            out.ffs[ff_idx].d = src_net;
        }
    }

    out
}

/// Resolve an `AigEdge` to a net id in the output module, allocating
/// an inverter cell on demand for negated edges.
fn resolve_fanin(
    out: &mut GateModule,
    pos_net: &mut [Option<NetId>],
    neg_net: &mut [Option<NetId>],
    edge: AigEdge,
) -> NetId {
    let idx = edge.node() as usize;
    if edge.is_negated() {
        if let Some(n) = neg_net[idx] {
            return n;
        }
        // Need the positive net first, then a Not.
        let p = pos_net[idx].expect("positive net should be emitted before negated request");
        // Constants: the aig module uses node 0 for const0; its negation
        // is already const1, materialised as NET_CONST1.
        if idx == 0 {
            return NET_CONST1;
        }
        let origin = out.nets[p as usize].origin;
        let not_net = out.nets.len() as NetId;
        out.nets.push(NetInfo {
            driver: NetDriver::Undriven,
            origin,
        });
        let cell_idx = out.cells.len();
        out.cells.push(Cell {
            kind: CellKind::Not,
            inputs: vec![p],
            output: not_net,
        });
        out.nets[not_net as usize].driver = NetDriver::Cell(cell_idx);
        neg_net[idx] = Some(not_net);
        not_net
    } else {
        pos_net[idx].unwrap_or_else(|| panic!("AIG node {} has no positive net allocated yet", idx))
    }
}

/// Reference to silence dead_code when we extract `FfCell` just to keep
/// the `ffs` field unchanged across the round-trip.
#[allow(dead_code)]
fn _keep_ffcell_type_alive(_: &FfCell) {}
