pub mod check_attribute;
pub mod check_clock_domain;
pub mod check_clock_reset;
pub mod check_direction;
pub mod check_embed_include;
pub mod check_enum;
pub mod check_expression;
pub mod check_function;
pub mod check_identifier;
pub mod check_modport;
pub mod check_msb_lsb;
pub mod check_number;
pub mod check_proto;
pub mod check_separator;
pub mod check_statement;
pub mod check_type;
pub mod check_unsafe;
pub mod check_var_ref;
pub mod create_reference;
pub mod create_symbol_table;
use check_attribute::*;
use check_clock_domain::*;
use check_clock_reset::*;
use check_direction::*;
use check_embed_include::*;
use check_enum::*;
use check_expression::*;
use check_function::*;
use check_identifier::*;
use check_modport::*;
use check_msb_lsb::*;
use check_number::*;
use check_proto::*;
use check_separator::*;
use check_statement::*;
use check_type::*;
use check_unsafe::*;
use check_var_ref::*;
use create_reference::*;
use create_symbol_table::*;

use crate::analyzer_error::AnalyzerError;
use veryl_metadata::{Build, Lint};
use veryl_parser::veryl_walker::Handler;

pub struct Pass1Handlers<'a> {
    check_attribute: CheckAttribute<'a>,
    check_direction: CheckDirection<'a>,
    check_embed_include: CheckEmbedInclude<'a>,
    check_identifier: CheckIdentifier<'a>,
    check_number: CheckNumber<'a>,
    check_statement: CheckStatement<'a>,
    check_unsafe: CheckUnsafe<'a>,
    create_symbol_table: CreateSymbolTable<'a>,
}

impl<'a> Pass1Handlers<'a> {
    pub fn new(text: &'a str, build_opt: &'a Build, lint_opt: &'a Lint) -> Self {
        Self {
            check_attribute: CheckAttribute::new(text),
            check_direction: CheckDirection::new(text),
            check_embed_include: CheckEmbedInclude::new(text),
            check_identifier: CheckIdentifier::new(text, lint_opt),
            check_number: CheckNumber::new(text),
            check_statement: CheckStatement::new(text),
            check_unsafe: CheckUnsafe::new(text),
            create_symbol_table: CreateSymbolTable::new(text, build_opt),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![
            &mut self.check_attribute as &mut dyn Handler,
            &mut self.check_direction as &mut dyn Handler,
            &mut self.check_embed_include as &mut dyn Handler,
            &mut self.check_identifier as &mut dyn Handler,
            &mut self.check_number as &mut dyn Handler,
            &mut self.check_statement as &mut dyn Handler,
            &mut self.check_unsafe as &mut dyn Handler,
            &mut self.create_symbol_table as &mut dyn Handler,
        ]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();
        ret.append(&mut self.check_attribute.errors);
        ret.append(&mut self.check_direction.errors);
        ret.append(&mut self.check_embed_include.errors);
        ret.append(&mut self.check_identifier.errors);
        ret.append(&mut self.check_number.errors);
        ret.append(&mut self.check_statement.errors);
        ret.append(&mut self.check_unsafe.errors);
        ret.append(&mut self.create_symbol_table.errors);
        ret
    }
}

pub struct Pass2Handlers<'a> {
    check_separator: CheckSeparator<'a>,
    check_enum: CheckEnum<'a>,
    check_modport: CheckModport<'a>,
    check_function: CheckFunction<'a>,
    check_msb_lsb: CheckMsbLsb<'a>,
    check_var_ref: CheckVarRef<'a>,
    check_clock_reset: CheckClockReset<'a>,
    create_reference: CreateReference<'a>,
    check_expression: CheckExpression<'a>,
    check_clock_domain: CheckClockDomain<'a>,
    check_proto: CheckProto<'a>,
    check_type: CheckType<'a>,
}

impl<'a> Pass2Handlers<'a> {
    pub fn new(text: &'a str, _build_opt: &'a Build, _lint_opt: &'a Lint) -> Self {
        Self {
            check_separator: CheckSeparator::new(text),
            check_enum: CheckEnum::new(text),
            check_modport: CheckModport::new(text),
            check_function: CheckFunction::new(text),
            check_msb_lsb: CheckMsbLsb::new(text),
            check_var_ref: CheckVarRef::new(text),
            check_clock_reset: CheckClockReset::new(text),
            create_reference: CreateReference::new(text),
            check_expression: CheckExpression::new(text),
            check_clock_domain: CheckClockDomain::new(text),
            check_proto: CheckProto::new(text),
            check_type: CheckType::new(text),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![
            &mut self.check_separator as &mut dyn Handler,
            &mut self.check_enum as &mut dyn Handler,
            &mut self.check_modport as &mut dyn Handler,
            &mut self.check_function as &mut dyn Handler,
            &mut self.check_msb_lsb as &mut dyn Handler,
            &mut self.check_var_ref as &mut dyn Handler,
            &mut self.check_clock_reset as &mut dyn Handler,
            &mut self.create_reference as &mut dyn Handler,
            &mut self.check_expression as &mut dyn Handler,
            &mut self.check_clock_domain as &mut dyn Handler,
            &mut self.check_proto as &mut dyn Handler,
            &mut self.check_type as &mut dyn Handler,
        ]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();
        ret.append(&mut self.check_separator.errors);
        ret.append(&mut self.check_enum.errors);
        ret.append(&mut self.check_modport.errors);
        ret.append(&mut self.check_function.errors);
        ret.append(&mut self.check_msb_lsb.errors);
        ret.append(&mut self.check_var_ref.errors);
        ret.append(&mut self.check_clock_reset.errors);
        ret.append(&mut self.create_reference.errors);
        ret.append(&mut self.check_expression.errors);
        ret.append(&mut self.check_clock_domain.errors);
        ret.append(&mut self.check_proto.errors);
        ret.append(&mut self.check_type.errors);
        ret
    }
}
