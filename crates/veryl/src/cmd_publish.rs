use crate::cmd_check::CheckError;
use crate::OptPublish;
use log::{info, warn};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::fs;
use veryl_analyzer::Analyzer;
use veryl_metadata::{Metadata, UrlPath};
use veryl_parser::Parser;

pub struct CmdPublish {
    opt: OptPublish,
}

impl CmdPublish {
    pub fn new(opt: OptPublish) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths::<&str>(&[], false)?;
        let paths_symlink = metadata.paths::<&str>(&[], true)?;

        for path in &paths_symlink {
            if paths.iter().all(|x| x.src != path.src) {
                bail!(
                    "path \"{}\" is symbolic link, it can't be published",
                    path.src.to_string_lossy()
                );
            }
        }

        for url in metadata.dependencies.keys() {
            if let UrlPath::Path(x) = url {
                bail!(
                    "path dependency \"{}\" is used, it can't be published",
                    x.to_string_lossy()
                );
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
            let mut errors = analyzer.analyze_pass1(&path.prj, &input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;

            contexts.push((path, input, parser, analyzer));
        }

        Analyzer::analyze_post_pass1();

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass2(&path.prj, input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;
        }

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass3(&path.prj, input, &path.src, &parser.veryl);
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
