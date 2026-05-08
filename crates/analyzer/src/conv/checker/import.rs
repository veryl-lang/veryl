use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::symbol::SymbolKind;
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;

pub fn check_import(context: &mut Context, value: &ImportDeclaration) {
    let path: GenericSymbolPath = value.scoped_identifier.as_ref().into();
    if let Ok(symbol) = symbol_table::resolve(&path) {
        let is_wildcard = value.import_declaration_opt.is_some();
        let is_valid_import = if matches!(symbol.found.kind, SymbolKind::SystemVerilog) {
            true
        } else if is_wildcard {
            symbol.found.is_package(false)
                || matches!(symbol.found.kind, SymbolKind::ProtoAliasPackage(_)) && symbol.imported
                || matches!(symbol.found.kind, SymbolKind::Enum(_))
        } else if symbol.full_path.len() >= 2 {
            let parent_symbol = symbol
                .full_path
                .get(symbol.full_path.len() - 2)
                .map(|x| symbol_table::get(*x).unwrap())
                .unwrap();
            if matches!(parent_symbol.kind, SymbolKind::ProtoAliasPackage(_)) {
                let parent_path = path.slice(path.len() - 2);
                symbol_table::resolve(&parent_path)
                    .map(|parent| parent.imported && symbol.found.is_importable(true))
                    .unwrap()
            } else {
                // The preceding symbol must be a package, an enum, or
                // a proto-package referenced through a generic parameter.
                (parent_symbol.is_package(false)
                    || matches!(parent_symbol.kind, SymbolKind::Enum(_)))
                    && symbol.found.is_importable(true)
            }
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
