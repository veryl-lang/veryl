use crate::analyzer_error::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::SymbolKind;
use crate::symbol_table::{self, ResolveError, ResolveErrorCause, ResolveSymbol, SymbolPath};
use veryl_parser::resource_table::TokenId;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CreateReference<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    top_level: bool,
    file_scope_imported_items: Vec<TokenId>,
    file_scope_imported_packages: Vec<Namespace>,
}

impl<'a> CreateReference<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            top_level: true,
            ..Default::default()
        }
    }

    fn push_resolve_error(&mut self, err: ResolveError, token: &Token) {
        if let Some(last_found) = err.last_found {
            let name = last_found.token.to_string();
            match err.cause {
                ResolveErrorCause::NotFound(not_found) => {
                    let member = format!("{}", not_found);
                    self.errors.push(AnalyzerError::unknown_member(
                        &name, &member, self.text, token,
                    ));
                }
                ResolveErrorCause::Private => {
                    self.errors
                        .push(AnalyzerError::private_member(&name, self.text, token));
                }
            }
        } else if let ResolveErrorCause::NotFound(not_found) = err.cause {
            let name = format!("{}", not_found);
            self.errors
                .push(AnalyzerError::undefined_identifier(&name, self.text, token));
        } else {
            unreachable!();
        }
    }
}

impl<'a> Handler for CreateReference<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CreateReference<'a> {
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    for id in symbol.full_path {
                        symbol_table::add_reference(id, &arg.identifier.identifier_token.token);
                    }
                }
                Err(err) => {
                    let is_single_identifier = SymbolPath::from(arg).as_slice().len() == 1;
                    let name = arg.identifier.identifier_token.to_string();
                    if name == "_" && is_single_identifier {
                        return Ok(());
                    }

                    // TODO check SV-side member to suppress error
                    self.push_resolve_error(err, &arg.identifier.identifier_token.token);
                }
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    for id in symbol.full_path {
                        symbol_table::add_reference(id, &arg.identifier.identifier_token.token);
                    }
                }
                Err(err) => {
                    self.push_resolve_error(err, &arg.identifier.identifier_token.token);
                }
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    for id in symbol.full_path {
                        symbol_table::add_reference(id, &arg.identifier.identifier_token.token);
                    }
                }
                Err(err) => {
                    let is_single_identifier = SymbolPath::from(arg).as_slice().len() == 1;
                    let name = arg.identifier.identifier_token.to_string();
                    if name == "_" && is_single_identifier {
                        return Ok(());
                    }

                    self.push_resolve_error(err, &arg.identifier.identifier_token.token);
                }
            }
        }
        Ok(())
    }

    fn modport_item(&mut self, arg: &ModportItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg.identifier.as_ref()) {
                Ok(symbol) => {
                    for id in symbol.full_path {
                        symbol_table::add_reference(id, &arg.identifier.identifier_token.token);
                    }
                }
                Err(err) => {
                    self.push_resolve_error(err, &arg.identifier.identifier_token.token);
                }
            }
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            // implicit port connection by name
            if arg.inst_port_item_opt.is_none() {
                match symbol_table::resolve(arg.identifier.as_ref()) {
                    Ok(symbol) => {
                        for id in symbol.full_path {
                            symbol_table::add_reference(id, &arg.identifier.identifier_token.token);
                        }
                    }
                    Err(err) => {
                        self.push_resolve_error(err, &arg.identifier.identifier_token.token);
                    }
                }
            }
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.top_level = false;
                let mut namespace = Namespace::default();
                namespace.push(arg.identifier.identifier_token.token.text);
                for x in &self.file_scope_imported_items {
                    symbol_table::add_imported_item(*x, &namespace);
                }
                for x in &self.file_scope_imported_packages {
                    symbol_table::add_imported_package(x, &namespace);
                }
            }
            HandlerPoint::After => {
                self.top_level = true;
            }
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.top_level = false;
                let mut namespace = Namespace::default();
                namespace.push(arg.identifier.identifier_token.token.text);
                for x in &self.file_scope_imported_items {
                    symbol_table::add_imported_item(*x, &namespace);
                }
                for x in &self.file_scope_imported_packages {
                    symbol_table::add_imported_package(x, &namespace);
                }
            }
            HandlerPoint::After => {
                self.top_level = true;
            }
        }
        Ok(())
    }

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.top_level = false;
                let mut namespace = Namespace::default();
                namespace.push(arg.identifier.identifier_token.token.text);
                for x in &self.file_scope_imported_items {
                    symbol_table::add_imported_item(*x, &namespace);
                }
                for x in &self.file_scope_imported_packages {
                    symbol_table::add_imported_package(x, &namespace);
                }
            }
            HandlerPoint::After => {
                self.top_level = true;
            }
        }
        Ok(())
    }

    fn import_declaration(&mut self, arg: &ImportDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let is_wildcard = arg.import_declaration_opt.is_some();
            let namespace =
                namespace_table::get(arg.scoped_identifier.identifier.identifier_token.token.id)
                    .unwrap();
            match symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                Ok(symbol) => {
                    if let ResolveSymbol::Symbol(x) = symbol.found {
                        match x.kind {
                            SymbolKind::Package(_) if is_wildcard => {
                                let mut target = x.namespace.clone();
                                target.push(x.token.text);

                                if self.top_level {
                                    self.file_scope_imported_packages.push(target);
                                } else {
                                    symbol_table::add_imported_package(&target, &namespace);
                                }
                            }
                            SymbolKind::SystemVerilog => (),
                            _ if is_wildcard => {
                                self.errors.push(AnalyzerError::invalid_import(
                                    self.text,
                                    &arg.scoped_identifier.identifier.identifier_token.token,
                                ));
                            }
                            _ => {
                                if self.top_level {
                                    self.file_scope_imported_items.push(x.token.id);
                                } else {
                                    symbol_table::add_imported_item(x.token.id, &namespace);
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    self.push_resolve_error(
                        err,
                        &arg.scoped_identifier.identifier.identifier_token.token,
                    );
                }
            }
        }
        Ok(())
    }
}
