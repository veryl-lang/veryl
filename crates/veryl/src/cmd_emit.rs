use crate::utils;
use crate::Emit;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;
use veryl_emitter::Emitter;
use veryl_parser::miette::{IntoDiagnostic, Result, WrapErr};
use veryl_parser::Parser;

pub struct CmdEmit {
    opt: Emit,
}

impl CmdEmit {
    pub fn new(opt: Emit) -> Self {
        Self { opt }
    }

    pub fn exec(&self) -> Result<bool> {
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
            let mut emitter = Emitter::new();
            emitter.emit(&parser.veryl);

            let output = if let Some(ref dir) = self.opt.target_directory {
                dir.join(file.with_extension("sv").file_name().unwrap())
            } else {
                file.with_extension("sv")
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
