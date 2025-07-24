use crate::OptMigrate;
use crate::diff::print_diff;
use crate::utils;
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;
use veryl_analyzer::Analyzer;
use veryl_formatter::Formatter;
use veryl_metadata::Metadata;
use veryl_migrator::Migrator;
use veryl_migrator::Parser as OldParser;
use veryl_parser::Parser;

pub struct CmdMigrate {
    opt: OptMigrate,
}

impl CmdMigrate {
    pub fn new(opt: OptMigrate) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata, quiet: bool) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, true)?;

        let mut all_pass = true;
        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;

            // Check whether new parser is passed
            let parser = Parser::parse(&input, &path.src);

            if parser.is_err() {
                let parser = OldParser::parse(&input, &path.src)?;
                let mut migrator = Migrator::new(metadata);
                migrator.migrate(&parser.veryl);

                let parser = Parser::parse(migrator.as_str(), &path.src)?;
                let analyzer = Analyzer::new(metadata);
                let _ = analyzer.analyze_pass1(&path.prj, &path.src, &parser.veryl);

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
        }

        Ok(all_pass)
    }
}
