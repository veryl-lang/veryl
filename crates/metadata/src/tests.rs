use crate::git::Git;
use crate::*;
use semver::Version;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GIT_IGNORE: &str = r#"
Veryl.lock
"#;

const TEST_TOML: &str = r#"
[project]
name = "test"
version = "0.1.0"

[build]
clock_type = "posedge"
reset_type = "async_low"
reset_low_suffix = "_n"
target = {type = "source"}
#target = {type = "directory", path = "aaa"}

[format]
indent_width = 4
"#;

const EXTENSION_EXTERNAL_TOOL_TOML: &str = r#"
[project]
name = "test"
version = "0.1.0"

[metadata.external_tool]
files = ["src/**/*.v"]
attrs = { role = "core" }
"#;

const EXTENSION_OTHER_TOOL_TOML: &str = r#"
[project]
name = "test"
version = "0.1.0"

[metadata.other_tool]
enabled = true
"#;

const UNKNOWN_TOP_LEVEL_TOML: &str = r#"
[project]
name = "test"
version = "0.1.0"

[unknown]
value = true
"#;

const MAIN_TOML: &str = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
sub1   = {git = "file://{}/sub1", version = "0.1.0"}
sub2   = {git = "file://{}/sub2", version = "0.1.0"}
sub3_2 = {git = "file://{}/sub3", project = "sub3", version = "0.2.0"}
sub3_3 = {git = "file://{}/sub3", project = "sub3", version = "1.0.0"}
sub4   = {path = "../sub4"}
sub6   = {path = "../sub6"}
"#;

const SUB1_TOML: &str = r#"
[project]
name = "sub1"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true

[dependencies]
sub2 = {git = "file://{}/sub2", version = "1.0.0"}
"#;

const SUB2_TOML: &str = r#"
[project]
name = "sub2"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true
"#;

const SUB3_TOML: &str = r#"
[project]
name = "sub3"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true

[dependencies]
sub1 = {git = "file://{}/sub1", version = "0.1.0"}
"#;

const SUB4_TOML: &str = r#"
[project]
name = "sub4"
version = "0.4.0"

[publish]
bump_commit = true
publish_commit = true

[dependencies]
sub5 = {path = "./sub5"}
sub6 = {path = "../sub6"}
"#;

const SUB5_TOML: &str = r#"
[project]
name = "sub5"
version = "0.5.0"

[publish]
bump_commit = true
publish_commit = true
"#;

const SUB6_TOML: &str = r#"
[project]
name = "sub6"
version = "0.6.0"

[publish]
bump_commit = true
publish_commit = true
"#;

fn create_metadata_simple() -> (Metadata, TempDir) {
    let tempdir = tempfile::tempdir().unwrap();
    let metadata = create_project(tempdir.path(), "test", TEST_TOML, false);

    (metadata, tempdir)
}

fn create_metadata_multi() -> (Metadata, TempDir) {
    let tempdir = tempfile::tempdir().unwrap();
    let metadata = create_project(tempdir.path(), "main", MAIN_TOML, false);
    create_project(tempdir.path(), "sub1", SUB1_TOML, true);
    create_project(tempdir.path(), "sub2", SUB2_TOML, true);
    create_project(tempdir.path(), "sub3", SUB3_TOML, true);
    create_project(tempdir.path(), "sub4", SUB4_TOML, true);
    create_project(&tempdir.path().join("sub4"), "sub5", SUB5_TOML, true);
    create_project(tempdir.path(), "sub6", SUB6_TOML, true);

    (metadata, tempdir)
}

const INNER_A_TOML: &str = r#"
[project]
name = "inner_a"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true
"#;

const INNER_B_TOML: &str = r#"
[project]
name = "inner_b"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true
"#;

fn create_metadata_inner_repo() -> (Metadata, TempDir) {
    unsafe {
        std::env::set_var("GIT_AUTHOR_NAME", "veryl");
        std::env::set_var("GIT_AUTHOR_EMAIL", "veryl");
        std::env::set_var("GIT_COMMITTER_NAME", "veryl");
        std::env::set_var("GIT_COMMITTER_EMAIL", "veryl");
    }

    let tempdir = tempfile::tempdir().unwrap();

    let repo_path = tempdir.path().join("repo");
    fs::create_dir(&repo_path).unwrap();
    let gitignore_path = repo_path.join(".gitignore");
    fs::write(&gitignore_path, GIT_IGNORE).unwrap();

    let a_path = repo_path.join("a_prj");
    fs::create_dir(&a_path).unwrap();
    let a_toml_path = a_path.join("Veryl.toml");
    fs::write(&a_toml_path, INNER_A_TOML).unwrap();

    let b_path = repo_path.join("b_prj");
    fs::create_dir(&b_path).unwrap();
    let b_toml_path = b_path.join("Veryl.toml");
    fs::write(&b_toml_path, INNER_B_TOML).unwrap();

    let git = Git::init(&repo_path).unwrap();
    git.add(&gitignore_path).unwrap();
    git.add(&a_toml_path).unwrap();
    git.add(&b_toml_path).unwrap();
    git.commit("Add inner projects").unwrap();

    let mut a_metadata = Metadata::load(&a_toml_path).unwrap();
    a_metadata.publish().unwrap();
    let mut b_metadata = Metadata::load(&b_toml_path).unwrap();
    b_metadata.publish().unwrap();

    let main_toml = format!(
        r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
inner_a = {{git = "file://{repo}", project = "inner_a", version = "0.1.0"}}
inner_b = {{git = "file://{repo}", project = "inner_b", version = "0.1.0"}}
"#,
        repo = repo_path.to_string_lossy().replace("\\", "/"),
    );
    let metadata = create_project(tempdir.path(), "main", &main_toml, false);

    (metadata, tempdir)
}

fn create_project(root: &Path, name: &str, toml: &str, publish: bool) -> Metadata {
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
    let git_ignore_path = path.join(".gitignore");
    fs::write(&git_ignore_path, GIT_IGNORE).unwrap();
    let git = Git::init(&path).unwrap();
    git.add(&toml_path).unwrap();
    git.add(&git_ignore_path).unwrap();
    git.commit("Add Veryl.toml").unwrap();
    let mut metadata = Metadata::load(&toml_path).unwrap();
    if publish {
        metadata.publish().unwrap();
        metadata.bump_version(BumpKind::Patch).unwrap();
        metadata.publish().unwrap();
        metadata.bump_version(BumpKind::Minor).unwrap();
        metadata.publish().unwrap();
        metadata.bump_version(BumpKind::Major).unwrap();
        metadata.publish().unwrap();
    }
    metadata
}

#[test]
fn check_toml() {
    let metadata: Metadata = toml::from_str(TEST_TOML).unwrap();
    assert_eq!(metadata.project.name, "test");
    assert_eq!(
        metadata.project.version,
        Some(Version::parse("0.1.0").unwrap())
    );
    assert_eq!(metadata.build.clock_type, ClockType::PosEdge);
    assert_eq!(metadata.build.reset_type, ResetType::AsyncLow);
    assert!(metadata.build.clock_posedge_prefix.is_none());
    assert!(metadata.build.clock_posedge_suffix.is_none());
    assert!(metadata.build.clock_negedge_prefix.is_none());
    assert!(metadata.build.clock_negedge_suffix.is_none());
    assert!(metadata.build.reset_high_prefix.is_none());
    assert!(metadata.build.reset_high_suffix.is_none());
    assert!(metadata.build.reset_low_prefix.is_none());
    assert_eq!(metadata.build.reset_low_suffix.unwrap(), "_n");
    assert_eq!(metadata.format.indent_width, 4);
}

#[test]
fn load_extension_namespace_metadata() {
    let tempdir = tempfile::tempdir().unwrap();
    let toml_path = tempdir.path().join("Veryl.toml");
    fs::write(&toml_path, EXTENSION_EXTERNAL_TOOL_TOML).unwrap();

    let metadata = Metadata::load(&toml_path).unwrap();
    let external_tool = metadata
        .metadata
        .get("external_tool")
        .unwrap()
        .as_table()
        .unwrap();

    assert_eq!(
        external_tool.get("files").unwrap().as_array().unwrap()[0]
            .as_str()
            .unwrap(),
        "src/**/*.v"
    );
    assert_eq!(
        external_tool
            .get("attrs")
            .unwrap()
            .as_table()
            .unwrap()
            .get("role")
            .unwrap()
            .as_str()
            .unwrap(),
        "core"
    );
}

#[test]
fn load_extension_namespace_other_tool() {
    let tempdir = tempfile::tempdir().unwrap();
    let toml_path = tempdir.path().join("Veryl.toml");
    fs::write(&toml_path, EXTENSION_OTHER_TOOL_TOML).unwrap();

    let metadata = Metadata::load(&toml_path).unwrap();

    assert!(metadata.metadata.contains_key("other_tool"));
    assert!(
        metadata
            .metadata
            .get("other_tool")
            .unwrap()
            .as_table()
            .unwrap()
            .get("enabled")
            .unwrap()
            .as_bool()
            .unwrap()
    );
}

#[test]
fn reject_unknown_top_level_table() {
    let tempdir = tempfile::tempdir().unwrap();
    let toml_path = tempdir.path().join("Veryl.toml");
    fs::write(&toml_path, UNKNOWN_TOP_LEVEL_TOML).unwrap();

    assert!(Metadata::load(&toml_path).is_err());
}

#[test]
fn metadata_output_v2_simple_project() {
    let tempdir = tempfile::tempdir().unwrap();
    let metadata = create_project(tempdir.path(), "test", EXTENSION_EXTERNAL_TOOL_TOML, false);
    let project_path = tempdir.path().join("test").canonicalize().unwrap();

    let output = MetadataOutputV2::from_metadata(&metadata).unwrap();
    let encoded = serde_json::to_string(&output).unwrap();
    let value = serde_json::to_value(&output).unwrap();

    assert_eq!(output.format_version, 2);
    assert_eq!(value["format_version"], 2);
    assert_eq!(output.root.name, "test");
    assert_eq!(output.root.version, Some(Version::parse("0.1.0").unwrap()));
    assert_eq!(output.root.local_path, project_path);
    assert_eq!(
        output
            .root
            .metadata
            .get("external_tool")
            .unwrap()
            .as_table()
            .unwrap()
            .get("attrs")
            .unwrap()
            .as_table()
            .unwrap()
            .get("role")
            .unwrap()
            .as_str()
            .unwrap(),
        "core"
    );
    assert!(output.dependencies.is_empty());
    assert_eq!(value["root"]["name"], "test");
    assert_eq!(
        value["root"]["local_path"].as_str().unwrap(),
        project_path.to_string_lossy().as_ref()
    );
    assert_eq!(
        value["root"]["metadata"]["external_tool"]["attrs"]["role"],
        "core"
    );
    assert!(value.get("metadata").is_none());
    assert!(!encoded.contains("metadata_path"));
    assert!(!encoded.contains("lockfile_path"));
    assert!(!encoded.contains("pubfile_path"));
    assert!(!encoded.contains("build_info"));
}

#[test]
fn metadata_output_v2_multi_dependency_deterministic() {
    let (mut metadata, _tempdir) = create_metadata_multi();
    metadata.update_lockfile().unwrap();

    let output = MetadataOutputV2::from_metadata(&metadata).unwrap();
    let repeated = MetadataOutputV2::from_metadata(&metadata).unwrap();
    let encoded = serde_json::to_string(&output).unwrap();
    let repeated_encoded = serde_json::to_string(&repeated).unwrap();
    let value = serde_json::to_value(&output).unwrap();
    let ids = output
        .dependencies
        .iter()
        .map(|dependency| dependency.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(encoded, repeated_encoded);
    assert_eq!(value["format_version"], 2);
    assert!(!output.dependencies.is_empty());
    assert!(ids.windows(2).all(|window| window[0] <= window[1]));
    assert!(ids.contains(&"dep:sub1"));
    assert!(ids.contains(&"dep:sub2"));
    assert!(ids.contains(&"dep:sub2_0"));
    assert!(output.dependencies.iter().any(|dependency| {
        dependency.id == "dep:sub1" && dependency.dependencies.contains(&"dep:sub2_0".to_string())
    }));
    assert!(output.dependencies.iter().any(|dependency| {
        matches!(dependency.source, MetadataSourceV2::Repository { .. })
            && !dependency.local_path.as_os_str().is_empty()
    }));
    assert!(output.dependencies.iter().any(|dependency| {
        matches!(dependency.source, MetadataSourceV2::Path { .. })
            && !dependency.local_path.as_os_str().is_empty()
    }));
    assert!(
        output
            .dependencies
            .iter()
            .all(|dependency| dependency.local_path.is_absolute())
    );
    assert!(
        output
            .dependencies
            .iter()
            .all(|dependency| dependency.metadata.is_empty())
    );
    assert!(
        output
            .dependencies
            .iter()
            .all(|dependency| !dependency.local_path.as_os_str().is_empty())
    );
    assert!(
        value["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .all(|dependency| {
                dependency
                    .get("local_path")
                    .and_then(|local_path| local_path.as_str())
                    .is_some_and(|local_path| !local_path.is_empty())
            })
    );
    assert!(!encoded.contains("metadata_path"));
    assert!(!encoded.contains("lockfile_path"));
    assert!(!encoded.contains("pubfile_path"));
    assert!(!encoded.contains("build_info"));
}

#[test]
fn search_config() {
    let path = Metadata::search_from_current();
    assert!(path.is_ok());
}

#[test]
fn load() {
    let (metadata, _tempdir) = create_metadata_simple();
    assert!(metadata.metadata_path.exists());
}

#[test]
fn check() {
    let (mut metadata, _tempdir) = create_metadata_simple();
    assert!(metadata.check().is_ok());

    metadata.project.name = "0".to_string();
    assert!(metadata.check().is_err());

    metadata.project.name = "---".to_string();
    assert!(metadata.check().is_err());
}

#[test]
fn publish() {
    let (mut metadata, tempdir) = create_metadata_simple();
    metadata.publish().unwrap();

    let project_version = metadata.project.version.clone().unwrap();

    assert_eq!(metadata.pubfile.releases[0].version, project_version);
    assert!(metadata.pubfile_path.exists());
    let git = Git::open(&tempdir.path().join("test")).unwrap();
    assert!(!git.is_clean().unwrap());
}

#[test]
fn publish_with_commit() {
    let (mut metadata, tempdir) = create_metadata_simple();
    metadata.publish.publish_commit = true;
    metadata.publish.publish_commit_message = "chore: Publish".to_string();
    metadata.publish().unwrap();

    let project_version = metadata.project.version.clone().unwrap();

    assert_eq!(metadata.pubfile.releases[0].version, project_version);
    assert!(metadata.pubfile_path.exists());
    let git = Git::open(&tempdir.path().join("test")).unwrap();
    assert!(git.is_clean().unwrap());
}

#[test]
fn bump_version() {
    let (mut metadata, tempdir) = create_metadata_simple();

    metadata.bump_version(BumpKind::Major).unwrap();
    assert_eq!(
        metadata.project.version,
        Some(Version::parse("1.0.0").unwrap())
    );

    metadata.bump_version(BumpKind::Minor).unwrap();
    assert_eq!(
        metadata.project.version,
        Some(Version::parse("1.1.0").unwrap())
    );

    metadata.bump_version(BumpKind::Patch).unwrap();
    assert_eq!(
        metadata.project.version,
        Some(Version::parse("1.1.1").unwrap())
    );

    let git = Git::open(&tempdir.path().join("test")).unwrap();
    assert!(!git.is_clean().unwrap());
}

#[test]
fn bump_version_with_commit() {
    let (mut metadata, tempdir) = create_metadata_simple();
    metadata.publish.bump_commit = true;
    metadata.publish.bump_commit_message = "chore: Bump version".to_string();

    metadata.bump_version(BumpKind::Major).unwrap();
    assert_eq!(
        metadata.project.version,
        Some(Version::parse("1.0.0").unwrap())
    );

    metadata.bump_version(BumpKind::Minor).unwrap();
    assert_eq!(
        metadata.project.version,
        Some(Version::parse("1.1.0").unwrap())
    );

    metadata.bump_version(BumpKind::Patch).unwrap();
    assert_eq!(
        metadata.project.version,
        Some(Version::parse("1.1.1").unwrap())
    );

    let git = Git::open(&tempdir.path().join("test")).unwrap();
    assert!(git.is_clean().unwrap());
}

#[test]
fn lockfile() {
    let (metadata, _tempdir) = create_metadata_multi();
    let lockfile = Lockfile::new(&metadata).unwrap();
    let tbl = &lockfile.lock_table;
    let sub1 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub1"));
    let sub1_0 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub1_0"));
    let sub2 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub2"));
    let sub2_0 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub2_0"));
    let sub3_2 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub3_2"));
    let sub3_3 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub3_3"));
    let sub4 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub4"));
    let sub5 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub5"));
    let sub6 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub6"));
    let sub6_0 = tbl
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub6_0"));
    assert!(sub1.is_some());
    assert!(sub1_0.is_none());
    assert!(sub2.is_some());
    assert!(sub2_0.is_some());
    assert!(sub3_2.is_some());
    assert!(sub3_3.is_some());
    assert!(sub4.is_some());
    assert!(sub5.is_some());
    assert!(sub6.is_some());
    assert!(sub6_0.is_none());
    assert_eq!(
        sub1.unwrap().source.get_version(),
        Some(&Version::parse("0.1.1").unwrap())
    );
    assert_eq!(
        sub2.unwrap().source.get_version(),
        Some(&Version::parse("0.1.1").unwrap())
    );
    assert_eq!(
        sub2_0.unwrap().source.get_version(),
        Some(&Version::parse("1.0.0").unwrap())
    );
    assert_eq!(
        sub3_2.unwrap().source.get_version(),
        Some(&Version::parse("0.2.0").unwrap())
    );
    assert_eq!(
        sub3_3.unwrap().source.get_version(),
        Some(&Version::parse("1.0.0").unwrap())
    );

    let _ = lockfile.clear_cache();
}

#[test]
fn lockfile_inner_projects() {
    let (metadata, _tempdir) = create_metadata_inner_repo();
    let lockfile = Lockfile::new(&metadata).unwrap();

    let inner_a = lockfile
        .lock_table
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "inner_a"))
        .expect("inner_a should be present in the lockfile");
    let inner_b = lockfile
        .lock_table
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "inner_b"))
        .expect("inner_b should be present in the lockfile");

    assert_eq!(
        inner_a.source.get_version(),
        Some(&Version::parse("0.1.0").unwrap())
    );
    assert_eq!(
        inner_b.source.get_version(),
        Some(&Version::parse("0.1.0").unwrap())
    );

    let _paths = lockfile
        .paths(Path::new("target"))
        .expect("paths() should resolve inner-project metadata");

    let _ = lockfile.clear_cache();
}

// Concurrent test functions each rewrite the same Veryl.lock via Lockfile::save
// while others read it; a non-atomic (truncate-then-write) save let a reader
// observe an empty file and fail with "missing field `projects`".
#[test]
fn lockfile_save_is_atomic() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path().join("Veryl.lock");
    Lockfile::default().save(&path).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..1)
        .map(|_| {
            let path = path.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    // On Windows a read can transiently fail during the atomic
                    // replace; only a successful read must never be truncated.
                    if let Ok(text) = fs::read_to_string(&path) {
                        toml::from_str::<Lockfile>(&text)
                            .unwrap_or_else(|e| panic!("read a partial lock file ({e}): {text:?}"));
                    }
                }
            })
        })
        .collect();

    for _ in 0..2000 {
        Lockfile::default().save(&path).unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.join().unwrap();
    }
}
