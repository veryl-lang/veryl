use crate::OptDoc;
use crate::doc::{DocBuilder, TopLevelItem};
use crate::pipeline::{self, AnalyzeOptions};
use miette::Result;
use std::collections::BTreeMap;
use veryl_analyzer::symbol::{SymbolId, SymbolKind};
use veryl_analyzer::symbol_table;
use veryl_metadata::Metadata;
use veryl_parser::resource_table;

pub struct CmdDoc {
    opt: OptDoc,
}

impl CmdDoc {
    pub fn new(opt: OptDoc) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, true)?;

        // Abort on a fatal error rather than document a half-analyzed tree.
        let options = AnalyzeOptions {
            defines: &[],
            emit_mode: false,
            incremental: false,
            fail_fast: true,
        };
        let _ = pipeline::analyze(metadata, &paths, options, None, None)?;

        let mut modules = BTreeMap::new();
        let mut proto_modules = BTreeMap::new();
        let mut interfaces = BTreeMap::new();
        let mut packages = BTreeMap::new();

        for symbol in veryl_analyzer::symbol_table::get_all() {
            let text = resource_table::get_str_value(symbol.token.text).unwrap();
            let file_name = text.clone();
            let symbol = symbol.clone();
            if format!("{}", symbol.namespace) == metadata.project.name && symbol.public {
                match &symbol.kind {
                    SymbolKind::Module(x) if !x.is_proto => {
                        let html_name = fmt_generic_parameters(&text, &x.generic_parameters);
                        let item = TopLevelItem {
                            file_name,
                            html_name,
                            symbol,
                        };
                        modules.insert(text, item);
                    }
                    SymbolKind::Module(x) if x.is_proto => {
                        let html_name = file_name.clone();
                        let item = TopLevelItem {
                            file_name,
                            html_name,
                            symbol,
                        };
                        proto_modules.insert(text, item);
                    }
                    SymbolKind::Interface(x) if !x.is_proto => {
                        let html_name = fmt_generic_parameters(&text, &x.generic_parameters);
                        let item = TopLevelItem {
                            file_name,
                            html_name,
                            symbol,
                        };
                        interfaces.insert(text, item);
                    }
                    SymbolKind::Package(x) if !x.is_proto => {
                        let html_name = fmt_generic_parameters(&text, &x.generic_parameters);
                        let item = TopLevelItem {
                            file_name,
                            html_name,
                            symbol,
                        };
                        packages.insert(text, item);
                    }
                    _ => (),
                }
            }
        }

        let modules: Vec<_> = modules.into_values().collect();
        let proto_modules: Vec<_> = proto_modules.into_values().collect();
        let interfaces: Vec<_> = interfaces.into_values().collect();
        let packages: Vec<_> = packages.into_values().collect();

        let builder = DocBuilder::new(metadata, modules, proto_modules, interfaces, packages)?;
        builder.build()?;

        Ok(true)
    }
}

fn fmt_generic_parameters(name: &str, params: &[SymbolId]) -> String {
    if params.is_empty() {
        name.to_string()
    } else {
        let mut name = name.to_string();
        name.push_str("::&lt;");
        for param in params {
            let symbol = symbol_table::get(*param).unwrap();
            name.push_str(&format!("{}, ", symbol.token.text));
        }
        format!("{}&gt;", name.strip_suffix(", ").unwrap())
    }
}
