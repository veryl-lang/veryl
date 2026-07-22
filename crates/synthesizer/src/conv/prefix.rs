//! Parallel-prefix restructuring of linear scan chains.
//!
//! An unrolled running scan whose every partial result is observed (a
//! priority encoder's `found`, an arbiter's grant mask, gray-to-binary's
//! running XOR) is a linear chain `g[j] = OP(g[j-1], leaf[j])` with
//! fan-out ≥ 2 interiors — [`super::balance`] cannot touch it, since
//! dissolving an observed node would duplicate its cone. Linear is depth
//! O(N) where yosys+ABC produce O(log N); these scans are the largest depth
//! gaps against the yosys reference (64-bit priority encoder: 64 vs 17).
//!
//! Each chain is rebuilt as a Sklansky network — every right-half prefix
//! combines with the left half's last prefix, depth ⌈log₂ N⌉ at
//! ~(N/2)·log₂ N gates. Operand order is preserved (contiguous ranges in
//! sequence), so only associativity is required. Original chain cells become
//! `Buf`s of their network prefix, keeping their output nets — consumers need
//! no rewiring. The gate-count increase is the classic area-for-depth trade;
//! the caller's keep-the-better guard arbitrates it.

use crate::ir::{Cell, CellKind, GateModule, NetDriver, NetId, NetInfo};

/// Below this the log-depth win is small and the Sklansky overhead is not.
const MIN_CHAIN_CELLS: usize = 8;

fn is_assoc(kind: CellKind) -> bool {
    matches!(kind, CellKind::And2 | CellKind::Or2 | CellKind::Xor2)
}

/// Structural absorption against an inner gate of the dual kind:
///   `a | (a & b)  → a`          `a & (a | b)  → a`
///   `a | (!a & b) → a | b`      `a & (!a | b) → a & b`
/// The inner keeps serving its other consumers (or dies via DCE), so nothing
/// is duplicated and the outer cone strictly shortens.
///
/// conv emits each stage of an unrolled priority scan as
/// `found | (!found & take)` — three levels; absorption collapses it to the
/// plain `found | take` scan that [`prefix_parallelize`] can rebuild. Runs
/// only on the restructuring clone, not in the always-on worklist: changing
/// the `(a&b)|c` shape also changes what compound-cell fusion can pack, which
/// measurably deepens some fused datapaths — the guard arbitrates that.
pub(super) fn absorb_complement(module: &mut GateModule) -> bool {
    let mut changed_any = false;
    loop {
        let mut changed = false;
        for ci in 0..module.cells.len() {
            let kind = module.cells[ci].kind;
            let inner_kind = match kind {
                CellKind::Or2 => CellKind::And2,
                CellKind::And2 => CellKind::Or2,
                _ => continue,
            };
            let complementary = |p: NetId, a: NetId| {
                let not_drives = |n: NetId, target: NetId| {
                    if let NetDriver::Cell(i) = module.nets[n as usize].driver {
                        let c = &module.cells[i];
                        c.kind == CellKind::Not && c.inputs[0] == target
                    } else {
                        false
                    }
                };
                not_drives(p, a) || not_drives(a, p)
            };
            let inputs = module.cells[ci].inputs.clone();
            'rewrite: for (xi, ai) in [(0, 1), (1, 0)] {
                let x = inputs[xi];
                let a = inputs[ai];
                let NetDriver::Cell(ix) = module.nets[x as usize].driver else {
                    continue;
                };
                let inner = &module.cells[ix];
                if inner.kind != inner_kind {
                    continue;
                }
                for (pi, qi) in [(0, 1), (1, 0)] {
                    let p = inner.inputs[pi];
                    let q = inner.inputs[qi];
                    if p == a {
                        module.cells[ci].kind = CellKind::Buf;
                        module.cells[ci].inputs = vec![a];
                        changed = true;
                        break 'rewrite;
                    }
                    if complementary(p, a) {
                        module.cells[ci].inputs = vec![q, a];
                        changed = true;
                        break 'rewrite;
                    }
                }
            }
        }
        changed_any |= changed;
        if !changed {
            break;
        }
    }
    changed_any
}

/// Rebuild every wide linear scan chain as a Sklansky prefix network.
/// Returns whether anything was rebuilt.
pub(super) fn prefix_parallelize(module: &mut GateModule) -> bool {
    // A net consumed by two same-kind cells forks the scan — neither consumer
    // extends a *linear* chain through it.
    let n_cells = module.cells.len();
    let mut same_kind_consumers = vec![0u32; module.nets.len()];
    for cell in &module.cells {
        if !is_assoc(cell.kind) {
            continue;
        }
        for &inp in &cell.inputs {
            if let NetDriver::Cell(di) = module.nets[inp as usize].driver
                && module.cells[di].kind == cell.kind
            {
                same_kind_consumers[inp as usize] += 1;
            }
        }
    }

    // `pred[c]` = the unique unforked same-kind cell feeding `c`. Both inputs
    // same-kind-driven means `c` is a tree join, not a scan step.
    let pred: Vec<Option<usize>> = (0..n_cells)
        .map(|ci| {
            let cell = &module.cells[ci];
            if !is_assoc(cell.kind) {
                return None;
            }
            let mut link = None;
            for &inp in &cell.inputs {
                if let NetDriver::Cell(di) = module.nets[inp as usize].driver
                    && module.cells[di].kind == cell.kind
                {
                    if link.is_some() {
                        return None; // tree join
                    }
                    if same_kind_consumers[inp as usize] != 1 {
                        return None; // forked scan
                    }
                    link = Some(di);
                }
            }
            link
        })
        .collect();

    let mut is_linked = vec![false; n_cells];
    for p in pred.iter().flatten() {
        is_linked[*p] = true;
    }

    let mut changed = false;
    // Walk maximal chains from each tail (a linked-from cell nobody links to).
    for tail in 0..n_cells {
        if is_linked[tail] || pred[tail].is_none() {
            continue;
        }
        let mut chain = vec![tail];
        let mut cur = tail;
        while let Some(p) = pred[cur] {
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        if chain.len() < MIN_CHAIN_CELLS {
            continue;
        }
        rebuild_chain(module, &chain);
        changed = true;
    }
    changed
}

/// Replace chain cells `c_0..c_m` (head first) with a Sklansky network over
/// their leaf sequence, aliasing each original output net to its prefix.
fn rebuild_chain(module: &mut GateModule, chain: &[usize]) {
    let kind = module.cells[chain[0]].kind;

    // Leaf sequence: the head contributes both inputs, every later cell its
    // non-chain input. `leaves[0..=j+1]` reduce to cell `c_j`'s value.
    let mut leaves: Vec<NetId> = module.cells[chain[0]].inputs.clone();
    for w in chain.windows(2) {
        let (p, c) = (w[0], w[1]);
        let p_out = module.cells[p].output;
        let cell = &module.cells[c];
        let leaf = if cell.inputs[0] == p_out {
            cell.inputs[1]
        } else {
            cell.inputs[0]
        };
        leaves.push(leaf);
    }

    let prefixes = sklansky(module, kind, &leaves);

    // The simplify that follows dissolves the Bufs; the network gates remain.
    for (j, &c) in chain.iter().enumerate() {
        let q = prefixes[j + 1];
        module.cells[c].kind = CellKind::Buf;
        module.cells[c].inputs = vec![q];
    }
}

/// Sklansky prefix network over `leaves`, returning one net per prefix
/// (`out[i]` = `leaves[0] OP … OP leaves[i]`; `out[0]` is `leaves[0]`
/// itself). Recursion depth is log₂ of the chain length.
fn sklansky(module: &mut GateModule, kind: CellKind, leaves: &[NetId]) -> Vec<NetId> {
    if leaves.len() == 1 {
        return vec![leaves[0]];
    }
    let half = leaves.len() / 2;
    let left = sklansky(module, kind, &leaves[..half]);
    let right = sklansky(module, kind, &leaves[half..]);
    let carry = *left.last().expect("left half is non-empty");
    let mut out = left;
    for r in right {
        let net = module.nets.len() as NetId;
        module.nets.push(NetInfo {
            driver: NetDriver::Undriven,
            origin: None,
        });
        let idx = module.cells.len();
        module.cells.push(Cell {
            kind,
            inputs: vec![carry, r],
            output: net,
        });
        module.nets[net as usize].driver = NetDriver::Cell(idx);
        out.push(net);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{GatePort, PortDir};
    use std::collections::HashMap;
    use veryl_parser::resource_table;

    /// A linear scan `g[j] = OP(g[j-1], in[j+1])` whose every stage output is
    /// observed by an output port — the shape balance cannot touch.
    fn scan_chain(kind: CellKind, n_leaves: usize) -> (GateModule, Vec<NetId>, Vec<NetId>) {
        let mut m = GateModule::default();
        let inputs: Vec<NetId> = (0..n_leaves)
            .map(|_| {
                let id = m.nets.len() as NetId;
                m.nets.push(NetInfo {
                    driver: NetDriver::PortInput,
                    origin: None,
                });
                id
            })
            .collect();
        let mut stages = Vec::new();
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
            stages.push(out);
            acc = out;
        }
        m.ports.push(GatePort {
            name: resource_table::insert_str("stages"),
            path: vec![],
            dir: PortDir::Output,
            nets: stages.clone(),
        });
        (m, inputs, stages)
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
                    CellKind::And2 => a && eval(m, c.inputs[1], inval),
                    CellKind::Or2 => a || eval(m, c.inputs[1], inval),
                    CellKind::Xor2 => a ^ eval(m, c.inputs[1], inval),
                    other => panic!("unexpected cell {other:?} in test eval"),
                }
            }
            other => panic!("unexpected driver {other:?} in test eval"),
        }
    }

    fn depth(m: &GateModule, net: NetId) -> usize {
        match &m.nets[net as usize].driver {
            NetDriver::Cell(ci) => {
                let c = &m.cells[*ci];
                let d = c.inputs.iter().map(|&i| depth(m, i)).max().unwrap_or(0);
                d + usize::from(c.kind != CellKind::Buf)
            }
            _ => 0,
        }
    }

    #[test]
    fn prefix_rebuild_preserves_every_stage_and_is_log_depth() {
        let n = 12;
        for kind in [CellKind::And2, CellKind::Or2, CellKind::Xor2] {
            let (mut m, inputs, stages) = scan_chain(kind, n);
            assert_eq!(depth(&m, *stages.last().unwrap()), n - 1);

            assert!(prefix_parallelize(&mut m), "wide scan should rebuild");
            let deepest = depth(&m, *stages.last().unwrap());
            assert!(
                deepest <= 4,
                "{kind:?}: deepest prefix {deepest}, expected ⌈log2({n})⌉"
            );

            for mask in 0..(1u32 << n) {
                let inval: HashMap<NetId, bool> = inputs
                    .iter()
                    .enumerate()
                    .map(|(i, &net)| (net, (mask >> i) & 1 == 1))
                    .collect();
                let mut expect = inval[&inputs[0]];
                for (j, &leaf) in inputs[1..].iter().enumerate() {
                    let b = inval[&leaf];
                    expect = match kind {
                        CellKind::And2 => expect && b,
                        CellKind::Or2 => expect || b,
                        CellKind::Xor2 => expect ^ b,
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        eval(&m, stages[j], &inval),
                        expect,
                        "{kind:?} stage {j} mismatch at mask {mask:#x}"
                    );
                }
            }
        }
    }

    #[test]
    fn short_scan_is_left_linear() {
        // `scan_chain(n)` builds n-1 cells: one below the threshold…
        let (mut m, _in, _st) = scan_chain(CellKind::Or2, MIN_CHAIN_CELLS);
        assert!(!prefix_parallelize(&mut m), "short scan must be skipped");
        // …and exactly at it.
        let (mut m, _in, _st) = scan_chain(CellKind::Or2, MIN_CHAIN_CELLS + 1);
        assert!(prefix_parallelize(&mut m), "threshold scan must rebuild");
    }
}
