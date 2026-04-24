//! Sensitivity-fanout + topological-rank index for an event-driven
//! comb settle path (opt-in via `Config::use_seeded_worklist`).
//!
//! The runtime walks a worklist of `StmtId`s seeded from the set of
//! offsets whose pending entries just drained. For each entry popped off
//! the worklist it evaluates the statement and any of its outputs that
//! materially changed are pushed onto the worklist as the next generation
//! of "dirty" offsets. This module defines the build-time index that
//! enables that lookup:
//!
//! * `stmt_inputs[i]` / `stmt_outputs[i]` — what statement `i` reads
//!   and writes, by `VarOffset`.
//! * `output_to_readers[off]` — inverse map: when `off` changes, these
//!   stmt indices are candidates for re-evaluation.
//! * `topo_rank[i]` — lower ranks evaluate first; used as the worklist
//!   ordering key.

use crate::HashMap;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::{ModuleVariableMeta, VarOffset, value_size};
use smallvec::SmallVec;

pub type StmtId = u32;

#[derive(Default, Debug, Clone)]
pub struct IrSchedule {
    /// Total number of `comb_statements` this schedule describes.
    pub n_stmts: u32,
    /// Per-stmt input offsets (variable reads).
    pub stmt_inputs: Vec<SmallVec<[VarOffset; 4]>>,
    /// Per-stmt output offsets (variable writes).
    pub stmt_outputs: Vec<SmallVec<[VarOffset; 4]>>,
    /// Inverse fanout: `output_to_readers[off]` lists every `StmtId`
    /// whose `stmt_inputs` contains `off`.
    pub output_to_readers: HashMap<VarOffset, SmallVec<[StmtId; 4]>>,
    /// Topological rank per stmt. Lower = earlier in the DAG.
    pub topo_rank: Vec<StmtId>,
    /// Byte size of the value slot at each known `VarOffset` (in either the
    /// ff_values or comb_values buffer depending on tag). Populated by
    /// `attach_offset_sizes` from the module's `ModuleVariableMeta`.
    /// Offsets not present here are treated as zero-sized (the worklist
    /// will ignore them for change detection).
    pub offset_sizes: HashMap<VarOffset, u32>,
}

impl IrSchedule {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build an inverse fanout map from already-populated `stmt_inputs`.
    /// Exposed independently of the build path so unit tests can validate
    /// the index without going through full ProtoStatement conversion.
    pub fn rebuild_fanout(&mut self) {
        self.output_to_readers.clear();
        for (sid, inputs) in self.stmt_inputs.iter().enumerate() {
            for off in inputs {
                self.output_to_readers
                    .entry(*off)
                    .or_default()
                    .push(sid as StmtId);
            }
        }
    }

    /// Reset all internal state. Primarily for re-use across cached Ir
    /// instances.
    pub fn clear(&mut self) {
        self.n_stmts = 0;
        self.stmt_inputs.clear();
        self.stmt_outputs.clear();
        self.output_to_readers.clear();
        self.topo_rank.clear();
        self.offset_sizes.clear();
    }

    /// Given the byte state of `comb_values` before and after evaluating
    /// a set of stmts, collect every reader stmt whose input offset's
    /// byte-range changed between `before` and `after`. The caller
    /// supplies the stmt IDs it just evaluated (so we only diff their
    /// declared outputs). Offsets missing from `offset_sizes` or whose
    /// size overflows the buffer are skipped conservatively. FF outputs
    /// are ignored by this routine — those writes go through the
    /// ff_commit path, not the comb worklist.
    pub fn compute_dirty_from_diff(
        &self,
        before: &[u8],
        after: &[u8],
        stmt_ids: impl IntoIterator<Item = StmtId>,
        out: &mut SmallVec<[StmtId; 32]>,
    ) {
        use crate::HashSet;
        let mut seen: HashSet<StmtId> = HashSet::default();
        for sid in stmt_ids {
            let outs = match self.stmt_outputs.get(sid as usize) {
                Some(o) => o,
                None => continue,
            };
            for off in outs {
                if off.is_ff() {
                    continue;
                }
                let size = match self.offset_sizes.get(off) {
                    Some(&s) => s as usize,
                    None => continue,
                };
                let raw = off.raw();
                if raw < 0 {
                    continue;
                }
                let o = raw as usize;
                if o + size > before.len() || o + size > after.len() {
                    continue;
                }
                if before[o..o + size] != after[o..o + size]
                    && let Some(readers) = self.output_to_readers.get(off)
                {
                    for &r in readers {
                        if seen.insert(r) {
                            out.push(r);
                        }
                    }
                }
            }
        }
    }

    /// Populate `offset_sizes` by walking a `ModuleVariableMeta` tree.
    /// Each `VariableElement::current` offset maps to its value-slot byte
    /// size; FF `next_offset` entries are also recorded under
    /// `VarOffset::Ff(next_offset)` so writes to the FF next-buffer can
    /// be diffed by the worklist. Called from `ProtoModule::from_source`
    /// after `build_from_proto`.
    pub fn attach_offset_sizes(&mut self, meta: &ModuleVariableMeta, use_4state: bool) {
        self.offset_sizes.clear();
        collect_offset_sizes_recursive(meta, use_4state, &mut self.offset_sizes);
    }

    /// Build a schedule from a topologically-sorted slice of `ProtoStatement`s.
    ///
    /// The statements are assumed to be ordered by `analyze_dependency` +
    /// `reorder_by_level`, so `topo_rank[i] = i`. The fanout index is
    /// populated from each statement's declared input offsets; statements
    /// with no measurable I/O (e.g. `ProtoStatement::Break`, system calls
    /// with only side effects) contribute empty slots but still occupy an
    /// index so the `StmtId`-space matches the `comb_statements` order.
    pub fn build_from_proto_with_meta(
        stmts: &[ProtoStatement],
        meta: &ModuleVariableMeta,
        use_4state: bool,
    ) -> Self {
        let mut sched = Self::build_from_proto(stmts);
        sched.attach_offset_sizes(meta, use_4state);
        sched
    }

    pub fn build_from_proto(stmts: &[ProtoStatement]) -> Self {
        let n = stmts.len();
        let mut sched = IrSchedule {
            n_stmts: n as u32,
            stmt_inputs: Vec::with_capacity(n),
            stmt_outputs: Vec::with_capacity(n),
            output_to_readers: HashMap::default(),
            topo_rank: (0..n as StmtId).collect(),
            offset_sizes: HashMap::default(),
        };

        let mut ins_scratch: Vec<VarOffset> = Vec::new();
        let mut outs_scratch: Vec<VarOffset> = Vec::new();
        for stmt in stmts {
            ins_scratch.clear();
            outs_scratch.clear();
            stmt.gather_variable_offsets(&mut ins_scratch, &mut outs_scratch);

            // Deduplicate while preserving order so SmallVec stays inline
            // (≤4 unique offsets is the common case for Assign / If bodies).
            let mut ins: SmallVec<[VarOffset; 4]> = SmallVec::new();
            for &off in &ins_scratch {
                if !ins.contains(&off) {
                    ins.push(off);
                }
            }
            let mut outs: SmallVec<[VarOffset; 4]> = SmallVec::new();
            for &off in &outs_scratch {
                if !outs.contains(&off) {
                    outs.push(off);
                }
            }
            sched.stmt_inputs.push(ins);
            sched.stmt_outputs.push(outs);
        }

        sched.rebuild_fanout();
        sched
    }
}

fn collect_offset_sizes_recursive(
    meta: &ModuleVariableMeta,
    use_4state: bool,
    out: &mut HashMap<VarOffset, u32>,
) {
    for var_meta in meta.variable_meta.values() {
        for element in &var_meta.elements {
            let vs = value_size(element.native_bytes, use_4state) as u32;
            out.insert(element.current, vs);
            if element.is_ff() {
                out.insert(VarOffset::Ff(element.next_offset), vs);
            }
        }
    }
    for child in &meta.children {
        collect_offset_sizes_recursive(child, use_4state, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn off_comb(v: isize) -> VarOffset {
        VarOffset::Comb(v)
    }

    #[test]
    fn empty_schedule_has_zero_stmts() {
        let s = IrSchedule::empty();
        assert_eq!(s.n_stmts, 0);
        assert!(s.stmt_inputs.is_empty());
        assert!(s.stmt_outputs.is_empty());
        assert!(s.output_to_readers.is_empty());
        assert!(s.topo_rank.is_empty());
    }

    #[test]
    fn rebuild_fanout_from_inputs() {
        // 3 stmts: stmt 0 reads off=0, stmt 1 reads off=0 and 8, stmt 2 reads 8.
        let mut s = IrSchedule::empty();
        s.n_stmts = 3;
        s.stmt_inputs = vec![
            SmallVec::from_slice(&[off_comb(0)]),
            SmallVec::from_slice(&[off_comb(0), off_comb(8)]),
            SmallVec::from_slice(&[off_comb(8)]),
        ];
        s.stmt_outputs = vec![SmallVec::new(); 3];
        s.topo_rank = vec![0, 1, 2];

        s.rebuild_fanout();

        let r0 = s.output_to_readers.get(&off_comb(0)).unwrap();
        assert_eq!(r0.as_slice(), &[0u32, 1]);
        let r8 = s.output_to_readers.get(&off_comb(8)).unwrap();
        assert_eq!(r8.as_slice(), &[1u32, 2]);
    }

    #[test]
    fn build_from_proto_captures_assign_io() {
        use crate::ir::expression::ExpressionContext;
        use crate::ir::statement::ProtoAssignStatement;
        use crate::ir::{ProtoExpression, ProtoStatement, VarOffset};
        use veryl_parser::token_range::TokenRange;

        // Build 3 ProtoStatement::Assigns that form a linear comb chain:
        //   s0: comb(0) <= comb(8)
        //   s1: comb(4) <= comb(0) op comb(8)
        //   s2: comb(12) <= comb(4)
        // Expected fanout:
        //   off 8  -> [0, 1]
        //   off 0  -> [1]
        //   off 4  -> [2]
        let mk_var = |off: isize| ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: None,
            dynamic_select: None,
            width: 32,
            var_full_width: 32,
            expr_context: ExpressionContext {
                width: 32,
                signed: false,
            },
        };
        let mk_bin = |a: ProtoExpression, b: ProtoExpression| ProtoExpression::Binary {
            x: Box::new(a),
            op: crate::ir::Op::Add,
            y: Box::new(b),
            width: 32,
            expr_context: ExpressionContext {
                width: 32,
                signed: false,
            },
        };
        let mk_stmt = |dst_off: isize, rhs: ProtoExpression| {
            ProtoStatement::Assign(ProtoAssignStatement {
                dst: VarOffset::Comb(dst_off),
                dst_width: 32,
                select: None,
                dynamic_select: None,
                rhs_select: None,
                expr: rhs,
                dst_ff_current_offset: 0,
                token: TokenRange::default(),
            })
        };

        let protos = vec![
            mk_stmt(0, mk_var(8)),
            mk_stmt(4, mk_bin(mk_var(0), mk_var(8))),
            mk_stmt(12, mk_var(4)),
        ];

        let sched = IrSchedule::build_from_proto(&protos);
        assert_eq!(sched.n_stmts, 3);
        assert_eq!(sched.stmt_inputs.len(), 3);
        assert_eq!(sched.stmt_outputs.len(), 3);
        assert_eq!(sched.topo_rank, vec![0u32, 1, 2]);

        assert_eq!(sched.stmt_inputs[0].as_slice(), &[off_comb(8)]);
        assert_eq!(sched.stmt_outputs[0].as_slice(), &[off_comb(0)]);
        assert_eq!(sched.stmt_inputs[1].as_slice(), &[off_comb(0), off_comb(8)]);
        assert_eq!(sched.stmt_outputs[1].as_slice(), &[off_comb(4)]);
        assert_eq!(sched.stmt_inputs[2].as_slice(), &[off_comb(4)]);
        assert_eq!(sched.stmt_outputs[2].as_slice(), &[off_comb(12)]);

        let r0 = sched.output_to_readers.get(&off_comb(0)).unwrap();
        assert_eq!(r0.as_slice(), &[1u32]);
        let r4 = sched.output_to_readers.get(&off_comb(4)).unwrap();
        assert_eq!(r4.as_slice(), &[2u32]);
        let r8 = sched.output_to_readers.get(&off_comb(8)).unwrap();
        assert_eq!(r8.as_slice(), &[0u32, 1]);
        assert!(!sched.output_to_readers.contains_key(&off_comb(12)));
    }

    #[test]
    fn build_from_proto_dedup_inputs() {
        use crate::ir::expression::ExpressionContext;
        use crate::ir::statement::ProtoAssignStatement;
        use crate::ir::{ProtoExpression, ProtoStatement, VarOffset};
        use veryl_parser::token_range::TokenRange;

        // Same offset read twice (self-add) must appear once in the dedup set.
        let var = |off: isize| ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: None,
            dynamic_select: None,
            width: 32,
            var_full_width: 32,
            expr_context: ExpressionContext {
                width: 32,
                signed: false,
            },
        };
        let stmt = ProtoStatement::Assign(ProtoAssignStatement {
            dst: VarOffset::Comb(16),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: ProtoExpression::Binary {
                x: Box::new(var(8)),
                op: crate::ir::Op::Add,
                y: Box::new(var(8)),
                width: 32,
                expr_context: ExpressionContext {
                    width: 32,
                    signed: false,
                },
            },
            dst_ff_current_offset: 0,
            token: TokenRange::default(),
        });

        let sched = IrSchedule::build_from_proto(&[stmt]);
        assert_eq!(sched.stmt_inputs[0].as_slice(), &[off_comb(8)]);
        let r8 = sched.output_to_readers.get(&off_comb(8)).unwrap();
        assert_eq!(r8.as_slice(), &[0u32]);
    }

    #[test]
    fn attach_offset_sizes_walks_children() {
        use crate::HashMap as FxMap;
        use crate::ir::variable::{ModuleVariableMeta, VariableElement, VariableMeta};
        use veryl_analyzer::ir::{Type, VarId, VarPath};
        use veryl_parser::resource_table::insert_str;

        let ty = Type::default();

        // Parent module: one comb (width=32, 4 bytes) at comb=0 and one FF
        // (width=64, 8 bytes) with current at ff=0 and next at ff=8.
        let parent_path = VarPath(vec![insert_str("parent")]);
        let mut parent_vars = FxMap::default();
        let mut parent_id = VarId::default();
        parent_vars.insert(
            parent_id,
            VariableMeta {
                path: parent_path.clone(),
                r#type: ty.clone(),
                width: 32,
                native_bytes: 4,
                elements: vec![VariableElement {
                    native_bytes: 4,
                    current: VarOffset::Comb(0),
                    next_offset: 0,
                }],
                initial_values: vec![],
            },
        );
        parent_id.inc();
        parent_vars.insert(
            parent_id,
            VariableMeta {
                path: parent_path.clone(),
                r#type: ty.clone(),
                width: 64,
                native_bytes: 8,
                elements: vec![VariableElement {
                    native_bytes: 8,
                    current: VarOffset::Ff(0),
                    next_offset: 8,
                }],
                initial_values: vec![],
            },
        );

        // Child module: one comb (width=128, 16 bytes) at comb=32.
        let child_path = VarPath(vec![insert_str("child")]);
        let mut child_vars = FxMap::default();
        let child_id = VarId::default();
        child_vars.insert(
            child_id,
            VariableMeta {
                path: child_path,
                r#type: ty,
                width: 128,
                native_bytes: 16,
                elements: vec![VariableElement {
                    native_bytes: 16,
                    current: VarOffset::Comb(32),
                    next_offset: 0,
                }],
                initial_values: vec![],
            },
        );

        let meta = ModuleVariableMeta {
            name: insert_str("top"),
            variable_meta: parent_vars,
            children: vec![ModuleVariableMeta {
                name: insert_str("child"),
                variable_meta: child_vars,
                children: vec![],
            }],
        };

        // 2-state: value_size == native_bytes
        let mut sched = IrSchedule::empty();
        sched.attach_offset_sizes(&meta, false);
        assert_eq!(sched.offset_sizes.get(&VarOffset::Comb(0)), Some(&4));
        assert_eq!(sched.offset_sizes.get(&VarOffset::Ff(0)), Some(&8));
        assert_eq!(sched.offset_sizes.get(&VarOffset::Ff(8)), Some(&8)); // FF next
        assert_eq!(sched.offset_sizes.get(&VarOffset::Comb(32)), Some(&16));

        // 4-state: value_size doubles
        sched.attach_offset_sizes(&meta, true);
        assert_eq!(sched.offset_sizes.get(&VarOffset::Comb(0)), Some(&8));
        assert_eq!(sched.offset_sizes.get(&VarOffset::Ff(0)), Some(&16));
        assert_eq!(sched.offset_sizes.get(&VarOffset::Ff(8)), Some(&16));
        assert_eq!(sched.offset_sizes.get(&VarOffset::Comb(32)), Some(&32));
    }

    #[test]
    fn clear_resets_offset_sizes() {
        let mut s = IrSchedule::empty();
        s.offset_sizes.insert(off_comb(0), 4);
        s.offset_sizes.insert(off_comb(8), 8);
        assert_eq!(s.offset_sizes.len(), 2);
        s.clear();
        assert!(s.offset_sizes.is_empty());
    }

    #[test]
    fn compute_dirty_from_diff_basic() {
        // 3 stmts:
        //   s0 writes off=0 (4 bytes), reads nothing
        //   s1 writes off=8 (4 bytes), reads off=0
        //   s2 writes off=16 (4 bytes), reads off=8
        let mut s = IrSchedule::empty();
        s.n_stmts = 3;
        s.stmt_inputs = vec![
            SmallVec::new(),
            SmallVec::from_slice(&[off_comb(0)]),
            SmallVec::from_slice(&[off_comb(8)]),
        ];
        s.stmt_outputs = vec![
            SmallVec::from_slice(&[off_comb(0)]),
            SmallVec::from_slice(&[off_comb(8)]),
            SmallVec::from_slice(&[off_comb(16)]),
        ];
        s.topo_rank = vec![0, 1, 2];
        s.offset_sizes.insert(off_comb(0), 4);
        s.offset_sizes.insert(off_comb(8), 4);
        s.offset_sizes.insert(off_comb(16), 4);
        s.rebuild_fanout();

        let before = vec![0u8; 24];
        let mut after = vec![0u8; 24];
        // Flip bytes at off=0 (s0's output) — s1 (reader of off=0) should be dirty.
        after[0] = 0xab;

        let mut dirty: SmallVec<[StmtId; 32]> = SmallVec::new();
        s.compute_dirty_from_diff(&before, &after, 0..3, &mut dirty);
        assert_eq!(dirty.as_slice(), &[1u32]);

        // Second case: change at off=8 (s1's output) → s2 dirty.
        let mut after2 = before.clone();
        after2[8] = 0xcd;
        let mut dirty2: SmallVec<[StmtId; 32]> = SmallVec::new();
        s.compute_dirty_from_diff(&before, &after2, 0..3, &mut dirty2);
        assert_eq!(dirty2.as_slice(), &[2u32]);
    }

    #[test]
    fn compute_dirty_from_diff_skips_ff_and_unsized() {
        let mut s = IrSchedule::empty();
        s.n_stmts = 1;
        s.stmt_inputs = vec![SmallVec::new()];
        s.stmt_outputs = vec![SmallVec::from_slice(&[
            VarOffset::Ff(0), // skipped: FF
            off_comb(100),    // skipped: no offset_sizes entry
        ])];
        s.topo_rank = vec![0];
        // Populate a fake reader for each just to ensure we don't fire
        s.output_to_readers
            .insert(VarOffset::Ff(0), SmallVec::from_slice(&[99u32]));
        s.output_to_readers
            .insert(off_comb(100), SmallVec::from_slice(&[88u32]));

        let before = vec![0u8; 8];
        let after = vec![0xff; 8];

        let mut dirty: SmallVec<[StmtId; 32]> = SmallVec::new();
        s.compute_dirty_from_diff(&before, &after, 0..1, &mut dirty);
        assert!(dirty.is_empty());
    }

    #[test]
    fn compute_dirty_from_diff_dedup_readers() {
        // Two stmts both writing off=0; with the same reader, the reader
        // should appear in dirty only once.
        let mut s = IrSchedule::empty();
        s.n_stmts = 3;
        s.stmt_inputs = vec![
            SmallVec::new(),
            SmallVec::new(),
            SmallVec::from_slice(&[off_comb(0)]),
        ];
        s.stmt_outputs = vec![
            SmallVec::from_slice(&[off_comb(0)]),
            SmallVec::from_slice(&[off_comb(0)]),
            SmallVec::new(),
        ];
        s.topo_rank = vec![0, 1, 2];
        s.offset_sizes.insert(off_comb(0), 4);
        s.rebuild_fanout();

        let before = vec![0u8; 8];
        let mut after = before.clone();
        after[0] = 1;

        let mut dirty: SmallVec<[StmtId; 32]> = SmallVec::new();
        // Both s0 and s1 supposedly wrote off=0 — only unique readers should be added.
        s.compute_dirty_from_diff(&before, &after, [0u32, 1], &mut dirty);
        assert_eq!(dirty.as_slice(), &[2u32]);
    }

    #[test]
    fn build_from_proto_empty_input() {
        let sched = IrSchedule::build_from_proto(&[]);
        assert_eq!(sched.n_stmts, 0);
        assert!(sched.stmt_inputs.is_empty());
        assert!(sched.stmt_outputs.is_empty());
        assert!(sched.topo_rank.is_empty());
        assert!(sched.output_to_readers.is_empty());
    }

    #[test]
    fn clear_resets_all_fields() {
        let mut s = IrSchedule::empty();
        s.n_stmts = 5;
        s.stmt_inputs = vec![SmallVec::from_slice(&[off_comb(0)])];
        s.stmt_outputs = vec![SmallVec::new()];
        s.topo_rank = vec![0];
        s.rebuild_fanout();
        assert!(!s.output_to_readers.is_empty());

        s.clear();

        assert_eq!(s.n_stmts, 0);
        assert!(s.stmt_inputs.is_empty());
        assert!(s.stmt_outputs.is_empty());
        assert!(s.output_to_readers.is_empty());
        assert!(s.topo_rank.is_empty());
    }
}
