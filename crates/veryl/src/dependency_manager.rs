use crate::git::Git;
use crate::utils::{self, PathPair};
use directories::ProjectDirs;
use miette::{IntoDiagnostic, Result};
use veryl_metadata::Metadata;

#[derive(Clone, Debug)]
pub struct DependencyManager;

impl DependencyManager {
    pub fn gather(metadata: &Metadata) -> Result<Vec<PathPair>> {
        let project_dir = ProjectDirs::from("", "dalance", "veryl").unwrap();
        let cache_dir = project_dir.cache_dir();

        let dst_base = metadata
            .metadata_path
            .parent()
            .unwrap()
            .join("dependencies");
        if !dst_base.exists() {
            std::fs::create_dir(&dst_base).into_diagnostic()?;
        }

        let mut ret = Vec::new();

        for (name, dep) in &metadata.dependencies {
            if let Some(ref git) = dep.git {
                let mut path = cache_dir.to_path_buf();
                path.push("repository");
                if let Some(host) = git.host_str() {
                    path.push(host);
                }
                path.push(git.path().to_string().trim_start_matches('/'));

                if let Some(ref rev) = dep.rev {
                    path.set_extension(rev);
                } else if let Some(ref tag) = dep.tag {
                    path.set_extension(tag);
                } else if let Some(ref branch) = dep.branch {
                    path.set_extension(branch);
                }

                let parent = path.parent().unwrap();
                if !parent.exists() {
                    std::fs::create_dir_all(parent).into_diagnostic()?;
                }

                let git = Git::clone(
                    git,
                    &path,
                    dep.rev.as_deref(),
                    dep.tag.as_deref(),
                    dep.branch.as_deref(),
                )?;
                git.fetch()?;
                git.checkout()?;

                for src in &utils::gather_vl_files(&path)? {
                    let rel = src.strip_prefix(&path).into_diagnostic()?;
                    let mut dst = dst_base.join(name);
                    dst.push(rel);
                    dst.set_extension("sv");
                    ret.push(PathPair {
                        prj: name.clone(),
                        src: src.to_path_buf(),
                        dst,
                    });
                }
            }
        }

        Ok(ret)
    }
}
