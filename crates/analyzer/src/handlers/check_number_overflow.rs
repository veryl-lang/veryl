use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckNumberOverflow<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CheckNumberOverflow<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl<'a> Handler for CheckNumberOverflow<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckNumberOverflow<'a> {
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
                    let actual_width = number.chars().count();
                    if actual_width > width {
                        self.errors
                            .push(AnalyzerError::number_overflow(width, self.text, token));
                    }
                }
                "o" => {
                    let mut actual_width = number.chars().count() * 3;
                    match number.chars().next() {
                        Some('1') => actual_width -= 2,
                        Some('2') => actual_width -= 1,
                        Some('3') => actual_width -= 1,
                        _ => (),
                    }
                    if actual_width > width {
                        self.errors
                            .push(AnalyzerError::number_overflow(width, self.text, token));
                    }
                }
                "d" => {}
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
                            .push(AnalyzerError::number_overflow(width, self.text, token));
                    }
                }
                _ => unreachable!(),
            }
        }

        Ok(())
    }
}
