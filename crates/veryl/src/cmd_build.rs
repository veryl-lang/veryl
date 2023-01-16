use crate::cmd_check::CheckError;
use crate::utils;
use crate::OptBuild;
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;
use veryl_analyzer::Analyzer;
use veryl_emitter::Emitter;
use veryl_metadata::{Metadata, Target};
use veryl_parser::Parser;

pub struct CmdBuild {
    opt: OptBuild,
}

impl CmdBuild {
    pub fn new(opt: OptBuild) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &Metadata) -> Result<bool> {
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
            let errors = analyzer.analyze(&parser.veryl);
            if !errors.is_empty() {
                all_pass = false;

                for error in errors {
                    check_error.related.push(error);
                }
            }

            let mut emitter = Emitter::new(metadata);
            emitter.emit(&parser.veryl);

            let output = match metadata.build.target {
                Target::Source => file.with_extension("sv"),
                Target::Directory { ref path } => {
                    let base = metadata.metadata_path.parent().unwrap().to_owned();
                    base.join(path.join(file.with_extension("sv").file_name().unwrap()))
                }
            };

            self.print(&format!("[Info] Output file: {}", output.to_string_lossy()));
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(output)
                .into_diagnostic()?;
            file.write_all(emitter.as_str().as_bytes())
                .into_diagnostic()?;
            file.flush().into_diagnostic()?;
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
