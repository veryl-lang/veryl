//! A local root override must satisfy transitive references to the same release.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use veryl::pipeline::{self, AnalyzeOptions};
use veryl_metadata::{Git, Metadata};

fn project(
    root: &Path,
    directory: &str,
    name: &str,
    dependencies: &str,
    source: &str,
    publish: bool,
) -> Metadata {
    let path = root.join(directory);
    fs::create_dir(&path).unwrap();
    let manifest = path.join("Veryl.toml");
    fs::write(
        &manifest,
        format!(
            "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\
             [build]\nexclude_std = true\n\
             [publish]\npublish_commit = true\nregister = false\n\
             [dependencies]\n{dependencies}\n"
        ),
    )
    .unwrap();
    let source_path = path.join("source.veryl");
    fs::write(&source_path, source).unwrap();
    fs::write(path.join(".gitignore"), "Veryl.lock\n").unwrap();
    let git = Git::init(&path).unwrap();
    for (key, value) in [("user.name", "veryl"), ("user.email", "veryl@example.com")] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(["config", key, value])
                .status()
                .unwrap()
                .success()
        );
    }
    git.add(&manifest).unwrap();
    git.add(&source_path).unwrap();
    git.add(&path.join(".gitignore")).unwrap();
    git.commit("Fixture").unwrap();
    let mut metadata = Metadata::load(manifest).unwrap();
    if publish {
        metadata.publish().unwrap();
    }
    metadata
}

#[test]
fn local_override_resolves_transitive_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    project(
        root,
        "leaf",
        "leaf",
        "",
        "pub package Values { const WIDTH: u32 = 4; }",
        true,
    );
    project(
        root,
        "local",
        "leaf",
        "",
        "pub package Values { const WIDTH: u32 = 8; }",
        false,
    );
    let url = |name: &str| {
        format!(
            "file://{}/{}",
            root.to_string_lossy().replace('\\', "/"),
            name
        )
    };
    project(
        root,
        "parent",
        "parent",
        &format!("leaf = {{git = {:?}, version = \"=0.1.0\"}}", url("leaf")),
        "pub module Pass(i: input logic<leaf::Values::WIDTH>, o: output logic<leaf::Values::WIDTH>) { assign o = i; }",
        true,
    );
    let mut metadata = project(
        root,
        "consumer",
        "consumer",
        &format!(
            "selected = {{git = {:?}, project = \"leaf\", version = \"=0.1.0\", path = \"../local\"}}\n\
             parent = {{git = {:?}, version = \"=0.1.0\"}}",
            url("leaf"),
            url("parent")
        ),
        "module Top(i: input logic<selected::Values::WIDTH>, o: output logic<selected::Values::WIDTH>) { inst p: parent::Pass(i, o); }",
        false,
    );
    let paths = metadata.paths::<&str>(&[], true, true).unwrap();
    let expected: PathBuf = root.join("local/source.veryl").canonicalize().unwrap();
    assert!(
        paths
            .iter()
            .any(|p| p.prj == "selected" && p.src == expected)
    );
    let options = AnalyzeOptions {
        defines: &[],
        output_intent: veryl::incremental::OutputIntent::Never,
        incremental: false,
        fail_fast: true,
        export: veryl::pipeline::ExportStep::None,
    };
    let output = pipeline::analyze(&metadata, &paths, options, None, None).unwrap();
    output.check_error.check_all().unwrap();
    let selected = output
        .contexts
        .iter()
        .find(|p| p.path.prj == "selected")
        .unwrap();
    assert!(selected.input.contains("WIDTH: u32 = 8"));
    assert_eq!(metadata.lockfile.projects().len(), 2);
}
