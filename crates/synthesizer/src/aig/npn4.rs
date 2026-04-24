//! 4-variable Boolean truth tables, NPN4 canonicalization, and an
//! enumeration-based AIG pattern library used by [`aig_rewrite`]. The
//! library is populated lazily on first use; at program start we
//! enumerate every AIG pattern with up to [`MAX_ANDS`] AND gates over 4
//! input variables, record the smallest pattern for each
//! NPN-equivalence class, and then query the library to see whether a
//! 4-cut in the real AIG can be replaced with fewer ANDs.
//!
//! Two operations make this work: [`npn_canonical`] computes a canonical
//! representative of the cut's truth table across all
//! (negate/permute/negate) transforms, and [`transform_pattern`] applies
//! the inverse transform at rewrite time so the stored canonical pattern
//! is adapted to the real cut's leaf ordering and polarities.

use std::collections::HashMap;
use std::sync::OnceLock;

/// 4-input Boolean function TT: 16 entries packed into u16. Bit m is the
/// function value when inputs encode m (bit 0 → x0, bit 3 → x3).
pub type Tt4 = u16;

/// Canonical TTs for the 4 input variables.
pub const VAR_TT: [Tt4; 4] = [0xAAAA, 0xCCCC, 0xF0F0, 0xFF00];

/// Maximum AIG size enumerated for the library. K=3 yields ~800k program
/// records which populates all 4-var NPN classes realisable with 3
/// ANDs (covers AND/OR/XOR/MUX variants). K=4 explodes to ~10^8 records
/// and is not worth the startup cost.
pub const MAX_ANDS: u8 = 3;

/// All 24 permutations of [0, 1, 2, 3].
const ALL_PERMS: [[u8; 4]; 24] = [
    [0, 1, 2, 3],
    [0, 1, 3, 2],
    [0, 2, 1, 3],
    [0, 2, 3, 1],
    [0, 3, 1, 2],
    [0, 3, 2, 1],
    [1, 0, 2, 3],
    [1, 0, 3, 2],
    [1, 2, 0, 3],
    [1, 2, 3, 0],
    [1, 3, 0, 2],
    [1, 3, 2, 0],
    [2, 0, 1, 3],
    [2, 0, 3, 1],
    [2, 1, 0, 3],
    [2, 1, 3, 0],
    [2, 3, 0, 1],
    [2, 3, 1, 0],
    [3, 0, 1, 2],
    [3, 0, 2, 1],
    [3, 1, 0, 2],
    [3, 1, 2, 0],
    [3, 2, 0, 1],
    [3, 2, 1, 0],
];

/// Permute a TT so that new variable i corresponds to old variable perm[i].
/// Equivalently: new_tt(y_0..y_3) = old_tt(y_{perm_inv[0]}, ..., y_{perm_inv[3]}).
pub fn perm_tt(tt: Tt4, perm: [u8; 4]) -> Tt4 {
    let mut out: u32 = 0;
    let tt = tt as u32;
    for m in 0..16u32 {
        let mut mprime: u32 = 0;
        for (i, &p) in perm.iter().enumerate() {
            let bit = (m >> i) & 1;
            mprime |= bit << p;
        }
        out |= ((tt >> mprime) & 1) << m;
    }
    out as Tt4
}

/// Invert the specified input variables. Bit i of `mask` set ⇒ x_i is
/// negated in the *input* space. new_tt[m] = old_tt[m ^ mask].
pub fn flip_inputs(tt: Tt4, mask: u8) -> Tt4 {
    let mask = mask as u32 & 0xF;
    let mut out: u32 = 0;
    let tt = tt as u32;
    for m in 0..16u32 {
        out |= ((tt >> (m ^ mask)) & 1) << m;
    }
    out as Tt4
}

/// NPN transform: permute inputs, negate specific inputs, and/or negate
/// output. Applied to a TT as `flip_output(flip_inputs(perm_tt(tt, perm), in_neg), out_neg)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct NpnTransform {
    /// perm[i] = old variable index assigned to new position i.
    pub perm: [u8; 4],
    /// Bit i set ⇒ negate the i-th *new* (post-perm) input variable.
    pub in_neg: u8,
    /// Whether to negate the output.
    pub out_neg: bool,
}

impl NpnTransform {
    pub const IDENTITY: NpnTransform = NpnTransform {
        perm: [0, 1, 2, 3],
        in_neg: 0,
        out_neg: false,
    };

    /// Apply this transform to a TT.
    pub fn apply(self, tt: Tt4) -> Tt4 {
        let t = perm_tt(tt, self.perm);
        let t = flip_inputs(t, self.in_neg);
        if self.out_neg { !t } else { t }
    }
}

/// Precomputed lookup tables: `PERM_TABLE[i * 65536 + tt] = perm_tt(tt, ALL_PERMS[i])`.
/// 24 × 65536 × 2 B = 3 MB. Flat Vec keeps allocation off the stack.
fn perm_table() -> &'static Vec<Tt4> {
    static T: OnceLock<Vec<Tt4>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![0u16; 24 * 65536];
        for (i, &perm) in ALL_PERMS.iter().enumerate() {
            let base = i * 65536;
            for tt in 0..=65535u32 {
                t[base + tt as usize] = perm_tt(tt as Tt4, perm);
            }
        }
        t
    })
}

/// Compute the canonical NPN4 form of `tt`. Returns `(canonical, t)` such
/// that `t.apply(tt) = canonical`. The canonical is the numerically
/// smallest TT reachable by any of the 768 (perm × in_neg × out_neg)
/// transforms.
pub fn npn_canonical(tt: Tt4) -> (Tt4, NpnTransform) {
    let table = perm_table();
    let mut best_tt = tt;
    let mut best = NpnTransform::IDENTITY;

    for (pi, &perm) in ALL_PERMS.iter().enumerate() {
        let permed = table[pi * 65536 + tt as usize];
        for in_neg in 0..16u8 {
            let flipped = flip_inputs(permed, in_neg);
            if flipped < best_tt {
                best_tt = flipped;
                best = NpnTransform {
                    perm,
                    in_neg,
                    out_neg: false,
                };
            }
            let neg = !flipped;
            if neg < best_tt {
                best_tt = neg;
                best = NpnTransform {
                    perm,
                    in_neg,
                    out_neg: true,
                };
            }
        }
    }
    (best_tt, best)
}

/// Edge in a pattern: `(node, negated)`. `node < 4` ⇒ input variable.
/// `node >= 4` ⇒ AND gate at index (node - 4) in the pattern's `ands`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatEdge(pub u8, pub bool);

#[derive(Clone, Debug)]
pub struct AigPattern {
    pub ands: Vec<(PatEdge, PatEdge)>,
    pub output: PatEdge,
}

impl AigPattern {
    pub fn size(&self) -> usize {
        self.ands.len()
    }

    /// Evaluate this pattern over TTs for its four input variables.
    pub fn eval(&self, vars: [Tt4; 4]) -> Tt4 {
        let mut values: Vec<Tt4> = vec![vars[0], vars[1], vars[2], vars[3]];
        values.reserve(self.ands.len());
        for &(a, b) in &self.ands {
            let va = values[a.0 as usize] ^ if a.1 { !0 } else { 0 };
            let vb = values[b.0 as usize] ^ if b.1 { !0 } else { 0 };
            values.push(va & vb);
        }
        let o = self.output;
        values[o.0 as usize] ^ if o.1 { !0 } else { 0 }
    }

    pub fn tt(&self) -> Tt4 {
        self.eval(VAR_TT)
    }
}

/// Apply `t` to pattern `pat`. Returns a new pattern computing
/// `t.apply(pat.tt())`.
///
/// Derivation: `t.apply(tt)(y) = XOR^out_neg tt(z)` where
/// `z_{perm[i]} = y_i XOR in_neg_bit(i)`, i.e. `z_j = y_{perm_inv[j]} XOR in_neg_bit(perm_inv[j])`.
/// So wherever the old pattern references variable j, we substitute
/// `y_{perm_inv[j]}` with the matching polarity; the output XORs with
/// `out_neg`.
pub fn transform_pattern(pat: &AigPattern, t: NpnTransform) -> AigPattern {
    let mut perm_inv = [0u8; 4];
    for (i, &p) in t.perm.iter().enumerate() {
        perm_inv[p as usize] = i as u8;
    }
    let var_subst: [(u8, bool); 4] = [0u8, 1, 2, 3].map(|j| {
        let new_node = perm_inv[j as usize];
        let neg = ((t.in_neg >> new_node) & 1) != 0;
        (new_node, neg)
    });

    let map_edge = |PatEdge(node, neg): PatEdge| -> PatEdge {
        if node < 4 {
            let (n, vn) = var_subst[node as usize];
            PatEdge(n, neg ^ vn)
        } else {
            PatEdge(node, neg)
        }
    };

    let ands = pat
        .ands
        .iter()
        .map(|&(a, b)| (map_edge(a), map_edge(b)))
        .collect();
    let o = map_edge(pat.output);
    let output = PatEdge(o.0, o.1 ^ t.out_neg);
    AigPattern { ands, output }
}

/// Returns the process-wide NPN4 library. Built lazily on first access.
fn library() -> &'static HashMap<Tt4, AigPattern> {
    static LIB: OnceLock<HashMap<Tt4, AigPattern>> = OnceLock::new();
    LIB.get_or_init(build_library)
}

/// Look up the smallest stored pattern that computes the canonical TT
/// `canonical_tt`. Note: the returned pattern's `tt()` equals
/// `canonical_tt` directly — no further transform is required before
/// adaptation via [`transform_pattern`] for a specific cut.
pub fn lookup_canonical(canonical_tt: Tt4) -> Option<&'static AigPattern> {
    library().get(&canonical_tt)
}

/// Enumerate every AIG pattern of up to `MAX_ANDS` ANDs over 4 input
/// variables, and for each NPN-class store the smallest (in AND count)
/// pattern that *computes the canonical representative directly*. Later
/// callers adapt via [`transform_pattern`].
fn build_library() -> HashMap<Tt4, AigPattern> {
    // First pass: enumerate every pattern, dedup by its raw TT and keep
    // the smallest-size pattern per TT. Avoids re-canonicalising patterns
    // with the same TT many times.
    let mut by_tt: HashMap<Tt4, AigPattern> = HashMap::with_capacity(65536);

    fn enumerate(
        ands: &mut Vec<(PatEdge, PatEdge)>,
        remaining: u8,
        by_tt: &mut HashMap<Tt4, AigPattern>,
    ) {
        // Try each node as output (positive + negated polarity).
        let n_nodes = 4 + ands.len() as u8;
        for out_node in 0..n_nodes {
            for &out_neg in &[false, true] {
                let pat = AigPattern {
                    ands: ands.clone(),
                    output: PatEdge(out_node, out_neg),
                };
                let tt = pat.tt();
                match by_tt.get(&tt) {
                    Some(existing) if existing.size() <= pat.size() => {}
                    _ => {
                        by_tt.insert(tt, pat);
                    }
                }
            }
        }

        if remaining == 0 {
            return;
        }

        let max_edge = 4 + ands.len() as u8;
        for a_node in 0..max_edge {
            for &a_neg in &[false, true] {
                for b_node in a_node..max_edge {
                    for &b_neg in &[false, true] {
                        if a_node == b_node {
                            continue;
                        }
                        ands.push((PatEdge(a_node, a_neg), PatEdge(b_node, b_neg)));
                        enumerate(ands, remaining - 1, by_tt);
                        ands.pop();
                    }
                }
            }
        }
    }

    let mut ands = Vec::with_capacity(MAX_ANDS as usize);
    enumerate(&mut ands, MAX_ANDS, &mut by_tt);

    // Second pass: reduce to canonical keys by applying npn_canonical
    // once per unique TT, transforming the stored pattern accordingly.
    let mut best: HashMap<Tt4, AigPattern> = HashMap::with_capacity(by_tt.len());
    for (tt, pat) in by_tt {
        let (canonical, t) = npn_canonical(tt);
        let canon_pat = transform_pattern(&pat, t);
        debug_assert_eq!(canon_pat.tt(), canonical);
        match best.get(&canonical) {
            Some(existing) if existing.size() <= canon_pat.size() => {}
            _ => {
                best.insert(canonical, canon_pat);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_tts_basic() {
        assert_eq!(VAR_TT[0], 0xAAAA);
        assert_eq!(VAR_TT[1], 0xCCCC);
        assert_eq!(VAR_TT[2], 0xF0F0);
        assert_eq!(VAR_TT[3], 0xFF00);
    }

    #[test]
    fn perm_identity() {
        let tt = 0x1234;
        assert_eq!(perm_tt(tt, [0, 1, 2, 3]), tt);
    }

    #[test]
    fn perm_swap01() {
        // TT for x0 & x1 = 0x8888. Swapping x0 and x1 leaves AND unchanged.
        let and01 = VAR_TT[0] & VAR_TT[1];
        assert_eq!(perm_tt(and01, [1, 0, 2, 3]), and01);
    }

    #[test]
    fn flip_inputs_xor() {
        // f = x0, f(!x0) = !x0. flip_inputs(VAR0, 0b0001) should equal !VAR0.
        assert_eq!(flip_inputs(VAR_TT[0], 0b0001), !VAR_TT[0]);
    }

    #[test]
    fn canonical_of_var_is_smallest() {
        let (c, _) = npn_canonical(VAR_TT[0]);
        // The canonical of any var must equal canonical of any other var
        // (same NPN class: trivial projections).
        for &v in VAR_TT.iter().skip(1) {
            assert_eq!(npn_canonical(v).0, c);
        }
    }

    #[test]
    fn transform_round_trip() {
        let tt = 0x6A5B;
        let (canonical, t) = npn_canonical(tt);
        assert_eq!(t.apply(tt), canonical);
    }

    #[test]
    fn transform_pattern_computes_canonical() {
        // x0 & x1, TT = 0x8888.
        let pat = AigPattern {
            ands: vec![(PatEdge(0, false), PatEdge(1, false))],
            output: PatEdge(4, false),
        };
        let tt = pat.tt();
        assert_eq!(tt, 0x8888);
        let (canonical, t) = npn_canonical(tt);
        let canon_pat = transform_pattern(&pat, t);
        assert_eq!(canon_pat.tt(), canonical);
    }

    #[test]
    fn library_contains_and2() {
        // NPN class of x0 & x1 must be present with 1 AND.
        let (c, _) = npn_canonical(VAR_TT[0] & VAR_TT[1]);
        let pat = lookup_canonical(c).expect("AND2 class present");
        assert_eq!(pat.size(), 1);
        assert_eq!(pat.tt(), c);
    }

    #[test]
    fn library_contains_xor2() {
        let xor = VAR_TT[0] ^ VAR_TT[1];
        let (c, _) = npn_canonical(xor);
        let pat = lookup_canonical(c).expect("XOR2 class present");
        // XOR2 needs 3 ANDs via AIG.
        assert!(
            pat.size() <= 3,
            "xor2 library entry has {} ands",
            pat.size()
        );
        assert_eq!(pat.tt(), c);
    }

    #[test]
    fn library_size_sanity() {
        let lib = super::library();
        // With MAX_ANDS=3 only a small fraction of the 222 NPN4 classes
        // are realisable; the rest require ≥4 ANDs. We still expect ≥10
        // (the common projections, AND/OR/AND3/OR3/XOR/MUX families).
        assert!(lib.len() >= 10, "library has only {} classes", lib.len());
    }

    #[test]
    fn library_contains_mux2() {
        // s ? b : a = (s & b) | (!s & a), needs 3 ANDs.
        let mux = (VAR_TT[0] & VAR_TT[1]) | (!VAR_TT[0] & VAR_TT[2]);
        let (c, _) = npn_canonical(mux);
        let pat = lookup_canonical(c).expect("MUX class present");
        assert!(pat.size() <= 3, "mux library entry has {} ands", pat.size());
    }

    #[test]
    fn library_contains_and3() {
        let and3 = VAR_TT[0] & VAR_TT[1] & VAR_TT[2];
        let (c, _) = npn_canonical(and3);
        let pat = lookup_canonical(c).expect("AND3 class present");
        assert_eq!(pat.size(), 2, "AND3 should be 2 ANDs");
    }
}
