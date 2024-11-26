use crate::metadata::UrlPath;
use crate::metadata_error::MetadataError;
use log::debug;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

pub struct Git {
    path: PathBuf,
}

#[derive(Error, Debug)]
#[error("git command failed: \"{msg}\"\n  {context}")]
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
    #[cfg(test)]
    pub fn init(path: &Path) -> Result<Self, MetadataError> {
        let output = Command::new(GIT_COMMAND)
            .arg("init")
            .current_dir(path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!("failed to init: {}", path.to_string_lossy());
            return Err(GitCommandError { msg, context }.into());
        }

        Ok(Git {
            path: path.to_path_buf(),
        })
    }

    pub fn open(path: &Path) -> Result<Self, MetadataError> {
        Ok(Git {
            path: path.to_path_buf(),
        })
    }

    pub fn clone(url: &UrlPath, path: &Path) -> Result<Self, MetadataError> {
        let current_dir = path.parent().unwrap();
        let target = path.file_name().unwrap();

        if !path.exists() {
            let output = Command::new(GIT_COMMAND)
                .arg("clone")
                .arg(url.to_string())
                .arg(target)
                .current_dir(current_dir)
                .output()?;
            if !output.status.success() {
                // retry at checkout failure
                if path.exists() {
                    let output = Command::new(GIT_COMMAND)
                        .arg("restore")
                        .arg("--source=HEAD")
                        .arg(":/")
                        .current_dir(path)
                        .output()?;
                    if output.status.success() {
                        return Ok(Git {
                            path: path.to_path_buf(),
                        });
                    }
                }

                let context = String::from_utf8_lossy(&output.stderr).to_string();
                let msg = format!("failed to clone repository: {}", url);
                return Err(GitCommandError { msg, context }.into());
            }
            debug!("Cloned repository ({})", url);
        }

        Ok(Git {
            path: path.to_path_buf(),
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

        debug!("Fetched repository ({})", self.path.to_string_lossy());

        Ok(())
    }

    pub fn checkout(&self, rev: Option<&str>) -> Result<(), MetadataError> {
        let dst = if let Some(ref rev) = rev {
            rev.to_string()
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

        debug!(
            "Checkouted repository ({} @ {})",
            self.path.to_string_lossy(),
            dst
        );

        Ok(())
    }

    pub fn get_revision(&self) -> Result<String, MetadataError> {
        let output = Command::new(GIT_COMMAND)
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(&self.path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!("failed to get revision: {}", self.path.to_string_lossy());
            return Err(GitCommandError { msg, context }.into());
        }

        let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();

        Ok(revision)
    }

    pub fn is_clean(&self) -> Result<bool, MetadataError> {
        let output = Command::new(GIT_COMMAND)
            .arg("status")
            .arg("-s")
            .current_dir(&self.path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!("failed to get status: {}", self.path.to_string_lossy());
            return Err(GitCommandError { msg, context }.into());
        }

        Ok(output.stdout.is_empty())
    }

    pub fn add(&self, file: &Path) -> Result<(), MetadataError> {
        let output = Command::new(GIT_COMMAND)
            .arg("add")
            .arg(file)
            .current_dir(&self.path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!("failed to add: {}", self.path.to_string_lossy());
            return Err(GitCommandError { msg, context }.into());
        }

        Ok(())
    }

    pub fn commit(&self, msg: &str) -> Result<(), MetadataError> {
        let output = Command::new(GIT_COMMAND)
            .arg("commit")
            .arg("-m")
            .arg(msg)
            .current_dir(&self.path)
            .output()?;
        if !output.status.success() {
            let context = String::from_utf8_lossy(&output.stderr).to_string();
            let msg = format!("failed to commit: {}", self.path.to_string_lossy());
            return Err(GitCommandError { msg, context }.into());
        }

        Ok(())
    }
}
