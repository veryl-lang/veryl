use crate::runner::{LogRegex, Runner};
use log::{error, info, warn};
use miette::{IntoDiagnostic, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::process::Command;
use veryl_metadata::Metadata;
use veryl_parser::resource_table::StrId;

pub struct Vcs;

impl Runner for Vcs {
    fn run(&mut self, metadata: &Metadata, test: StrId) -> Result<bool> {
        let temp_dir = tempfile::tempdir().into_diagnostic()?;

        info!("Compiling test ({})", test);

        let define = format!("+define+__veryl_test_{}_{}__", metadata.project.name, test);

        let compile = Command::new("vcs")
            .arg("-sverilog")
            .arg("-f")
            .arg(metadata.filelist_path())
            .arg(&define)
            .args(&metadata.test.vcs.compile_args)
            .current_dir(temp_dir.path())
            .output()
            .into_diagnostic()?;

        if !self.parse_compile(compile) {
            error!("Failed compile ({})", test);
            return Ok(false);
        }

        info!("Executing test ({})", test);

        let simulate = Command::new("./simv")
            .args(&metadata.test.vcs.simulate_args)
            .current_dir(temp_dir.path())
            .output()
            .into_diagnostic()?;

        if !self.parse_simulate(simulate) {
            warn!("Failed test ({})", test);
            Ok(false)
        } else {
            info!("Succeeded test ({})", test);
            Ok(true)
        }
    }

    fn name(&self) -> &'static str {
        "VCS"
    }

    fn regex_compile(&self) -> LogRegex {
        static WARNING: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"(?m:^Warning-.*$\n(^.+$\n)*)").unwrap());
        static ERROR: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"(?m:^Error-.*$\n(^.+$\n)*)").unwrap());

        LogRegex {
            warning: Some(&WARNING),
            error: Some(&ERROR),
            fatal: None,
        }
    }

    fn regex_simulate(&self) -> LogRegex {
        static WARNING: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"(?m:^Warning: .*$\n^.*$\n)").unwrap());
        static ERROR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m:^Error: .*$\n^.*$\n)").unwrap());
        static FATAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m:^Fatal: .*$\n^.*$\n)").unwrap());

        LogRegex {
            warning: Some(&WARNING),
            error: Some(&ERROR),
            fatal: Some(&FATAL),
        }
    }
}
