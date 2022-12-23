use crate::utils;
use crate::OptBuild;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;
use veryl_config::{Config, Target};
use veryl_emitter::Emitter;
use veryl_parser::miette::{IntoDiagnostic, Result, WrapErr};
use veryl_parser::Parser;

pub struct CmdBuild {
    opt: OptBuild,
}

impl CmdBuild {
    pub fn new(opt: OptBuild) -> Self {
        Self { opt }
    }

    pub fn exec(&self, config: &Config) -> Result<bool> {
        let files = if self.opt.files.is_empty() {
            utils::gather_files("./")?
        } else {
            self.opt.files.clone()
        };

        let now = Instant::now();

        for file in &files {
            self.print(&format!(
                "[Info] Processing file: {}",
                file.to_string_lossy()
            ));

            let input = fs::read_to_string(file).into_diagnostic().wrap_err("")?;
            let parser = Parser::parse(&input, file)?;
            let mut emitter = Emitter::new(config);
            emitter.emit(&parser.veryl);

            let output = match config.build.target {
                Target::Source => file.with_extension("sv"),
                Target::Directory { ref path } => {
                    path.join(file.with_extension("sv").file_name().unwrap())
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

        Ok(true)
    }

    fn print(&self, msg: &str) {
        if self.opt.verbose {
            println!("{}", msg);
        }
    }
}
