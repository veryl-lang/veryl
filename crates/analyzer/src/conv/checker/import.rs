use crate::analyzer_error::AnalyzerError;
use crate::conv::Context;
use crate::symbol::SymbolKind;
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table::{self, ResolveErrorCause};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

pub fn check_import(context: &mut Context, value: &ImportDeclaration) {
    let base: GenericSymbolPath = value.scoped_identifier.as_ref().into();

    match value
        .import_declaration_opt
        .as_ref()
        .map(|x| x.import_declaration_opt_group.as_ref())
    {
        Some(ImportDeclarationOptGroup::Star(_)) => {
            check_import_path(
                context,
                &base,
                true,
                &value.scoped_identifier.as_ref().into(),
            );
        }
        Some(ImportDeclarationOptGroup::MultipleImportList(x)) => {
            // `import pkg::{a, b};` imports each item individually, so check
            // the path of each item instead of the base path.
            let items: Vec<&MultipleImportItem> = x.multiple_import_list.as_ref().into();
            for item in items {
                let mut path = base.clone();
                path.append(&item.identifier.identifier_token.token, &[]);
                check_import_path(context, &path, false, &item.identifier.as_ref().into());
            }
        }
        None => {
            check_import_path(
                context,
                &base,
                false,
                &value.scoped_identifier.as_ref().into(),
            );
        }
    }
}

fn check_import_path(
    context: &mut Context,
    path: &GenericSymbolPath,
    is_wildcard: bool,
    range: &TokenRange,
) {
    let symbol = match symbol_table::resolve(path) {
        Ok(symbol) => symbol,
        Err(err) => {
            // An ambiguous import path (the same package name reachable via two
            // wildcard imports) would otherwise be dropped silently by
            // apply_import; surface it here. Other causes are reported by the
            // reference checker.
            if let ResolveErrorCause::Ambiguous(name) = err.cause {
                context.insert_error(AnalyzerError::ambiguous_identifier(
                    &format!("{name}"),
                    range,
                ));
            }
            return;
        }
    };

    let is_valid_import = if matches!(symbol.found.kind, SymbolKind::SystemVerilog) {
        true
    } else if is_wildcard {
        symbol.found.is_package(false)
            || matches!(&symbol.found.kind, SymbolKind::AliasPackage(x) if x.is_proto)
                && symbol.imported
            || matches!(symbol.found.kind, SymbolKind::Enum(_))
            // `pkg` resolved to a same-named member; the wildcard still
            // targets the package.
            || symbol
                .found
                .get_parent_package()
                .is_some_and(|pkg| pkg.token.text == symbol.found.token.text)
    } else if (symbol.found.is_component(false) || symbol.found.is_proto_package(false))
        && !matches!(symbol.found.kind, SymbolKind::GenericParameter(_))
    {
        // Importing a component itself (package, module or interface),
        // so that its members can be referenced with the component name
        // as a qualifier (e.g. `import dep::MyPkg;` then `MyPkg::Raw`).
        // A proto package is included so it can be referenced as a
        // generic constraint without a fully qualified name. A generic
        // component must be imported as its definition, not as an
        // instantiated instance; members of an instance are imported
        // through the path instead (`import pkg::<8>::member;`), and
        // wildcard instance imports (`import pkg::<8>::*;`) keep their
        // existing behavior. A proto interface bound also makes a generic
        // parameter a component, but one that names no package to qualify
        // with, so the emitter would produce a bare SystemVerilog `import`.
        // https://github.com/veryl-lang/veryl/issues/3122
        // https://github.com/veryl-lang/veryl/issues/1588
        !path.paths.last().is_some_and(|p| !p.arguments.is_empty())
    } else if symbol.full_path.len() >= 2 {
        let parent_symbol = symbol
            .full_path
            .get(symbol.full_path.len() - 2)
            .map(|x| symbol_table::get(*x).unwrap())
            .unwrap();
        if matches!(&parent_symbol.kind, SymbolKind::AliasPackage(x) if x.is_proto) {
            let parent_path = path.slice(path.len() - 2);
            symbol_table::resolve(&parent_path)
                .map(|parent| parent.imported && symbol.found.is_importable(true))
                .unwrap()
        } else {
            // The preceding symbol must be a package, an enum, or
            // a proto-package referenced through a generic parameter.
            (parent_symbol.is_package(false) || matches!(parent_symbol.kind, SymbolKind::Enum(_)))
                && symbol.found.is_importable(true)
        }
    } else {
        false
    };

    if !is_valid_import {
        context.insert_error(AnalyzerError::invalid_import(range));
    }
}
