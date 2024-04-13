use crate::runner::{LogRegex, Runner};
use log::{error, info, warn};
use miette::{IntoDiagnostic, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::process::Command;
use veryl_metadata::Metadata;
use veryl_parser::resource_table::StrId;

pub struct Vivado;

impl Runner for Vivado {
    fn run(&mut self, metadata: &Metadata, test: StrId) -> Result<bool> {
        let temp_dir = tempfile::tempdir().into_diagnostic()?;

        info!("Compiling test ({})", test);

        let define = format!("__veryl_test_{}_{}__", metadata.project.name, test);

        let compile = Command::new("xvlog")
            .arg("--sv")
            .arg("-f")
            .arg(metadata.filelist_path())
            .arg("-d")
            .arg(&define)
            .args(&metadata.test.vivado.compile_args)
            .current_dir(temp_dir.path())
            .output()
            .into_diagnostic()?;

        if !self.parse_compile(compile) {
            error!("Failed compile ({})", test);
            return Ok(false);
        }

        info!("Elaborating test ({})", test);

        let elaborate = Command::new("xelab")
            .arg(&test.to_string())
            .args(&metadata.test.vivado.elaborate_args)
            .current_dir(temp_dir.path())
            .output()
            .into_diagnostic()?;

        if !self.parse_elaborate(elaborate) {
            error!("Failed elaborate ({})", test);
            return Ok(false);
        }

        info!("Executing test ({})", test);

        let simulate = Command::new("xsim")
            .arg(&format!("work.{}", test))
            .arg("--runall")
            .args(&metadata.test.vivado.simulate_args)
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
        "Vivado"
    }

    fn regex_compile(&self) -> LogRegex {
        static WARNING: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m:^WARNING:.*$\n)").unwrap());
        static ERROR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m:^ERROR:.*$\n)").unwrap());

        LogRegex {
            warning: Some(&WARNING),
            error: Some(&ERROR),
            fatal: None,
        }
    }

    fn regex_elaborate(&self) -> LogRegex {
        static WARNING: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m:^WARNING:.*$\n)").unwrap());
        static ERROR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m:^ERROR:.*$\n)").unwrap());

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
