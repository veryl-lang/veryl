use std::mem;

use crate::ir::{Cell, CellKind, GateModule, NET_CONST0, NET_CONST1, NetDriver, NetId};

const CSE_PAD: NetId = NetId::MAX;

/// Walk Buf-driven aliases on `net` until a non-Buf driver (or a non-Cell
/// endpoint like a port input or const) is reached. Used to rewire FF / port
/// nets past Bufs that [`worklist_simplify`] leaves behind before it compacts
/// the cell list.
pub(super) fn resolve_alias(module: &GateModule, mut net: NetId) -> NetId {
    loop {
        if let NetDriver::Cell(idx) = module.nets[net as usize].driver {
            let cell = &module.cells[idx];
            if cell.kind == CellKind::Buf {
                net = cell.inputs[0];
                continue;
            }
        }
        return net;
    }
}

/// Returns the canonical CSE key for a cell, or `None` for Buf cells (which
/// we don't dedup). Commutative gates get sorted input pairs so that `a & b`
/// and `b & a` hash the same; Mux keeps its (sel, d0, d1) order.
fn cse_key(kind: CellKind, inputs: &[NetId]) -> Option<(CellKind, [NetId; 4])> {
    match kind {
        CellKind::Buf => None,
        CellKind::Not => Some((kind, [inputs[0], CSE_PAD, CSE_PAD, CSE_PAD])),
        CellKind::And2
        | CellKind::Or2
        | CellKind::Nand2
        | CellKind::Nor2
        | CellKind::Xor2
        | CellKind::Xnor2 => {
            let (a, b) = (inputs[0], inputs[1]);
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            Some((kind, [lo, hi, CSE_PAD, CSE_PAD]))
        }
        CellKind::And3 | CellKind::Or3 | CellKind::Nand3 | CellKind::Nor3 => {
            let mut s = [inputs[0], inputs[1], inputs[2]];
            s.sort();
            Some((kind, [s[0], s[1], s[2], CSE_PAD]))
        }
        // The first two inputs are the AND/OR leg (commutative); the third
        // is the odd leg and keeps its position. Shared by inverted (Aoi21
        // / Oai21) and non-inverted (Ao21 / Oa21) variants.
        CellKind::Ao21 | CellKind::Aoi21 | CellKind::Oa21 | CellKind::Oai21 => {
            let (a, b) = (inputs[0], inputs[1]);
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            Some((kind, [lo, hi, inputs[2], CSE_PAD]))
        }
        // Two commutative AND pairs, and the pairs themselves commute:
        //   (A&B) | (C&D) == (C&D) | (A&B) == (B&A) | (D&C) ...
        // Canonicalize each pair internally, then order the pairs. Shared
        // by Ao22 / Aoi22 (same fan-in shape, different polarity).
        CellKind::Ao22 | CellKind::Aoi22 => {
            let (a, b) = (inputs[0], inputs[1]);
            let (c, d) = (inputs[2], inputs[3]);
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            let (c, d) = if c <= d { (c, d) } else { (d, c) };
            let ((a1, b1), (c1, d1)) = if (a, b) <= (c, d) {
                ((a, b), (c, d))
            } else {
                ((c, d), (a, b))
            };
            Some((kind, [a1, b1, c1, d1]))
        }
        // Oai22 uses two OR pairs — same commutativity properties.
        CellKind::Oai22 => {
            let (a, b) = (inputs[0], inputs[1]);
            let (c, d) = (inputs[2], inputs[3]);
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            let (c, d) = if c <= d { (c, d) } else { (d, c) };
            let ((a1, b1), (c1, d1)) = if (a, b) <= (c, d) {
                ((a, b), (c, d))
            } else {
                ((c, d), (a, b))
            };
            Some((kind, [a1, b1, c1, d1]))
        }
        // Ao31 / Aoi31 = ((A & B & C) | D) [inverted for Aoi31]. The AND
        // leg's three inputs are freely commutative; the odd leg keeps its
        // slot. Sort the AND triple for a canonical key.
        CellKind::Ao31 | CellKind::Aoi31 => {
            let mut s = [inputs[0], inputs[1], inputs[2]];
            s.sort();
            Some((kind, [s[0], s[1], s[2], inputs[3]]))
        }
        CellKind::Mux2 => Some((kind, [inputs[0], inputs[1], inputs[2], CSE_PAD])),
    }
}

enum Simpl {
    Const(bool),
    Alias(NetId),
    Invert(NetId),
}

/// Worklist-time algebraic rewrite. Kept intentionally narrow:
/// * `Not(X)` cases that `simpl` can't express (Not(Not), polarity flips)
/// * Mux2 const-data collapse that needs no auxiliary cells — these must
///   run in the worklist so the produced And2/Or2 see CSE. Deferring them
///   to the post-pass leaves duplicate boolean cells and measurably
///   worsens decoder area.
///
/// Skipped here (handled only in the post-pass) to keep this function
/// fast on Mux2-heavy designs:
/// * Mux2 variants that need materialising a `!sel` cell
/// * Mux2(s, s, x) / Mux2(s, x, s) sel-equals-data collapse
/// * Aoi21/Oai21 and compound fusions
fn algebraic_fuse(
    module: &GateModule,
    cell_kind: CellKind,
    cell_inputs: &[NetId],
    consumers: &[Vec<u32>],
) -> Option<(CellKind, Vec<NetId>)> {
    match cell_kind {
        CellKind::Not => {
            let inp = cell_inputs[0];
            let NetDriver::Cell(up_idx) = module.nets[inp as usize].driver else {
                return None;
            };
            let up = &module.cells[up_idx];
            if up.kind == CellKind::Not {
                return Some((CellKind::Buf, vec![up.inputs[0]]));
            }
            if consumers[up.output as usize].len() != 1 {
                return None;
            }
            let fused = match up.kind {
                CellKind::And2 => Some(CellKind::Nand2),
                CellKind::Or2 => Some(CellKind::Nor2),
                CellKind::Xor2 => Some(CellKind::Xnor2),
                CellKind::Nand2 => Some(CellKind::And2),
                CellKind::Nor2 => Some(CellKind::Or2),
                CellKind::Xnor2 => Some(CellKind::Xor2),
                CellKind::And3 => Some(CellKind::Nand3),
                CellKind::Or3 => Some(CellKind::Nor3),
                CellKind::Nand3 => Some(CellKind::And3),
                CellKind::Nor3 => Some(CellKind::Or3),
                _ => None,
            };
            fused.map(|k| (k, up.inputs.clone()))
        }
        CellKind::Mux2 => {
            let sel = cell_inputs[0];
            let d0 = cell_inputs[1];
            let d1 = cell_inputs[2];
            if let NetDriver::Const(false) = module.nets[d0 as usize].driver {
                return Some((CellKind::And2, vec![sel, d1]));
            }
            if let NetDriver::Const(true) = module.nets[d1 as usize].driver {
                return Some((CellKind::Or2, vec![sel, d0]));
            }
            None
        }
        _ => None,
    }
}

/// Dispatch table: map each [`CellKind`] to its per-kind simplifier. New
/// primitives just plug a new arm in here and write their own `simpl_*`
/// function — the shared helpers below cover the common absorbing /
/// parity / AOI patterns.
fn simplify(kind: CellKind, iv: &[Option<bool>], nets: &[NetId]) -> Option<Simpl> {
    match kind {
        CellKind::Buf => iv[0].map(Simpl::Const),
        CellKind::Not => iv[0].map(|b| Simpl::Const(!b)),
        CellKind::And2 => simpl_absorb_2(iv, nets, AbsorbParams::AND),
        CellKind::Or2 => simpl_absorb_2(iv, nets, AbsorbParams::OR),
        CellKind::Nand2 => simpl_absorb_2(iv, nets, AbsorbParams::NAND),
        CellKind::Nor2 => simpl_absorb_2(iv, nets, AbsorbParams::NOR),
        CellKind::Xor2 => simpl_xor_family(iv, nets, false),
        CellKind::Xnor2 => simpl_xor_family(iv, nets, true),
        CellKind::Mux2 => simpl_mux2(iv, nets),
        CellKind::And3 => simpl_absorb_3(iv, nets, AbsorbParams::AND),
        CellKind::Or3 => simpl_absorb_3(iv, nets, AbsorbParams::OR),
        CellKind::Nand3 => simpl_absorb_3(iv, nets, AbsorbParams::NAND),
        CellKind::Nor3 => simpl_absorb_3(iv, nets, AbsorbParams::NOR),
        CellKind::Ao21 => simpl_ao_family(iv, nets, true),
        CellKind::Aoi21 => simpl_aoi_family(iv, nets, true),
        CellKind::Oa21 => simpl_ao_family(iv, nets, false),
        CellKind::Oai21 => simpl_aoi_family(iv, nets, false),
        CellKind::Ao31 => simpl_ao31(iv, nets, false),
        CellKind::Aoi31 => simpl_ao31(iv, nets, true),
        CellKind::Ao22 => simpl_ao22(iv, nets),
        // Aoi22 / Oai22 share Ao22's structural shape — reuse its folder
        // and invert (Aoi22) or De-Morgan-dualise (Oai22) the result.
        CellKind::Aoi22 => simpl_ao22(iv, nets).map(invert_simpl),
        CellKind::Oai22 => simpl_oai22(iv, nets),
    }
}

/// Flip a Simpl result so `Simpl::Const(v)` → `Const(!v)`, `Alias(n)` →
/// `Invert(n)`, `Invert(n)` → `Alias(n)`. Used by Aoi22 which is just !Ao22.
fn invert_simpl(s: Simpl) -> Simpl {
    match s {
        Simpl::Const(v) => Simpl::Const(!v),
        Simpl::Alias(n) => Simpl::Invert(n),
        Simpl::Invert(n) => Simpl::Alias(n),
    }
}

/// Oai22 = !((A | B) & (C | D)). Dual of Ao22: either OR pair reaching 0
/// forces output to 1; both pairs at 1 forces output to 0. A single-net
/// alias is only possible when one pair is fully 0 (forced AND of two OR
/// terms = AND of a single literal) — extremely rare; we leave the tree
/// alone in that case.
fn simpl_oai22(iv: &[Option<bool>], nets: &[NetId]) -> Option<Simpl> {
    let ab_zero = iv[0] == Some(false) && iv[1] == Some(false);
    let cd_zero = iv[2] == Some(false) && iv[3] == Some(false);
    if ab_zero || cd_zero {
        return Some(Simpl::Const(true));
    }
    let ab_one = iv[0] == Some(true)
        || iv[1] == Some(true)
        || nets[0] == NET_CONST1
        || nets[1] == NET_CONST1;
    let cd_one = iv[2] == Some(true)
        || iv[3] == Some(true)
        || nets[2] == NET_CONST1
        || nets[3] == NET_CONST1;
    if ab_one && cd_one {
        return Some(Simpl::Const(false));
    }
    None
}

/// Ao22 = (A & B) | (C & D). Either AND pair reaching 1 forces output to 1;
/// both pairs forced to 0 forces output to 0. If one pair is known 0 (e.g.
/// A=0 or B=0), the output reduces to the other pair's AND.
fn simpl_ao22(iv: &[Option<bool>], nets: &[NetId]) -> Option<Simpl> {
    let ab_one = iv[0] == Some(true) && iv[1] == Some(true);
    let cd_one = iv[2] == Some(true) && iv[3] == Some(true);
    if ab_one || cd_one {
        return Some(Simpl::Const(true));
    }
    let ab_zero = iv[0] == Some(false)
        || iv[1] == Some(false)
        || nets[0] == NET_CONST0
        || nets[1] == NET_CONST0;
    let cd_zero = iv[2] == Some(false)
        || iv[3] == Some(false)
        || nets[2] == NET_CONST0
        || nets[3] == NET_CONST0;
    if ab_zero && cd_zero {
        return Some(Simpl::Const(false));
    }
    // Leaves AB: CD is zero so output = AB. But AB itself isn't a single net,
    // so we can only alias when one AND leg already evaluates to a net alias
    // of the other (e.g. A=1 ⇒ AB=B).
    if ab_zero {
        if iv[2] == Some(true) {
            return Some(Simpl::Alias(nets[3]));
        }
        if iv[3] == Some(true) {
            return Some(Simpl::Alias(nets[2]));
        }
        if nets[2] == nets[3] {
            return Some(Simpl::Alias(nets[2]));
        }
    }
    if cd_zero {
        if iv[0] == Some(true) {
            return Some(Simpl::Alias(nets[1]));
        }
        if iv[1] == Some(true) {
            return Some(Simpl::Alias(nets[0]));
        }
        if nets[0] == nets[1] {
            return Some(Simpl::Alias(nets[0]));
        }
    }
    None
}

/// Describes a 2-/3-input absorbing gate: when a `dominant` input is seen
/// the output is forced to `dominant_out`; otherwise a `!dominant` input
/// either aliases to the remaining input (AND/OR family) or inverts it
/// (NAND/NOR family).
#[derive(Clone, Copy)]
struct AbsorbParams {
    dominant: bool,
    dominant_out: bool,
    /// true for NAND/NOR (unit input produces `!other`), false for AND/OR.
    unit_invert: bool,
    /// AND/OR family also fold `f(a, a) = a`; NAND/NOR skip this (marginal win).
    self_alias: bool,
}

impl AbsorbParams {
    const AND: Self = Self {
        dominant: false,
        dominant_out: false,
        unit_invert: false,
        self_alias: true,
    };
    const OR: Self = Self {
        dominant: true,
        dominant_out: true,
        unit_invert: false,
        self_alias: true,
    };
    const NAND: Self = Self {
        dominant: false,
        dominant_out: true,
        unit_invert: true,
        self_alias: false,
    };
    const NOR: Self = Self {
        dominant: true,
        dominant_out: false,
        unit_invert: true,
        self_alias: false,
    };
}

fn simpl_absorb_2(iv: &[Option<bool>], nets: &[NetId], p: AbsorbParams) -> Option<Simpl> {
    if iv[0] == Some(p.dominant) || iv[1] == Some(p.dominant) {
        return Some(Simpl::Const(p.dominant_out));
    }
    let unit = !p.dominant;
    let make = |net: NetId| {
        if p.unit_invert {
            Simpl::Invert(net)
        } else {
            Simpl::Alias(net)
        }
    };
    if iv[0] == Some(unit) {
        return Some(make(nets[1]));
    }
    if iv[1] == Some(unit) {
        return Some(make(nets[0]));
    }
    if p.self_alias && nets[0] == nets[1] {
        return Some(Simpl::Alias(nets[0]));
    }
    None
}

fn simpl_absorb_3(iv: &[Option<bool>], nets: &[NetId], p: AbsorbParams) -> Option<Simpl> {
    if iv.contains(&Some(p.dominant)) {
        return Some(Simpl::Const(p.dominant_out));
    }
    if iv.iter().all(|v| *v == Some(!p.dominant)) {
        return Some(Simpl::Const(!p.dominant_out));
    }
    if p.self_alias && nets[0] == nets[1] && nets[1] == nets[2] {
        return Some(Simpl::Alias(nets[0]));
    }
    None
}

/// XOR2 / XNOR2: identity element is `base_parity` (XNOR=true, XOR=false),
/// so an input matching it aliases the other, the inverse inverts it, and
/// two equal inputs fold to `base_parity`.
fn simpl_xor_family(iv: &[Option<bool>], nets: &[NetId], base_parity: bool) -> Option<Simpl> {
    if let (Some(a), Some(b)) = (iv[0], iv[1]) {
        return Some(Simpl::Const(if base_parity { a == b } else { a != b }));
    }
    let id = base_parity;
    if iv[0] == Some(id) {
        return Some(Simpl::Alias(nets[1]));
    }
    if iv[1] == Some(id) {
        return Some(Simpl::Alias(nets[0]));
    }
    if iv[0] == Some(!id) {
        return Some(Simpl::Invert(nets[1]));
    }
    if iv[1] == Some(!id) {
        return Some(Simpl::Invert(nets[0]));
    }
    if nets[0] == nets[1] {
        return Some(Simpl::Const(base_parity));
    }
    None
}

fn simpl_mux2(iv: &[Option<bool>], nets: &[NetId]) -> Option<Simpl> {
    match iv[0] {
        Some(false) => Some(Simpl::Alias(nets[1])),
        Some(true) => Some(Simpl::Alias(nets[2])),
        None => {
            if nets[1] == nets[2] {
                Some(Simpl::Alias(nets[1]))
            } else if iv[1] == Some(false) && iv[2] == Some(true) {
                Some(Simpl::Alias(nets[0]))
            } else if iv[1] == Some(true) && iv[2] == Some(false) {
                Some(Simpl::Invert(nets[0]))
            } else {
                None
            }
        }
    }
}

/// Aoi21 (`is_aoi=true`) is `!((A & B) | C)`; Oai21 is `!((A | B) & C)`.
/// When C equals the value that short-circuits the outer op the output is
/// forced; when one of A/B neutralises its inner leg the output becomes
/// `!C`.
fn simpl_aoi_family(iv: &[Option<bool>], nets: &[NetId], is_aoi: bool) -> Option<Simpl> {
    let short_circuit_c = is_aoi;
    if iv[2] == Some(short_circuit_c) {
        return Some(Simpl::Const(!short_circuit_c));
    }
    let leg_neutral = !short_circuit_c;
    if iv[0] == Some(leg_neutral) || iv[1] == Some(leg_neutral) {
        return Some(match iv[2] {
            Some(b) => Simpl::Const(!b),
            None => Simpl::Invert(nets[2]),
        });
    }
    None
}

/// Ao31 (invert=false) = `(A & B & C) | D`; Aoi31 (invert=true) is its
/// complement. Handles the constant-input cases: the odd leg forcing the
/// outer OR, any AND-leg input falsifying the product, and all three
/// AND-leg inputs being 1.
fn simpl_ao31(iv: &[Option<bool>], nets: &[NetId], invert: bool) -> Option<Simpl> {
    // iv[3] = D forces the output when D = 1.
    if iv[3] == Some(true) {
        return Some(Simpl::Const(!invert));
    }
    // Any AND-leg input = 0 kills the product; output reduces to D.
    if iv[0] == Some(false) || iv[1] == Some(false) || iv[2] == Some(false) {
        return Some(match iv[3] {
            Some(b) => Simpl::Const(if invert { !b } else { b }),
            None => {
                if invert {
                    Simpl::Invert(nets[3])
                } else {
                    Simpl::Alias(nets[3])
                }
            }
        });
    }
    // All three AND inputs = 1: product = 1; output = 1 (or 0 inverted).
    if iv[0] == Some(true) && iv[1] == Some(true) && iv[2] == Some(true) {
        return Some(Simpl::Const(!invert));
    }
    None
}

/// Non-inverted AO-family: Ao21 (`is_ao=true`) = `(A & B) | C`; Oa21 =
/// `(A | B) & C`. Mirror of `simpl_aoi_family` without the output
/// inversion: when one leg neutralises the output becomes `C` (alias),
/// and when C or both leg inputs saturate the outer op the output is a
/// known constant.
fn simpl_ao_family(iv: &[Option<bool>], nets: &[NetId], is_ao: bool) -> Option<Simpl> {
    let short_circuit = is_ao; // value of C that dominates the outer op
    if iv[2] == Some(short_circuit) {
        return Some(Simpl::Const(short_circuit));
    }
    let leg_neutral = !short_circuit;
    if iv[0] == Some(leg_neutral) || iv[1] == Some(leg_neutral) {
        return Some(match iv[2] {
            Some(b) => Simpl::Const(b),
            None => Simpl::Alias(nets[2]),
        });
    }
    if iv[0] == Some(short_circuit) && iv[1] == Some(short_circuit) {
        return Some(Simpl::Const(short_circuit));
    }
    None
}

/// Combined single-sweep simplification: const-propagation, algebraic
/// rewrites, and CSE. Cells are driven by a worklist — the first pass
/// visits everything, subsequent iterations only revisit cells whose inputs
/// have been rewired. When a cell simplifies, its consumers are rewired to
/// point past it (so we don't pay for deep alias-chain walks) and enqueued.
pub(super) fn worklist_simplify(module: &mut GateModule) {
    let n_nets = module.nets.len();
    let n_cells = module.cells.len();

    let mut consumers: Vec<Vec<u32>> = vec![Vec::new(); n_nets];
    for (i, cell) in module.cells.iter().enumerate() {
        for &inp in &cell.inputs {
            consumers[inp as usize].push(i as u32);
        }
    }

    let mut seen: fxhash::FxHashMap<(CellKind, [NetId; 4]), u32> = fxhash::FxHashMap::default();

    let mut in_wl: Vec<bool> = vec![true; n_cells];
    let mut wl: Vec<u32> = (0..n_cells as u32).collect();

    fn push_wl(wl: &mut Vec<u32>, in_wl: &mut [bool], idx: u32) {
        let i = idx as usize;
        if !in_wl[i] {
            in_wl[i] = true;
            wl.push(idx);
        }
    }

    // Pre-pass: rewire consumers of existing Buf cells (created upstream
    // during comb block finalization). Without this, cells reading a
    // Buf-driven persistent net never see the aliased source.
    for cell_idx in 0..n_cells {
        let cell = &module.cells[cell_idx];
        if cell.kind != CellKind::Buf {
            continue;
        }
        let cell_output = cell.output;
        let src = cell.inputs[0];
        if src == cell_output {
            continue;
        }
        let old_consumers = mem::take(&mut consumers[cell_output as usize]);
        for consumer_idx in old_consumers {
            if consumer_idx as usize == cell_idx {
                continue;
            }
            let consumer = &mut module.cells[consumer_idx as usize];
            for inp in consumer.inputs.iter_mut() {
                if *inp == cell_output {
                    *inp = src;
                }
            }
            consumers[src as usize].push(consumer_idx);
            push_wl(&mut wl, &mut in_wl, consumer_idx);
        }
    }

    while let Some(cell_idx) = wl.pop() {
        in_wl[cell_idx as usize] = false;
        let cell = &module.cells[cell_idx as usize];
        if cell.kind == CellKind::Buf {
            continue;
        }
        let cell_kind = cell.kind;
        let cell_inputs = cell.inputs.clone();
        let cell_output = cell.output;

        let ivs: Vec<Option<bool>> = cell_inputs
            .iter()
            .map(|&n| match module.nets[n as usize].driver {
                NetDriver::Const(b) => Some(b),
                _ => None,
            })
            .collect();
        let simpl = simplify(cell_kind, &ivs, &cell_inputs);

        // Structural upstream fusion only fires when the upstream cell has a
        // single consumer — otherwise the upstream stays live and we'd just
        // add a parallel cell without removing the old one.
        let algebraic = if simpl.is_none() {
            algebraic_fuse(module, cell_kind, &cell_inputs, &consumers)
        } else {
            None
        };

        let cse_fold = if simpl.is_none() && algebraic.is_none() {
            if let Some(key) = cse_key(cell_kind, &cell_inputs) {
                if let Some(&other_idx) = seen.get(&key) {
                    if other_idx != cell_idx {
                        let other_out = module.cells[other_idx as usize].output;
                        Some(other_out)
                    } else {
                        None
                    }
                } else {
                    seen.insert(key, cell_idx);
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let new_kind: CellKind;
        let new_inputs: Vec<NetId>;
        let buf_src: Option<NetId>;

        if let Some(s) = simpl {
            match s {
                Simpl::Const(v) => {
                    let c = if v { NET_CONST1 } else { NET_CONST0 };
                    new_kind = CellKind::Buf;
                    new_inputs = vec![c];
                    buf_src = Some(c);
                }
                Simpl::Alias(n) => {
                    new_kind = CellKind::Buf;
                    new_inputs = vec![n];
                    buf_src = Some(n);
                }
                Simpl::Invert(n) => {
                    new_kind = CellKind::Not;
                    new_inputs = vec![n];
                    buf_src = None;
                }
            }
        } else if let Some((k, inputs)) = algebraic {
            if k == CellKind::Buf {
                buf_src = Some(inputs[0]);
            } else {
                buf_src = None;
            }
            new_kind = k;
            new_inputs = inputs;
        } else if let Some(src) = cse_fold {
            new_kind = CellKind::Buf;
            new_inputs = vec![src];
            buf_src = Some(src);
        } else {
            continue;
        }

        for &n in &cell_inputs {
            if let Some(list) = consumers.get_mut(n as usize)
                && let Some(pos) = list.iter().position(|&c| c == cell_idx)
            {
                list.swap_remove(pos);
            }
        }
        for &n in &new_inputs {
            if let Some(list) = consumers.get_mut(n as usize) {
                list.push(cell_idx);
            }
        }
        if let Some(old_key) = cse_key(cell_kind, &cell_inputs)
            && seen.get(&old_key) == Some(&cell_idx)
        {
            seen.remove(&old_key);
        }

        let cell = &mut module.cells[cell_idx as usize];
        cell.kind = new_kind;
        let new_inputs_snapshot = new_inputs.clone();
        cell.inputs = new_inputs;

        // Rewire consumers past this Buf so subsequent visits don't walk a
        // growing alias chain.
        if let Some(src) = buf_src {
            let old_consumers = mem::take(&mut consumers[cell_output as usize]);
            for consumer_idx in old_consumers {
                if consumer_idx == cell_idx {
                    continue;
                }
                // Consumer's CSE key may change once its input is rewired.
                let consumer = &module.cells[consumer_idx as usize];
                if let Some(ck) = cse_key(consumer.kind, &consumer.inputs)
                    && seen.get(&ck) == Some(&consumer_idx)
                {
                    seen.remove(&ck);
                }
                let consumer = &mut module.cells[consumer_idx as usize];
                for inp in consumer.inputs.iter_mut() {
                    if *inp == cell_output {
                        *inp = src;
                    }
                }
                consumers[src as usize].push(consumer_idx);
                push_wl(&mut wl, &mut in_wl, consumer_idx);
            }
        } else {
            // Kind changed but output stays — re-check CSE for this cell and
            // let its consumers react to the new kind.
            let _ = new_inputs_snapshot;
            push_wl(&mut wl, &mut in_wl, cell_idx);
            for &c in &consumers[cell_output as usize] {
                push_wl(&mut wl, &mut in_wl, c);
            }
        }
    }

    // Rewire FF and port nets past any remaining Bufs. These endpoints were
    // not in the consumers reverse graph, so their inputs may still reference
    // Buf output nets.
    let mut ff_resolved: Vec<(usize, NetId, NetId, Option<NetId>)> = Vec::new();
    for (i, ff) in module.ffs.iter().enumerate() {
        let d = resolve_alias(module, ff.d);
        let clk = resolve_alias(module, ff.clock);
        let rst = ff.reset.as_ref().map(|r| resolve_alias(module, r.net));
        ff_resolved.push((i, d, clk, rst));
    }
    for (i, d, clk, rst) in ff_resolved {
        let ff = &mut module.ffs[i];
        ff.d = d;
        ff.clock = clk;
        if let (Some(r), Some(new_net)) = (ff.reset.as_mut(), rst) {
            r.net = new_net;
        }
    }
    let mut port_resolved: Vec<(usize, Vec<NetId>)> = Vec::new();
    for (i, port) in module.ports.iter().enumerate() {
        let resolved: Vec<NetId> = port
            .nets
            .iter()
            .map(|&n| resolve_alias(module, n))
            .collect();
        port_resolved.push((i, resolved));
    }
    for (i, nets) in port_resolved {
        module.ports[i].nets = nets;
    }

    let old_cells = mem::take(&mut module.cells);
    let mut index_map: Vec<Option<u32>> = vec![None; old_cells.len()];
    let mut new_cells: Vec<Cell> = Vec::with_capacity(old_cells.len());
    for (old_idx, cell) in old_cells.into_iter().enumerate() {
        if cell.kind == CellKind::Buf {
            continue;
        }
        index_map[old_idx] = Some(new_cells.len() as u32);
        new_cells.push(cell);
    }
    module.cells = new_cells;
    for net in module.nets.iter_mut() {
        if let NetDriver::Cell(idx) = &mut net.driver {
            match index_map[*idx] {
                Some(new_idx) => *idx = new_idx as usize,
                None => net.driver = NetDriver::Undriven,
            }
        }
    }
}

/// Removes cells whose output is not consumed by any cell, FF, or port.
/// Seeds the live set from FF inputs and port nets, then walks backwards
/// through the cell graph marking every cell that contributes to a live net.
pub(super) fn dead_cell_elimination(module: &mut GateModule) {
    // Reverse BFS from live endpoints (FF inputs + module ports) back
    // through the cell graph. Each cell is visited at most once — O(N ×
    // avg_arity) instead of the O(N × depth) fixed-point sweep. For 400K
    // cells this is the difference between 230ms and sub-10ms per call.
    let mut alive = vec![false; module.cells.len()];
    let mut queue: Vec<NetId> = Vec::new();
    for ff in &module.ffs {
        queue.push(ff.d);
        queue.push(ff.clock);
        if let Some(r) = &ff.reset {
            queue.push(r.net);
        }
    }
    for port in &module.ports {
        queue.extend(port.nets.iter().copied());
    }
    while let Some(net) = queue.pop() {
        if let NetDriver::Cell(idx) = module.nets[net as usize].driver {
            if alive[idx] {
                continue;
            }
            alive[idx] = true;
            queue.extend(module.cells[idx].inputs.iter().copied());
        }
    }

    if alive.iter().all(|&a| a) {
        return;
    }

    let old = mem::take(&mut module.cells);
    let mut index_map: Vec<Option<usize>> = vec![None; old.len()];
    let mut new_cells: Vec<Cell> = Vec::with_capacity(old.len());
    for (old_idx, cell) in old.into_iter().enumerate() {
        if !alive[old_idx] {
            continue;
        }
        index_map[old_idx] = Some(new_cells.len());
        new_cells.push(cell);
    }
    module.cells = new_cells;

    for net in module.nets.iter_mut() {
        if let NetDriver::Cell(idx) = &mut net.driver {
            match index_map[*idx] {
                Some(new_idx) => *idx = new_idx,
                None => net.driver = NetDriver::Undriven,
            }
        }
    }
}
