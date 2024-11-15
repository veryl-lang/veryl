use crate::analyzer_error::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{GenericMap, SymbolKind};
use crate::symbol_path::{GenericSymbolPath, SymbolPath};
use crate::symbol_table::{self, ResolveError, ResolveErrorCause};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenRange};
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CreateReference<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
}

impl<'a> CreateReference<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    fn push_resolve_error(
        &mut self,
        err: ResolveError,
        token: &TokenRange,
        generics_token: Option<Token>,
    ) {
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
            if let Some(generics_token) = generics_token {
                self.errors
                    .push(AnalyzerError::unresolvable_generic_argument(
                        &name,
                        self.text,
                        token,
                        &generics_token.into(),
                    ));
            } else {
                self.errors
                    .push(AnalyzerError::undefined_identifier(&name, self.text, token));
            }
        } else {
            unreachable!();
        }
    }

    fn generic_symbol_path(
        &mut self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
        generics_token: Option<Token>,
    ) {
        if path.is_generic_reference() {
            return;
        }

        let mut path = path.clone();
        path.resolve_imported(namespace);

        for i in 0..path.len() {
            let base_path = path.base_path(i);

            match symbol_table::resolve((&base_path, namespace)) {
                Ok(symbol) => {
                    symbol_table::add_reference(symbol.found.id, &path.paths[0].base);

                    // Check number of arguments
                    let params = symbol.found.generic_parameters();
                    let n_args = path.paths[i].arguments.len();
                    let match_artiy = if params.len() > n_args {
                        params[n_args].1.default_value.is_some()
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
                        continue;
                    }

                    let mut path = path.paths[i].clone();

                    for param in params.iter().skip(n_args) {
                        //  apply default value
                        path.arguments
                            .push(param.1.default_value.as_ref().unwrap().clone());
                    }

                    if let Some((token, new_symbol)) = path.get_generic_instance(&symbol.found) {
                        if let Some(ref x) = symbol_table::insert(&token, new_symbol) {
                            symbol_table::add_generic_instance(symbol.found.id, *x);
                        }

                        let table = symbol.found.generic_table(&path.arguments);
                        let map = vec![GenericMap {
                            name: "".to_string(),
                            map: table,
                        }];
                        let mut references = symbol.found.generic_references();
                        for path in &mut references {
                            path.apply_map(&map);
                            self.generic_symbol_path(
                                path,
                                &symbol.found.inner_namespace(),
                                Some(symbol.found.token),
                            );
                        }
                    }
                }
                Err(err) => {
                    let single_path = path.paths.len() == 1;
                    let anonymous = path.paths[0].base.to_string() == "_";
                    if single_path && (anonymous || !path.is_resolvable()) {
                        return;
                    }

                    self.push_resolve_error(err, &path.range, generics_token);
                }
            }
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
                    self.push_resolve_error(err, &arg.into(), None);
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

            self.generic_symbol_path(&path, &namespace, None);
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        // Should be executed after scoped_identifier to handle hierarchical access only
        if let HandlerPoint::After = self.point {
            let ident = arg.identifier().token;
            let namespace = namespace_table::get(ident.id).unwrap();
            let mut path: GenericSymbolPath = arg.scoped_identifier.as_ref().into();
            path.resolve_imported(&namespace);
            let mut path = path.mangled_path();

            for x in &arg.expression_identifier_list0 {
                path.push(x.identifier.identifier_token.token.text);

                match symbol_table::resolve((&path, &namespace)) {
                    Ok(symbol) => {
                        symbol_table::add_reference(symbol.found.id, &ident);
                    }
                    Err(err) => {
                        self.push_resolve_error(err, &arg.into(), None);
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
                    self.push_resolve_error(err, &arg.identifier.as_ref().into(), None);
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
                        self.push_resolve_error(err, &arg.identifier.as_ref().into(), None);
                    }
                }
            }
        }
        Ok(())
    }

    fn import_declaration(&mut self, arg: &ImportDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            let is_wildcard = arg.import_declaration_opt.is_some();
            match symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                Ok(symbol) => {
                    let symbol = symbol.found;
                    match symbol.kind {
                        SymbolKind::Package(_) if is_wildcard => (),
                        SymbolKind::SystemVerilog => (),
                        _ if is_wildcard => {
                            self.errors.push(AnalyzerError::invalid_import(
                                self.text,
                                &arg.scoped_identifier.as_ref().into(),
                            ));
                        }
                        _ => (),
                    }
                }
                Err(err) => {
                    self.push_resolve_error(err, &arg.scoped_identifier.as_ref().into(), None);
                }
            }
        }
        Ok(())
    }
}
