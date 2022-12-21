use crate::analyze_error::AnalyzeError;
use veryl_parser::miette::Result;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

const BINARY_CHARS: [char; 6] = ['0', '1', 'x', 'z', 'X', 'Z'];
const OCTAL_CHARS: [char; 12] = ['0', '1', '2', '3', '4', '5', '6', '7', 'x', 'z', 'X', 'Z'];
const DECIMAL_CHARS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

#[derive(Default)]
pub struct InvalidNumberCharacter<'a> {
    pub errors: Vec<AnalyzeError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> InvalidNumberCharacter<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for InvalidNumberCharacter<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for InvalidNumberCharacter<'a> {
    fn based(&mut self, arg: &Based) -> Result<()> {
        if let HandlerPoint::Before = self.point {
            let token = &arg.based_token;
            let text = token.token.token.text();
            let (_, tail) = text.split_once('\'').unwrap();
            let base = &tail[0..1];
            let number = &tail[1..];

            let number = number.replace('_', "");
            let number = number.trim_start_matches('0');

            match base {
                "b" => {
                    if let Some(x) = number.chars().find(|x| !BINARY_CHARS.contains(x)) {
                        self.errors.push(AnalyzeError::invalid_number_character(
                            x, "binary", self.text, token,
                        ));
                    }
                }
                "o" => {
                    if let Some(x) = number.chars().find(|x| !OCTAL_CHARS.contains(x)) {
                        self.errors.push(AnalyzeError::invalid_number_character(
                            x, "octal", self.text, token,
                        ));
                    }
                }
                "d" => {
                    if let Some(x) = number.chars().find(|x| !DECIMAL_CHARS.contains(x)) {
                        self.errors.push(AnalyzeError::invalid_number_character(
                            x, "decimal", self.text, token,
                        ));
                    }
                }
                _ => (),
            }
        }

        Ok(())
    }
}
