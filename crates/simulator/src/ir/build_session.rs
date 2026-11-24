use super::comb_pipeline_cache::CombPipelineCache;
use super::{Config, Context, Conv, Ir, ProtoModule};
use crate::backend::BackendRegistry;
use crate::backend::inst::DutReuseCache;
use crate::backend::registry::ChunkArtifactCache;
use crate::simulator_error::SimulatorError;
use crate::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex, OnceLock};
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

/// A group of simulator builds sharing one analysis IR and a fixed configuration.
///
/// Create this before starting workers, with all test tops used to select the
/// recurring DUT boundaries. Workers may call `build_ir` concurrently. The IR
/// is borrowed immutably, keeping its component addresses valid until the last
/// build finishes; the configuration is snapshotted, so subsequent caller edits
/// cannot change the meaning of a cache entry. A new session has fresh caches,
/// even when it borrows the same IR as another session.
///
/// A session cannot outlive the analysis IR whose addresses its caches use:
/// ```compile_fail
/// use veryl_simulator::ir::{BuildSession, Config};
/// let config = Config::default();
/// let session = {
///     let ir = veryl_analyzer::ir::Ir::default();
///     BuildSession::new(&ir, &config, &[])
/// };
/// session.build_ir("Top".into());
/// ```
pub struct BuildSession<'ir> {
    ir: &'ir air::Ir,
    config: Config,
    tops: Vec<StrId>,
    dut_reuse: Arc<DutReuseCache>,
    comb_cache: Arc<CombPipelineCache>,
    chunk_cache: ChunkArtifactCache,
    /// The configuration and reuse cache without hierarchy templates, for
    /// the tops whose templates fell back.
    flat: OnceLock<(Config, Arc<DutReuseCache>)>,
    fell_back: Mutex<HashSet<StrId>>,
}

impl<'ir> BuildSession<'ir> {
    #[cfg(test)]
    pub(crate) fn hier_conversions(&self) -> usize {
        self.dut_reuse
            .hier_conversions
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn new(ir: &'ir air::Ir, config: &Config, tops: &[StrId]) -> Self {
        Self {
            ir,
            config: config.clone(),
            tops: tops.to_vec(),
            dut_reuse: Arc::new(DutReuseCache::new(ir, tops, config)),
            comb_cache: Arc::default(),
            chunk_cache: Arc::default(),
            flat: OnceLock::new(),
            fell_back: Mutex::default(),
        }
    }

    /// Build a top with fresh simulation storage, sharing conversion and code
    /// artifacts with other builds in this session when DUT reuse is enabled.
    pub fn build_ir(&self, top: StrId) -> Result<Ir, SimulatorError> {
        let entry = self.convert(top)?;
        Ok(self.instantiate(&entry))
    }

    /// `Config::shared_modules` entries none of the session's tops
    /// instantiates twice.
    pub fn unmatched_shared_modules(&self) -> &[String] {
        &self.dut_reuse.unmatched_shared_modules
    }

    fn convert(&self, top: StrId) -> Result<CacheEntry, SimulatorError> {
        let module = self.ir.components.iter().find_map(|c| match c {
            air::Component::Module(m) if m.name == top => Some(m),
            _ => None,
        });
        let module = module.ok_or_else(|| SimulatorError::TopModuleNotFound {
            module_name: top.to_string(),
        })?;
        let templates = self.config.has_hier_templates();
        if templates && !self.fell_back.lock().unwrap().contains(&top) {
            let (proto, fell_back) = self.convert_with(module, &self.config, &self.dut_reuse)?;
            let Some(why) = fell_back else {
                return Ok(CacheEntry {
                    proto,
                    token: module.token,
                });
            };
            if !self.config.shared_modules.is_empty() {
                log::warn!("[test] shared_modules: {top} is built without them: {why}");
            }
            self.fell_back.lock().unwrap().insert(top);
        }
        let (config, dut_reuse) = if templates {
            let (config, dut_reuse) = self.flat.get_or_init(|| {
                let mut config = self.config.clone();
                config.hier_templates = None;
                config.shared_modules.clear();
                let dut_reuse = Arc::new(DutReuseCache::new(self.ir, &self.tops, &config));
                (config, dut_reuse)
            });
            (config, dut_reuse)
        } else {
            (&self.config, &self.dut_reuse)
        };
        let (proto, _) = self.convert_with(module, config, dut_reuse)?;
        Ok(CacheEntry {
            proto,
            token: module.token,
        })
    }

    /// The top converted under `config`, and why its hierarchy templates fell
    /// back if they did.
    fn convert_with(
        &self,
        module: &air::Module,
        config: &Config,
        dut_reuse: &Arc<DutReuseCache>,
    ) -> Result<(ProtoModule, Option<String>), SimulatorError> {
        let mut backends = BackendRegistry::for_config(config);
        backends.chunk_cache = Arc::clone(&self.chunk_cache);
        let mut context = Context {
            config: config.clone(),
            backends,
            dut_reuse: Arc::clone(dut_reuse),
            comb_cache: Arc::clone(&self.comb_cache),
            ..Default::default()
        };
        let proto = Conv::conv(&mut context, module)?;
        Ok((proto, context.hier_fell_back))
    }

    fn instantiate(&self, entry: &CacheEntry) -> Ir {
        Ir::from_module(entry.proto.instantiate(), &self.config, entry.token)
    }
}

struct CacheEntry {
    proto: ProtoModule,
    token: TokenRange,
}

/// Per-worker cache of complete top modules, bound to one build session.
/// Keys are top names, which are meaningful only within that session's IR and
/// configuration. Each hit instantiates fresh buffers; compiled artifacts are
/// kept alive by the `Arc`s embedded in the cached `ProtoModule`.
pub struct ProtoModuleCache<'a> {
    session: &'a BuildSession<'a>,
    entries: HashMap<StrId, CacheEntry>,
}

impl<'a> ProtoModuleCache<'a> {
    pub fn new(session: &'a BuildSession<'a>) -> Self {
        Self {
            session,
            entries: HashMap::default(),
        }
    }

    /// Drop the cached conversion of `top`, once no later build needs it.
    pub fn release(&mut self, top: StrId) {
        self.entries.remove(&top);
    }

    pub fn build_ir(&mut self, top: StrId) -> Result<Ir, SimulatorError> {
        let entry = match self.entries.entry(top) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(self.session.convert(top)?),
        };
        Ok(self.session.instantiate(entry))
    }
}
