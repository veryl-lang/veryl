//! A dependency's `#[test]` modules are excluded from the consumer's
//! analysis and test collection, like dependency tests in cargo. In
//! particular a dependency testbench may use its own verification component
//! by bare name (`$comp::name`), which resolves only in the
//! dependency's own context.

use std::fs;
use std::path::Path;
use veryl::pipeline::{self, AnalyzeOptions};
use veryl_metadata::{Git, Metadata};

const GIT_IGNORE: &str = r#"
Veryl.lock
"#;

const VIP_TOML: &str = r#"
[project]
name = "vip"
version = "0.1.0"

[build]
exclude_std = true

[publish]
bump_commit = true
publish_commit = true
"#;

const VIP_SRC: &str = r#"
pub module Checker {
    var a: logic;
    assign a = 1;
}

#[test(test_checker)]
module test_checker {
    inst c: $comp::edge_checker;

    initial {
        $finish();
    }
}
"#;

const MAIN_TOML: &str = r#"
[project]
name = "main"
version = "0.1.0"

[build]
exclude_std = true

[dependencies]
vip = {git = "file://{}/vip", version = "0.1.0"}
"#;

const MAIN_SRC: &str = r#"
module Top {
    inst c: vip::Checker;
}
"#;

fn create_project(root: &Path, name: &str, toml: &str, src: &str, publish: bool) -> Metadata {
    unsafe {
        std::env::set_var("GIT_AUTHOR_NAME", "veryl");
        std::env::set_var("GIT_AUTHOR_EMAIL", "veryl");
        std::env::set_var("GIT_COMMITTER_NAME", "veryl");
        std::env::set_var("GIT_COMMITTER_EMAIL", "veryl");
    }

    let path = root.join(name);
    fs::create_dir(&path).unwrap();
    let toml_path = path.join("Veryl.toml");
    fs::write(
        &toml_path,
        toml.replace("{}", &root.to_string_lossy().replace("\\", "/")),
    )
    .unwrap();
    let src_path = path.join(format!("{name}.veryl"));
    fs::write(&src_path, src).unwrap();
    let git_ignore_path = path.join(".gitignore");
    fs::write(&git_ignore_path, GIT_IGNORE).unwrap();
    let git = Git::init(&path).unwrap();
    git.add(&toml_path).unwrap();
    git.add(&src_path).unwrap();
    git.add(&git_ignore_path).unwrap();
    git.commit("Initial commit").unwrap();
    let mut metadata = Metadata::load(&toml_path).unwrap();
    if publish {
        metadata.publish().unwrap();
    }
    metadata
}

#[test]
fn dependency_tests_are_excluded() {
    let tempdir = tempfile::tempdir().unwrap();
    create_project(tempdir.path(), "vip", VIP_TOML, VIP_SRC, true);
    let mut metadata = create_project(tempdir.path(), "main", MAIN_TOML, MAIN_SRC, false);

    let paths = metadata.paths::<&str>(&[], true, true).unwrap();

    let options = AnalyzeOptions {
        defines: &[],
        emit_mode: false,
        incremental: false,
        fail_fast: true,
    };
    let output = pipeline::analyze(&metadata, &paths, options, None, None).unwrap();
    output.check_error.check_all().unwrap();

    // The consumer's `veryl test` collects no tests from the dependency.
    assert!(veryl_analyzer::symbol_table::get_tests("main").is_empty());
    assert!(veryl_analyzer::symbol_table::get_tests("vip").is_empty());

    // Emission of the dependency source skips its testbench.
    for context in &output.contexts {
        let path = &context.path;
        let mut emitter =
            veryl_emitter::Emitter::new(&metadata, &path.prj, &path.src, &path.dst, &path.map);
        emitter.emit(&context.parser.veryl, &context.input);
        if path.prj == "vip" {
            assert!(emitter.as_str().contains("module vip_Checker"));
            assert!(!emitter.as_str().contains("test_checker"));
        }
    }
}
