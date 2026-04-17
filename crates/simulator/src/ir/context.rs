use crate::FuncPtr;
use crate::HashMap;
use crate::HashSet;
use crate::ir::Config;
use crate::ir::ProtoStatement;
use crate::ir::VarId;
use crate::ir::VariableMeta;
use crate::ir::event::Event;
use crate::ir::statement::StmtDep;
use crate::ir::variable::VarOffset;
use crate::simulator_error::SimulatorError;
use veryl_parser::resource_table::StrId;

pub struct ScopeContext {
    pub variable_meta: HashMap<VarId, VariableMeta>,
    pub analyzer_context: veryl_analyzer::conv::Context,
}

/// A cached JIT-compiled function for a group of statements, along with
/// the variable offsets it reads/writes (used for dependency analysis).
pub struct JitCachedFunc {
    pub func: FuncPtr,
    pub input_offsets: Vec<VarOffset>,
    pub output_offsets: Vec<VarOffset>,
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
    pub binary: Vec<super::BinaryStorage>,
    pub ff_total_bytes: usize,
    pub comb_total_bytes: usize,
    pub pending_statements: Vec<ProtoStatement>,
    pub jit_cache: HashMap<StrId, JitCacheEntry>,
    pub expanding_functions: HashSet<VarId>,
    pub in_initial: bool,
    /// Byte offset of the cold dirty flag in comb_values.
    /// Set during top-level module Conv, used by event JIT to emit flag stores.
    pub cold_dirty_flag_offset: Option<i64>,
    /// Size of hot comb region (bytes) for cold-write detection in JIT.
    pub comb_hot_size: usize,
    /// Byte offset of event→comb dirty flag in comb_values.
    /// Set during top-level module Conv, used by event JIT to emit flag stores.
    pub event_comb_dirty_flag_offset: Option<i64>,
    /// True when the JIT is currently compiling an event-phase function.
    /// Used to gate scheduled-write redirection (strict NBA): comb stores
    /// performed during `event_eval` are routed to the write-log instead of
    /// writing `comb_values` directly.  Must be `false` for comb-JIT
    /// compilation paths even if they share the same `build_binary` entry
    /// point (e.g. `declaration.rs` per-module comb function).
    pub in_event: bool,
    /// Write-log buffer for sparse FF commit and scheduled comb writes.
    /// Heap-allocated so its pointer is stable during JIT compilation.
    /// See `WriteLogEntry` for the per-entry layout.
    pub write_log_buffer: Option<Box<WriteLogBuffer>>,
}

pub const MAX_WRITE_LOG_ENTRIES: usize = 65536;

/// Write-log entry kind.
pub const LOG_KIND_FF: u32 = 0;
pub const LOG_KIND_COMB: u32 = 1;

/// Byte stride of `WriteLogEntry` for JIT arithmetic.
/// Must match `size_of::<WriteLogEntry>()` (asserted in debug builds).
pub const WRITE_LOG_ENTRY_STRIDE: usize = 64;
/// Log2 of `WRITE_LOG_ENTRY_STRIDE` for JIT shift instructions.
pub const WRITE_LOG_ENTRY_STRIDE_LOG2: i64 = 6;

/// Fixed-size write-log buffer for sparse FF commit and scheduled comb writes.
pub struct WriteLogBuffer {
    pub entries: Box<[WriteLogEntry; MAX_WRITE_LOG_ENTRIES]>,
    pub count: u64,
    /// Base address of ff_values buffer. Set after allocation so JIT can
    /// compute ff_delta = ff_values_param - ff_values_base at runtime.
    pub ff_values_base: u64,
    /// Peak log count observed during simulation (for overflow monitoring).
    pub peak_count: u64,
}

/// One write recorded during `event_eval`.
///
/// FF (kind=`LOG_KIND_FF`): the value has already been stored to
/// `ff_values[current_offset + value_size]` (the "next" slot). `ff_commit`
/// uses (offset, size) to copy next → current. `value`/`mask` are unused.
///
/// Comb (kind=`LOG_KIND_COMB`): the event's computed value is carried
/// inline in `value[..value_size]`, together with a write mask in
/// `mask[..value_size]` that indicates which bits this statement actually
/// intends to update.  The drain step (after `event_eval`) applies:
/// ```text
///     comb_values[off..] = (comb_values[off..] & ~mask) | (value & mask);
/// ```
/// This gives correct SV bit-select NBA semantics: multiple bit-select
/// writes to the same variable compose their modifications rather than
/// overwriting each other. For whole-word writes the mask is all-ones.
#[repr(C, align(64))]
#[derive(Copy, Clone, Default)]
pub struct WriteLogEntry {
    pub current_offset: u32,
    pub value_size: u32,
    pub kind: u32,
    pub _pad0: u32,
    pub value: [u8; 16],
    pub mask: [u8; 16],
    pub _pad1: [u8; 16],
}

const _: () = {
    assert!(
        std::mem::size_of::<WriteLogEntry>() == WRITE_LOG_ENTRY_STRIDE,
        "WRITE_LOG_ENTRY_STRIDE must match size_of::<WriteLogEntry>()",
    );
    assert!(1usize << WRITE_LOG_ENTRY_STRIDE_LOG2 == WRITE_LOG_ENTRY_STRIDE);
};

impl Default for WriteLogBuffer {
    fn default() -> Self {
        WriteLogBuffer {
            entries: Box::new([WriteLogEntry::default(); MAX_WRITE_LOG_ENTRIES]),
            count: 0,
            ff_values_base: 0,
            peak_count: 0,
        }
    }
}

impl Context {
    pub fn scope(&mut self) -> &mut ScopeContext {
        self.scope_contexts.last_mut().unwrap()
    }
}

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Result<Self, SimulatorError>;
}
