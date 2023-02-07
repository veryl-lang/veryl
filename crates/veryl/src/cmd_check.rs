use crate::OptCheck;
use log::{debug, info};
use miette::{self, Diagnostic, IntoDiagnostic, Result, Severity, WrapErr};
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
#[error("veryl check failed")]
pub struct CheckError {
    #[related]
    pub related: Vec<AnalyzerError>,
}

impl CheckError {
    pub fn append(mut self, x: &mut Vec<AnalyzerError>) -> Self {
        self.related.append(x);
        self
    }

    pub fn check_err(self) -> Result<Self> {
        if self
            .related
            .iter()
            .all(|x| !matches!(x.severity(), Some(Severity::Error) | None))
        {
            Ok(self)
        } else {
            Err(self.into())
        }
    }

    pub fn check_all(self) -> Result<Self> {
        if self.related.is_empty() {
            Ok(self)
        } else {
            Err(self.into())
        }
    }
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

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());

        let _ = check_error.check_all()?;
        Ok(true)
    }
}
