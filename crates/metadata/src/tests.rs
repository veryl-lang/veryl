use crate::git::Git;
use crate::*;
use semver::Version;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
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

[[components]]
path = "models/rv_iss"

[[components]]
path = "models/golden"
wasm = "prebuilt/golden.wasm"

[test]
seed = 42
component_backend = "wasm"
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
        repo = path_to_toml_str(&repo_path),
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
    fs::write(&toml_path, toml.replace("{}", &path_to_toml_str(root))).unwrap();
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

// Convert a path into a form that can be embedded in a TOML basic string.
// On Windows the backslash separators would otherwise be interpreted as
// escape sequences, so replace them with forward slashes.
fn path_to_toml_str(path: &Path) -> String {
    path.to_string_lossy().replace("\\", "/")
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
    assert_eq!(metadata.test.seed, Some(42));
    assert_eq!(metadata.components.len(), 2);
    let rv_iss = &metadata.components[0];
    assert_eq!(rv_iss.path, PathBuf::from("models/rv_iss"));
    assert!(rv_iss.wasm.is_none());
    let golden = &metadata.components[1];
    assert_eq!(golden.path, PathBuf::from("models/golden"));
    assert_eq!(golden.wasm, Some(PathBuf::from("prebuilt/golden.wasm")));
}

#[test]
fn four_state_defaults_off_and_parses() {
    let metadata: Metadata = toml::from_str(TEST_TOML).unwrap();
    assert!(!metadata.test.four_state);

    let toml = r#"
[project]
name = "test"
version = "0.1.0"

[test]
four_state = true
"#;
    let metadata: Metadata = toml::from_str(toml).unwrap();
    assert!(metadata.test.four_state);
}

#[test]
fn lint_portability_defaults_empty_and_parses() {
    let metadata: Metadata = toml::from_str(TEST_TOML).unwrap();
    assert!(metadata.lint.portability.allow_in_dependencies.is_empty());

    let toml = r#"
[project]
name = "test"
version = "0.1.0"

[lint.portability]
allow_in_dependencies = ["initial_assign"]
"#;
    let metadata: Metadata = toml::from_str(toml).unwrap();
    assert_eq!(
        metadata.lint.portability.allow_in_dependencies,
        vec![NonPortableItem::InitialAssign]
    );

    // A typo is a manifest error rather than a silently ignored entry.
    let toml = r#"
[project]
name = "test"
version = "0.1.0"

[lint.portability]
allow_in_dependencies = ["initial_assignment"]
"#;
    assert!(toml::from_str::<Metadata>(toml).is_err());
}

#[test]
fn synth_ram_thresholds_default_and_override() {
    // Omitted RAM thresholds fall back to the built-in defaults.
    let metadata: Metadata = toml::from_str(TEST_TOML).unwrap();
    assert_eq!(metadata.synth.ram_min_bits, 1024);
    assert_eq!(metadata.synth.ram_max_read_ports, 16);
    assert_eq!(metadata.synth.ram_max_write_ports, 8);
    assert_eq!(metadata.synth.ram_max_ff_bits, 1 << 16);

    // Explicit values in `[synth]` override the defaults.
    let toml = r#"
[project]
name = "test"
version = "0.1.0"

[synth]
ram_min_bits = 256
ram_max_read_ports = 4
ram_max_write_ports = 2
ram_max_ff_bits = 4096
"#;
    let metadata: Metadata = toml::from_str(toml).unwrap();
    assert_eq!(metadata.synth.ram_min_bits, 256);
    assert_eq!(metadata.synth.ram_max_read_ports, 4);
    assert_eq!(metadata.synth.ram_max_write_ports, 2);
    assert_eq!(metadata.synth.ram_max_ff_bits, 4096);
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

const VIP_TOML: &str = r#"
[project]
name = "vip"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true

[[components]]
path = "models/mirror"

[dependencies]
trans = {git = "file://{}/trans", version = "0.1.0"}
"#;

const TRANS_TOML: &str = r#"
[project]
name = "trans"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true

[[components]]
path = "models/hidden"
"#;

#[test]
fn collect_dependency_components() {
    let main_toml = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
the_vip = {git = "file://{}/vip", project = "vip", version = "0.1.0"}
"#;

    let tempdir = tempfile::tempdir().unwrap();
    create_project(tempdir.path(), "trans", TRANS_TOML, true);
    create_project(tempdir.path(), "vip", VIP_TOML, true);
    let mut metadata = create_project(tempdir.path(), "main", main_toml, false);

    metadata.lockfile = Lockfile::new(&metadata).unwrap();
    let deps = metadata.collect_dependency_components().unwrap();

    // Only the direct dependency's components are collected, under its
    // alias name; the transitive `trans` component stays hidden.
    assert_eq!(deps.len(), 1);
    let dep = &deps[0];
    assert_eq!(dep.project, "the_vip");
    assert_eq!(dep.components.len(), 1);
    assert_eq!(dep.components[0].path, PathBuf::from("models/mirror"));
    assert!(dep.root.join("Veryl.toml").exists());
    // Repository dependencies build into the revision-keyed shared cache.
    assert!(
        dep.target_dir
            .starts_with(veryl_path::cache_path().join("components"))
    );

    let _ = metadata.lockfile.clear_cache();
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

const EXAMPLES_TOML: &str = r#"
[project]
name = "test"
version = "0.1.0"

[build]
sources = ["src"]
target = {type = "directory", path = "target"}
"#;

fn create_metadata_with_examples() -> (Metadata, TempDir) {
    let tempdir = tempfile::tempdir().unwrap();
    let metadata = create_project(tempdir.path(), "test", EXAMPLES_TOML, false);
    let project_path = metadata.project_path();
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(project_path.join("src/a.veryl"), "module A {}\n").unwrap();
    fs::create_dir_all(project_path.join("examples")).unwrap();
    fs::write(project_path.join("examples/ex.veryl"), "module Ex {}\n").unwrap();

    (metadata, tempdir)
}

#[test]
fn paths_include_root_examples() {
    let (mut metadata, _tempdir) = create_metadata_with_examples();

    let paths = metadata.paths::<&str>(&[], false, false).unwrap();
    assert_eq!(paths.len(), 2);

    let src = paths.iter().find(|x| x.src.ends_with("a.veryl")).unwrap();
    let example = paths.iter().find(|x| x.src.ends_with("ex.veryl")).unwrap();
    assert!(!src.example);
    assert!(example.example);
}

#[test]
fn paths_include_examples_with_default_sources() {
    // The default `sources = [""]` walks the whole project.
    let toml = r#"
[project]
name = "test"
version = "0.1.0"

[build]
target = {type = "directory", path = "target"}
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "test", toml, false);
    let project_path = metadata.project_path();
    fs::create_dir_all(project_path.join("src")).unwrap();
    fs::write(project_path.join("src/a.veryl"), "module A {}\n").unwrap();
    fs::create_dir_all(project_path.join("examples")).unwrap();
    fs::write(project_path.join("examples/ex.veryl"), "module Ex {}\n").unwrap();

    let paths = metadata.paths::<&str>(&[], false, false).unwrap();
    assert_eq!(paths.len(), 2);

    let src = paths.iter().find(|x| x.src.ends_with("a.veryl")).unwrap();
    let example = paths.iter().find(|x| x.src.ends_with("ex.veryl")).unwrap();
    assert!(!src.example);
    assert!(example.example);
}

#[test]
fn sources_under_examples_are_rejected() {
    for source in ["examples", "examples/sub"] {
        let toml = format!(
            r#"
[project]
name = "test"
version = "0.1.0"

[build]
sources = ["src", "{source}"]
target = {{type = "directory", path = "target"}}
"#
        );
        let tempdir = tempfile::tempdir().unwrap();
        let mut metadata = create_project(tempdir.path(), "test", &toml, false);
        let project_path = metadata.project_path();
        fs::create_dir_all(project_path.join("src")).unwrap();
        fs::write(project_path.join("src/a.veryl"), "module A {}\n").unwrap();
        fs::create_dir_all(project_path.join("examples/sub")).unwrap();
        fs::write(project_path.join("examples/ex.veryl"), "module Ex {}\n").unwrap();

        let result = metadata.paths::<&str>(&[], false, false);
        assert!(matches!(result, Err(MetadataError::ReservedSourceDir(_))));
    }
}

#[test]
fn paths_route_explicit_example_file() {
    let (mut metadata, _tempdir) = create_metadata_with_examples();
    let file = metadata.project_path().join("examples/ex.veryl");

    let paths = metadata.paths(&[file], false, false).unwrap();
    assert_eq!(paths.len(), 1);
    assert!(paths[0].example);
}

#[test]
fn lockfile_paths_exclude_dependency_examples() {
    let tempdir = tempfile::tempdir().unwrap();

    let dep_path = tempdir.path().join("dep");
    fs::create_dir_all(dep_path.join("src")).unwrap();
    fs::create_dir_all(dep_path.join("examples")).unwrap();
    fs::write(
        dep_path.join("Veryl.toml"),
        r#"
[project]
name = "dep"
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(dep_path.join("src/a.veryl"), "module A {}\n").unwrap();
    fs::write(dep_path.join("examples/ex.veryl"), "module Ex {}\n").unwrap();

    let main_toml = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
dep = {path = "../dep"}
"#;
    let metadata = create_project(tempdir.path(), "main", main_toml, false);

    let lockfile = Lockfile::new(&metadata).unwrap();
    let paths = lockfile.paths(Path::new("target")).unwrap();
    assert!(paths.iter().any(|x| x.src.ends_with("a.veryl")));
    assert!(!paths.iter().any(|x| x.src.ends_with("ex.veryl")));
}

#[test]
fn lockfile_paths_order_is_deterministic() {
    let tempdir = tempfile::tempdir().unwrap();

    let deps = ["dep_a", "dep_b", "dep_c", "dep_d"];
    for dep in deps {
        let dep_path = tempdir.path().join(dep);
        fs::create_dir_all(dep_path.join("src")).unwrap();
        fs::write(
            dep_path.join("Veryl.toml"),
            format!(
                r#"
[project]
name = "{dep}"
version = "0.1.0"
"#
            ),
        )
        .unwrap();
        fs::write(dep_path.join("src/a.veryl"), "module A {}\n").unwrap();
    }

    let main_toml = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
dep_a = {path = "../dep_a"}
dep_b = {path = "../dep_b"}
dep_c = {path = "../dep_c"}
dep_d = {path = "../dep_d"}
"#;
    let metadata = create_project(tempdir.path(), "main", main_toml, false);

    // Each Lockfile has a freshly created lock_table, so iteration order
    // depending on the hasher seed would show up across iterations.
    for _ in 0..10 {
        let lockfile = Lockfile::new(&metadata).unwrap();
        let paths = lockfile.paths(Path::new("target")).unwrap();
        let prjs: Vec<_> = paths.iter().map(|x| x.prj.as_str()).collect();
        assert_eq!(prjs, deps);
    }
}

#[test]
fn define_global_properties() {
    let toml = r#"
[project]
name = "main"
[properties]
foo = 32
bar = true
"#;

    let tempdir = tempfile::tempdir().unwrap();
    let metadata = create_project(tempdir.path(), "main", toml, false);

    assert_eq!(metadata.properties["foo"], ProjectProperty::Int(32));
    assert_eq!(metadata.properties["bar"], ProjectProperty::Bool(true));
}

#[test]
fn override_global_properties_of_dependencies() {
    let tempdir = tempfile::tempdir().unwrap();

    let sub_toml = r#"
[project]
name = "sub"
version = "0.1.0"
[properties]
foo = 32
bar = false
"#;

    create_project(tempdir.path(), "sub", sub_toml, false);

    let sub_path = tempdir.path().join("sub");
    let main_toml = format!(
        r#"
[project]
name = "main"
[dependencies]
sub = {{
    path = "{}",
    properties = {{ foo = 16 }}
}}
"#,
        path_to_toml_str(&sub_path)
    );

    let metadata = create_project(tempdir.path(), "main", &main_toml, false);
    let lockfile = Lockfile::new(&metadata).unwrap();

    let sub_metadata = lockfile
        .lock_table
        .iter()
        .find_map(|(_, x)| x.iter().find(|x| x.name == "sub"))
        .unwrap();
    assert_eq!(sub_metadata.properties["foo"], ProjectProperty::Int(16));
    assert_eq!(sub_metadata.properties["bar"], ProjectProperty::Bool(false));
}

#[test]
fn override_unknown_global_property() {
    let tempdir = tempfile::tempdir().unwrap();

    let sub_toml = r#"
[project]
name = "sub"
version = "0.1.0"
[properties]
foo = 32
bar = false
"#;

    create_project(tempdir.path(), "sub", sub_toml, false);

    let sub_path = tempdir.path().join("sub");
    let main_toml = format!(
        r#"
[project]
name = "main"
[dependencies]
sub = {{
    path = "{}",
    properties = {{ baz = 1 }}
}}
"#,
        path_to_toml_str(&sub_path)
    );

    let metadata = create_project(tempdir.path(), "main", &main_toml, false);
    let err = Lockfile::new(&metadata).unwrap_err();
    assert!(matches!(err, MetadataError::UnknownProperty { .. }));
}

#[test]
fn type_mismatch_on_global_property_override() {
    let tempdir = tempfile::tempdir().unwrap();

    let sub_toml = r#"
[project]
name = "sub"
version = "0.1.0"
[properties]
foo = 32
bar = false
"#;

    create_project(tempdir.path(), "sub", sub_toml, false);

    let sub_path = tempdir.path().join("sub");
    let main_toml = format!(
        r#"
[project]
name = "main"
[dependencies]
sub = {{
    path = "{}",
    properties = {{ foo = true }}
}}
"#,
        path_to_toml_str(&sub_path)
    );

    let metadata = create_project(tempdir.path(), "main", &main_toml, false);
    let err = Lockfile::new(&metadata).unwrap_err();
    assert!(matches!(err, MetadataError::MismatchType { .. }));
}

#[test]
fn unpublished_dependency_error() {
    let unpub_toml = r#"
[project]
name = "unpub"
version = "0.1.0"
"#;
    let main_toml = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
unpub = {git = "file://{}/unpub", version = "0.1.0"}
"#;

    let tempdir = tempfile::tempdir().unwrap();
    create_project(tempdir.path(), "unpub", unpub_toml, false);
    let metadata = create_project(tempdir.path(), "main", main_toml, false);

    let err = Lockfile::new(&metadata).unwrap_err();
    assert!(matches!(err, MetadataError::UnpublishedDependency { .. }));
    let msg = err.to_string();
    assert!(
        msg.contains("dependency `unpub` has no published release"),
        "unexpected error: {msg}"
    );
}

#[test]
fn version_only_dependency_error() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
bare = "1.0"
"#;

    let tempdir = tempfile::tempdir().unwrap();
    let toml_path = tempdir.path().join("Veryl.toml");
    fs::write(&toml_path, toml).unwrap();
    let metadata = Metadata::load(&toml_path).unwrap();

    let err = Lockfile::new(&metadata).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("bare") && msg.contains("version-only dependencies are not supported yet"),
        "unexpected error: {msg}"
    );
}

#[test]
fn project_name_validation() {
    assert!(check_project_name("valid_name").is_ok());
    assert!(check_project_name("_leading").is_ok());
    assert!(check_project_name("Name1").is_ok());
    assert!(check_project_name("kebab-case").is_err());
    assert!(check_project_name("1leading").is_err());
    assert!(check_project_name("").is_err());
    assert!(check_project_name("has space").is_err());
    assert!(check_project_name("__reserved").is_err());
}

fn write_sv_files(root: &Path, files: &[&str]) {
    for file in files {
        let path = root.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("// {file}\n")).unwrap();
    }
}

fn create_sv_dirs(root: &Path, dirs: &[&str]) {
    for dir in dirs {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

fn path_list(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

/// Creates a plain (non-git) project directory, used as a path dependency.
fn create_sv_path_project(
    root: &Path,
    name: &str,
    toml: &str,
    files: &[&str],
    dirs: &[&str],
) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("Veryl.toml"), toml).unwrap();
    write_sv_files(&path, files);
    create_sv_dirs(&path, dirs);
    path
}

/// Creates a published git project whose SV files are committed, so it can
/// be cloned into the cache as a git dependency.
fn create_sv_git_project(root: &Path, name: &str, toml: &str, files: &[&str]) {
    unsafe {
        std::env::set_var("GIT_AUTHOR_NAME", "veryl");
        std::env::set_var("GIT_AUTHOR_EMAIL", "veryl");
        std::env::set_var("GIT_COMMITTER_NAME", "veryl");
        std::env::set_var("GIT_COMMITTER_EMAIL", "veryl");
    }

    let path = root.join(name);
    fs::create_dir(&path).unwrap();
    let toml_path = path.join("Veryl.toml");
    fs::write(&toml_path, toml).unwrap();
    let git_ignore_path = path.join(".gitignore");
    fs::write(&git_ignore_path, GIT_IGNORE).unwrap();
    write_sv_files(&path, files);

    let git = Git::init(&path).unwrap();
    git.add(&toml_path).unwrap();
    git.add(&git_ignore_path).unwrap();
    for file in files {
        git.add(&path.join(file)).unwrap();
    }
    git.commit("Add project").unwrap();

    let mut metadata = Metadata::load(&toml_path).unwrap();
    metadata.publish().unwrap();
    git.add(&metadata.pubfile_path).unwrap();
    git.commit("Publish").unwrap();
}

#[test]
fn sv_filesets_default_empty() {
    let (metadata, _tempdir) = create_metadata_simple();
    assert!(metadata.sv.filesets.is_empty());
}

#[test]
fn sv_filesets_parse() {
    let toml = r#"
[project]
name = "test"
version = "0.1.0"

[properties]
for_asic = false
width = 32

[[sv.filesets]]
files = ["ip/a.sv"]

[[sv.filesets]]
root         = "ip/foo"
active       = { for_asic = false, width = 32 }
files        = ["rtl/foo_pkg.sv", "rtl/foo.sv"]
include_dirs = ["include"]
"#;
    let metadata = Metadata::from_str(toml).unwrap();
    let filesets = &metadata.sv.filesets;
    assert_eq!(filesets.len(), 2);

    assert_eq!(filesets[0].root, None);
    assert_eq!(filesets[0].files, vec![PathBuf::from("ip/a.sv")]);
    assert!(filesets[0].include_dirs.is_empty());
    assert!(filesets[0].active.is_empty());

    assert_eq!(filesets[1].root, Some(PathBuf::from("ip/foo")));
    assert_eq!(
        filesets[1].files,
        vec![PathBuf::from("rtl/foo_pkg.sv"), PathBuf::from("rtl/foo.sv")]
    );
    assert_eq!(filesets[1].include_dirs, vec![PathBuf::from("include")]);
    assert_eq!(filesets[1].active["for_asic"], ProjectProperty::Bool(false));
    assert_eq!(filesets[1].active["width"], ProjectProperty::Int(32));
}

#[test]
fn sv_sources_resolve_without_root() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[[sv.filesets]]
files        = ["ip/rtl/a.sv", "ip/rtl/b.sv"]
include_dirs = ["ip/include"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    let prj = metadata.project_path();
    write_sv_files(&prj, &["ip/rtl/a.sv", "ip/rtl/b.sv"]);
    create_sv_dirs(&prj, &["ip/include"]);

    let sources = metadata.sv_sources(false).unwrap();
    assert_eq!(sources.files, path_list(&["ip/rtl/a.sv", "ip/rtl/b.sv"]));
    assert_eq!(sources.include_dirs, path_list(&["ip/include"]));
}

#[test]
fn sv_sources_resolve_with_relative_root() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[[sv.filesets]]
root         = "ip/foo"
files        = ["rtl/foo.sv"]
include_dirs = ["include"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    let prj = metadata.project_path();
    write_sv_files(&prj, &["ip/foo/rtl/foo.sv"]);
    create_sv_dirs(&prj, &["ip/foo/include"]);

    let sources = metadata.sv_sources(false).unwrap();
    assert_eq!(sources.files, path_list(&["ip/foo/rtl/foo.sv"]));
    assert_eq!(sources.include_dirs, path_list(&["ip/foo/include"]));
}

#[test]
fn sv_sources_resolve_with_absolute_root() {
    let tempdir = tempfile::tempdir().unwrap();
    let vendor = tempdir.path().join("vendor");
    write_sv_files(&vendor, &["rtl/vendor.sv"]);
    create_sv_dirs(&vendor, &["include"]);

    let toml = format!(
        r#"
[project]
name = "main"
version = "0.1.0"

[[sv.filesets]]
root         = "{}"
files        = ["rtl/vendor.sv"]
include_dirs = ["include"]
"#,
        path_to_toml_str(&vendor)
    );
    let mut metadata = create_project(tempdir.path(), "main", &toml, false);

    // Paths under an absolute root are kept absolute
    let sources = metadata.sv_sources(false).unwrap();
    assert_eq!(
        sources.files,
        vec![canonical(&vendor.join("rtl/vendor.sv"))]
    );
    assert_eq!(
        sources.include_dirs,
        vec![canonical(&vendor.join("include"))]
    );
}

#[test]
fn sv_sources_absolute_path_ignores_root() {
    let tempdir = tempfile::tempdir().unwrap();
    let vendor = tempdir.path().join("vendor");
    write_sv_files(&vendor, &["rtl/vendor.sv"]);
    create_sv_dirs(&vendor, &["include"]);

    let toml = format!(
        r#"
[project]
name = "main"
version = "0.1.0"

[[sv.filesets]]
root         = "ip/foo"
files        = ["rtl/foo.sv", "{}"]
include_dirs = ["{}"]
"#,
        path_to_toml_str(&vendor.join("rtl/vendor.sv")),
        path_to_toml_str(&vendor.join("include"))
    );
    let mut metadata = create_project(tempdir.path(), "main", &toml, false);
    write_sv_files(&metadata.project_path(), &["ip/foo/rtl/foo.sv"]);

    let sources = metadata.sv_sources(false).unwrap();
    assert_eq!(
        sources.files,
        vec![
            PathBuf::from("ip/foo/rtl/foo.sv"),
            canonical(&vendor.join("rtl/vendor.sv"))
        ]
    );
    assert_eq!(
        sources.include_dirs,
        vec![canonical(&vendor.join("include"))]
    );
}

#[test]
fn sv_sources_active() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[properties]
for_asic = false
width = 32

[[sv.filesets]]
files = ["ip/always.sv"]

[[sv.filesets]]
active       = { for_asic = false }
files        = ["ip/fpga.sv"]
include_dirs = ["ip/fpga_include"]

[[sv.filesets]]
active       = { for_asic = true }
files        = ["ip/asic.sv"]
include_dirs = ["ip/asic_include"]

[[sv.filesets]]
active = { for_asic = false, width = 32 }
files  = ["ip/fpga_32.sv"]

[[sv.filesets]]
active = { for_asic = false, width = 64 }
files  = ["ip/fpga_64.sv"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    let prj = metadata.project_path();
    write_sv_files(
        &prj,
        &[
            "ip/always.sv",
            "ip/fpga.sv",
            "ip/asic.sv",
            "ip/fpga_32.sv",
            "ip/fpga_64.sv",
        ],
    );
    create_sv_dirs(&prj, &["ip/fpga_include", "ip/asic_include"]);

    let sources = metadata.sv_sources(false).unwrap();
    assert_eq!(
        sources.files,
        path_list(&["ip/always.sv", "ip/fpga.sv", "ip/fpga_32.sv"])
    );
    assert_eq!(sources.include_dirs, path_list(&["ip/fpga_include"]));
}

#[test]
fn sv_sources_active_unknown_property() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[properties]
for_asic = false

[[sv.filesets]]
active = { for_fpga = true }
files  = ["ip/a.sv"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    write_sv_files(&metadata.project_path(), &["ip/a.sv"]);

    let err = metadata.sv_sources(false).unwrap_err();
    assert!(matches!(err, MetadataError::UnknownProperty { .. }));
}

#[test]
fn sv_sources_active_type_mismatch() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[properties]
for_asic = false

[[sv.filesets]]
active = { for_asic = 1 }
files  = ["ip/a.sv"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    write_sv_files(&metadata.project_path(), &["ip/a.sv"]);

    let err = metadata.sv_sources(false).unwrap_err();
    assert!(matches!(err, MetadataError::MismatchType { .. }));
}

#[test]
fn sv_sources_missing_file() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[[sv.filesets]]
files = ["ip/missing.sv"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);

    let err = metadata.sv_sources(false).unwrap_err();
    assert!(matches!(err, MetadataError::SvPathNotFound(_)));
}

#[test]
fn sv_sources_missing_include_dir() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[[sv.filesets]]
files        = ["ip/a.sv"]
include_dirs = ["ip/missing"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    write_sv_files(&metadata.project_path(), &["ip/a.sv"]);

    let err = metadata.sv_sources(false).unwrap_err();
    assert!(matches!(err, MetadataError::SvPathNotFound(_)));
}

#[test]
fn sv_sources_inactive_missing_file_is_ignored() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[properties]
for_asic = false

[[sv.filesets]]
active = { for_asic = true }
files  = ["ip/missing.sv"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);

    let sources = metadata.sv_sources(false).unwrap();
    assert!(sources.files.is_empty());
}

#[test]
fn sv_sources_remove_duplicates() {
    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[[sv.filesets]]
files        = ["ip/common.sv", "ip/a.sv"]
include_dirs = ["ip/include"]

[[sv.filesets]]
root         = "ip"
files        = ["common.sv", "b.sv"]
include_dirs = ["include"]
"#;
    let tempdir = tempfile::tempdir().unwrap();
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    let prj = metadata.project_path();
    write_sv_files(&prj, &["ip/common.sv", "ip/a.sv", "ip/b.sv"]);
    create_sv_dirs(&prj, &["ip/include"]);

    let sources = metadata.sv_sources(false).unwrap();
    assert_eq!(
        sources.files,
        path_list(&["ip/common.sv", "ip/a.sv", "ip/b.sv"])
    );
    assert_eq!(sources.include_dirs, path_list(&["ip/include"]));
}

const SV_DEP_TOML: &str = r#"
[project]
name = "dep"
version = "0.1.0"

[properties]
for_asic = false

[[sv.filesets]]
root         = "ip"
files        = ["common.sv"]
include_dirs = ["include"]

[[sv.filesets]]
active = { for_asic = false }
files  = ["ip/fpga.sv"]

[[sv.filesets]]
active = { for_asic = true }
files  = ["ip/asic.sv"]
"#;

const SV_DEP_FILES: &[&str] = &[
    "ip/common.sv",
    "ip/fpga.sv",
    "ip/asic.sv",
    "ip/include/common.svh",
    "ip/include/sub/sub.svh",
];

#[test]
fn sv_sources_path_dependency() {
    let tempdir = tempfile::tempdir().unwrap();
    create_sv_path_project(tempdir.path(), "dep", SV_DEP_TOML, SV_DEP_FILES, &[]);

    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
dep = {path = "../dep", properties = {for_asic = true}}

[[sv.filesets]]
files = ["ip/main.sv"]
"#;
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    let prj = metadata.project_path();
    write_sv_files(&prj, &["ip/main.sv"]);

    // Without dependencies
    let sources = metadata.sv_sources(false).unwrap();
    assert_eq!(sources.files, path_list(&["ip/main.sv"]));

    // Dependency files are referred at their original location, evaluated
    // with the overridden property, and placed before the project's own.
    let sources = metadata.sv_sources(true).unwrap();
    assert_eq!(
        sources.files,
        path_list(&["../dep/ip/common.sv", "../dep/ip/asic.sv", "ip/main.sv"])
    );
    assert_eq!(sources.include_dirs, path_list(&["../dep/ip/include"]));
    assert!(!prj.join("dependencies/dep/ip/common.sv").exists());
}

#[test]
fn sv_sources_git_dependency_is_copied() {
    let tempdir = tempfile::tempdir().unwrap();
    create_sv_git_project(tempdir.path(), "dep", SV_DEP_TOML, SV_DEP_FILES);

    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
dep = {git = "file://{}/dep", version = "0.1.0"}
"#;
    let mut metadata = create_project(tempdir.path(), "main", toml, false);
    let base = metadata.output_dependencies_path().join("dep");

    let sources = metadata.sv_sources(true).unwrap();
    assert_eq!(
        sources.files,
        path_list(&[
            "dependencies/dep/ip/common.sv",
            "dependencies/dep/ip/fpga.sv"
        ])
    );
    assert_eq!(
        sources.include_dirs,
        path_list(&["dependencies/dep/ip/include"])
    );
    // Include directories are copied recursively
    assert!(base.join("ip/include/common.svh").is_file());
    assert!(base.join("ip/include/sub/sub.svh").is_file());
    // Inactive files are not copied
    assert!(!base.join("ip/asic.sv").exists());

    let _ = metadata.lockfile.clear_cache();
}

#[test]
fn sv_sources_same_dependency_with_different_properties() {
    let tempdir = tempfile::tempdir().unwrap();
    create_sv_path_project(tempdir.path(), "dep", SV_DEP_TOML, SV_DEP_FILES, &[]);
    create_sv_path_project(
        tempdir.path(),
        "mid",
        r#"
[project]
name = "mid"
version = "0.1.0"

[dependencies]
dep = {path = "../dep", properties = {for_asic = false}}
"#,
        &[],
        &[],
    );

    let toml = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
dep = {path = "../dep", properties = {for_asic = true}}
mid = {path = "../mid"}
"#;
    let mut metadata = create_project(tempdir.path(), "main", toml, false);

    // Both variants are merged without duplicating the common file
    let files = metadata.sv_sources(true).unwrap().files;
    let expected = path_list(&[
        "../dep/ip/common.sv",
        "../dep/ip/fpga.sv",
        "../dep/ip/asic.sv",
    ]);
    assert_eq!(files.len(), expected.len());
    for file in &expected {
        assert_eq!(files.iter().filter(|x| *x == file).count(), 1);
    }
}

fn publish_sv_project(fileset: &str) -> Result<(), MetadataError> {
    let tempdir = tempfile::tempdir().unwrap();
    let outside = tempdir.path().join("outside");
    write_sv_files(&outside, &["a.sv"]);

    let toml = format!(
        r#"
[project]
name = "main"
version = "0.1.0"

[properties]
for_asic = false

[[sv.filesets]]
{}
"#,
        fileset.replace("<outside>", &path_to_toml_str(&outside))
    );
    let mut metadata = create_project(tempdir.path(), "main", &toml, false);
    metadata.publish()
}

#[test]
fn publish_sv_filesets() {
    assert!(publish_sv_project(r#"files = ["ip/a.sv"]"#).is_ok());
    assert!(publish_sv_project("root = \"ip\"\nfiles = [\"a.sv\"]").is_ok());
}

#[test]
fn publish_rejects_absolute_sv_path() {
    let err = publish_sv_project(r#"files = ["<outside>/a.sv"]"#).unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));

    let err =
        publish_sv_project("files = [\"ip/a.sv\"]\ninclude_dirs = [\"<outside>\"]").unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));
}

#[test]
fn publish_rejects_sv_path_with_parent_dir() {
    // `..` is rejected even if the resolved path stays in the project
    let err = publish_sv_project("root = \"ip/foo\"\nfiles = [\"../foo/a.sv\"]").unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));

    let err = publish_sv_project("root = \"ip/../ip\"\nfiles = [\"a.sv\"]").unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));
}

#[test]
fn publish_rejects_sv_path_outside_project() {
    let err = publish_sv_project(r#"files = ["../outside/a.sv"]"#).unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));

    let err = publish_sv_project("root = \"ip\"\nfiles = [\"../../outside/a.sv\"]").unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));
}

#[test]
fn publish_rejects_invalid_sv_root() {
    let err = publish_sv_project("root = \"<outside>\"\nfiles = [\"a.sv\"]").unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));

    let err = publish_sv_project("root = \"../outside\"\nfiles = [\"a.sv\"]").unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));
}

#[test]
fn publish_checks_inactive_sv_filesets() {
    let err = publish_sv_project("active = { for_asic = true }\nfiles = [\"<outside>/a.sv\"]")
        .unwrap_err();
    assert!(matches!(err, MetadataError::InvalidSvPath(_)));
}
