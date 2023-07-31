use crate::analyzer_error::AnalyzerError;
use crate::handlers::*;
use crate::namespace_table;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use std::path::Path;
use veryl_metadata::{Lint, Metadata};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, VerylWalker};

pub struct AnalyzerPass1<'a> {
    handlers: Pass1Handlers<'a>,
}

impl<'a> AnalyzerPass1<'a> {
    pub fn new(text: &'a str, lint_opt: &'a Lint) -> Self {
        AnalyzerPass1 {
            handlers: Pass1Handlers::new(text, lint_opt),
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
    pub fn new(text: &'a str, lint_opt: &'a Lint) -> Self {
        AnalyzerPass2 {
            handlers: Pass2Handlers::new(text, lint_opt),
        }
    }
}

impl<'a> VerylWalker for AnalyzerPass2<'a> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct Analyzer {
    lint_opt: Lint,
}

impl Analyzer {
    pub fn new(metadata: &Metadata) -> Self {
        for locks in metadata.lockfile.lock_table.values() {
            for lock in locks {
                let prj = resource_table::insert_str(&lock.name);
                for lock_dep in &lock.dependencies {
                    let from = resource_table::insert_str(&lock_dep.name);
                    let to = metadata.lockfile.lock_table.get(&lock_dep.url).unwrap();
                    let to = to.iter().find(|x| x.version == lock_dep.version).unwrap();
                    let to = resource_table::insert_str(&to.name);
                    symbol_table::add_project_local(prj, from, to);
                }
            }
        }
        Analyzer {
            lint_opt: metadata.lint.clone(),
        }
    }

    pub fn analyze_pass1<T: AsRef<Path>>(
        &self,
        project_name: &str,
        text: &str,
        _path: T,
        input: &Veryl,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let mut pass1 = AnalyzerPass1::new(text, &self.lint_opt);
        pass1.veryl(input);
        ret.append(&mut pass1.handlers.get_errors());

        ret
    }

    pub fn analyze_pass2<T: AsRef<Path>>(
        &self,
        project_name: &str,
        text: &str,
        _path: T,
        input: &Veryl,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let mut pass2 = AnalyzerPass2::new(text, &self.lint_opt);
        pass2.veryl(input);
        ret.append(&mut pass2.handlers.get_errors());

        ret
    }

    pub fn analyze_pass3<T: AsRef<Path>>(
        &self,
        project_name: &str,
        text: &str,
        path: T,
        _input: &Veryl,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        ret.append(&mut Analyzer::check_symbol_table(path.as_ref(), text));

        ret
    }

    fn check_symbol_table(path: &Path, text: &str) -> Vec<AnalyzerError> {
        let path = resource_table::get_path_id(path.to_path_buf()).unwrap();
        let mut ret = Vec::new();
        let symbols = symbol_table::get_all();
        for symbol in symbols {
            if symbol.token.file_path == path {
                if let SymbolKind::Variable(_) = symbol.kind {
                    if symbol.references.is_empty() && !symbol.allow_unused {
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
