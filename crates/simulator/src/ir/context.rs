use crate::HashMap;
use crate::HashSet;
use crate::backend::{BackendRegistry, ChunkArtifact};
use crate::ir::Config;
use crate::ir::ProtoStatement;
use crate::ir::VarId;
use crate::ir::VariableMeta;
use crate::ir::event::Event;
use crate::ir::statement::StmtDep;
use crate::ir::variable::VarOffset;
use crate::simulator_error::SimulatorError;
use std::sync::Arc;
use veryl_analyzer::ir as air;
use veryl_analyzer::symbol::Affiliation;

pub struct ScopeContext {
    pub variable_meta: HashMap<VarId, VariableMeta>,
    pub analyzer_context: veryl_analyzer::conv::Context,
    pub ff_table: air::FfTable,
    /// Lazily-built reverse index `comb byte offset -> owning VarId` for
    /// variables affiliated with a function body.  Used to relocate the
    /// inlined-function storage per call-site (see the `FunctionCall`
    /// factor in `expression.rs`).  `None` until first queried.
    pub func_offset_index: Option<HashMap<isize, VarId>>,
}

impl ScopeContext {
    /// Map a comb byte offset to the function-affiliated variable that owns
    /// it, or `None` if the offset belongs to a non-function (module /
    /// always-block) variable.  Builds the index on first call.
    pub fn func_offset_varid(&mut self, off: isize) -> Option<VarId> {
        if self.func_offset_index.is_none() {
            let mut idx: HashMap<isize, VarId> = HashMap::default();
            for (vid, var) in &self.analyzer_context.variables {
                if var.affiliation != Affiliation::Function {
                    continue;
                }
                if let Some(meta) = self.variable_meta.get(vid) {
                    for e in &meta.elements {
                        if let VarOffset::Comb(o) = e.current {
                            idx.insert(o, *vid);
                        }
                    }
                }
            }
            self.func_offset_index = Some(idx);
        }
        self.func_offset_index.as_ref().unwrap().get(&off).copied()
    }
}

/// Cached compiled chunk + variable offsets it reads/writes (for
/// dependency analysis).  `artifact` is shared so cache hits clone the
/// `Arc` rather than duplicating the keepalive.
pub struct CachedChunk {
    pub artifact: Arc<ChunkArtifact>,
    pub input_offsets: Vec<VarOffset>,
    pub output_offsets: Vec<VarOffset>,
    /// Canonical (current) offsets for FF variables written by this function.
    pub ff_canonical_offsets: Vec<isize>,
    pub stmt_deps: Vec<StmtDep>,
    /// Originals before JIT compilation; cache hits expand from these
    /// for fine-grained dependency analysis with offset deltas applied.
    pub original_stmts: Vec<ProtoStatement>,
}

/// Cache entry for a module type's compiled internal logic.  Stores
/// the reference ff/comb start offsets so instances compute a delta
/// to reuse the same compiled code.
pub struct ChunkCacheEntry {
    pub ref_ff_start_bytes: isize,
    pub ref_comb_start_bytes: isize,
    pub event_funcs: HashMap<Event, CachedChunk>,
    pub comb_func: Option<CachedChunk>,
}

#[derive(Default)]
pub struct Context {
    pub config: Config,
    pub scope_contexts: Vec<ScopeContext>,
    pub ff_total_bytes: usize,
    pub comb_total_bytes: usize,
    pub pending_statements: Vec<ProtoStatement>,
    /// Keyed by `Arc<Component>` pointer so that distinct parameter
    /// specializations (distinct `Arc`s from the analyzer) do not collide.
    pub chunk_cache: HashMap<*const air::Component, ChunkCacheEntry>,
    pub expanding_functions: HashSet<VarId>,
    pub in_initial: bool,
    /// True while converting a de-aliased DUT's subtree, so only the topmost
    /// boundary is de-aliased — the DUT's internals stay aliased (they relocate
    /// uniformly with it, no per-cycle boundary copies on hot paths).
    pub in_reuse_dut: bool,
    /// Per-testbench id (assigned at `ProtoModule::conv`).  A component is a reuse
    /// DUT only when it recurs across DIFFERENT tops; replication within one top
    /// (SMP harts) shares this id and so does not de-alias.
    pub test_top_id: u64,
    /// Populated from `Config`.  Empty → interpreter-only.
    pub backends: BackendRegistry,
    /// See `alloc_internal_event_id`.
    pub internal_event_ids_allocated: u32,
}

impl Context {
    pub fn scope(&mut self) -> &mut ScopeContext {
        self.scope_contexts.last_mut().unwrap()
    }

    /// Mint a globally-unique VarId for an event declared inside a child
    /// instance.  Ids come from the top of the u32 range (just below
    /// `VarId::SYNTHETIC`), which real per-scope ids never reach, so the
    /// inst-boundary event remap is a guaranteed no-op for them at every
    /// ancestor level.  See the re-key in `InstDeclaration`'s `Conv` impl
    /// (ir/declaration.rs) for the collision this prevents.
    pub fn alloc_internal_event_id(&mut self) -> VarId {
        self.internal_event_ids_allocated += 1;
        // SYNTHETIC is u32::MAX; start below it.  Real ids count up from
        // 0, so the ranges meet only after ~2^31 allocations.
        debug_assert!(
            self.internal_event_ids_allocated < u32::MAX / 2,
            "internal event id allocator exhausted its half of the u32 range"
        );
        VarId::from_raw(u32::MAX - self.internal_event_ids_allocated)
    }
}

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Result<Self, SimulatorError>;
}
