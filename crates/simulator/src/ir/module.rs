use crate::backend::inst::next_test_top_id;
use crate::backend::{ChunkOutput, CompileCtx, CompiledWhole, whole};
use crate::ir::big_array::BigArrayFold;
use crate::ir::comb_layout;
use crate::ir::comb_pipeline_cache;
use crate::ir::context::{Context, Conv, ScopeContext};
use crate::ir::declaration::{stable_topo_sort, stable_topo_sort_with_blocks};
use crate::ir::derived_clock::{
    DerivedClockSchedule, build_schedule as build_derived_clock_schedule, extract_eval_proto_stmts,
};
use crate::ir::external::{ExternalComponentInst, ProtoExternalComponent};
use crate::ir::inst_layout::InstLayout;
use crate::ir::opt::comb_fusion;
use crate::ir::opt::cone_gate;
use crate::ir::opt::dead_var_dce;
use crate::ir::opt::dup_assign_dce::dce_aggressive;
use crate::ir::opt::multi_write_analysis::analyze_multi_write;
use crate::ir::opt::multi_write_analysis::collect_dyn_indexed_vars;
use crate::ir::opt::version_split;
use crate::ir::site_table::{SiteInfo, SiteKind, SiteTable};
#[cfg(not(target_family = "wasm"))]
use crate::ir::statement::CompiledBlockStatement;
use crate::ir::variable::{
    ModuleVariableMeta, ModuleVariables, VarOffset, Variable, VariableMeta, align_up_64,
    create_variable_meta, ff_cacheline_pad_enabled, value_size, write_native_value,
};
use crate::ir::{
    CompiledBatchStmt, Event, ProtoDeclaration, ProtoExpression, ProtoIfStatement, ProtoStatement,
    ProtoStatementBlock, ProtoStatements, Statement, VarId, VarPath,
};
use crate::simulator_error::SimulatorError;
use crate::{HashMap, HashSet};
use daggy::Dag;
use daggy::petgraph::Direction::Outgoing;
use daggy::petgraph::algo;
use std::collections::VecDeque;
use std::sync::Arc;
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;

pub struct Module {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_values: Box<[u8]>,
    pub comb_values: Box<[u8]>,
    pub module_variables: ModuleVariables,

    pub event_statements: HashMap<Event, Vec<Statement>>,
    /// Unified comb statements: all port connections, child comb, and internal
    /// comb combined into a single dependency-sorted list.
    pub comb_statements: Vec<Statement>,
    /// Number of eval_comb passes needed for full convergence.
    /// Pre-computed from backward edges in the sorted comb statement list.
    pub required_comb_passes: usize,
    /// FF write site table: compile-time metadata for each FF write site
    /// reachable from the pre-JIT event ProtoStatements.  Consumed for
    /// log buffer sizing and NBA invariant checks.
    pub site_table: SiteTable,
    /// Per-top-level-Inst FF byte range metadata.  Foundation for
    /// cache-line aligned padding between Inst FF blocks and per-Inst
    /// independent commit.
    pub inst_layout: InstLayout,
    /// Derived (gated / FF-divided) clocks in this module; empty when none.
    pub derived_clock_schedule: DerivedClockSchedule,
    /// JIT-compiled evaluation chunk for derived clocks; empty when none.
    pub derived_clock_eval_stmts: Vec<Statement>,
    /// Diagnostic: number of non-trivial strongly-connected components in
    /// the pre-JIT `unified_sorted` dataflow graph.  Real RTL combinational
    /// loops are rejected up-front by `analyze_dependency`, so any non-zero
    /// value here is a duplication artifact in the simulator IR assembly.
    /// Exposed for regression tests.
    pub nontrivial_comb_scc: usize,
    /// Whole-comb dispatch handle, populated when a backend committed
    /// to a one-function compile via `Backend::compile_whole_comb`.
    /// `None` keeps `settle_comb` on the per-chunk Cranelift loop.
    pub whole_comb: Option<Arc<dyn CompiledWhole>>,
    /// Per-event whole-event dispatch handles (today populated by AOT-C
    /// when `Config::aot_c_event` is set).  Empty when no backend
    /// covered the event.
    pub whole_events: HashMap<Event, Arc<dyn CompiledWhole>>,
    /// User-defined component instances (`$comp::<name>`), driven by
    /// the simulator around event evaluation.
    pub external_components: Vec<ExternalComponentInst>,
    /// Top-level variables written by RTL statements; component outputs
    /// must not overlap them (sole-driver check at load time).
    pub rtl_driven: crate::HashSet<air::VarId>,
    /// Comb offsets whose defs the fusion pass consumed
    /// (`VERYL_COMB_FUSION`): their storage is never written, so raw-buffer
    /// comparisons (the dual-run checker) must skip them.  Diagnostic only.
    pub fused_comb_offsets: Vec<isize>,
    /// Superset of every variable offset the comb list can touch
    /// (`collect_comb_touched_offsets`).  `Arc` so instantiating a `Module`
    /// per test does not deep-clone the whole set.  The testbench uses it to
    /// decide which of its own statements really invalidate the comb.
    pub comb_touched_offsets: Arc<HashSet<VarOffset>>,
    pub cone_segments: Vec<crate::ir::opt::cone_gate::RtSegment>,
}

pub struct ProtoModule {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_bytes: usize,
    pub comb_bytes: usize,
    pub use_4state: bool,
    pub module_variable_meta: ModuleVariableMeta,

    pub event_statements: HashMap<Event, ProtoStatements>,
    /// Unified comb statements: all port connections, child comb, and internal
    /// comb combined into a single dependency-sorted list.
    pub comb_statements: ProtoStatements,
    /// Number of eval_comb passes needed for full convergence.
    pub required_comb_passes: usize,
    /// See `Module::site_table`.
    pub site_table: SiteTable,
    /// See `Module::inst_layout`.
    pub inst_layout: InstLayout,
    /// See `Module::derived_clock_schedule`.
    pub derived_clock_schedule: DerivedClockSchedule,
    /// Pre-JIT form of `Module::derived_clock_eval_stmts`.
    pub derived_clock_eval: ProtoStatements,
    /// See `Module::nontrivial_comb_scc`.
    pub nontrivial_comb_scc: usize,
    /// See `Module::whole_comb`.  Built in `conv()` and shared
    /// (`Arc::clone`) with every `Module` produced by `instantiate()`.
    pub whole_comb: Option<Arc<dyn CompiledWhole>>,
    /// See `Module::whole_events`.  Built in `conv()`, shared
    /// (`Arc::clone`) with every `Module` from `instantiate()`.
    pub whole_events: HashMap<Event, Arc<dyn CompiledWhole>>,
    /// See `Module::external_components` (pre-pointer-binding form).
    pub external_components: Vec<ProtoExternalComponent>,
    /// See `Module::rtl_driven`.
    pub rtl_driven: crate::HashSet<air::VarId>,
    /// See `Module::fused_comb_offsets`.
    pub fused_comb_offsets: Vec<isize>,
    /// See `Module::comb_touched_offsets`.
    pub comb_touched_offsets: Arc<HashSet<VarOffset>>,
    /// Cone-gate segments in BLOCK space (`comb_statements.0` indices) with
    /// their state offsets assigned; `instantiate` maps them to the flat
    /// statement space.
    pub cone_segments: Vec<crate::ir::opt::cone_gate::ConeSegment>,
}

fn create_buffers(
    module_variable_meta: &ModuleVariableMeta,
    ff_bytes: usize,
    comb_bytes: usize,
    use_4state: bool,
) -> (Box<[u8]>, Box<[u8]>) {
    let mut ff_values = vec![0u8; ff_bytes];
    let mut comb_values = vec![0u8; comb_bytes];

    fill_buffers_recursive(
        module_variable_meta,
        &mut ff_values,
        &mut comb_values,
        use_4state,
    );

    (ff_values.into_boxed_slice(), comb_values.into_boxed_slice())
}

/// Fill byte buffers with initial values, writing at the offsets stored in VariableElement.
fn fill_buffers_recursive(
    module_meta: &ModuleVariableMeta,
    ff_values: &mut [u8],
    comb_values: &mut [u8],
    use_4state: bool,
) {
    let mut sorted: Vec<_> = module_meta.variable_meta.iter().collect();
    sorted.sort_by_key(|(k, _)| **k);

    for (_, meta) in &sorted {
        // Single-entry initial_values on a multi-element variable is the
        // compact template form used for large arrays.
        let template_mode = meta.initial_values.len() == 1 && meta.elements.len() > 1;
        for (i, element) in meta.elements.iter().enumerate() {
            let initial = if template_mode {
                &meta.initial_values[0]
            } else {
                match meta.initial_values.get(i) {
                    Some(v) => v,
                    None => continue,
                }
            };
            let nb = element.native_bytes;
            let _vs = value_size(nb, use_4state);
            if element.is_ff() {
                #[cfg(debug_assertions)]
                {
                    let off = element.current_offset() as usize;
                    debug_assert!(
                        off + _vs <= ff_values.len(),
                        "FF current_offset out of bounds"
                    );
                    debug_assert!(
                        element.next_offset as usize + _vs <= ff_values.len(),
                        "FF next_offset out of bounds"
                    );
                }
                let cur =
                    &mut ff_values[element.current_offset() as usize..] as *mut [u8] as *mut u8;
                let nxt = &mut ff_values[element.next_offset as usize..] as *mut [u8] as *mut u8;
                unsafe {
                    write_native_value(cur, nb, use_4state, initial);
                    write_native_value(nxt, nb, use_4state, initial);
                }
            } else {
                #[cfg(debug_assertions)]
                debug_assert!(
                    element.current_offset() as usize + _vs <= comb_values.len(),
                    "Comb current_offset out of bounds"
                );
                let cur =
                    &mut comb_values[element.current_offset() as usize..] as *mut [u8] as *mut u8;
                unsafe {
                    write_native_value(cur, nb, use_4state, initial);
                }
            }
        }
    }

    for child in &module_meta.children {
        fill_buffers_recursive(child, ff_values, comb_values, use_4state);
    }
}

fn create_variables_recursive(
    module_meta: &ModuleVariableMeta,
    ff_base: *mut u8,
    comb_base: *mut u8,
) -> ModuleVariables {
    let mut variables = HashMap::default();

    for (id, meta) in &module_meta.variable_meta {
        let mut current_values: Vec<*mut u8> = vec![];
        let mut next_values: Vec<*mut u8> = vec![];

        for element in &meta.elements {
            let current = unsafe {
                let base = if element.is_ff() { ff_base } else { comb_base };
                base.add(element.current_offset() as usize)
            };
            current_values.push(current);

            if element.is_ff() {
                let next = unsafe { ff_base.add(element.next_offset as usize) };
                next_values.push(next);
            }
        }

        variables.insert(
            *id,
            Variable {
                path: meta.path.clone(),
                r#type: meta.r#type.clone(),
                width: meta.width,
                native_bytes: meta.native_bytes,
                current_values,
                next_values,
            },
        );
    }

    let children = module_meta
        .children
        .iter()
        .map(|child| create_variables_recursive(child, ff_base, comb_base))
        .collect();

    ModuleVariables {
        name: module_meta.name,
        variables,
        children,
    }
}

impl ProtoModule {
    pub fn instantiate(&self) -> Module {
        log::trace!(
            "instantiate: module={}, ff_bytes={}, comb_bytes={}",
            self.name,
            self.ff_bytes,
            self.comb_bytes,
        );
        let (mut ff_values, mut comb_values) = create_buffers(
            &self.module_variable_meta,
            self.ff_bytes,
            self.comb_bytes,
            self.use_4state,
        );

        let ff_base = ff_values.as_mut_ptr();
        let comb_base = comb_values.as_mut_ptr();

        let module_variables =
            create_variables_recursive(&self.module_variable_meta, ff_base, comb_base);

        let ff_ptr = ff_values.as_mut_ptr();
        let comb_ptr = comb_values.as_mut_ptr();

        let ff_len = self.ff_bytes;
        let comb_len = self.comb_bytes;

        let event_statements = self
            .event_statements
            .iter()
            .map(|(event, stmts)| {
                let s = stmts.to_statements(ff_ptr, ff_len, comb_ptr, comb_len, self.use_4state);
                let s = batch_compiled_statements(s);
                (event.clone(), s)
            })
            .collect();

        // Temporary diagnostic: histogram the statements that stay
        // interpreted in the settle loop (VERYL_INTERP_DIAG=1).
        #[cfg(not(target_family = "wasm"))]
        if std::env::var("VERYL_INTERP_DIAG").ok().as_deref() == Some("1") {
            let mut hist: HashMap<String, usize> = HashMap::default();
            let interp = self
                .comb_statements
                .0
                .iter()
                .filter_map(|b| match b {
                    crate::ir::statement::ProtoStatementBlock::Interpreted(v) => Some(v),
                    _ => None,
                })
                .flatten();
            for s in interp {
                let kind = match s {
                    ProtoStatement::Assign(x) => {
                        let loc = x
                            .token
                            .beg
                            .source
                            .to_string()
                            .rsplit('/')
                            .next()
                            .map(|f| format!("{f}:{}", x.token.beg.line))
                            .unwrap_or_default();
                        format!(
                            "Assign jit={} w={} {loc}",
                            x.expr.can_build_binary(),
                            x.dst_width
                        )
                    }
                    ProtoStatement::AssignDynamic(_) => "AssignDynamic".to_string(),
                    ProtoStatement::If(_) => "If".to_string(),
                    ProtoStatement::Case(_) => "Case".to_string(),
                    ProtoStatement::For(_) => "For".to_string(),
                    ProtoStatement::SequentialBlock(_) => "SeqBlock".to_string(),
                    ProtoStatement::SystemFunctionCall(_) => "SysFn".to_string(),
                    ProtoStatement::TbMethodCall { .. } => "TbMethod".to_string(),
                    ProtoStatement::Break => "Break".to_string(),
                    ProtoStatement::CompiledBlock(_) => "NestedCB".to_string(),
                };
                *hist.entry(kind).or_insert(0) += 1;
            }
            let mut v: Vec<_> = hist.into_iter().collect();
            v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            for (k, c) in v.iter().take(30) {
                eprintln!("[InterpDiag] {c:5}  {k}");
            }
        }

        let comb_flat =
            self.comb_statements
                .to_statements(ff_ptr, ff_len, comb_ptr, comb_len, self.use_4state);
        // Cone-gate segments: map BLOCK ranges to FLAT statement ranges
        // (an Interpreted block flattens to its statement count, a Compiled
        // block to one entry).  Requires the unbatched flat list below.
        let cone_segments: Vec<crate::ir::opt::cone_gate::RtSegment> =
            if self.cone_segments.is_empty() {
                Vec::new()
            } else {
                let mut flat_at: Vec<usize> = Vec::with_capacity(self.comb_statements.0.len() + 1);
                let mut pos = 0usize;
                for b in &self.comb_statements.0 {
                    flat_at.push(pos);
                    pos += match b {
                        ProtoStatementBlock::Interpreted(v) => v.len(),
                        ProtoStatementBlock::Compiled(_) => 1,
                    };
                }
                flat_at.push(pos);
                self.cone_segments
                    .iter()
                    .filter(|s| s.block_lo < s.block_hi && s.block_hi < flat_at.len())
                    .map(|s| crate::ir::opt::cone_gate::RtSegment {
                        lo: flat_at[s.block_lo],
                        hi: flat_at[s.block_hi],
                        compare: s.compare.clone(),
                        backedge: s.backedge.clone(),
                        compare_pre: s.compare_pre.clone(),
                        replay: s.replay.clone(),
                        off_decay: s.off_decay,
                        cone: s.cone.clone(),
                    })
                    .collect()
            };
        // Batching merges consecutive same-artifact chunks into one statement,
        // which would break the 1:1 alignment gating needs — and a batched
        // chunk could not be skipped per instance anyway.  The cone gate needs
        // its segment edges intact for the same reason.
        let comb_statements = if cone_segments.is_empty() {
            batch_compiled_statements(comb_flat)
        } else {
            comb_flat
        };

        let derived_clock_eval_stmts = if self.derived_clock_eval.0.is_empty() {
            Vec::new()
        } else {
            batch_compiled_statements(self.derived_clock_eval.to_statements(
                ff_ptr,
                ff_len,
                comb_ptr,
                comb_len,
                self.use_4state,
            ))
        };

        #[cfg(debug_assertions)]
        self.validate_offsets();

        Module {
            name: self.name,
            ports: self.ports.clone(),
            ff_values,
            comb_values,
            module_variables,
            derived_clock_eval_stmts,

            event_statements,
            comb_statements,
            required_comb_passes: self.required_comb_passes,
            site_table: self.site_table.clone(),
            inst_layout: self.inst_layout.clone(),
            derived_clock_schedule: self.derived_clock_schedule.clone(),
            nontrivial_comb_scc: self.nontrivial_comb_scc,
            whole_comb: self.whole_comb.clone(),
            whole_events: self.whole_events.clone(),
            external_components: self
                .external_components
                .iter()
                .map(|x| unsafe {
                    x.instantiate(ff_ptr, ff_len, comb_ptr, comb_len, self.use_4state)
                })
                .collect(),
            rtl_driven: self.rtl_driven.clone(),
            fused_comb_offsets: self.fused_comb_offsets.clone(),
            comb_touched_offsets: Arc::clone(&self.comb_touched_offsets),
            cone_segments,
        }
    }

    /// Validate that all variable offsets in statements are within buffer bounds.
    #[cfg(debug_assertions)]
    fn validate_offsets(&self) {
        let ff_bytes = self.ff_bytes;
        let comb_bytes = self.comb_bytes;
        let use_4state = self.use_4state;

        let check = |off: &VarOffset, context: &str| {
            let raw = off.raw() as usize;
            if off.is_ff() {
                assert!(
                    raw < ff_bytes || ff_bytes == 0,
                    "validate_offsets [{}]: ff offset {} >= ff_bytes {} (module={})",
                    context,
                    raw,
                    ff_bytes,
                    self.name,
                );
            } else {
                assert!(
                    raw < comb_bytes || comb_bytes == 0,
                    "validate_offsets [{}]: comb offset {} >= comb_bytes {} (module={})",
                    context,
                    raw,
                    comb_bytes,
                    self.name,
                );
            }
        };

        let validate_stmts = |stmts: &ProtoStatements, label: &str| {
            for block in &stmts.0 {
                if let ProtoStatementBlock::Interpreted(proto) = block {
                    for s in proto {
                        let mut ins = vec![];
                        let mut outs = vec![];
                        s.gather_variable_offsets(&mut ins, &mut outs);
                        for off in ins.iter().chain(outs.iter()) {
                            check(off, label);
                        }
                    }
                }
            }
        };

        for (event, stmts) in &self.event_statements {
            validate_stmts(stmts, &format!("event {event:?}"));
        }
        validate_stmts(&self.comb_statements, "comb");

        // Validate variable metadata offsets
        validate_meta_offsets(&self.module_variable_meta, ff_bytes, comb_bytes, use_4state);
    }
}

#[cfg(debug_assertions)]
fn validate_meta_offsets(
    meta: &ModuleVariableMeta,
    ff_bytes: usize,
    comb_bytes: usize,
    use_4state: bool,
) {
    for (id, var_meta) in &meta.variable_meta {
        let vs = value_size(var_meta.native_bytes, use_4state);
        for (i, elem) in var_meta.elements.iter().enumerate() {
            let off = elem.current_offset() as usize;
            if elem.is_ff() {
                // Packed FFs have `next_offset == current_offset`
                // (sentinel) and occupy only `vs` bytes; unpacked
                // (multi-RMW) FFs have `next_offset == current_offset + vs`
                // and need `vs * 2` bytes total.
                let packed = elem.next_offset == elem.current_offset();
                let span = if packed { vs } else { vs * 2 };
                assert!(
                    off + span <= ff_bytes,
                    "validate_meta: ff var {:?}[{}] offset {} + span {} > ff_bytes {} (packed={})",
                    id,
                    i,
                    off,
                    span,
                    ff_bytes,
                    packed,
                );
            } else {
                assert!(
                    off + vs <= comb_bytes,
                    "validate_meta: comb var {:?}[{}] offset {} + vs {} > comb_bytes {}",
                    id,
                    i,
                    off,
                    vs,
                    comb_bytes,
                );
            }
        }
    }
    for child in &meta.children {
        validate_meta_offsets(child, ff_bytes, comb_bytes, use_4state);
    }
}

/// Maximum number of statements per JIT function.
/// Keeps regalloc2 cost manageable (O(N^2) in SSA variable count).
/// Sweet spot around 1024-2048: per-step enum-match dispatch overhead
/// grows as chunks shrink below ~256, while Cranelift regalloc spill
/// cascade / load_cache eviction churn grows as chunks exceed ~4096.
/// Overridable via `VERYL_JIT_CHUNK_SIZE` env var for sweeps.
const JIT_CHUNK_SIZE_DEFAULT: usize = 1024;

fn jit_chunk_size() -> usize {
    std::env::var("VERYL_JIT_CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(JIT_CHUNK_SIZE_DEFAULT)
}

/// Per-event JIT path: load_cache CSE enabled, no nested CompiledBlocks
/// expected.  `VERYL_EVENT_CHUNK_SIZE=N` is diagnostic: splitting here gives
/// each chunk its own perf-map entry, attributing event-side samples per
/// module, and leaves the comb side's chunking alone.
fn try_jit(context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
    let event_split: Option<usize> = std::env::var("VERYL_EVENT_CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse().ok());
    if let Some(n) = event_split.filter(|&n| n > 0 && n < proto.len()) {
        let mut blocks = Vec::new();
        for piece in proto.chunks(n) {
            blocks.extend(build_chunked_via_registry(context, piece.to_vec(), false).0);
        }
        return ProtoStatements(blocks);
    }
    build_chunked_via_registry(context, proto, /* contains_compiled_block= */ false)
}

/// Offsets a testbench-body chunk may write: those of the testbench's own
/// variables that the design's comb does not read.  A comb-touched element
/// disqualifies its whole variable, which is the granularity `tb_dirty`'s
/// spans use; this test has to be at least as strict as that filter, since
/// the filter only gets to judge the chunk once it exists.
#[cfg(not(target_family = "wasm"))]
fn tb_private_offsets(
    variable_meta: &HashMap<VarId, VariableMeta>,
    comb_touched: &HashSet<VarOffset>,
) -> HashSet<VarOffset> {
    let mut private = HashSet::default();
    for meta in variable_meta.values() {
        let offsets: Vec<VarOffset> = meta
            .elements
            .iter()
            .flat_map(|e| {
                let next = e.is_ff().then_some(VarOffset::Ff(e.next_offset));
                std::iter::once(e.current).chain(next)
            })
            .collect();
        if !offsets.iter().any(|o| comb_touched.contains(o)) {
            private.extend(offsets);
        }
    }
    private
}

/// A statement a testbench-body chunk may contain: buildable by the chunk
/// backends, writes comb storage only (an FF write would push the write log,
/// whose pointer testbench statements do not carry), and free of control the
/// testbench runner must interpret (`$tb` method calls advance simulation,
/// system calls buffer output, `break` exits the testbench loop).
///
/// It must also write nothing the design's comb reads.  The runner settles
/// comb before EVERY testbench statement and a chunk collapses those into one
/// settle ahead of the run, so a write the design can see would leave a later
/// statement in the same run reading a stale comb value.
#[cfg(not(target_family = "wasm"))]
fn tb_chunkable(s: &ProtoStatement, private: &HashSet<VarOffset>) -> bool {
    fn pure(s: &ProtoStatement) -> bool {
        match s {
            ProtoStatement::Assign(a) => !a.dst.is_ff(),
            ProtoStatement::AssignDynamic(a) => !a.dst_base.is_ff(),
            ProtoStatement::If(x) => x.true_side.iter().all(pure) && x.false_side.iter().all(pure),
            ProtoStatement::Case(c) => {
                c.arms.iter().all(|a| a.body.iter().all(pure)) && c.default.iter().all(pure)
            }
            ProtoStatement::For(f) => !f.var_offset.is_ff() && f.body.iter().all(pure),
            ProtoStatement::SequentialBlock(b) => b.iter().all(pure),
            _ => false,
        }
    }
    if !pure(s) || !s.can_build_binary() {
        return false;
    }
    let (mut ins, mut outs) = (Vec::new(), Vec::new());
    // `private` is keyed on unfolded offsets, so this side must expand too.
    s.gather_variable_offsets_expanded(&BigArrayFold::default(), &mut ins, &mut outs);
    outs.iter().all(|o| private.contains(o))
}

/// Testbench-body pre-chunking (`Event::Initial` only).  A `#[test]` module's
/// initial block is a control skeleton — a cycle loop around `$tb` clock
/// calls — whose bodies the testbench runner walks statement-by-statement
/// through the interpreter EVERY cycle.  The skeleton cannot compile, but
/// maximal runs of plain statements inside its `for`/`if` bodies can: each
/// run becomes one `CompiledBlock` (executed as a single native dispatch),
/// with its write set recorded so `tb_dirty` can still classify it
/// comb-clean.  Non-chunkable control statements keep their shape and are
/// descended into.
#[cfg(not(target_family = "wasm"))]
fn precompile_tb_bodies(
    context: &mut Context,
    stmts: Vec<ProtoStatement>,
    private: &HashSet<VarOffset>,
) -> Vec<ProtoStatement> {
    fn descend(
        context: &mut Context,
        s: ProtoStatement,
        private: &HashSet<VarOffset>,
    ) -> ProtoStatement {
        match s {
            ProtoStatement::For(mut f) => {
                f.body = walk(context, std::mem::take(&mut f.body), private);
                ProtoStatement::For(f)
            }
            ProtoStatement::If(mut x) => {
                x.true_side = walk(context, std::mem::take(&mut x.true_side), private);
                x.false_side = walk(context, std::mem::take(&mut x.false_side), private);
                ProtoStatement::If(x)
            }
            ProtoStatement::Case(mut c) => {
                for arm in &mut c.arms {
                    arm.body = walk(context, std::mem::take(&mut arm.body), private);
                }
                c.default = walk(context, std::mem::take(&mut c.default), private);
                ProtoStatement::Case(c)
            }
            ProtoStatement::SequentialBlock(b) => {
                ProtoStatement::SequentialBlock(walk(context, b, private))
            }
            other => other,
        }
    }
    fn flush(context: &mut Context, run: &mut Vec<ProtoStatement>, out: &mut Vec<ProtoStatement>) {
        if run.is_empty() {
            return;
        }
        // A lone plain assign interprets about as fast as it dispatches.
        if run.len() == 1 && matches!(run[0], ProtoStatement::Assign(_)) {
            out.append(run);
            return;
        }
        let originals = std::mem::take(run);
        let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
        // The output count below is the expansion itself, so nothing folds.
        let unfolded = BigArrayFold::default();
        for s in &originals {
            s.gather_variable_offsets_expanded(&unfolded, &mut inputs, &mut outputs);
        }
        for v in [&mut inputs, &mut outputs] {
            v.sort_unstable_by_key(|o: &VarOffset| (o.is_ff(), o.raw()));
            v.dedup();
        }
        // A dynamic write into a large array expands to one output per
        // element; such runs are not worth a chunk's bookkeeping.
        if outputs.len() > 4096 {
            out.extend(originals);
            return;
        }
        let blocks = build_chunked_via_registry(context, originals.clone(), false);
        for block in blocks.0 {
            match block {
                ProtoStatementBlock::Compiled(artifact) => {
                    out.push(ProtoStatement::CompiledBlock(CompiledBlockStatement {
                        artifact,
                        ff_delta_bytes: 0,
                        comb_delta_bytes: 0,
                        input_offsets: inputs.clone(),
                        output_offsets: outputs.clone(),
                        ff_canonical_offsets: Vec::new(),
                        stmt_deps: Vec::new(),
                        original_stmts: originals.clone(),
                    }));
                }
                ProtoStatementBlock::Interpreted(stmts) => out.extend(stmts),
            }
        }
    }
    fn walk(
        context: &mut Context,
        body: Vec<ProtoStatement>,
        private: &HashSet<VarOffset>,
    ) -> Vec<ProtoStatement> {
        let mut out = Vec::new();
        let mut run: Vec<ProtoStatement> = Vec::new();
        for s in body {
            if tb_chunkable(&s, private) {
                run.push(s);
            } else {
                flush(context, &mut run, &mut out);
                out.push(descend(context, s, private));
            }
        }
        flush(context, &mut run, &mut out);
        out
    }
    // Top level stays with `try_jit` (its runs already chunk); only the
    // bodies of the interpreted control skeleton need this pass.
    stmts
        .into_iter()
        .map(|s| descend(context, s, private))
        .collect()
}

/// Unified-comb JIT path: nested CompiledBlocks may mutate comb storage
/// between loads, so load_cache CSE is disabled in the emitted chunks.
fn try_jit_no_cache(context: &mut Context, proto: Vec<ProtoStatement>) -> ProtoStatements {
    build_chunked_via_registry(context, proto, /* contains_compiled_block= */ true)
}

/// `try_jit_no_cache` with chunk splits forced at `boundaries` (sorted pre-JIT
/// statement indices), so a gated cone segment maps to a whole number of
/// blocks.  Returns, per boundary-delimited piece, its `[lo, hi)` block range
/// in the produced `ProtoStatements`.
fn try_jit_with_boundaries(
    context: &mut Context,
    mut proto: Vec<ProtoStatement>,
    boundaries: &[usize],
) -> (
    ProtoStatements,
    Vec<(usize, usize, usize)>, // (piece_start_stmt, block_lo, block_hi)
) {
    let mut blocks: Vec<ProtoStatementBlock> = Vec::new();
    let mut pieces: Vec<(usize, usize, usize)> = Vec::new();
    // Split back to front so indices stay valid.
    let mut cuts: Vec<usize> = boundaries.to_vec();
    cuts.push(proto.len());
    cuts.push(0);
    cuts.sort_unstable();
    cuts.dedup();
    let mut tails: Vec<(usize, Vec<ProtoStatement>)> = Vec::new();
    for w in cuts.windows(2).rev() {
        let (s, e) = (w[0], w[1]);
        tails.push((s, proto.split_off(s.min(proto.len()))));
        debug_assert!(e >= s);
    }
    tails.reverse();
    for (start, piece) in tails {
        if piece.is_empty() {
            pieces.push((start, blocks.len(), blocks.len()));
            continue;
        }
        let lo = blocks.len();
        let ps = build_chunked_via_registry(context, piece, true);
        blocks.extend(ps.0);
        pieces.push((start, lo, blocks.len()));
    }
    (ProtoStatements(blocks), pieces)
}

/// Shared chunk-building helper.  Asks `context.backends` to group
/// `proto` into chunks; jittable groups become `Compiled`, others stay
/// `Interpreted`.  Empty registry → fully interpreted (wasm /
/// `use_jit=false` paths arrive here with zero backends registered).
fn build_chunked_via_registry(
    context: &mut Context,
    proto: Vec<ProtoStatement>,
    contains_compiled_block: bool,
) -> ProtoStatements {
    if context.backends.is_empty() {
        return ProtoStatements(vec![ProtoStatementBlock::Interpreted(proto)]);
    }

    // CompileCtx borrows from `context.config` (shared), while
    // `build_chunked` also needs `&mut context.backends` — distinct fields,
    // so Rust's split borrow permits both.
    let max_chunk_size = jit_chunk_size();
    let outputs = {
        let ctx = CompileCtx {
            config: &context.config,
            use_4state: context.config.use_4state,
            contains_compiled_block,
        };
        context.backends.build_chunked(&ctx, proto, max_chunk_size)
    };

    let mut blocks = Vec::with_capacity(outputs.len());
    for out in outputs {
        match out {
            ChunkOutput::Compiled(artifact) => blocks.push(ProtoStatementBlock::Compiled(artifact)),
            ChunkOutput::Interpreted(stmts) => blocks.push(ProtoStatementBlock::Interpreted(stmts)),
        }
    }
    ProtoStatements(blocks)
}

fn pass_diag_phase(phase: &str) {
    if std::env::var("VERYL_PASS_DIAG").is_ok() {
        log::info!("pass_diag: analyze_dependency exit = {phase}");
    }
}

/// Structural key for the whole comb pipeline: the comb list's fingerprint
/// folded with a digest of the event statements and DCE protect set. Those two
/// are the only inputs, besides the comb list, that dead-var DCE reads, so a key
/// match guarantees the memoised pipeline reproduces the exact result. Token-
/// and pointer-agnostic (see `ProtoAssignStatement`/`ChunkArtifact` `Debug`).
fn comb_pipeline_key(
    use_4state: bool,
    unified: &[ProtoStatement],
    events: &HashMap<Event, Vec<ProtoStatement>>,
    protect: &HashSet<VarOffset>,
) -> u128 {
    use crate::backend::registry::whole_comb_fingerprint;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Event digest: only the liveness the dead-var DCE actually reads, not the
    // full event content — otherwise per-test constants / `$readmemh` paths /
    // `$display` strings (which don't change deadness) would make every test a
    // miss. Pooled across all events (DCE sees them as one joint census).
    let event_slices: Vec<&[ProtoStatement]> = events.values().map(|v| v.as_slice()).collect();
    let evt = dead_var_dce::census_digest(&event_slices);
    // Protect digest: hash the sorted offsets (order-independent, collision-safe).
    let mut prot_offs: Vec<isize> = protect.iter().map(|o| o.raw()).collect();
    prot_offs.sort_unstable();
    let mut h = DefaultHasher::new();
    evt.hash(&mut h);
    prot_offs.hash(&mut h);
    whole_comb_fingerprint(use_4state, unified, h.finish() as u128)
}

/// Run the comb pipeline: `analyze_dependency` → `reorder_by_level` →
/// `dce_aggressive` → dead-var DCE → `try_jit_no_cache`. Mutates
/// Temporary diagnostic (VERYL_STMT_ORDER_DUMP=1): dump the stmt
/// order at a pipeline stage to localize run-to-run nondeterminism.
pub(crate) fn dump_stmt_order(tag: &str, module_name: StrId, stmts: &[ProtoStatement]) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("VERYL_STMT_ORDER_DUMP").as_deref() == Ok("1")) {
        return;
    }
    fn dump_one(module_name: StrId, tag: &str, path: &str, s: &ProtoStatement) {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        let tok = s
            .token()
            .map(|t| format!("{}:{}", t.beg.source, t.beg.line))
            .unwrap_or_default();
        let kind = match s {
            ProtoStatement::Assign(_) => "Assign",
            ProtoStatement::AssignDynamic(_) => "AssignDyn",
            ProtoStatement::If(_) => "If",
            ProtoStatement::Case(_) => "Case",
            ProtoStatement::For(_) => "For",
            ProtoStatement::Break => "Break",
            ProtoStatement::SystemFunctionCall(_) => "SysFn",
            ProtoStatement::CompiledBlock(_) => "CB",
            ProtoStatement::SequentialBlock(_) => "SeqBlock",
            ProtoStatement::TbMethodCall { .. } => "TbMethod",
        };
        eprintln!("[stmtord] {module_name} {tag} {path} {kind} tok={tok} out={outs:?} in={ins:?}");
        match s {
            ProtoStatement::SequentialBlock(inner) => {
                for (j, t) in inner.iter().enumerate() {
                    dump_one(module_name, tag, &format!("{path}.{j}"), t);
                }
            }
            ProtoStatement::If(x) => {
                for (j, t) in x.true_side.iter().enumerate() {
                    dump_one(module_name, tag, &format!("{path}.t{j}"), t);
                }
                for (j, t) in x.false_side.iter().enumerate() {
                    dump_one(module_name, tag, &format!("{path}.f{j}"), t);
                }
            }
            ProtoStatement::Case(x) => {
                for (a, arm) in x.arms.iter().enumerate() {
                    for (j, t) in arm.body.iter().enumerate() {
                        dump_one(module_name, tag, &format!("{path}.a{a}.{j}"), t);
                    }
                }
                for (j, t) in x.default.iter().enumerate() {
                    dump_one(module_name, tag, &format!("{path}.d{j}"), t);
                }
            }
            _ => {}
        }
    }
    for (i, s) in stmts.iter().enumerate() {
        dump_one(module_name, tag, &format!("{i}"), s);
    }
}

/// Comb offsets written anywhere in the event statement lists
/// (misclassified-FF: ICG enables and other event-written comb).  The
/// AOT-C const-cone split must treat any read of these as non-constant —
/// no comb statement writes them, but their value changes across settles.
/// Dynamic writes taint their whole element range.
/// `None` when an event contains writes this walker cannot bound (a
/// CompiledBlock without original statements, a tb-method call whose
/// return destination is an unresolved `VarId`) — the caller must not
/// arm the split then.
/// Narrowest storage an element can occupy, turning an element count into a
/// lower bound on the bytes the cone gate's fold weighs.
const MIN_ELEMENT_BYTES: usize = 4;

fn collect_event_written_comb(
    events: &HashMap<Event, Vec<ProtoStatement>>,
) -> Option<HashSet<isize>> {
    use crate::ir::statement::ProtoTbMethodKind;
    fn walk(s: &ProtoStatement, out: &mut HashSet<isize>) -> bool {
        match s {
            ProtoStatement::Assign(a) => {
                if !a.dst.is_ff() {
                    out.insert(a.dst.raw());
                }
                true
            }
            ProtoStatement::AssignDynamic(a) => {
                if !a.dst_base.is_ff() && a.dst_stride != 0 {
                    // Past the compare budget the run owns one folded span, so
                    // its ends reach what the interior would; below it each
                    // element owns a span and has to be named.  The fold weighs
                    // elements, not the range they stride over.
                    let least_bytes = a.dst_num_elements * MIN_ELEMENT_BYTES;
                    if least_bytes > cone_gate::MAX_ELEMENT_OWNER_BYTES {
                        let last = a.dst_num_elements.saturating_sub(1) as isize;
                        out.insert(a.dst_base.raw());
                        out.insert(a.dst_base.raw() + a.dst_stride * last);
                    } else {
                        for k in 0..a.dst_num_elements {
                            out.insert(a.dst_base.raw() + a.dst_stride * k as isize);
                        }
                    }
                }
                true
            }
            ProtoStatement::If(x) => x
                .true_side
                .iter()
                .chain(x.false_side.iter())
                .all(|s| walk(s, out)),
            ProtoStatement::Case(x) => x
                .arms
                .iter()
                .flat_map(|a| a.body.iter())
                .chain(x.default.iter())
                .all(|s| walk(s, out)),
            ProtoStatement::SequentialBlock(inner) => inner.iter().all(|s| walk(s, out)),
            ProtoStatement::CompiledBlock(cb) => {
                // `output_offsets` compresses a dynamic array write to
                // base + last element, hiding interior elements — walk the
                // original statements.  No originals = unboundable.
                if cb.original_stmts.is_empty() {
                    return false;
                }
                cb.original_stmts.iter().all(|s| walk(s, out))
            }
            // The loop counter itself is an event-written comb variable;
            // every body write is either a fixed offset or an AssignDynamic
            // whose whole element range is tainted — both bounded.
            ProtoStatement::For(f) => {
                if !f.var_offset.is_ff() {
                    out.insert(f.var_offset.raw());
                }
                f.body.iter().all(|s| walk(s, out))
            }
            ProtoStatement::SystemFunctionCall(c) => {
                // Readmemh writes are boundable: one offset per element, or
                // just the ends once the image owns a folded span (see
                // `AssignDynamic` above).
                if let crate::ir::ProtoSystemFunctionCall::Readmemh { elements, .. } = c {
                    let comb = || elements.iter().filter(|e| !e.current.is_ff());
                    // The elements carry no width, so bound the fold's measure
                    // by the narrowest element there is.
                    let least_bytes = comb().count() * MIN_ELEMENT_BYTES;
                    let lo = comb().map(|e| e.current.raw()).min();
                    let hi = comb().map(|e| e.current.raw()).max();
                    if let (Some(lo), Some(hi)) = (lo, hi) {
                        if least_bytes > cone_gate::MAX_ELEMENT_OWNER_BYTES {
                            out.insert(lo);
                            out.insert(hi);
                        } else {
                            for e in comb() {
                                out.insert(e.current.raw());
                            }
                        }
                    }
                }
                true
            }
            // A tb-method return value lands in a variable this walker
            // cannot resolve (`VarId`, not an offset) — unboundable.
            ProtoStatement::TbMethodCall { method, .. } => !matches!(
                method,
                ProtoTbMethodKind::Component { ret: Some(_), .. }
                    | ProtoTbMethodKind::RandomGet { ret: Some(_), .. }
                    | ProtoTbMethodKind::RandomGetRange { ret: Some(_), .. }
                    | ProtoTbMethodKind::RandomGetSeed { ret: Some(_) }
            ),
            // No comb writes.
            ProtoStatement::Break => true,
        }
    }
    let mut out = HashSet::default();
    for stmts in events.values() {
        for s in stmts {
            if !walk(s, &mut out) {
                return None;
            }
        }
    }
    Some(out)
}

/// `all_event_statements` in place with the dead-var drop (mirroring the miss
/// path); the returned `dead_offsets` let a cache hit reproduce that drop.
#[allow(clippy::too_many_arguments)]
fn run_comb_pipeline(
    context: &mut Context,
    unified: Vec<ProtoStatement>,
    all_event_statements: &mut HashMap<Event, Vec<ProtoStatement>>,
    protect: &HashSet<VarOffset>,
    layout_inputs: Option<&comb_layout::LayoutInputs>,
    fusion_extra: Option<&[VarOffset]>,
    cone_inputs: Option<&cone_gate::ConeGateInputs>,
    module_name: StrId,
) -> Result<comb_pipeline_cache::CombPipeline, SimulatorError> {
    dump_stmt_order("conv", module_name, &unified);
    // Version-split: fuse multi-write (versioned) comb chains into single
    // writers.  Module-level always_combs were already handled during conv
    // (see `ProtoDeclaration::conv`); this covers testbench-level blocks
    // that appear directly in the merged list.

    // Comb bytes this pass reserves for rename temps; recorded on the
    // pipeline so a cache hit — which skips this whole function — can
    // reserve the same span (see `CombPipeline::vsplit_temp_bytes`).
    let mut vsplit_temp_bytes = 0usize;
    let unified = {
        let mut unified = unified;
        if version_split::pass_enabled(context.config.use_4state) {
            let use_4state = context.config.use_4state;
            let before = context.comb_total_bytes;
            let comb_total = &mut context.comb_total_bytes;
            let mut alloc = |width: usize| -> isize {
                let nb = crate::ir::variable::native_bytes(width);
                let off = *comb_total as isize;
                *comb_total += crate::ir::variable::value_size(nb, use_4state);
                off
            };
            let stats = version_split::run(&mut unified, &mut alloc);
            vsplit_temp_bytes = context.comb_total_bytes - before;
            version_split::accumulate(&stats);
            log::info!(
                "version_split totals ({module_name}): {}",
                version_split::totals_line()
            );
        }
        unified
    };
    dump_stmt_order("post-vsplit", module_name, &unified);
    let (unified_sorted, passes_hint) = analyze_dependency(unified)?;
    dump_stmt_order("post-topo", module_name, &unified_sorted);
    // No DCE/inlining: unified list includes internal child comb that would be incorrectly removed.
    // reorder_by_level preserves the sort's dependency relations (readers
    // stay relative to their version writers via the RAW/WAR leveling), so
    // an exact pass hint derived from the schedule remains valid.
    let unified_sorted = reorder_by_level(unified_sorted);
    dump_stmt_order("post-level", module_name, &unified_sorted);
    let required_comb_passes =
        passes_hint.unwrap_or_else(|| compute_required_passes(&unified_sorted));
    if passes_hint.is_some() && std::env::var("VERYL_PASS_DIAG").is_ok() {
        log::info!(
            "pass_diag: exact hint {} passes (positional metric would give {})",
            required_comb_passes,
            compute_required_passes(&unified_sorted)
        );
    }

    // A non-trivial SCC in the expanded dataflow view is either
    // structurally-cyclic-but-logically-false feedback (the multi-pass
    // settle resolves it) or duplicate ProtoStatements from an IR
    // assembly bug.  Under the positional metric the settled kind
    // always leaves a counted backward edge, so SCC + single-pass can
    // only be the duplicate bug.  Exact-hint paths are exempt (a
    // strict block-aware schedule legitimately settles a false SCC in
    // one pass); the test-local `nontrivial_comb_scc == 0` assertions
    // cover the historical duplicate scenarios there.
    //
    // Skip the (heavy: Tarjan + per-stmt I/O scan) computation in
    // release-without-tests since the assert is a no-op there and the
    // field is only consumed by tests.
    let nontrivial_comb_scc = if cfg!(any(debug_assertions, test)) {
        compute_scc_stats(&unified_sorted).0
    } else {
        0
    };
    debug_assert!(
        nontrivial_comb_scc == 0 || passes_hint.is_some() || required_comb_passes > 1,
        "ProtoModule {:?}: {} nontrivial SCC(s) in unified_sorted but a \
         single-pass schedule — this indicates duplicate ProtoStatements \
         in the simulator IR.",
        module_name,
        nontrivial_comb_scc,
    );

    let unified_sorted = dce_aggressive(unified_sorted);

    // Dead Variable DCE: drop full-width `Assign`s whose dst has zero
    // consumers anywhere in this module's pre-JIT comb stmts and every
    // event stmt set.  Complements `dup_assign_dce` (which handles the
    // overwriting-store case) by killing writes that nobody reads in
    // the first place — typical residue of `comb_to_ff_hoist` leaving
    // the original comb-side Variable dead once the FF consumes the
    // hoisted expression.  Env-gated by `VERYL_DEAD_VAR_DCE`, default
    // ON; opt out with `VERYL_DEAD_VAR_DCE=0`.  `protect` is built by the
    // caller (it feeds the cache key too).  The union of every pass's dead
    // set is returned so a cache hit can re-apply it to another test's events.
    let mut dead_union: HashSet<VarOffset> = HashSet::default();
    let unified_sorted = if dead_var_dce::enabled() {
        // Multi-pass DCE default ON: iterate to fixpoint so that
        // cascaded drops (a dst becomes dead once its only consumer
        // was itself dropped) are caught in subsequent passes.  Opt
        // out via `VERYL_DEAD_VAR_DCE_MULTI=0`.
        let multi_pass = std::env::var("VERYL_DEAD_VAR_DCE_MULTI").ok().as_deref() != Some("0");
        let mut unified_sorted = unified_sorted;
        let mut total_dropped = 0usize;
        let mut pass = 0usize;
        loop {
            let mut slices: Vec<&[ProtoStatement]> =
                Vec::with_capacity(1 + all_event_statements.len());
            slices.push(unified_sorted.as_slice());
            for stmts in all_event_statements.values() {
                slices.push(stmts.as_slice());
            }
            let mut dead = dead_var_dce::collect_dead_offsets(&slices);
            for p in protect {
                dead.remove(p);
            }
            if dead.is_empty() {
                break;
            }
            pass += 1;
            let (rewritten, dropped_here) = dead_var_dce::apply_counting(unified_sorted, &dead);
            unified_sorted = rewritten;
            let mut total_dropped_here = dropped_here;
            for stmts in all_event_statements.values_mut() {
                let taken = std::mem::take(stmts);
                let (new_stmts, d) = dead_var_dce::apply_counting(taken, &dead);
                *stmts = new_stmts;
                total_dropped_here += d;
            }
            let dead_len = dead.len();
            dead_union.extend(dead);
            if std::env::var("VERYL_DEAD_VAR_DCE_DIAG").ok().as_deref() == Some("1") {
                eprintln!(
                    "[DeadVarDce] module={} pass={} dead_set={} dropped_stmts={}",
                    module_name, pass, dead_len, total_dropped_here,
                );
            }
            total_dropped += total_dropped_here;
            if total_dropped_here == 0 || !multi_pass {
                break;
            }
        }
        if std::env::var("VERYL_DEAD_VAR_DCE_DIAG").ok().as_deref() == Some("1") {
            eprintln!(
                "[DeadVarDce] module={} total_passes={} total_dropped={}",
                module_name, pass, total_dropped,
            );
        }
        unified_sorted
    } else {
        unified_sorted
    };

    // Comb fusion (`VERYL_COMB_FUSION`, P1): fold single-reader comb defs
    // into their reader's expression.  Runs before the
    // relayout so the freed storage is already unreferenced when the
    // schedule is built (it parks as a cold unit; DCE cannot see it earlier
    // because the def only loses its reader here).
    let (unified_sorted, fused_offsets) = if comb_fusion::enabled(context.config.use_4state) {
        let mut externals: HashSet<VarOffset> = protect.clone();
        if let Some(extra) = fusion_extra {
            externals.extend(extra.iter().copied());
        }
        comb_fusion::inline_single_readers(unified_sorted, all_event_statements, &externals)
    } else {
        (unified_sorted, Vec::new())
    };

    // Cone scheduling: cluster each qualifying module
    // subtree into few contiguous segments so the settle can skip them by
    // one compare each.  The reorder is a legal schedule of the same
    // dependency graph, so pass counts and the relayout below stay valid.
    let (unified_sorted, cone_plan) = match cone_inputs {
        Some(ci) => match cone_gate::plan(&unified_sorted, ci) {
            Some(plan) => {
                let mut reordered = Vec::with_capacity(unified_sorted.len());
                let mut src: Vec<Option<ProtoStatement>> =
                    unified_sorted.into_iter().map(Some).collect();
                for &oi in &plan.order {
                    reordered.push(src[oi as usize].take().expect("permutation is a bijection"));
                }
                (reordered, Some(plan))
            }
            None => (unified_sorted, None),
        },
        None => (unified_sorted, None),
    };
    // The clustered order is a different topological order; a settle
    // back-edge may sit later in it, so the positional pass metric must be
    // re-taken (never lowered: the hint may be exact for the OLD order only).
    let required_comb_passes = if cone_plan.is_some() {
        let repositioned = compute_required_passes(&unified_sorted);
        if cone_gate::diag() {
            eprintln!(
                "[cone_gate] passes: pre-reorder {required_comb_passes} positional {repositioned}"
            );
        }
        required_comb_passes.max(repositioned)
    } else {
        required_comb_passes
    };

    // Settle-order relayout (`VERYL_COMB_LAYOUT`): derive the storage
    // permutation from the final execution order and rewrite the comb
    // statements through it before the JIT bakes their offsets in.  The
    // event statements are only READ here (rank + orphan detection, in the
    // old offset space); the caller replays the schedule on them and on
    // every other offset-bearing structure the pipeline does not own.
    let layout = layout_inputs.and_then(|li| {
        comb_layout::build_schedule(
            &li.meta_units,
            &unified_sorted,
            all_event_statements,
            &li.extra_offsets,
            // NOT li.comb_total: version_split just bump-allocated its rename
            // temps above that Conv-time figure, and they need units too.
            context.comb_total_bytes,
        )
        .map(Arc::new)
    });
    let mut unified_sorted = unified_sorted;
    if let Some(sched) = layout.as_deref() {
        comb_layout::apply_to_stmts(&mut unified_sorted, sched);
    }

    // Snapshot before JIT consumes it: the whole-comb backend needs the
    // pre-JIT stmts (JIT CompiledBlocks hide stmt-level I/O).
    let pre_jit_stmts = Arc::new(unified_sorted.clone());
    let (comb_statements, cone_segments) = match &cone_plan {
        Some(plan) => {
            let mut bounds: Vec<usize> = plan
                .segments
                .iter()
                .flat_map(|s| [s.start, s.end])
                .collect();
            bounds.sort_unstable();
            bounds.dedup();
            let (ps, pieces) = try_jit_with_boundaries(context, unified_sorted, &bounds);
            let mut segs: Vec<cone_gate::ConeSegment> = Vec::new();
            for s in &plan.segments {
                let Some(&(_, blo, bhi)) = pieces.iter().find(|&&(st, _, _)| st == s.start) else {
                    continue;
                };
                // Bring the compare ranges into the FINAL storage space —
                // piecewise, because a merged span can straddle relayout
                // units that land apart.
                let mut compare: Vec<(bool, u32, u32)> = Vec::new();
                for &(ff, cs, ce) in &s.compare {
                    match (ff, layout.as_deref()) {
                        (false, Some(sched)) => {
                            for (ns, ne) in sched.translate_range(cs as isize, ce as isize) {
                                compare.push((false, ns as u32, ne as u32));
                            }
                        }
                        _ => compare.push((ff, cs, ce)),
                    }
                }
                compare.sort_unstable();
                let translate_pairs = |v: &[(u32, u32)]| -> Vec<(u32, u32)> {
                    let mut out: Vec<(u32, u32)> = Vec::new();
                    for &(cs, ce) in v {
                        match layout.as_deref() {
                            Some(sched) => {
                                for (ns, ne) in sched.translate_range(cs as isize, ce as isize) {
                                    out.push((ns as u32, ne as u32));
                                }
                            }
                            None => out.push((cs, ce)),
                        }
                    }
                    out.sort_unstable();
                    out
                };
                // Relayout scatters the whole-variable spans, leaving
                // thousands of 4-8 byte memcmp/memcpy ranges whose per-span
                // setup dwarfs the byte traffic.  Fuse ranges across small
                // gaps: comparing a gap byte is at worst a spurious dirty,
                // and replaying one is exact because the pre-run compare
                // (`compare_pre` covers every replay range, gaps included)
                // just proved it unchanged since the stored run.  The
                // backedge ranges must stay exact — a gap byte the segment
                // legitimately rewrites would read as non-convergence.
                let coalesce = |v: &mut Vec<(u32, u32)>| {
                    let mut out: Vec<(u32, u32)> = Vec::with_capacity(v.len());
                    for &(cs, ce) in v.iter() {
                        match out.last_mut() {
                            Some(p) if cs <= p.1 + 32 => p.1 = p.1.max(ce),
                            _ => out.push((cs, ce)),
                        }
                    }
                    *v = out;
                };
                {
                    let mut ffs: Vec<(u32, u32)> = Vec::new();
                    let mut combs: Vec<(u32, u32)> = Vec::new();
                    for &(ff, cs, ce) in &compare {
                        if ff { &mut ffs } else { &mut combs }.push((cs, ce));
                    }
                    coalesce(&mut ffs);
                    coalesce(&mut combs);
                    compare = combs
                        .into_iter()
                        .map(|(cs, ce)| (false, cs, ce))
                        .chain(ffs.into_iter().map(|(cs, ce)| (true, cs, ce)))
                        .collect();
                }
                let backedge = translate_pairs(&s.backedge);
                let mut replay = translate_pairs(&s.replay);
                coalesce(&mut replay);
                // The plan guarantees `compare_pre == replay`; keep the
                // fused ranges identical so every replayed byte stays
                // covered by the pre-run compare.
                let compare_pre = replay.clone();
                segs.push(cone_gate::ConeSegment {
                    block_lo: blo,
                    block_hi: bhi,
                    stmt_lo: s.start,
                    stmt_hi: s.end,
                    state_off: 0,
                    compare,
                    backedge,
                    compare_pre,
                    replay,
                    off_decay: s.off_decay,
                    cone: s.cone.clone(),
                });
            }
            (ps, segs)
        }
        None => {
            let a = try_jit_no_cache(context, unified_sorted);
            (a, Vec::new())
        }
    };
    Ok(comb_pipeline_cache::CombPipeline {
        pre_jit_stmts,
        required_comb_passes,
        comb_statements,
        dead_offsets: dead_union.into_iter().collect(),
        // Fused (retired) offsets in the FINAL storage space: the checker
        // compares element addresses derived from the replayed meta, so a
        // pre-layout offset would name the wrong storage.
        fused_offsets: match layout.as_deref() {
            Some(sched) => fused_offsets.iter().map(|&o| sched.translate(o)).collect(),
            None => fused_offsets,
        },
        cone_segments: Arc::new(cone_segments),
        layout,
        nontrivial_comb_scc,
        vsplit_temp_bytes,
    })
}

/// Returns the scheduled statements plus an exact required-pass hint when the
/// block-aware sort could derive one (see `stable_topo_sort_with_blocks`);
/// `None` means the caller must fall back to `compute_required_passes`.
pub(crate) fn analyze_dependency(
    statements: Vec<ProtoStatement>,
) -> Result<(Vec<ProtoStatement>, Option<usize>), SimulatorError> {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    enum Node {
        Var(VarOffset),
        Statement(usize),
    }

    let mut table = HashMap::default();
    for (i, x) in statements.into_iter().enumerate() {
        table.insert(i, x);
    }

    // Helper: build DAG and attempt stable topological sort (Kahn's algorithm).
    // Returns Ok(sorted) on success, Err(failed_id) on cycle.
    // Uses FIFO queue initialized in source order to preserve source ordering
    // for statements that have no explicit dependency between them.
    let try_topo_sort =
        |table: &HashMap<usize, ProtoStatement>| -> Result<Vec<ProtoStatement>, usize> {
            let mut dag = Dag::<Node, ()>::new();
            let mut dag_nodes: HashMap<Node, _> = HashMap::default();

            let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
            sorted_keys.sort();

            let mut node_to_stmt: HashMap<daggy::NodeIndex, usize> = HashMap::default();

            for id in &sorted_keys {
                let x = &table[id];
                let mut inputs = vec![];
                let mut outputs = vec![];
                x.gather_variable_offsets(&mut inputs, &mut outputs);
                let stmt_node = Node::Statement(*id);
                let stmt = dag.add_node(stmt_node);
                dag_nodes.insert(stmt_node, stmt);
                node_to_stmt.insert(stmt, *id);

                let output_set: HashSet<VarOffset> = outputs.iter().cloned().collect();
                let mut ok = true;
                for var_key in inputs {
                    if output_set.contains(&var_key) {
                        continue;
                    }
                    let var_node = Node::Var(var_key);
                    let var = *dag_nodes
                        .entry(var_node)
                        .or_insert_with(|| dag.add_node(var_node));
                    if dag.add_edge(var, stmt, ()).is_err() {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    return Err(*id);
                }
                for var_key in outputs {
                    let var_node = Node::Var(var_key);
                    let var = *dag_nodes
                        .entry(var_node)
                        .or_insert_with(|| dag.add_node(var_node));
                    if dag.add_edge(stmt, var, ()).is_err() {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    return Err(*id);
                }
            }

            let graph = dag.graph();
            let node_count = graph.node_count();
            let mut in_degree: HashMap<daggy::NodeIndex, usize> = HashMap::default();
            for idx in graph.node_indices() {
                in_degree.insert(idx, 0);
            }
            for edge in graph.edge_indices() {
                if let Some((_src, tgt)) = graph.edge_endpoints(edge) {
                    *in_degree.entry(tgt).or_insert(0) += 1;
                }
            }

            let mut queue: VecDeque<daggy::NodeIndex> = VecDeque::new();
            let mut zero_nodes: Vec<daggy::NodeIndex> = in_degree
                .iter()
                .filter(|&(_, &deg)| deg == 0)
                .map(|(&idx, _)| idx)
                .collect();
            zero_nodes.sort_by_key(|&idx| node_to_stmt.get(&idx).copied().unwrap_or(usize::MAX));
            for idx in zero_nodes {
                queue.push_back(idx);
            }

            let mut ret = vec![];
            let mut t = table.clone();
            let mut visited = 0;
            while let Some(idx) = queue.pop_front() {
                visited += 1;
                if let Node::Statement(x) = graph[idx]
                    && let Some(s) = t.remove(&x)
                {
                    ret.push(s);
                }
                let mut successors: Vec<daggy::NodeIndex> =
                    graph.neighbors_directed(idx, Outgoing).collect();
                successors.sort_by_key(|&s| node_to_stmt.get(&s).copied().unwrap_or(usize::MAX));
                for succ in successors {
                    let deg = in_degree.get_mut(&succ).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(succ);
                    }
                }
            }

            if visited != node_count {
                return Err(sorted_keys[0]);
            }

            Ok(ret)
        };

    // Phase 1: Try with CompiledBlocks as atomic nodes. The bipartite model
    // orders every reader after ALL writers of its inputs, so the schedule
    // settles in exactly one pass.
    if let Ok(sorted) = try_topo_sort(&table) {
        pass_diag_phase("phase1: bipartite, CBs atomic");
        return Ok((sorted, Some(1)));
    }

    // Phase 2: Expand CompiledBlocks and SequentialBlocks and retry.
    // Rebuild the table with fresh sequential IDs so expanded sub-statements
    // keep their parent's position; Phase 3's fallback sorts by ID and relies
    // on that ordering for `let x = expr` vs `always_comb { x = expr; }` to
    // produce equivalent schedules when the block participates in a cycle.
    let has_expandable = table.values().any(|x| {
        matches!(x, ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty())
            || matches!(x, ProtoStatement::SequentialBlock(_))
    });

    // Flattened stmt id → source block (original table key); set by the
    // Phase-2 full flatten.
    let mut block_of: Option<Vec<usize>> = None;

    if has_expandable {
        // FAST PATH: flatten only blocks WITHOUT an intra-block reorder hazard
        // (a write to a comb var an earlier statement of the block read or
        // wrote — WAR/WAW/reassignment); those keep their program order by
        // staying atomic. A bipartite topological sort then interleaves the
        // hazard-free statements across blocks into a backward-edge-free order
        // that settles in ONE comb pass — the common case, where the full
        // recursive flatten + stable_topo_sort below instead leaves a backward
        // edge for cross-block no-prior-writer reads that doubles the passes.
        //
        // On failure fall through to that full-flatten path: an atomic hazard
        // block's conflated I/O can form a phantom cross-block cycle that the
        // bipartite sort rejects but the per-statement flatten resolves.
        fn block_has_reorder_hazard(stmts: &[ProtoStatement]) -> bool {
            let mut seen: HashSet<VarOffset> = HashSet::default();
            for s in stmts {
                let mut ins = vec![];
                let mut outs = vec![];
                s.gather_variable_offsets(&mut ins, &mut outs);
                ins.retain(|o| !o.is_ff());
                outs.retain(|o| !o.is_ff());
                if outs.iter().any(|o| seen.contains(o)) {
                    return true;
                }
                seen.extend(ins);
                seen.extend(outs);
            }
            false
        }
        fn hazard_flatten(stmt: ProtoStatement, out: &mut Vec<ProtoStatement>) {
            match stmt {
                ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty() => {
                    if block_has_reorder_hazard(&cb.original_stmts) {
                        out.push(ProtoStatement::CompiledBlock(cb));
                    } else {
                        for sub in cb.original_stmts {
                            hazard_flatten(sub, out);
                        }
                    }
                }
                ProtoStatement::SequentialBlock(body) => {
                    if block_has_reorder_hazard(&body) {
                        out.push(ProtoStatement::SequentialBlock(body));
                    } else {
                        for sub in body {
                            hazard_flatten(sub, out);
                        }
                    }
                }
                other => out.push(other),
            }
        }
        let mut keys: Vec<usize> = table.keys().cloned().collect();
        keys.sort();
        let mut fast: HashMap<usize, ProtoStatement> = HashMap::default();
        let mut id = 0usize;
        for key in &keys {
            let mut flat = Vec::new();
            hazard_flatten(table[key].clone(), &mut flat);
            for sub in flat {
                fast.insert(id, sub);
                id += 1;
            }
        }
        if let Ok(sorted) = try_topo_sort(&fast) {
            pass_diag_phase("phase2-fast: hazard-flatten + bipartite");
            return Ok((sorted, Some(1)));
        }

        // Recursive: SequentialBlock's gather conflates per-stmt I/O, so
        // nested SeqBlocks (e.g. inside a CompiledBlock's original_stmts)
        // must be unwrapped too or they manufacture phantom edges.
        fn flatten(stmt: ProtoStatement, out: &mut Vec<ProtoStatement>) {
            match stmt {
                ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty() => {
                    for sub in cb.original_stmts {
                        flatten(sub, out);
                    }
                }
                ProtoStatement::SequentialBlock(body) => {
                    for sub in body {
                        flatten(sub, out);
                    }
                }
                other => out.push(other),
            }
        }

        let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
        sorted_keys.sort();

        let mut new_table: HashMap<usize, ProtoStatement> = HashMap::default();
        let mut flat_block_of: Vec<usize> = Vec::new();
        let mut new_id = 0usize;
        for key in sorted_keys {
            let stmt = table.remove(&key).unwrap();
            let mut flat = Vec::new();
            flatten(stmt, &mut flat);
            for sub in flat {
                new_table.insert(new_id, sub);
                flat_block_of.push(key);
                new_id += 1;
            }
        }
        table = new_table;
        block_of = Some(flat_block_of);

        // Sort the flattened (program-order) statements with the block-aware
        // `stable_topo_sort`, NOT the bipartite `try_topo_sort`: the bipartite
        // model makes a reader wait for EVERY writer of a var, reordering
        // sequential reassignment — flattened `x=a; y=x; x=b` would become
        // `x=a; x=b; y=x`, so `y` wrongly reads `b`. `stable_topo_sort` links
        // `y` to its most recent PRIOR writer only. (reorder_by_level applies
        // the matching WAR/WAW leveling downstream.)
        let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
        sorted_keys.sort();
        let stmts: Vec<ProtoStatement> = sorted_keys.iter().map(|k| table[k].clone()).collect();
        let blocks: Vec<usize> = sorted_keys
            .iter()
            .map(|k| block_of.as_ref().unwrap()[*k])
            .collect();
        let (sorted, passes_hint, fell_back) = stable_topo_sort_with_blocks(stmts, &blocks);
        if !fell_back {
            pass_diag_phase("phase2-full: flatten + stable_topo_sort");
            return Ok((sorted, passes_hint));
        }
    }

    // Phase 3: Check for genuine combinational loop vs false positive
    // from non-expandable CompiledBlocks (shared JIT cache).
    let has_non_expandable_cb = table
        .values()
        .any(|x| matches!(x, ProtoStatement::CompiledBlock(cb) if cb.original_stmts.is_empty()));
    let has_any_cb = table
        .values()
        .any(|x| matches!(x, ProtoStatement::CompiledBlock(_)));

    if !has_any_cb || !has_non_expandable_cb {
        // DAG-based sort failed (false cycle from inlined function bodies).
        // Fall back to direct statement-level sort (block-aware when Phase 2
        // flattened the table and recorded statement origins).
        let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
        sorted_keys.sort();
        let stmts: Vec<ProtoStatement> = sorted_keys.iter().map(|k| table[k].clone()).collect();
        let (sorted, passes_hint, _) = if let Some(block_of) = &block_of {
            let blocks: Vec<usize> = sorted_keys.iter().map(|k| block_of[*k]).collect();
            stable_topo_sort_with_blocks(stmts, &blocks)
        } else {
            (stable_topo_sort(stmts), None, false)
        };
        // Verify no genuine combinational loop remains.
        let n = sorted.len();
        let mut s_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
        let mut s_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
        for s in &sorted {
            let mut ins = vec![];
            let mut outs = vec![];
            s.gather_variable_offsets(&mut ins, &mut outs);
            // FF reads/writes don't participate in comb cycles, and
            // `packed_ff` collapses next_offset onto current_offset so they'd
            // share a VarOffset and form phantom edges otherwise.
            ins.retain(|o| !o.is_ff());
            outs.retain(|o| !o.is_ff());
            s_inputs.push(ins);
            s_outputs.push(outs);
        }
        let mut w: HashMap<VarOffset, Vec<usize>> = HashMap::default();
        for (i, outs) in s_outputs.iter().enumerate() {
            for &key in outs {
                w.entry(key).or_default().push(i);
            }
        }
        let mut a: Vec<HashSet<usize>> = vec![HashSet::default(); n];
        let mut deg: Vec<usize> = vec![0; n];
        for (ri, ins) in s_inputs.iter().enumerate() {
            for key in ins {
                if let Some(wis) = w.get(key) {
                    if wis.len() == 1 {
                        let wi = wis[0];
                        if wi != ri && a[wi].insert(ri) {
                            deg[ri] += 1;
                        }
                    } else if let Some(&wi) = wis.iter().rev().find(|&&w| w < ri)
                        && a[wi].insert(ri)
                    {
                        deg[ri] += 1;
                    }
                }
            }
        }
        let mut q: VecDeque<usize> = VecDeque::new();
        for (i, &d) in deg.iter().enumerate() {
            if d == 0 {
                q.push_back(i);
            }
        }
        let mut cnt = 0;
        while let Some(idx) = q.pop_front() {
            cnt += 1;
            for &succ in &a[idx] {
                deg[succ] -= 1;
                if deg[succ] == 0 {
                    q.push_back(succ);
                }
            }
        }
        if cnt == n {
            pass_diag_phase("phase3: stable_topo_sort (no non-expandable CB)");
            return Ok((sorted, passes_hint));
        }
        // `deg > 0` includes the cycle's downstream cone; isolate just the
        // cycle (SCC size >= 2) so the diagnostic focuses on the real loop.
        let cycle_indices: Vec<usize> = {
            use daggy::petgraph::Graph;
            use daggy::petgraph::algo::tarjan_scc;
            let mut g: Graph<usize, ()> = Graph::new();
            let nodes: Vec<_> = (0..n).map(|i| g.add_node(i)).collect();
            for (u, succs) in a.iter().enumerate() {
                for &v in succs {
                    g.add_edge(nodes[u], nodes[v], ());
                }
            }
            let mut cycle: Vec<usize> = tarjan_scc(&g)
                .into_iter()
                .filter(|c| c.len() >= 2)
                .flat_map(|c| c.into_iter().map(|ni| g[ni]))
                .collect();
            cycle.sort();
            cycle
        };
        let mut tokens: Vec<_> = cycle_indices
            .iter()
            .filter_map(|&i| sorted[i].token())
            .filter(|t| *t != Default::default())
            .collect();
        let trigger = tokens.pop().unwrap_or_default();
        return Err(SimulatorError::combinational_loop(&trigger, &tokens));
    }

    // Relaxed ordering: skip edges that would create cycles when at least
    // one endpoint is a non-expandable CompiledBlock.
    // NOTE: With original_stmts now stored in shared JIT cache, all CBs
    // should be expandable. This path should be unreachable in practice.
    log::warn!(
        "analyze_dependency: falling back to relaxed ordering for {} stmts with non-expandable CompiledBlocks",
        table.len()
    );
    let mut dag_relaxed = Dag::<Node, ()>::new();
    let mut dag_nodes_relaxed: HashMap<Node, _> = HashMap::default();
    let cb_ids: HashSet<usize> = table
        .iter()
        .filter_map(|(id, x)| {
            if matches!(x, ProtoStatement::CompiledBlock(_)) {
                Some(*id)
            } else {
                None
            }
        })
        .collect();

    let mut sorted_keys: Vec<usize> = table.keys().cloned().collect();
    sorted_keys.sort();
    for id in &sorted_keys {
        let x = &table[id];
        let mut inputs = vec![];
        let mut outputs = vec![];
        x.gather_variable_offsets(&mut inputs, &mut outputs);
        let stmt_node = Node::Statement(*id);
        let stmt = dag_relaxed.add_node(stmt_node);
        dag_nodes_relaxed.insert(stmt_node, stmt);

        let output_set: HashSet<VarOffset> = outputs.iter().cloned().collect();
        for var_key in &inputs {
            if output_set.contains(var_key) {
                continue;
            }
            let var_node = Node::Var(*var_key);
            let var = *dag_nodes_relaxed
                .entry(var_node)
                .or_insert_with(|| dag_relaxed.add_node(var_node));
            if dag_relaxed.add_edge(var, stmt, ()).is_err() {
                if cb_ids.contains(id) {
                    continue;
                }
                let written_by_cb = table.iter().any(|(oid, ox)| {
                    cb_ids.contains(oid) && {
                        let mut o_outs = vec![];
                        let mut o_ins = vec![];
                        ox.gather_variable_offsets(&mut o_ins, &mut o_outs);
                        o_outs.contains(var_key)
                    }
                });
                if written_by_cb {
                    continue;
                }
            }
        }
        for var_key in &outputs {
            let var_node = Node::Var(*var_key);
            let var = *dag_nodes_relaxed
                .entry(var_node)
                .or_insert_with(|| dag_relaxed.add_node(var_node));
            if dag_relaxed.add_edge(stmt, var, ()).is_err() {
                if cb_ids.contains(id) {
                    continue;
                }
                let read_by_cb = table.iter().any(|(oid, ox)| {
                    cb_ids.contains(oid) && {
                        let mut o_outs = vec![];
                        let mut o_ins = vec![];
                        ox.gather_variable_offsets(&mut o_ins, &mut o_outs);
                        o_ins.contains(var_key)
                    }
                });
                if read_by_cb {
                    continue;
                }
            }
        }
    }

    let nodes = algo::toposort(dag_relaxed.graph(), None).unwrap();
    let mut ret = vec![];
    for i in nodes {
        if let Node::Statement(x) = dag_relaxed[i]
            && let Some(stmt) = table.remove(&x)
        {
            ret.push(stmt);
        }
    }
    Ok((ret, None))
}

/// Bit range for a partial comb assignment. `None` = full-width write/read.
/// Pair is (high_bit_inclusive, low_bit_inclusive) matching Veryl's
/// `Assign.select` convention. Overlap: two ranges overlap if their
/// bit intervals intersect.
pub(crate) type BitRange = Option<(usize, usize)>;

pub(crate) fn ranges_overlap(a: BitRange, b: BitRange) -> bool {
    match (a, b) {
        (None, _) | (_, None) => true,
        (Some((a_hi, a_lo)), Some((b_hi, b_lo))) => a_lo <= b_hi && b_lo <= a_hi,
    }
}

/// Collect (offset, bit_range) outputs for bit-aware SCC analysis.
/// Only captures writes that are precisely bit-ranged (via Assign.select);
/// everything else falls back to full-width (None).
pub(crate) fn gather_bit_aware_outputs(
    stmt: &ProtoStatement,
    out: &mut Vec<(VarOffset, BitRange)>,
) {
    match stmt {
        ProtoStatement::Assign(x) => out.push((x.dst, x.select)),
        ProtoStatement::AssignDynamic(x) => {
            out.push((x.dst_base, None));
            if x.dst_num_elements > 1 {
                let last = VarOffset::new(
                    x.dst_base.is_ff(),
                    x.dst_base.raw() + x.dst_stride * (x.dst_num_elements as isize - 1),
                );
                out.push((last, None));
            }
        }
        ProtoStatement::If(x) => {
            for s in &x.true_side {
                gather_bit_aware_outputs(s, out);
            }
            for s in &x.false_side {
                gather_bit_aware_outputs(s, out);
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &x.arms {
                for s in &arm.body {
                    gather_bit_aware_outputs(s, out);
                }
            }
            for s in &x.default {
                gather_bit_aware_outputs(s, out);
            }
        }
        ProtoStatement::For(x) => {
            for s in &x.body {
                gather_bit_aware_outputs(s, out);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                gather_bit_aware_outputs(s, out);
            }
        }
        ProtoStatement::CompiledBlock(x) => {
            if !x.original_stmts.is_empty() {
                // Match gather_variable_offsets: FF offsets written inside a
                // compiled block are event-time state, and keeping them here
                // would let `stable_topo_sort`'s RAW/WAR edges manufacture
                // false comb cycles through registers.
                let mut inner = vec![];
                for s in &x.original_stmts {
                    gather_bit_aware_outputs(s, &mut inner);
                }
                out.extend(inner.into_iter().filter(|(off, _)| !off.is_ff()));
            } else {
                for &off in &x.output_offsets {
                    if !off.is_ff() {
                        out.push((VarOffset::Comb(off.raw()), None));
                    }
                }
            }
        }
        _ => {}
    }
}

/// Diagnostic: compute strongly-connected components of the stmt-level
/// dataflow graph (stmt A → stmt B when A writes a variable B reads).
/// Returns (num_nontrivial_sccs, max_scc_size, total_stmts_in_sccs).
/// Enabled by VERYL_SCC_DIAG=1.
///
/// When VERYL_SCC_NARROW=1, uses the conservative base+last array
/// dependency encoding so that cycles formed only by array-element
/// overlap are filtered out — what remains is scalar comb cycles
/// that would be flagged by a logic synthesis tool.
///
/// When VERYL_SCC_BITAWARE=1, treats partial-width writes (via
/// Assign.select bit ranges) as independent edges: a write to x[7:4]
/// does not create an edge to readers of x[3:0].  This eliminates
/// SCCs formed only by bit-lane overlap in the VarOffset-level IR.
fn compute_scc_stats(sorted: &[ProtoStatement]) -> (usize, usize, usize) {
    use daggy::petgraph::Graph;
    use daggy::petgraph::algo::tarjan_scc;

    let n = sorted.len();
    if n == 0 {
        return (0, 0, 0);
    }

    // Gather per-stmt I/O. Expanded by default (captures per-element
    // array deps); narrow mode uses base+last (what synthesis tools see).
    let narrow = std::env::var("VERYL_SCC_NARROW").is_ok();
    let bitaware = std::env::var("VERYL_SCC_BITAWARE").is_ok();
    let fold = BigArrayFold::from_statements(sorted.iter());
    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_output_bits: Vec<Vec<(VarOffset, BitRange)>> = Vec::with_capacity(n);
    for s in sorted {
        let mut ins = vec![];
        let mut outs = vec![];
        if narrow {
            s.gather_variable_offsets(&mut ins, &mut outs);
        } else {
            s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
        }
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
        if bitaware {
            let mut bit_outs = vec![];
            gather_bit_aware_outputs(s, &mut bit_outs);
            stmt_output_bits.push(bit_outs);
        } else {
            stmt_output_bits.push(vec![]);
        }
    }

    // var → list of (writer stmt index, bit range for bit-aware mode).
    // In non-bitaware mode, bit_range is always None and overlap is trivial.
    let mut writers: HashMap<VarOffset, Vec<(usize, BitRange)>> = HashMap::default();
    if bitaware {
        for (i, outs) in stmt_output_bits.iter().enumerate() {
            for &(off, br) in outs {
                if off.is_ff() {
                    continue;
                }
                writers.entry(off).or_default().push((i, br));
            }
        }
    } else {
        for (i, outs) in stmt_outputs.iter().enumerate() {
            for &off in outs {
                if off.is_ff() {
                    continue;
                }
                writers.entry(off).or_default().push((i, None));
            }
        }
    }

    let mut graph: Graph<usize, ()> = Graph::new();
    let nodes: Vec<_> = (0..n).map(|i| graph.add_node(i)).collect();
    let mut edge_set: HashSet<(usize, usize)> = HashSet::default();
    // For bit-aware mode, we need to know the reader's bit range for this
    // offset. Currently ProtoExpression::Variable reads don't expose
    // per-field select in gather_variable_offsets, so we conservatively
    // treat reader ranges as None (= full width).  This still filters
    // out false cycles that arise from multiple writers on non-overlapping
    // bit slices, which is the common IR artifact.
    for (reader, ins) in stmt_inputs.iter().enumerate() {
        for &off in ins {
            if off.is_ff() {
                continue;
            }
            if let Some(ws) = writers.get(&off) {
                for &(w, wbr) in ws {
                    if w == reader {
                        continue;
                    }
                    if bitaware && !ranges_overlap(wbr, None) {
                        // Reader range is None (full-width) so this should
                        // never trigger, but kept for structural clarity.
                        continue;
                    }
                    if edge_set.insert((w, reader)) {
                        graph.add_edge(nodes[w], nodes[reader], ());
                    }
                }
            }
        }
    }

    let sccs = tarjan_scc(&graph);
    let mut nontrivial = 0usize;
    let mut max_size = 0usize;
    let mut total = 0usize;
    let mut size_hist: Vec<usize> = Vec::new();
    for scc in &sccs {
        if scc.len() > 1 {
            nontrivial += 1;
            total += scc.len();
            if scc.len() > max_size {
                max_size = scc.len();
            }
            size_hist.push(scc.len());
        }
    }
    size_hist.sort_unstable();
    size_hist.reverse();
    if nontrivial > 0 && std::env::var("VERYL_SCC_DIAG").is_ok() {
        let top: Vec<String> = size_hist.iter().take(10).map(|s| s.to_string()).collect();
        log::info!(
            "SCC stats: {} nontrivial SCCs, max={}, total_stmts_in_SCCs={}, top sizes=[{}]",
            nontrivial,
            max_size,
            total,
            top.join(", ")
        );
        // Position ranges of the largest SCCs to verify contiguity.
        let mut sccs_sorted = sccs.clone();
        sccs_sorted.sort_by_key(|scc| std::cmp::Reverse(scc.len()));
        for (i, scc) in sccs_sorted.iter().take(5).enumerate() {
            if scc.len() <= 1 {
                break;
            }
            let mut positions: Vec<usize> = scc.iter().map(|&idx| idx.index()).collect();
            positions.sort_unstable();
            let min_pos = positions[0];
            let max_pos = positions[positions.len() - 1];
            let range_span = max_pos - min_pos + 1;
            let density = scc.len() as f64 / range_span as f64;
            log::info!(
                "  SCC[{}]: size={}, position range=[{}..{}], span={}, density={:.2}",
                i,
                scc.len(),
                min_pos,
                max_pos,
                range_span,
                density
            );
        }

        // Count unique comb-output offsets written by the SCC stmts.
        // This tells us how narrow a snapshot/compare scope for
        // SCC-only convergence would be.
        let mut all_scc_outs: HashSet<VarOffset> = HashSet::default();
        for scc in &sccs {
            if scc.len() > 1 {
                for node in scc {
                    let idx = node.index();
                    for &off in &stmt_outputs[idx] {
                        if !off.is_ff() {
                            all_scc_outs.insert(off);
                        }
                    }
                }
            }
        }
        log::info!(
            "SCC comb outputs: {} unique offsets ({} stmts in SCCs)",
            all_scc_outs.len(),
            total
        );

        // Kind histogram + in/out offset histogram for the largest SCC.
        let largest = sccs_sorted.first().filter(|s| s.len() > 1);
        if let Some(scc) = largest {
            let mut kind_hist: HashMap<&'static str, usize> = HashMap::default();
            let mut out_counts: HashMap<VarOffset, usize> = HashMap::default();
            let mut in_counts: HashMap<VarOffset, usize> = HashMap::default();
            let mut source_hist: HashMap<String, usize> = HashMap::default();
            let mut line_samples: Vec<(String, u32)> = Vec::new();
            for node in scc {
                let idx = node.index();
                if let ProtoStatement::Assign(x) = &sorted[idx] {
                    let src = x.token.beg.source.to_string();
                    let line = x.token.beg.line;
                    *source_hist.entry(src.clone()).or_insert(0) += 1;
                    line_samples.push((src, line));
                }
            }
            let mut sources: Vec<_> = source_hist.into_iter().collect();
            sources.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            let src_str: Vec<String> = sources
                .iter()
                .take(15)
                .map(|(s, c)| format!("{}={}", s, c))
                .collect();
            log::info!("SCC[0] source file distribution: {}", src_str.join(", "));
            line_samples.sort();
            let uniq: Vec<String> = line_samples
                .iter()
                .take(10)
                .map(|(s, l)| format!("{}:{}", s, l))
                .collect();
            log::info!("SCC[0] first 10 (src:line): {:?}", uniq);

            for node in scc {
                let idx = node.index();
                let kind = match &sorted[idx] {
                    ProtoStatement::Assign(_) => "Assign",
                    ProtoStatement::AssignDynamic(_) => "AssignDynamic",
                    ProtoStatement::If(_) => "If",
                    ProtoStatement::Case(_) => "Case",
                    ProtoStatement::For(_) => "For",
                    ProtoStatement::Break => "Break",
                    ProtoStatement::SystemFunctionCall(_) => "SystemFunctionCall",
                    ProtoStatement::CompiledBlock(_) => "CompiledBlock",
                    ProtoStatement::SequentialBlock(_) => "SequentialBlock",
                    ProtoStatement::TbMethodCall { .. } => "TbMethodCall",
                };
                *kind_hist.entry(kind).or_insert(0) += 1;
                for &off in &stmt_outputs[idx] {
                    if !off.is_ff() {
                        *out_counts.entry(off).or_insert(0) += 1;
                    }
                }
                for &off in &stmt_inputs[idx] {
                    if !off.is_ff() {
                        *in_counts.entry(off).or_insert(0) += 1;
                    }
                }
            }
            let mut kinds: Vec<_> = kind_hist.into_iter().collect();
            kinds.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            let kstr: Vec<String> = kinds.iter().map(|(k, c)| format!("{}={}", k, c)).collect();
            log::info!("SCC[0] kind histogram: {}", kstr.join(", "));

            // Find offsets that are BOTH written and read many times within
            // the SCC — these are the "pivots" forming the cycles.
            let mut pivots: Vec<(VarOffset, usize, usize)> = out_counts
                .iter()
                .filter_map(|(&off, &wc)| in_counts.get(&off).map(|&rc| (off, wc, rc)))
                .collect();
            pivots.sort_by_key(|(_, wc, rc)| std::cmp::Reverse(wc * rc));
            log::info!("SCC[0] top pivots (offset, writers, readers):");
            for (off, wc, rc) in pivots.iter().take(10) {
                // Collect the bit ranges of writers to this offset within the SCC.
                let mut bit_writers: Vec<BitRange> = Vec::new();
                for node in scc {
                    let idx = node.index();
                    let mut outs: Vec<(VarOffset, BitRange)> = Vec::new();
                    gather_bit_aware_outputs(&sorted[idx], &mut outs);
                    for (w_off, br) in &outs {
                        if w_off == off {
                            bit_writers.push(*br);
                        }
                    }
                }
                let full_count = bit_writers.iter().filter(|b| b.is_none()).count();
                let partial_count = bit_writers.len() - full_count;
                let ranges: Vec<String> = bit_writers
                    .iter()
                    .filter_map(|b| b.map(|(hi, lo)| format!("[{}:{}]", hi, lo)))
                    .take(8)
                    .collect();
                log::info!(
                    "    {:?}: {} writers ({} full, {} partial), {} readers; partial ranges: {:?}",
                    off,
                    wc,
                    full_count,
                    partial_count,
                    rc,
                    ranges
                );
            }
        }
    }
    (nontrivial, max_size, total)
}

/// Build a map from comb VarOffset → human-readable variable path.
/// Walks ModuleVariableMeta recursively and records the offset of each
/// VariableElement's `current` slot together with the module hierarchy
/// prefix.
fn build_offset_path_map(meta: &ModuleVariableMeta) -> HashMap<VarOffset, String> {
    let mut map = HashMap::default();
    fn walk(meta: &ModuleVariableMeta, prefix: &str, out: &mut HashMap<VarOffset, String>) {
        let name = meta.name.to_string();
        let mod_prefix = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{}.{}", prefix, name)
        };
        for var_meta in meta.variable_meta.values() {
            let var_name = var_meta
                .path
                .0
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(".");
            for (i, element) in var_meta.elements.iter().enumerate() {
                let display = if var_meta.elements.len() > 1 {
                    format!("{}.{}[{}]", mod_prefix, var_name, i)
                } else {
                    format!("{}.{}", mod_prefix, var_name)
                };
                out.insert(element.current, display);
            }
        }
        for child in &meta.children {
            walk(child, &mod_prefix, out);
        }
    }
    walk(meta, "", &mut map);
    map
}

/// Diagnostic: trace a concrete cycle in the largest SCC of the comb
/// dataflow graph and print it as a sequence of variable names.
/// Helps pinpoint the exact combinational loop in source.
fn trace_scc_cycles(sorted: &[ProtoStatement], meta: &ModuleVariableMeta) {
    use daggy::petgraph::Graph;
    use daggy::petgraph::algo::tarjan_scc;

    let n = sorted.len();
    if n == 0 {
        return;
    }

    let path_map = build_offset_path_map(meta);

    // Build stmt-level dataflow graph (same as compute_scc_stats).
    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in sorted {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    let mut writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        for &off in outs {
            if off.is_ff() {
                continue;
            }
            writers.entry(off).or_default().push(i);
        }
    }

    // adj[reader] = list of writer-stmt indices whose outputs the reader reads.
    // (Edge direction in trace: reader ← writer, but for cycle finding we
    // walk writer → reader.)
    let mut adj: Vec<Vec<(usize, VarOffset)>> = vec![vec![]; n];
    let mut graph: Graph<usize, ()> = Graph::new();
    let nodes: Vec<_> = (0..n).map(|i| graph.add_node(i)).collect();
    let mut edge_set: HashSet<(usize, usize)> = HashSet::default();
    for (reader, ins) in stmt_inputs.iter().enumerate() {
        for &off in ins {
            if off.is_ff() {
                continue;
            }
            if let Some(ws) = writers.get(&off) {
                for &w in ws {
                    if w != reader && edge_set.insert((w, reader)) {
                        graph.add_edge(nodes[w], nodes[reader], ());
                        adj[w].push((reader, off));
                    }
                }
            }
        }
    }

    let sccs = tarjan_scc(&graph);
    let mut sccs_sorted = sccs.clone();
    sccs_sorted.sort_by_key(|s| std::cmp::Reverse(s.len()));

    for (scc_idx, scc) in sccs_sorted.iter().enumerate() {
        if scc.len() <= 1 {
            break;
        }
        let member_set: HashSet<usize> = scc.iter().map(|n| n.index()).collect();

        // Find a concrete cycle: BFS from the first member, through
        // edges confined to the SCC, find shortest path back to start.
        let start = scc[0].index();
        let mut parent: HashMap<usize, (usize, VarOffset)> = HashMap::default();
        let mut queue: std::collections::VecDeque<usize> = Default::default();
        queue.push_back(start);
        let mut found_back_to_start = None;
        while let Some(u) = queue.pop_front() {
            for &(v, off) in &adj[u] {
                if !member_set.contains(&v) {
                    continue;
                }
                if v == start {
                    // Found cycle: start → ... → u → (off) → start
                    found_back_to_start = Some((u, off));
                    break;
                }
                if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(v) {
                    e.insert((u, off));
                    queue.push_back(v);
                }
            }
            if found_back_to_start.is_some() {
                break;
            }
        }

        log::info!("SCC[{}] cycle trace (size={}):", scc_idx, scc.len());
        if let Some((last, last_off)) = found_back_to_start {
            // Rebuild path from start → last.
            let mut path: Vec<(usize, VarOffset)> = vec![(last, last_off)];
            let mut cur = last;
            while cur != start {
                if let Some(&(p, off)) = parent.get(&cur) {
                    path.push((p, off));
                    cur = p;
                } else {
                    break;
                }
            }
            path.reverse();
            let describe_offset = |off: VarOffset| -> String {
                path_map
                    .get(&off)
                    .cloned()
                    .unwrap_or_else(|| format!("{:?}", off))
            };
            let describe_stmt = |idx: usize| -> String {
                let (tok_beg, kind) = match &sorted[idx] {
                    ProtoStatement::Assign(x) => (Some(x.token.beg), "Assign"),
                    _ => (
                        None,
                        match &sorted[idx] {
                            ProtoStatement::If(_) => "If",
                            ProtoStatement::AssignDynamic(_) => "AssignDynamic",
                            ProtoStatement::For(_) => "For",
                            ProtoStatement::SequentialBlock(_) => "SeqBlock",
                            ProtoStatement::CompiledBlock(_) => "CompiledBlock",
                            ProtoStatement::SystemFunctionCall(_) => "SysFn",
                            ProtoStatement::TbMethodCall { .. } => "TbCall",
                            ProtoStatement::Break => "Break",
                            _ => "?",
                        },
                    ),
                };
                if let Some(tok) = tok_beg {
                    let src = tok.source.to_string();
                    let file = src.rsplit('/').next().unwrap_or(&src);
                    format!("[{}] {}:{}", kind, file, tok.line)
                } else {
                    format!("[{}] #{}", kind, idx)
                }
            };
            log::info!("  start at stmt {} ({})", start, describe_stmt(start));
            for (stmt_idx, via_off) in &path {
                log::info!(
                    "    ── writes {} ──→ stmt {} ({})",
                    describe_offset(*via_off),
                    stmt_idx,
                    describe_stmt(*stmt_idx)
                );
            }
            log::info!(
                "    ── writes {} ──→ back to start",
                describe_offset(last_off)
            );
        } else {
            log::info!("  (no cycle found from start; graph error?)");
        }

        // Also list the top pivot variables by name.
        let mut out_counts: HashMap<VarOffset, usize> = HashMap::default();
        let mut in_counts: HashMap<VarOffset, usize> = HashMap::default();
        for &idx in &member_set {
            for &off in &stmt_outputs[idx] {
                if !off.is_ff() {
                    *out_counts.entry(off).or_insert(0) += 1;
                }
            }
            for &off in &stmt_inputs[idx] {
                if !off.is_ff() {
                    *in_counts.entry(off).or_insert(0) += 1;
                }
            }
        }
        let mut pivots: Vec<(VarOffset, usize, usize)> = out_counts
            .iter()
            .filter_map(|(&off, &wc)| in_counts.get(&off).map(|&rc| (off, wc, rc)))
            .collect();
        pivots.sort_by_key(|(_, wc, rc)| std::cmp::Reverse(wc * rc));
        log::info!("  top pivot variables (by writers × readers):");
        for (off, wc, rc) in pivots.iter().take(10) {
            let name = path_map
                .get(off)
                .cloned()
                .unwrap_or_else(|| format!("{:?}", off));
            log::info!("    {}: {} writers, {} readers", name, wc, rc);
        }

        if scc_idx >= 3 {
            break; // Print at most a few SCCs.
        }
    }
}

/// Number of eval_comb passes needed for the comb list to converge.
///
/// A backward dataflow edge (a statement reads a value written later in
/// the sorted order) costs one extra pass; the result is the longest
/// backward-edge chain + 1 over the statement dependency graph.
fn compute_required_passes(sorted: &[ProtoStatement]) -> usize {
    use daggy::petgraph::Graph;
    use daggy::petgraph::algo::tarjan_scc;

    let n = sorted.len();
    if n == 0 {
        return 1;
    }

    if std::env::var("VERYL_SCC_DIAG").is_ok() {
        compute_scc_stats(sorted);
    }

    let mut writer_ranges: HashMap<VarOffset, Vec<(usize, BitRange)>> = HashMap::default();
    let mut full_writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (pos, stmt) in sorted.iter().enumerate() {
        let mut outs = vec![];
        gather_bit_aware_outputs(stmt, &mut outs);
        for (key, br) in outs {
            writer_ranges.entry(key).or_default().push((pos, br));
        }
        if let ProtoStatement::Assign(a) = stmt
            && a.select.is_none()
            && a.dynamic_select.is_none()
        {
            full_writers.entry(a.dst).or_default().push(pos);
        }
    }

    // Statement dataflow edges writer→reader.  Self-references and
    // non-overlapping writers are skipped; a BACKWARD edge is also dropped
    // when an unconditional full-width write earlier in the pass already
    // produced the value (the read is stable on re-execution).  Forward
    // edges stay: the read inherits whatever extra passes its producer
    // needs.
    let mut graph: Graph<usize, ()> = Graph::new();
    let nodes: Vec<_> = (0..n).map(|i| graph.add_node(i)).collect();
    let mut in_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    {
        let mut edge_set: HashSet<(usize, usize)> = HashSet::default();
        let mut ins = vec![];
        let mut outs = vec![];
        let mut reads = vec![];
        let mut output_set: HashSet<VarOffset> = HashSet::default();
        for (pos, stmt) in sorted.iter().enumerate() {
            ins.clear();
            outs.clear();
            stmt.gather_variable_offsets(&mut ins, &mut outs);
            output_set.clear();
            output_set.extend(outs.iter().cloned());
            reads.clear();
            stmt.gather_reads_with_ranges(&mut reads);

            for (key, rr) in &reads {
                if output_set.contains(key) {
                    continue;
                }
                let covered = full_writers
                    .get(key)
                    .is_some_and(|fw| fw.first().is_some_and(|&f| f < pos));
                if let Some(wranges) = writer_ranges.get(key) {
                    for (writer_pos, wr) in wranges {
                        if *writer_pos == pos
                            || (covered && *writer_pos > pos)
                            || !ranges_overlap(*wr, *rr)
                        {
                            continue;
                        }
                        if edge_set.insert((*writer_pos, pos)) {
                            graph.add_edge(nodes[*writer_pos], nodes[pos], ());
                            in_edges[pos].push(*writer_pos);
                        }
                    }
                }
            }
        }
    }

    // Delay D[stmt] = extra eval passes before its value settles: forward
    // in-edges cost 0, backward 1, and delay flows THROUGH forward edges
    // (a backward→forward→backward chain is depth 2, not 1).  Propagate
    // over the SCC condensation (tarjan_scc yields reverse topological
    // order, so the reversed walk visits writers first); within a
    // non-trivial SCC, seed from cross-SCC edges and scan backward edges
    // highest-position-first (positions strictly decrease along a backward
    // chain, so it terminates; intra-SCC forward hops stay uncounted).
    let sccs = tarjan_scc(&graph);
    let mut delay = vec![0usize; n];
    let mut members: HashSet<usize> = HashSet::default();
    let mut positions: Vec<usize> = Vec::new();
    for scc in sccs.iter().rev() {
        // Singleton SCCs (the common, acyclic case) stay allocation-free.
        if let [node] = scc.as_slice() {
            let r = node.index();
            let mut d = 0usize;
            for &w in &in_edges[r] {
                d = d.max(delay[w] + usize::from(w > r));
            }
            delay[r] = d;
            continue;
        }
        members.clear();
        members.extend(scc.iter().map(|nx| nx.index()));
        positions.clear();
        positions.extend(scc.iter().map(|nx| nx.index()));
        positions.sort_unstable();
        // Cross-SCC seeds.
        for &r in &positions {
            let mut d = 0usize;
            for &w in &in_edges[r] {
                if members.contains(&w) {
                    continue;
                }
                d = d.max(delay[w] + usize::from(w > r));
            }
            delay[r] = d;
        }
        // Intra-SCC backward chains, highest position first.
        for idx in (0..positions.len()).rev() {
            let r = positions[idx];
            for &w in &in_edges[r] {
                if members.contains(&w) && w > r {
                    delay[r] = delay[r].max(delay[w] + 1);
                }
            }
        }
    }

    let max_delay = delay.iter().copied().max().unwrap_or(0);
    // +1 safety margin when any backward chain exists: intra-SCC forward
    // hops stay uncounted (a terminating approximation, measured one pass
    // short on a real design), and no tight static bound exists for
    // value-dependent false-SCC convergence.  VERYL_MIN_PASSES_OVERRIDE
    // remains the escape hatch.
    let passes = if max_delay == 0 { 1 } else { max_delay + 2 };
    if passes > 1 {
        log::info!(
            "compute_required_passes: {} passes needed ({} stmts, {} backward edge chain depth)",
            passes,
            n,
            max_delay
        );
    }
    if passes > 1 && std::env::var("VERYL_PASS_DIAG").is_ok() {
        dump_backward_edge_chain(
            sorted,
            &delay,
            &in_edges,
            &writer_ranges,
            &full_writers,
            max_delay,
        );
    }
    // SCC iteration depth is computed only as a diagnostic — `passes`
    // returned above uses DAG depth alone.  Counting backward edges into
    // the pass total would penalise every `settle_comb` call on designs
    // with false SCCs (e.g. multi-driver array writes the IR can't
    // disambiguate), and the regression is severe for large-memory
    // designs where each extra full pass walks every comb byte.
    if std::env::var("VERYL_SCC_DIAG").is_ok() {
        let scc_depth = compute_scc_iteration_depth(sorted);
        log::info!("  (diagnostic) SCC iteration depth: {}", scc_depth);
    }
    passes
}

/// `VERYL_PASS_DIAG=1` diagnostic: aggregate backward-edge stats
/// (classified by whether the reader also has a PRIOR writer of the same
/// var — sequential reassignment vs a read-before-producer the sort
/// failed to order) plus one max-delay chain walk.
fn dump_backward_edge_chain(
    sorted: &[ProtoStatement],
    delay: &[usize],
    in_edges: &[Vec<usize>],
    writer_ranges: &HashMap<VarOffset, Vec<(usize, BitRange)>>,
    full_writers: &HashMap<VarOffset, Vec<usize>>,
    max_delay: usize,
) {
    let covered_by_prior_full = |key: &VarOffset, pos: usize| -> bool {
        full_writers
            .get(key)
            .is_some_and(|fw| fw.first().is_some_and(|&f| f < pos))
    };
    // Latest writer past `pos` whose range overlaps the read.
    let later_writer = |key: &VarOffset, rr: BitRange, pos: usize| -> Option<usize> {
        writer_ranges.get(key).and_then(|ws| {
            ws.iter()
                .rev()
                .take_while(|(p, _)| *p > pos)
                .find(|(_, wr)| ranges_overlap(*wr, rr))
                .map(|(p, _)| *p)
        })
    };
    let n = sorted.len();
    let mut stmt_reads: Vec<Vec<(VarOffset, BitRange)>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in sorted {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        stmt_outputs.push(outs);
        let mut reads = vec![];
        s.gather_reads_with_ranges(&mut reads);
        stmt_reads.push(reads);
    }

    let describe_stmt = |idx: usize| -> String {
        let (tok_beg, kind) = match &sorted[idx] {
            ProtoStatement::Assign(x) => (Some(x.token.beg), "Assign"),
            _ => (
                None,
                match &sorted[idx] {
                    ProtoStatement::If(_) => "If",
                    ProtoStatement::AssignDynamic(_) => "AssignDynamic",
                    ProtoStatement::For(_) => "For",
                    ProtoStatement::SequentialBlock(_) => "SeqBlock",
                    ProtoStatement::CompiledBlock(_) => "CompiledBlock",
                    ProtoStatement::SystemFunctionCall(_) => "SysFn",
                    ProtoStatement::TbMethodCall { .. } => "TbCall",
                    _ => "?",
                },
            ),
        };
        if let Some(tok) = tok_beg {
            let src = tok.source.to_string();
            let file = src.rsplit('/').next().unwrap_or(&src);
            format!("#{idx} [{kind}] {file}:{}", tok.line)
        } else {
            format!("#{idx} [{kind}]")
        }
    };

    // Aggregate: classify every backward edge.
    let mut total = 0usize;
    let mut with_prior = 0usize;
    for (pos, reads) in stmt_reads.iter().enumerate() {
        let output_set: HashSet<VarOffset> = stmt_outputs[pos].iter().cloned().collect();
        for (key, rr) in reads {
            if output_set.contains(key) || covered_by_prior_full(key, pos) {
                continue;
            }
            if later_writer(key, *rr, pos).is_some() {
                total += 1;
                if writer_ranges[key].iter().any(|(w, _)| *w < pos) {
                    with_prior += 1;
                }
            }
        }
    }
    log::info!(
        "pass_diag: {} backward edges total, {} with a prior writer (reassignment), {} without",
        total,
        with_prior,
        total - with_prior
    );

    // Walk one max-delay chain over the metric's own edges: pick an
    // in-edge whose writer accounts for the current delay (a backward hop
    // costs one pass, a forward hop inherits).  Step-bounded — forward
    // sub-chains can be arbitrarily long.
    let Some(mut pos) = (0..n).find(|&p| delay[p] == max_delay) else {
        return;
    };
    log::info!("pass_diag: longest chain (depth {max_delay}):");
    for _ in 0..64 {
        if delay[pos] == 0 {
            break;
        }
        let Some(&w) = in_edges[pos]
            .iter()
            .find(|&&w| delay[w] + usize::from(w > pos) == delay[pos])
        else {
            log::info!("  (chain broken at {})", describe_stmt(pos));
            return;
        };
        if w > pos {
            log::info!(
                "  {} (delay {}) <- written later by {} (delay {})",
                describe_stmt(pos),
                delay[pos],
                describe_stmt(w),
                delay[w],
            );
        } else {
            log::info!(
                "  {} (delay {}) <- forward from {}",
                describe_stmt(pos),
                delay[pos],
                describe_stmt(w),
            );
        }
        pos = w;
    }
    log::info!("  chain tail: {}", describe_stmt(pos));
}

/// Compute the max backward-edge chain depth inside any non-trivial SCC
/// of the comb dataflow graph. Returns 0 if no non-trivial SCCs exist.
///
/// Intuition: within an SCC, some edges must run "backward" in any topo
/// order (that's what makes it an SCC). The longest chain of such
/// backward edges is how many extra full passes the design needs to
/// settle the cycle.
fn compute_scc_iteration_depth(sorted: &[ProtoStatement]) -> usize {
    use daggy::petgraph::Graph;
    use daggy::petgraph::algo::tarjan_scc;

    let n = sorted.len();
    if n == 0 {
        return 0;
    }

    let fold = BigArrayFold::from_statements(sorted.iter());
    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in sorted {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets_expanded(&fold, &mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    // Build stmt-level DAG edges (comb writer → comb reader) and find
    // SCCs.
    let mut writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        for &off in outs {
            if !off.is_ff() {
                writers.entry(off).or_default().push(i);
            }
        }
    }

    let mut graph: Graph<usize, ()> = Graph::new();
    let nodes: Vec<_> = (0..n).map(|i| graph.add_node(i)).collect();
    let mut edge_set: HashSet<(usize, usize)> = HashSet::default();
    for (reader, ins) in stmt_inputs.iter().enumerate() {
        for &off in ins {
            if off.is_ff() {
                continue;
            }
            if let Some(ws) = writers.get(&off) {
                for &w in ws {
                    if w != reader && edge_set.insert((w, reader)) {
                        graph.add_edge(nodes[w], nodes[reader], ());
                    }
                }
            }
        }
    }

    let sccs = tarjan_scc(&graph);
    let mut max_depth = 0usize;

    for scc in &sccs {
        if scc.len() <= 1 {
            continue;
        }
        // SCC members and their original positions.
        let member_positions: Vec<usize> = scc.iter().map(|&idx| idx.index()).collect();
        let mut member_set: HashSet<usize> = HashSet::default();
        for &p in &member_positions {
            member_set.insert(p);
        }

        // Within the SCC subgraph (restricted to members and their
        // edges), compute backward-chain depth using the same algorithm
        // as the DAG case. Use original sorted position as the topo
        // order — this is a topo order of the whole graph but may
        // include many "backward" edges inside the SCC (which is
        // expected; that's what iteration resolves).
        let mut sorted_positions = member_positions.clone();
        sorted_positions.sort_unstable();

        // Build a writer map restricted to SCC members.
        let mut scc_writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
        for &p in &sorted_positions {
            for &off in &stmt_outputs[p] {
                if !off.is_ff() {
                    scc_writers.entry(off).or_default().push(p);
                }
            }
        }

        // Map original position → internal order (0, 1, 2, ...).
        let mut pos_to_ord: HashMap<usize, usize> = HashMap::default();
        for (ord, &p) in sorted_positions.iter().enumerate() {
            pos_to_ord.insert(p, ord);
        }

        let scc_n = sorted_positions.len();
        let mut delay = vec![0usize; scc_n];
        // Reverse scan by internal order.
        for ord in (0..scc_n).rev() {
            let p = sorted_positions[ord];
            let output_set: HashSet<VarOffset> = stmt_outputs[p].iter().cloned().collect();
            for key in &stmt_inputs[p] {
                if output_set.contains(key) {
                    continue;
                }
                if let Some(ws) = scc_writers.get(key) {
                    // A backward edge exists if any writer's internal
                    // order is strictly greater than this stmt's order.
                    for &wp in ws {
                        if wp == p {
                            continue;
                        }
                        if !member_set.contains(&wp) {
                            continue;
                        }
                        if let Some(&wo) = pos_to_ord.get(&wp)
                            && wo > ord
                        {
                            delay[ord] = delay[ord].max(delay[wo] + 1);
                        }
                    }
                }
            }
        }

        // Add 1 for the safety of propagation through the cycle head:
        // the reverse-scan counts "backward edges along longest path"
        // but the cycle head needs one extra iteration for its own
        // stale-input read to settle.  Empirically a real design needed
        // K_runtime = 4 where the marginless algo returns 3.
        let scc_max = delay.iter().copied().max().unwrap_or(0) + 1;
        if scc_max > max_depth {
            max_depth = scc_max;
        }
    }

    max_depth
}

/// Compute dependency levels for sorted ProtoStatements and reorder within
/// each level so that CompiledBlocks with the same func pointer are adjacent.
/// This enables batching of same-function JIT calls.
fn reorder_by_level(sorted: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    // Level = max(var_level[input]) + 1. For CBs, use all offsets
    // (including FF) since gather_variable_offsets filters FF for DAG.
    let mut var_level: HashMap<VarOffset, usize> = HashMap::default();
    // WAR/WAW guard: last level at which each comb var was read or written. A
    // re-writer must land strictly ABOVE it; otherwise pure-RAW leveling hoists
    // the re-write (`x=b`, reads only `b`) ahead of a reader of the old value
    // (`y=x`), re-introducing the reorder analyze_dependency just fixed. FF
    // offsets are excluded: written at event time, not the comb settle.
    let mut var_last_use: HashMap<VarOffset, usize> = HashMap::default();
    let mut levels: Vec<usize> = Vec::with_capacity(sorted.len());

    for stmt in &sorted {
        let mut inputs = vec![];
        let mut outputs = vec![];
        match stmt {
            ProtoStatement::CompiledBlock(x) => {
                if !x.stmt_deps.is_empty() {
                    for (ins, outs) in &x.stmt_deps {
                        inputs.extend_from_slice(ins);
                        outputs.extend_from_slice(outs);
                    }
                } else {
                    inputs.extend_from_slice(&x.input_offsets);
                    outputs.extend_from_slice(&x.output_offsets);
                }
            }
            _ => {
                stmt.gather_variable_offsets(&mut inputs, &mut outputs);
            }
        }

        let raw_level = inputs
            .iter()
            .filter_map(|key| var_level.get(key))
            .copied()
            .max()
            .map(|l| l + 1)
            .unwrap_or(0);
        let hazard_level = outputs
            .iter()
            .filter(|key| !key.is_ff())
            .filter_map(|key| var_last_use.get(key))
            .copied()
            .max()
            .map(|l| l + 1)
            .unwrap_or(0);
        let level = raw_level.max(hazard_level);

        for key in &outputs {
            let e = var_level.entry(*key).or_insert(0);
            if level > *e {
                *e = level;
            }
            if !key.is_ff() {
                let u = var_last_use.entry(*key).or_insert(level);
                if level > *u {
                    *u = level;
                }
            }
        }
        // A read marks the var as used at this level so a LATER writer (WAR)
        // is ordered after it.
        for key in &inputs {
            if !key.is_ff() {
                let u = var_last_use.entry(*key).or_insert(level);
                if level > *u {
                    *u = level;
                }
            }
        }

        levels.push(level);
    }

    // Group statements by level
    let max_level = levels.iter().copied().max().unwrap_or(0);
    let mut groups: Vec<Vec<ProtoStatement>> = vec![vec![]; max_level + 1];
    for (stmt, level) in sorted.into_iter().zip(levels) {
        groups[level].push(stmt);
    }

    // Within each level, topological sort by actual variable dependencies.
    for group in groups.iter_mut() {
        if group.len() <= 1 {
            continue;
        }
        *group = topo_sort_within_level(std::mem::take(group));
    }

    groups.into_iter().flatten().collect()
}

/// Local topological sort within a single level group.
///
/// Builds RAW dependency edges among the statements in this group and
/// performs a stable Kahn's-algorithm sort.  Statements with no intra-group
/// dependencies retain their original order (stable).  On cycle detection
/// the original order is preserved as a safe fallback.
fn topo_sort_within_level(stmts: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    let n = stmts.len();

    // Type priority for tie-breaking: CBs before Assigns when unordered.
    let type_priority: Vec<u8> = stmts
        .iter()
        .map(|s| match s {
            ProtoStatement::CompiledBlock(_) => 0,
            ProtoStatement::Assign(_) => 1,
            ProtoStatement::AssignDynamic(_) => 2,
            ProtoStatement::If(_) => 3,
            ProtoStatement::SequentialBlock(_) => 1,
            _ => 4,
        })
        .collect();

    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in &stmts {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    let mut var_writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        for key in outs {
            var_writers.entry(*key).or_default().push(i);
        }
    }

    // RAW edges: writer → reader (skip self-edges).
    let mut adj: Vec<HashSet<usize>> = vec![HashSet::default(); n];
    let mut in_degree = vec![0usize; n];
    for (reader, ins) in stmt_inputs.iter().enumerate() {
        for key in ins {
            if let Some(writers) = var_writers.get(key) {
                for &writer in writers {
                    if writer == reader {
                        continue; // skip self-edge
                    }
                    if adj[writer].insert(reader) {
                        in_degree[reader] += 1;
                    }
                }
            }
        }
    }

    // Kahn's with BTreeSet<(priority, index)> for stable tie-breaking.
    let mut queue: std::collections::BTreeSet<(u8, usize)> = std::collections::BTreeSet::new();
    for i in 0..n {
        if in_degree[i] == 0 {
            queue.insert((type_priority[i], i));
        }
    }

    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(&key) = queue.iter().next() {
        queue.remove(&key);
        let idx = key.1;
        order.push(idx);
        for &next in &adj[idx] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                queue.insert((type_priority[next], next));
            }
        }
    }

    if order.len() != n {
        let mut result = stmts;
        result.sort_by_key(|s| match s {
            ProtoStatement::CompiledBlock(_) => 0,
            ProtoStatement::Assign(_) => 1,
            ProtoStatement::AssignDynamic(_) => 2,
            ProtoStatement::If(_) => 3,
            ProtoStatement::SequentialBlock(_) => 1,
            _ => 4,
        });
        return result;
    }

    let mut indexed: Vec<Option<ProtoStatement>> = stmts.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|i| indexed[i].take().unwrap())
        .collect()
}

/// Cond-hoist transform.  See call site for rationale.  Walks `stmts`, and for
/// every top-level `If` whose cond is jittable but whose body contains a
/// non-jittable stmt (e.g. `$display`/`$write`), allocates a comb byte and
/// rewrites to `[Assign(temp = cond), If(temp, body)]`.  The Assign joins the
/// JIT chunk; the If stays interpreted but its cond is now a single byte load.
#[cfg(not(target_family = "wasm"))]
fn cond_hoist_transform(stmts: &mut Vec<ProtoStatement>, context: &mut Context) {
    use crate::ir::expression::ExpressionContext;
    use crate::ir::expression::ProtoExpression as PE;
    use crate::ir::native_bytes;
    use crate::ir::statement::{ProtoAssignStatement, ProtoIfStatement};
    use veryl_parser::token_range::TokenRange;
    let verbose = std::env::var("VERYL_COND_HOIST_VERBOSE").ok().as_deref() == Some("1");
    let mut hoisted = 0usize;
    let mut skipped_cond_nonjit = 0usize;
    let mut skipped_body_jit = 0usize;
    let original = std::mem::take(stmts);
    let mut result: Vec<ProtoStatement> = Vec::with_capacity(original.len() * 2);
    for stmt in original {
        match stmt {
            ProtoStatement::If(if_stmt) => {
                let body_jit = if_stmt.true_side.iter().all(|s| s.can_build_binary())
                    && if_stmt.false_side.iter().all(|s| s.can_build_binary());
                let cond_jit = match &if_stmt.cond {
                    Some(c) => c.can_build_binary(),
                    None => false,
                };
                if !cond_jit {
                    skipped_cond_nonjit += 1;
                    result.push(ProtoStatement::If(if_stmt));
                    continue;
                }
                if body_jit {
                    skipped_body_jit += 1;
                    result.push(ProtoStatement::If(if_stmt));
                    continue;
                }
                // Hoist: allocate a comb temp and split into Assign + If.
                let cond_expr = if_stmt.cond.clone().unwrap();
                let nb = native_bytes(1);
                let temp_offset = context.comb_total_bytes as isize;
                context.comb_total_bytes += value_size(nb, context.config.use_4state);
                let temp_off = VarOffset::Comb(temp_offset);
                let ctx = ExpressionContext {
                    width: 1,
                    signed: false,
                };
                let assign = ProtoStatement::Assign(ProtoAssignStatement {
                    dst: temp_off,
                    dst_width: 1,
                    select: None,
                    dynamic_select: None,
                    rhs_select: None,
                    expr: cond_expr,
                    dst_ff_current_offset: 0,
                    token: TokenRange::default(),
                });
                let new_cond = PE::Variable {
                    var_offset: temp_off,
                    select: None,
                    dynamic_select: None,
                    width: 1,
                    var_full_width: 1,
                    expr_context: ctx,
                };
                let new_if = ProtoStatement::If(ProtoIfStatement {
                    cond: Some(new_cond),
                    true_side: if_stmt.true_side,
                    false_side: if_stmt.false_side,
                });
                result.push(assign);
                result.push(new_if);
                hoisted += 1;
            }
            other => result.push(other),
        }
    }
    *stmts = result;
    if verbose && hoisted > 0 {
        eprintln!(
            "[CondHoist] hoisted={hoisted} skipped_cond_nonjit={skipped_cond_nonjit} \
             skipped_body_jit={skipped_body_jit}"
        );
    }
}

/// Merge consecutive Compiled statements with the same artifact into CompiledBatch.
fn batch_compiled_statements(stmts: Vec<Statement>) -> Vec<Statement> {
    let mut result: Vec<Statement> = Vec::with_capacity(stmts.len());

    for stmt in stmts {
        match stmt {
            Statement::Compiled(c) => match result.last_mut() {
                Some(Statement::CompiledBatch(batch))
                    if Arc::ptr_eq(&batch.artifact, &c.artifact) =>
                {
                    batch.args.push((c.ff, c.comb, c.ff_delta));
                }
                Some(Statement::Compiled(prev)) if Arc::ptr_eq(&prev.artifact, &c.artifact) => {
                    let batch = CompiledBatchStmt {
                        artifact: Arc::clone(&prev.artifact),
                        log_buf: prev.log_buf,
                        args: vec![
                            (prev.ff, prev.comb, prev.ff_delta),
                            (c.ff, c.comb, c.ff_delta),
                        ],
                    };
                    *result.last_mut().unwrap() = Statement::CompiledBatch(batch);
                }
                _ => {
                    result.push(Statement::Compiled(c));
                }
            },
            other => result.push(other),
        }
    }

    result
}

impl Conv<&air::Module> for ProtoModule {
    fn conv(context: &mut Context, src: &air::Module) -> Result<Self, SimulatorError> {
        // This conv is one test top (testbench).  Tag it so cross-test DUT
        // recurrence can be told apart from within-top replication (SMP harts).
        context.test_top_id = next_test_top_id();

        let mut analyzer_context = veryl_analyzer::conv::Context::default();
        analyzer_context.variables = src.variables.clone();
        analyzer_context.functions = src.functions.clone();

        let mut ff_table = src.ff_table.clone();
        if context.config.disable_ff_opt {
            ff_table.force_all_ff();
        }

        // Comb-to-FF hoist: clone declarations and mutate them — move
        // comb-side `let` writes into the consuming FF block, rebuild
        // the FfTable on the hoisted form so all downstream simulator
        // processing runs against the hoisted IR.
        let mut hoisted_declarations = src.declarations.clone();
        {
            let plans = veryl_analyzer::ir::comb_to_ff_hoist::plan_hoists(
                &hoisted_declarations,
                &ff_table,
                &src.variables,
            );
            veryl_analyzer::ir::comb_to_ff_hoist::apply_hoists(
                &mut hoisted_declarations,
                &plans,
                &src.variables,
            );
            ff_table = air::FfTable::default();
            for (i, x) in hoisted_declarations.iter().enumerate() {
                x.gather_ff(&mut analyzer_context, &mut ff_table, i);
            }
            ff_table.update_is_ff(&hoisted_declarations, &mut analyzer_context);
            if context.config.disable_ff_opt {
                ff_table.force_all_ff();
            }
        }
        let declarations: &[air::Declaration] = &hoisted_declarations;

        if ff_cacheline_pad_enabled() {
            let aligned = align_up_64(context.ff_total_bytes as isize);
            context.ff_total_bytes = aligned as usize;
        }
        let ff_start = context.ff_total_bytes as isize;
        let comb_start = context.comb_total_bytes as isize;

        // Analyzer-IR pre-pass to identify multi-RMW FFs.  Result drives
        // packed-aware allocation in `create_variable_meta` — FFs that
        // receive ≥2 writes per event need dual-slot storage (multi-RMW
        // chain forwarding), all others use packed single-slot layout
        // (dead next bytes eliminated, ff_values shrunk).
        let multi_rmw_set = analyze_multi_write(
            declarations,
            &mut analyzer_context,
            context.config.disable_ff_opt,
        );

        let dyn_indexed = collect_dyn_indexed_vars(declarations);

        let (mut variable_meta, ff_bytes, comb_bytes) = create_variable_meta(
            &src.variables,
            &ff_table,
            &multi_rmw_set,
            &dyn_indexed,
            context.config.use_4state,
            ff_start,
            comb_start,
        )?;

        context.ff_total_bytes += ff_bytes;
        context.comb_total_bytes += comb_bytes;

        let scope = ScopeContext {
            variable_meta: variable_meta.clone(),
            analyzer_context,
            ff_table: ff_table.clone(),
            inst_reset_kind: collect_inst_reset_kinds(declarations),
            func_offset_index: None,
        };
        context.scope_contexts.push(scope);

        let mut all_event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        let mut all_comb_statements: Vec<ProtoStatement> = vec![];
        let mut all_post_comb_fns: Vec<ProtoStatement> = vec![];
        let mut all_child_modules: Vec<ModuleVariableMeta> = vec![];
        let mut nested_derived_clock_candidates: Vec<(air::VarId, VarOffset, usize)> = vec![];
        let mut all_external_components: Vec<ProtoExternalComponent> = vec![];

        for decl in declarations {
            let mut proto_decl: ProtoDeclaration = Conv::conv(context, decl)?;

            for (event, mut stmts) in proto_decl.event_statements {
                all_event_statements
                    .entry(event)
                    .and_modify(|v| v.append(&mut stmts))
                    .or_insert(stmts);
            }
            // Move (not clone): `proto_decl` is dropped after this iteration, so
            // draining its comb list avoids a full deep-copy of the child subtree
            // — the DUT's bulk — on every test.
            all_comb_statements.append(&mut proto_decl.comb_statements);
            all_post_comb_fns.extend(proto_decl.post_comb_fns);
            all_child_modules.extend(proto_decl.child_modules);
            nested_derived_clock_candidates.extend(proto_decl.derived_clock_candidates);
            all_external_components.extend(proto_decl.external_components);
        }

        // Hierarchical testbench references need the complete child meta
        // tree; resolve them before any optimization or backend runs.
        crate::ir::hier_ref::resolve_hier_refs(
            context,
            &mut all_event_statements,
            &all_child_modules,
        )?;

        // Component input connections may also reference DUT internals.
        for external in &mut all_external_components {
            for connect in &mut external.connects {
                crate::ir::hier_ref::resolve_expr(&mut connect.expr, context, &all_child_modules)?;
                // A hierarchical clock/reset connection carries no VarId at
                // conv time; recover the event key from the resolved offset.
                // A nested derived clock matches its re-keyed candidate id;
                // a child port aliased onto a testbench variable matches the
                // top-scope variable that fires the event.
                if (connect.is_clock || connect.is_reset)
                    && connect.event_var.is_none()
                    && let crate::ir::expression::ProtoExpression::Variable {
                        var_offset,
                        select: None,
                        ..
                    } = &connect.expr
                {
                    connect.event_var = nested_derived_clock_candidates
                        .iter()
                        .find(|(_, offset, _)| offset == var_offset)
                        .map(|(vid, _, _)| *vid)
                        .or_else(|| {
                            context
                                .scope()
                                .variable_meta
                                .iter()
                                .find_map(|(vid, meta)| {
                                    (meta.elements.len() == 1
                                        && meta.elements[0].current == *var_offset)
                                        .then_some(*vid)
                                })
                        });
                }
            }
        }

        context.scope_contexts.pop();

        // Build unified comb list: execution-side only.
        // Merged-JIT children contribute their comb-only CB via `post_comb_fns`;
        // the originals are preserved inside each CB's `original_stmts` and
        // expanded on demand by `analyze_dependency` Phase 2 when fine-grained
        // ordering is needed.  This eliminates the false SCC artifact from
        // keeping both CB and its originals in the parent's `unified` list.
        let mut unified: Vec<ProtoStatement> = all_comb_statements
            .into_iter()
            .chain(all_post_comb_fns)
            .collect();

        // Baked inst-chunk artifacts would freeze their spans into rigid
        // units (see `expand_compiled_blocks`) — expand them BEFORE the key
        // so the memoised pipeline and every hit see the same statements.
        if comb_layout::enabled(context.config.use_4state) {
            comb_layout::expand_compiled_blocks(&mut unified);
            for stmts in all_event_statements.values_mut() {
                comb_layout::expand_compiled_blocks(stmts);
            }
        }
        let unified = unified;

        // Dead-var DCE protect set (also folded into the cache key): offsets
        // that must survive DCE.  `comb_to_ff_hoist` only rewrites `VarKind::Let`,
        // so the dead residue DCE targets is always Let-kind; user `var`s and
        // ports may have no in-module reader yet be live externally (parent port
        // wiring, or `Simulator::get_var` from a harness), and clock-typed lets
        // (derived clocks) are read only through `always_ff` sensitivity — both
        // are kept out of the candidate set.  Built here once so the key and the
        // miss-path pipeline share it.
        let dce_protect: HashSet<VarOffset> = if dead_var_dce::enabled() {
            use veryl_analyzer::ir::VarKind;
            let mut protect: HashSet<VarOffset> = HashSet::default();
            for (vid, var) in &src.variables {
                let is_let = matches!(var.kind, VarKind::Let);
                let is_clock = var.r#type.is_clock();
                if is_let && !is_clock {
                    continue;
                }
                if let Some(meta) = variable_meta.get(vid) {
                    for elem in &meta.elements {
                        protect.insert(elem.current);
                    }
                }
            }
            // Child-instance clock vars: only reader is `always_ff` sensitivity
            // (invisible to DCE); dropping the writer starves partial_settle.
            for (_, off, _) in &nested_derived_clock_candidates {
                protect.insert(*off);
            }
            // Component input connections read these offsets but appear in no
            // statement list.
            for external in &all_external_components {
                for connect in &external.connects {
                    let mut ins = vec![];
                    connect.expr.gather_variable_offsets(&mut ins);
                    protect.extend(ins);
                }
            }
            protect
        } else {
            HashSet::default()
        };

        // Comb relayout / fusion inputs: the offsets referenced outside any
        // statement list (external-component connects, nested derived-clock
        // candidates) — the relayout must not treat them as dead space, and
        // the fusion must not inline the defs they read.  Gathered while the
        // meta structures still hold the plain bump layout.  Folded into the
        // pipeline key below so a cache hit implies the same transforms.
        let aux_extra_offsets: Option<Vec<VarOffset>> =
            if comb_layout::enabled(context.config.use_4state)
                || comb_fusion::enabled(context.config.use_4state)
            {
                let mut extra_offsets: Vec<VarOffset> =
                    Vec::with_capacity(nested_derived_clock_candidates.len());
                for (_, off, _) in &nested_derived_clock_candidates {
                    extra_offsets.push(*off);
                }
                for external in &all_external_components {
                    for connect in &external.connects {
                        connect.expr.gather_variable_offsets(&mut extra_offsets);
                    }
                }
                Some(extra_offsets)
            } else {
                None
            };
        let layout_inputs: Option<comb_layout::LayoutInputs> =
            if comb_layout::enabled(context.config.use_4state) {
                let mut meta_units: Vec<(isize, isize)> = Vec::new();
                comb_layout::collect_meta_units_map(
                    &variable_meta,
                    context.config.use_4state,
                    &mut meta_units,
                );
                for child in &all_child_modules {
                    comb_layout::collect_meta_units_tree(
                        child,
                        context.config.use_4state,
                        &mut meta_units,
                    );
                }
                Some(comb_layout::LayoutInputs {
                    meta_units,
                    extra_offsets: aux_extra_offsets.clone().unwrap_or_default(),
                    comb_total: context.comb_total_bytes,
                })
            } else {
                None
            };

        // Whole comb pipeline (analyze_dependency + reorder + DCE + JIT),
        // memoised across tests that share a DUT.  A hit returns the pre-JIT
        // stmts, pass count, compiled comb, and the dead-var offset set — the
        // last re-applied to this test's events so they match the miss path
        // exactly (dead offsets are read nowhere, so the drop is value-neutral).
        // Single-flight (see `comb_pipeline_cache`); gated to `dut_reuse`.
        // Fusion and relayout rewrite offsets, so their inputs must flavor
        // the key: a shared base key would serve cached statements that
        // address a different comb layout.
        // Cone-gate inputs: node tables from the meta tree plus the
        // event-writable comb set.  `None` (also when the event writes cannot
        // be bounded) leaves the pipeline ungated.
        let cone_inputs: Option<cone_gate::ConeGateInputs> = if cone_gate::enabled() {
            collect_event_written_comb(&all_event_statements).map(|mut evt| {
                // External components write their output connects into comb
                // storage between settles — outside both the comb list and
                // the event statements.  Add every connect's variable offsets
                // (conservatively: inputs too) so a component write always
                // lands in some segment's compare set.
                let mut ins: Vec<VarOffset> = Vec::new();
                // The compare set is keyed on unfolded offsets.
                let unfolded = BigArrayFold::default();
                for ext in &all_external_components {
                    for c in &ext.connects {
                        ins.clear();
                        c.expr.gather_variable_offsets_expanded(&unfolded, &mut ins);
                        for o in &ins {
                            if let VarOffset::Comb(x) = o {
                                evt.insert(*x);
                            }
                        }
                    }
                }
                cone_gate::build_inputs(
                    &src.name.to_string(),
                    &variable_meta,
                    &all_child_modules,
                    &evt,
                    &context.comb_reloc,
                    context.config.use_4state,
                )
            })
        } else {
            None
        };
        let key = {
            let base = comb_pipeline_key(
                context.config.use_4state,
                &unified,
                &all_event_statements,
                &dce_protect,
            );
            if layout_inputs.is_some()
                || comb_fusion::enabled(context.config.use_4state)
                || cone_inputs.is_some()
            {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                comb_fusion::enabled(context.config.use_4state).hash(&mut h);
                cone_inputs.is_some().hash(&mut h);
                if let Some(extra) = &aux_extra_offsets {
                    extra.hash(&mut h);
                }
                if let Some(li) = &layout_inputs {
                    li.meta_units.hash(&mut h);
                    li.comb_total.hash(&mut h);
                }
                base ^ (h.finish() as u128)
            } else {
                base
            }
        };
        let cached: Arc<comb_pipeline_cache::CombPipeline> =
            match comb_pipeline_cache::try_get_or_claim(key, context.config.dut_reuse) {
                comb_pipeline_cache::Outcome::Hit(cached) => {
                    // The pipeline (incl. the in-place event DCE) did not run for
                    // this test; reproduce the dead-var drop on its events.
                    let dead: HashSet<VarOffset> = cached.dead_offsets.iter().copied().collect();
                    if !dead.is_empty() {
                        for stmts in all_event_statements.values_mut() {
                            let taken = std::mem::take(stmts);
                            *stmts = dead_var_dce::apply_counting(taken, &dead).0;
                        }
                    }
                    // ...nor did the version-split pass, whose rename temps the
                    // cached statements address.  A key match implies the same
                    // layout, so reserving the same span puts them back where
                    // the compiled code expects them.
                    context.comb_total_bytes += cached.vsplit_temp_bytes;
                    cached
                }
                // Compute (single-flight claim) or Disabled (reuse off): run the
                // pipeline once (it DCEs the events in place), then publish via
                // the guard or just wrap the result.
                other => {
                    let result = run_comb_pipeline(
                        context,
                        unified,
                        &mut all_event_statements,
                        &dce_protect,
                        layout_inputs.as_ref(),
                        aux_extra_offsets.as_deref(),
                        cone_inputs.as_ref(),
                        src.name,
                    )?;
                    match other {
                        comb_pipeline_cache::Outcome::Compute(guard) => guard.store(result),
                        _ => Arc::new(result),
                    }
                }
            };

        // Comb relayout replay: the pipeline rewrote (or the cache carries)
        // the memoised comb statements through the schedule; every
        // offset-bearing structure the pipeline does not own must follow —
        // this test's event statements, the variable meta tree (testbench
        // handles, derived clocks, localize blocklist and buffer fill all
        // read it), the nested derived-clock candidates, and the
        // external-component connect exprs.  `comb_total_bytes` advances to
        // the layout's end so later comb allocations (`cond_hoist_transform`)
        // stay clear of the packed region.
        if let Some(sched) = cached.layout.clone() {
            for stmts in all_event_statements.values_mut() {
                comb_layout::apply_to_stmts(stmts, &sched);
            }
            comb_layout::translate_meta_map(&mut variable_meta, &sched);
            for child in &mut all_child_modules {
                comb_layout::translate_meta_tree(child, &sched);
            }
            for (_, off, _) in &mut nested_derived_clock_candidates {
                *off = sched.translate_off(*off);
            }
            for external in &mut all_external_components {
                for connect in &mut external.connects {
                    connect.expr.remap_offsets_with(&|o| sched.translate_off(o));
                }
            }
            if sched.buffer_end > context.comb_total_bytes {
                context.comb_total_bytes = sched.buffer_end;
            }
        }
        // Cone-gate state region: per gated segment, flags + streak (8 bytes)
        // followed by its prerun/shadow/replay byte areas, appended past every
        // variable (and past the relayout's buffer_end) so nothing else can
        // land on it.  Zero-initialised buffers make `primed = 0` the natural
        // starting state, and per-instance buffers make the state safe under
        // instance reuse and concurrency.  Offsets are deterministic, so a
        // pipeline-cache hit recomputes the identical layout.
        let cone_segments: Vec<crate::ir::opt::cone_gate::ConeSegment> = {
            let mut segs: Vec<_> = cached.cone_segments.as_ref().clone();
            for s in &mut segs {
                let prerun: usize = s.backedge.iter().map(|&(a, b)| (b - a) as usize).sum();
                let pre: usize = s.compare_pre.iter().map(|&(a, b)| (b - a) as usize).sum();
                let shadow: usize = s.compare.iter().map(|&(_, a, b)| (b - a) as usize).sum();
                let replay: usize = s.replay.iter().map(|&(a, b)| (b - a) as usize).sum();
                let len = (8 + prerun + pre + shadow + replay).next_multiple_of(8);
                s.state_off = context.comb_total_bytes as u32;
                context.comb_total_bytes += len;
            }
            segs
        };
        // `pre_jit_stmts` is shared read-only downstream (Arc, no deep clone);
        // `comb_statements` is cloned into the ProtoModule (mostly `Arc::clone`s
        // of compiled chunks).
        let pre_jit_stmts = Arc::clone(&cached.pre_jit_stmts);
        let required_comb_passes = cached.required_comb_passes;
        let comb_statements = cached.comb_statements.clone();
        let nontrivial_comb_scc = cached.nontrivial_comb_scc;

        // Fusion-design census (`VERYL_FUSION_CENSUS=1`, diagnostic only):
        // per-comb-def reader-count distribution over the post-DCE statements,
        // to size what a full DFG contraction (inline single readers, delete
        // unobserved defs, keep multi-reader results) could remove.
        // `readers0` are defs alive only because of the DCE protect set.
        if std::env::var("VERYL_FUSION_CENSUS").as_deref() == Ok("1") {
            let mut readers: HashMap<VarOffset, usize> = HashMap::default();
            let mut defs: Vec<VarOffset> = Vec::new();
            let mut ins: Vec<VarOffset> = vec![];
            let mut outs: Vec<VarOffset> = vec![];
            for stmt in pre_jit_stmts.iter() {
                ins.clear();
                outs.clear();
                stmt.gather_variable_offsets(&mut ins, &mut outs);
                for off in ins.drain(..) {
                    if !off.is_ff() {
                        *readers.entry(off).or_insert(0) += 1;
                    }
                }
                if let ProtoStatement::Assign(a) = stmt
                    && !a.dst.is_ff()
                {
                    defs.push(a.dst);
                }
            }
            for stmts in all_event_statements.values() {
                for stmt in stmts {
                    ins.clear();
                    outs.clear();
                    stmt.gather_variable_offsets(&mut ins, &mut outs);
                    for off in ins.drain(..) {
                        if !off.is_ff() {
                            *readers.entry(off).or_insert(0) += 1;
                        }
                    }
                }
            }
            let (mut r0, mut r1, mut r2_4, mut r5p) = (0usize, 0, 0, 0);
            let (mut r0_prot, mut r1_prot) = (0usize, 0usize);
            for d in &defs {
                let n = readers.get(d).copied().unwrap_or(0);
                let prot = dce_protect.contains(d);
                match n {
                    0 => {
                        r0 += 1;
                        if prot {
                            r0_prot += 1;
                        }
                    }
                    1 => {
                        r1 += 1;
                        if prot {
                            r1_prot += 1;
                        }
                    }
                    2..=4 => r2_4 += 1,
                    _ => r5p += 1,
                }
            }
            eprintln!(
                "[fusion_census] module={:?} comb_stmts={} comb_defs={} \
                 readers0={} (protected {}) readers1={} (protected {}) \
                 readers2_4={} readers5plus={} protect_set={}",
                src.name,
                pre_jit_stmts.len(),
                defs.len(),
                r0,
                r0_prot,
                r1,
                r1_prot,
                r2_4,
                r5p,
                dce_protect.len(),
            );
        }

        // Top-level variables written by RTL (post-DCE), for the sole-driver
        // check on component outputs at load time.
        let rtl_driven: crate::HashSet<air::VarId> = if all_external_components.is_empty() {
            crate::HashSet::default()
        } else {
            let mut write_offsets: crate::HashSet<VarOffset> = crate::HashSet::default();
            let mut ins = vec![];
            let mut outs = vec![];
            for (event, stmts) in &all_event_statements {
                // Testbench-side writes (e.g. initialization in `initial`)
                // are not RTL drivers; a component may drive such a
                // variable.
                if matches!(event, Event::Initial | Event::Final) {
                    continue;
                }
                for stmt in stmts {
                    ins.clear();
                    outs.clear();
                    stmt.gather_variable_offsets(&mut ins, &mut outs);
                    write_offsets.extend(outs.drain(..));
                }
            }
            for stmt in pre_jit_stmts.iter() {
                ins.clear();
                outs.clear();
                stmt.gather_variable_offsets(&mut ins, &mut outs);
                write_offsets.extend(outs.drain(..));
            }
            variable_meta
                .iter()
                .filter(|(_, meta)| {
                    meta.elements
                        .iter()
                        .any(|e| write_offsets.contains(&e.current))
                })
                .map(|(vid, _)| *vid)
                .collect()
        };

        // Cond-hoist transform (disable with VERYL_COND_HOIST_DISABLE=1):
        // For each top-level `if cond { body }` whose body contains a
        // non-jittable stmt (e.g. `$display`/`$write`) but whose cond is
        // jittable, allocate a 1-bit comb temp and rewrite to
        //     temp = cond ? 1 : 0     (joins the JIT chunk)
        //     if temp != 0 { body }   (interp with a cheap byte-load cond)
        // so the per-cycle cond evaluation runs in JIT instead of through
        // the interpreter (Expression::eval + Op::eval_value_binary +
        // read_native_value etc).
        #[cfg(not(target_family = "wasm"))]
        {
            let cond_hoist_disabled =
                std::env::var("VERYL_COND_HOIST_DISABLE").ok().as_deref() == Some("1");
            if !cond_hoist_disabled {
                for stmts in all_event_statements.values_mut() {
                    cond_hoist_transform(stmts, context);
                }
            }
        }

        // Build FF write site_table from pre-JIT event ProtoStatements.
        // Walks all events before try_jit consumes them; FF writes only
        // appear inside event scopes (always_ff blocks).
        let mut site_table = SiteTable::new();
        for stmts in all_event_statements.values() {
            site_table.extend_from_protos(stmts);
        }

        if std::env::var("VERYL_SITE_TABLE_DIAG").ok().as_deref() == Some("1") {
            eprintln!(
                "[site_table_diag] module={:?} sites={}",
                src.name,
                site_table.len(),
            );
        }

        // Chunk-local localization (gated by `emit::localize_enabled`): while
        // events + derived-clock candidates are in scope, precompute the comb
        // offsets the emitter must NOT localize — event-touched, in a
        // runtime-indexed array range, or externally-visible (port / user-var
        // / clock).
        // LocalizeInfo = (blocklist offsets, array ranges).
        type LocalizeInfo = (HashSet<isize>, Vec<(isize, usize, isize)>);
        // Wasm registers no AOT-C backend, so there is nothing to localize;
        // the binding still exists so the whole-comb call site is uniform.
        #[cfg(target_family = "wasm")]
        let localize_info: Option<LocalizeInfo> = None;
        #[cfg(not(target_family = "wasm"))]
        let localize_info: Option<LocalizeInfo> = if crate::backend::aot_c::emit::localize_enabled()
        {
            let event_slices: Vec<&[ProtoStatement]> = all_event_statements
                .values()
                .map(|v| v.as_slice())
                .collect();
            let (mut block_vo, ranges) =
                crate::ir::opt::dead_var_dce::collect_localize_info(&pre_jit_stmts, &event_slices);
            // Protect externally-visible comb offsets (mirrors the
            // dead_var_dce protect set): a parent module's comb or a
            // testbench reads these from comb_values, bypassing any local.
            use veryl_analyzer::ir::VarKind;
            for (vid, var) in &src.variables {
                let is_let = matches!(var.kind, VarKind::Let);
                let is_clock = var.r#type.is_clock();
                if is_let && !is_clock {
                    continue;
                }
                if let Some(meta) = variable_meta.get(vid) {
                    for elem in &meta.elements {
                        block_vo.insert(elem.current);
                    }
                }
            }
            for (_, off, _) in &nested_derived_clock_candidates {
                block_vo.insert(*off);
            }
            let mut block: HashSet<isize> = HashSet::default();
            for vo in &block_vo {
                if !vo.is_ff() {
                    block.insert(vo.raw());
                }
            }
            Some((block, ranges))
        } else {
            None
        };

        // Const-cone split input (see `collect_event_written_comb`);
        // `None` leaves the split unarmed.
        let const_unsafe_comb: Option<HashSet<isize>> =
            collect_event_written_comb(&all_event_statements);

        // Diag: count FF offsets with multiple write sites — the
        // "multi-RMW candidates" that require scratch/cache forwarding
        // to preserve NBA semantics under packed [current]-only layout.
        // The remaining (single-site) FFs need no scratch at all.
        //
        // The "simple" count is an upper bound that includes if-else
        // mutual-exclusion sites.  The "true" multi-RMW count enumerates
        // execution paths through the AST and tallies writes per path —
        // sites under disjoint If branches collapse, sites that fire
        // together accumulate.  Path-aware result is the actual scratch
        // requirement.
        if std::env::var("VERYL_FF_MULTI_WRITE_DIAG").ok().as_deref() == Some("1") {
            use std::collections::BTreeMap;
            let mut by_offset: BTreeMap<u32, Vec<&SiteInfo>> = BTreeMap::new();
            for s in &site_table.sites {
                by_offset.entry(s.current_offset).or_default().push(s);
            }
            let total_offsets = by_offset.len();
            let upper_single = by_offset.values().filter(|v| v.len() == 1).count();
            let upper_multi = by_offset.values().filter(|v| v.len() > 1).count();
            let total_sites = site_table.len();

            // Path-aware: per-FF max-writes-in-any-execution-path.
            // Uses O(tree_size) recursive analysis instead of 2^n path
            // enumeration.  An offset with max_writes ≥ 2 is "true
            // multi-RMW" — requires scratch/cache forwarding.
            let mut true_multi: std::collections::BTreeSet<u32> =
                std::collections::BTreeSet::default();
            let mut max_chain_per_offset: BTreeMap<u32, u32> = BTreeMap::new();
            for stmts in all_event_statements.values() {
                let mw = collect_max_writes(stmts);
                for (off, cnt) in mw {
                    if cnt >= 2 {
                        true_multi.insert(off);
                    }
                    let e = max_chain_per_offset.entry(off).or_insert(0);
                    if cnt > *e {
                        *e = cnt;
                    }
                }
            }
            let true_multi_bytes: u32 = true_multi
                .iter()
                .filter_map(|off| by_offset.get(off).and_then(|v| v.first()))
                .map(|s| s.native_bytes as u32)
                .sum();
            eprintln!(
                "[ff_multi_write_diag] module={:?} total_offsets={} sites={} \
                 upper_single={} upper_multi={} true_multi={} true_multi_bytes={}",
                src.name,
                total_offsets,
                total_sites,
                upper_single,
                upper_multi,
                true_multi.len(),
                true_multi_bytes,
            );
            let mut multi_list: Vec<_> = max_chain_per_offset
                .iter()
                .filter(|(_, c)| **c >= 2)
                .collect();
            multi_list.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
            for (off, chain) in multi_list.iter().take(10) {
                let width = by_offset
                    .get(off)
                    .and_then(|v| v.first())
                    .map(|s| s.width_bits)
                    .unwrap_or(0);
                let kind = by_offset
                    .get(off)
                    .and_then(|v| v.first())
                    .map(|s| s.kind)
                    .unwrap_or(SiteKind::Static);
                eprintln!(
                    "  off=0x{:x} max_chain={} width_bits={} kind={:?}",
                    off, chain, width, kind,
                );
            }
        }

        // Fuse the per-block `if_reset` dispatches of one reset net, so a
        // module's clock event pays one test instead of one per `always_ff`.
        {
            let mut reset_offsets: crate::HashSet<VarOffset> = crate::HashSet::default();
            collect_reset_offsets(&variable_meta, &mut reset_offsets);
            for child in &all_child_modules {
                collect_reset_offsets_recursive(child, &mut reset_offsets);
            }
            if !reset_offsets.is_empty() {
                // Clock events only: a testbench `initial` may assign a
                // reset-typed variable, making the condition mutable between
                // two dispatches.  `always_ff` cannot (`invalid_clock_assignment`).
                for (event, stmts) in all_event_statements.iter_mut() {
                    if matches!(event, Event::Clock(_)) {
                        merge_reset_dispatch(stmts, &reset_offsets);
                    }
                }
            }
        }

        // AOT-C event path: compile each event's FF-next + write-log to C,
        // keyed by Event.  `prepare_event` returns None on any uncovered stmt,
        // so the map holds only fully-emittable events; the rest stay on
        // Cranelift.  Built before `all_event_statements` is consumed below.
        // Only engage whole-module backends on big-enough modules — see
        // Config::aot_c_min_stmts.  Below threshold, per-chunk Cranelift
        // wins on compile latency.
        let size_ok = {
            let n = pre_jit_stmts.len()
                + all_event_statements
                    .values()
                    .map(|v| v.len())
                    .sum::<usize>();
            n >= context.config.aot_c_min_stmts
        };
        let whole_events: HashMap<Event, Arc<dyn CompiledWhole>> = if !size_ok {
            HashMap::default()
        } else {
            let ctx = CompileCtx {
                config: &context.config,
                use_4state: context.config.use_4state,
                contains_compiled_block: false,
            };
            let mut map = HashMap::default();
            for (event, stmts) in all_event_statements.iter() {
                if let Some(whole) = context.backends.try_compile_whole_event(&ctx, event, stmts) {
                    map.insert(event.clone(), whole);
                }
            }
            map
        };

        if std::env::var("VERYL_AOT_C_EVENT_DIAG").as_deref() == Ok("1") {
            for (event, stmts) in all_event_statements.iter() {
                eprintln!(
                    "[aot_event_module] module={:?} event={:?} top_stmts={} aot_c={}",
                    src.name,
                    event,
                    stmts.len(),
                    whole_events.contains_key(event),
                );
                // Census of EVERY uncovered statement, not just the first, so
                // one fix does not simply surface the next bail.
                #[cfg(not(target_family = "wasm"))]
                if !whole_events.contains_key(event) {
                    let census = crate::backend::aot_c::emit::event_uncovered_census(stmts);
                    let mut counts: HashMap<String, usize> = Default::default();
                    for c in census {
                        *counts.entry(c).or_default() += 1;
                    }
                    let mut v: Vec<_> = counts.into_iter().collect();
                    v.sort_by_key(|x| std::cmp::Reverse(x.1));
                    eprintln!(
                        "[aot_event_census] {} distinct uncovered event stmts:",
                        v.len()
                    );
                    for (k, n) in v.iter().take(40) {
                        eprintln!("  {n:6}x  {k}");
                    }
                }
            }
        }

        // Event statements preserve source order (no topological sorting).
        // NBA semantics: reads come from current, writes go to next, then
        // ff_commit copies next → current. Source order must be preserved
        // for sequential writes to the same variable.
        let comb_touched_offsets = Arc::new(collect_comb_touched_offsets(&pre_jit_stmts));
        // No chunk backend on wasm, so the pre-chunking below would be an
        // identity transform.
        #[cfg(not(target_family = "wasm"))]
        let tb_private = tb_private_offsets(&variable_meta, &comb_touched_offsets);
        let event_statements: HashMap<Event, ProtoStatements> = all_event_statements
            .into_iter()
            .map(|(event, stmts)| {
                #[cfg(not(target_family = "wasm"))]
                let stmts = if event == Event::Initial {
                    precompile_tb_bodies(context, stmts, &tb_private)
                } else {
                    stmts
                };
                (event, try_jit(context, stmts))
            })
            .collect();

        // Collect derived clocks + input-clock offsets BEFORE
        // `variable_meta` moves into `module_variable_meta`.  We must look
        // at both the top module's own clock vars AND any clock vars
        // bubbled up from child instances (e.g. `var w_gclk: '_ clock;`
        // declared inside a sub-module): without the nested ones, a
        // testbench top whose DUT contains the gated clock would have an
        // empty `derived_clock_schedule` and `always_ff(w_gclk, ...)`
        // would never fire.
        let has_any_clock_var = src.variables.values().any(|v| v.r#type.is_clock())
            || !nested_derived_clock_candidates.is_empty();
        let (derived_clock_vars, input_clock_offsets) = if !has_any_clock_var {
            (Vec::new(), HashMap::default())
        } else {
            // O(V+P) port lookup via HashSet.
            let port_var_set: crate::HashSet<VarId> = src.ports.values().copied().collect();
            let mut dc_vars: Vec<(VarId, VarOffset, usize)> = src
                .variables
                .iter()
                .filter(|(vid, var)| var.r#type.is_clock() && !port_var_set.contains(*vid))
                .filter_map(|(vid, _)| {
                    let meta = variable_meta.get(vid)?;
                    let elem = meta.elements.first()?;
                    Some((*vid, elem.current, elem.native_bytes))
                })
                .collect();
            // Nested candidates already carry absolute (parent-rebased)
            // offsets and exclude child ports.  Dedup by (var_id, offset)
            // so a clock-typed input port re-exported through aliasing
            // can't be added twice.
            let mut seen: crate::HashSet<(VarId, VarOffset)> =
                dc_vars.iter().map(|(v, o, _)| (*v, *o)).collect();
            for (vid, off, nb) in nested_derived_clock_candidates.drain(..) {
                if seen.insert((vid, off)) {
                    dc_vars.push((vid, off, nb));
                }
            }
            // "Input clock" candidates: top-module clock-typed variables
            // whose value the simulator needs to drive directly so the
            // gated-clock expression sees a rising edge.  Historically
            // restricted to input ports, but testbench tops drive the
            // clock through `inst clk: $tb::clock_gen` (a non-port inst
            // output that's still a clock-typed top-level var), so
            // collect every clock-typed top var that has a backing
            // storage entry — `set_input_clock_bit` only walks the
            // top-level `module_variables.variables` map.
            let mut pc_offsets: HashMap<VarOffset, VarId> = HashMap::default();
            for (vid, var) in &src.variables {
                if !var.r#type.is_clock() {
                    continue;
                }
                if let Some(meta) = variable_meta.get(vid)
                    && let Some(elem) = meta.elements.first()
                {
                    pc_offsets.insert(elem.current, *vid);
                }
            }
            (dc_vars, pc_offsets)
        };

        let module_variable_meta = ModuleVariableMeta {
            name: src.name,
            hierarchy: vec![],
            variable_meta,
            children: all_child_modules,
        };

        let inst_layout = InstLayout::build_from_top(&module_variable_meta);
        debug_assert!(
            inst_layout.ranges_disjoint(),
            "inst_layout: top-level Inst FF ranges overlap (module={:?})",
            src.name,
        );
        if std::env::var("VERYL_INST_LAYOUT_DIAG").ok().as_deref() == Some("1") {
            eprintln!(
                "[inst_layout_diag] module={:?} insts={} disjoint={}",
                src.name,
                inst_layout.len(),
                inst_layout.ranges_disjoint(),
            );
            for r in &inst_layout.ranges {
                eprintln!(
                    "  inst={:?} ff_range=[{}..{}) bytes={}",
                    r.name,
                    r.ff_start,
                    r.ff_end,
                    r.ff_end - r.ff_start,
                );
            }
        }

        if std::env::var("VERYL_SCC_TRACE").is_ok() {
            trace_scc_cycles(&pre_jit_stmts, &module_variable_meta);
        }

        // Derived-clock eval is a separate `try_jit` chunk so the main
        // comb JIT/AOT-C blob stays intact while partial_settle is fast.
        let (derived_clock_schedule, derived_clock_eval) = if derived_clock_vars.is_empty() {
            (DerivedClockSchedule::default(), ProtoStatements(vec![]))
        } else {
            let (sched, eval_indices) = build_derived_clock_schedule(
                &derived_clock_vars,
                &pre_jit_stmts,
                &input_clock_offsets,
            );
            let eval_protos = extract_eval_proto_stmts(&eval_indices, &pre_jit_stmts);
            let eval = try_jit(context, eval_protos);
            (sched, eval)
        };

        // Whole-comb backend (today: AOT-C) — when registered + size_ok,
        // try compile_whole_comb; backends that decline (4-state,
        // unsupported construct) return None and Ir::settle_comb stays
        // on the per-chunk Cranelift loop.
        let dut_reuse = context.config.dut_reuse;
        let whole_comb: Option<Arc<dyn CompiledWhole>> = if !size_ok {
            None
        } else {
            // Memoise the whole-comb compile by the same structural `key` as the
            // comb pipeline: a shared DUT's C is emitted + fingerprinted once,
            // not per test (the emit is the dominant per-test build cost at
            // suite scale, and pure waste when the backend declines).
            whole::compile_whole_comb(
                &mut context.backends,
                &context.config,
                key,
                dut_reuse,
                &pre_jit_stmts,
                whole::WholeCombShape {
                    localize: localize_info.as_ref(),
                    const_unsafe: const_unsafe_comb.as_ref(),
                    cone_segments: &cone_segments,
                },
            )
        };

        // A whole-comb bail silently drops the whole module to per-chunk
        // dispatch — a perf regression with no other signal.  Each backend
        // exposes its own diagnostic gate (today: VERYL_AOT_C_DIAG); the
        // registry returns the first non-None diagnostic.
        if size_ok
            && whole_comb.is_none()
            && let Some(reason) = context
                .backends
                .diagnose_whole_comb_fallback(&pre_jit_stmts)
        {
            eprintln!(
                "[whole_comb] module {} fell back to per-chunk dispatch: {}",
                src.name, reason,
            );
            // Census of ALL uncovered comb stmts (not just the first) so a
            // single fix doesn't just surface the next bail.  AOT-C (and its
            // census) is gated off on wasm, like `backend::aot_c`.
            #[cfg(not(target_family = "wasm"))]
            if std::env::var("VERYL_AOT_C_DIAG").as_deref() == Ok("1") {
                let census = crate::backend::aot_c::emit::comb_uncovered_census(&pre_jit_stmts);
                let mut counts: HashMap<String, usize> = Default::default();
                for c in census {
                    *counts.entry(c).or_default() += 1;
                }
                let mut v: Vec<_> = counts.into_iter().collect();
                v.sort_by_key(|x| std::cmp::Reverse(x.1));
                eprintln!(
                    "[whole_comb_census] module {} uncovered comb stmts ({} distinct):",
                    src.name,
                    v.len()
                );
                for (k, n) in v.iter().take(40) {
                    eprintln!("  {n:6}x  {k}");
                }
            }
        }

        Ok(ProtoModule {
            name: src.name,
            ports: src.ports.clone(),
            ff_bytes: context.ff_total_bytes,
            comb_bytes: context.comb_total_bytes,
            use_4state: context.config.use_4state,
            module_variable_meta,
            event_statements,
            comb_statements,
            required_comb_passes,
            site_table,
            inst_layout,
            derived_clock_schedule,
            derived_clock_eval,
            nontrivial_comb_scc,
            whole_comb,
            whole_events,
            external_components: all_external_components,
            rtl_driven,
            fused_comb_offsets: cached.fused_offsets.clone(),
            cone_segments,
            comb_touched_offsets,
        })
    }
}

/// ProtoStatement-walking per-FF-offset max-writes-in-any-execution-path
/// analysis.  Used by the `VERYL_FF_MULTI_WRITE_DIAG=1` diag block (above)
/// to corroborate the analyzer-IR multi_write_analysis result against the
/// post-build ProtoStatement view.  Not on the hot path.
fn collect_max_writes(stmts: &[ProtoStatement]) -> HashMap<u32, u32> {
    let mut acc: HashMap<u32, u32> = HashMap::default();
    for s in stmts {
        let sub = collect_max_writes_one(s);
        for (off, n) in sub {
            *acc.entry(off).or_insert(0) += n;
        }
    }
    acc
}

fn collect_max_writes_one(stmt: &ProtoStatement) -> HashMap<u32, u32> {
    use crate::ir::statement::ProtoStatement as P;
    let mut result: HashMap<u32, u32> = HashMap::default();
    match stmt {
        P::Assign(a) if a.dst.is_ff() => {
            result.insert(a.dst_ff_current_offset as u32, 1);
        }
        P::AssignDynamic(a) if a.dst_base.is_ff() => {
            result.insert(a.dst_ff_current_base_offset as u32, 1);
        }
        P::If(i) => {
            let t = collect_max_writes(&i.true_side);
            let f = collect_max_writes(&i.false_side);
            for (off, n) in &t {
                let e = result.entry(*off).or_insert(0);
                if *n > *e {
                    *e = *n;
                }
            }
            for (off, n) in &f {
                let e = result.entry(*off).or_insert(0);
                if *n > *e {
                    *e = *n;
                }
            }
        }
        P::Case(c) => {
            // Only one arm (or the default) executes, so the per-offset write
            // count is the max across all arms and the default (as for `If`).
            let branches = c
                .arms
                .iter()
                .map(|arm| collect_max_writes(&arm.body))
                .chain(std::iter::once(collect_max_writes(&c.default)));
            for branch in branches {
                for (off, n) in &branch {
                    let e = result.entry(*off).or_insert(0);
                    if *n > *e {
                        *e = *n;
                    }
                }
            }
        }
        P::For(f) => {
            return collect_max_writes(&f.body);
        }
        P::SequentialBlock(body) => {
            return collect_max_writes(body);
        }
        _ => {}
    }
    result
}

/// `(active low, synchronous)` per module-local net, from the instance ports
/// it is wired to.  `None` records a wire whose ports disagree, which no
/// single level satisfies.
pub(crate) fn collect_inst_reset_kinds(
    declarations: &[air::Declaration],
) -> HashMap<air::VarId, Option<(bool, bool)>> {
    let mut out: HashMap<air::VarId, Option<(bool, bool)>> = HashMap::default();
    for decl in declarations {
        let air::Declaration::Inst(inst) = decl else {
            continue;
        };
        let air::Component::Module(child) = inst.component.as_ref() else {
            continue;
        };
        for (port_id, expr) in inst
            .inputs
            .iter()
            .filter_map(|x| x.single().map(|e| (x.id, e)))
        {
            let Some(kind) = child
                .variables
                .get(&port_id)
                .and_then(|v| declared_reset_kind(&v.r#type.kind))
            else {
                continue;
            };
            let air::Expression::Term(factor) = expr else {
                continue;
            };
            let air::Factor::Variable(net, idx, sel, _) = factor.as_ref() else {
                continue;
            };
            if !idx.0.is_empty() || !sel.is_empty() {
                continue;
            }
            record(&mut out, *net, kind);
        }
        // A reset the child DRIVES (a release synchroniser) decides it too.
        for output in &inst.outputs {
            let Some(kind) = child
                .variables
                .get(&output.id)
                .and_then(|v| declared_reset_kind(&v.r#type.kind))
            else {
                continue;
            };
            for dst in &output.dst {
                if dst.index.0.is_empty() && dst.select.is_empty() {
                    record(&mut out, dst.id, kind);
                }
            }
        }
    }
    out
}

fn record(
    out: &mut HashMap<air::VarId, Option<(bool, bool)>>,
    net: air::VarId,
    kind: (bool, bool),
) {
    out.entry(net)
        .and_modify(|e| {
            if *e != Some(kind) {
                *e = None;
            }
        })
        .or_insert(Some(kind));
}

/// `(active low, synchronous)` of a declared reset type.
fn declared_reset_kind(kind: &air::TypeKind) -> Option<(bool, bool)> {
    match kind {
        air::TypeKind::ResetAsyncLow => Some((true, false)),
        air::TypeKind::ResetAsyncHigh => Some((false, false)),
        air::TypeKind::ResetSyncLow => Some((true, true)),
        air::TypeKind::ResetSyncHigh => Some((false, true)),
        _ => None,
    }
}

/// Offsets of the reset-typed variables in one module's meta.
fn collect_reset_offsets(
    variable_meta: &HashMap<VarId, VariableMeta>,
    out: &mut crate::HashSet<VarOffset>,
) {
    for meta in variable_meta.values() {
        if !meta.r#type.is_reset() {
            continue;
        }
        for elem in &meta.elements {
            out.insert(elem.current);
        }
    }
}

fn collect_reset_offsets_recursive(m: &ModuleVariableMeta, out: &mut crate::HashSet<VarOffset>) {
    collect_reset_offsets(&m.variable_meta, out);
    for child in &m.children {
        collect_reset_offsets_recursive(child, out);
    }
}

/// The reset net an `if_reset` dispatch tests, or `None` for any other `if`.
fn reset_dispatch_key(
    x: &ProtoIfStatement,
    reset_offsets: &crate::HashSet<VarOffset>,
) -> Option<(VarOffset, Option<(usize, usize)>)> {
    match x.cond.as_ref()? {
        ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select: None,
            ..
        } if reset_offsets.contains(var_offset) => Some((*var_offset, *select)),
        _ => None,
    }
}

/// Fuse adjacent `if_reset` dispatches on the same reset net.
///
/// Sound because the analyzer rejects an `always_ff` that writes a reset-typed
/// variable, so nothing a clock event evaluates can change the condition
/// between two dispatches.  Only ADJACENT ones fuse, so nothing is reordered.
fn merge_reset_dispatch(
    stmts: &mut Vec<ProtoStatement>,
    reset_offsets: &crate::HashSet<VarOffset>,
) {
    let mut out: Vec<ProtoStatement> = Vec::with_capacity(stmts.len());
    let mut last_key: Option<(VarOffset, Option<(usize, usize)>)> = None;
    for stmt in stmts.drain(..) {
        let key = match &stmt {
            ProtoStatement::If(x) => reset_dispatch_key(x, reset_offsets),
            _ => None,
        };
        if key.is_some()
            && key == last_key
            && let Some(ProtoStatement::If(prev)) = out.last_mut()
        {
            let ProtoStatement::If(cur) = stmt else {
                unreachable!("key is Some only for If")
            };
            prev.true_side.extend(cur.true_side);
            prev.false_side.extend(cur.false_side);
            continue;
        }
        last_key = key;
        out.push(stmt);
    }
    *stmts = out;
}

/// Every variable offset the settled comb list can touch, as a SUPERSET.
///
/// Consumed by the testbench's comb-dirty filter: a testbench statement that
/// writes nothing in here cannot change what the next comb evaluation reads,
/// so it does not have to invalidate the comb.
///
/// This deliberately does NOT reuse `ProtoStatement::gather_variable_offsets`
/// at block level. That one is tuned for dependency analysis and drops a
/// `CompiledBlock`'s FF offsets, which it is right to drop there and wrong to
/// drop here. So the walk is done here, unioning inputs and outputs alike —
/// over-approximating costs only a missed optimization, under-approximating
/// would silently skip a required settle, leaving a stale comb value to be
/// read as settled.
pub(crate) fn collect_comb_touched_offsets(stmts: &[ProtoStatement]) -> HashSet<VarOffset> {
    fn walk(stmts: &[ProtoStatement], acc: &mut HashSet<VarOffset>) {
        let mut ins: Vec<VarOffset> = Vec::new();
        let mut outs: Vec<VarOffset> = Vec::new();
        for s in stmts {
            match s {
                ProtoStatement::SequentialBlock(body) => walk(body, acc),
                ProtoStatement::If(x) => {
                    if let Some(cond) = &x.cond {
                        cond.gather_variable_offsets(&mut ins);
                    }
                    walk(&x.true_side, acc);
                    walk(&x.false_side, acc);
                }
                ProtoStatement::Case(x) => {
                    for arm in &x.arms {
                        arm.cond.gather_variable_offsets(&mut ins);
                        walk(&arm.body, acc);
                    }
                    walk(&x.default, acc);
                }
                ProtoStatement::For(x) => {
                    // Written on every pass, so it belongs in the set even
                    // when the body does not read it.
                    acc.insert(x.var_offset);
                    for e in x.range.dynamic_bounds() {
                        e.gather_variable_offsets(&mut ins);
                    }
                    walk(&x.body, acc);
                }
                ProtoStatement::CompiledBlock(x) => {
                    acc.extend(x.input_offsets.iter().copied());
                    acc.extend(x.output_offsets.iter().copied());
                    for (dep_ins, dep_outs) in &x.stmt_deps {
                        acc.extend(dep_ins.iter().copied());
                        acc.extend(dep_outs.iter().copied());
                    }
                    walk(&x.original_stmts, acc);
                }
                other => other.gather_variable_offsets(&mut ins, &mut outs),
            }
            acc.extend(ins.drain(..));
            acc.extend(outs.drain(..));
        }
    }
    let mut acc = HashSet::default();
    walk(stmts, &mut acc);
    acc
}

#[cfg(test)]
mod event_written_comb_tests {
    use super::*;
    use crate::backend::ChunkArtifact;
    use crate::ir::statement::{ProtoTbMethodKind, ReadmemhElement};
    use crate::ir::{
        CompiledBlockStatement, ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoExpression,
        ProtoSystemFunctionCall, VarOffset,
    };
    use veryl_analyzer::value::{Value, ValueU64};
    use veryl_parser::token_range::TokenRange;

    fn lit(payload: u64, width: usize) -> ProtoExpression {
        ProtoExpression::Value {
            value: Value::U64(ValueU64 {
                payload,
                mask_xz: 0,
                width: width as u32,
                signed: false,
            }),
            width,
            expr_context: crate::ir::ExpressionContext {
                width,
                signed: false,
            },
        }
    }

    fn cassign(dst: VarOffset, w: usize) -> ProtoStatement {
        ProtoStatement::Assign(ProtoAssignStatement {
            dst,
            dst_width: w,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: lit(0, w),
            dst_ff_current_offset: 0,
            token: TokenRange::default(),
        })
    }

    fn cdyn_write(base: isize, stride: isize, num: usize) -> ProtoStatement {
        ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
            dst_base: VarOffset::Comb(base),
            dst_stride: stride,
            dst_num_elements: num,
            dst_index_expr: lit(0, 8),
            dst_width: 32,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: lit(0, 32),
            dst_ff_current_base_offset: 0,
        })
    }

    fn events(stmts: Vec<ProtoStatement>) -> HashMap<Event, Vec<ProtoStatement>> {
        HashMap::from_iter([(Event::Initial, stmts)])
    }

    #[test]
    fn collects_static_and_expands_dynamic_writes() {
        // FF writes are ignored; a dynamic write taints every element,
        // including the middle ones a base+last compression would hide.
        let out = collect_event_written_comb(&events(vec![
            cassign(VarOffset::Comb(0x0), 32),
            cassign(VarOffset::Ff(0x8), 32),
            cdyn_write(0x10, 0x8, 3),
        ]))
        .unwrap();
        assert_eq!(out, HashSet::from_iter([0x0isize, 0x10, 0x18, 0x20]));
    }

    #[test]
    fn registers_readmemh_element_offsets() {
        let stmts = vec![ProtoStatement::SystemFunctionCall(
            ProtoSystemFunctionCall::Readmemh {
                filename: "x.hex".into(),
                elements: vec![
                    ReadmemhElement {
                        current: VarOffset::Comb(0x30),
                        next_offset: None,
                    },
                    ReadmemhElement {
                        current: VarOffset::Ff(0x8),
                        next_offset: None,
                    },
                ],
                width: 32,
            },
        )];
        let out = collect_event_written_comb(&events(stmts)).unwrap();
        assert_eq!(out, HashSet::from_iter([0x30isize]));
    }

    #[test]
    fn walks_compiled_block_originals_and_disarms_without_them() {
        fn cb(originals: Vec<ProtoStatement>) -> ProtoStatement {
            unsafe extern "system" fn stub(_: *const u8, _: *const u8, _: *mut u8, _: isize) {}
            ProtoStatement::CompiledBlock(CompiledBlockStatement {
                artifact: std::sync::Arc::new(ChunkArtifact {
                    func: stub,
                    keepalive: None,
                    content_fp: None,
                }),
                ff_delta_bytes: 0,
                comb_delta_bytes: 0,
                input_offsets: vec![],
                output_offsets: vec![VarOffset::Comb(0x0), VarOffset::Comb(0x10)],
                ff_canonical_offsets: vec![],
                stmt_deps: vec![],
                original_stmts: originals,
            })
        }
        // The originals\u2019 dynamic write taints the middle element the
        // compressed output list omits.
        let out =
            collect_event_written_comb(&events(vec![cb(vec![cdyn_write(0x0, 0x8, 3)])])).unwrap();
        assert!(out.contains(&0x8isize));
        // No originals: the writes are unboundable, the split must disarm.
        assert!(collect_event_written_comb(&events(vec![cb(vec![])])).is_none());
    }

    #[test]
    fn disarms_on_a_tb_method_with_an_unresolvable_return() {
        let call = |ret| ProtoStatement::TbMethodCall {
            inst: StrId::default(),
            method: ProtoTbMethodKind::RandomGet {
                width: 32,
                signed: false,
                ret,
            },
        };
        assert!(collect_event_written_comb(&events(vec![call(None)])).is_some());
        assert!(
            collect_event_written_comb(&events(vec![call(Some((
                crate::ir::VarId::SYNTHETIC,
                crate::ir::statement::RetWidthCheck::Dst,
            )))]))
            .is_none()
        );
    }
}
