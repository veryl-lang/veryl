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
