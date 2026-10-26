use super::*;
use crate::pipeline::{self, AnalyzeOptions, Diag};

const SOURCE: &str =
    "module Top (data: input logic<8>, result: output logic<8>) { assign result = data >>> 1; }";

struct Project {
    _directory: tempfile::TempDir,
    metadata: Metadata,
    paths: Vec<PathSet>,
}

impl Project {
    fn new(source: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("Veryl.toml");
        fs::write(&manifest, "[project]\nname = \"diagnostic_cache\"\nversion = \"0.1.0\"\n[build]\nexclude_std = true\nincremental = true\n").unwrap();
        fs::write(directory.path().join("top.veryl"), source).unwrap();
        let mut metadata = Metadata::load(&manifest).unwrap();
        let paths = metadata.paths::<&str>(&[], true, true).unwrap();
        assert_eq!(paths.len(), 1);
        Self {
            _directory: directory,
            metadata,
            paths,
        }
    }

    fn analyze(&self) -> (usize, Vec<CachedDiagnostic>) {
        Analyzer::new(&self.metadata).clear();
        let options = AnalyzeOptions {
            defines: &[],
            output_intent: OutputIntent::Never,
            incremental: true,
            fail_fast: true,
        };
        let mut output =
            pipeline::analyze(&self.metadata, &self.paths, options, None, None).unwrap();
        let diagnostics = output
            .check_error
            .related
            .iter()
            .map(|diag| match diag {
                Diag::Analyzer(error) => CachedDiagnostic::from_error(error),
                Diag::Cached(diag) => diag.clone(),
            })
            .collect();
        let inc = output.incremental.as_mut().unwrap();
        let restored = inc.restored;
        inc.save(&pipeline::collect_diagnosed(&output.check_error));
        (restored, diagnostics)
    }

    fn store(&self) -> Store {
        Store::open(
            &self.metadata.project_dot_build_path().join("cache"),
            &global_key(&self.metadata, &[]).unwrap(),
        )
    }

    fn diagnostic_blob(&self) -> PathBuf {
        let store = self.store();
        let entry = store.entry(&self.paths[0].src.to_string_lossy()).unwrap();
        self.metadata
            .project_dot_build_path()
            .join("cache")
            .join(entry.diagnostics.as_ref().unwrap())
    }
}

fn check_recovery(damage: impl FnOnce(&Project)) {
    let project = Project::new(SOURCE);
    let (restored, expected) = project.analyze();
    assert_eq!(restored, 0);
    assert!(!expected.is_empty());
    assert_eq!(project.analyze(), (1, expected.clone()));
    damage(&project);
    assert_eq!(project.analyze(), (0, expected.clone()));
    assert_eq!(project.analyze(), (1, expected));
}

#[test]
fn corrupted_diagnostics_are_reanalyzed_and_repaired() {
    check_recovery(|project| {
        let path = project.diagnostic_blob();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
    });
}

#[test]
fn missing_diagnostics_are_reanalyzed_and_repaired() {
    check_recovery(|project| fs::remove_file(project.diagnostic_blob()).unwrap());
}

#[test]
fn undecodable_diagnostics_are_reanalyzed_and_repaired() {
    check_recovery(|project| {
        let mut store = project.store();
        let src = project.paths[0].src.to_string_lossy();
        store.keep(&src);
        store.set_diagnostics(&src, &[0xff]);
        store.save();
        let bytes = store.load_diagnostics(store.entry(&src).unwrap()).unwrap();
        assert!(fragment_cache::restore_diagnostics(&bytes).is_err());
    });
}

#[test]
fn files_without_diagnostics_still_restore() {
    let project = Project::new("module Top {}");
    assert_eq!(project.analyze(), (0, vec![]));
    assert_eq!(project.analyze(), (1, vec![]));
}
