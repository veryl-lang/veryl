pub mod check_attribute;
pub mod check_clock_domain;
pub mod check_clock_reset;
pub mod check_embed_include;
pub mod check_enum;
pub mod check_expression;
pub mod check_function;
pub mod check_identifier;
pub mod check_modport;
pub mod check_msb_lsb;
pub mod check_number;
pub mod check_port;
pub mod check_proto;
pub mod check_separator;
pub mod check_statement;
pub mod check_type;
pub mod check_unsafe;
pub mod check_var_ref;
pub mod create_symbol_table;
pub mod create_type_dag;
use check_attribute::*;
use check_clock_domain::*;
use check_clock_reset::*;
use check_embed_include::*;
use check_enum::*;
use check_expression::*;
use check_function::*;
use check_identifier::*;
use check_modport::*;
use check_msb_lsb::*;
use check_number::*;
use check_port::*;
use check_proto::*;
use check_separator::*;
use check_statement::*;
use check_type::*;
use check_unsafe::*;
use check_var_ref::*;
use create_symbol_table::*;
use create_type_dag::*;

use crate::analyzer_error::AnalyzerError;
use veryl_metadata::{Build, Lint};
use veryl_parser::veryl_walker::Handler;

pub struct Pass1Handlers {
    check_attribute: CheckAttribute,
    check_port: CheckPort,
    check_embed_include: CheckEmbedInclude,
    check_identifier: CheckIdentifier,
    check_number: CheckNumber,
    check_statement: CheckStatement,
    check_unsafe: CheckUnsafe,
    create_symbol_table: CreateSymbolTable,
}

impl Pass1Handlers {
    pub fn new(build_opt: &Build, lint_opt: &Lint) -> Self {
        Self {
            check_attribute: CheckAttribute::new(),
            check_port: CheckPort::new(),
            check_embed_include: CheckEmbedInclude::new(),
            check_identifier: CheckIdentifier::new(lint_opt),
            check_number: CheckNumber::new(),
            check_statement: CheckStatement::new(),
            check_unsafe: CheckUnsafe::new(),
            create_symbol_table: CreateSymbolTable::new(build_opt),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![
            &mut self.check_attribute as &mut dyn Handler,
            &mut self.check_port as &mut dyn Handler,
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
        ret.append(&mut self.check_port.errors);
        ret.append(&mut self.check_embed_include.errors);
        ret.append(&mut self.check_identifier.errors);
        ret.append(&mut self.check_number.errors);
        ret.append(&mut self.check_statement.errors);
        ret.append(&mut self.check_unsafe.errors);
        ret.append(&mut self.create_symbol_table.errors);
        ret
    }
}

pub struct Pass2Handlers {
    create_type_dag: CreateTypeDag,
    check_separator: CheckSeparator,
    check_enum: CheckEnum,
    check_modport: CheckModport,
    check_function: CheckFunction,
    check_msb_lsb: CheckMsbLsb,
    check_var_ref: CheckVarRef,
    check_clock_reset: CheckClockReset,
    check_expression: CheckExpression,
    check_clock_domain: CheckClockDomain,
    check_proto: CheckProto,
    check_type: CheckType,
}

impl Pass2Handlers {
    pub fn new(_build_opt: &Build, _lint_opt: &Lint) -> Self {
        Self {
            check_separator: CheckSeparator::new(),
            check_enum: CheckEnum::new(),
            check_modport: CheckModport::new(),
            check_function: CheckFunction::new(),
            check_msb_lsb: CheckMsbLsb::new(),
            check_var_ref: CheckVarRef::new(),
            check_clock_reset: CheckClockReset::new(),
            create_type_dag: CreateTypeDag::new(),
            check_expression: CheckExpression::new(vec![]),
            check_clock_domain: CheckClockDomain::new(),
            check_proto: CheckProto::new(),
            check_type: CheckType::new(),
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
            &mut self.create_type_dag as &mut dyn Handler,
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
        ret.append(&mut self.create_type_dag.errors);
        ret.append(&mut self.check_expression.errors);
        ret.append(&mut self.check_clock_domain.errors);
        ret.append(&mut self.check_proto.errors);
        ret.append(&mut self.check_type.errors);
        ret
    }
}
