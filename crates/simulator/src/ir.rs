mod context;
mod declaration;
mod event;
mod expression;
mod module;
mod optimize;
mod statement;
mod variable;

pub use context::{Context, Conv};
pub use declaration::ProtoDeclaration;
pub use event::Event;
pub use expression::{Expression, ProtoExpression};
pub use module::{Module, ProtoModule};
pub use statement::{
    CompiledBlockStatement, ProtoStatement, ProtoStatementBlock, ProtoStatements, Statement,
    SystemFunctionCall, TbMethodKind, parse_hex_content,
};
pub use variable::{
    ModuleVariableMeta, ModuleVariables, Variable, VariableElement, VariableMeta,
    create_variable_meta, native_bytes, read_native_value, read_payload, value_size,
    write_native_value, write_payload,
};
pub use veryl_analyzer::ir::{Op, Type, VarId, VarPath};
pub use veryl_analyzer::value::Value;

use crate::HashMap;
use crate::simulator_error::SimulatorError;
use memmap2::Mmap;
use std::sync::Arc;
use veryl_analyzer::ir as air;
use veryl_analyzer::value::MaskCache;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

pub struct Ir {
    pub name: StrId,
    pub token: TokenRange,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_values: Box<[u8]>,
    pub comb_values: Box<[u8]>,
    pub use_4state: bool,
    pub module_variables: ModuleVariables,
    pub event_statements: HashMap<Event, Vec<Statement>>,
    pub comb_statements: Vec<Statement>,
    /// Post-comb functions: child module comb-only JIT functions that run
    /// after lite comb (port connections) to compute child comb values
    /// before events fire.
    pub post_comb_fns: Vec<Statement>,
    /// Output port connections from post_comb_fns that propagate child
    /// comb values to parent variables. Subset of post_comb_fns: only
    /// the non-Binary (Assign) statements. Run after events for propagation.
    pub post_comb_ports: Vec<Statement>,
    /// Full comb statements (includes per-core internal comb).
    /// Used by get()/dump() when merged comb+event events exist.
    pub full_comb_statements: Option<Vec<Statement>>,
    /// When true, settle_comb() uses full_comb_statements.
    pub use_full_comb_in_step: bool,
    /// Number of eval_comb passes needed for full convergence.
    /// Pre-computed from backward edges in the sorted comb statement list.
    pub required_comb_passes: usize,
    /// FF swap entries: (current_offset, value_size) pairs.
    /// Swap value_size bytes between current_offset and current_offset + value_size.
    pub ff_swap_entries: Vec<(usize, usize)>,
    /// Keeps JIT-compiled code alive. Wrapped in `Arc` so that multiple `Ir`
    /// instances created from the same cached `ProtoModule` can share the binary.
    _binary: Arc<Vec<Mmap>>,
}

impl Ir {
    pub fn from_module(
        module: Module,
        binary: Vec<Mmap>,
        use_4state: bool,
        token: TokenRange,
    ) -> Ir {
        Ir::from_module_arc(module, Arc::new(binary), use_4state, token)
    }

    pub fn from_module_arc(
        module: Module,
        binary: Arc<Vec<Mmap>>,
        use_4state: bool,
        token: TokenRange,
    ) -> Ir {
        Ir {
            name: module.name,
            token,
            ports: module.ports,
            ff_values: module.ff_values,
            comb_values: module.comb_values,
            use_4state,
            module_variables: module.module_variables,
            event_statements: module.event_statements,
            comb_statements: module.comb_statements,
            post_comb_ports: module.post_comb_fns.clone(),
            post_comb_fns: module.post_comb_fns,
            full_comb_statements: module.full_comb_statements,
            use_full_comb_in_step: module.use_full_comb_in_step,
            required_comb_passes: module.required_comb_passes,
            ff_swap_entries: module.ff_swap_entries,
            _binary: binary,
        }
    }

    /// Evaluate lite comb (port connections + top-level comb only).
    /// Used during step() when merged comb+event functions handle per-core comb.
    pub fn eval_comb(&self, mask_cache: &mut MaskCache) {
        for x in &self.comb_statements {
            x.eval_step(mask_cache);
        }
    }

    /// Evaluate post-comb functions: child comb-only JIT functions that
    /// compute child module comb values after port connections have been
    /// set by eval_comb. Called between eval_comb and events in step().
    pub fn eval_post_comb(&self, mask_cache: &mut MaskCache) {
        for x in &self.post_comb_fns {
            x.eval_step(mask_cache);
        }
    }

    /// Evaluate only the output port connections from post_comb_fns.
    /// Called after events to propagate merged event comb outputs to
    /// parent variables without re-running expensive child comb functions.
    pub fn eval_post_comb_ports(&self, mask_cache: &mut MaskCache) {
        for x in &self.post_comb_ports {
            x.eval_step(mask_cache);
        }
    }

    /// Evaluate comb until convergence. Runs at least `required_comb_passes`
    /// times, then checks for additional convergence when hidden backward
    /// edges may exist (full_comb_statements present).
    pub fn settle_comb(&self, mask_cache: &mut MaskCache) {
        let min_passes = self.required_comb_passes;
        self.eval_comb_full(mask_cache);
        for _ in 1..min_passes {
            self.eval_comb_full(mask_cache);
        }
        if self.full_comb_statements.is_none() && min_passes <= 1 {
            return;
        }
        const MAX_EXTRA: usize = 4;
        for _ in 0..MAX_EXTRA {
            let before = self.comb_snapshot();
            self.eval_comb_full(mask_cache);
            if self.comb_values[..] == before[..] {
                return;
            }
        }
    }

    fn comb_snapshot(&self) -> Vec<u8> {
        self.comb_values.to_vec()
    }

    /// Evaluate full comb once (including per-core internal comb).
    /// Called by settle_comb() for each required pass.
    pub fn eval_comb_full(&self, mask_cache: &mut MaskCache) {
        if let Some(stmts) = &self.full_comb_statements {
            for x in stmts {
                x.eval_step(mask_cache);
            }
        } else if !self.post_comb_fns.is_empty() {
            // 3+ level hierarchy: full_comb_statements should have been
            // built. If somehow it wasn't, fall back to settle loop.
            self.eval_comb(mask_cache);
            self.eval_post_comb(mask_cache);
            self.eval_comb(mask_cache);
        } else {
            self.eval_comb(mask_cache);
        }
    }

    pub fn dump_variables(&self) -> String {
        format!("{}", self.module_variables)
    }

    /// Returns (jit_count, total_count) of top-level statements across all events and comb.
    pub fn jit_stats(&self) -> (usize, usize) {
        let mut jit = 0;
        let mut total = 0;
        for stmts in self.event_statements.values() {
            for s in stmts {
                total += 1;
                if s.is_binary() {
                    jit += 1;
                }
            }
        }
        for s in &self.comb_statements {
            total += 1;
            if s.is_binary() {
                jit += 1;
            }
        }
        (jit, total)
    }

    /// Returns detailed stats: (comb_jit, comb_interp, event_jit, event_interp)
    pub fn detailed_stats(&self) -> (usize, usize, usize, usize) {
        let mut comb_jit = 0;
        let mut comb_interp = 0;
        let mut event_jit = 0;
        let mut event_interp = 0;
        for s in &self.comb_statements {
            if s.is_binary() {
                comb_jit += 1;
            } else {
                comb_interp += 1;
            }
        }
        for stmts in self.event_statements.values() {
            for s in stmts {
                if s.is_binary() {
                    event_jit += 1;
                } else {
                    event_interp += 1;
                }
            }
        }
        (comb_jit, comb_interp, event_jit, event_interp)
    }
}

// SAFETY: Each Ir exclusively owns its ff_values/comb_values buffers.
// Raw pointers in Statements point into these buffers — no cross-Ir aliasing.
// _binary (Arc<Vec<Mmap>>) keeps JIT code pages alive.
// NOTE: Ir is intentionally NOT Sync. Sharing &Ir across threads would allow
// concurrent mutation of ff_values/comb_values via interior raw pointers.
unsafe impl Send for Ir {}

pub fn build_ir(ir: &air::Ir, top: StrId, config: &Config) -> Result<Ir, SimulatorError> {
    for x in &ir.components {
        if let air::Component::Module(x) = x
            && top == x.name
        {
            let token = x.token;
            let mut context = context::Context {
                config: config.clone(),
                ..Default::default()
            };
            let proto: ProtoModule = Conv::conv(&mut context, x)?;
            let module = proto.instantiate();
            return Ok(Ir::from_module(
                module,
                context.binary,
                config.use_4state,
                token,
            ));
        }
    }
    Err(SimulatorError::TopModuleNotFound {
        module_name: top.to_string(),
    })
}

struct CacheEntry {
    proto: ProtoModule,
    binary: Arc<Vec<Mmap>>,
    token: TokenRange,
}

/// Cache for `ProtoModule` and JIT binaries keyed by top module name.
/// `shared_jit_cache` persists sub-module JIT results across different top modules.
#[derive(Default)]
pub struct ProtoModuleCache {
    entries: HashMap<StrId, CacheEntry>,
    shared_jit_cache: HashMap<StrId, context::JitCacheEntry>,
    /// Keeps Mmap pages alive so function pointers in shared_jit_cache remain valid.
    shared_binaries: Vec<Arc<Vec<Mmap>>>,
}

pub fn build_ir_cached(
    ir: &air::Ir,
    top: StrId,
    config: &Config,
    cache: &mut ProtoModuleCache,
) -> Result<Ir, SimulatorError> {
    // Cache hit: reuse ProtoModule, just instantiate with fresh buffers
    if let Some(entry) = cache.entries.get(&top) {
        let module = entry.proto.instantiate();
        return Ok(Ir::from_module_arc(
            module,
            Arc::clone(&entry.binary),
            config.use_4state,
            entry.token,
        ));
    }

    // Cache miss: run Conv::conv
    for x in &ir.components {
        if let air::Component::Module(x) = x
            && top == x.name
        {
            let token = x.token;
            let mut context = context::Context {
                config: config.clone(),
                jit_cache: std::mem::take(&mut cache.shared_jit_cache),
                ..Default::default()
            };

            let proto: ProtoModule = Conv::conv(&mut context, x)?;
            let module = proto.instantiate();
            let binary = Arc::new(context.binary);

            let result = Ir::from_module_arc(module, Arc::clone(&binary), config.use_4state, token);

            cache.shared_jit_cache = context.jit_cache;
            cache.shared_binaries.push(Arc::clone(&binary));

            cache.entries.insert(
                top,
                CacheEntry {
                    proto,
                    binary,
                    token,
                },
            );

            return Ok(result);
        }
    }
    Err(SimulatorError::TopModuleNotFound {
        module_name: top.to_string(),
    })
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub use_4state: bool,
    pub use_jit: bool,
    pub dump_cranelift: bool,
    pub dump_asm: bool,
}

impl Config {
    pub fn all() -> Vec<Config> {
        let mut ret = vec![];

        for use_4state in [false, true] {
            for use_jit in [false, true] {
                ret.push(Config {
                    use_4state,
                    use_jit,
                    ..Default::default()
                });
            }
        }

        ret
    }
}
