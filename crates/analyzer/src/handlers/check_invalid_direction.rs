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
            if let Direction::Direction3(x) = arg {
                if !self.in_function {
                    self.errors.push(AnalyzeError::invalid_direction(
                        "ref",
                        self.text,
                        &x.r#ref.ref_token,
                    ));
                }
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
}
