use directories::ProjectDirs;
use git_repository::progress::Discard;
use git_repository::remote::ref_map::Options;
use git_repository::remote::Direction;
use miette::{IntoDiagnostic, Result};
use std::sync::atomic::AtomicBool;
use veryl_metadata::Metadata;

pub struct DependencyManager;

impl DependencyManager {
    pub fn update(metadata: &Metadata) -> Result<()> {
        let project_dir = ProjectDirs::from("", "dalance", "veryl").unwrap();
        let cache_dir = project_dir.cache_dir();

        //let path = metadata
        //    .metadata_path
        //    .parent()
        //    .unwrap()
        //    .join("dependencies");
        //if path.exists() {
        //    std::fs::remove_dir_all(&path).into_diagnostic()?;
        //    std::fs::create_dir(&path).into_diagnostic()?;
        //} else {
        //    std::fs::create_dir(&path).into_diagnostic()?;
        //}

        std::env::set_var("GIT_COMMITTER_NAME", "veryl");
        std::env::set_var("GIT_COMMITTER_EMAIL", "veryl");

        for (name, url) in &metadata.dependencies {
            if let Some(ref url) = url.git {
                let url: git_repository::Url = url.as_str().try_into().into_diagnostic()?;

                let mut path = cache_dir.to_path_buf();
                path.push("repository");
                if let Some(host) = url.host() {
                    path.push(host);
                }
                path.push(url.path.to_string().trim_start_matches('/'));

                let parent = path.parent().unwrap();
                if !parent.exists() {
                    std::fs::create_dir_all(parent).into_diagnostic()?;
                }

                let repo = if path.exists() {
                    git_repository::open(&path).into_diagnostic()?
                } else {
                    let mut repo = git_repository::prepare_clone(url, &path).into_diagnostic()?;
                    let (mut repo, _) = repo
                        .fetch_then_checkout(Discard, &AtomicBool::new(false))
                        .into_diagnostic()?;
                    let (repo, _) = repo
                        .main_worktree(Discard, &AtomicBool::new(false))
                        .into_diagnostic()?;
                    repo
                };

                let remote = repo
                    .find_default_remote(Direction::Fetch)
                    .unwrap()
                    .into_diagnostic()?;
                let connect = remote
                    .connect(Direction::Fetch, Discard)
                    .into_diagnostic()?;
                let prepare = connect
                    .prepare_fetch(Options::default())
                    .into_diagnostic()?;
                let outcome = prepare.receive(&AtomicBool::new(false)).into_diagnostic()?;
                dbg!(&outcome);

                dbg!(repo
                    .head_commit()
                    .into_diagnostic()?
                    .short_id()
                    .into_diagnostic()?);
            }
        }

        Ok(())
    }
}
