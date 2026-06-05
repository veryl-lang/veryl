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
}

#[derive(Clone, Debug, Default)]
pub struct DerivedClockSchedule {
    pub clocks: Vec<DerivedClock>,
    /// Input clocks toggled 0→1 in `step()` so gated-clock expressions
    /// see a rising edge.  Boundary inputs of the dependency closure
    /// that match a top-module clock-typed variable — either an input
    /// port or a testbench `$tb::clock_gen` inst output.
    pub master_input_clocks: SmallVec<[VarId; 4]>,
}

impl DerivedClockSchedule {
    pub fn is_empty(&self) -> bool {
        self.clocks.is_empty()
    }
}

/// Returns `(schedule, eval_indices)` where `eval_indices` are
/// dependency-closure stmt indices into `pre_jit_stmts` (already
/// topo-sorted by `analyze_dependency`).
pub fn build_schedule(
    derived_clock_vars: &[(VarId, VarOffset, usize)],
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

    let mut clocks: Vec<DerivedClock> = derived_clock_vars
        .iter()
        .filter(|(_, off, _)| {
            // Skip clocks with no writer: testbench-driven (e.g. `inst
            // clk: $tb::clock_gen`) clocks have edges supplied directly
            // by the testbench, so monitoring them would just push the
            // module onto `step_with_derived_clocks` for nothing.
            // FF-storage clocks always pass; their writer is the
            // always_ff stmt, which `output_to_writer` doesn't track.
            off.is_ff() || output_to_writer.contains_key(off)
        })
        .map(|(var_id, off, nb)| DerivedClock {
            var_id: *var_id,
            current_offset: *off,
            native_bytes: *nb,
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
    for clk in &clocks {
        if clk.current_offset.is_ff() {
            continue;
        }
        collect_comb_closure(
            clk.current_offset,
            pre_jit_stmts,
            &output_to_writer,
            input_clock_offsets,
            &mut dep_set,
            &mut master_set,
        );
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
