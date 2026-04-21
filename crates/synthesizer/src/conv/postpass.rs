use std::collections::HashMap;
use std::mem;

use crate::ir::{Cell, CellKind, GateModule, NetDriver, NetId, NetInfo};

/// Post-optimization tech-mapping sweep. Runs after the worklist loop
/// and DCE have settled, then applies a pipeline of structural rewrites
/// that need up-to-date consumer counts (which the worklist can't provide
/// mid-flight). The sweep is organised into independent phases, each
/// handling one class of rewrite; they share a consumer-count array and
/// caches for materialised helper cells via [`PostPassCtx`].
///
/// Pipeline order matters: early phases expose constants / shape patterns
/// that later phases pick up. The fusion fixed-point at the end absorbs
/// any residual primitive chains into compound cells.
pub(super) fn complex_gate_replacement(module: &mut GateModule) {
    let mut ctx = PostPassCtx::new(module);
    ctx.collapse_same_sel_nesting();
    ctx.combine_mux_of_mux_shared_leg();
    ctx.distribute_boolean_factor();
    ctx.factor_mux_common_input();
    ctx.collapse_mux_to_primitive();
    ctx.fuse_compound_gates();
}

/// Shared bookkeeping for [`complex_gate_replacement`]'s phases: an owned
/// consumer-count table that each rewrite keeps in sync, plus caches so
/// materialised helper cells (the `!sel` / `Or2(s1,s2)` / `And2(s1,s2)`
/// that several patterns need) are shared across phases instead of being
/// duplicated per phase.
struct PostPassCtx<'a> {
    module: &'a mut GateModule,
    consumer_count: Vec<u32>,
    not_cache: HashMap<NetId, NetId>,
    or_cache: HashMap<(NetId, NetId), NetId>,
    and_cache: HashMap<(NetId, NetId), NetId>,
}

/// Position of the inner Mux2 relative to the outer in a Mux-of-Mux
/// rewrite. See [`PostPassCtx::combine_mux_of_mux_shared_leg`] for the
/// algebraic identity behind each variant.
#[derive(Clone, Copy)]
enum MomPattern {
    /// outer.d0 = inner; outer.d1 == inner.d1 → `Mux2(Or2(s1, s2), a, c)`
    A,
    /// outer.d1 = inner; outer.d0 == inner.d0 → `Mux2(And2(s1, s2), a, c)`
    B,
    /// outer.d0 = inner; outer.d1 == inner.d0 → `Mux2(And2(!s1, s2), a, c)`
    C,
    /// outer.d1 = inner; outer.d0 == inner.d1 → `Mux2(And2(s1, !s2), a, c)`
    D,
}

/// How to collapse a Mux2 whose data inputs are constant (or mirror the
/// select signal). See [`PostPassCtx::collapse_mux_to_primitive`].
#[derive(Clone, Copy)]
#[allow(clippy::enum_variant_names)]
enum MuxRewrite {
    /// `Mux2(s, 0, x)` → `And2(s, x)` — no Not needed.
    AndSel,
    /// `Mux2(s, x, 1)` → `Or2(s, x)`.
    OrSel,
    /// `Mux2(s, s, x)` → `And2(s, x)` — sel equals d0.
    AndSelD1,
    /// `Mux2(s, x, s)` → `Or2(s, x)` — sel equals d1.
    OrSelD0,
    /// `Mux2(s, x, 0)` → `And2(!s, x)` — materialises Not(s).
    AndNotSel,
    /// `Mux2(s, 1, x)` → `Or2(!s, x)`.
    OrNotSel,
    /// `Mux2(s, x, !x)` → `Xor2(s, x)`.
    XorSel,
    /// `Mux2(s, !x, x)` → `Xnor2(s, x)`.
    XnorSel,
}

impl<'a> PostPassCtx<'a> {
    fn new(module: &'a mut GateModule) -> Self {
        let n_nets = module.nets.len();

        // Count every live consumer of each net: cell inputs + FF pins +
        // port nets. FF / port counts matter because fusion would orphan
        // the inner cell, and if the inner directly drives an endpoint
        // we must not drop it.
        let mut consumer_count = vec![0u32; n_nets];
        for cell in &module.cells {
            for &inp in &cell.inputs {
                consumer_count[inp as usize] += 1;
            }
        }
        for ff in &module.ffs {
            consumer_count[ff.d as usize] += 1;
            consumer_count[ff.clock as usize] += 1;
            if let Some(r) = &ff.reset {
                consumer_count[r.net as usize] += 1;
            }
        }
        for port in &module.ports {
            for &n in &port.nets {
                consumer_count[n as usize] += 1;
            }
        }

        // Seed the `!sel` cache from existing Not cells so later phases
        // reuse them instead of cloning. The Or / And caches start empty —
        // only the shared-leg phase and the cross-position Mux-of-Mux
        // patterns populate them on demand.
        let mut not_cache: HashMap<NetId, NetId> = HashMap::new();
        for cell in module.cells.iter() {
            if cell.kind == CellKind::Not {
                not_cache.entry(cell.inputs[0]).or_insert(cell.output);
            }
        }

        Self {
            module,
            consumer_count,
            not_cache,
            or_cache: HashMap::new(),
            and_cache: HashMap::new(),
        }
    }

    /// Allocate (or reuse) `Not(src)` and return the output net. Caches
    /// across phases so a single inverter serves every rewrite that
    /// needs `!src`.
    fn materialize_not(&mut self, src: NetId) -> NetId {
        if let Some(&existing) = self.not_cache.get(&src) {
            return existing;
        }
        let new_net = self.module.nets.len() as NetId;
        self.module.nets.push(NetInfo {
            driver: NetDriver::Undriven,
            origin: None,
        });
        self.consumer_count.push(0);
        let idx = self.module.cells.len();
        self.module.cells.push(Cell {
            kind: CellKind::Not,
            inputs: vec![src],
            output: new_net,
        });
        self.module.nets[new_net as usize].driver = NetDriver::Cell(idx);
        self.consumer_count[src as usize] += 1;
        self.not_cache.insert(src, new_net);
        new_net
    }

    /// Allocate (or reuse) a 2-input commutative gate `kind(a, b)`. The
    /// cache is keyed on the sorted `(lo, hi)` pair so `(a, b)` and
    /// `(b, a)` share a cell.
    fn materialize_binop(&mut self, kind: CellKind, a: NetId, b: NetId) -> NetId {
        let key = if a <= b { (a, b) } else { (b, a) };
        let cache = match kind {
            CellKind::Or2 => &mut self.or_cache,
            CellKind::And2 => &mut self.and_cache,
            _ => unreachable!("materialize_binop only supports Or2 / And2"),
        };
        if let Some(&existing) = cache.get(&key) {
            return existing;
        }
        let new_net = self.module.nets.len() as NetId;
        let idx = self.module.cells.len();
        self.module.nets.push(NetInfo {
            driver: NetDriver::Undriven,
            origin: None,
        });
        self.consumer_count.push(0);
        self.module.cells.push(Cell {
            kind,
            inputs: vec![key.0, key.1],
            output: new_net,
        });
        self.module.nets[new_net as usize].driver = NetDriver::Cell(idx);
        self.consumer_count[key.0 as usize] += 1;
        self.consumer_count[key.1 as usize] += 1;
        cache.insert(key, new_net);
        new_net
    }

    /// Swap a cell's inputs to a new list and keep the consumer counts
    /// in sync. Does not touch `kind`.
    fn retarget_inputs(&mut self, cell_idx: usize, new_inputs: Vec<NetId>) {
        let old = mem::take(&mut self.module.cells[cell_idx].inputs);
        for &n in &old {
            self.consumer_count[n as usize] -= 1;
        }
        for &n in &new_inputs {
            self.consumer_count[n as usize] += 1;
        }
        self.module.cells[cell_idx].inputs = new_inputs;
    }

    /// Rewrite a cell fully (kind + inputs) and keep consumer counts in sync.
    fn rewrite_cell(&mut self, cell_idx: usize, new_kind: CellKind, new_inputs: Vec<NetId>) {
        self.retarget_inputs(cell_idx, new_inputs);
        self.module.cells[cell_idx].kind = new_kind;
    }

    /// Collapse nested Mux2 whose selects match (or are inverted) with
    /// the outer's select.
    ///
    /// Same phase (inner.sel == outer.sel = s):
    /// * `Mux2(s, a, Mux2(s, b, c)) ≡ Mux2(s, a, c)`  — skip inner d0 (b)
    /// * `Mux2(s, Mux2(s, a, b), c) ≡ Mux2(s, a, c)`  — skip inner d1 (b)
    ///
    /// Opposite phase (inner.sel = Not(s)):
    /// * `Mux2(s, a, Mux2(!s, b, c)) ≡ Mux2(s, a, b)`  — pick inner.d0
    /// * `Mux2(s, Mux2(!s, a, b), c) ≡ Mux2(s, b, c)`  — pick inner.d1
    ///
    /// Safe without single-consumer checks since we only edit the outer
    /// mux; the inner stays alive for its other users. Iterates to a
    /// fixed point because collapsing one level exposes a deeper one.
    fn collapse_same_sel_nesting(&mut self) {
        loop {
            let mut any = false;
            for cell_idx in 0..self.module.cells.len() {
                if self.module.cells[cell_idx].kind != CellKind::Mux2 {
                    continue;
                }
                let s = self.module.cells[cell_idx].inputs[0];
                let d0 = self.module.cells[cell_idx].inputs[1];
                let d1 = self.module.cells[cell_idx].inputs[2];

                let d1_inner = self.peek_same_sel_mux(d1, s);
                let d0_inner = self.peek_same_sel_mux(d0, s);

                if let Some((in_d0, in_d1, same_phase)) = d1_inner {
                    let replacement = if same_phase { in_d1 } else { in_d0 };
                    self.consumer_count[d1 as usize] -= 1;
                    self.consumer_count[replacement as usize] += 1;
                    self.module.cells[cell_idx].inputs[2] = replacement;
                    any = true;
                } else if let Some((in_d0, in_d1, same_phase)) = d0_inner {
                    let replacement = if same_phase { in_d0 } else { in_d1 };
                    self.consumer_count[d0 as usize] -= 1;
                    self.consumer_count[replacement as usize] += 1;
                    self.module.cells[cell_idx].inputs[1] = replacement;
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
    }

    /// Helper for [`Self::collapse_same_sel_nesting`]: report the inner
    /// Mux2's (d0, d1, same_phase) if `net` is driven by a Mux2 whose
    /// select matches `outer_sel` directly or via a single Not cell.
    fn peek_same_sel_mux(&self, net: NetId, outer_sel: NetId) -> Option<(NetId, NetId, bool)> {
        let NetDriver::Cell(idx) = self.module.nets[net as usize].driver else {
            return None;
        };
        let inner = &self.module.cells[idx];
        if inner.kind != CellKind::Mux2 {
            return None;
        }
        let isel = inner.inputs[0];
        if isel == outer_sel {
            return Some((inner.inputs[1], inner.inputs[2], true));
        }
        if let NetDriver::Cell(not_idx) = self.module.nets[isel as usize].driver {
            let n = &self.module.cells[not_idx];
            if n.kind == CellKind::Not && n.inputs[0] == outer_sel {
                return Some((inner.inputs[1], inner.inputs[2], false));
            }
        }
        None
    }

    /// Collapse `Mux2(s1, ..., Mux2(s2, ..., ...))` when the outer and
    /// inner share one data leg. Four variants depending on which
    /// positions the shared leg occupies — see [`MomPattern`].
    ///
    /// Requires the inner to have a single consumer so it dies after
    /// the rewrite (otherwise the saving is lost to a parallel live
    /// inner). Iterates: the outer's new sel is a fresh And2/Or2 that
    /// may match with an enclosing mux on the next sweep.
    fn combine_mux_of_mux_shared_leg(&mut self) {
        loop {
            let mut did_any = false;
            for cell_idx in 0..self.module.cells.len() {
                if self.module.cells[cell_idx].kind != CellKind::Mux2 {
                    continue;
                }
                let s1 = self.module.cells[cell_idx].inputs[0];
                let d0 = self.module.cells[cell_idx].inputs[1];
                let d1 = self.module.cells[cell_idx].inputs[2];

                let hit = self
                    .classify_mom(s1, d0, d1, true)
                    .or_else(|| self.classify_mom(s1, d1, d0, false));
                let Some((pat, s2, other)) = hit else {
                    continue;
                };

                let (new_sel, new_d0, new_d1) = match pat {
                    MomPattern::A => (self.materialize_binop(CellKind::Or2, s1, s2), other, d1),
                    MomPattern::B => (self.materialize_binop(CellKind::And2, s1, s2), d0, other),
                    MomPattern::C => {
                        let not_s1 = self.materialize_not(s1);
                        (
                            self.materialize_binop(CellKind::And2, not_s1, s2),
                            d1,
                            other,
                        )
                    }
                    MomPattern::D => {
                        let not_s2 = self.materialize_not(s2);
                        (
                            self.materialize_binop(CellKind::And2, s1, not_s2),
                            d0,
                            other,
                        )
                    }
                };
                self.retarget_inputs(cell_idx, vec![new_sel, new_d0, new_d1]);
                did_any = true;
            }
            if !did_any {
                break;
            }
        }
    }

    /// Helper for [`Self::combine_mux_of_mux_shared_leg`]: classify a
    /// Mux2's data leg as one of the four MoM patterns. `outer_d_inner`
    /// is the leg that must be a Mux2; `outer_d_shared` is the leg
    /// whose value should match one of the inner's data legs.
    fn classify_mom(
        &self,
        s1: NetId,
        outer_d_inner: NetId,
        outer_d_shared: NetId,
        inner_is_d0_leg: bool,
    ) -> Option<(MomPattern, NetId, NetId)> {
        if self.consumer_count[outer_d_inner as usize] != 1 {
            return None;
        }
        let NetDriver::Cell(iidx) = self.module.nets[outer_d_inner as usize].driver else {
            return None;
        };
        let inner = &self.module.cells[iidx];
        if inner.kind != CellKind::Mux2 || inner.inputs[0] == s1 {
            return None;
        }
        let s2 = inner.inputs[0];
        let in_d0 = inner.inputs[1];
        let in_d1 = inner.inputs[2];
        if inner_is_d0_leg {
            if in_d1 == outer_d_shared {
                return Some((MomPattern::A, s2, in_d0));
            }
            if in_d0 == outer_d_shared {
                return Some((MomPattern::C, s2, in_d1));
            }
        } else {
            if in_d0 == outer_d_shared {
                return Some((MomPattern::B, s2, in_d1));
            }
            if in_d1 == outer_d_shared {
                return Some((MomPattern::D, s2, in_d0));
            }
        }
        None
    }

    /// Boolean distribution: factor a shared input out of a two-arm gate.
    ///   `Or2 (And2(x, a), And2(x, b))` → `And2(x, Or2 (a, b))`
    ///   `And2(Or2 (x, a), Or2 (x, b))` → `Or2 (x, And2(a, b))`
    ///
    /// Both arms must be single-consumer so the rewrite actually saves a
    /// cell. Reuses the existing arm cell `i0` as the new inner gate
    /// (kind flipped, inputs swapped) so no allocation is needed; the
    /// other arm cell dies via DCE later.
    fn distribute_boolean_factor(&mut self) {
        loop {
            let mut did_any = false;
            for cell_idx in 0..self.module.cells.len() {
                let outer_kind = self.module.cells[cell_idx].kind;
                let (target_arm_kind, new_outer_kind) = match outer_kind {
                    CellKind::Or2 => (CellKind::And2, CellKind::And2),
                    CellKind::And2 => (CellKind::Or2, CellKind::Or2),
                    _ => continue,
                };
                let inputs = self.module.cells[cell_idx].inputs.clone();
                if self.consumer_count[inputs[0] as usize] != 1
                    || self.consumer_count[inputs[1] as usize] != 1
                {
                    continue;
                }
                let NetDriver::Cell(i0) = self.module.nets[inputs[0] as usize].driver else {
                    continue;
                };
                let NetDriver::Cell(i1) = self.module.nets[inputs[1] as usize].driver else {
                    continue;
                };
                if self.module.cells[i0].kind != target_arm_kind
                    || self.module.cells[i1].kind != target_arm_kind
                    || i0 == i1
                {
                    continue;
                }
                let a0 = self.module.cells[i0].inputs[0];
                let a1 = self.module.cells[i0].inputs[1];
                let b0 = self.module.cells[i1].inputs[0];
                let b1 = self.module.cells[i1].inputs[1];
                let Some((common, left, right)) = find_common_input(a0, a1, b0, b1) else {
                    continue;
                };

                self.rewrite_cell(i0, outer_kind, vec![left, right]);
                let inner_out = self.module.cells[i0].output;
                self.rewrite_cell(cell_idx, new_outer_kind, vec![common, inner_out]);
                did_any = true;
            }
            if !did_any {
                break;
            }
        }
    }

    /// Common factor extraction from Mux2 arms.
    ///   `Mux2(s, Op(x, a), Op(x, b))` → `Op(x, Mux2(s, a, b))`
    /// for commutative 2-input `Op ∈ {And2, Or2, Xor2}`. Both arm cells
    /// must be single-consumer. Allocates a fresh Mux2 for `(s, a, b)`
    /// and rewrites the original Mux2 cell into `Op(x, new_mux_out)`.
    fn factor_mux_common_input(&mut self) {
        loop {
            let mut did_any = false;
            for cell_idx in 0..self.module.cells.len() {
                if self.module.cells[cell_idx].kind != CellKind::Mux2 {
                    continue;
                }
                let s = self.module.cells[cell_idx].inputs[0];
                let d0 = self.module.cells[cell_idx].inputs[1];
                let d1 = self.module.cells[cell_idx].inputs[2];
                if self.consumer_count[d0 as usize] != 1 || self.consumer_count[d1 as usize] != 1 {
                    continue;
                }
                let NetDriver::Cell(i0) = self.module.nets[d0 as usize].driver else {
                    continue;
                };
                let NetDriver::Cell(i1) = self.module.nets[d1 as usize].driver else {
                    continue;
                };
                let op = self.module.cells[i0].kind;
                if op != self.module.cells[i1].kind
                    || !matches!(op, CellKind::And2 | CellKind::Or2 | CellKind::Xor2)
                {
                    continue;
                }
                let a0 = self.module.cells[i0].inputs[0];
                let a1 = self.module.cells[i0].inputs[1];
                let b0 = self.module.cells[i1].inputs[0];
                let b1 = self.module.cells[i1].inputs[1];
                let Some((common, left, right)) = find_common_input(a0, a1, b0, b1) else {
                    continue;
                };

                // Bypass materialize_binop: Mux2 isn't commutative, so the
                // (s, left, right) tuple shouldn't share the 2-input cache.
                let new_net = self.module.nets.len() as NetId;
                let new_mux_idx = self.module.cells.len();
                self.module.nets.push(NetInfo {
                    driver: NetDriver::Undriven,
                    origin: None,
                });
                self.consumer_count.push(0);
                self.module.cells.push(Cell {
                    kind: CellKind::Mux2,
                    inputs: vec![s, left, right],
                    output: new_net,
                });
                self.module.nets[new_net as usize].driver = NetDriver::Cell(new_mux_idx);
                self.consumer_count[s as usize] += 1;
                self.consumer_count[left as usize] += 1;
                self.consumer_count[right as usize] += 1;

                self.rewrite_cell(cell_idx, op, vec![common, new_net]);
                did_any = true;
            }
            if !did_any {
                break;
            }
        }
    }

    /// Collapse Mux2 whose data inputs are constants or mirror the
    /// select signal into primitive two-input gates. See [`MuxRewrite`]
    /// for the full set of patterns. Non-const patterns that need a
    /// `!sel` materialise via [`Self::materialize_not`] — break-even at
    /// one use and profitable when the sel is shared.
    fn collapse_mux_to_primitive(&mut self) {
        // Collect matches first so mutating cells doesn't disturb iteration.
        let mut rewrites: Vec<(usize, MuxRewrite)> = Vec::new();
        for (i, c) in self.module.cells.iter().enumerate() {
            if c.kind != CellKind::Mux2 {
                continue;
            }
            let s = c.inputs[0];
            let d0 = c.inputs[1];
            let d1 = c.inputs[2];
            let d0_const = match self.module.nets[d0 as usize].driver {
                NetDriver::Const(b) => Some(b),
                _ => None,
            };
            let d1_const = match self.module.nets[d1 as usize].driver {
                NetDriver::Const(b) => Some(b),
                _ => None,
            };
            let not_of = |n: NetId| -> Option<NetId> {
                if let NetDriver::Cell(idx) = self.module.nets[n as usize].driver {
                    let cell = &self.module.cells[idx];
                    if cell.kind == CellKind::Not {
                        return Some(cell.inputs[0]);
                    }
                }
                None
            };
            // Patterns that need no extra Not come first (always a win).
            if d0_const == Some(false) && d1_const.is_none() {
                rewrites.push((i, MuxRewrite::AndSel));
            } else if d1_const == Some(true) && d0_const.is_none() {
                rewrites.push((i, MuxRewrite::OrSel));
            } else if d0 == s {
                rewrites.push((i, MuxRewrite::AndSelD1));
            } else if d1 == s {
                rewrites.push((i, MuxRewrite::OrSelD0));
            } else if d1_const == Some(false) && d0_const.is_none() {
                rewrites.push((i, MuxRewrite::AndNotSel));
            } else if d0_const == Some(true) && d1_const.is_none() {
                rewrites.push((i, MuxRewrite::OrNotSel));
            } else if d0_const.is_none() && not_of(d1) == Some(d0) {
                rewrites.push((i, MuxRewrite::XorSel));
            } else if d1_const.is_none() && not_of(d0) == Some(d1) {
                rewrites.push((i, MuxRewrite::XnorSel));
            }
        }

        for (cell_idx, kind) in rewrites {
            let s = self.module.cells[cell_idx].inputs[0];
            let d0 = self.module.cells[cell_idx].inputs[1];
            let d1 = self.module.cells[cell_idx].inputs[2];
            let (new_kind, new_inputs): (CellKind, Vec<NetId>) = match kind {
                MuxRewrite::AndSel | MuxRewrite::AndSelD1 => (CellKind::And2, vec![s, d1]),
                MuxRewrite::OrSel | MuxRewrite::OrSelD0 => (CellKind::Or2, vec![s, d0]),
                MuxRewrite::AndNotSel => {
                    let not_s = self.materialize_not(s);
                    (CellKind::And2, vec![not_s, d0])
                }
                MuxRewrite::OrNotSel => {
                    let not_s = self.materialize_not(s);
                    (CellKind::Or2, vec![not_s, d1])
                }
                MuxRewrite::XorSel => (CellKind::Xor2, vec![s, d0]),
                MuxRewrite::XnorSel => (CellKind::Xnor2, vec![s, d1]),
            };
            self.rewrite_cell(cell_idx, new_kind, new_inputs);
        }
    }

    /// Final fusion fixed-point: absorb primitive pairs into compound
    /// cells (Aoi21, Oai21, And3, Or3, Nand3, Nor3, and Not-polarity
    /// flips on 3-input gates). See [`try_complex_fuse`] for the
    /// pattern table.
    fn fuse_compound_gates(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;
            for cell_idx in 0..self.module.cells.len() {
                let Some((new_kind, new_inputs)) =
                    try_complex_fuse(self.module, &self.consumer_count, cell_idx)
                else {
                    continue;
                };
                self.rewrite_cell(cell_idx, new_kind, new_inputs);
                changed = true;
            }
        }
    }
}

/// Given two 2-input cells with inputs `(a0, a1)` and `(b0, b1)`, return
/// `(common, left_of_a, right_of_b)` if they share one input. Helper for
/// the distributivity / common-factor extraction phases.
fn find_common_input(a0: NetId, a1: NetId, b0: NetId, b1: NetId) -> Option<(NetId, NetId, NetId)> {
    if a0 == b0 {
        Some((a0, a1, b1))
    } else if a0 == b1 {
        Some((a0, a1, b0))
    } else if a1 == b0 {
        Some((a1, a0, b1))
    } else if a1 == b1 {
        Some((a1, a0, b0))
    } else {
        None
    }
}

fn try_complex_fuse(
    module: &GateModule,
    consumer_count: &[u32],
    cell_idx: usize,
) -> Option<(CellKind, Vec<NetId>)> {
    let cell = &module.cells[cell_idx];
    let kind = cell.kind;
    let inputs = &cell.inputs;

    if kind == CellKind::Not {
        let inp = inputs[0];
        let NetDriver::Cell(up_idx) = module.nets[inp as usize].driver else {
            return None;
        };
        if consumer_count[inp as usize] != 1 {
            return None;
        }
        let up = &module.cells[up_idx];
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
        return fused.map(|k| (k, up.inputs.clone()));
    }

    let rewrite = |outer: CellKind, inner: CellKind| -> Option<CellKind> {
        match (outer, inner) {
            (CellKind::Nor2, CellKind::And2) => Some(CellKind::Aoi21),
            (CellKind::Nand2, CellKind::Or2) => Some(CellKind::Oai21),
            (CellKind::And2, CellKind::And2) => Some(CellKind::And3),
            (CellKind::Or2, CellKind::Or2) => Some(CellKind::Or3),
            (CellKind::Nand2, CellKind::And2) => Some(CellKind::Nand3),
            (CellKind::Nor2, CellKind::Or2) => Some(CellKind::Nor3),
            _ => None,
        }
    };
    if !matches!(
        kind,
        CellKind::And2 | CellKind::Or2 | CellKind::Nand2 | CellKind::Nor2
    ) {
        return None;
    }
    for (pivot, other) in [(0usize, 1usize), (1, 0)] {
        let up_net = inputs[pivot];
        if consumer_count[up_net as usize] != 1 {
            continue;
        }
        let NetDriver::Cell(up_idx) = module.nets[up_net as usize].driver else {
            continue;
        };
        let up = &module.cells[up_idx];
        if let Some(new_kind) = rewrite(kind, up.kind) {
            return Some((new_kind, vec![up.inputs[0], up.inputs[1], inputs[other]]));
        }
    }

    None
}
