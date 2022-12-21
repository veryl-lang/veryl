use crate::utils;
use crate::Check;
use std::fs;
use std::time::Instant;
use thiserror::Error;
use veryl_analyzer::{AnalyzeError, Analyzer};
use veryl_parser::miette::{self, Diagnostic, IntoDiagnostic, Result, WrapErr};
use veryl_parser::Parser;

pub struct CmdCheck {
    opt: Check,
}

#[derive(Error, Diagnostic, Debug, Default)]
#[error("Check error")]
pub struct CheckError {
    #[related]
    related: Vec<AnalyzeError>,
}

impl CmdCheck {
    pub fn new(opt: Check) -> Self {
        Self { opt }
    }

    pub fn exec(&self) -> Result<bool> {
        let files = if self.opt.files.is_empty() {
            utils::gather_files("./")?
        } else {
            self.opt.files.clone()
        };

        let mut all_pass = true;
        let now = Instant::now();

        let mut check_error = CheckError::default();

        for file in &files {
            self.print(&format!(
                "[Info] Processing file: {}",
                file.to_string_lossy()
            ));

            let input = fs::read_to_string(file).into_diagnostic().wrap_err("")?;
            let parser = Parser::parse(&input, file)?;

            let mut analyzer = Analyzer::new(&input);
            analyzer.analyze(&parser.veryl);
            if !analyzer.errors.is_empty() {
                all_pass = false;

                for error in analyzer.errors {
                    check_error.related.push(error);
                }
            }
        }

        let elapsed_time = now.elapsed();
        self.print(&format!(
            "[Info] Elapsed time: {} milliseconds.",
            elapsed_time.as_millis()
        ));

        if check_error.related.is_empty() {
            Ok(all_pass)
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
