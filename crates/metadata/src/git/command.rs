use crate::metadata_error::MetadataError;
use std::path::{Path, PathBuf};
use std::process::Command;
use url::Url;

pub struct Git {
    rev: Option<String>,
    tag: Option<String>,
    branch: Option<String>,
    path: PathBuf,
}

impl Git {
    pub fn clone(
        url: &Url,
        path: &Path,
        rev: Option<&str>,
        tag: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Self, MetadataError> {
        let current_dir = path.parent().unwrap();
        let target = path.file_name().unwrap();
        if !path.exists() {
            let output = Command::new("git")
                .arg("clone")
                .arg(url.as_str())
                .arg(target)
                .current_dir(current_dir)
                .output()?;
            if !output.status.success() {
                let context = String::from_utf8_lossy(&output.stderr).to_string();
                let msg = format!("failed to clone repository: {}", url.as_str());
                return Err(MetadataError::Git { msg, context });
            }
        }

        Ok(Git {
            path: path.to_path_buf(),
            rev: rev.map(|x| x.to_owned()),
            tag: tag.map(|x| x.to_owned()),
            branch: branch.map(|x| x.to_owned()),
        })
    }

    pub fn fetch(&self) -> Result<(), MetadataError> {
        let output = Command::new("git")
            .arg("fetch")
            .current_dir(&self.path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!(
                "failed to fetch repository: {}",
                self.path.to_string_lossy()
            );
            return Err(MetadataError::Git { msg, context });
        }

        Ok(())
    }

    pub fn checkout(&self) -> Result<(), MetadataError> {
        let dst = if let Some(ref rev) = self.rev {
            rev.to_string()
        } else if let Some(ref tag) = self.tag {
            tag.to_string()
        } else if let Some(ref branch) = self.branch {
            format!("origin/{}", branch)
        } else {
            "origin/HEAD".to_string()
        };

        let output = Command::new("git")
            .arg("checkout")
            .arg(dst)
            .current_dir(&self.path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!(
                "failed to checkout repository: {}",
                self.path.to_string_lossy()
            );
            return Err(MetadataError::Git { msg, context });
        }

        Ok(())
    }
}
