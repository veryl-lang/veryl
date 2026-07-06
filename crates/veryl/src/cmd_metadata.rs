use crate::{Format, OptMetadata};
use miette::{IntoDiagnostic, Result, bail};
use veryl_metadata::{Metadata, MetadataOutputV2};

pub struct CmdMetadata {
    opt: OptMetadata,
}

impl CmdMetadata {
    pub fn new(opt: OptMetadata) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let text = self.format_metadata(metadata)?;

        println!("{text}");

        Ok(true)
    }

    fn format_metadata(&self, metadata: &mut Metadata) -> Result<String> {
        match (self.opt.format, self.opt.format_version) {
            (Format::Json, None) => serde_json::to_string(metadata).into_diagnostic(),
            (Format::Pretty, None) => Ok(format!("{metadata:#?}")),
            (Format::Json, Some(1)) => serde_json::to_string(metadata).into_diagnostic(),
            (Format::Json, Some(2)) => {
                metadata.update_lockfile()?;
                let output = MetadataOutputV2::from_metadata(metadata)?;
                serde_json::to_string(&output).into_diagnostic()
            }
            (Format::Pretty, Some(_)) => {
                bail!("--format-version is only supported with --format json")
            }
            (Format::Json, Some(version)) => {
                bail!("unsupported --format-version {version}; supported versions: 1, 2")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    const TEST_TOML: &str = r#"
[project]
name = "test"
version = "0.1.0"

[metadata.external_tool]
files = ["src/**/*.v"]
"#;

    fn load_metadata() -> (Metadata, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().unwrap();
        let project_dir = tempdir.path().join("test");
        fs::create_dir(&project_dir).unwrap();
        let toml_path = project_dir.join("Veryl.toml");
        fs::write(&toml_path, TEST_TOML).unwrap();
        let metadata = Metadata::load(&toml_path).unwrap();
        (metadata, tempdir)
    }

    fn load_metadata_with_path_dependency() -> (Metadata, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().unwrap();
        let root_dir = tempdir.path().join("root");
        let dependency_dir = tempdir.path().join("dep");
        fs::create_dir(&root_dir).unwrap();
        fs::create_dir(&dependency_dir).unwrap();
        fs::write(
            root_dir.join("Veryl.toml"),
            r#"
[project]
name = "root"
version = "0.1.0"

[dependencies]
dep = {path = "../dep"}
"#,
        )
        .unwrap();
        fs::write(
            dependency_dir.join("Veryl.toml"),
            r#"
[project]
name = "real_dep"
version = "0.1.0"

[metadata.external_tool]
role = "dependency"
"#,
        )
        .unwrap();
        let metadata = Metadata::load(root_dir.join("Veryl.toml")).unwrap();
        (metadata, tempdir)
    }

    fn command(format: Format, format_version: Option<u32>) -> CmdMetadata {
        CmdMetadata::new(OptMetadata {
            format,
            format_version,
        })
    }

    #[test]
    fn json_format_version_1_preserves_internal_metadata_shape() {
        let (mut metadata, _tempdir) = load_metadata();

        let versioned_text = command(Format::Json, Some(1))
            .format_metadata(&mut metadata)
            .unwrap();
        let unversioned_text = command(Format::Json, None)
            .format_metadata(&mut metadata)
            .unwrap();
        let versioned_value: serde_json::Value = serde_json::from_str(&versioned_text).unwrap();
        let unversioned_value: serde_json::Value = serde_json::from_str(&unversioned_text).unwrap();

        assert_eq!(versioned_value, unversioned_value);
        assert!(versioned_value.get("format_version").is_none());
        assert!(versioned_value.get("root").is_none());
        assert!(versioned_value.get("project").is_some());
        assert_eq!(
            versioned_value["metadata"]["external_tool"]["files"][0],
            "src/**/*.v"
        );
    }

    #[test]
    fn json_format_version_2_emits_stable_graph_metadata_shape() {
        let (mut metadata, _tempdir) = load_metadata();

        let text = command(Format::Json, Some(2)).format_metadata(&mut metadata);

        assert!(
            text.as_ref().is_ok(),
            "format version 2 should be supported: {:?}",
            text.as_ref().err()
        );
        let value: serde_json::Value = serde_json::from_str(&text.unwrap()).unwrap();
        assert_eq!(value["format_version"], 2);
        assert_eq!(value["root"]["name"], "test");
        assert_eq!(
            value["root"]["metadata"]["external_tool"]["files"][0],
            "src/**/*.v"
        );
        assert!(value["dependencies"].as_array().unwrap().is_empty());
        assert!(value.get("metadata").is_none());
        assert!(value.get("project").is_none());
    }

    #[test]
    fn unversioned_json_preserves_internal_metadata_shape() {
        let (mut metadata, _tempdir) = load_metadata();

        let text = command(Format::Json, None)
            .format_metadata(&mut metadata)
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert!(value.get("format_version").is_none());
        assert!(value.get("root").is_none());
        assert!(value.get("project").is_some());
    }

    #[test]
    fn pretty_format_version_is_rejected() {
        let (mut metadata, _tempdir) = load_metadata();

        for version in [1, 2] {
            let error = command(Format::Pretty, Some(version))
                .format_metadata(&mut metadata)
                .unwrap_err();

            assert!(
                error
                    .to_string()
                    .contains("--format-version is only supported with --format json")
            );
        }
    }

    #[test]
    fn unsupported_format_version_is_rejected() {
        let (mut metadata, _tempdir) = load_metadata();

        let error = command(Format::Json, Some(3))
            .format_metadata(&mut metadata)
            .unwrap_err();

        assert!(error.to_string().contains("unsupported --format-version 3"));
        assert!(error.to_string().contains("supported versions: 1, 2"));
    }

    #[test]
    fn json_format_version_2_resolves_missing_lockfile() {
        let (mut metadata, _tempdir) = load_metadata_with_path_dependency();
        assert!(!metadata.lockfile_path.exists());

        let text = command(Format::Json, Some(2))
            .format_metadata(&mut metadata)
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert!(metadata.lockfile_path.exists());
        assert_eq!(value["format_version"], 2);
        assert_eq!(value["root"]["metadata"], serde_json::json!({}));
        assert_eq!(value["dependencies"][0]["name"], "dep");
        assert_eq!(value["dependencies"][0]["project"], "real_dep");
        assert_eq!(value["dependencies"][0]["source"]["kind"], "path");
        let local_path = value["dependencies"][0]["local_path"].as_str().unwrap();
        assert!(Path::new(local_path).is_absolute());
        assert_eq!(
            value["dependencies"][0]["metadata"]["external_tool"]["role"],
            "dependency"
        );
    }

    #[test]
    fn legacy_json_does_not_resolve_missing_lockfile() {
        let (mut metadata, _tempdir) = load_metadata_with_path_dependency();
        assert!(!metadata.lockfile_path.exists());

        let _text = command(Format::Json, Some(1))
            .format_metadata(&mut metadata)
            .unwrap();

        assert!(!metadata.lockfile_path.exists());
    }

    #[test]
    fn unversioned_json_does_not_resolve_missing_lockfile() {
        let (mut metadata, _tempdir) = load_metadata_with_path_dependency();
        assert!(!metadata.lockfile_path.exists());

        let _text = command(Format::Json, None)
            .format_metadata(&mut metadata)
            .unwrap();

        assert!(!metadata.lockfile_path.exists());
    }
}
