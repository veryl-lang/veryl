use crate::OptFmt;
use crate::diff::print_diff;
use crate::utils;
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;
use veryl_analyzer::Analyzer;
use veryl_formatter::Formatter;
use veryl_metadata::Metadata;
use veryl_parser::Parser;

pub struct CmdFmt {
    opt: OptFmt,
}

impl CmdFmt {
    pub fn new(opt: OptFmt) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata, quiet: bool) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, false)?;

        let mut all_pass = true;
        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;
            let analyzer = Analyzer::new(metadata);
            let _ = analyzer.analyze_pass1(&path.prj, &parser.veryl);

            let mut formatter = Formatter::new(metadata);
            formatter.format(&parser.veryl);

            let pass = input.as_str() == formatter.as_str();

            if !pass {
                if self.opt.check {
                    if !quiet {
                        print_diff(&path.src, input.as_str(), formatter.as_str());
                    }
                    all_pass = false;
                } else {
                    let written =
                        utils::write_file_if_changed(&path.src, formatter.as_str().as_bytes())?;
                    if written {
                        debug!("Overwritten file ({})", path.src.to_string_lossy());
                    }
                }
            }
        }

        Ok(all_pass)
    }
}
