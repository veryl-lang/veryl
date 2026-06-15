//! Language-server fragment cache.
//!
//! Restores the background analysis's pass1 state from disk so opening a
//! file in a large project skips a full re-scan. Uses a store separate from
//! the build cache (`.build/cache-ls`), keyed on the `veryl-ls` binary and
//! opened non-blocking (a second `veryl-ls` just falls back to parsing).
//! Editor-open files are never cached (their buffer differs from disk);
//! only disk-backed background files are.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use veryl_analyzer::fragment_cache::{self, Fragment, FragmentWatermark};
use veryl_analyzer::{
    attribute_table, definition_table, namespace_table, symbol_table, type_dag, unsafe_table,
};
use veryl_cache::Store;
use veryl_metadata::Metadata;
use veryl_parser::{resource_table, text_table};
use veryl_path::PathSet;

pub struct LsIncremental {
    store: Store,
    root_project: String,
    pub restored: usize,
}

impl LsIncremental {
    /// Opens the language-server store for a project, or `None` when
    /// incremental builds are disabled or another `veryl-ls` holds it.
    /// Never blocks.
    pub fn open(metadata: &Metadata) -> Option<LsIncremental> {
        if !metadata.build.incremental {
            return None;
        }
        let key = global_key(metadata)?;
        let root = metadata.project_dot_build_path().join("cache-ls");
        let store = Store::try_open(&root, &key)?;
        Some(LsIncremental {
            store,
            root_project: metadata.project.name.clone(),
            restored: 0,
        })
    }

    /// Restores `path`'s pass1 state from the cache when its on-disk
    /// contents are unchanged. `text` is the content the caller already
    /// read. Returns `true` on a hit (the file then needs no parse/pass1).
    pub fn try_restore(&mut self, path: &PathSet, text: &str) -> bool {
        let src = path.src.to_string_lossy().to_string();
        let hash = veryl_cache::content_hash(text.as_bytes());

        let entry = self.store.entry(&src);
        if !entry.is_some_and(|x| x.hash == hash && x.fragment.is_some()) {
            return false;
        }
        let Some(bytes) = entry.and_then(|x| self.store.load(x)) else {
            return false;
        };
        let Ok(fragment) = Fragment::from_bytes(&bytes) else {
            return false;
        };

        // Clear any stale state from a previous analysis of this file
        // before re-registering it.
        drop_file_state(&path.src);

        let is_root = path.prj == self.root_project;
        namespace_table::set_project(path.prj.as_str().into(), is_root);

        match fragment_cache::restore(&fragment) {
            Ok(()) => {
                self.store.keep(&src);
                self.restored += 1;
                true
            }
            Err(_) => {
                drop_file_state(&path.src);
                false
            }
        }
    }

    /// Captures `path`'s pass1 output into the store. `cacheable` must be
    /// false when pass1 produced diagnostics.
    pub fn capture(
        &mut self,
        path: &PathSet,
        text: &str,
        watermark: &FragmentWatermark,
        cacheable: bool,
    ) {
        let src = path.src.to_string_lossy().to_string();
        let hash = veryl_cache::content_hash(text.as_bytes());

        let blob = if cacheable {
            fragment_cache::capture(&path.src, text, watermark)
                .ok()
                .and_then(|x| x.to_bytes().ok())
        } else {
            None
        };
        self.store.put(src, hash, blob.as_deref());
    }

    /// Persists the dependency map and manifest. Call after the project's
    /// `analyze_post_pass1`.
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

/// Builds the store's invalidation key. Mirrors the build cache, but keyed
/// on the `veryl-ls` binary (the fragment writer) instead of `veryl`.
fn global_key(metadata: &Metadata) -> Option<String> {
    let build = toml::to_string(&metadata.build).ok()?;
    let lint = toml::to_string(&metadata.lint).ok()?;
    let lockfile = fs::read_to_string(&metadata.lockfile_path).unwrap_or_default();
    let binary = veryl_cache::binary_fingerprint()?;
    Some(veryl_cache::global_key(&[
        veryl_metadata::VERYL_VERSION,
        &binary,
        &metadata.project.name,
        &build,
        &lint,
        &lockfile,
    ]))
}

/// Removes everything a file may have registered in the global tables
/// (same set as the server's `drop_tables`).
fn drop_file_state(src: &Path) {
    let path = resource_table::insert_path(src);
    symbol_table::drop(path);
    namespace_table::drop(path);
    text_table::drop(path);
    attribute_table::drop(path);
    unsafe_table::drop(path);
    definition_table::drop(path);
}

/// Per-project stores, so a server handling files from several projects
/// keeps one store each. Keyed by metadata path.
#[derive(Default)]
pub struct LsIncrementalMap {
    stores: HashMap<PathBuf, Option<LsIncremental>>,
}

impl LsIncrementalMap {
    pub fn get(&mut self, metadata: &Metadata) -> Option<&mut LsIncremental> {
        self.stores
            .entry(metadata.metadata_path.clone())
            .or_insert_with(|| LsIncremental::open(metadata))
            .as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use veryl_analyzer::Analyzer;
    use veryl_parser::Parser;

    const FILE_A: &str = "package P { const W: u32 = 8; }\n";
    const FILE_B: &str = "module M ( o: output logic<P::W> ) { assign o = 0; }\n";

    fn write_project(root: &std::path::Path) -> Metadata {
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            root.join("Veryl.toml"),
            r#"[project]
name = "lsinc"
version = "0.1.0"
[build]
sources = ["src"]
target = {type = "directory", path = "target"}
exclude_std = true
incremental = true
"#,
        )
        .unwrap();
        fs::write(src.join("a.veryl"), FILE_A).unwrap();
        fs::write(src.join("b.veryl"), FILE_B).unwrap();
        Metadata::load(root.join("Veryl.toml")).unwrap()
    }

    /// Runs the background-analysis pass1 over the project in a fresh
    /// thread (fresh thread-local tables/counters), using the disk store
    /// for restore/capture, and returns the symbol-table dump plus the
    /// number of files restored from cache.
    fn run(root: std::path::PathBuf) -> (String, usize) {
        let builder = thread::Builder::new().stack_size(16 * 1024 * 1024);
        builder
            .spawn(move || {
                let mut metadata = Metadata::load(root.join("Veryl.toml")).unwrap();
                let paths = metadata.paths::<&str>(&[], false, false).unwrap();
                let analyzer = Analyzer::new(&metadata);
                let mut inc = LsIncremental::open(&metadata).expect("store opens");

                for path in &paths {
                    let text = fs::read_to_string(&path.src).unwrap();
                    if inc.try_restore(path, &text) {
                        continue;
                    }
                    let watermark = fragment_cache::watermark();
                    let parser = Parser::parse(&text, &path.src).unwrap();
                    let errors = analyzer.analyze_pass1(&path.prj, &parser.veryl);
                    inc.capture(path, &text, &watermark, errors.is_empty());
                }
                Analyzer::analyze_post_pass1();
                inc.save();

                let restored = inc.restored;
                let dump = symbol_table::dump();
                // Drop the store (releases the lock) before the thread ends.
                drop(inc);
                (dump, restored)
            })
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn ls_store_restores_and_matches_cold() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_project(dir.path());
        let root = root.metadata_path.parent().unwrap().to_path_buf();

        let (cold_dump, cold_restored) = run(root.clone());
        assert_eq!(cold_restored, 0, "cold build restores nothing");

        let (warm_dump, warm_restored) = run(root.clone());
        assert_eq!(warm_restored, 2, "warm build restores both files");

        // Fresh-thread runs allocate identical ID ranges, so the restored
        // symbol table must match the freshly analyzed one byte-for-byte.
        assert_eq!(cold_dump, warm_dump);
    }
}
