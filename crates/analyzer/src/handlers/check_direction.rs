use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckDirection<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    in_function: bool,
    in_module: bool,
}

impl<'a> CheckDirection<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckDirection<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckDirection<'a> {
    fn direction(&mut self, arg: &Direction) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match arg {
                Direction::Ref(x) => {
                    if !self.in_function {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "ref",
                            self.text,
                            &x.r#ref.ref_token,
                        ));
                    }
                }
                Direction::Modport(x) => {
                    if !self.in_module {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "modport",
                            self.text,
                            &x.modport.modport_token,
                        ));
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, _arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_function = true,
            HandlerPoint::After => self.in_function = false,
        }
        Ok(())
    }

    fn module_declaration(&mut self, _arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_module = true,
            HandlerPoint::After => self.in_module = false,
        }
        Ok(())
    }
}
