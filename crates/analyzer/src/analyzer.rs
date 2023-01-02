use crate::analyze_error::AnalyzeError;
use crate::handlers::*;
use veryl_parser::veryl_grammar_trait::*;
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
    pub fn new(text: &'a str) -> Self {
        Analyzer { text }
    }

    pub fn analyze(&'a mut self, input: &Veryl) -> Vec<AnalyzeError> {
        let mut ret = Vec::new();

        let mut pass1 = AnalyzerPass1::new(self.text);
        pass1.veryl(input);
        ret.append(&mut pass1.handlers.get_errors());

        let mut pass2 = AnalyzerPass2::new(self.text);
        pass2.veryl(input);
        ret.append(&mut pass2.handlers.get_errors());

        ret
    }
}
