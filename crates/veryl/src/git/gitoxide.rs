use git_repository::progress::Discard;
use git_repository::remote::ref_map::Options;
use git_repository::remote::Direction;
use git_repository::Repository;
use miette::{IntoDiagnostic, Result};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use url::Url;

pub struct Git {
    repo: Repository,
}

impl Git {
    pub fn clone(url: &Url, path: &Path) -> Result<Self> {
        std::env::set_var("GIT_COMMITTER_NAME", "veryl");
        std::env::set_var("GIT_COMMITTER_EMAIL", "veryl");

        let repo = if path.exists() {
            git_repository::open(&path).into_diagnostic()?
        } else {
            let url: git_repository::Url = url.as_str().try_into().into_diagnostic()?;
            let mut repo = git_repository::prepare_clone(url, &path).into_diagnostic()?;
            let (mut repo, _) = repo
                .fetch_then_checkout(Discard, &AtomicBool::new(false))
                .into_diagnostic()?;
            let (repo, _) = repo
                .main_worktree(Discard, &AtomicBool::new(false))
                .into_diagnostic()?;
            repo
        };

        Ok(Git { repo })
    }

    pub fn fetch(self) -> Result<()> {
        let remote = self
            .repo
            .find_default_remote(Direction::Fetch)
            .unwrap()
            .into_diagnostic()?;
        let connect = remote
            .connect(Direction::Fetch, Discard)
            .into_diagnostic()?;
        let prepare = connect
            .prepare_fetch(Options::default())
            .into_diagnostic()?;
        let _outcome = prepare.receive(&AtomicBool::new(false)).into_diagnostic()?;
        Ok(())
    }
}
