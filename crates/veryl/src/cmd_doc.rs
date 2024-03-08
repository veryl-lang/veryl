use crate::doc_builder::DocBuilder;
use crate::OptDoc;
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::BTreeMap;
use std::fs;
use std::time::Instant;
use veryl_analyzer::symbol::SymbolKind;
use veryl_analyzer::Analyzer;
use veryl_metadata::Metadata;
use veryl_parser::resource_table;
use veryl_parser::Parser;

pub struct CmdDoc {
    opt: OptDoc,
}

impl CmdDoc {
    pub fn new(opt: OptDoc) -> Self {
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
            let analyzer = Analyzer::new(metadata);
            analyzer.analyze_pass1(&path.prj, &input, &path.src, &parser.veryl);

            contexts.push((path, input, parser, analyzer));
        }

        for (path, input, parser, analyzer) in &contexts {
            analyzer.analyze_pass2(&path.prj, input, &path.src, &parser.veryl);
        }

        for (path, input, parser, analyzer) in &contexts {
            analyzer.analyze_pass3(&path.prj, input, &path.src, &parser.veryl);
        }

        let mut modules = BTreeMap::new();
        let mut interfaces = BTreeMap::new();
        let mut packages = BTreeMap::new();

        for symbol in veryl_analyzer::symbol_table::get_all() {
            let text = resource_table::get_str_value(symbol.token.text).unwrap();
            if format!("{}", symbol.namespace) == metadata.project.name && symbol.public {
                match symbol.kind {
                    SymbolKind::Module(_) => {
                        modules.insert(text, symbol.clone());
                    }
                    SymbolKind::Interface(_) => {
                        interfaces.insert(text, symbol.clone());
                    }
                    SymbolKind::Package(_) => {
                        packages.insert(text, symbol.clone());
                    }
                    _ => (),
                }
            }
        }

        let builder = DocBuilder::new(metadata, modules, interfaces, packages)?;
        builder.build()?;

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());

        Ok(true)
    }
}
