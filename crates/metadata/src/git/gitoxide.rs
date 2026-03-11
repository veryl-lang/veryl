use crate::metadata::UrlPath;
use crate::metadata_error::MetadataError;
use gix::bstr::ByteSlice;
use gix::objs::tree::EntryKind;
use log::debug;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use thiserror::Error;

pub struct Git {
    path: PathBuf,
}

#[derive(Error, Debug)]
pub enum GitoxideError {
    #[error("gitoxide error: {0}")]
    Open(#[from] gix::open::Error),

    #[error("gitoxide error: {0}")]
    Init(#[from] gix::init::Error),

    #[error("gitoxide error: {0}")]
    Clone(#[from] gix::clone::Error),

    #[error("gitoxide error: {0}")]
    CloneFetch(#[from] gix::clone::fetch::Error),

    #[error("gitoxide error: {0}")]
    CloneCheckoutMainWorktree(#[from] gix::clone::checkout::main_worktree::Error),

    #[error("gitoxide error: {0}")]
    RemoteFindExisting(#[from] gix::remote::find::existing::Error),

    #[error("gitoxide error: {0}")]
    RemoteConnect(#[from] gix::remote::connect::Error),

    #[error("gitoxide error: {0}")]
    RemoteFetchPrepare(#[from] gix::remote::fetch::prepare::Error),

    #[error("gitoxide error: {0}")]
    RemoteFetch(#[from] gix::remote::fetch::Error),

    #[error("gitoxide error: {0}")]
    RevisionSpecParseSingle(#[from] gix::revision::spec::parse::single::Error),

    #[error("gitoxide error: {0}")]
    HeadId(#[from] gix::reference::head_id::Error),

    #[error("gitoxide error: {0}")]
    HeadTreeId(#[from] gix::reference::head_tree_id::Error),

    #[error("gitoxide error: {0}")]
    OpenIndex(#[from] gix::worktree::open_index::Error),

    #[error("gitoxide error: {0}")]
    IndexFromTree(#[from] gix::repository::index_from_tree::Error),

    #[error("gitoxide error: {0}")]
    IndexWrite(#[from] gix::index::file::write::Error),

    #[error("gitoxide error: {0}")]
    WorktreeCheckout(#[from] gix::worktree::state::checkout::Error),

    #[error("gitoxide error: {0}")]
    ObjectFind(#[from] gix::object::find::existing::Error),

    #[error("gitoxide error: {0}")]
    Commit(#[from] gix::commit::Error),

    #[error("gitoxide error: {0}")]
    IsDirty(#[from] gix::status::is_dirty::Error),

    #[error("gitoxide error: {0}")]
    CheckoutOptions(#[from] gix::config::checkout_options::Error),

    #[error("gitoxide error: {0}")]
    PeelToCommit(#[from] gix::object::peel::to_kind::Error),

    #[error("gitoxide error: {0}")]
    ObjectDecode(#[from] gix::objs::decode::Error),

    #[error("gitoxide error: {0}")]
    TreeEditor(#[from] gix::object::tree::editor::write::Error),

    #[error("gitoxide error: {0}")]
    TreeEditorInit(#[from] gix::object::tree::editor::init::Error),

    #[error("gitoxide error: {0}")]
    TreeEditorEdit(#[from] gix::objs::tree::editor::Error),

    #[error("gitoxide error: {0}")]
    EditTree(#[from] gix::repository::edit_tree::Error),

    #[error("gitoxide error: {0}")]
    ObjectWrite(#[from] gix::object::write::Error),

    #[error("gitoxide error: {0}")]
    Io(#[from] std::io::Error),

    #[error("gitoxide error: {msg}")]
    Generic { msg: String },
}

impl From<GitoxideError> for MetadataError {
    fn from(x: GitoxideError) -> MetadataError {
        MetadataError::Git(Box::new(x))
    }
}

impl Git {
    pub fn exists() -> bool {
        true
    }

    pub fn init(path: &Path) -> Result<Self, MetadataError> {
        let ret = Git {
            path: path.to_path_buf(),
        };

        if !ret.is_git()? {
            gix::init(path).map_err(GitoxideError::from)?;
        }

        Ok(ret)
    }

    pub fn open(path: &Path) -> Result<Self, MetadataError> {
        Ok(Git {
            path: path.to_path_buf(),
        })
    }

    pub fn clone(url: &UrlPath, path: &Path) -> Result<Self, MetadataError> {
        if !path.exists() {
            let url_str = url.to_string();
            let mut prepare =
                gix::prepare_clone(url_str.as_str(), path).map_err(GitoxideError::from)?;
            let (mut checkout, _outcome) = prepare
                .fetch_then_checkout(gix::progress::Discard, &AtomicBool::new(false))
                .map_err(GitoxideError::from)?;
            checkout
                .main_worktree(gix::progress::Discard, &AtomicBool::new(false))
                .map_err(GitoxideError::from)?;
            debug!("Cloned repository ({url})");
        }

        Ok(Git {
            path: path.to_path_buf(),
        })
    }

    pub fn fetch(&self) -> Result<(), MetadataError> {
        let repo = gix::open(&self.path).map_err(GitoxideError::from)?;
        let remote = repo
            .find_default_remote(gix::remote::Direction::Fetch)
            .unwrap()
            .map_err(GitoxideError::from)?;
        let connection = remote
            .connect(gix::remote::Direction::Fetch)
            .map_err(GitoxideError::from)?;
        let prepare = connection
            .prepare_fetch(gix::progress::Discard, Default::default())
            .map_err(GitoxideError::from)?;
        let _outcome = prepare
            .receive(gix::progress::Discard, &AtomicBool::new(false))
            .map_err(GitoxideError::from)?;

        debug!("Fetched repository ({})", self.path.to_string_lossy());

        Ok(())
    }

    pub fn checkout(&self, rev: Option<&str>) -> Result<(), MetadataError> {
        let repo = gix::open(&self.path).map_err(GitoxideError::from)?;

        let dst = if let Some(rev) = rev {
            rev.to_string()
        } else {
            "origin/HEAD".to_string()
        };

        // Resolve the revision to a commit id
        let commit_id = repo
            .rev_parse_single(dst.as_str())
            .map_err(GitoxideError::from)?;
        let commit = commit_id
            .object()
            .map_err(GitoxideError::from)?
            .peel_to_commit()
            .map_err(GitoxideError::from)?;
        let tree_id = commit.tree_id().map_err(GitoxideError::from)?;

        // Create index from the tree
        let mut index = repo
            .index_from_tree(&tree_id)
            .map_err(GitoxideError::from)?;

        let workdir = repo.workdir().ok_or_else(|| GitoxideError::Generic {
            msg: "repository is bare".to_string(),
        })?;

        // Remove files from the working tree that are not in the new index.
        // gix::worktree::state::checkout only creates/updates files but does
        // not remove files from a previous checkout, unlike `git checkout`.
        if let Ok(old_index) = repo.open_index() {
            let new_paths: std::collections::HashSet<&gix::bstr::BStr> =
                index.entries().iter().map(|e| e.path(&index)).collect();
            for entry in old_index.entries() {
                let path = entry.path(&old_index);
                if !new_paths.contains(path) {
                    let file_path = workdir.join(gix::path::from_bstr(path));
                    let _ = std::fs::remove_file(&file_path);
                }
            }
        }

        let opts = repo
            .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
            .map_err(GitoxideError::from)?;

        gix::worktree::state::checkout(
            &mut index,
            workdir,
            repo.objects
                .clone()
                .into_arc()
                .map_err(|e| GitoxideError::Generic {
                    msg: format!("failed to convert objects to arc: {e}"),
                })?,
            &gix::progress::Discard,
            &gix::progress::Discard,
            &AtomicBool::new(false),
            opts,
        )
        .map_err(GitoxideError::from)?;

        index
            .write(Default::default())
            .map_err(GitoxideError::from)?;

        debug!(
            "Checkouted repository ({} @ {})",
            self.path.to_string_lossy(),
            dst
        );

        Ok(())
    }

    pub fn get_revision(&self) -> Result<String, MetadataError> {
        let repo = gix::open(&self.path).map_err(GitoxideError::from)?;
        let head_id = repo.head_id().map_err(GitoxideError::from)?;
        Ok(head_id.to_hex().to_string())
    }

    pub fn is_git(&self) -> Result<bool, MetadataError> {
        Ok(gix::open(&self.path).is_ok())
    }

    pub fn is_clean(&self) -> Result<bool, MetadataError> {
        let repo = gix::open(&self.path).map_err(GitoxideError::from)?;

        // Use status with untracked files included (like `git status -s`)
        let platform = match repo.status(gix::progress::Discard) {
            Ok(p) => p,
            Err(_) => return Ok(true),
        };
        let has_changes = match platform
            .untracked_files(gix::status::UntrackedFiles::Files)
            .into_index_worktree_iter(Vec::new())
        {
            Ok(mut iter) => iter.any(|item| item.is_ok()),
            Err(_) => false,
        };

        Ok(!has_changes)
    }

    pub fn add(&self, file: &Path) -> Result<(), MetadataError> {
        let repo = gix::open(&self.path).map_err(GitoxideError::from)?;
        let mut index = open_or_create_index(&repo)?;

        let workdir = repo.workdir().ok_or_else(|| GitoxideError::Generic {
            msg: "repository is bare".to_string(),
        })?;

        // Get the relative path from workdir
        let abs_file = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.path.join(file)
        };
        let rel_path = abs_file.strip_prefix(workdir).unwrap_or(file);

        let rel_path_bstr: &gix::bstr::BStr = path_to_bstr(rel_path);

        // Read the file, write it as blob to the ODB
        let file_content = std::fs::read(&abs_file).map_err(GitoxideError::from)?;
        let blob_id = repo
            .write_blob(&file_content)
            .map_err(GitoxideError::from)?;

        // Get file metadata for stat
        let file_meta = std::fs::metadata(&abs_file).map_err(GitoxideError::from)?;
        let stat = stat_from_metadata(&file_meta);
        let mode = if is_executable(&file_meta) {
            gix::index::entry::Mode::FILE_EXECUTABLE
        } else {
            gix::index::entry::Mode::FILE
        };

        // Check if the entry already exists; if so, update it
        if let Some(idx) = index
            .entry_index_by_path_and_stage(rel_path_bstr, gix::index::entry::Stage::Unconflicted)
        {
            let entries = index.entries_mut();
            entries[idx].id = blob_id.into();
            entries[idx].stat = stat;
            entries[idx].mode = mode;
        } else {
            index.dangerously_push_entry(
                stat,
                blob_id.into(),
                gix::index::entry::Flags::empty(),
                mode,
                rel_path_bstr,
            );
            index.sort_entries();
        }

        index
            .write(Default::default())
            .map_err(GitoxideError::from)?;

        Ok(())
    }

    pub fn commit(&self, msg: &str) -> Result<(), MetadataError> {
        let repo = gix::open(&self.path).map_err(GitoxideError::from)?;

        // Build tree from the current index using tree editor
        let index = open_or_create_index(&repo)?;
        let mut editor = repo
            .edit_tree(gix::ObjectId::empty_tree(repo.object_hash()))
            .map_err(GitoxideError::from)?;

        for entry in index.entries() {
            let path = entry.path(&index);
            let entry_kind = index_mode_to_entry_kind(entry.mode);
            editor
                .upsert(path, entry_kind, entry.id)
                .map_err(GitoxideError::from)?;
        }

        let tree_id = editor.write().map_err(GitoxideError::from)?;

        // Get parents
        let parents: Vec<gix::ObjectId> = match repo.head_id() {
            Ok(id) => vec![id.detach()],
            Err(_) => vec![], // Initial commit, no parents
        };

        repo.commit("HEAD", msg, tree_id, parents)
            .map_err(GitoxideError::from)?;

        Ok(())
    }
}

fn open_or_create_index(repo: &gix::Repository) -> Result<gix::index::File, MetadataError> {
    match repo.open_index() {
        Ok(index) => Ok(index),
        Err(_) => {
            // Index doesn't exist (e.g. fresh repo), create an empty one
            Ok(gix::index::File::from_state(
                gix::index::State::new(repo.object_hash()),
                repo.index_path(),
            ))
        }
    }
}

fn path_to_bstr(path: &Path) -> &gix::bstr::BStr {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().as_bstr()
    }
    #[cfg(not(unix))]
    {
        // On Windows, paths might contain non-UTF8 but this is a reasonable fallback
        let s = path.to_string_lossy();
        // We need to return a reference, so leak the string for now
        // This is acceptable since this is only used in short-lived operations
        let leaked: &'static str = Box::leak(s.into_owned().into_boxed_str());
        leaked.as_bytes().as_bstr()
    }
}

fn index_mode_to_entry_kind(mode: gix::index::entry::Mode) -> EntryKind {
    if mode == gix::index::entry::Mode::FILE {
        EntryKind::Blob
    } else if mode == gix::index::entry::Mode::FILE_EXECUTABLE {
        EntryKind::BlobExecutable
    } else if mode == gix::index::entry::Mode::SYMLINK {
        EntryKind::Link
    } else if mode == gix::index::entry::Mode::DIR {
        EntryKind::Tree
    } else if mode == gix::index::entry::Mode::COMMIT {
        EntryKind::Commit
    } else {
        EntryKind::Blob
    }
}

fn stat_from_metadata(meta: &std::fs::Metadata) -> gix::index::entry::Stat {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        gix::index::entry::Stat {
            mtime: gix::index::entry::stat::Time {
                secs: meta.mtime() as u32,
                nsecs: meta.mtime_nsec() as u32,
            },
            ctime: gix::index::entry::stat::Time {
                secs: meta.ctime() as u32,
                nsecs: meta.ctime_nsec() as u32,
            },
            dev: meta.dev() as u32,
            ino: meta.ino() as u32,
            uid: meta.uid(),
            gid: meta.gid(),
            size: meta.size() as u32,
        }
    }
    #[cfg(not(unix))]
    {
        gix::index::entry::Stat {
            mtime: gix::index::entry::stat::Time { secs: 0, nsecs: 0 },
            ctime: gix::index::entry::stat::Time { secs: 0, nsecs: 0 },
            dev: 0,
            ino: 0,
            uid: 0,
            gid: 0,
            size: meta.len() as u32,
        }
    }
}

fn is_executable(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}
