use crate::cmd_check::CheckError;
use crate::OptPublish;
use log::{debug, info, warn};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::time::Instant;
use veryl_analyzer::Analyzer;
use veryl_metadata::Metadata;
use veryl_parser::Parser;

pub struct CmdPublish {
    opt: OptPublish,
}

impl CmdPublish {
    pub fn new(opt: OptPublish) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let now = Instant::now();

        let paths = metadata.paths::<&str>(&[])?;

        let mut check_error = CheckError::default();
        let mut contexts = Vec::new();

        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;

            let analyzer = Analyzer::new(&path.prj, metadata);
            let mut errors = analyzer.analyze_pass1(&input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;

            contexts.push((path, input, parser, analyzer));
        }

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass2(input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;
        }

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass3(input, &path.src, &parser.veryl);
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

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());
        Ok(true)
    }
}
