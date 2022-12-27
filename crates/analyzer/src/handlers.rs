pub mod check_invalid_direction;
pub mod check_invalid_number_character;
pub mod check_invalid_reset;
pub mod check_invalid_statement;
pub mod check_number_overflow;
pub mod check_system_function;
pub mod create_symbol_table;
use check_invalid_direction::*;
use check_invalid_number_character::*;
use check_invalid_reset::*;
use check_invalid_statement::*;
use check_number_overflow::*;
use check_system_function::*;
use create_symbol_table::*;

use crate::analyze_error::AnalyzeError;
use crate::symbol_table::SymbolTable;
use std::marker::PhantomData;
use veryl_parser::veryl_walker::Handler;

pub struct Pass1Handlers<'a> {
    check_invalid_direction: CheckInvalidDirection<'a>,
    check_invalid_number_character: CheckInvalidNumberCharacter<'a>,
    check_invalid_reset: CheckInvalidReset<'a>,
    check_invalid_statement: CheckInvalidStatement<'a>,
    check_number_overflow: CheckNumberOverflow<'a>,
    check_system_function: CheckSystemFunction<'a>,
    create_symbol_table: CreateSymbolTable<'a>,
}

impl<'a> Pass1Handlers<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            check_invalid_direction: CheckInvalidDirection::new(text),
            check_invalid_number_character: CheckInvalidNumberCharacter::new(text),
            check_invalid_reset: CheckInvalidReset::new(text),
            check_invalid_statement: CheckInvalidStatement::new(text),
            check_number_overflow: CheckNumberOverflow::new(text),
            check_system_function: CheckSystemFunction::new(text),
            create_symbol_table: CreateSymbolTable::new(text),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![
            &mut self.check_invalid_direction as &mut dyn Handler,
            &mut self.check_invalid_number_character as &mut dyn Handler,
            &mut self.check_invalid_reset as &mut dyn Handler,
            &mut self.check_invalid_statement as &mut dyn Handler,
            &mut self.check_number_overflow as &mut dyn Handler,
            &mut self.check_system_function as &mut dyn Handler,
            &mut self.create_symbol_table as &mut dyn Handler,
        ]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzeError> {
        let mut ret = Vec::new();
        ret.append(&mut self.check_invalid_direction.errors);
        ret.append(&mut self.check_invalid_number_character.errors);
        ret.append(&mut self.check_invalid_reset.errors);
        ret.append(&mut self.check_invalid_statement.errors);
        ret.append(&mut self.check_number_overflow.errors);
        ret.append(&mut self.check_system_function.errors);
        ret.append(&mut self.create_symbol_table.errors);
        ret
    }

    pub fn get_symbol_table(&mut self) -> &SymbolTable {
        &self.create_symbol_table.table
    }
}

pub struct Pass2Handlers<'a> {
    x: PhantomData<&'a ()>,
}

impl<'a> Pass2Handlers<'a> {
    pub fn new(_text: &'a str) -> Self {
        Self {
            x: Default::default(),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzeError> {
        vec![]
    }
}
