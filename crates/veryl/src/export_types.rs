//! The description of the packages and interfaces marked `#[export(rust)]`
//! (`veryl.types.json`), written into each component crate, where
//! `veryl_component::import!` reads it.

use crate::diff::print_diff;
use crate::utils;
use log::{info, warn};
use miette::{IntoDiagnostic, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use veryl_metadata::{ExportDescription, Metadata};

/// Writes the descriptions, by project, built by the analysis of the
/// exported part: the project's own, and those of the dependencies given by
/// a path, which are local projects edited together with it. A dependency
/// from a repository keeps the description its publisher committed.
/// See [`write_project`] for each. Returns whether every file is up to date
/// (with `check`), and the files written.
pub fn write(
    metadata: &Metadata,
    descriptions: &HashMap<String, ExportDescription>,
    check: bool,
    quiet: bool,
) -> Result<(bool, Vec<PathBuf>)> {
    let empty = ExportDescription::new();
    let mut up_to_date = true;
    let mut written = Vec::new();

    let description = descriptions.get(&metadata.project.name).unwrap_or(&empty);
    up_to_date &= write_project(metadata, description, check, quiet, &mut written)?;

    match metadata.collect_path_dependencies() {
        Ok(dependencies) => {
            for (name, dependency) in &dependencies {
                let description = descriptions.get(name).unwrap_or(&empty);
                up_to_date &= write_project(dependency, description, check, quiet, &mut written)?;
            }
        }
        Err(e) => warn!("Failed to collect path dependencies: {e}"),
    }

    Ok((up_to_date, written))
}

/// Writes `description` into each `[[components]]` package of the project
/// of `metadata`, libraries included. The file is committed with the
/// package, so it is written only when its content changes. With nothing
/// exported, no file is written, and one left from an earlier export is
/// removed: `import!` would expand what is gone. With `check`, the files are
/// compared instead, and the result tells whether every file is up to date.
fn write_project(
    metadata: &Metadata,
    description: &ExportDescription,
    check: bool,
    quiet: bool,
    written: &mut Vec<PathBuf>,
) -> Result<bool> {
    let description = (!description.is_empty()).then(|| description.to_json());

    let mut up_to_date = true;
    for component in &metadata.components {
        let crate_dir = metadata.project_path().join(&component.path);
        if !crate_dir.is_dir() {
            continue;
        }
        let path = crate_dir.join(veryl_metadata::EXPORT_DESCRIPTION_FILE);
        let current = fs::read_to_string(&path).ok();
        if current == description {
            continue;
        }
        if check {
            if !quiet {
                print_diff(
                    &path,
                    current.as_deref().unwrap_or_default(),
                    description.as_deref().unwrap_or_default(),
                );
            }
            up_to_date = false;
        } else if let Some(description) = &description {
            utils::write_file_if_changed(&path, description.as_bytes())?;
            info!("Output file ({})", path.to_string_lossy());
            written.push(path);
        } else {
            fs::remove_file(&path).into_diagnostic()?;
            info!("Removed file ({})", path.to_string_lossy());
        }
    }
    Ok(up_to_date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use veryl_metadata::export_description::{ExportInterface, ExportModport};

    fn metadata(root: &std::path::Path, toml: &str) -> Metadata {
        fs::write(root.join("Veryl.toml"), toml).unwrap();
        let mut metadata: Metadata = toml.parse().unwrap();
        metadata.metadata_path = root.join("Veryl.toml");
        metadata
    }

    #[test]
    fn written_to_components_and_libraries_and_removed_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for x in ["comp", "lib", "other"] {
            fs::create_dir(root.join(x)).unwrap();
        }
        let metadata = metadata(
            root,
            r#"
[project]
name = "prj"
version = "0.1.0"

[[components]]
path = "comp"

[[components]]
path = "lib"
library = true
"#,
        );

        let mut description = ExportDescription::new();
        description.interfaces.insert(
            "bus_if".to_string(),
            ExportInterface {
                modports: vec![ExportModport {
                    name: "master".to_string(),
                    members: vec![],
                }],
            },
        );
        let mut written = Vec::new();
        write_project(&metadata, &description, false, true, &mut written).unwrap();
        assert_eq!(written.len(), 2);
        let file = |x: &str| root.join(x).join(veryl_metadata::EXPORT_DESCRIPTION_FILE);
        assert!(file("comp").exists() && file("lib").exists() && !file("other").exists());

        // Up to date: nothing is written, and the check passes.
        let mut written = Vec::new();
        let up_to_date = write_project(&metadata, &description, true, true, &mut written).unwrap();
        assert!(up_to_date && written.is_empty());

        // With nothing exported, the files are stale: reported, then removed.
        let empty = ExportDescription::new();
        let up_to_date = write_project(&metadata, &empty, true, true, &mut written).unwrap();
        assert!(!up_to_date);
        write_project(&metadata, &empty, false, true, &mut written).unwrap();
        assert!(written.is_empty());
        assert!(!file("comp").exists() && !file("lib").exists());
    }
}
