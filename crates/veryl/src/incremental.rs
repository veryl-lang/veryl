//! Incremental build driver: per file, decides whether to restore a cached
//! fragment instead of parse + pass1, and maintains the on-disk store.
//!
//! A file misses (needs full parse + pass1, maybe pass2/emit) if its hash
//! changed, it produced diagnostics, its output is stale (`check_skip`'s
//! predicate), or anything it depends on missed (transitive, from the
//! previous build's dependency map). The miss set is a superset of the
//! files needing pass2/emit, so a restored file is never asked for its AST.

use log::debug;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use veryl_analyzer::fragment_cache::{self, Fragment, FragmentWatermark};
use veryl_analyzer::{
    attribute_table, definition_table, namespace_table, symbol_table, type_dag, unsafe_table,
};
use veryl_cache::Store;
use veryl_metadata::Metadata;
use veryl_parser::{resource_table, text_table};
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
}

impl Drop for Incremental {
    fn drop(&mut self) {
        LAST_RESTORED.with(|x| x.set(self.restored));
    }
}

impl Incremental {
    /// Opens the store and computes the miss set. Returns `None` when
    /// incremental builds are disabled.
    pub fn open(metadata: &Metadata, paths: &[PathSet], defines: &[String]) -> Option<Incremental> {
        if !metadata.build.incremental {
            return None;
        }

        // On a compiler version change every output must be re-emitted
        // (`check_skip` is disabled then for the same reason), so nothing
        // may be restored. Fragments are still captured for the next build.
        let version_match = metadata.build_info.veryl_version_match();

        let key = global_key(metadata, defines)?;
        let store = Store::open(&metadata.project_dot_build_path().join("cache"), &key);

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

            let hit = version_match
                && store
                    .entry(&path.src.to_string_lossy())
                    .is_some_and(|x| x.hash == hash && x.fragment.is_some())
                && !Self::dst_is_stale(metadata, path);
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
        })
    }

    /// Same staleness predicate as `CmdBuild::check_skip` (plus an
    /// existence check): the build output is missing or older than the
    /// source.
    fn dst_is_stale(metadata: &Metadata, path: &PathSet) -> bool {
        let Some(generated) = metadata.build_info.generated_files.get(&path.dst) else {
            return true;
        };
        if !path.dst.exists() {
            return true;
        }
        let modified = fs::metadata(&path.src)
            .and_then(|x| x.modified())
            .unwrap_or(SystemTime::now());
        modified > *generated
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
        let Ok(fragment) = Fragment::from_bytes(&bytes) else {
            debug!("Failed to decode fragment ({src})");
            self.miss.insert(path.src.clone());
            return false;
        };

        // What analyze_pass1 would otherwise register for the project.
        let is_root = path.prj == self.root_project;
        namespace_table::set_project(path.prj.as_str().into(), is_root);

        match fragment_cache::restore(&fragment) {
            Ok(()) => {
                self.store.keep(&src);
                self.restored += 1;
                self.inputs.remove(&path.src);
                true
            }
            Err(x) => {
                debug!("Failed to restore fragment ({src}): {x}");
                drop_file_state(&path.src);
                self.miss.insert(path.src.clone());
                false
            }
        }
    }

    /// Captures the file's pass1 output into the store. `cacheable` must
    /// be false when pass1 produced any diagnostics (they would be lost on
    /// restore).
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

    /// Records the dependency map of this build and persists the manifest.
    /// Call only after a successful build.
    pub fn save(&mut self) {
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

        self.store.save();
    }
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

/// Removes everything a partially restored file may have left in the
/// global tables (same set as the language server's file drop).
fn drop_file_state(src: &Path) {
    let path = resource_table::insert_path(src);
    symbol_table::drop(path);
    namespace_table::drop(path);
    text_table::drop(path);
    attribute_table::drop(path);
    unsafe_table::drop(path);
    definition_table::drop(path);
}
