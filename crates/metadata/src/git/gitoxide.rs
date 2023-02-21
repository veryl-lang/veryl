use crate::metadata_error::MetadataError;
use git_repository::index::File;
use git_repository::objs::BlobRef;
use git_repository::prelude::FindExt;
use git_repository::progress::Discard;
use git_repository::remote::ref_map::Options;
use git_repository::remote::Direction;
use git_repository::worktree::index;
use git_repository::Repository;
use log::debug;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use thiserror::Error;
use url::Url;

pub struct Git {
    rev: Option<String>,
    tag: Option<String>,
    branch: Option<String>,
    repo: Repository,
}

#[derive(Error, Debug)]
pub enum GitoxideError {
    #[error("git error")]
    Open(#[from] git_repository::open::Error),

    #[error("git error")]
    UrlParse(#[from] git_repository::url::parse::Error),

    #[error("git error")]
    Clone(#[from] git_repository::clone::Error),

    #[error("git error")]
    CloneFetch(#[from] git_repository::clone::fetch::Error),

    #[error("git error")]
    CloneCheckoutMainWorktree(#[from] git_repository::clone::checkout::main_worktree::Error),

    #[error("git error")]
    RemoteFindExisting(#[from] git_repository::remote::find::existing::Error),

    #[error("git error")]
    RemoteConnect(#[from] git_repository::remote::connect::Error),

    #[error("git error")]
    RemoteFetchPrepare(#[from] git_repository::remote::fetch::prepare::Error),

    #[error("git error")]
    RemoteFetch(#[from] git_repository::remote::fetch::Error),

    #[error("git error")]
    ReferenceFindExisting(#[from] git_repository::reference::find::existing::Error),

    #[error("git error")]
    ReferencePeel(#[from] git_repository::reference::peel::Error),

    #[error("git error")]
    IndexFileInit(#[from] git_repository::index::file::init::Error),

    #[error("git error")]
    WorktreeIndexCheckout(
        #[from] git_repository::worktree::index::checkout::Error<Box<GitoxideError>>,
    ),

    #[error("git error")]
    RevisionSpecParseSingle(#[from] git_repository::revision::spec::parse::single::Error),

    #[error("git error")]
    OdbStoreFind(
        #[from]
        git_repository::odb::find::existing_object::Error<git_repository::odb::store::find::Error>,
    ),
}

impl From<GitoxideError> for MetadataError {
    fn from(x: GitoxideError) -> MetadataError {
        MetadataError::Git(Box::new(x))
    }
}

macro_rules! from_for_metadata_error {
    ($x:ty) => {
        impl From<$x> for MetadataError {
            fn from(x: $x) -> MetadataError {
                x.into()
            }
        }
    };
}

from_for_metadata_error!(git_repository::open::Error);
from_for_metadata_error!(git_repository::url::parse::Error);
from_for_metadata_error!(git_repository::clone::Error);
from_for_metadata_error!(git_repository::clone::fetch::Error);
from_for_metadata_error!(git_repository::clone::checkout::main_worktree::Error);
from_for_metadata_error!(git_repository::remote::find::existing::Error);
from_for_metadata_error!(git_repository::remote::connect::Error);
from_for_metadata_error!(git_repository::remote::fetch::prepare::Error);
from_for_metadata_error!(git_repository::remote::fetch::Error);
from_for_metadata_error!(git_repository::reference::find::existing::Error);
from_for_metadata_error!(git_repository::reference::peel::Error);
from_for_metadata_error!(git_repository::index::file::init::Error);
from_for_metadata_error!(git_repository::worktree::index::checkout::Error<Box<GitoxideError>>);
from_for_metadata_error!(
    git_repository::worktree::index::checkout::Error<
        git_repository::odb::find::existing_object::Error<git_repository::odb::store::find::Error>,
    >
);
from_for_metadata_error!(git_repository::revision::spec::parse::single::Error);
from_for_metadata_error!(
    git_repository::odb::find::existing_object::Error<git_repository::odb::store::find::Error>
);

impl Git {
    pub fn clone(
        url: &Url,
        path: &Path,
        rev: Option<&str>,
        tag: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Self, MetadataError> {
        std::env::set_var("GIT_COMMITTER_NAME", "veryl");
        std::env::set_var("GIT_COMMITTER_EMAIL", "veryl");

        let repo = if path.exists() {
            git_repository::open(path)?
        } else {
            let git_url: git_repository::Url = url.as_str().try_into()?;
            let mut repo = git_repository::prepare_clone(git_url, path)?;
            let (mut repo, _) = repo.fetch_then_checkout(Discard, &AtomicBool::new(false))?;
            let (repo, _) = repo.main_worktree(Discard, &AtomicBool::new(false))?;
            debug!("Cloned repository ({})", url);
            repo
        };

        Ok(Git {
            rev: rev.map(|x| x.to_owned()),
            tag: tag.map(|x| x.to_owned()),
            branch: branch.map(|x| x.to_owned()),
            repo,
        })
    }

    pub fn fetch(&self) -> Result<(), MetadataError> {
        let remote = self.repo.find_default_remote(Direction::Fetch).unwrap()?;
        let connect = remote.connect(Direction::Fetch, Discard)?;
        let prepare = connect.prepare_fetch(Options::default())?;
        let _outcome = prepare.receive(&AtomicBool::new(false))?;

        debug!(
            "Fetched repository ({})",
            self.repo.work_dir().unwrap().to_string_lossy()
        );

        Ok(())
    }

    pub fn checkout(&self) -> Result<(), MetadataError> {
        let id = if let Some(ref rev) = self.rev {
            self.repo.rev_parse_single(rev.as_str())?
        } else {
            let dst = if let Some(ref tag) = self.tag {
                tag.to_string()
            } else if let Some(ref branch) = self.branch {
                format!("origin/{branch}")
            } else {
                "origin/HEAD".to_string()
            };

            let mut reference = self.repo.find_reference(&dst)?;
            reference.peel_to_id_in_place()?
        };

        let file = File::at(self.repo.index_path(), id.kind(), Default::default())?;
        let (mut state, _path) = file.into_parts();
        index::checkout(
            &mut state,
            self.repo.work_dir().unwrap(),
            {
                let objects = self.repo.objects.clone().into_arc()?;
                move |oid, buf| objects.find_blob(oid, buf)
            },
            &mut Discard,
            &mut Discard,
            &AtomicBool::new(false),
            Default::default(),
        )?;

        debug!(
            "Checkouted repository ({})",
            self.repo.work_dir().unwrap().to_string_lossy()
        );
        Ok(())
    }
}
