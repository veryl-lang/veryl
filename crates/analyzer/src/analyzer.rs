use crate::analyze_error::AnalyzeError;
use crate::handlers::*;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, VerylWalker};

pub struct Analyzer<'a> {
    pub errors: Vec<AnalyzeError>,
    handlers: Handlers<'a>,
}

impl<'a> Analyzer<'a> {
    pub fn new(text: &'a str) -> Self {
        Analyzer {
            errors: Vec::new(),
            handlers: Handlers::new(text),
        }
    }

    pub fn analyze(&mut self, input: &Veryl) {
        self.veryl(input);
        self.errors.append(&mut self.handlers.get_errors());
    }
}

impl<'a> VerylWalker for Analyzer<'a> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}
