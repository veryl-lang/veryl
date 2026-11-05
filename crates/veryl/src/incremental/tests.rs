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
        Self::with_files(&[("top.veryl", source)], "")
    }

    fn with_manifest(source: &str, extra: &str) -> Self {
        Self::with_files(&[("top.veryl", source)], extra)
    }

    fn with_files(files: &[(&str, &str)], extra: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("Veryl.toml");
        fs::write(&manifest, format!("[project]\nname = \"diagnostic_cache\"\nversion = \"0.1.0\"\n[build]\nexclude_std = true\nincremental = true\n{extra}")).unwrap();
        for (name, source) in files {
            fs::write(directory.path().join(name), source).unwrap();
        }
        let mut metadata = Metadata::load(&manifest).unwrap();
        let paths = metadata.paths::<&str>(&[], true, true).unwrap();
        assert_eq!(paths.len(), files.len());
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

    /// Whether the analysis reports an error, which leaves the cache as it
    /// was.
    fn fails(&self) -> bool {
        Analyzer::new(&self.metadata).clear();
        let options = AnalyzeOptions {
            defines: &[],
            output_intent: OutputIntent::Never,
            incremental: true,
            fail_fast: true,
        };
        pipeline::analyze(&self.metadata, &self.paths, options, None, None).is_err()
    }

    fn write(&self, name: &str, source: &str) {
        fs::write(self._directory.path().join(name), source).unwrap();
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

#[test]
fn files_with_pass1_warnings_still_restore() {
    let project = Project::with_manifest(
        "module BadName {}",
        "[lint.naming]\ncase_module = \"snake\"\n",
    );
    let (restored, expected) = project.analyze();
    assert_eq!(restored, 0);
    assert!(!expected.is_empty());
    assert_eq!(project.analyze(), (1, expected));
}

#[test]
fn components_of_restored_files_are_replayed() {
    const SUB: &str = "module Sub (i: input logic, o: output logic) { assign o = i; }\n";
    const LEAF: &str = "module Leaf (o: output logic) { inst u: Sub (o); }\n";
    const TOP: &str = "module Top (o: output logic) { inst u: Leaf (o); }\n";
    let files = [("sub.veryl", SUB), ("leaf.veryl", LEAF), ("top.veryl", TOP)];
    let edited = format!("{TOP}// edited\n");

    let project = Project::with_files(&files, "");
    project.analyze();
    project.write("top.veryl", &edited);
    let (restored, diagnostics) = project.analyze();
    assert_eq!(restored, 2);
    assert!(unit_cache::stub_count() > 0);

    let cold = Project::with_files(&[files[0], files[1], ("top.veryl", &edited)], "");
    let (_, expected) = cold.analyze();
    assert!(!expected.is_empty());
    // By the directory's own name: `Debug` escapes the separators of the
    // whole path on Windows.
    let directory = |x: &Project| {
        x._directory
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    let paths = |diagnostics: Vec<CachedDiagnostic>| {
        diagnostics
            .iter()
            .map(|x| format!("{x:?}").replace(&directory(&project), &directory(&cold)))
            .collect::<Vec<_>>()
    };
    assert_eq!(paths(diagnostics), paths(expected));
}

#[test]
fn replay_follows_what_generic_arguments_name() {
    const SUB: &str = "module Sub (i: input logic, o: output logic) { assign o = i; }\n";
    const GEN: &str = "module Gen::<V: u32> (o: output logic) {\n    if V == 8 :g { inst u: Sub (o); } else { assign o = 0; }\n}\n";
    const TOP: &str = "module Top (o: output logic) { inst u: Gen::<PkgX::W> (o); }\n";
    let pkg = |w: u32| format!("package PkgX {{ const W: u32 = {w}; }}\n");
    let narrow = pkg(8);
    let files = [("sub.veryl", SUB), ("gen.veryl", GEN), ("top.veryl", TOP)];

    let project = Project::with_files(&[files[0], files[1], files[2], ("pkg.veryl", &pkg(16))], "");
    let (_, wide) = project.analyze();
    assert!(wide.is_empty());
    project.write("pkg.veryl", &narrow);
    let (_, diagnostics) = project.analyze();

    let cold = Project::with_files(&[files[0], files[1], files[2], ("pkg.veryl", &narrow)], "");
    let (_, expected) = cold.analyze();
    assert!(!expected.is_empty());
    assert_eq!(diagnostics.len(), expected.len());
}

#[test]
fn replay_keeps_conflicts_with_types_inferred_before() {
    // The library's own conversion enters `a` first; the replayed `M #(W: 1)`
    // only agrees with it, and that run of the library is not repeated.
    const LIB: &str = "module M #(param W: u32 = 1) (i: input logic<W>, o: output logic) {\n    var a;\n    assign a = i;\n    assign o = a[0];\n}\n";
    const TOP: &str = "module Top (i: input logic<4>, o: output logic<2>) {\n    inst u1: M #(W: 1) (i: i[0], o: o[0]);\n    assign o[1] = 0;\n}\n";
    let edited = TOP.replace(
        "assign o[1] = 0;",
        "inst u2: M #(W: 2) (i: i[1:0], o: o[1]);",
    );

    let project = Project::with_files(&[("lib.veryl", LIB), ("top.veryl", TOP)], "");
    assert_eq!(project.analyze(), (0, vec![]));
    project.write("top.veryl", &edited);
    assert!(project.fails());

    let cold = Project::with_files(&[("lib.veryl", LIB), ("top.veryl", &edited)], "");
    assert!(cold.fails());
}
