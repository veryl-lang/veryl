pub mod check_attribute;
pub mod check_embed_include;
pub mod check_identifier;
pub mod check_statement;
pub mod check_unsafe;
pub mod create_literal_table;
pub mod create_symbol_table;
use check_attribute::*;
use check_embed_include::*;
use check_identifier::*;
use check_statement::*;
use check_unsafe::*;
use create_literal_table::*;
use create_symbol_table::*;

use crate::analyzer_error::AnalyzerError;
use veryl_metadata::{Build, Lint};
use veryl_parser::veryl_walker::Handler;

pub struct Pass1Handlers {
    check_attribute: CheckAttribute,
    check_embed_include: CheckEmbedInclude,
    check_identifier: CheckIdentifier,
    check_statement: CheckStatement,
    check_unsafe: CheckUnsafe,
    create_literal_table: CreateLiteralTable,
    create_symbol_table: CreateSymbolTable,
}

impl Pass1Handlers {
    pub fn new(build_opt: &Build, lint_opt: &Lint) -> Self {
        Self {
            check_attribute: CheckAttribute::new(),
            check_embed_include: CheckEmbedInclude::new(),
            check_identifier: CheckIdentifier::new(lint_opt),
            check_statement: CheckStatement::new(),
            check_unsafe: CheckUnsafe::new(),
            create_literal_table: CreateLiteralTable::new(),
            create_symbol_table: CreateSymbolTable::new(build_opt),
        }
    }

    pub fn get_handlers(&mut self) -> Vec<&mut dyn Handler> {
        vec![
            &mut self.check_attribute as &mut dyn Handler,
            &mut self.check_embed_include as &mut dyn Handler,
            &mut self.check_identifier as &mut dyn Handler,
            &mut self.check_statement as &mut dyn Handler,
            &mut self.check_unsafe as &mut dyn Handler,
            &mut self.create_literal_table as &mut dyn Handler,
            &mut self.create_symbol_table as &mut dyn Handler,
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
