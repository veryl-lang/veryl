use crate::analyze_error::AnalyzeError;
use crate::handlers::*;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, VerylWalker};

enum AnalyzePass {
    Pass1,
    Pass2,
}

pub struct Analyzer<'a> {
    pub errors: Vec<AnalyzeError>,
    pass1_handlers: Pass1Handlers<'a>,
    pass2_handlers: Pass2Handlers<'a>,
    pass: AnalyzePass,
}

impl<'a> Analyzer<'a> {
    pub fn new(text: &'a str) -> Self {
        Analyzer {
            errors: Vec::new(),
            pass1_handlers: Pass1Handlers::new(text),
            pass2_handlers: Pass2Handlers::new(text),
            pass: AnalyzePass::Pass1,
        }
    }

    pub fn analyze(&mut self, input: &Veryl) {
        self.pass = AnalyzePass::Pass1;
        self.veryl(input);
        self.errors.append(&mut self.pass1_handlers.get_errors());

        self.pass = AnalyzePass::Pass2;
        self.veryl(input);
        self.errors.append(&mut self.pass2_handlers.get_errors());
    }
}

impl<'a> VerylWalker for Analyzer<'a> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        match self.pass {
            AnalyzePass::Pass1 => Some(self.pass1_handlers.get_handlers()),
            AnalyzePass::Pass2 => Some(self.pass2_handlers.get_handlers()),
        }
    }
}
