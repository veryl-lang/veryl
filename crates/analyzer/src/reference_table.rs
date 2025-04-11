use crate::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{Direction, GenericMap, Symbol, SymbolKind};
use crate::symbol_path::{GenericSymbol, GenericSymbolPath, SymbolPath, SymbolPathNamespace};
use crate::symbol_table;
use crate::symbol_table::{ResolveError, ResolveErrorCause};
use std::cell::RefCell;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::{
    ExpressionIdentifier, HierarchicalIdentifier, Identifier, InstPortItem, ModportItem,
    ScopedIdentifier, StructConstructorItem,
};
use veryl_parser::veryl_token::{Token, TokenSource, is_anonymous_text};

#[derive(Clone, Debug)]
pub enum ReferenceCandidate {
    Identifier {
        arg: Identifier,
        namespace: Namespace,
    },
    HierarchicalIdentifier {
        arg: HierarchicalIdentifier,
        namespace: Namespace,
    },
    ScopedIdentifier {
        arg: ScopedIdentifier,
        namespace: Namespace,
    },
    ExpressionIdentifier {
        arg: ExpressionIdentifier,
        namespace: Namespace,
    },
    ModportItem {
        arg: ModportItem,
        namespace: Namespace,
    },
    InstPortItem {
        arg: InstPortItem,
        namespace: Namespace,
    },
    StructConstructorItem {
        arg: StructConstructorItem,
        r#type: ExpressionIdentifier,
    },
    NamedArgument {
        arg: ScopedIdentifier,
        function: ExpressionIdentifier,
    },
}

impl From<&Identifier> for ReferenceCandidate {
    fn from(value: &Identifier) -> Self {
        Self::Identifier {
            arg: value.clone(),
            namespace: namespace_table::get_default(),
        }
    }
}

impl From<&HierarchicalIdentifier> for ReferenceCandidate {
    fn from(value: &HierarchicalIdentifier) -> Self {
        Self::HierarchicalIdentifier {
            arg: value.clone(),
            namespace: namespace_table::get_default(),
        }
    }
}

impl From<&ScopedIdentifier> for ReferenceCandidate {
    fn from(value: &ScopedIdentifier) -> Self {
        Self::ScopedIdentifier {
            arg: value.clone(),
            namespace: namespace_table::get_default(),
        }
    }
}

impl From<&ExpressionIdentifier> for ReferenceCandidate {
    fn from(value: &ExpressionIdentifier) -> Self {
        Self::ExpressionIdentifier {
            arg: value.clone(),
            namespace: namespace_table::get_default(),
        }
    }
}

impl From<&ModportItem> for ReferenceCandidate {
    fn from(value: &ModportItem) -> Self {
        Self::ModportItem {
            arg: value.clone(),
            namespace: namespace_table::get_default(),
        }
    }
}

impl From<&InstPortItem> for ReferenceCandidate {
    fn from(value: &InstPortItem) -> Self {
        Self::InstPortItem {
            arg: value.clone(),
            namespace: namespace_table::get_default(),
        }
    }
}

#[derive(Default, Debug)]
pub struct ReferenceTable {
    candidates: Vec<ReferenceCandidate>,
    errors: Vec<AnalyzerError>,
}

impl ReferenceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, cand: ReferenceCandidate) {
        self.candidates.push(cand);
    }

    fn push_resolve_error(
        &mut self,
        err: ResolveError,
        token: &TokenRange,
        generics_token: Option<Token>,
        struct_token: Option<Token>,
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
                        self.errors
                            .push(AnalyzerError::unknown_member(&name, &member, token));
                    }
                }
                ResolveErrorCause::Private => {
                    self.errors
                        .push(AnalyzerError::private_member(&name, token));
                }
                ResolveErrorCause::Invisible => {
                    self.errors
                        .push(AnalyzerError::invisible_identifier(&name, token));
                }
            }
        } else if let ResolveErrorCause::NotFound(not_found) = err.cause {
            let name = format!("{}", not_found);
            if let Some(generics_token) = generics_token {
                self.errors
                    .push(AnalyzerError::unresolvable_generic_argument(
                        &name,
                        token,
                        &generics_token.into(),
                    ));
            } else if is_anonymous_text(not_found) {
                // AnonymousIdentifierUsage is handled at create_type_dag
            } else if let Some(struct_token) = struct_token {
                self.errors.push(AnalyzerError::unknown_member(
                    &struct_token.text.to_string(),
                    &name,
                    token,
                ));
            } else {
                self.errors
                    .push(AnalyzerError::undefined_identifier(&name, token));
            }
        } else {
            unreachable!();
        }
    }

    fn check_pacakge_reference(&mut self, symbol: &Symbol, token_range: &TokenRange) {
        if !matches!(symbol.kind, SymbolKind::Package(_)) {
            return;
        }

        let base_token = token_range.end;
        let package_token = symbol.token;
        if let (
            TokenSource::File {
                path: package_file, ..
            },
            TokenSource::File {
                path: base_file, ..
            },
        ) = (package_token.source, base_token.source)
        {
            let referecne_before_definition = package_file == base_file
                && (package_token.line > base_token.line
                    || package_token.line == base_token.line
                        && package_token.column > base_token.column);
            if referecne_before_definition {
                self.errors
                    .push(AnalyzerError::referring_package_before_definition(
                        &package_token.to_string(),
                        token_range,
                    ));
            }
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

        let orig_len = path.len();
        let mut path = path.clone();
        path.resolve_imported(namespace);

        // Prefix paths added by `resolve_imported` have already been resolved.
        // They should be skipped.
        let prefix_len = path.len() - orig_len;
        for i in prefix_len..path.len() {
            let base_path = path.base_path(i);
            match symbol_table::resolve((&base_path, namespace)) {
                Ok(symbol) => {
                    self.check_pacakge_reference(&symbol.found, &path.range);
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
                            &path.range,
                        ));
                        continue;
                    }

                    let mut args = path.paths[i].arguments.clone();
                    for param in params.iter().skip(n_args) {
                        //  apply default value
                        args.push(param.1.default_value.as_ref().unwrap().clone());
                    }

                    for (j, arg) in args.iter_mut().enumerate() {
                        if let Ok(symbol) = symbol_table::resolve((&arg.mangled_path(), namespace))
                        {
                            // Replace arg with its target if arg is alias
                            if let Some(target) = symbol.found.alias_target() {
                                path.paths[i].replace_generic_argument(j, target.clone());
                                *arg = target;
                            }
                        }
                    }

                    let path = GenericSymbol {
                        base: path.paths[i].base,
                        arguments: args,
                    };
                    if let Some((token, new_symbol)) = path.get_generic_instance(&symbol.found) {
                        if let Some(ref x) = symbol_table::insert(&token, new_symbol) {
                            symbol_table::add_generic_instance(symbol.found.id, *x);
                        }

                        let table = symbol.found.generic_table(&path.arguments);
                        let map = vec![GenericMap {
                            name: "".to_string(),
                            id: None,
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
                    self.push_resolve_error(err, &path.range, generics_token, None);
                }
            }
        }
    }

    pub fn apply(&mut self) -> Vec<AnalyzerError> {
        let candidates: Vec<_> = self.candidates.drain(0..).collect();

        for x in &candidates {
            match x {
                ReferenceCandidate::Identifier { arg, namespace } => {
                    namespace_table::set_default(&namespace.paths);

                    match symbol_table::resolve(arg) {
                        Ok(symbol) => {
                            for id in symbol.full_path {
                                symbol_table::add_reference(id, &arg.identifier_token.token);
                            }
                        }
                        Err(err) => {
                            self.push_resolve_error(err, &arg.into(), None, None);
                        }
                    }
                }
                ReferenceCandidate::HierarchicalIdentifier { arg, namespace } => {
                    namespace_table::set_default(&namespace.paths);

                    match symbol_table::resolve(arg) {
                        Ok(symbol) => {
                            for id in symbol.full_path {
                                symbol_table::add_reference(
                                    id,
                                    &arg.identifier.identifier_token.token,
                                );
                            }
                        }
                        Err(err) => {
                            // hierarchical identifier is used for:
                            //  - LHS of assign declaratoin
                            //  - identifier to specfy clock/reset in always_ff event list
                            // therefore, it should be known indentifer
                            // and we don't have to consider it is anonymous

                            // TODO check SV-side member to suppress error
                            self.push_resolve_error(err, &arg.into(), None, None);
                        }
                    }
                }
                ReferenceCandidate::ScopedIdentifier { arg, namespace } => {
                    namespace_table::set_default(&namespace.paths);

                    let ident = arg.identifier().token;
                    let path: GenericSymbolPath = arg.into();
                    let namespace = namespace_table::get(ident.id).unwrap();

                    self.generic_symbol_path(&path, &namespace, None);
                }
                ReferenceCandidate::ExpressionIdentifier { arg, namespace } => {
                    namespace_table::set_default(&namespace.paths);

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
                                self.push_resolve_error(err, &arg.into(), None, None);
                            }
                        }
                    }
                }
                ReferenceCandidate::ModportItem { arg, namespace } => {
                    namespace_table::set_default(&namespace.paths);

                    let mut path: SymbolPathNamespace = arg.identifier.as_ref().into();
                    path.pop_namespace();

                    match symbol_table::resolve(path) {
                        Ok(symbol) => {
                            for id in symbol.full_path {
                                symbol_table::add_reference(
                                    id,
                                    &arg.identifier.identifier_token.token,
                                );
                            }
                        }
                        Err(err) => {
                            self.push_resolve_error(
                                err,
                                &arg.identifier.as_ref().into(),
                                None,
                                None,
                            );
                        }
                    }
                }
                ReferenceCandidate::InstPortItem { arg, namespace } => {
                    namespace_table::set_default(&namespace.paths);

                    if arg.inst_port_item_opt.is_none() {
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
                                self.push_resolve_error(
                                    err,
                                    &arg.identifier.as_ref().into(),
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                }
                ReferenceCandidate::StructConstructorItem { arg, r#type } => {
                    if let Ok(symbol) = symbol_table::resolve(r#type) {
                        let namespace = symbol.found.inner_namespace();
                        let symbol_path: SymbolPath = arg.identifier.as_ref().into();

                        match symbol_table::resolve((&symbol_path, &namespace)) {
                            Ok(symbol) => {
                                for id in symbol.full_path {
                                    symbol_table::add_reference(
                                        id,
                                        &arg.identifier.identifier_token.token,
                                    );
                                }
                            }
                            Err(err) => {
                                self.push_resolve_error(
                                    err,
                                    &arg.identifier.as_ref().into(),
                                    None,
                                    Some(symbol.found.token),
                                );
                            }
                        }
                    }
                }
                ReferenceCandidate::NamedArgument { arg, function } => {
                    if let Ok(symbol) = symbol_table::resolve(function) {
                        let namespace = symbol.found.inner_namespace();
                        let symbol_path: SymbolPath = arg.into();

                        match symbol_table::resolve((&symbol_path, &namespace)) {
                            Ok(symbol) => {
                                for id in symbol.full_path {
                                    symbol_table::add_reference(id, &arg.identifier().token);
                                }
                            }
                            Err(err) => {
                                self.push_resolve_error(
                                    err,
                                    &arg.into(),
                                    None,
                                    Some(symbol.found.token),
                                );
                            }
                        }
                    }
                }
            }
        }

        self.errors.drain(0..).collect()
    }
}

thread_local!(static REFERENCE_TABLE: RefCell<ReferenceTable> = RefCell::new(ReferenceTable::new()));

pub fn add(cand: ReferenceCandidate) {
    REFERENCE_TABLE.with(|f| f.borrow_mut().add(cand))
}

pub fn apply() -> Vec<AnalyzerError> {
    REFERENCE_TABLE.with(|f| f.borrow_mut().apply())
}
