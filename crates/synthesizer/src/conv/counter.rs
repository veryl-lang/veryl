//! Serial conditional-increment counters rebuilt as popcount trees.
//!
//! An unrolled `if c { cnt = cnt + 1; }` scan (count-leading/trailing-zeros
//! and similar) lowers to one increment-and-select stage per iteration:
//!
//! ```text
//! B0' = Mux2(c, B0, Not(B0))
//! Bk' = Mux2(c, Bk, Xor2(Bk, AND(B0 … B(k-1))))       k = 1 …
//! ```
//!
//! chained through the counter bus `B`, depth O(N·W); an `if / else if`
//! writeback wraps each stage in passthrough mux layers. The final value is
//! just `seed + popcount(c_0 … c_(m-1)) mod 2^W` — associativity of addition
//! that neither the chain balancer nor the prefix pass can see through mux
//! stages. The rebuild sums the stage conditions with a full-adder compressor
//! tree, adds the seed (the counter bus entering the first chained stage),
//! and aliases the last stage's bus to the result: depth O(log N). Chains
//! only link between equal-width stages — see `follows` for why a width step
//! must break the chain. Observed intermediate counts keep their serial cone
//! alive on their own; only the last stage is rewired.
//!
//! Runs on the restructuring clone under the same keep-the-better guard as
//! its sibling passes.

use std::collections::HashMap;

use crate::ir::{Cell, CellKind, GateModule, NET_CONST0, NetDriver, NetId, NetInfo};

/// Below this many stages the serial form is already shallow and the
/// compressor-tree overhead is not worth trying.
const MIN_STAGES: usize = 8;

/// One recognised increment stage: the counter bus it reads and produces,
/// and the increment condition as a product of (net, negated) terms — the
/// nested-mux writeback of an `if / else if` gives `!outer && inner`, which
/// is only materialised for chains that actually rebuild.
struct Stage {
    cond_terms: Vec<(NetId, bool)>,
    out_bus: Vec<NetId>,
    /// The bus this stage read — the previous stage's `out_bus`, or the seed.
    in_bus: Vec<NetId>,
    /// Cells to alias when this is the final stage (one per bit).
    cells: Vec<usize>,
}

/// Rebuild every long conditional-increment chain. Returns whether anything
/// was rebuilt.
pub(super) fn count_scan_rebuild(module: &mut GateModule) -> bool {
    let stages = collect_stages(module);
    if stages.is_empty() {
        return false;
    }

    // Link stages into chains only when the bus matches exactly. A width
    // step is a chain break: the narrow segment wraps mod its own width,
    // which a wider rebuild would carry past — whether the wrap can really
    // happen is a value-range property the structure cannot prove, so
    // linking across widths would miscompile a genuinely wrapping narrow
    // sub-counter. Constant-folded narrow head stages therefore stay
    // serial; the full-width tail is by far the longest segment anyway.
    let follows = |a: &Stage, b: &Stage| a.out_bus == b.in_bus;
    let mut succ: Vec<Option<usize>> = vec![None; stages.len()];
    let mut has_pred = vec![false; stages.len()];
    for (i, a) in stages.iter().enumerate() {
        let mut cand = None;
        for (j, b) in stages.iter().enumerate() {
            if i != j && follows(a, b) {
                if cand.is_some() {
                    cand = None; // ambiguous — leave unlinked
                    break;
                }
                cand = Some(j);
            }
        }
        if let Some(j) = cand {
            succ[i] = Some(j);
            has_pred[j] = true;
        }
    }

    let mut changed = false;
    for (head, &linked) in has_pred.iter().enumerate() {
        if linked {
            continue;
        }
        let mut chain = vec![head];
        while let Some(next) = succ[*chain.last().expect("chain is non-empty")] {
            chain.push(next);
        }
        if chain.len() < MIN_STAGES {
            continue;
        }
        rebuild_chain(module, &stages, &chain);
        changed = true;
    }
    changed
}

/// `final = (seed + popcount(conds)) mod 2^W`, aliased onto the last stage.
fn rebuild_chain(module: &mut GateModule, stages: &[Stage], chain: &[usize]) {
    let conds: Vec<NetId> = chain
        .iter()
        .map(|&i| materialize_cond(module, &stages[i].cond_terms))
        .collect();
    let seed = stages[chain[0]].in_bus.clone();
    let width = seed.len();

    let count = popcount(module, &conds);
    let total = add_mod(module, &seed, &count, width);

    let last = &stages[*chain.last().expect("chain is non-empty")];
    for (k, &cell) in last.cells.iter().enumerate() {
        module.cells[cell].kind = CellKind::Buf;
        module.cells[cell].inputs = vec![total[k]];
    }
}

/// AND together the stage's condition terms, inverting where needed.
fn materialize_cond(module: &mut GateModule, terms: &[(NetId, bool)]) -> NetId {
    let mut acc: Option<NetId> = None;
    for &(net, neg) in terms {
        let lit = if neg {
            mk_cell(module, CellKind::Not, vec![net])
        } else {
            net
        };
        acc = Some(match acc {
            None => lit,
            Some(a) => mk_cell(module, CellKind::And2, vec![a, lit]),
        });
    }
    acc.expect("at least one condition term")
}

/// A mux whose one leg is the increment of the other: `S = Not(B)` for bit 0,
/// `S = Xor2(B, AND(lower bits))` above it. `sel_high` records which leg the
/// select must pick for the increment to happen.
struct IncMux {
    mux: usize,
    k: usize,
    b: NetId,
    leaves: Vec<NetId>,
    sel_high: bool,
}

fn classify_inc_mux(module: &GateModule, m: usize) -> Option<IncMux> {
    let driver_cell = |net: NetId| -> Option<&Cell> {
        match module.nets[net as usize].driver {
            NetDriver::Cell(i) => Some(&module.cells[i]),
            _ => None,
        }
    };
    let mux = &module.cells[m];
    for (b, sum, sel_high) in [
        (mux.inputs[1], mux.inputs[2], true),
        (mux.inputs[2], mux.inputs[1], false),
    ] {
        let Some(t) = driver_cell(sum) else { continue };
        match t.kind {
            CellKind::Not if t.inputs[0] == b => {
                return Some(IncMux {
                    mux: m,
                    k: 0,
                    b,
                    leaves: Vec::new(),
                    sel_high,
                });
            }
            CellKind::Xor2 => {
                let carry = if t.inputs[0] == b {
                    t.inputs[1]
                } else if t.inputs[1] == b {
                    t.inputs[0]
                } else {
                    continue;
                };
                let mut leaves = Vec::new();
                collect_and_leaves(module, carry, &mut leaves);
                if !leaves.is_empty() {
                    return Some(IncMux {
                        mux: m,
                        k: leaves.len(),
                        b,
                        leaves,
                        sel_high,
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// Find every cell group matching the stage signature, then fold enclosing
/// passthrough mux layers (`if / else if` writeback) into the condition.
fn collect_stages(module: &GateModule) -> Vec<Stage> {
    // (sel, sel_high) -> increment muxes, i.e. one candidate stage.
    let mut groups: HashMap<(NetId, bool), Vec<IncMux>> = HashMap::new();
    for m in 0..module.cells.len() {
        if module.cells[m].kind != CellKind::Mux2 {
            continue;
        }
        if let Some(im) = classify_inc_mux(module, m) {
            groups
                .entry((module.cells[im.mux].inputs[0], im.sel_high))
                .or_default()
                .push(im);
        }
    }

    // Deterministic order: hash-map iteration would make net numbering (and
    // dump diffs) vary run to run when several chains rebuild.
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_unstable_by_key(|(key, _)| *key);
    let mut stages = Vec::new();
    'group: for ((sel, sel_high), mut incs) in groups {
        incs.sort_unstable_by_key(|im| im.k);
        // Bus = (B0, B1, …): carry widths must be exactly 0, 1, 2, … and each
        // carry's leaves must be the lower bus bits.
        let mut in_bus = Vec::new();
        let mut out_bus = Vec::new();
        let mut cells = Vec::new();
        for (pos, im) in incs.iter().enumerate() {
            if im.k != pos {
                continue 'group;
            }
            let mut leaves = im.leaves.clone();
            leaves.sort_unstable();
            let mut expect = in_bus.clone();
            expect.sort_unstable();
            if leaves != expect {
                continue 'group;
            }
            in_bus.push(im.b);
            out_bus.push(module.cells[im.mux].output);
            cells.push(im.mux);
        }
        if in_bus.len() < 2 {
            continue;
        }
        let mut stage = Stage {
            cond_terms: vec![(sel, !sel_high)],
            out_bus,
            in_bus,
            cells,
        };
        fold_passthrough_layers(module, &mut stage);
        stages.push(stage);
    }
    stages
}

/// While every stage output bit `I_k` feeds one mux layer `Mux(ca, I_k ↔ B_k)`
/// with a shared select and orientation, fold that layer in: the stage's
/// outputs become the outer muxes and the increment additionally requires the
/// outer select to pass the incremented leg.
fn fold_passthrough_layers(module: &GateModule, stage: &mut Stage) {
    'outer: loop {
        let mut layer: Option<(NetId, bool)> = None; // (sel, inc leg is d1)
        let mut outer_cells = Vec::with_capacity(stage.out_bus.len());
        for (&ik, &bk) in stage.out_bus.iter().zip(&stage.in_bus) {
            let found = module.cells.iter().enumerate().find(|(_, c)| {
                c.kind == CellKind::Mux2
                    && ((c.inputs[1] == bk && c.inputs[2] == ik)
                        || (c.inputs[1] == ik && c.inputs[2] == bk))
            });
            let Some((mi, mc)) = found else {
                break 'outer;
            };
            let inc_is_d1 = mc.inputs[2] == ik;
            match layer {
                None => layer = Some((mc.inputs[0], inc_is_d1)),
                Some(l) if l == (mc.inputs[0], inc_is_d1) => {}
                Some(_) => break 'outer,
            }
            outer_cells.push(mi);
        }
        let Some((sel, inc_is_d1)) = layer else {
            break;
        };
        stage.cond_terms.push((sel, !inc_is_d1));
        stage.out_bus = outer_cells
            .iter()
            .map(|&m| module.cells[m].output)
            .collect();
        stage.cells = outer_cells;
    }
}

/// Flatten an And2/And3 cone into its leaves (single-level check is not
/// enough: the carry over ≥3 bits is an AND tree).
fn collect_and_leaves(module: &GateModule, net: NetId, out: &mut Vec<NetId>) {
    if let NetDriver::Cell(i) = module.nets[net as usize].driver {
        let c = &module.cells[i];
        if matches!(c.kind, CellKind::And2 | CellKind::And3) {
            for &inp in &c.inputs {
                collect_and_leaves(module, inp, out);
            }
            return;
        }
    }
    out.push(net);
}

// ---------------------------------------------------------------------------
// Rebuild
// ---------------------------------------------------------------------------

fn mk_cell(module: &mut GateModule, kind: CellKind, inputs: Vec<NetId>) -> NetId {
    let net = module.nets.len() as NetId;
    module.nets.push(NetInfo {
        driver: NetDriver::Undriven,
        origin: None,
    });
    let idx = module.cells.len();
    module.cells.push(Cell {
        kind,
        inputs,
        output: net,
    });
    module.nets[net as usize].driver = NetDriver::Cell(idx);
    net
}

/// Binary popcount of `bits` (LSB first) via full/half-adder compression:
/// three equal-weight bits become sum + carry, so every weight level shrinks
/// geometrically and the tree depth is O(log N).
fn popcount(module: &mut GateModule, bits: &[NetId]) -> Vec<NetId> {
    let mut levels: Vec<Vec<NetId>> = vec![bits.to_vec()];
    let mut w = 0;
    while w < levels.len() {
        while levels[w].len() >= 2 {
            if levels[w].len() >= 3 {
                let (a, b, c) = (
                    levels[w].remove(0),
                    levels[w].remove(0),
                    levels[w].remove(0),
                );
                let ab = mk_cell(module, CellKind::Xor2, vec![a, b]);
                let sum = mk_cell(module, CellKind::Xor2, vec![ab, c]);
                let ab_and = mk_cell(module, CellKind::And2, vec![a, b]);
                let c_ab = mk_cell(module, CellKind::And2, vec![c, ab]);
                let carry = mk_cell(module, CellKind::Or2, vec![ab_and, c_ab]);
                levels[w].push(sum);
                if levels.len() == w + 1 {
                    levels.push(Vec::new());
                }
                levels[w + 1].push(carry);
            } else {
                let (a, b) = (levels[w].remove(0), levels[w].remove(0));
                let sum = mk_cell(module, CellKind::Xor2, vec![a, b]);
                let carry = mk_cell(module, CellKind::And2, vec![a, b]);
                levels[w].push(sum);
                if levels.len() == w + 1 {
                    levels.push(Vec::new());
                }
                levels[w + 1].push(carry);
            }
        }
        w += 1;
    }
    levels
        .into_iter()
        .map(|l| l.into_iter().next().unwrap_or(NET_CONST0))
        .collect()
}

/// `(a + b) mod 2^width` with a ripple carry — the operands are only
/// counter-width (a handful of bits), so the ripple depth is negligible.
fn add_mod(module: &mut GateModule, a: &[NetId], b: &[NetId], width: usize) -> Vec<NetId> {
    let bit = |bus: &[NetId], i: usize| bus.get(i).copied().unwrap_or(NET_CONST0);
    let mut out = Vec::with_capacity(width);
    let mut carry = NET_CONST0;
    for i in 0..width {
        let (x, y) = (bit(a, i), bit(b, i));
        let xy = mk_cell(module, CellKind::Xor2, vec![x, y]);
        out.push(mk_cell(module, CellKind::Xor2, vec![xy, carry]));
        if i + 1 < width {
            let and_xy = mk_cell(module, CellKind::And2, vec![x, y]);
            let and_c = mk_cell(module, CellKind::And2, vec![carry, xy]);
            carry = mk_cell(module, CellKind::Or2, vec![and_xy, and_c]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{GatePort, PortDir};
    use std::collections::HashMap;
    use veryl_parser::resource_table;

    /// Build the canonical serial counter over `n` condition inputs with a
    /// `width`-bit bus seeded from constant zero.
    fn serial_counter(n: usize, width: usize) -> (GateModule, Vec<NetId>, Vec<NetId>) {
        let mut m = GateModule::default();
        // Reserve const nets 0/1 like the real converter does.
        m.nets.push(NetInfo {
            driver: NetDriver::Const(false),
            origin: None,
        });
        m.nets.push(NetInfo {
            driver: NetDriver::Const(true),
            origin: None,
        });
        let conds: Vec<NetId> = (0..n)
            .map(|_| {
                let id = m.nets.len() as NetId;
                m.nets.push(NetInfo {
                    driver: NetDriver::PortInput,
                    origin: None,
                });
                id
            })
            .collect();
        // Seed the bus with distinct port inputs so the head stage is
        // structurally regular (a constant seed would fold stage 1 away).
        let mut bus: Vec<NetId> = (0..width)
            .map(|_| {
                let id = m.nets.len() as NetId;
                m.nets.push(NetInfo {
                    driver: NetDriver::PortInput,
                    origin: None,
                });
                id
            })
            .collect();
        let seed = bus.clone();
        for &c in &conds {
            let mut next = Vec::with_capacity(width);
            let n0 = mk_cell(&mut m, CellKind::Not, vec![bus[0]]);
            next.push(mk_cell(&mut m, CellKind::Mux2, vec![c, bus[0], n0]));
            for k in 1..width {
                let mut carry = bus[0];
                for &lower in &bus[1..k] {
                    carry = mk_cell(&mut m, CellKind::And2, vec![carry, lower]);
                }
                let t = mk_cell(&mut m, CellKind::Xor2, vec![bus[k], carry]);
                next.push(mk_cell(&mut m, CellKind::Mux2, vec![c, bus[k], t]));
            }
            bus = next;
        }
        m.ports.push(GatePort {
            name: resource_table::insert_str("cnt"),
            path: vec![],
            dir: PortDir::Output,
            nets: bus.clone(),
        });
        (m, conds, seed)
    }

    fn eval(
        m: &GateModule,
        net: NetId,
        inval: &HashMap<NetId, bool>,
        memo: &mut HashMap<NetId, bool>,
    ) -> bool {
        if let Some(&v) = memo.get(&net) {
            return v;
        }
        let v = match &m.nets[net as usize].driver {
            NetDriver::PortInput => inval[&net],
            NetDriver::Const(b) => *b,
            NetDriver::Cell(ci) => {
                let c = &m.cells[*ci];
                let v: Vec<bool> = c.inputs.iter().map(|&i| eval(m, i, inval, memo)).collect();
                match c.kind {
                    CellKind::Buf => v[0],
                    CellKind::Not => !v[0],
                    CellKind::And2 => v[0] && v[1],
                    CellKind::Or2 => v[0] || v[1],
                    CellKind::Xor2 => v[0] ^ v[1],
                    CellKind::Mux2 => {
                        if v[0] {
                            v[2]
                        } else {
                            v[1]
                        }
                    }
                    other => panic!("unexpected cell {other:?} in test eval"),
                }
            }
            other => panic!("unexpected driver {other:?} in test eval"),
        };
        memo.insert(net, v);
        v
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

    /// The rebuilt counter must equal `seed + popcount(conds) mod 2^W` for
    /// every input, at compressor-tree depth.
    #[test]
    fn count_scan_matches_serial_semantics_exhaustively() {
        let (n, width) = (9, 4);
        let (mut m, conds, seed) = serial_counter(n, width);
        let out: Vec<NetId> = m.ports[0].nets.clone();
        let serial_depth = depth(&m, *out.last().expect("bus non-empty"));

        assert!(count_scan_rebuild(&mut m), "counter chain should rebuild");
        let rebuilt_depth = depth(&m, *out.last().expect("bus non-empty"));
        assert!(
            rebuilt_depth < serial_depth && rebuilt_depth <= 12,
            "depth {serial_depth} -> {rebuilt_depth}, expected compressor-tree"
        );

        for mask in 0..(1u32 << (n + width)) {
            let mut inval = HashMap::new();
            for (i, &c) in conds.iter().enumerate() {
                inval.insert(c, (mask >> i) & 1 == 1);
            }
            for (i, &s) in seed.iter().enumerate() {
                inval.insert(s, (mask >> (n + i)) & 1 == 1);
            }
            let ones = (mask & ((1 << n) - 1)).count_ones();
            let seed_val = mask >> n;
            let expect = (seed_val + ones) % (1 << width);
            let mut memo = HashMap::new();
            for (k, &net) in out.iter().enumerate() {
                assert_eq!(
                    eval(&m, net, &inval, &mut memo),
                    (expect >> k) & 1 == 1,
                    "bit {k} mismatch at mask {mask:#x}"
                );
            }
        }
    }

    /// An `if / else if` writeback wraps each stage in a passthrough mux
    /// layer; folding it must recover `!outer && sel` as the condition.
    #[test]
    fn nested_passthrough_layer_matches_serial_semantics() {
        let (n, width) = (8, 2);
        let mut m = GateModule::default();
        m.nets.push(NetInfo {
            driver: NetDriver::Const(false),
            origin: None,
        });
        m.nets.push(NetInfo {
            driver: NetDriver::Const(true),
            origin: None,
        });
        let pi = |m: &mut GateModule| {
            let id = m.nets.len() as NetId;
            m.nets.push(NetInfo {
                driver: NetDriver::PortInput,
                origin: None,
            });
            id
        };
        let conds: Vec<NetId> = (0..n).map(|_| pi(&mut m)).collect();
        let outers: Vec<NetId> = (0..n).map(|_| pi(&mut m)).collect();
        let seed: Vec<NetId> = (0..width).map(|_| pi(&mut m)).collect();
        let mut bus = seed.clone();
        for (&c, &o) in conds.iter().zip(&outers) {
            let mut inner = Vec::with_capacity(width);
            let n0 = mk_cell(&mut m, CellKind::Not, vec![bus[0]]);
            inner.push(mk_cell(&mut m, CellKind::Mux2, vec![c, bus[0], n0]));
            for k in 1..width {
                let mut carry = bus[0];
                for &lower in &bus[1..k] {
                    carry = mk_cell(&mut m, CellKind::And2, vec![carry, lower]);
                }
                let t = mk_cell(&mut m, CellKind::Xor2, vec![bus[k], carry]);
                inner.push(mk_cell(&mut m, CellKind::Mux2, vec![c, bus[k], t]));
            }
            // Outer layer: increment survives only when `o` is low.
            bus = (0..width)
                .map(|k| mk_cell(&mut m, CellKind::Mux2, vec![o, inner[k], bus[k]]))
                .collect();
        }
        m.ports.push(GatePort {
            name: resource_table::insert_str("cnt"),
            path: vec![],
            dir: PortDir::Output,
            nets: bus.clone(),
        });

        assert!(count_scan_rebuild(&mut m), "nested chain should rebuild");
        for mask in 0..(1u32 << (2 * n + width)) {
            let mut inval = HashMap::new();
            let mut count = 0u32;
            for i in 0..n {
                let c = (mask >> i) & 1 == 1;
                let o = (mask >> (n + i)) & 1 == 1;
                inval.insert(conds[i], c);
                inval.insert(outers[i], o);
                if c && !o {
                    count += 1;
                }
            }
            let mut seed_val = 0u32;
            for (i, &sn) in seed.iter().enumerate() {
                let v = (mask >> (2 * n + i)) & 1 == 1;
                inval.insert(sn, v);
                seed_val |= (v as u32) << i;
            }
            let expect = (seed_val + count) % (1 << width);
            let mut memo = HashMap::new();
            for (k, &net) in bus.iter().enumerate() {
                assert_eq!(
                    eval(&m, net, &inval, &mut memo),
                    (expect >> k) & 1 == 1,
                    "bit {k} mismatch at mask {mask:#x}"
                );
            }
        }
    }

    /// A genuinely wrapping narrow stage feeding a wider chain: the width
    /// step must break the chain (the serial form wraps mod 4; carrying into
    /// the grown bits would miscompile — the adversarial-review wrap8 case).
    /// The wide tail still rebuilds, and the whole must stay bit-exact.
    #[test]
    fn width_step_breaks_the_chain_and_stays_exact() {
        let n = 8; // wide stages
        let mut m = GateModule::default();
        m.nets.push(NetInfo {
            driver: NetDriver::Const(false),
            origin: None,
        });
        m.nets.push(NetInfo {
            driver: NetDriver::Const(true),
            origin: None,
        });
        let pi = |m: &mut GateModule| {
            let id = m.nets.len() as NetId;
            m.nets.push(NetInfo {
                driver: NetDriver::PortInput,
                origin: None,
            });
            id
        };
        let c0 = pi(&mut m);
        let conds: Vec<NetId> = (0..n).map(|_| pi(&mut m)).collect();
        let t: Vec<NetId> = (0..2).map(|_| pi(&mut m)).collect(); // 2-bit seed
        let h = pi(&mut m); // becomes bit 2 after the width step

        // Narrow wrapping stage: t' = t + c0 mod 4.
        let inc_stage = |m: &mut GateModule, c: NetId, bus: &[NetId]| -> Vec<NetId> {
            let mut next = Vec::with_capacity(bus.len());
            let n0 = mk_cell(m, CellKind::Not, vec![bus[0]]);
            next.push(mk_cell(m, CellKind::Mux2, vec![c, bus[0], n0]));
            for k in 1..bus.len() {
                let mut carry = bus[0];
                for &lower in &bus[1..k] {
                    carry = mk_cell(m, CellKind::And2, vec![carry, lower]);
                }
                let x = mk_cell(m, CellKind::Xor2, vec![bus[k], carry]);
                next.push(mk_cell(m, CellKind::Mux2, vec![c, bus[k], x]));
            }
            next
        };
        let tp = inc_stage(&mut m, c0, &t);
        // Width step: bus = {h, t'} — then 8 wide stages of +1 mod 8.
        let mut bus = vec![tp[0], tp[1], h];
        for &c in &conds {
            bus = inc_stage(&mut m, c, &bus);
        }
        m.ports.push(GatePort {
            name: resource_table::insert_str("cnt"),
            path: vec![],
            dir: PortDir::Output,
            nets: bus.clone(),
        });

        assert!(count_scan_rebuild(&mut m), "wide tail should rebuild");
        for mask in 0..(1u32 << (1 + n + 3)) {
            let mut inval = HashMap::new();
            inval.insert(c0, mask & 1 == 1);
            let mut ones = 0u32;
            for (i, &c) in conds.iter().enumerate() {
                let v = (mask >> (1 + i)) & 1 == 1;
                inval.insert(c, v);
                ones += v as u32;
            }
            let t_val = (mask >> (1 + n)) & 3;
            let h_val = (mask >> (1 + n + 2)) & 1;
            inval.insert(t[0], t_val & 1 == 1);
            inval.insert(t[1], t_val & 2 == 2);
            inval.insert(h, h_val == 1);
            let wrapped = (t_val + (mask & 1)) % 4; // narrow stage wraps mod 4
            let expect = ((h_val << 2 | wrapped) + ones) % 8;
            let mut memo = HashMap::new();
            for (k, &net) in bus.iter().enumerate() {
                assert_eq!(
                    eval(&m, net, &inval, &mut memo),
                    (expect >> k) & 1 == 1,
                    "bit {k} mismatch at mask {mask:#x}"
                );
            }
        }
    }

    /// Short chains stay serial.
    #[test]
    fn short_count_scan_is_left_serial() {
        let (mut m, _c, _s) = serial_counter(MIN_STAGES - 1, 4);
        assert!(!count_scan_rebuild(&mut m), "short chain must be skipped");
    }
}
