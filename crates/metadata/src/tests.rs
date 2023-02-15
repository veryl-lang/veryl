use crate::git::Git;
use crate::*;
use semver::Version;
use std::fs;
use tempfile::TempDir;

const TEST_TOML: &'static str = r#"
[project]
name = "test"
version = "0.1.0"

[build]
clock_type = "posedge"
reset_type = "async_low"
target = {type = "source"}
#target = {type = "directory", path = "aaa"}

[format]
indent_width = 4
    "#;

fn create_metadata() -> (Metadata, TempDir) {
    std::env::set_var("GIT_AUTHOR_NAME", "veryl");
    std::env::set_var("GIT_AUTHOR_EMAIL", "veryl");
    std::env::set_var("GIT_COMMITTER_NAME", "veryl");
    std::env::set_var("GIT_COMMITTER_EMAIL", "veryl");

    let tempdir = tempfile::tempdir().unwrap();
    let toml_path = tempdir.path().join("Veryl.toml");
    fs::write(&toml_path, TEST_TOML).unwrap();
    let git = Git::init(tempdir.path()).unwrap();
    git.add(&toml_path).unwrap();
    git.commit(&"Add Veryl.toml").unwrap();
    (Metadata::load(&toml_path).unwrap(), tempdir)
}

#[test]
fn check_toml() {
    let metadata: Metadata = toml::from_str(TEST_TOML).unwrap();
    assert_eq!(metadata.project.name, "test");
    assert_eq!(metadata.project.version, Version::parse("0.1.0").unwrap());
    assert_eq!(metadata.build.clock_type, ClockType::PosEdge);
    assert_eq!(metadata.build.reset_type, ResetType::AsyncLow);
    assert_eq!(metadata.format.indent_width, 4);
}

#[test]
fn search_config() {
    let path = Metadata::search_from_current();
    assert!(path.is_ok());
}

#[test]
fn load() {
    let (metadata, _tempdir) = create_metadata();
    assert!(metadata.metadata_path.exists());
}

#[test]
fn check() {
    let (mut metadata, _tempdir) = create_metadata();
    assert!(metadata.check().is_ok());

    metadata.project.name = "0".to_string();
    assert!(metadata.check().is_err());

    metadata.project.name = "---".to_string();
    assert!(metadata.check().is_err());
}

#[test]
fn publish() {
    let (mut metadata, tempdir) = create_metadata();
    metadata.publish().unwrap();

    assert_eq!(
        metadata.pubdata.releases[0].version,
        metadata.project.version
    );
    assert!(metadata.pubdata_path.exists());
    let git = Git::open(tempdir.path()).unwrap();
    assert!(!git.is_clean().unwrap());
}

#[test]
fn publish_with_commit() {
    let (mut metadata, tempdir) = create_metadata();
    metadata.publish.publish_commit = true;
    metadata.publish.publish_commit_message = "chore: Publish".to_string();
    metadata.publish().unwrap();

    assert_eq!(
        metadata.pubdata.releases[0].version,
        metadata.project.version
    );
    assert!(metadata.pubdata_path.exists());
    let git = Git::open(tempdir.path()).unwrap();
    assert!(git.is_clean().unwrap());
}

#[test]
fn bump_version() {
    let (mut metadata, tempdir) = create_metadata();

    metadata.bump_version(BumpKind::Major).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.0.0").unwrap());

    metadata.bump_version(BumpKind::Minor).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.1.0").unwrap());

    metadata.bump_version(BumpKind::Patch).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.1.1").unwrap());

    let git = Git::open(tempdir.path()).unwrap();
    assert!(!git.is_clean().unwrap());
}

#[test]
fn bump_version_with_commit() {
    let (mut metadata, tempdir) = create_metadata();
    metadata.publish.bump_commit = true;
    metadata.publish.bump_commit_message = "chore: Bump version".to_string();

    metadata.bump_version(BumpKind::Major).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.0.0").unwrap());

    metadata.bump_version(BumpKind::Minor).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.1.0").unwrap());

    metadata.bump_version(BumpKind::Patch).unwrap();
    assert_eq!(metadata.project.version, Version::parse("1.1.1").unwrap());

    let git = Git::open(tempdir.path()).unwrap();
    assert!(git.is_clean().unwrap());
}
