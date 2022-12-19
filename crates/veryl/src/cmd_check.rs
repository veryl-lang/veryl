use crate::utils;
use crate::Check;
use miette::{Diagnostic, IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::time::Instant;
use thiserror::Error;
use veryl_parser::parser::Parser;
use veryl_parser::veryl_error::VerylError;

pub struct CmdCheck {
    opt: Check,
}

#[derive(Error, Diagnostic, Debug, Default)]
#[error("Check error")]
pub struct CheckError {
    #[related]
    related: Vec<VerylError>,
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
            if !parser.errors.is_empty() {
                all_pass = false;

                for error in parser.errors {
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
