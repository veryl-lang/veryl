#[cfg(feature = "git-command")]
mod command;
#[cfg(gitoxide_enabled)]
mod gitoxide;

use crate::metadata::UrlPath;
use crate::metadata_error::MetadataError;
use log::warn;
use std::path::Path;

const ENV_VAR: &str = "VERYL_GIT_BACKEND";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Selection {
    Auto,
    Gitoxide,
    Command,
}

fn parse_selection() -> Selection {
    match std::env::var(ENV_VAR).ok().as_deref() {
        Some("gitoxide") => Selection::Gitoxide,
        Some("command") => Selection::Command,
        Some("auto") | None | Some("") => Selection::Auto,
        Some(other) => {
            warn!("unknown {ENV_VAR}='{other}', using auto");
            Selection::Auto
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pick {
    #[cfg(gitoxide_enabled)]
    Gitoxide,
    #[cfg(feature = "git-command")]
    Command,
}

fn pick_name(p: Pick) -> &'static str {
    match p {
        #[cfg(gitoxide_enabled)]
        Pick::Gitoxide => "gitoxide",
        #[cfg(feature = "git-command")]
        Pick::Command => "command",
    }
}

fn pick_backends(selection: Selection) -> (Pick, Option<Pick>) {
    match selection {
        Selection::Auto => default_with_fallback(),
        Selection::Gitoxide => {
            #[cfg(gitoxide_enabled)]
            {
                (Pick::Gitoxide, None)
            }
            #[cfg(all(not(gitoxide_enabled), feature = "git-command"))]
            {
                warn!("{ENV_VAR}=gitoxide requested but feature is not enabled, using command");
                (Pick::Command, None)
            }
        }
        Selection::Command => {
            #[cfg(feature = "git-command")]
            {
                (Pick::Command, None)
            }
            #[cfg(all(not(feature = "git-command"), gitoxide_enabled))]
            {
                warn!("{ENV_VAR}=command requested but feature is not enabled, using gitoxide");
                (Pick::Gitoxide, None)
            }
        }
    }
}

fn default_with_fallback() -> (Pick, Option<Pick>) {
    #[cfg(all(gitoxide_enabled, feature = "git-command"))]
    {
        (Pick::Gitoxide, Some(Pick::Command))
    }
    #[cfg(all(gitoxide_enabled, not(feature = "git-command")))]
    {
        (Pick::Gitoxide, None)
    }
    #[cfg(all(not(gitoxide_enabled), feature = "git-command"))]
    {
        (Pick::Command, None)
    }
}

pub enum Git {
    #[cfg(gitoxide_enabled)]
    Gitoxide(gitoxide::Git),
    #[cfg(feature = "git-command")]
    Command(command::Git),
}

impl Git {
    pub fn exists() -> bool {
        #[cfg(gitoxide_enabled)]
        if gitoxide::Git::exists() {
            return true;
        }
        #[cfg(feature = "git-command")]
        if command::Git::exists() {
            return true;
        }
        false
    }

    pub fn init(path: &Path) -> Result<Self, MetadataError> {
        dispatch_construct(|p| try_init(p, path))
    }

    pub fn open(path: &Path) -> Result<Self, MetadataError> {
        dispatch_construct(|p| try_open(p, path))
    }

    pub fn clone(url: &UrlPath, path: &Path) -> Result<Self, MetadataError> {
        dispatch_construct(|p| try_clone(p, url, path))
    }

    pub fn fetch(&self) -> Result<(), MetadataError> {
        match self {
            #[cfg(gitoxide_enabled)]
            Self::Gitoxide(g) => g.fetch(),
            #[cfg(feature = "git-command")]
            Self::Command(g) => g.fetch(),
        }
    }

    pub fn checkout(&self, rev: Option<&str>) -> Result<(), MetadataError> {
        match self {
            #[cfg(gitoxide_enabled)]
            Self::Gitoxide(g) => g.checkout(rev),
            #[cfg(feature = "git-command")]
            Self::Command(g) => g.checkout(rev),
        }
    }

    pub fn get_revision(&self) -> Result<String, MetadataError> {
        match self {
            #[cfg(gitoxide_enabled)]
            Self::Gitoxide(g) => g.get_revision(),
            #[cfg(feature = "git-command")]
            Self::Command(g) => g.get_revision(),
        }
    }

    pub fn is_git(&self) -> Result<bool, MetadataError> {
        match self {
            #[cfg(gitoxide_enabled)]
            Self::Gitoxide(g) => g.is_git(),
            #[cfg(feature = "git-command")]
            Self::Command(g) => g.is_git(),
        }
    }

    pub fn is_clean(&self) -> Result<bool, MetadataError> {
        match self {
            #[cfg(gitoxide_enabled)]
            Self::Gitoxide(g) => g.is_clean(),
            #[cfg(feature = "git-command")]
            Self::Command(g) => g.is_clean(),
        }
    }

    pub fn add(&self, file: &Path) -> Result<(), MetadataError> {
        match self {
            #[cfg(gitoxide_enabled)]
            Self::Gitoxide(g) => g.add(file),
            #[cfg(feature = "git-command")]
            Self::Command(g) => g.add(file),
        }
    }

    pub fn commit(&self, msg: &str) -> Result<(), MetadataError> {
        match self {
            #[cfg(gitoxide_enabled)]
            Self::Gitoxide(g) => g.commit(msg),
            #[cfg(feature = "git-command")]
            Self::Command(g) => g.commit(msg),
        }
    }
}

fn try_init(pick: Pick, path: &Path) -> Result<Git, MetadataError> {
    match pick {
        #[cfg(gitoxide_enabled)]
        Pick::Gitoxide => gitoxide::Git::init(path).map(Git::Gitoxide),
        #[cfg(feature = "git-command")]
        Pick::Command => command::Git::init(path).map(Git::Command),
    }
}

fn try_open(pick: Pick, path: &Path) -> Result<Git, MetadataError> {
    match pick {
        #[cfg(gitoxide_enabled)]
        Pick::Gitoxide => gitoxide::Git::open(path).map(Git::Gitoxide),
        #[cfg(feature = "git-command")]
        Pick::Command => command::Git::open(path).map(Git::Command),
    }
}

fn try_clone(pick: Pick, url: &UrlPath, path: &Path) -> Result<Git, MetadataError> {
    match pick {
        #[cfg(gitoxide_enabled)]
        Pick::Gitoxide => gitoxide::Git::clone(url, path).map(Git::Gitoxide),
        #[cfg(feature = "git-command")]
        Pick::Command => command::Git::clone(url, path).map(Git::Command),
    }
}

fn dispatch_construct<F>(f: F) -> Result<Git, MetadataError>
where
    F: Fn(Pick) -> Result<Git, MetadataError>,
{
    let (primary, fallback) = pick_backends(parse_selection());
    match f(primary) {
        Ok(g) => Ok(g),
        Err(e) => {
            if let Some(fb) = fallback {
                warn!(
                    "git backend '{}' failed ({e}), falling back to '{}'",
                    pick_name(primary),
                    pick_name(fb)
                );
                f(fb)
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(all(test, gitoxide_enabled, feature = "git-command"))]
mod tests {
    use super::*;
    use crate::metadata::UrlPath;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "veryl")
            .env("GIT_AUTHOR_EMAIL", "veryl")
            .env("GIT_COMMITTER_NAME", "veryl")
            .env("GIT_COMMITTER_EMAIL", "veryl")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn create_remote(root: &Path) -> (UrlPath, PathBuf) {
        let remote = root.join("remote");
        fs::create_dir(&remote).unwrap();
        run_git(&remote, &["init", "-b", "main"]);
        fs::write(remote.join("a.txt"), "1").unwrap();
        run_git(&remote, &["add", "a.txt"]);
        run_git(&remote, &["commit", "-m", "first"]);
        let url = format!("file://{}", remote.to_string_lossy().replace('\\', "/"));
        (UrlPath::Url(url.parse().unwrap()), remote)
    }

    fn advance_remote(remote: &Path) -> String {
        fs::write(remote.join("b.txt"), "2").unwrap();
        run_git(remote, &["add", "b.txt"]);
        run_git(remote, &["commit", "-m", "second"]);
        run_git(remote, &["rev-parse", "HEAD"])
    }

    fn check_fetch_updated_remote<C, O>(clone: C, open: O)
    where
        C: Fn(&UrlPath, &Path) -> Result<Git, MetadataError>,
        O: Fn(&Path) -> Result<Git, MetadataError>,
    {
        let tempdir = tempfile::tempdir().unwrap();
        let (url, remote) = create_remote(tempdir.path());
        let clone_path = tempdir.path().join("clone");

        let git = clone(&url, &clone_path).unwrap();
        git.checkout(None).unwrap();

        let new_rev = advance_remote(&remote);

        let git = open(&clone_path).unwrap();
        git.fetch().unwrap();
        git.checkout(None).unwrap();
        assert_eq!(git.get_revision().unwrap(), new_rev);
    }

    // Simulate a clone left by an older gitoxide backend, which wrote
    // origin/HEAD as a direct ref pinned to the commit at clone time.
    fn pollute_origin_head(clone_path: &Path) {
        let old = run_git(clone_path, &["rev-parse", "refs/remotes/origin/HEAD"]);
        run_git(
            clone_path,
            &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
        );
        run_git(
            clone_path,
            &["update-ref", "refs/remotes/origin/HEAD", &old],
        );
    }

    fn check_fetch_heals_direct_origin_head<O>(open: O)
    where
        O: Fn(&Path) -> Result<Git, MetadataError>,
    {
        let tempdir = tempfile::tempdir().unwrap();
        let (url, remote) = create_remote(tempdir.path());
        let clone_path = tempdir.path().join("clone");

        gitoxide::Git::clone(&url, &clone_path).unwrap();
        pollute_origin_head(&clone_path);

        let new_rev = advance_remote(&remote);

        let git = open(&clone_path).unwrap();
        git.fetch().unwrap();
        git.checkout(None).unwrap();
        assert_eq!(git.get_revision().unwrap(), new_rev);
        let target = run_git(&clone_path, &["symbolic-ref", "refs/remotes/origin/HEAD"]);
        assert_eq!(target, "refs/remotes/origin/main");
    }

    #[test]
    fn gitoxide_fetch_heals_direct_origin_head() {
        check_fetch_heals_direct_origin_head(|path| gitoxide::Git::open(path).map(Git::Gitoxide));
    }

    #[test]
    fn command_fetch_heals_direct_origin_head() {
        check_fetch_heals_direct_origin_head(|path| command::Git::open(path).map(Git::Command));
    }

    #[test]
    fn gitoxide_clone_writes_symbolic_origin_head() {
        let tempdir = tempfile::tempdir().unwrap();
        let (url, _remote) = create_remote(tempdir.path());
        let clone_path = tempdir.path().join("clone");
        gitoxide::Git::clone(&url, &clone_path).unwrap();
        let target = run_git(&clone_path, &["symbolic-ref", "refs/remotes/origin/HEAD"]);
        assert_eq!(target, "refs/remotes/origin/main");
    }

    #[test]
    fn gitoxide_fetch_updated_remote() {
        check_fetch_updated_remote(
            |url, path| gitoxide::Git::clone(url, path).map(Git::Gitoxide),
            |path| gitoxide::Git::open(path).map(Git::Gitoxide),
        );
    }

    #[test]
    fn command_fetch_updated_remote() {
        check_fetch_updated_remote(
            |url, path| command::Git::clone(url, path).map(Git::Command),
            |path| command::Git::open(path).map(Git::Command),
        );
    }

    #[test]
    fn command_fetch_updated_remote_after_gitoxide_clone() {
        check_fetch_updated_remote(
            |url, path| gitoxide::Git::clone(url, path).map(Git::Gitoxide),
            |path| command::Git::open(path).map(Git::Command),
        );
    }

    #[test]
    fn gitoxide_fetch_updated_remote_after_command_clone() {
        check_fetch_updated_remote(
            |url, path| command::Git::clone(url, path).map(Git::Command),
            |path| gitoxide::Git::open(path).map(Git::Gitoxide),
        );
    }
}
