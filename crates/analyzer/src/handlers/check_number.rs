use crate::analyzer_error::AnalyzerError;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

const BINARY_CHARS: [char; 6] = ['0', '1', 'x', 'z', 'X', 'Z'];
const OCTAL_CHARS: [char; 12] = ['0', '1', '2', '3', '4', '5', '6', '7', 'x', 'z', 'X', 'Z'];
const DECIMAL_CHARS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

#[derive(Default)]
pub struct CheckNumber {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
}

impl CheckNumber {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckNumber {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckNumber {
    fn based(&mut self, arg: &Based) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let token = &arg.based_token.token;
            let text = token.to_string();
            let (width, tail) = text.split_once('\'').unwrap();
            let signed = &tail[0..1] == "s";
            let base = if signed { &tail[1..2] } else { &tail[0..1] };
            let number = if signed { &tail[2..] } else { &tail[1..] };

            let width: Option<usize> = if width.is_empty() {
                None
            } else {
                Some(width.replace('_', "").parse().unwrap())
            };
            let number = number.replace('_', "");
            let number = number.trim_start_matches('0');

            let base = match base {
                "b" => {
                    if let Some(x) = number.chars().find(|x| !BINARY_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x,
                            "binary",
                            &token.into(),
                        ));
                    }
                    2
                }
                "o" => {
                    if let Some(x) = number.chars().find(|x| !OCTAL_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x,
                            "octal",
                            &token.into(),
                        ));
                    }
                    8
                }
                "d" => {
                    if let Some(x) = number.chars().find(|x| !DECIMAL_CHARS.contains(x)) {
                        self.errors.push(AnalyzerError::invalid_number_character(
                            x,
                            "decimal",
                            &token.into(),
                        ));
                    }
                    10
                }
                "h" => 16,
                _ => unreachable!(),
            };

            let zero = number.trim_start_matches('0').is_empty();

            if let Some(actual_width) = strnum_bitwidth::bitwidth(number, base) {
                if let Some(width) = width {
                    if actual_width > width {
                        self.errors
                            .push(AnalyzerError::too_large_number(width, &token.into()));
                    }
                }
            } else if width.is_none() && !zero {
                // bitwidth calculation may be failed over 128bit.
                self.errors
                    .push(AnalyzerError::too_large_number(128, &token.into()));
            }
        }

        Ok(())
    }
}
