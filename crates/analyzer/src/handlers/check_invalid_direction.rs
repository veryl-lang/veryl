use crate::analyze_error::AnalyzeError;
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckInvalidDirection<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    point: HandlerPoint,
    in_function: bool,
    in_module: bool,
}

impl<'a> CheckInvalidDirection<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckInvalidDirection<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckInvalidDirection<'a> {
    fn direction(&mut self, arg: &Direction) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            match arg {
                Direction::Ref(x) => {
                    if !self.in_function {
                        self.errors.push(AnalyzeError::invalid_direction(
                            "ref",
                            self.text,
                            &x.r#ref.ref_token,
                        ));
                    }
                }
                Direction::Modport(x) => {
                    if !self.in_module {
                        self.errors.push(AnalyzeError::invalid_direction(
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

    fn function_declaration(&mut self, _arg: &FunctionDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => self.in_function = true,
            HandlerPoint::After => self.in_function = false,
        }
        Ok(())
    }

    fn module_declaration(&mut self, _arg: &ModuleDeclaration) -> Result<()> {
        match self.point {
            HandlerPoint::Before => self.in_module = true,
            HandlerPoint::After => self.in_module = false,
        }
        Ok(())
    }
}
