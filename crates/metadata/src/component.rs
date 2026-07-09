use crate::ComponentManifest;
use crate::component_manifest::{COMMITTED_MANIFEST_FILE, parse_library_manifest};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A cargo package providing user-defined verification components,
/// declared as a `[[components]]` entry. Every name the package exports
/// with `veryl_component_export!` becomes available as `$comp::<name>`
/// in `#[test]` modules.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    /// Path to the component's cargo package, relative to the directory
    /// containing Veryl.toml.
    pub path: PathBuf,
    /// Optional committed prebuilt wasm binary.
    #[serde(default)]
    pub wasm: Option<PathBuf>,
}

impl Component {
    /// Enumerates the export names and interface manifests this package
    /// provides. A single source is used wholly — the newer of the build
    /// sidecar under `target_dir` and the committed `veryl.manifest.json`
    /// (by file mtime), then the prebuilt wasm's manifest section — so
    /// exports removed from the sources do not linger from a staler file.
    /// Non-identifier names are dropped with a warning (see
    /// [`veryl_component_sys::is_valid_component_name`]).
    pub fn collect_manifests(
        &self,
        root: &Path,
        target_dir: &Path,
    ) -> Vec<(String, ComponentManifest)> {
        let crate_dir = root.join(&self.path);
        let sidecar =
            component_crate_name(&crate_dir).map(|name| sidecar_manifest_path(target_dir, &name));
        let committed = crate_dir.join(COMMITTED_MANIFEST_FILE);
        let found =
            read_newest_manifest_file(&[sidecar.as_deref(), Some(&committed)]).or_else(|| {
                let wasm = std::fs::read(root.join(self.wasm.as_ref()?)).ok()?;
                ComponentManifest::parse_all_from_wasm(&wasm)
            });
        let mut ret: Vec<_> = found
            .unwrap_or_default()
            .into_iter()
            .filter(|(name, _)| {
                let valid = veryl_component_sys::is_valid_component_name(name);
                if !valid {
                    log::warn!(
                        "component export `{name}` in {} is not an identifier and cannot be referenced as $comp::<name>; ignored",
                        self.path.display()
                    );
                }
                valid
            })
            .collect();
        ret.sort_by(|a, b| a.0.cmp(&b.0));
        ret
    }
}

/// The `[package].name` of the cargo package at `crate_dir`.
pub fn component_crate_name(crate_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(crate_dir.join("Cargo.toml")).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    Some(value.get("package")?.get("name")?.as_str()?.to_string())
}

/// Path of the build-output manifest sidecar for a component crate. The
/// name derives from the cargo package name — not the built artifact — so
/// the writer (`veryl test`) and this reader agree regardless of platform
/// library prefixes or a `[lib] name` override.
pub fn sidecar_manifest_path(target_dir: &Path, crate_name: &str) -> PathBuf {
    let snake = crate_name.replace('-', "_");
    target_dir
        .join("release")
        .join(format!("{snake}.manifest.json"))
}

/// Reads every export from the committed `veryl.manifest.json` in the
/// component crate.
pub fn read_committed_manifests(crate_dir: &Path) -> Option<HashMap<String, ComponentManifest>> {
    read_manifest_file(&crate_dir.join(COMMITTED_MANIFEST_FILE))
}

/// Reads an aggregated manifest file; an absent, unparsable or empty one
/// counts as no source at all so a fallback can take over.
fn read_manifest_file(path: &Path) -> Option<HashMap<String, ComponentManifest>> {
    let json = std::fs::read_to_string(path).ok()?;
    let manifests = parse_library_manifest(&json);
    (!manifests.is_empty()).then_some(manifests)
}

/// Reads the most recently modified of the candidate manifest files that
/// parses to a non-empty type map. Recency decides between a build
/// sidecar and a committed manifest: a fresh checkout makes the committed
/// file newer than a leftover sidecar, and a local build the reverse.
fn read_newest_manifest_file(
    candidates: &[Option<&Path>],
) -> Option<HashMap<String, ComponentManifest>> {
    let mut found: Vec<(SystemTime, &Path)> = candidates
        .iter()
        .flatten()
        .filter_map(|path| {
            let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
            Some((mtime, *path))
        })
        .collect();
    found.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    found
        .into_iter()
        .find_map(|(_, path)| read_manifest_file(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(ty: &str) -> String {
        format!(r#"{{"types":{{"{ty}":{{"kind":"clocked"}}}}}}"#)
    }

    fn setup(dir: &Path) -> Component {
        std::fs::create_dir_all(dir.join("crate")).unwrap();
        std::fs::create_dir_all(dir.join("target/release")).unwrap();
        std::fs::write(
            dir.join("crate/Cargo.toml"),
            "[package]\nname = \"demo-comp\"\n",
        )
        .unwrap();
        Component {
            path: "crate".into(),
            wasm: None,
        }
    }

    fn names(entry: &Component, root: &Path) -> Vec<String> {
        entry
            .collect_manifests(root, &root.join("target"))
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    }

    #[test]
    fn newest_manifest_source_wins() {
        let dir = std::env::temp_dir().join(format!("veryl_newest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let entry = setup(&dir);
        let committed = dir.join("crate").join(COMMITTED_MANIFEST_FILE);
        let sidecar = sidecar_manifest_path(&dir.join("target"), "demo-comp");

        std::fs::write(&committed, manifest_json("from_committed")).unwrap();
        assert_eq!(names(&entry, &dir), ["from_committed"]);

        // A later build sidecar shadows the committed manifest...
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&sidecar, manifest_json("from_sidecar")).unwrap();
        assert_eq!(names(&entry, &dir), ["from_sidecar"]);

        // ...until the committed manifest is refreshed (e.g. a checkout).
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&committed, manifest_json("from_committed")).unwrap();
        assert_eq!(names(&entry, &dir), ["from_committed"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_identifier_export_names_are_dropped() {
        let dir = std::env::temp_dir().join(format!("veryl_names_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let entry = setup(&dir);
        std::fs::write(
            dir.join("crate").join(COMMITTED_MANIFEST_FILE),
            r#"{"types":{"ok_name":{"kind":"clocked"},"bus::monitor":{"kind":"clocked"},"1bad":{}}}"#,
        )
        .unwrap();
        assert_eq!(names(&entry, &dir), ["ok_name"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
