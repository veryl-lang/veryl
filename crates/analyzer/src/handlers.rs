pub mod check_attribute;
pub mod check_direction;
pub mod check_enum;
pub mod check_function;
pub mod check_instance;
pub mod check_msb_lsb;
pub mod check_number;
pub mod check_reset;
pub mod check_statement;
pub mod check_system_function;
pub mod create_reference;
pub mod create_symbol_table;
use check_attribute::*;
use check_direction::*;
use check_enum::*;
use check_function::*;
use check_instance::*;
use check_msb_lsb::*;
use check_number::*;
use check_reset::*;
use check_statement::*;
use check_system_function::*;
use create_reference::*;
use create_symbol_table::*;

use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_walker::Handler;

pub struct Pass1Handlers<'a> {
    check_attribute: CheckAttribute<'a>,
    check_direction: CheckDirection<'a>,
    check_number: CheckNumber<'a>,
    check_reset: CheckReset<'a>,
    check_statement: CheckStatement<'a>,
    check_system_function: CheckSystemFunction<'a>,
    create_symbol_table: CreateSymbolTable<'a>,
}

impl<'a> Pass1Handlers<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            check_attribute: CheckAttribute::new(text),
            check_direction: CheckDirection::new(text),
            check_number: CheckNumber::new(text),
            check_reset: CheckReset::new(text),
            check_statement: CheckStatement::new(text),
            check_system_function: CheckSystemFunction::new(text),
            create_symbol_table: CreateSymbolTable::new(text),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![
            &mut self.check_attribute as &mut dyn Handler,
            &mut self.check_direction as &mut dyn Handler,
            &mut self.check_number as &mut dyn Handler,
            &mut self.check_reset as &mut dyn Handler,
            &mut self.check_statement as &mut dyn Handler,
            &mut self.check_system_function as &mut dyn Handler,
            &mut self.create_symbol_table as &mut dyn Handler,
        ]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();
        ret.append(&mut self.check_attribute.errors);
        ret.append(&mut self.check_direction.errors);
        ret.append(&mut self.check_number.errors);
        ret.append(&mut self.check_reset.errors);
        ret.append(&mut self.check_statement.errors);
        ret.append(&mut self.check_system_function.errors);
        ret.append(&mut self.create_symbol_table.errors);
        ret
    }
}

pub struct Pass2Handlers<'a> {
    check_attribute: CheckAttribute<'a>,
    check_enum: CheckEnum<'a>,
    check_function: CheckFunction<'a>,
    check_instance: CheckInstance<'a>,
    check_msb_lsb: CheckMsbLsb<'a>,
    create_reference: CreateReference<'a>,
}

impl<'a> Pass2Handlers<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            check_attribute: CheckAttribute::new(text),
            check_enum: CheckEnum::new(text),
            check_function: CheckFunction::new(text),
            check_instance: CheckInstance::new(text),
            check_msb_lsb: CheckMsbLsb::new(text),
            create_reference: CreateReference::new(text),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![
            &mut self.check_attribute as &mut dyn Handler,
            &mut self.check_enum as &mut dyn Handler,
            &mut self.check_function as &mut dyn Handler,
            &mut self.check_instance as &mut dyn Handler,
            &mut self.check_msb_lsb as &mut dyn Handler,
            &mut self.create_reference as &mut dyn Handler,
        ]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();
        // errors of check_attribute are reported at pass1
        ret.append(&mut self.check_enum.errors);
        ret.append(&mut self.check_function.errors);
        ret.append(&mut self.check_instance.errors);
        ret.append(&mut self.check_msb_lsb.errors);
        ret.append(&mut self.create_reference.errors);
        ret
    }
}
