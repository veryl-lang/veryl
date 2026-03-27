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
use crate::simulator::SimProfile;
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
    /// Unified comb statements: all port connections, child comb, and internal
    /// comb combined into a single dependency-sorted list.
    pub comb_statements: Vec<Statement>,
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
            required_comb_passes: module.required_comb_passes,
            ff_swap_entries: module.ff_swap_entries,
            _binary: binary,
        }
    }

    /// Evaluate comb until convergence.
    /// Returns true if converged on the first extra pass (or no extra needed).
    pub fn settle_comb(
        &self,
        mask_cache: &mut MaskCache,
        snapshot_buf: &mut Vec<u8>,
        profile: &mut SimProfile,
        skip_convergence_check: bool,
    ) -> bool {
        #[cfg(feature = "profile")]
        {
            profile.settle_comb_count += 1;
        }
        let _ = profile; // suppress unused warning when profile feature is off

        let min_passes = self.required_comb_passes;
        self.eval_comb_full(mask_cache, profile);
        #[cfg(feature = "profile")]
        {
            profile.comb_eval_count += 1;
        }
        for _ in 1..min_passes {
            self.eval_comb_full(mask_cache, profile);
            #[cfg(feature = "profile")]
            {
                profile.comb_eval_count += 1;
            }
        }
        // In debug builds, verify that required_comb_passes is sufficient
        // by checking that one more eval doesn't change comb_values.
        #[cfg(debug_assertions)]
        if !skip_convergence_check {
            snapshot_buf.resize(self.comb_values.len(), 0);
            snapshot_buf.copy_from_slice(&self.comb_values);
            self.eval_comb_full(mask_cache, profile);
            debug_assert!(
                self.comb_values[..] == snapshot_buf[..],
                "comb did not converge after {} required passes ({} stmts) — \
                 required_comb_passes may be underestimated",
                min_passes,
                self.comb_statements.len()
            );
        }
        if min_passes <= 1 {
            return true;
        }
        if skip_convergence_check {
            return true;
        }
        const MAX_EXTRA: usize = 4;
        for _i in 0..MAX_EXTRA {
            snapshot_buf.resize(self.comb_values.len(), 0);
            snapshot_buf.copy_from_slice(&self.comb_values);
            self.eval_comb_full(mask_cache, profile);
            #[cfg(feature = "profile")]
            {
                profile.comb_eval_count += 1;
                profile.extra_pass_count += 1;
            }
            if self.comb_values[..] == snapshot_buf[..] {
                #[cfg(feature = "profile")]
                if _i == 0 {
                    profile.converged_first_try += 1;
                }
                return _i == 0;
            }
        }
        log::warn!(
            "comb convergence failed after {} + {} passes ({} stmts)",
            min_passes,
            MAX_EXTRA,
            self.comb_statements.len()
        );
        false
    }

    /// Evaluate unified comb once.
    /// Called by settle_comb() for each required pass.
    pub fn eval_comb_full(&self, mask_cache: &mut MaskCache, profile: &mut SimProfile) {
        let _ = profile;
        #[cfg(feature = "profile")]
        let start = std::time::Instant::now();

        for x in &self.comb_statements {
            x.eval_step(mask_cache);
        }

        #[cfg(feature = "profile")]
        {
            profile.eval_comb_full_ns += start.elapsed().as_nanos() as u64;
        }
    }

    /// Number of statements in comb_statements (for profiling).
    pub fn comb_stmt_count(&self) -> (usize, usize, usize) {
        let mut binary = 0;
        let mut interp = 0;
        let mut total = 0;
        for s in &self.comb_statements {
            total += 1;
            if s.is_binary() {
                binary += 1;
            } else {
                interp += 1;
            }
        }
        (total, binary, interp)
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
