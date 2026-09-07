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
    /// Declared `clock_negedge`: the active edge is the net's FALL, so the
    /// monitored bit is read inverted and every 0→1 test below means "fell".
    pub negedge: bool,
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

/// A net to monitor: `(var, offset, native bytes, polarity, negedge)`, where
/// the polarity is `None` for a clock and `Some(active_low)` for an async
/// reset, and `negedge` marks a `clock_negedge` (meaningless for a reset).
pub type EdgeCandidate = (VarId, VarOffset, usize, Option<bool>, bool);

/// Statement indices that write one offset, ascending.  A single writer is
/// the ordinary case; sibling conditional arms are what make it more.
type Writers = SmallVec<[usize; 1]>;

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

/// Returns `(schedule, eval_indices, master_indices)` where `eval_indices`
/// are dependency-closure stmt indices into `pre_jit_stmts` (already
/// topo-sorted by `analyze_dependency`) and `master_indices` the subset a
/// master input clock alone can change (see `master_downstream`).
pub fn build_schedule(
    candidates: &[EdgeCandidate],
    pre_jit_stmts: &[ProtoStatement],
    input_clock_offsets: &HashMap<VarOffset, VarId>,
) -> (DerivedClockSchedule, Vec<usize>, Vec<usize>) {
    // FF outputs go through the event/commit path.  Every writer, not the
    // last: an offset written by sibling conditional arms has one fragment
    // per arm, and keeping only the last leaves the extracted subsequence
    // with a read whose producer runs later.  The schedule then asks for
    // settle passes it cannot justify — measured 3, and 1 once the closure
    // is complete.
    let mut output_to_writer: HashMap<VarOffset, Writers> = HashMap::default();
    let mut scratch_in: Vec<VarOffset> = Vec::new();
    let mut scratch_out: Vec<VarOffset> = Vec::new();
    for (i, stmt) in pre_jit_stmts.iter().enumerate() {
        scratch_in.clear();
        scratch_out.clear();
        stmt.gather_variable_offsets(&mut scratch_in, &mut scratch_out);
        for off in &scratch_out {
            if !off.is_ff() {
                // A statement holding sibling arms gathers its output once per
                // arm, and only WHICH statements to run matters below.  Its
                // pushes are adjacent, so the tail is enough to dedupe.
                let writers = output_to_writer.entry(*off).or_default();
                if writers.last() != Some(&i) {
                    writers.push(i);
                }
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
        .filter(|(_, off, _, polarity, _)| polarity.is_none() && driven(off))
        .map(|(var_id, off, nb, _, negedge)| DerivedClock {
            var_id: *var_id,
            current_offset: *off,
            native_bytes: *nb,
            master_gated: false,
            negedge: *negedge,
        })
        .collect();

    let resets: Vec<DerivedReset> = candidates
        .iter()
        .filter_map(|(var_id, off, nb, polarity, _)| {
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
            // The offset only holds its settled value once every fragment
            // that can write it has run.
            let writer = output_to_writer
                .get(&c.current_offset)
                .and_then(|w| w.last().copied())
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
    absorb_forwarders(&mut eval_indices, pre_jit_stmts);
    eval_indices.sort_unstable();
    let master_indices = master_downstream(&eval_indices, pre_jit_stmts, input_clock_offsets);

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
        master_indices,
    )
}

/// Extend the closure with the plain comb assigns outside it that read only
/// what it already keeps current (its inputs and outputs, transitively).
/// Left outside, such a statement (a clock net copied onto a port nobody
/// reads, say) is a reader of the closure's inputs that the settle filter
/// must honour, so every change of those inputs forces a full settle;
/// evaluated with the closure it is refreshed at the closure's cost.  Bounded
/// so the closure stays small.
fn absorb_forwarders(eval_indices: &mut Vec<usize>, pre_jit_stmts: &[ProtoStatement]) {
    const CAP: usize = 64;
    let mut in_closure: HashSet<usize> = eval_indices.iter().copied().collect();
    let mut visible: HashSet<VarOffset> = HashSet::default();
    let (mut ins, mut outs) = (Vec::new(), Vec::new());
    for &i in eval_indices.iter() {
        ins.clear();
        outs.clear();
        pre_jit_stmts[i].gather_variable_offsets(&mut ins, &mut outs);
        visible.extend(ins.iter().copied());
        visible.extend(outs.iter().copied());
    }
    let mut absorbed = 0usize;
    let mut grew = true;
    while grew && absorbed < CAP {
        grew = false;
        for (i, stmt) in pre_jit_stmts.iter().enumerate() {
            if in_closure.contains(&i)
                || !matches!(stmt, ProtoStatement::Assign(a) if !a.dst.is_ff())
            {
                continue;
            }
            ins.clear();
            outs.clear();
            stmt.gather_variable_offsets(&mut ins, &mut outs);
            if ins.is_empty() || !ins.iter().all(|o| visible.contains(o)) {
                continue;
            }
            in_closure.insert(i);
            eval_indices.push(i);
            visible.extend(outs.iter().copied());
            absorbed += 1;
            grew = true;
            if absorbed == CAP {
                break;
            }
        }
    }
}

/// The closure statements a master input clock can change: those reading a
/// master, plus everything downstream of them within `eval_indices`.  With the
/// rest of the design settled, toggling the master alone leaves every other
/// closure statement's value in place.  Sorted like `eval_indices`.
pub fn master_downstream(
    eval_indices: &[usize],
    pre_jit_stmts: &[ProtoStatement],
    input_clock_offsets: &HashMap<VarOffset, VarId>,
) -> Vec<usize> {
    let mut readers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    let mut outs_of: HashMap<usize, Vec<VarOffset>> = HashMap::default();
    let mut seeds: Vec<usize> = Vec::new();
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    for &i in eval_indices {
        let Some(stmt) = pre_jit_stmts.get(i) else {
            continue;
        };
        ins.clear();
        outs.clear();
        stmt.gather_variable_offsets(&mut ins, &mut outs);
        if ins.iter().any(|o| input_clock_offsets.contains_key(o)) {
            seeds.push(i);
        }
        for o in &ins {
            readers.entry(*o).or_default().push(i);
        }
        outs_of.insert(i, outs.clone());
    }
    let mut set: HashSet<usize> = HashSet::default();
    let mut stack = seeds;
    while let Some(i) = stack.pop() {
        if !set.insert(i) {
            continue;
        }
        for o in outs_of.get(&i).into_iter().flatten() {
            for &r in readers.get(o).into_iter().flatten() {
                if !set.contains(&r) {
                    stack.push(r);
                }
            }
        }
    }
    let mut v: Vec<usize> = set.into_iter().collect();
    v.sort_unstable();
    v
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

/// Walks back from `target_offset` through `output_to_writer`.  FF inputs
/// are leaves; boundary clock-typed inputs (top-module ports or testbench
/// inst outputs) are recorded as masters.
fn collect_comb_closure(
    target_offset: VarOffset,
    pre_jit_stmts: &[ProtoStatement],
    output_to_writer: &HashMap<VarOffset, Writers>,
    input_clock_offsets: &HashMap<VarOffset, VarId>,
    dep_set: &mut HashSet<usize>,
    master_set: &mut HashSet<VarId>,
) {
    let mut scratch_in: Vec<VarOffset> = Vec::new();
    let mut scratch_out: Vec<VarOffset> = Vec::new();
    let mut stack: Vec<usize> = match output_to_writer.get(&target_offset) {
        Some(writers) => writers.to_vec(),
        None => return,
    };
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
                Some(writers) => {
                    for &writer in writers {
                        if !dep_set.contains(&writer) {
                            stack.push(writer);
                        }
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
