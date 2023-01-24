use crate::analyzer_error::AnalyzerError;
use crate::handlers::*;
use crate::namespace_table;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use std::path::Path;
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, VerylWalker};

pub struct AnalyzerPass1<'a> {
    handlers: Pass1Handlers<'a>,
}

impl<'a> AnalyzerPass1<'a> {
    pub fn new(text: &'a str) -> Self {
        AnalyzerPass1 {
            handlers: Pass1Handlers::new(text),
        }
    }
}

impl<'a> VerylWalker for AnalyzerPass1<'a> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct AnalyzerPass2<'a> {
    handlers: Pass2Handlers<'a>,
}

impl<'a> AnalyzerPass2<'a> {
    pub fn new(text: &'a str) -> Self {
        AnalyzerPass2 {
            handlers: Pass2Handlers::new(text),
        }
    }
}

impl<'a> VerylWalker for AnalyzerPass2<'a> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct Analyzer<'a> {
    text: &'a str,
}

impl<'a> Analyzer<'a> {
    pub fn new<T: AsRef<str>>(text: &'a str, project_paths: &'a [T]) -> Self {
        let mut ids = Vec::new();
        for path in project_paths {
            ids.push(resource_table::insert_str(path.as_ref()));
        }
        namespace_table::set_default(&ids);
        Analyzer { text }
    }

    pub fn analyze_tree(&'a mut self, input: &Veryl) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        let mut pass1 = AnalyzerPass1::new(self.text);
        pass1.veryl(input);
        ret.append(&mut pass1.handlers.get_errors());

        let mut pass2 = AnalyzerPass2::new(self.text);
        pass2.veryl(input);
        ret.append(&mut pass2.handlers.get_errors());

        ret
    }

    pub fn analyze_post(path: &Path, text: &str) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();
        ret.append(&mut Analyzer::check_symbol_table(path, text));
        ret
    }

    fn check_symbol_table(path: &Path, text: &str) -> Vec<AnalyzerError> {
        let path = resource_table::get_path_id(path.to_path_buf()).unwrap();
        let mut ret = Vec::new();
        let symbols = symbol_table::get_all();
        for symbol in symbols {
            if symbol.token.file_path == path {
                if let SymbolKind::Variable(_) = symbol.kind {
                    if symbol.references.is_empty() {
                        let name = format!("{}", symbol.token.text);
                        if name.starts_with('_') {
                            continue;
                        }

                        let token = VerylToken {
                            token: symbol.token,
                            comments: Vec::new(),
                        };
                        ret.push(AnalyzerError::unused_variable(
                            &format!("{}", symbol.token.text),
                            text,
                            &token,
                        ));
                    }
                }
            }
        }
        ret
    }
}
