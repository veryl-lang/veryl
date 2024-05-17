use crate::analyzer_error::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{DocComment, GenericInstanceProperty, Symbol, SymbolKind};
use crate::symbol_path::{GenericSymbol, GenericSymbolPath, SymbolPath};
use crate::symbol_table::{self, ResolveError, ResolveErrorCause};
use veryl_parser::resource_table::TokenId;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenRange};
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

    fn push_resolve_error(&mut self, err: ResolveError, token: &TokenRange) {
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

    fn generic_symbol_path(&mut self, path: &GenericSymbolPath, namespace: &Namespace) {
        if path.is_generic_reference() {
            return;
        }

        for i in 0..path.len() {
            let base_path = path.base_path(i);

            match symbol_table::resolve((&base_path, namespace)) {
                Ok(symbol) => {
                    symbol_table::add_reference(symbol.found.id, &path.paths[0].base);

                    // Check number of arguments
                    let params = symbol.found.generic_parameters();
                    let n_args = path.paths[i].arguments.len();
                    let match_artiy = if params.len() > n_args {
                        params[n_args].1.is_some()
                    } else {
                        params.len() == n_args
                    };

                    if !match_artiy {
                        self.errors.push(AnalyzerError::mismatch_generics_arity(
                            &path.paths[i].base.to_string(),
                            params.len(),
                            n_args,
                            self.text,
                            &path.range,
                        ));
                        break;
                    }

                    let mut path = path.paths[i].clone();

                    for param in params.iter().skip(n_args) {
                        //  apply default value
                        path.arguments.push(param.1.as_ref().unwrap().clone());
                    }

                    if let Some((token, new_symbol)) =
                        self.get_generic_instance(&symbol.found, &path)
                    {
                        if let Some(ref x) = symbol_table::insert(&token, new_symbol) {
                            symbol_table::add_generic_instance(symbol.found.id, *x);
                        }

                        let table = symbol.found.generic_table(&path.arguments);
                        let mut references = symbol.found.generic_references();
                        for path in &mut references {
                            path.apply_map(&table);
                            self.generic_symbol_path(path, &symbol.found.inner_namespace());
                        }
                    }
                }
                Err(err) => {
                    let single_path = path.paths.len() == 1;
                    let anonymous = path.paths[0].base.to_string() == "_";
                    if single_path && (anonymous || !path.resolvable) {
                        return;
                    }

                    self.push_resolve_error(err, &path.range);
                }
            }
        }
    }

    fn get_generic_instance(&self, base: &Symbol, path: &GenericSymbol) -> Option<(Token, Symbol)> {
        if path.arguments.is_empty() {
            None
        } else {
            let property = GenericInstanceProperty {
                base: base.id,
                arguments: path.arguments.clone(),
            };
            let kind = SymbolKind::GenericInstance(property);
            let token = &path.base;
            let token = Token::new(
                &path.mangled().to_string(),
                token.line,
                token.column,
                token.length,
                token.pos,
                token.source,
            );
            let symbol = Symbol::new(&token, kind, &base.namespace, false, DocComment::default());
            Some((token, symbol))
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
                    self.push_resolve_error(err, &arg.into());
                }
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let ident = arg.identifier().token;
            let path: GenericSymbolPath = arg.into();
            let namespace = namespace_table::get(ident.id).unwrap();

            self.generic_symbol_path(&path, &namespace);
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        // Should be executed after scoped_identifier to handle hierarchical access only
        if let HandlerPoint::After = self.point {
            let ident = arg.identifier().token;
            let mut path: SymbolPath = arg.scoped_identifier.as_ref().into();
            let namespace = namespace_table::get(ident.id).unwrap();

            for x in &arg.expression_identifier_list0 {
                path.push(x.identifier.identifier_token.token.text);

                match symbol_table::resolve((&path, &namespace)) {
                    Ok(symbol) => {
                        symbol_table::add_reference(symbol.found.id, &ident);
                    }
                    Err(err) => {
                        self.push_resolve_error(err, &arg.into());
                    }
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
                    self.push_resolve_error(err, &arg.identifier.as_ref().into());
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
                        self.push_resolve_error(err, &arg.identifier.as_ref().into());
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
            let id = arg.scoped_identifier.identifier().token.id;
            let namespace = namespace_table::get(id).unwrap();
            match symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                Ok(symbol) => {
                    let symbol = symbol.found;
                    match symbol.kind {
                        SymbolKind::Package(_) if is_wildcard => {
                            let mut target = symbol.namespace.clone();
                            target.push(symbol.token.text);

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
                                &arg.scoped_identifier.as_ref().into(),
                            ));
                        }
                        _ => {
                            if self.top_level {
                                self.file_scope_imported_items.push(symbol.token.id);
                            } else {
                                symbol_table::add_imported_item(symbol.token.id, &namespace);
                            }
                        }
                    }
                }
                Err(err) => {
                    self.push_resolve_error(err, &arg.scoped_identifier.as_ref().into());
                }
            }
        }
        Ok(())
    }
}
