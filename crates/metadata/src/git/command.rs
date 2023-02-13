use crate::metadata_error::MetadataError;
use log::info;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use url::Url;

pub struct Git {
    rev: Option<String>,
    tag: Option<String>,
    branch: Option<String>,
    path: PathBuf,
}

#[derive(Error, Debug)]
#[error("git operation failure: \"{msg}\"\n  {context}")]
pub struct GitCommandError {
    msg: String,
    context: String,
}

impl From<GitCommandError> for MetadataError {
    fn from(x: GitCommandError) -> MetadataError {
        MetadataError::Git(Box::new(x))
    }
}

#[cfg(windows)]
const GIT_COMMAND: &str = "git.exe";

#[cfg(not(windows))]
const GIT_COMMAND: &str = "git";

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
            let output = Command::new(GIT_COMMAND)
                .arg("clone")
                .arg(url.as_str())
                .arg(target)
                .current_dir(current_dir)
                .output()?;
            if !output.status.success() {
                let context = String::from_utf8_lossy(&output.stderr).to_string();
                let msg = format!("failed to clone repository: {}", url.as_str());
                return Err(GitCommandError { msg, context }.into());
            }
            info!("Cloned repository ({})", url);
        }

        Ok(Git {
            path: path.to_path_buf(),
            rev: rev.map(|x| x.to_owned()),
            tag: tag.map(|x| x.to_owned()),
            branch: branch.map(|x| x.to_owned()),
        })
    }

    pub fn fetch(&self) -> Result<(), MetadataError> {
        let output = Command::new(GIT_COMMAND)
            .arg("fetch")
            .current_dir(&self.path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!(
                "failed to fetch repository: {}",
                self.path.to_string_lossy()
            );
            return Err(GitCommandError { msg, context }.into());
        }

        info!("Fetched repository ({})", self.path.to_string_lossy());

        Ok(())
    }

    pub fn checkout(&self) -> Result<(), MetadataError> {
        let dst = if let Some(ref rev) = self.rev {
            rev.to_string()
        } else if let Some(ref tag) = self.tag {
            tag.to_string()
        } else if let Some(ref branch) = self.branch {
            format!("origin/{branch}")
        } else {
            "origin/HEAD".to_string()
        };

        let output = Command::new(GIT_COMMAND)
            .arg("checkout")
            .arg(&dst)
            .current_dir(&self.path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!(
                "failed to checkout repository: {}",
                self.path.to_string_lossy()
            );
            return Err(GitCommandError { msg, context }.into());
        }

        info!(
            "Checkouted repository ({} @ {})",
            self.path.to_string_lossy(),
            dst
        );

        Ok(())
    }

    pub fn get_revision(path: &Path) -> Result<String, MetadataError> {
        let output = Command::new(GIT_COMMAND)
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!("failed to get revision: {}", path.to_string_lossy());
            return Err(GitCommandError { msg, context }.into());
        }

        let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();

        Ok(revision)
    }

    pub fn is_clean(path: &Path) -> Result<bool, MetadataError> {
        let output = Command::new(GIT_COMMAND)
            .arg("status")
            .arg("-s")
            .current_dir(path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!("failed to get status: {}", path.to_string_lossy());
            return Err(GitCommandError { msg, context }.into());
        }

        Ok(output.stdout.is_empty())
    }
}
