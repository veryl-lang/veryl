use crate::OptDump;
use log::{debug, info};
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

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let now = Instant::now();

        let paths = metadata.paths(&self.opt.files)?;

        let mut contexts = Vec::new();

        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;
            let analyzer = Analyzer::new(&path.prj, metadata);
            analyzer.analyze_pass1(&input, &path.src, &parser.veryl);

            contexts.push((path, input, parser, analyzer));
        }

        for (path, input, parser, analyzer) in &contexts {
            analyzer.analyze_pass2(input, &path.src, &parser.veryl);
        }

        for (path, input, parser, analyzer) in &contexts {
            analyzer.analyze_pass3(input, &path.src, &parser.veryl);
        }

        if self.opt.symbol_table {
            println!("{}", veryl_analyzer::symbol_table::dump());
        }

        if self.opt.namespace_table {
            println!("{}", veryl_analyzer::namespace_table::dump());
        }

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());

        Ok(true)
    }
}
