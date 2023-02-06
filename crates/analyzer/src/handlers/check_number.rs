use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

const BINARY_CHARS: [char; 6] = ['0', '1', 'x', 'z', 'X', 'Z'];
const OCTAL_CHARS: [char; 12] = ['0', '1', '2', '3', '4', '5', '6', '7', 'x', 'z', 'X', 'Z'];
const DECIMAL_CHARS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

#[derive(Default)]
pub struct CheckNumber<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckNumber<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckNumber<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckNumber<'a> {
    fn based(&mut self, arg: &Based) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = &arg.based_token;
            let text = token.text();
            let (width, tail) = text.split_once('\'').unwrap();
            let base = &tail[0..1];
            let number = &tail[1..];

            let width: usize = width.replace('_', "").parse().unwrap();
            let number = number.replace('_', "");
            let number = number.trim_start_matches('0');

            match base {
                "b" => {
                    if let Some(x) = number.chars().find(|x| !BINARY_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x, "binary", self.text, token,
                        ));
                    }

                    let actual_width = number.chars().count();
                    if actual_width > width {
                        self.errors
                            .push(AnalyzerError::too_large_number(width, self.text, token));
                    }
                }
                "o" => {
                    if let Some(x) = number.chars().find(|x| !OCTAL_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x, "octal", self.text, token,
                        ));
                    }

                    let mut actual_width = number.chars().count() * 3;
                    match number.chars().next() {
                        Some('1') => actual_width -= 2,
                        Some('2') => actual_width -= 1,
                        Some('3') => actual_width -= 1,
                        _ => (),
                    }
                    if actual_width > width {
                        self.errors
                            .push(AnalyzerError::too_large_number(width, self.text, token));
                    }
                }
                "d" => {
                    if let Some(x) = number.chars().find(|x| !DECIMAL_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x, "decimal", self.text, token,
                        ));
                    }
                }
                "h" => {
                    let mut actual_width = number.chars().count() * 4;
                    match number.chars().next() {
                        Some('1') => actual_width -= 3,
                        Some('2') => actual_width -= 2,
                        Some('3') => actual_width -= 2,
                        Some('4') => actual_width -= 1,
                        Some('5') => actual_width -= 1,
                        Some('6') => actual_width -= 1,
                        Some('7') => actual_width -= 1,
                        _ => (),
                    }
                    if actual_width > width {
                        self.errors
                            .push(AnalyzerError::too_large_number(width, self.text, token));
                    }
                }
                _ => unreachable!(),
            }
        }

        Ok(())
    }
}
