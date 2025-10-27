pub mod check_anonymous;
pub mod check_attribute;
pub mod check_clock_domain;
pub mod check_clock_reset;
pub mod check_connect_operation;
pub mod check_embed_include;
pub mod check_identifier;
pub mod check_msb_lsb;
pub mod check_statement;
pub mod check_type;
pub mod check_unsafe;
pub mod check_var_ref;
pub mod create_literal_table;
pub mod create_symbol_table;
use check_anonymous::*;
use check_attribute::*;
use check_clock_domain::*;
use check_clock_reset::*;
use check_connect_operation::*;
use check_embed_include::*;
use check_identifier::*;
use check_msb_lsb::*;
use check_statement::*;
use check_type::*;
use check_unsafe::*;
use check_var_ref::*;
use create_literal_table::*;
use create_symbol_table::*;

use crate::analyzer_error::AnalyzerError;
use veryl_metadata::{Build, EnvVar, Lint};
use veryl_parser::veryl_walker::Handler;

pub struct Pass1Handlers {
    check_attribute: CheckAttribute,
    check_embed_include: CheckEmbedInclude,
    check_identifier: CheckIdentifier,
    check_statement: CheckStatement,
    check_unsafe: CheckUnsafe,
    create_literal_table: CreateLiteralTable,
    create_symbol_table: CreateSymbolTable,
    enables: [bool; 7],
}

impl Pass1Handlers {
    pub fn new(build_opt: &Build, lint_opt: &Lint, env_var: &EnvVar) -> Self {
        Self {
            check_attribute: CheckAttribute::new(),
            check_embed_include: CheckEmbedInclude::new(),
            check_identifier: CheckIdentifier::new(lint_opt),
            check_statement: CheckStatement::new(),
            check_unsafe: CheckUnsafe::new(),
            create_literal_table: CreateLiteralTable::new(),
            create_symbol_table: CreateSymbolTable::new(build_opt),
            enables: env_var.analyzer_pass1_enables,
        }
    }

    pub fn get_handlers(&mut self) -> Vec<(bool, &mut dyn Handler)> {
        let en = &self.enables;
        vec![
            (en[0], &mut self.check_attribute as &mut dyn Handler),
            (en[1], &mut self.check_embed_include as &mut dyn Handler),
            (en[2], &mut self.check_identifier as &mut dyn Handler),
            (en[3], &mut self.check_statement as &mut dyn Handler),
            (en[4], &mut self.check_unsafe as &mut dyn Handler),
            (en[5], &mut self.create_literal_table as &mut dyn Handler),
            (en[6], &mut self.create_symbol_table as &mut dyn Handler),
        ]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();
        ret.append(&mut self.check_attribute.errors);
        ret.append(&mut self.check_embed_include.errors);
        ret.append(&mut self.check_identifier.errors);
        ret.append(&mut self.check_statement.errors);
        ret.append(&mut self.check_unsafe.errors);
        ret.append(&mut self.create_literal_table.errors);
        ret.append(&mut self.create_symbol_table.errors);
        ret
    }
}

pub struct Pass2Handlers {
    check_msb_lsb: CheckMsbLsb,
    check_connect_operation: CheckConnectOperation,
    check_var_ref: CheckVarRef,
    check_clock_reset: CheckClockReset,
    check_anonymous: CheckAnonymous,
    check_clock_domain: CheckClockDomain,
    check_type: CheckType,
    enables: [bool; 7],
}

impl Pass2Handlers {
    pub fn new(_build_opt: &Build, _lint_opt: &Lint, env_var: &EnvVar) -> Self {
        Self {
            check_msb_lsb: CheckMsbLsb::new(),
            check_connect_operation: CheckConnectOperation::new(),
            check_var_ref: CheckVarRef::new(),
            check_clock_reset: CheckClockReset::new(),
            check_anonymous: CheckAnonymous::new(),
            check_clock_domain: CheckClockDomain::new(),
            check_type: CheckType::new(),
            enables: env_var.analyzer_pass2_enables,
        }
    }

    pub fn get_handlers(&mut self) -> Vec<(bool, &mut dyn Handler)> {
        let en = &self.enables;
        vec![
            (en[0], &mut self.check_msb_lsb as &mut dyn Handler),
            (en[1], &mut self.check_connect_operation as &mut dyn Handler),
            (en[2], &mut self.check_var_ref as &mut dyn Handler),
            (en[3], &mut self.check_clock_reset as &mut dyn Handler),
            (en[4], &mut self.check_anonymous as &mut dyn Handler),
            (en[5], &mut self.check_clock_domain as &mut dyn Handler),
            (en[6], &mut self.check_type as &mut dyn Handler),
        ]
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();
        ret.append(&mut self.check_msb_lsb.errors);
        ret.append(&mut self.check_connect_operation.errors);
        ret.append(&mut self.check_var_ref.errors);
        ret.append(&mut self.check_clock_reset.errors);
        ret.append(&mut self.check_anonymous.errors);
        ret.append(&mut self.check_clock_domain.errors);
        ret.append(&mut self.check_type.errors);
        ret
    }
}
