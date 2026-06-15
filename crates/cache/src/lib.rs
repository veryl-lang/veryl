//! On-disk content-addressed blob store for incremental compilation.
//!
//! Lives in `.build/cache`: a `manifest.toml` index plus one blob per
//! source file under `fragments/`. The blob contents are opaque (pass1
//! fragments today). Non-per-file invalidation inputs (compiler version,
//! build options, lockfile) fold into one `global_key` — a mismatch
//! discards the store; per-file validity is the source's BLAKE3 hash.
//! Every failure degrades to a miss; the store never fails a build.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST: &str = "manifest.toml";
const FRAGMENT_DIR: &str = "fragments";
const FRAGMENT_EXT: &str = "frag";
/// Bump on any incompatible change to the blob or manifest layout.
pub const SCHEMA_VERSION: u32 = 1;
const BLOB_MAGIC: &[u8; 4] = b"VFRG";

/// Returns the BLAKE3 hash of `data` as a hex string.
pub fn content_hash(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// Builds a global key from all non-per-file invalidation inputs.
pub fn global_key(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

/// Hash of the running binary, for use as a global-key part. The version
/// string alone cannot tell apart two binaries built from different
/// sources (developer builds, nightlies), and cached blobs depend on the
/// exact data layout of the binary that wrote them.
pub fn binary_fingerprint() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let data = fs::read(exe).ok()?;
    Some(content_hash(&data))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// BLAKE3 hex hash of the source file contents.
    pub hash: String,
    /// Blob path relative to the store root; `None` if the file was
    /// analyzed but is not cacheable (e.g. it produced diagnostics).
    pub fragment: Option<String>,
    /// Source paths of all files that (transitively) depend on this file,
    /// from the last successful build.
    pub dependents: Vec<String>,
    /// Names of the tests declared in this file, from the last successful
    /// build. Files containing selected tests need pass2 (their IR is
    /// simulated), so they must not be restored from a fragment.
    #[serde(default)]
    pub tests: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    global_key: String,
    files: BTreeMap<String, FileEntry>,
}

pub struct Store {
    root: PathBuf,
    manifest: Manifest,
    /// Entries for the build in progress; replaces `manifest.files` on save.
    next_files: BTreeMap<String, FileEntry>,
    /// True when the on-disk manifest matches `manifest` in memory, so an
    /// identical re-scan can skip the write. False after a fresh init or a
    /// key/schema mismatch, where the next save must rewrite and GC.
    on_disk_current: bool,
    #[cfg(not(target_family = "wasm"))]
    _lock: Option<fs::File>,
}

impl Store {
    /// Opens (or initializes) the store at `root`, waiting for the
    /// exclusive lock. A schema or global-key mismatch discards all
    /// existing entries.
    pub fn open(root: &Path, global_key: &str) -> Store {
        Self::open_with_lock(root, global_key, true).unwrap()
    }

    /// Like [`Store::open`], but returns `None` instead of waiting when
    /// another process holds the store. For long-lived callers (the
    /// language server) that must not block on a running build.
    pub fn try_open(root: &Path, global_key: &str) -> Option<Store> {
        Self::open_with_lock(root, global_key, false)
    }

    fn open_with_lock(root: &Path, global_key: &str, blocking: bool) -> Option<Store> {
        let _ = fs::create_dir_all(root.join(FRAGMENT_DIR));

        #[cfg(not(target_family = "wasm"))]
        let lock = match acquire_lock(root, blocking) {
            LockResult::Acquired(x) => Some(x),
            // Without a lock the store can still be used; writes stay
            // atomic and readers degrade to misses.
            LockResult::Unavailable if blocking => None,
            LockResult::Unavailable => return None,
        };
        #[cfg(target_family = "wasm")]
        let _ = blocking;

        let parsed = fs::read_to_string(root.join(MANIFEST))
            .ok()
            .and_then(|x| toml::from_str::<Manifest>(&x).ok());

        let mut manifest = parsed.clone().unwrap_or_default();
        let mut on_disk_current = parsed.is_some()
            && manifest.schema == SCHEMA_VERSION
            && manifest.global_key == global_key;

        if !on_disk_current {
            if !manifest.files.is_empty() {
                log::debug!("cache: global key mismatch, discarding all entries");
            }
            // The on-disk manifest is absent, corrupt, or for a different
            // key; force the next save to rewrite it and GC stale blobs.
            manifest = Manifest::default();
            on_disk_current = false;
        }
        manifest.schema = SCHEMA_VERSION;
        manifest.global_key = global_key.to_string();

        Some(Store {
            root: root.to_path_buf(),
            manifest,
            next_files: BTreeMap::new(),
            on_disk_current,
            #[cfg(not(target_family = "wasm"))]
            _lock: lock,
        })
    }

    /// Looks up the previous build's entry for a source path.
    pub fn entry(&self, src: &str) -> Option<&FileEntry> {
        self.manifest.files.get(src)
    }

    /// Loads and verifies the blob behind an entry. Any failure is a miss.
    pub fn load(&self, entry: &FileEntry) -> Option<Vec<u8>> {
        let fragment = entry.fragment.as_ref()?;
        let data = fs::read(self.root.join(fragment)).ok()?;
        let payload = data.strip_prefix(BLOB_MAGIC.as_slice())?;
        let (version, payload) = payload.split_first_chunk::<4>()?;
        if u32::from_le_bytes(*version) != SCHEMA_VERSION {
            return None;
        }
        Some(payload.to_vec())
    }

    /// Records an entry for the build in progress. When `blob` is given it
    /// is written content-addressed; an identical existing blob is reused.
    pub fn put(&mut self, src: String, hash: String, blob: Option<&[u8]>) {
        let fragment = blob.and_then(|payload| {
            let mut data = Vec::with_capacity(payload.len() + 8);
            data.extend_from_slice(BLOB_MAGIC);
            data.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
            data.extend_from_slice(payload);

            let name = content_hash(&data);
            let rel = format!("{FRAGMENT_DIR}/{}/{}.{FRAGMENT_EXT}", &name[..2], name);
            let path = self.root.join(&rel);
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if let Err(x) = veryl_path::atomic_write(&path, &data) {
                    log::debug!("cache: failed to write blob {}: {x}", path.display());
                    return None;
                }
            }
            Some(rel)
        });

        self.next_files.insert(
            src,
            FileEntry {
                hash,
                fragment,
                dependents: Vec::new(),
                tests: Vec::new(),
            },
        );
    }

    /// Marks the file as analyzed this build but keeps its existing blob
    /// from the previous manifest (a cache hit).
    pub fn keep(&mut self, src: &str) {
        if let Some(entry) = self.manifest.files.get(src) {
            self.next_files.insert(src.to_string(), entry.clone());
        }
    }

    /// Drops a file's fragment (keeping its dependents/tests) so a warm run
    /// misses it — for files with pass2/post diagnostics the pass1-only fragment
    /// would hide. The orphaned blob is GC'd by `save`.
    pub fn invalidate(&mut self, src: &str) {
        if let Some(entry) = self.next_files.get_mut(src) {
            entry.fragment = None;
        }
    }

    /// Sets the dependents of a file for the build in progress.
    pub fn set_dependents(&mut self, src: &str, dependents: Vec<String>) {
        if let Some(entry) = self.next_files.get_mut(src) {
            entry.dependents = dependents;
        }
    }

    /// Sets the test names declared in a file for the build in progress.
    pub fn set_tests(&mut self, src: &str, tests: Vec<String>) {
        if let Some(entry) = self.next_files.get_mut(src) {
            entry.tests = tests;
        }
    }

    /// Writes the manifest for the build in progress and removes blobs no
    /// longer referenced by it. Call only after a successful build.
    pub fn save(&mut self) {
        // An unchanged re-scan rebuilds an identical entry set; skip the
        // write and GC walk. Guarded by `on_disk_current` so a key mismatch
        // (where stale blobs still need GC) never skips.
        if self.on_disk_current && self.next_files == self.manifest.files {
            self.next_files.clear();
            return;
        }
        self.manifest.files = std::mem::take(&mut self.next_files);

        let manifest = match toml::to_string(&self.manifest) {
            Ok(x) => x,
            Err(x) => {
                log::debug!("cache: failed to serialize manifest: {x}");
                return;
            }
        };
        if let Err(x) = veryl_path::atomic_write(self.root.join(MANIFEST), manifest.as_bytes()) {
            log::debug!("cache: failed to write manifest: {x}");
            return;
        }
        self.on_disk_current = true;

        self.gc();
    }

    /// Removes blobs not referenced by the current manifest.
    fn gc(&self) {
        let referenced: HashSet<PathBuf> = self
            .manifest
            .files
            .values()
            .filter_map(|x| x.fragment.as_ref())
            .map(|x| self.root.join(x))
            .collect();

        let Ok(dirs) = fs::read_dir(self.root.join(FRAGMENT_DIR)) else {
            return;
        };
        for dir in dirs.flatten() {
            let Ok(files) = fs::read_dir(dir.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|x| x == FRAGMENT_EXT)
                    && !referenced.contains(&path)
                {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
enum LockResult {
    Acquired(fs::File),
    Unavailable,
}

#[cfg(not(target_family = "wasm"))]
fn acquire_lock(root: &Path, blocking: bool) -> LockResult {
    let Ok(lock) = fs::File::create(root.join("lock")) else {
        return LockResult::Unavailable;
    };
    let locked = if blocking {
        fs4::FileExt::lock(&lock).is_ok()
    } else {
        fs4::FileExt::try_lock(&lock).is_ok()
    };
    if locked {
        LockResult::Acquired(lock)
    } else {
        log::debug!("cache: store is locked by another process");
        LockResult::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_gc() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");

        let mut store = Store::open(&root, "key1");
        store.put("a.veryl".to_string(), "h1".to_string(), Some(b"blob-a"));
        store.put("b.veryl".to_string(), "h2".to_string(), None);
        store.set_dependents("a.veryl", vec!["b.veryl".to_string()]);
        store.save();
        drop(store);

        let store = Store::open(&root, "key1");
        let entry = store.entry("a.veryl").unwrap();
        assert_eq!(entry.hash, "h1");
        assert_eq!(entry.dependents, vec!["b.veryl".to_string()]);
        assert_eq!(store.load(entry).unwrap(), b"blob-a");
        let entry = store.entry("b.veryl").unwrap();
        assert!(entry.fragment.is_none());
        drop(store);

        // Global key change discards everything; save() GCs the old blob.
        let mut store = Store::open(&root, "key2");
        assert!(store.entry("a.veryl").is_none());
        store.save();
        drop(store);

        let store = Store::open(&root, "key1");
        assert!(store.entry("a.veryl").is_none());
        let blobs: Vec<_> = walk_blobs(&root);
        assert!(blobs.is_empty(), "GC left blobs behind: {blobs:?}");
    }

    #[test]
    fn corrupt_blob_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");

        let mut store = Store::open(&root, "key");
        store.put("a.veryl".to_string(), "h".to_string(), Some(b"payload"));
        store.save();

        let rel = store.entry("a.veryl").unwrap().fragment.clone().unwrap();
        fs::write(root.join(&rel), b"garbage").unwrap();
        assert!(store.load(store.entry("a.veryl").unwrap()).is_none());

        // Truncated header
        fs::write(root.join(&rel), b"VF").unwrap();
        assert!(store.load(store.entry("a.veryl").unwrap()).is_none());
    }

    #[test]
    fn unchanged_rescan_skips_manifest_write() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        let manifest = root.join(MANIFEST);

        let mut store = Store::open(&root, "key");
        store.put("a.veryl".to_string(), "h".to_string(), Some(b"blob"));
        store.save();
        assert!(manifest.exists());

        // Delete the on-disk manifest; a re-scan that reproduces the same
        // in-memory entries must skip the write, leaving it absent.
        fs::remove_file(&manifest).unwrap();
        store.keep("a.veryl");
        store.save();
        assert!(!manifest.exists(), "identical re-scan rewrote the manifest");
        assert!(store.entry("a.veryl").is_some());

        // A real change does write.
        store.put("a.veryl".to_string(), "h2".to_string(), Some(b"blob2"));
        store.save();
        assert!(manifest.exists());
        drop(store);

        let reopened = Store::open(&root, "key");
        assert_eq!(reopened.entry("a.veryl").unwrap().hash, "h2");
    }

    fn walk_blobs(root: &Path) -> Vec<PathBuf> {
        let mut ret = vec![];
        if let Ok(dirs) = fs::read_dir(root.join(FRAGMENT_DIR)) {
            for dir in dirs.flatten() {
                if let Ok(files) = fs::read_dir(dir.path()) {
                    for file in files.flatten() {
                        ret.push(file.path());
                    }
                }
            }
        }
        ret
    }
}
