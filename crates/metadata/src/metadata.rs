use crate::MetadataError;
use semver::Version;
use serde::Deserialize;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub package: Package,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub format: Format,
}

impl Metadata {
    pub fn search_from_current() -> Result<PathBuf, MetadataError> {
        Metadata::search_from(env::current_dir()?)
    }

    pub fn search_from<T: AsRef<Path>>(from: T) -> Result<PathBuf, MetadataError> {
        for path in from.as_ref().ancestors() {
            let path = path.join("Veryl.toml");
            if path.is_file() {
                return Ok(path);
            }
        }

        Err(MetadataError::FileNotFound)
    }

    pub fn load<T: AsRef<Path>>(path: T) -> Result<Self, MetadataError> {
        let text = std::fs::read_to_string(path)?;
        let metadata: Metadata = toml::from_str(&text)?;
        Ok(metadata)
    }
}

#[derive(Debug, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: Version,
    #[serde(default)]
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub repository: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Build {
    #[serde(default)]
    pub clock_type: ClockType,
    #[serde(default)]
    pub reset_type: ResetType,
    #[serde(default)]
    pub target: Target,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum ClockType {
    #[default]
    #[serde(rename = "posedge")]
    PosEdge,
    #[serde(rename = "negedge")]
    NegEdge,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum ResetType {
    #[default]
    #[serde(rename = "async_low")]
    AsyncLow,
    #[serde(rename = "async_high")]
    AsyncHigh,
    #[serde(rename = "sync_low")]
    SyncLow,
    #[serde(rename = "sync_high")]
    SyncHigh,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum Target {
    #[default]
    #[serde(rename = "source")]
    Source,
    #[serde(rename = "directory")]
    Directory { path: PathBuf },
}

#[derive(Debug, Deserialize)]
pub struct Format {
    #[serde(default = "default_indent_width")]
    pub indent_width: usize,
}

const DEFAULT_INDENT_WIDTH: usize = 4;

impl Default for Format {
    fn default() -> Self {
        Self {
            indent_width: DEFAULT_INDENT_WIDTH,
        }
    }
}

fn default_indent_width() -> usize {
    DEFAULT_INDENT_WIDTH
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::{BuildMetadata, Prerelease};

    const TEST_TOML: &'static str = r#"
[package]
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

    #[test]
    fn load_toml() {
        let metadata: Metadata = toml::from_str(TEST_TOML).unwrap();
        assert_eq!(metadata.package.name, "test");
        assert_eq!(
            metadata.package.version,
            Version {
                major: 0,
                minor: 1,
                patch: 0,
                pre: Prerelease::EMPTY,
                build: BuildMetadata::EMPTY,
            }
        );
        assert_eq!(metadata.build.clock_type, ClockType::PosEdge);
        assert_eq!(metadata.build.reset_type, ResetType::AsyncLow);
        assert_eq!(metadata.format.indent_width, 4);
    }

    #[test]
    fn search_config() {
        let path = Metadata::search_from_current();
        assert!(path.is_ok());
    }
}
