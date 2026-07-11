use crate::OptPublish;
use crate::pipeline::{self, AnalyzeOptions, AnalyzeOutput};
use log::warn;
use miette::{IntoDiagnostic, Result, bail};
use veryl_metadata::{Git, LockSource, Metadata};

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

        // Prebuilt component wasm and committed interface manifests are part
        // of the published tree; stale ones are regenerated here. With
        // `bump_commit` they are committed (before the version bump, so the
        // bump stays HEAD) and publish continues; otherwise they are left for
        // the user and the single gate below stops before publish.
        let mut component_paths = crate::component_publish::update_prebuilt_wasm(metadata)?;
        component_paths.extend(crate::component_publish::update_committed_manifests(
            metadata,
        )?);
        if !component_paths.is_empty() && metadata.publish.bump_commit {
            let git = Git::open(&metadata.project_path()).into_diagnostic()?;
            for path in &component_paths {
                git.add(path).into_diagnostic()?;
            }
            git.commit("chore: Update prebuilt component artifacts")
                .into_diagnostic()?;
        }

        // Runs even when the wasm was stale, so `--bump` is never silently
        // skipped. `bump_version` commits Veryl.toml itself under `bump_commit`.
        if let Some(kind) = self.opt.bump {
            metadata.bump_version(kind.into()).into_diagnostic()?;
        }

        // Single pre-publish gate: without auto-commit the release artifacts
        // stay uncommitted, so stop and let the user commit them. The version
        // is already written, so the re-run must omit `--bump` to avoid a
        // second bump.
        if !metadata.publish.bump_commit && (!component_paths.is_empty() || self.opt.bump.is_some())
        {
            if !component_paths.is_empty() {
                warn!("Please git add and commit the updated component artifacts");
            }
            if self.opt.bump.is_some() {
                warn!(
                    "Please git add and commit Veryl.toml, then re-run `veryl publish` without --bump"
                );
            }
            return Ok(true);
        }

        metadata.publish()?;

        crate::cmd_register::maybe_register(metadata);

        Ok(true)
    }
}
