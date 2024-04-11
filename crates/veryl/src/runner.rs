use log::{debug, log_enabled, Level};
use miette::Result;
use regex::Regex;
use std::process::Output;
use veryl_metadata::Metadata;
use veryl_parser::resource_table::StrId;

mod vcs;
mod verilator;
mod vivado;
pub use vcs::*;
pub use verilator::*;
pub use vivado::*;

#[derive(Default)]
pub struct LogRegex {
    warning: Option<&'static Regex>,
    error: Option<&'static Regex>,
    fatal: Option<&'static Regex>,
}

pub trait Runner {
    fn run(&mut self, metadata: &Metadata, test: StrId) -> Result<bool>;

    fn name(&self) -> &'static str;

    fn regex_compile(&self) -> LogRegex {
        LogRegex::default()
    }

    fn regex_elaborate(&self) -> LogRegex {
        LogRegex::default()
    }

    fn regex_simulate(&self) -> LogRegex {
        LogRegex::default()
    }

    fn parse_compile(&self, output: Output) -> bool {
        self.parse(output, self.regex_compile())
    }

    fn parse_elaborate(&self, output: Output) -> bool {
        self.parse(output, self.regex_elaborate())
    }

    fn parse_simulate(&self, output: Output) -> bool {
        self.parse(output, self.regex_simulate())
    }

    fn parse(&self, output: Output, regex: LogRegex) -> bool {
        let stdout = String::from_utf8_lossy(output.stdout.as_slice());
        let stderr = String::from_utf8_lossy(output.stderr.as_slice());
        let text = format!("{}{}", stdout, stderr);

        let mut warnings = if let Some(x) = regex.warning {
            x.find_iter(&text)
                .map(|x| (x.start(), x.as_str().to_string()))
                .collect()
        } else {
            vec![]
        };

        let mut errors = if let Some(x) = regex.error {
            x.find_iter(&text)
                .map(|x| (x.start(), x.as_str().to_string()))
                .collect()
        } else {
            vec![]
        };

        let mut fatals = if let Some(x) = regex.fatal {
            x.find_iter(&text)
                .map(|x| (x.start(), x.as_str().to_string()))
                .collect()
        } else {
            vec![]
        };

        let success = errors.is_empty() && fatals.is_empty();

        if log_enabled!(Level::Debug) {
            for line in text.lines() {
                debug!("{} : {}", self.name(), line);
            }
        } else {
            let mut all = Vec::new();
            all.append(&mut warnings);
            all.append(&mut errors);
            all.append(&mut fatals);
            all.sort_by(|x, y| x.0.cmp(&y.0));
            for (_, log) in &all {
                println!("{}", log);
            }
        }

        success
    }
}
