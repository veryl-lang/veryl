pub mod invalid_direction;
pub mod invalid_number_character;
pub mod invalid_reset;
pub mod invalid_statement;
pub mod number_overflow;
use invalid_direction::*;
use invalid_number_character::*;
use invalid_reset::*;
use invalid_statement::*;
use number_overflow::*;

use crate::analyze_error::AnalyzeError;
use veryl_parser::veryl_walker::Handler;

pub struct Handlers<'a> {
    invalid_direction: InvalidDirection<'a>,
    invalid_number_character: InvalidNumberCharacter<'a>,
    invalid_reset: InvalidReset<'a>,
    invalid_statement: InvalidStatement<'a>,
    number_overflow: NumberOverflow<'a>,
}

impl<'a> Handlers<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            invalid_direction: InvalidDirection::new(text),
            invalid_number_character: InvalidNumberCharacter::new(text),
            invalid_reset: InvalidReset::new(text),
            invalid_statement: InvalidStatement::new(text),
            number_overflow: NumberOverflow::new(text),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![
            &mut self.invalid_direction as &mut dyn Handler,
            &mut self.invalid_number_character as &mut dyn Handler,
            &mut self.invalid_reset as &mut dyn Handler,
            &mut self.invalid_statement as &mut dyn Handler,
            &mut self.number_overflow as &mut dyn Handler,
        ]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzeError> {
        let mut ret = Vec::new();
        ret.append(&mut self.invalid_direction.errors);
        ret.append(&mut self.invalid_number_character.errors);
        ret.append(&mut self.invalid_reset.errors);
        ret.append(&mut self.invalid_statement.errors);
        ret.append(&mut self.number_overflow.errors);
        ret
    }
}
