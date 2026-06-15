use crate::OptPublish;
use crate::pipeline::{self, AnalyzeOptions, AnalyzeOutput};
use log::warn;
use miette::{IntoDiagnostic, Result, bail};
use veryl_metadata::{LockSource, Metadata};

pub struct CmdPublish {
    opt: OptPublish,
}

impl CmdPublish {
    pub fn new(opt: OptPublish) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths::<&str>(&[], false, true)?;
        let paths_symlink = metadata.paths::<&str>(&[], true, true)?;

        for path in &paths_symlink {
            if paths.iter().all(|x| x.src != path.src) {
                bail!(
                    "path \"{}\" is symbolic link, it can't be published",
                    path.src.to_string_lossy()
                );
            }
        }

        for locks in metadata.lockfile.lock_table.values() {
            for lock in locks {
                if let LockSource::Path(x) = &lock.source {
                    bail!(
                        "path dependency \"{}\" is used, it can't be published",
                        x.to_string_lossy()
                    );
                }
            }
        }

        // Same pipeline as check (no emit, fails on warnings). The symlink guard
        // above keeps `paths` identical to build/check, so the cache is shared.
        let options = AnalyzeOptions {
            defines: &[],
            emit_mode: false,
            incremental: true,
            fail_fast: true,
        };
        let AnalyzeOutput {
            incremental,
            check_error,
            ..
        } = pipeline::analyze(metadata, &paths, options, None, None)?;
        if let Some(mut inc) = incremental {
            inc.save(&pipeline::collect_diagnosed(&check_error));
        }
        let _ = check_error.check_all()?;

        if let Some(kind) = self.opt.bump {
            metadata.bump_version(kind.into()).into_diagnostic()?;
            if !metadata.publish.bump_commit {
                warn!("Please git add and commit: Veryl.toml");
                return Ok(true);
            }
        }

        metadata.publish()?;

        Ok(true)
    }
}
