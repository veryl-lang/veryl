use crate::HashMap;
use crate::cranelift::FuncPtr;
use crate::ir::Config;
use crate::ir::ProtoStatement;
use crate::ir::VarId;
use crate::ir::VariableMeta;
use crate::ir::event::Event;
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
}

impl Context {
    pub fn scope(&mut self) -> &mut ScopeContext {
        self.scope_contexts.last_mut().unwrap()
    }
}

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Option<Self>;
}
