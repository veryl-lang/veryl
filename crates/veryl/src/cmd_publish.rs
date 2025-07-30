use crate::OptPublish;
use crate::cmd_check::CheckError;
use crate::context::Context;
use log::{info, warn};
use miette::{IntoDiagnostic, Result, WrapErr, bail};
use std::fs;
use veryl_analyzer::Analyzer;
use veryl_metadata::{LockSource, Metadata};
use veryl_parser::Parser;

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

        let mut check_error = CheckError::default();
        let mut contexts = Vec::new();

        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;

            let analyzer = Analyzer::new(metadata);
            let mut errors = analyzer.analyze_pass1(&path.prj, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;

            let context = Context::new(path.clone(), input, parser, analyzer)?;
            contexts.push(context);
        }

        let mut errors = Analyzer::analyze_post_pass1();
        check_error = check_error.append(&mut errors).check_err()?;

        for context in &contexts {
            let path = &context.path;
            let mut errors =
                context
                    .analyzer
                    .analyze_pass2(&path.prj, &path.src, &context.parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;
        }

        let info = Analyzer::analyze_post_pass2();

        for context in &contexts {
            let path = &context.path;
            let mut errors =
                context
                    .analyzer
                    .analyze_pass3(&path.prj, &path.src, &context.parser.veryl, &info);
            check_error = check_error.append(&mut errors).check_err()?;
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
