use crate::HashMap;
use crate::HashSet;
use crate::cranelift::FuncPtr;
use crate::ir::Config;
use crate::ir::ProtoStatement;
use crate::ir::VarId;
use crate::ir::VariableMeta;
use crate::ir::event::Event;
use crate::ir::statement::StmtDep;
use crate::simulator_error::SimulatorError;
use memmap2::Mmap;
use veryl_parser::resource_table::StrId;

pub struct ScopeContext {
    pub variable_meta: HashMap<VarId, VariableMeta>,
    pub analyzer_context: veryl_analyzer::conv::Context,
}

/// A cached JIT-compiled function for a group of statements, along with
/// the variable offsets it reads/writes (used for dependency analysis).
pub struct JitCachedFunc {
    pub func: FuncPtr,
    pub input_offsets: Vec<(bool, isize)>,
    pub output_offsets: Vec<(bool, isize)>,
    /// Canonical (current) offsets for FF variables written by this function.
    pub ff_canonical_offsets: Vec<isize>,
    pub stmt_deps: Vec<StmtDep>,
    /// Original individual statements before JIT compilation.
    /// Stored in the cache so that subsequent instances can expand
    /// CompiledBlocks for fine-grained dependency analysis after
    /// applying offset deltas.
    pub original_stmts: Vec<ProtoStatement>,
}

/// Cache entry for a module type's JIT-compiled internal logic.
/// Stores the reference ff/comb start byte offsets so that subsequent
/// instances can compute a delta to reuse the same compiled code.
pub struct JitCacheEntry {
    pub ref_ff_start_bytes: isize,
    pub ref_comb_start_bytes: isize,
    pub event_funcs: HashMap<Event, JitCachedFunc>,
    pub comb_func: Option<JitCachedFunc>,
    /// Merged comb+event functions (per event).
    /// When present, the event function includes comb computation,
    /// allowing load CSE across the comb-to-event boundary.
    pub merged_funcs: HashMap<Event, JitCachedFunc>,
}

#[derive(Default)]
pub struct Context {
    pub config: Config,
    pub scope_contexts: Vec<ScopeContext>,
    pub binary: Vec<Mmap>,
    pub ff_total_bytes: usize,
    pub comb_total_bytes: usize,
    pub pending_statements: Vec<ProtoStatement>,
    pub jit_cache: HashMap<StrId, JitCacheEntry>,
    pub expanding_functions: HashSet<VarId>,
    pub in_initial: bool,
}

impl Context {
    pub fn scope(&mut self) -> &mut ScopeContext {
        self.scope_contexts.last_mut().unwrap()
    }
}

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Result<Self, SimulatorError>;
}
