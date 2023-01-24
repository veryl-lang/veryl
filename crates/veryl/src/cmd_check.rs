use crate::OptCheck;
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
        let paths = metadata.paths(&self.opt.files)?;

        let now = Instant::now();

        let mut check_error = CheckError::default();
        let mut contexts = Vec::new();

        for path in &paths {
            self.print(&format!(
                "[Info] Processing file: {}",
                path.src.to_string_lossy()
            ));

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;

            let mut analyzer = Analyzer::new(&input, &path.prj);
            let errors = analyzer.analyze_tree(&parser.veryl);
            for error in errors {
                check_error.related.push(error);
            }

            contexts.push((path, input));
        }

        for (path, input) in &contexts {
            let errors = Analyzer::analyze_post(&path.src, input);
            for error in errors {
                check_error.related.push(error);
            }
        }

        let elapsed_time = now.elapsed();
        self.print(&format!(
            "[Info] Elapsed time: {} milliseconds.",
            elapsed_time.as_millis()
        ));

        if check_error.related.is_empty() {
            Ok(true)
        } else {
            Err(check_error.into())
        }
    }

    fn print(&self, msg: &str) {
        if self.opt.verbose {
            println!("{}", msg);
        }
    }
}
