//! Derived (gated / divided) clock support.
//!
//! A derived clock is a non-port `clock`-typed variable whose value is
//! produced inside the module — by a comb expression (`let clk_g: clock
//! = i_clk & i_en;`) or an `always_ff` write.  The simulator detects
//! 0→1 transitions after each `step()` and synthesizes
//! `Event::Clock(VarId)` for downstream `always_ff(derived_clk)`.
//!
//! Derived-clock values are refreshed by a dedicated
//! `derived_clock_eval` ProtoStatements chunk (dependency closure only),
//! JIT-compiled separately so the main comb JIT/AOT-C blob is untouched.
//!
//! A non-port ASYNC RESET the design produces itself rides the same
//! machinery: no testbench drives it, so its assertion is a transition of
//! an internal net just like a derived clock's rising edge, and firing
//! `Event::Reset(VarId)` there is what keeps `if_reset` from waiting for
//! the next clock edge.

use crate::HashMap;
use crate::HashSet;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;
use smallvec::SmallVec;
use veryl_analyzer::ir::VarId;

#[derive(Clone, Debug)]
pub struct DerivedClock {
    pub var_id: VarId,
    /// `is_ff()` selects between `ff_values` and `comb_values`.
    pub current_offset: VarOffset,
    /// Always 1 for a clock; carried for the `read_native_value` ABI.
    pub native_bytes: usize,
    /// The comb closure reaches a master input clock (`gclk = clk & en`):
    /// fires pre-commit with the master edge (ICG semantics) instead of
    /// in the post-commit loop.  See `step_with_derived_clocks`.
    pub master_gated: bool,
}

/// An internally produced async reset, monitored for its ASSERTION.
#[derive(Clone, Debug)]
pub struct DerivedReset {
    pub var_id: VarId,
    /// `is_ff()` selects between `ff_values` and `comb_values`.
    pub current_offset: VarOffset,
    /// Always 1 for a reset; carried for the `read_native_value` ABI.
    pub native_bytes: usize,
    /// The net asserts when it reads 0.
    pub active_low: bool,
}

/// A net to monitor: `(var, offset, native bytes, polarity)`, where the
/// polarity is `None` for a clock and `Some(active_low)` for an async reset.
pub type EdgeCandidate = (VarId, VarOffset, usize, Option<bool>);

#[derive(Clone, Debug, Default)]
pub struct DerivedClockSchedule {
    pub clocks: Vec<DerivedClock>,
    pub resets: Vec<DerivedReset>,
    /// Input clocks toggled 0→1 in `step()` so gated-clock expressions
    /// see a rising edge.  Boundary inputs of the dependency closure
    /// that match a top-module clock-typed variable — either an input
    /// port or a testbench `$tb::clock_gen` inst output.
    pub master_input_clocks: SmallVec<[VarId; 4]>,
}

impl DerivedClockSchedule {
    pub fn is_empty(&self) -> bool {
        self.clocks.is_empty() && self.resets.is_empty()
    }
}

/// Returns `(schedule, eval_indices)` where `eval_indices` are
/// dependency-closure stmt indices into `pre_jit_stmts` (already
/// topo-sorted by `analyze_dependency`).
pub fn build_schedule(
    candidates: &[EdgeCandidate],
    pre_jit_stmts: &[ProtoStatement],
    input_clock_offsets: &HashMap<VarOffset, VarId>,
) -> (DerivedClockSchedule, Vec<usize>) {
    // Comb-only reverse map: VarOffset -> writer stmt index.  FF outputs
    // go through the event/commit path so they're not tracked.
    let mut output_to_writer: HashMap<VarOffset, usize> = HashMap::default();
    let mut scratch_in: Vec<VarOffset> = Vec::new();
    let mut scratch_out: Vec<VarOffset> = Vec::new();
    for (i, stmt) in pre_jit_stmts.iter().enumerate() {
        scratch_in.clear();
        scratch_out.clear();
        stmt.gather_variable_offsets(&mut scratch_in, &mut scratch_out);
        for off in &scratch_out {
            if !off.is_ff() {
                output_to_writer.insert(*off, i);
            }
        }
    }

    // Skip nets with no writer: testbench-driven (e.g. `inst clk:
    // $tb::clock_gen`, `inst rst: $tb::reset_gen`) nets have their edges
    // supplied directly by the testbench, so monitoring them would just
    // push the module onto `step_with_derived_clocks` for nothing.
    // FF-storage nets always pass; their writer is the always_ff stmt,
    // which `output_to_writer` doesn't track.
    let driven = |off: &VarOffset| off.is_ff() || output_to_writer.contains_key(off);

    let mut clocks: Vec<DerivedClock> = candidates
        .iter()
        .filter(|(_, off, _, polarity)| polarity.is_none() && driven(off))
        .map(|(var_id, off, nb, _)| DerivedClock {
            var_id: *var_id,
            current_offset: *off,
            native_bytes: *nb,
            master_gated: false,
        })
        .collect();

    let resets: Vec<DerivedReset> = candidates
        .iter()
        .filter_map(|(var_id, off, nb, polarity)| {
            polarity
                .filter(|_| driven(off))
                .map(|active_low| DerivedReset {
                    var_id: *var_id,
                    current_offset: *off,
                    native_bytes: *nb,
                    active_low,
                })
        })
        .collect();

    // FF-derived first, then comb-derived in topo order — matches the
    // chain-fire fixpoint's natural firing order.
    clocks.sort_by_key(|c| {
        if c.current_offset.is_ff() {
            (0u32, 0u32)
        } else {
            let writer = output_to_writer
                .get(&c.current_offset)
                .copied()
                .unwrap_or(usize::MAX);
            (1u32, writer as u32)
        }
    });

    let mut dep_set: HashSet<usize> = HashSet::default();
    let mut master_set: HashSet<VarId> = HashSet::default();
    for clk in &mut clocks {
        if clk.current_offset.is_ff() {
            continue;
        }
        // Per-clock closure walk so each clock learns whether ITS
        // expression reaches a master input (→ ICG semantics).
        let mut local_dep: HashSet<usize> = HashSet::default();
        let mut local_master: HashSet<VarId> = HashSet::default();
        collect_comb_closure(
            clk.current_offset,
            pre_jit_stmts,
            &output_to_writer,
            input_clock_offsets,
            &mut local_dep,
            &mut local_master,
        );
        clk.master_gated = !local_master.is_empty();
        dep_set.extend(local_dep);
        master_set.extend(local_master);
    }
    for rst in &resets {
        if rst.current_offset.is_ff() {
            continue;
        }
        // Same closure, for `partial_settle` only: a reset has no ICG
        // reading, so its master inputs stay out of `master_input_clocks`
        // (toggling one would fabricate a clock edge nothing asked for).
        let mut local_dep: HashSet<usize> = HashSet::default();
        let mut local_master: HashSet<VarId> = HashSet::default();
        collect_comb_closure(
            rst.current_offset,
            pre_jit_stmts,
            &output_to_writer,
            input_clock_offsets,
            &mut local_dep,
            &mut local_master,
        );
        dep_set.extend(local_dep);
    }

    // Sort by pre_jit_stmts index so partial_settle runs deps first.
    let mut eval_indices: Vec<usize> = dep_set.into_iter().collect();
    eval_indices.sort_unstable();

    let mut master_input_clocks: SmallVec<[VarId; 4]> = SmallVec::new();
    for vid in master_set {
        master_input_clocks.push(vid);
    }

    (
        DerivedClockSchedule {
            clocks,
            resets,
            master_input_clocks,
        },
        eval_indices,
    )
}

pub fn extract_eval_proto_stmts(
    eval_indices: &[usize],
    pre_jit_stmts: &[ProtoStatement],
) -> Vec<ProtoStatement> {
    eval_indices
        .iter()
        .filter_map(|i| pre_jit_stmts.get(*i).cloned())
        .collect()
}

/// BFS back from `target_offset` through `output_to_writer`.  FF inputs
/// are leaves; boundary clock-typed inputs (top-module ports or testbench
/// inst outputs) are recorded as masters.
fn collect_comb_closure(
    target_offset: VarOffset,
    pre_jit_stmts: &[ProtoStatement],
    output_to_writer: &HashMap<VarOffset, usize>,
    input_clock_offsets: &HashMap<VarOffset, VarId>,
    dep_set: &mut HashSet<usize>,
    master_set: &mut HashSet<VarId>,
) {
    let start = match output_to_writer.get(&target_offset) {
        Some(&idx) => idx,
        None => return,
    };

    let mut scratch_in: Vec<VarOffset> = Vec::new();
    let mut scratch_out: Vec<VarOffset> = Vec::new();
    let mut stack: Vec<usize> = vec![start];
    while let Some(idx) = stack.pop() {
        if !dep_set.insert(idx) {
            continue;
        }
        let stmt = match pre_jit_stmts.get(idx) {
            Some(s) => s,
            None => continue,
        };
        scratch_in.clear();
        scratch_out.clear();
        stmt.gather_variable_offsets(&mut scratch_in, &mut scratch_out);
        for off in &scratch_in {
            if off.is_ff() {
                continue;
            }
            match output_to_writer.get(off) {
                Some(&writer) => {
                    if !dep_set.contains(&writer) {
                        stack.push(writer);
                    }
                }
                None => {
                    if let Some(&vid) = input_clock_offsets.get(off) {
                        master_set.insert(vid);
                    }
                }
            }
        }
    }
}
