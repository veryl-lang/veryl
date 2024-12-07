use crate::analyzer_error::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{Direction, GenericBoundKind, GenericMap, Port, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table::{self, ResolveError, ResolveErrorCause};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{is_anonymous_text, Token, TokenRange};
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CreateReference<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    inst_ports: Vec<Port>,
    inst_sv_module: bool,
    is_anonymous_identifier: bool,
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
                    let is_generic_if = if let SymbolKind::Port(ref port) = last_found.kind {
                        port.direction == Direction::Interface
                    } else {
                        false
                    };

                    if !is_generic_if {
                        let member = format!("{}", not_found);
                        self.errors.push(AnalyzerError::unknown_member(
                            &name, &member, self.text, token,
                        ));
                    }
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
            } else if is_anonymous_text(not_found) {
                self.errors
                    .push(AnalyzerError::anonymous_identifier_usage(self.text, token));
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
                    if single_path && !path.is_resolvable() {
                        return;
                    }

                    self.push_resolve_error(err, &path.range, generics_token);
                }
            }
        }
    }
}

impl Handler for CreateReference<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CreateReference<'_> {
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match symbol_table::resolve(arg) {
                Ok(symbol) => {
                    for id in symbol.full_path {
                        symbol_table::add_reference(id, &arg.identifier.identifier_token.token);
                    }
                }
                Err(err) => {
                    // hierarchical identifier is used for:
                    //  - LHS of assign declaratoin
                    //  - identifier to specfy clock/reset in always_ff event list
                    // therefore, it should be known indentifer
                    // and we don't have to consider it is anonymous

                    // TODO check SV-side member to suppress error
                    self.push_resolve_error(err, &arg.into(), None);
                }
            }
        }
        Ok(())
    }

    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if !self.is_anonymous_identifier {
                let ident = arg.identifier().token;
                let path: GenericSymbolPath = arg.into();
                let namespace = namespace_table::get(ident.id).unwrap();

                self.generic_symbol_path(&path, &namespace, None);
            }
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

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let Ok(symbol) = symbol_table::resolve(arg.scoped_identifier.as_ref()) {
                    match symbol.found.kind {
                        SymbolKind::Module(x) => self.inst_ports.extend(x.ports),
                        SymbolKind::GenericParameter(x) => {
                            if let GenericBoundKind::Proto(ref prot) = x.bound {
                                if let SymbolKind::ProtoModule(prot) =
                                    symbol_table::resolve((prot, &symbol.found.namespace))
                                        .unwrap()
                                        .found
                                        .kind
                                {
                                    self.inst_ports.extend(prot.ports);
                                }
                            }
                        }
                        SymbolKind::SystemVerilog => self.inst_sv_module = true,
                        _ => {}
                    }
                }
            }
            HandlerPoint::After => {
                self.inst_ports.clear();
                self.inst_sv_module = false;
            }
        }
        Ok(())
    }

    fn inst_port_item(&mut self, arg: &InstPortItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let Some(ref x) = arg.inst_port_item_opt {
                    if let Some(port) = self
                        .inst_ports
                        .iter()
                        .find(|x| x.name == arg.identifier.identifier_token.token.text)
                    {
                        if let SymbolKind::Port(port) = symbol_table::get(port.symbol).unwrap().kind
                        {
                            self.is_anonymous_identifier = port.direction == Direction::Output
                                && is_anonymous_expression(&x.expression);
                        }
                    } else if self.inst_sv_module {
                        // For SV module, any ports can be connected with anonymous identifier
                        self.is_anonymous_identifier = is_anonymous_expression(&x.expression);
                    }
                } else {
                    // implicit port connection by name
                    match symbol_table::resolve(arg.identifier.as_ref()) {
                        Ok(symbol) => {
                            for id in symbol.full_path {
                                symbol_table::add_reference(
                                    id,
                                    &arg.identifier.identifier_token.token,
                                );
                            }
                        }
                        Err(err) => {
                            self.push_resolve_error(err, &arg.identifier.as_ref().into(), None);
                        }
                    }
                }
            }
            HandlerPoint::After => self.is_anonymous_identifier = false,
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
