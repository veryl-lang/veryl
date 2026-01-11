use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::symbol::SymbolKind;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;

pub fn check_import(context: &mut Context, value: &ImportDeclaration) {
    if let Ok(symbol) = symbol_table::resolve(value.scoped_identifier.as_ref()) {
        let is_wildcard = value.import_declaration_opt.is_some();
        let is_valid_import = if matches!(symbol.found.kind, SymbolKind::SystemVerilog) {
            true
        } else if is_wildcard {
            symbol.found.is_package(false)
        } else if symbol.full_path.len() >= 2 {
            let package_symbol = symbol
                .full_path
                .get(symbol.full_path.len() - 2)
                .map(|x| symbol_table::get(*x).unwrap())
                .unwrap();
            // The preceding symbol must be a package or
            // a proto-package referenced through a generic parameter.
            package_symbol.is_package(false) && symbol.found.is_importable(true)
        } else {
            false
        };

        if !is_valid_import {
            context.insert_error(AnalyzerError::invalid_import(
                &value.scoped_identifier.as_ref().into(),
            ));
        }
    }
}
