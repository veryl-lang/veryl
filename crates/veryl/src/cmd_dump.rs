use crate::utils::{self, PathPair};
use crate::OptDump;
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::time::Instant;
use veryl_analyzer::Analyzer;
use veryl_metadata::Metadata;
use veryl_parser::Parser;

pub struct CmdDump {
    opt: OptDump,
}

impl CmdDump {
    pub fn new(opt: OptDump) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &Metadata, deps: &[PathPair]) -> Result<bool> {
        let mut files = utils::gather_files(&self.opt.files, metadata)?;
        for dep in deps {
            files.push(dep.clone());
        }

        let now = Instant::now();

        for file in &files {
            self.print(&format!(
                "[Info] Processing file: {}",
                file.src.to_string_lossy()
            ));

            let input = fs::read_to_string(&file.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &file.src)?;
            let mut analyzer = Analyzer::new(&input, &file.prj);
            analyzer.analyze_tree(&parser.veryl);
        }

        if self.opt.symbol_table {
            println!("{}", veryl_analyzer::symbol_table::dump());
        }

        if self.opt.namespace_table {
            println!("{}", veryl_analyzer::namespace_table::dump());
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
