use crate::OptCheck;
use log::{debug, info};
use miette::{self, Diagnostic, IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::time::Instant;
use thiserror::Error;
use veryl_analyzer::{Analyzer, AnalyzerError};
use veryl_metadata::Metadata;
use veryl_parser::Parser;

pub struct CmdCheck {
    opt: OptCheck,
}

#[derive(Error, Diagnostic, Debug, Default)]
#[error("Check error")]
pub struct CheckError {
    #[related]
    pub related: Vec<AnalyzerError>,
}

impl CmdCheck {
    pub fn new(opt: OptCheck) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &Metadata) -> Result<bool> {
        let now = Instant::now();

        let paths = metadata.paths(&self.opt.files, false)?;

        let mut check_error = CheckError::default();
        let mut contexts = Vec::new();

        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;

            let analyzer = Analyzer::new(&path.prj);
            let mut errors = analyzer.analyze_pass1(&input, &path.src, &parser.veryl);
            check_error.related.append(&mut errors);

            contexts.push((path, input, parser, analyzer));
        }

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass2(&input, &path.src, &parser.veryl);
            check_error.related.append(&mut errors);
        }

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass3(&input, &path.src, &parser.veryl);
            check_error.related.append(&mut errors);
        }

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());

        if check_error.related.is_empty() {
            Ok(true)
        } else {
            Err(check_error.into())
        }
    }
}
