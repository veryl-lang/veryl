use crate::git::Git;
use crate::*;
use semver::Version;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GIT_IGNORE: &'static str = r#"
Veryl.lock
"#;

const TEST_TOML: &'static str = r#"
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

const MAIN_TOML: &'static str = r#"
[project]
name = "main"
version = "0.1.0"

[dependencies]
"file://{}/sub1" = "0.1.0"
"file://{}/sub2" = "0.1.0"
"file://{}/sub3" = [
    {version = "0.2.0", name = "sub3_2"},
    {version = "1.0.0", name = "sub3_3"},
]
"#;

const SUB1_TOML: &'static str = r#"
[project]
name = "sub1"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true

[dependencies]
"file://{}/sub3" = "0.1.0"
"#;

const SUB2_TOML: &'static str = r#"
[project]
name = "sub2"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true
"#;

const SUB3_TOML: &'static str = r#"
[project]
name = "sub2"
version = "0.1.0"

[publish]
bump_commit = true
publish_commit = true

[dependencies]
"file://{}/sub1" = "0.1.0"
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

    (metadata, tempdir)
}

fn create_project(root: &Path, name: &str, toml: &str, publish: bool) -> Metadata {
    std::env::set_var("GIT_AUTHOR_NAME", "veryl");
    std::env::set_var("GIT_AUTHOR_EMAIL", "veryl");
    std::env::set_var("GIT_COMMITTER_NAME", "veryl");
    std::env::set_var("GIT_COMMITTER_EMAIL", "veryl");

    let path = root.join(name);
    fs::create_dir(&path).unwrap();
    let toml_path = path.join("Veryl.toml");
    fs::write(
        &toml_path,
        &toml.replace("{}", &root.to_string_lossy().replace("\\", "/")),
    )
    .unwrap();
    let git_ignore_path = path.join(".gitignore");
    fs::write(&git_ignore_path, GIT_IGNORE).unwrap();
    let git = Git::init(&path).unwrap();
    git.add(&toml_path).unwrap();
    git.add(&git_ignore_path).unwrap();
    git.commit(&"Add Veryl.toml").unwrap();
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
    assert_eq!(metadata.project.version, Version::parse("0.1.0").unwrap());
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

    assert_eq!(
        metadata.pubfile.releases[0].version,
        metadata.project.version
    );
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

    assert_eq!(
        metadata.pubfile.releases[0].version,
        metadata.project.version
    );
    assert!(metadata.pubfile_path.exists());
    let git = Git::open(&tempdir.path().join("test")).unwrap();
    assert!(git.is_clean().unwrap());
}

#[test]
fn bump_version() {
    let (mut metadata, tempdir) = create_metadata_simple();

    metadata.bump_version(BumpKind::Major).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.0.0").unwrap());

    metadata.bump_version(BumpKind::Minor).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.1.0").unwrap());

    metadata.bump_version(BumpKind::Patch).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.1.1").unwrap());

    let git = Git::open(&tempdir.path().join("test")).unwrap();
    assert!(!git.is_clean().unwrap());
}

#[test]
fn bump_version_with_commit() {
    let (mut metadata, tempdir) = create_metadata_simple();
    metadata.publish.bump_commit = true;
    metadata.publish.bump_commit_message = "chore: Bump version".to_string();

    metadata.bump_version(BumpKind::Major).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.0.0").unwrap());

    metadata.bump_version(BumpKind::Minor).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.1.0").unwrap());

    metadata.bump_version(BumpKind::Patch).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.1.1").unwrap());

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
    assert!(sub1.is_some());
    assert!(sub2.is_some());
    assert!(sub2_0.is_some());
    assert!(sub3_2.is_some());
    assert!(sub3_3.is_some());
    assert_eq!(sub1.unwrap().version, Version::parse("0.1.1").unwrap());
    assert_eq!(sub2.unwrap().version, Version::parse("0.1.1").unwrap());
    assert_eq!(sub2_0.unwrap().version, Version::parse("0.1.1").unwrap());
    assert_eq!(sub3_2.unwrap().version, Version::parse("0.2.0").unwrap());
    assert_eq!(sub3_3.unwrap().version, Version::parse("1.0.0").unwrap());

    let _ = lockfile.clear_cache();
}
