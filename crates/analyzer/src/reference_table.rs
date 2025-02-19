use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{Direction, GenericMap, Symbol, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use crate::symbol_table::{ResolveError, ResolveErrorCause};
use crate::AnalyzerError;
use std::cell::RefCell;
use std::collections::HashMap;
use veryl_parser::veryl_grammar_trait::{
    ExpressionIdentifier, HierarchicalIdentifier, InstPortItem, ModportItem, ScopedIdentifier,
};
use veryl_parser::veryl_token::{is_anonymous_text, Token, TokenRange, TokenSource};

#[derive(Clone, Debug)]
pub enum ReferenceCandidate {
    HierarchicalIdentifier {
        arg: HierarchicalIdentifier,
        source: TokenSource,
    },
    ScopedIdentifier {
        arg: ScopedIdentifier,
        source: TokenSource,
    },
    ExpressionIdentifier {
        arg: ExpressionIdentifier,
        source: TokenSource,
    },
    ModportItem {
        arg: ModportItem,
        source: TokenSource,
    },
    InstPortItem {
        arg: InstPortItem,
        source: TokenSource,
    },
}

impl ReferenceCandidate {
    fn get_source(&self) -> TokenSource {
        match self {
            ReferenceCandidate::HierarchicalIdentifier { source, .. } => *source,
            ReferenceCandidate::ScopedIdentifier { source, .. } => *source,
            ReferenceCandidate::ExpressionIdentifier { source, .. } => *source,
            ReferenceCandidate::ModportItem { source, .. } => *source,
            ReferenceCandidate::InstPortItem { source, .. } => *source,
        }
    }
}

impl From<&HierarchicalIdentifier> for ReferenceCandidate {
    fn from(value: &HierarchicalIdentifier) -> Self {
        Self::HierarchicalIdentifier {
            arg: value.clone(),
            source: value.identifier.identifier_token.token.source,
        }
    }
}

impl From<&ScopedIdentifier> for ReferenceCandidate {
    fn from(value: &ScopedIdentifier) -> Self {
        Self::ScopedIdentifier {
            arg: value.clone(),
            source: value.identifier().token.source,
        }
    }
}

impl From<&ExpressionIdentifier> for ReferenceCandidate {
    fn from(value: &ExpressionIdentifier) -> Self {
        Self::ExpressionIdentifier {
            arg: value.clone(),
            source: value.identifier().token.source,
        }
    }
}

impl From<&ModportItem> for ReferenceCandidate {
    fn from(value: &ModportItem) -> Self {
        Self::ModportItem {
            arg: value.clone(),
            source: value.identifier.identifier_token.token.source,
        }
    }
}

impl From<&InstPortItem> for ReferenceCandidate {
    fn from(value: &InstPortItem) -> Self {
        Self::InstPortItem {
            arg: value.clone(),
            source: value.identifier.identifier_token.token.source,
        }
    }
}

#[derive(Default, Debug)]
pub struct ReferenceTable {
    candidates: Vec<ReferenceCandidate>,
    text_table: HashMap<TokenSource, String>,
    errors: Vec<AnalyzerError>,
}

impl ReferenceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, cand: ReferenceCandidate, text: &str) {
        let source = cand.get_source();
        self.candidates.push(cand);
        self.text_table
            .entry(source)
            .or_insert_with(|| text.to_string());
    }

    fn push_resolve_error(
        &mut self,
        source: &TokenSource,
        err: ResolveError,
        token: &TokenRange,
        generics_token: Option<Token>,
    ) {
        if let Some(last_found) = err.last_found {
            let name = last_found.token.to_string();
            let text = self.text_table.get(source).unwrap();
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
                            .push(AnalyzerError::unknown_member(&name, &member, text, token));
                    }
                }
                ResolveErrorCause::Private => {
                    self.errors
                        .push(AnalyzerError::private_member(&name, text, token));
                }
            }
        } else if let ResolveErrorCause::NotFound(not_found) = err.cause {
            let name = format!("{}", not_found);
            let text = self.text_table.get(source).unwrap();
            if let Some(generics_token) = generics_token {
                self.errors
                    .push(AnalyzerError::unresolvable_generic_argument(
                        &name,
                        text,
                        token,
                        &generics_token.into(),
                    ));
            } else if is_anonymous_text(not_found) {
                //self.errors
                //    .push(AnalyzerError::anonymous_identifier_usage(text, token));
            } else {
                self.errors
                    .push(AnalyzerError::undefined_identifier(&name, text, token));
            }
        } else {
            unreachable!();
        }
    }

    fn check_pacakge_reference(
        &mut self,
        source: &TokenSource,
        symbol: &Symbol,
        token_range: &TokenRange,
    ) {
        if !matches!(symbol.kind, SymbolKind::Package(_)) {
            return;
        }

        let base_token = token_range.end;
        let package_token = symbol.token;
        if let (TokenSource::File(package_file), TokenSource::File(base_file)) =
            (package_token.source, base_token.source)
        {
            let referecne_before_definition = package_file == base_file
                && (package_token.line > base_token.line
                    || package_token.line == base_token.line
                        && package_token.column > base_token.column);
            if referecne_before_definition {
                let text = self.text_table.get(source).unwrap();
                self.errors
                    .push(AnalyzerError::referring_package_before_definition(
                        &package_token.to_string(),
                        text,
                        token_range,
                    ));
            }
        }
    }

    fn generic_symbol_path(
        &mut self,
        source: &TokenSource,
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
                    self.check_pacakge_reference(source, &symbol.found, &path.range);
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
                        let text = self.text_table.get(source).unwrap();
                        self.errors.push(AnalyzerError::mismatch_generics_arity(
                            &path.paths[i].base.to_string(),
                            params.len(),
                            n_args,
                            text,
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
                                source,
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

                    self.push_resolve_error(source, err, &path.range, generics_token);
                }
            }
        }
    }

    pub fn apply(&mut self) -> Vec<AnalyzerError> {
        let candidates: Vec<_> = self.candidates.drain(0..).collect();

        for x in &candidates {
            match x {
                ReferenceCandidate::HierarchicalIdentifier { arg, source } => {
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
                            self.push_resolve_error(source, err, &arg.into(), None);
                        }
                    }
                }
                ReferenceCandidate::ScopedIdentifier { arg, source } => {
                    let ident = arg.identifier().token;
                    let path: GenericSymbolPath = arg.into();
                    let namespace = namespace_table::get(ident.id).unwrap();

                    self.generic_symbol_path(source, &path, &namespace, None);
                }
                ReferenceCandidate::ExpressionIdentifier { arg, source } => {
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
                                self.push_resolve_error(source, err, &arg.into(), None);
                            }
                        }
                    }
                }
                ReferenceCandidate::ModportItem { arg, source } => {
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
                                source,
                                err,
                                &arg.identifier.as_ref().into(),
                                None,
                            );
                        }
                    }
                }
                ReferenceCandidate::InstPortItem { arg, source } => {
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
                                    source,
                                    err,
                                    &arg.identifier.as_ref().into(),
                                    None,
                                );
                            }
                        }
                    }
                }
            }
        }

        self.text_table.clear();
        self.errors.drain(0..).collect()
    }
}

thread_local!(static REFERENCE_TABLE: RefCell<ReferenceTable> = RefCell::new(ReferenceTable::new()));

pub fn add(cand: ReferenceCandidate, text: &str) {
    REFERENCE_TABLE.with(|f| f.borrow_mut().add(cand, text))
}

pub fn apply() -> Vec<AnalyzerError> {
    REFERENCE_TABLE.with(|f| f.borrow_mut().apply())
}
