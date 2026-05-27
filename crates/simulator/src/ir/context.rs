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

pub struct ScopeContext {
    pub variable_meta: HashMap<VarId, VariableMeta>,
    pub analyzer_context: veryl_analyzer::conv::Context,
    pub ff_table: air::FfTable,
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
    /// Populated from `Config`.  Empty → interpreter-only.
    pub backends: BackendRegistry,
}

impl Context {
    pub fn scope(&mut self) -> &mut ScopeContext {
        self.scope_contexts.last_mut().unwrap()
    }
}

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Result<Self, SimulatorError>;
}
