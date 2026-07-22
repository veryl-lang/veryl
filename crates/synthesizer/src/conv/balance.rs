//! Depth-reducing rebalance of associative gate chains.
//!
//! `conv` builds log-depth structures for arithmetic, but a reduction written
//! as a running fold (`&`/`|`/`^` reductions, RTL accumulating into one
//! variable across an unrolled loop) arrives as a left-deep ripple, depth
//! O(N) where yosys+ABC produce O(log N).
//!
//! Chains of one operator whose interior nodes have exactly one consumer are
//! rebuilt as minimum-depth trees, pairing the two earliest-arriving operands
//! each step (Huffman on arrival level). Interior fan-out-1 is required: a
//! node observed elsewhere can't be dissolved without duplicating its cone.
//! The same `N-1` cells remain, re-linked; the ops are associative and
//! commutative, so any tree over the same leaf multiset computes the same
//! value. Dissolved interiors are swept by the following DCE.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::ir::{Cell, CellKind, GateModule, NetDriver, NetId, NetInfo};

/// Narrower chains are left to compound-cell fusion (see [`rebalance_chain`]).
const MIN_CHAIN_LEAVES: usize = 8;

/// Associative and commutative, so any tree shape / operand order is valid.
fn is_assoc(kind: CellKind) -> bool {
    matches!(kind, CellKind::And2 | CellKind::Or2 | CellKind::Xor2)
}

/// Longest cell-path to each net, counting non-`Buf` cells (a `Buf` is a wire
/// alias, not a gate). Iterative post-order so a deep ripple can't overflow
/// the stack; the gate IR is acyclic (comb loops are an analyzer error, FF-Q
/// breaks sequential cycles), so the walk terminates.
fn compute_levels(module: &GateModule) -> Vec<u32> {
    const UNVISITED: u32 = u32::MAX;
    let n = module.nets.len();
    let mut level = vec![UNVISITED; n];
    let mut stack: Vec<NetId> = Vec::new();

    for start in 0..n as NetId {
        if level[start as usize] != UNVISITED {
            continue;
        }
        stack.push(start);
        while let Some(&net) = stack.last() {
            let NetDriver::Cell(ci) = module.nets[net as usize].driver else {
                level[net as usize] = 0;
                stack.pop();
                continue;
            };
            // Push the first not-yet-resolved input, else fold this cell.
            let mut pending = None;
            for &inp in &module.cells[ci].inputs {
                if level[inp as usize] == UNVISITED {
                    pending = Some(inp);
                    break;
                }
            }
            match pending {
                Some(inp) => stack.push(inp),
                None => {
                    let mx = module.cells[ci]
                        .inputs
                        .iter()
                        .map(|&i| level[i as usize])
                        .max()
                        .unwrap_or(0);
                    level[net as usize] = mx + u32::from(module.cells[ci].kind != CellKind::Buf);
                    stack.pop();
                }
            }
        }
    }
    level
}

/// Per-net consumer census. A net read by an FF, port, or RAM must survive
/// dissolution, so those consumers are tracked as "non-cell".
struct Fanout {
    count: Vec<u32>,
    /// Meaningful only when `count == 1 && !non_cell` (the sole cell
    /// consumer); otherwise an arbitrary last-counted consumer.
    consumer_cell: Vec<u32>,
    non_cell: Vec<bool>,
}

fn compute_fanout(module: &GateModule) -> Fanout {
    let n = module.nets.len();
    let mut count = vec![0u32; n];
    let mut consumer_cell = vec![u32::MAX; n];
    let mut non_cell = vec![false; n];

    for (ci, cell) in module.cells.iter().enumerate() {
        for &inp in &cell.inputs {
            count[inp as usize] += 1;
            consumer_cell[inp as usize] = ci as u32;
        }
    }
    let mark_non_cell = |net: NetId, count: &mut [u32], non_cell: &mut [bool]| {
        count[net as usize] += 1;
        non_cell[net as usize] = true;
    };
    for ff in &module.ffs {
        mark_non_cell(ff.d, &mut count, &mut non_cell);
        mark_non_cell(ff.clock, &mut count, &mut non_cell);
        if let Some(r) = &ff.reset {
            mark_non_cell(r.net, &mut count, &mut non_cell);
        }
    }
    for port in &module.ports {
        for &net in &port.nets {
            mark_non_cell(net, &mut count, &mut non_cell);
        }
    }
    module.for_each_ram_input_net(|net| mark_non_cell(net, &mut count, &mut non_cell));

    Fanout {
        count,
        consumer_cell,
        non_cell,
    }
}

impl Fanout {
    /// Whether `net`'s *driver* is a same-kind gate this chain can dissolve:
    /// its only observer is the descending chain node itself.
    fn producer_dissolvable(&self, net: NetId, k: CellKind, module: &GateModule) -> bool {
        if self.count[net as usize] != 1 || self.non_cell[net as usize] {
            return false;
        }
        let NetDriver::Cell(di) = module.nets[net as usize].driver else {
            return false;
        };
        module.cells[di].kind == k
    }

    /// Whether `c_idx`'s output goes fan-out-1 into a same-kind *consumer*,
    /// i.e. an enclosing chain will collect it — not a chain root.
    fn is_interior_node(&self, c_idx: usize, module: &GateModule) -> bool {
        let out = module.cells[c_idx].output;
        if self.count[out as usize] != 1 || self.non_cell[out as usize] {
            return false;
        }
        let consumer = self.consumer_cell[out as usize];
        module.cells[consumer as usize].kind == module.cells[c_idx].kind
    }
}

/// Rebalance every wide associative chain to minimum depth. Returns whether
/// anything changed.
pub(super) fn balance_assoc_chains(module: &mut GateModule) -> bool {
    let fanout = compute_fanout(module);
    let level = compute_levels(module);

    // Snapshot the roots before mutating; the rebuild only appends cells, so
    // original indices stay valid.
    let roots: Vec<usize> = module
        .cells
        .iter()
        .enumerate()
        .filter(|(_, c)| is_assoc(c.kind))
        .filter(|(i, _)| !fanout.is_interior_node(*i, module))
        .map(|(i, _)| i)
        .collect();

    let mut changed = false;
    for root in roots {
        changed |= rebalance_chain(module, root, &fanout, &level);
    }
    changed
}

/// Rebuild the chain rooted at `root` as a minimum-depth tree, reusing the
/// root cell as the final combine so its observed output net is preserved.
fn rebalance_chain(module: &mut GateModule, root: usize, fanout: &Fanout, level: &[u32]) -> bool {
    let kind = module.cells[root].kind;

    let mut leaves: Vec<NetId> = Vec::new();
    let mut stack: Vec<NetId> = module.cells[root].inputs.clone();
    while let Some(net) = stack.pop() {
        if fanout.producer_dissolvable(net, kind, module) {
            let NetDriver::Cell(di) = module.nets[net as usize].driver else {
                unreachable!("producer_dissolvable guarantees a cell driver");
            };
            stack.extend_from_slice(&module.cells[di].inputs);
        } else {
            leaves.push(net);
        }
    }

    // Short chains are usually fragments of a carry / comparator structure
    // that compound-cell fusion packs more tightly as a ripple; wide
    // reductions are what fusion leaves long. Only a cheap pre-filter — the
    // caller's keep-the-better guard is what prevents regressions.
    if leaves.len() < MIN_CHAIN_LEAVES {
        return false;
    }

    // New nets carry their level in the heap; original leaves read `level`.
    let mut heap: BinaryHeap<Reverse<(u32, NetId)>> = leaves
        .iter()
        .map(|&n| Reverse((level[n as usize], n)))
        .collect();

    let mut remaining = leaves.len() - 1;
    while remaining > 0 {
        let Reverse((la, na)) = heap.pop().expect("two operands available");
        let Reverse((lb, nb)) = heap.pop().expect("two operands available");
        remaining -= 1;
        if remaining == 0 {
            module.cells[root].inputs = vec![na, nb];
        } else {
            let new_net = module.nets.len() as NetId;
            module.nets.push(NetInfo {
                driver: NetDriver::Undriven,
                origin: None,
            });
            let idx = module.cells.len();
            module.cells.push(Cell {
                kind,
                inputs: vec![na, nb],
                output: new_net,
            });
            module.nets[new_net as usize].driver = NetDriver::Cell(idx);
            heap.push(Reverse((la.max(lb) + 1, new_net)));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{GatePort, PortDir};
    use std::collections::HashMap;
    use veryl_parser::resource_table;

    /// Left-deep ripple with fan-out-1 interiors, output on a port — the
    /// exact shape [`balance_assoc_chains`] rebalances.
    fn ripple_chain(kind: CellKind, n: usize) -> (GateModule, Vec<NetId>, NetId) {
        let mut m = GateModule::default();
        let inputs: Vec<NetId> = (0..n)
            .map(|_| {
                let id = m.nets.len() as NetId;
                m.nets.push(NetInfo {
                    driver: NetDriver::PortInput,
                    origin: None,
                });
                id
            })
            .collect();
        let mut acc = inputs[0];
        for &leaf in &inputs[1..] {
            let out = m.nets.len() as NetId;
            m.nets.push(NetInfo {
                driver: NetDriver::Undriven,
                origin: None,
            });
            let ci = m.cells.len();
            m.cells.push(Cell {
                kind,
                inputs: vec![acc, leaf],
                output: out,
            });
            m.nets[out as usize].driver = NetDriver::Cell(ci);
            acc = out;
        }
        m.ports.push(GatePort {
            name: resource_table::insert_str("y"),
            path: vec![],
            dir: PortDir::Output,
            nets: vec![acc],
        });
        (m, inputs, acc)
    }

    fn eval(m: &GateModule, net: NetId, inval: &HashMap<NetId, bool>) -> bool {
        match &m.nets[net as usize].driver {
            NetDriver::PortInput => inval[&net],
            NetDriver::Const(b) => *b,
            NetDriver::Cell(ci) => {
                let c = &m.cells[*ci];
                let a = eval(m, c.inputs[0], inval);
                match c.kind {
                    CellKind::Buf => a,
                    CellKind::Not => !a,
                    CellKind::And2 => a && eval(m, c.inputs[1], inval),
                    CellKind::Or2 => a || eval(m, c.inputs[1], inval),
                    CellKind::Xor2 => a ^ eval(m, c.inputs[1], inval),
                    other => panic!("unexpected cell {other:?} in test eval"),
                }
            }
            other => panic!("unexpected driver {other:?} in test eval"),
        }
    }

    fn reference(kind: CellKind, bits: &[bool]) -> bool {
        bits.iter()
            .copied()
            .reduce(|a, b| match kind {
                CellKind::And2 => a && b,
                CellKind::Or2 => a || b,
                CellKind::Xor2 => a ^ b,
                _ => unreachable!(),
            })
            .unwrap()
    }

    #[test]
    fn balance_reduces_depth_and_preserves_function() {
        let n = 12; // exhaustive over 2^12 vectors
        for kind in [CellKind::And2, CellKind::Or2, CellKind::Xor2] {
            let (mut m, inputs, out) = ripple_chain(kind, n);
            // Depth of the observed output net (the dissolved interior cells
            // linger until DCE, so the global max is not meaningful here).
            let out_depth = |m: &GateModule| compute_levels(m)[out as usize] as usize;
            let before = out_depth(&m);
            assert_eq!(before, n - 1, "ripple depth should be n-1");

            assert!(balance_assoc_chains(&mut m), "wide chain should balance");
            let after = out_depth(&m);
            assert!(
                after < before && after <= 5,
                "{kind:?}: depth {before} -> {after}, expected ~log2({n})"
            );

            for mask in 0..(1u32 << n) {
                let inval: HashMap<NetId, bool> = inputs
                    .iter()
                    .enumerate()
                    .map(|(i, &net)| (net, (mask >> i) & 1 == 1))
                    .collect();
                let bits: Vec<bool> = inputs.iter().map(|&net| inval[&net]).collect();
                assert_eq!(
                    eval(&m, out, &inval),
                    reference(kind, &bits),
                    "{kind:?} mismatch at mask {mask:#x}"
                );
            }
        }
    }

    #[test]
    fn narrow_chain_is_left_for_fusion() {
        let (mut m, _in, _out) = ripple_chain(CellKind::Or2, MIN_CHAIN_LEAVES - 1);
        assert!(
            !balance_assoc_chains(&mut m),
            "narrow chain must be skipped"
        );
    }

    #[test]
    fn tapped_interior_node_stays_a_leaf() {
        let n = 10;
        let (mut m, inputs, out) = ripple_chain(CellKind::And2, n);
        // Interior cell outputs occupy nets n, n+1, …; a tapped one gains a
        // second consumer and must stay a leaf.
        let mid = n as NetId + 2;
        m.ports.push(GatePort {
            name: resource_table::insert_str("tap"),
            path: vec![],
            dir: PortDir::Output,
            nets: vec![mid],
        });
        balance_assoc_chains(&mut m);
        for mask in 0..(1u32 << n) {
            let inval: HashMap<NetId, bool> = inputs
                .iter()
                .enumerate()
                .map(|(i, &net)| (net, (mask >> i) & 1 == 1))
                .collect();
            let bits: Vec<bool> = inputs.iter().map(|&net| inval[&net]).collect();
            assert_eq!(eval(&m, out, &inval), reference(CellKind::And2, &bits));
        }
    }
}
