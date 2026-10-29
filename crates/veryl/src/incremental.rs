//! Incremental build driver: per file, decides whether to restore a cached
//! fragment instead of parse + pass1, and maintains the on-disk store.
//!
//! A file misses (needs full parse + pass1, maybe pass2/emit) if its hash
//! changed, it produced diagnostics, its output is stale (`dst_is_stale`, only
//! when the caller emits), or anything it depends on missed (transitive, from
//! the previous build's dependency map). The miss set is a superset of the
//! files needing pass2/emit, so a restored file is never asked for its AST.

use log::debug;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use veryl_analyzer::conv::unit_cache;
use veryl_analyzer::fragment_cache::{self, Fragment, FragmentWatermark};
use veryl_analyzer::symbol::TestType;
use veryl_analyzer::{Analyzer, CachedDiagnostic, scope, symbol_table, type_dag};
use veryl_cache::Store;
use veryl_metadata::Metadata;
use veryl_parser::resource_table;
use veryl_parser::resource_table::StrId;
use veryl_path::PathSet;

thread_local!(static LAST_RESTORED: Cell<usize> = const { Cell::new(0) });

/// Number of fragments restored by the most recent build on this thread.
/// Introspection for tests and diagnostics.
pub fn last_restored_count() -> usize {
    LAST_RESTORED.with(|x| x.get())
}

pub struct Incremental {
    store: Store,
    /// Files that must go through the full pipeline.
    miss: HashSet<PathBuf>,
    /// Source contents read during miss computation, reused for parsing.
    inputs: HashMap<PathBuf, String>,
    hashes: HashMap<PathBuf, String>,
    root_project: String,
    pub restored: usize,
    /// Cached diagnostics of restored files, to re-report on a warm run.
    restored_diagnostics: Vec<CachedDiagnostic>,
    /// Whether output staleness was ignored because the store said no test
    /// needs an external simulator. The caller must re-check that against the
    /// symbol table: a file re-analyzed this run may have introduced one.
    pub predicted_no_emit: bool,
    /// When this run started, which precedes everything it emits.
    started_at: SystemTime,
    /// Files restored from a fragment this build.
    restored_paths: HashSet<PathBuf>,
    /// Per-file generic instances hash, computed after post_pass1.
    generic_instance_hashes: Option<HashMap<PathBuf, String>>,
}

/// What a run will do with the emitted SystemVerilog, which decides whether a
/// stale or missing output has to force a cache miss.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputIntent {
    /// The run emits (`build`), so a stale output must be regenerated.
    Emit,
    /// The run never emits (`check`, `doc`, `dump`, `synth`, `publish`).
    Never,
    /// The run emits only if a test needs an external simulator (`test`).
    SkipIfNativeOnly,
}

impl Drop for Incremental {
    fn drop(&mut self) {
        LAST_RESTORED.with(|x| x.set(self.restored));
    }
}

impl Incremental {
    /// Opens the store and computes the miss set. Returns `None` when
    /// incremental builds are disabled.
    ///
    /// `selected_tests`: `None` for a plain build, `Some(filter)` when the
    /// caller simulates the matching tests (`None` filter = all). Files with
    /// a selected test are forced to miss so their pass2 IR is available.
    ///
    /// `forced_miss`: files that must miss regardless of the store, e.g. the
    /// ones whose generic instances changed in a previous attempt.
    pub fn open(
        metadata: &Metadata,
        paths: &[PathSet],
        defines: &[String],
        selected_tests: Option<Option<&str>>,
        intent: OutputIntent,
        forced_miss: &HashSet<PathBuf>,
    ) -> Option<Incremental> {
        if !metadata.build.incremental {
            return None;
        }

        // The global key hashes `binary_fingerprint()`, so a compiler/binary
        // change discards every entry — no separate version check needed.
        let key = global_key(metadata, defines)?;
        let store = Store::open(&metadata.project_dot_build_path().join("cache"), &key);

        // A file the store has never seen is assumed native-only: on a cold
        // store nothing restores anyway, so the guess is free there. It can
        // only be wrong for a file being re-analyzed right now.
        let predicted_no_emit = intent == OutputIntent::SkipIfNativeOnly
            && !paths.iter().any(|path| {
                store
                    .entry(&path.src.to_string_lossy())
                    .is_some_and(|x| x.has_non_native_test)
            });
        let consider_output = match intent {
            OutputIntent::Emit => true,
            OutputIntent::Never => false,
            OutputIntent::SkipIfNativeOnly => !predicted_no_emit,
        };

        let started_at = SystemTime::now();
        let mut miss = HashSet::new();
        let mut inputs = HashMap::new();
        let mut hashes = HashMap::new();

        for path in paths {
            let Ok(input) = fs::read_to_string(&path.src) else {
                // Leave unreadable files to the regular pipeline's error
                // reporting.
                miss.insert(path.src.clone());
                continue;
            };
            let hash = veryl_cache::content_hash(input.as_bytes());

            let entry = store.entry(&path.src.to_string_lossy());
            let has_selected_test = match selected_tests {
                None => false,
                Some(filter) => entry
                    .is_some_and(|x| x.tests.iter().any(|t| filter.is_none_or(|f| t.contains(f)))),
            };

            // examples/ files are never emitted, so output staleness does not
            // apply to them.
            let hit = entry.is_some_and(|x| x.file_hash == hash && x.fragment.is_some())
                && (!consider_output
                    || path.example
                    || !Self::dst_is_stale(metadata, path, entry.and_then(|x| x.updated_time)))
                && !has_selected_test
                && !forced_miss.contains(&path.src);
            if !hit {
                miss.insert(path.src.clone());
            }

            inputs.insert(path.src.clone(), input);
            hashes.insert(path.src.clone(), hash);
        }

        // Anything depending on a miss is a miss too. The dependency map
        // from the previous build is already transitively closed.
        let mut dependents = HashSet::new();
        for path in &miss {
            if let Some(entry) = store.entry(&path.to_string_lossy()) {
                dependents.extend(entry.dependents.iter().map(PathBuf::from));
            }
        }
        miss.extend(dependents);

        debug!(
            "Incremental build: {} files, {} cache hits",
            paths.len(),
            paths.len() - miss.iter().filter(|x| hashes.contains_key(*x)).count(),
        );

        Some(Incremental {
            store,
            miss,
            inputs,
            hashes,
            root_project: metadata.project.name.clone(),
            restored: 0,
            restored_diagnostics: Vec::new(),
            predicted_no_emit,
            started_at,
            restored_paths: HashSet::new(),
            generic_instance_hashes: None,
        })
    }

    /// Checked only when emitting and on a key-matched store, so
    /// `generated_files` is always from the same build environment.
    /// `updated_time` is when something the output depends on but the source
    /// does not (its generic instances) last changed.
    fn dst_is_stale(metadata: &Metadata, path: &PathSet, updated_time: Option<SystemTime>) -> bool {
        let Some(generated) = metadata.build_info.generated_files.get(&path.dst) else {
            return true;
        };
        if !path.dst.exists() {
            return true;
        }
        let modified = fs::metadata(&path.src)
            .and_then(|x| x.modified())
            .unwrap_or(SystemTime::now());
        modified.max(updated_time.unwrap_or(modified)) > *generated
    }

    /// Takes the already-read source contents for a file.
    pub fn take_input(&mut self, src: &Path) -> Option<String> {
        self.inputs.remove(src)
    }

    /// Tries to replace parse + pass1 by restoring the cached fragment.
    /// Returns `true` on success; the file then needs no pass2/emit either
    /// (guaranteed by the miss-set construction). On failure the file's
    /// partial state is dropped and the caller proceeds normally.
    pub fn try_restore(&mut self, path: &PathSet) -> bool {
        if self.miss.contains(&path.src) {
            return false;
        }
        let src = path.src.to_string_lossy().to_string();
        let Some(entry) = self.store.entry(&src) else {
            return false;
        };
        let Some(bytes) = self.store.load(entry) else {
            debug!("Failed to load fragment ({src})");
            self.miss.insert(path.src.clone());
            return false;
        };
        // Diagnostics need no ID codec, so validate before restoring analyzer state.
        let diagnostics = if entry.diagnostics.is_some() {
            let Some(bytes) = self.store.load_diagnostics(entry) else {
                debug!("Failed to load diagnostics ({src})");
                self.miss.insert(path.src.clone());
                return false;
            };
            match fragment_cache::restore_diagnostics(&bytes) {
                Ok(diagnostics) => diagnostics,
                Err(x) => {
                    debug!("Failed to decode diagnostics ({src}): {x}");
                    self.miss.insert(path.src.clone());
                    return false;
                }
            }
        } else {
            Vec::new()
        };
        let Ok(fragment) = Fragment::from_bytes(&bytes) else {
            debug!("Failed to decode fragment ({src})");
            self.miss.insert(path.src.clone());
            return false;
        };

        // What analyze_pass1 would otherwise register for the project.
        let prj: StrId = path.prj.as_str().into();
        let is_root = path.prj == self.root_project;
        scope::set_project(prj, is_root);

        match fragment_cache::restore(&fragment, prj) {
            Ok(window) => {
                let path_id = resource_table::insert_path(&path.src);
                unit_cache::set_file_window(path_id, window);
                if let Some(bytes) = self.store.load_units(entry) {
                    unit_cache::decode_file(path_id, &bytes);
                }
                self.store.keep(&src);
                self.restored += 1;
                self.restored_paths.insert(path.src.clone());
                self.inputs.remove(&path.src);
                self.restored_diagnostics.extend(diagnostics);
                true
            }
            Err(x) => {
                debug!("Failed to restore fragment ({src}): {x}");
                Analyzer::drop_file(resource_table::insert_path(&path.src), Some(prj));
                self.miss.insert(path.src.clone());
                false
            }
        }
    }

    /// Captures the file's pass1 output into the store. `cacheable` must
    /// be false when pass1 produced an error. Its warnings are kept with the
    /// file's other diagnostics by `save`.
    pub fn capture(
        &mut self,
        path: &PathSet,
        input: &str,
        watermark: &FragmentWatermark,
        cacheable: bool,
    ) {
        let src = path.src.to_string_lossy().to_string();
        let Some(hash) = self.hashes.get(&path.src).cloned() else {
            return;
        };
        unit_cache::set_file_window(
            resource_table::insert_path(&path.src),
            fragment_cache::window_since(watermark),
        );

        let blob = if cacheable {
            match fragment_cache::capture(&path.src, input, watermark) {
                Ok(fragment) => fragment.to_bytes().ok(),
                Err(x) => {
                    debug!("Non-cacheable file ({src}): {x}");
                    None
                }
            }
        } else {
            None
        };

        self.store.put(src, hash, blob.as_deref());
    }

    /// The files restored this build, whose components may be replayed.
    pub fn restored_files(&self) -> impl Iterator<Item = &PathBuf> {
        self.restored_paths.iter()
    }

    /// Drains the diagnostics of files restored this build, for the caller to
    /// re-report. Call once, after the restore pass.
    pub fn take_restored_diagnostics(&mut self) -> Vec<CachedDiagnostic> {
        std::mem::take(&mut self.restored_diagnostics)
    }

    /// Returns the restored files whose generic instances differ from the last
    /// analysis. Call after post_pass1, which registers the instances.
    ///
    /// The instances of a declaration are registered by the referencing
    /// files, so a restored file may need analyzing (and emitting) for them
    /// even though neither it nor anything it depends on changed. A run that
    /// doesn't emit records the change in `updated_time` for `dst_is_stale`,
    /// so the next run doesn't analyze it again but the next build emits it.
    pub fn generic_instance_changes(&mut self) -> HashSet<PathBuf> {
        let current = self
            .generic_instance_hashes
            .get_or_insert_with(generic_instance_hashes);
        let empty = veryl_cache::content_hash(&[]);
        self.restored_paths
            .iter()
            .filter(|path| {
                let previous = self
                    .store
                    .entry(&path.to_string_lossy())
                    .map(|x| x.generic_instance_hash.as_str());
                let current = current.get(*path).unwrap_or(&empty);
                previous != Some(current.as_str())
            })
            .cloned()
            .collect()
    }

    /// Persists the manifest (dependents + per-file tests); call only after a
    /// successful build. `diagnosed` maps each warning file (errors abort
    /// earlier) to its diagnostics, cached so a warm run re-reports them.
    pub fn save(&mut self, diagnosed: &HashMap<PathBuf, Vec<CachedDiagnostic>>) {
        let dependent_files = type_dag::dependent_files();
        for (path, dependents) in dependent_files {
            let Some(src) = resource_table::get_path_value(path) else {
                continue;
            };
            let dependents = dependents
                .iter()
                .filter_map(|x| resource_table::get_path_value(*x))
                .map(|x| x.to_string_lossy().to_string())
                .collect();
            self.store
                .set_dependents(&src.to_string_lossy(), dependents);
        }

        let generic_instances = self
            .generic_instance_hashes
            .take()
            .unwrap_or_else(generic_instance_hashes);
        let empty = veryl_cache::content_hash(&[]);
        for path in self.hashes.keys() {
            let src = path.to_string_lossy();
            let hash = generic_instances.get(path).unwrap_or(&empty).clone();
            let updated_time = match self.store.entry(&src) {
                Some(x) if x.generic_instance_hash == hash => x.updated_time,
                _ => Some(self.started_at),
            };
            self.store.set_generic_instance_hash(&src, hash);
            self.store.set_updated_time(&src, updated_time);
        }

        // Only the root project's tests are ever simulated.
        let mut tests: HashMap<PathBuf, (Vec<String>, bool)> = HashMap::new();
        for (name, property) in symbol_table::get_tests(&self.root_project) {
            if let Some(src) = resource_table::get_path_value(property.path) {
                let entry = tests.entry(src).or_default();
                entry.0.push(name.to_string());
                entry.1 |= !matches!(property.r#type, TestType::Native);
            }
        }
        for (src, (names, has_non_native_test)) in tests {
            self.store
                .set_tests(&src.to_string_lossy(), names, has_non_native_test);
        }

        for path in self.hashes.keys() {
            if let Some(blob) = unit_cache::encode_file(resource_table::insert_path(path)) {
                self.store.set_units(&path.to_string_lossy(), &blob);
            }
        }

        // Restored files' diagnostics are already preserved by `Store::keep`,
        // so only freshly analyzed files appear here.
        for (src, diagnostics) in diagnosed {
            match fragment_cache::capture_diagnostics(diagnostics) {
                Ok(blob) => self.store.set_diagnostics(&src.to_string_lossy(), &blob),
                Err(x) => debug!("Failed to capture diagnostics ({}): {x}", src.display()),
            }
        }

        self.store.save();
    }
}

/// Hashes, per file, the mangled names of the generic instances of the
/// declarations in it.
fn generic_instance_hashes() -> HashMap<PathBuf, String> {
    let mut names: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for symbol in symbol_table::get_generic_instance_owners() {
        let Some(path) = symbol
            .token
            .source
            .get_path()
            .and_then(resource_table::get_path_value)
        else {
            continue;
        };
        let entry = names.entry(path).or_default();
        for map in symbol.generic_maps() {
            entry.push(map.name(true, true));
        }
    }

    names
        .into_iter()
        .map(|(path, mut names)| {
            names.sort();
            let hash = veryl_cache::content_hash(names.join("\n").as_bytes());
            (path, hash)
        })
        .collect()
}

/// Builds the global invalidation key from everything that influences
/// pass1 output but is not per-file.
fn global_key(metadata: &Metadata, defines: &[String]) -> Option<String> {
    let build = toml::to_string(&metadata.build).ok()?;
    let lint = toml::to_string(&metadata.lint).ok()?;
    let lockfile = fs::read_to_string(&metadata.lockfile_path).unwrap_or_default();
    let defines = defines.join("\x1f");
    // Keyed on the binary itself, not just the version (see binary_fingerprint).
    let binary = veryl_cache::binary_fingerprint()?;
    Some(veryl_cache::global_key(&[
        veryl_metadata::VERYL_VERSION,
        &binary,
        &metadata.project.name,
        &build,
        &lint,
        &lockfile,
        &defines,
    ]))
}

#[cfg(test)]
mod tests;
