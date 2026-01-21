use crate::AnalyzerError;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{Direction, GenericMap, Symbol, SymbolKind};
use crate::symbol_path::{GenericSymbolPath, SymbolPath, SymbolPathNamespace};
use crate::symbol_table;
use crate::symbol_table::{ResolveError, ResolveErrorCause};
use std::cell::RefCell;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::{
    ExpressionIdentifier, GenericArgIdentifier, HierarchicalIdentifier, Identifier,
    InstParameterItem, InstPortItem, ModportItem, ScopedIdentifier, StructConstructorItem,
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
        in_import_declaration: bool,
    },
    ExpressionIdentifier {
        arg: ExpressionIdentifier,
        namespace: Namespace,
    },
    GenericArgIdentifier {
        arg: GenericArgIdentifier,
        namespace: Namespace,
    },
    ModportItem {
        arg: ModportItem,
        namespace: Namespace,
    },
    InstParameterItem {
        arg: InstParameterItem,
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
        arg: ExpressionIdentifier,
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

impl From<(&ScopedIdentifier, bool)> for ReferenceCandidate {
    fn from(value: (&ScopedIdentifier, bool)) -> Self {
        Self::ScopedIdentifier {
            arg: value.0.clone(),
            namespace: namespace_table::get_default(),
            in_import_declaration: value.1,
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

impl From<&GenericArgIdentifier> for ReferenceCandidate {
    fn from(value: &GenericArgIdentifier) -> Self {
        Self::GenericArgIdentifier {
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

impl From<&InstParameterItem> for ReferenceCandidate {
    fn from(value: &InstParameterItem) -> Self {
        Self::InstParameterItem {
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
        generics_token: Option<&Token>,
        struct_token: Option<&Token>,
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
                        let member = format!("{not_found}");
                        self.errors
                            .push(AnalyzerError::unknown_member(&name, &member, token));
                    }
                }
                ResolveErrorCause::Private => {
                    if matches!(last_found.kind, SymbolKind::Namespace) {
                        self.errors
                            .push(AnalyzerError::private_namespace(&name, token));
                    } else {
                        self.errors
                            .push(AnalyzerError::private_member(&name, token));
                    }
                }
                ResolveErrorCause::Invisible => {
                    self.errors
                        .push(AnalyzerError::invisible_identifier(&name, token));
                }
            }
        } else if let ResolveErrorCause::NotFound(not_found) = err.cause {
            let name = format!("{not_found}");
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
                self.errors.push(AnalyzerError::referring_before_definition(
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
        in_import_declaration: bool,
        generics_token: Option<&Token>,
        generic_maps: Option<&Vec<GenericMap>>,
    ) {
        let mut path = path.clone();

        let orig_len = path.len();
        path.resolve_imported(namespace, generic_maps);

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

                    if in_import_declaration
                        && !params.is_empty()
                        && matches!(
                            symbol.found.kind,
                            SymbolKind::Function(_) | SymbolKind::Struct(_) | SymbolKind::Union(_)
                        )
                    {
                        // Generic function, struct and union should be imorted as-is
                        // but not as thier instances.
                        // https://github.com/veryl-lang/veryl/issues/1619
                        if n_args != 0 {
                            self.errors.push(AnalyzerError::invalid_import(&path.range))
                        }
                        continue;
                    }

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

                    if (params.len() + n_args) == 0 {
                        continue;
                    }

                    let target_symbol = symbol.found;

                    let mut args: Vec<_> = path.paths[i].arguments.drain(0..).collect();
                    for param in params.iter().skip(n_args) {
                        //  apply default value
                        args.push(param.1.default_value.as_ref().unwrap().clone());
                    }

                    for arg in args.iter_mut() {
                        arg.unalias();
                        arg.append_namespace_path(namespace, &target_symbol.namespace);
                    }

                    path.paths[i].arguments.append(&mut args);
                    if path.is_generic_reference() {
                        Self::add_generic_reference(&target_symbol, namespace, &path, i);
                    } else {
                        Self::insert_generic_instance(&path, i, namespace, &target_symbol, None);
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

    fn insert_generic_instance(
        path: &GenericSymbolPath,
        ith: usize,
        namespace: &Namespace,
        target: &Symbol,
        affiliation_symbol: Option<&Symbol>,
    ) {
        let instance_path = &path.paths[ith];
        let Some((token, symbol)) = instance_path.get_generic_instance(target, affiliation_symbol)
        else {
            return;
        };

        if let Some(ref id) = symbol_table::insert(&token, symbol.clone()) {
            symbol_table::add_generic_instance(target.id, *id);
        }

        let map = vec![GenericMap {
            id: None,
            map: target.generic_table(&instance_path.arguments),
        }];

        let target_namespace = &target.inner_namespace();
        let mut referecnes = target.generic_references();
        for path in &mut referecnes {
            // check recursive reference
            if path.paths[0].base.text == target.token.text {
                continue;
            }

            path.apply_map(&map);
            path.unalias();
            path.append_namespace_path(namespace, &target.namespace);

            if let Ok(target) = symbol_table::resolve((&path.generic_path(), target_namespace)) {
                let ith = path.len() - 1;
                Self::insert_generic_instance(
                    path,
                    ith,
                    target_namespace,
                    &target.found,
                    Some(&symbol),
                );
            }
        }
    }

    fn add_generic_reference(
        symbol: &Symbol,
        namespace: &Namespace,
        path: &GenericSymbolPath,
        ith: usize,
    ) {
        fn get_parent_generic_component(namespace: &Namespace) -> Option<Symbol> {
            let target = namespace.get_symbol()?;
            if target.has_generic_paramters() {
                Some(target)
            } else {
                get_parent_generic_component(&target.namespace)
            }
        }

        let mut namespace = namespace.clone();
        namespace.strip_anonymous_path();

        let Some(mut target) = get_parent_generic_component(&namespace) else {
            return;
        };
        let path = path.slice(ith);

        let generic_maps = target.generic_maps();
        for map in generic_maps {
            // existing generic maps means that the target symbol has
            // already been processed.
            // For this case, need to insert generic instance generated from
            // the given path explicitly.
            let affiliation_symbol = map.id.map(|id| symbol_table::get(id).unwrap());
            let mut path = path.clone();
            let ith = path.len() - 1;
            path.apply_map(&[map]);
            path.append_namespace_path(&namespace, &symbol.namespace);

            Self::insert_generic_instance(
                &path,
                ith,
                &namespace,
                symbol,
                affiliation_symbol.as_ref(),
            );
        }

        let kind = match target.kind {
            SymbolKind::Function(mut x) => {
                x.generic_references.push(path);
                SymbolKind::Function(x)
            }
            SymbolKind::Module(mut x) => {
                x.generic_references.push(path);
                SymbolKind::Module(x)
            }
            SymbolKind::Interface(mut x) => {
                x.generic_references.push(path);
                SymbolKind::Interface(x)
            }
            SymbolKind::Package(mut x) => {
                x.generic_references.push(path);
                SymbolKind::Package(x)
            }
            SymbolKind::Struct(mut x) => {
                x.generic_references.push(path);
                SymbolKind::Struct(x)
            }
            SymbolKind::Union(mut x) => {
                x.generic_references.push(path);
                SymbolKind::Union(x)
            }
            _ => return,
        };

        target.kind = kind;
        symbol_table::update(target);
    }

    fn check_simple_identifier(
        &mut self,
        path: &SymbolPathNamespace,
        default_namespace: Option<&Namespace>,
        token: &TokenRange,
        struct_token: Option<&Token>,
    ) {
        if let Some(default_namespace) = default_namespace {
            namespace_table::set_default(&default_namespace.paths);
        }

        match symbol_table::resolve(path) {
            Ok(symbol) => {
                for id in symbol.full_path {
                    symbol_table::add_reference(id, &token.beg);
                }
            }
            Err(err) => {
                self.push_resolve_error(err, token, None, struct_token);
            }
        }
    }

    fn check_complex_identifier(
        &mut self,
        path: &GenericSymbolPath,
        default_namespace: &Namespace,
        token: &Token,
        in_import_declaration: bool,
    ) {
        namespace_table::set_default(&default_namespace.paths);
        let namespace = namespace_table::get(token.id).unwrap();
        self.generic_symbol_path(path, &namespace, in_import_declaration, None, None);
    }

    pub fn apply(&mut self) -> Vec<AnalyzerError> {
        let candidates: Vec<_> = self.candidates.drain(0..).collect();

        for x in &candidates {
            match x {
                ReferenceCandidate::Identifier { arg, namespace } => {
                    self.check_simple_identifier(&arg.into(), Some(namespace), &arg.into(), None);
                }
                ReferenceCandidate::HierarchicalIdentifier { arg, namespace } => {
                    let token = arg.identifier.identifier_token.token;
                    self.check_complex_identifier(&arg.into(), namespace, &token, false);
                }
                ReferenceCandidate::ScopedIdentifier {
                    arg,
                    namespace,
                    in_import_declaration,
                } => {
                    let token = arg.identifier().token;
                    self.check_complex_identifier(
                        &arg.into(),
                        namespace,
                        &token,
                        *in_import_declaration,
                    );
                }
                ReferenceCandidate::ExpressionIdentifier { arg, namespace } => {
                    let token = arg.scoped_identifier.identifier().token;
                    self.check_complex_identifier(&arg.into(), namespace, &token, false);
                }
                ReferenceCandidate::GenericArgIdentifier { arg, namespace } => {
                    let token = arg.scoped_identifier.identifier().token;
                    self.check_complex_identifier(&arg.into(), namespace, &token, false);
                }
                ReferenceCandidate::ModportItem { arg, namespace } => {
                    let mut path: SymbolPathNamespace = arg.identifier.as_ref().into();
                    path.pop_namespace();
                    self.check_simple_identifier(&path, Some(namespace), &arg.into(), None);
                }
                ReferenceCandidate::InstParameterItem { arg, namespace } => {
                    if arg.inst_parameter_item_opt.is_none() {
                        // implicit port connection by name
                        let identifier = arg.identifier.as_ref();
                        self.check_simple_identifier(
                            &identifier.into(),
                            Some(namespace),
                            &identifier.into(),
                            None,
                        );
                    }
                }
                ReferenceCandidate::InstPortItem { arg, namespace } => {
                    if arg.inst_port_item_opt.is_none() {
                        // implicit port connection by name
                        let identifier = arg.identifier.as_ref();
                        self.check_simple_identifier(
                            &identifier.into(),
                            Some(namespace),
                            &identifier.into(),
                            None,
                        );
                    }
                }
                ReferenceCandidate::StructConstructorItem { arg, r#type } => {
                    if let Ok(symbol) = symbol_table::resolve(r#type)
                        && !matches!(symbol.found.kind, SymbolKind::SystemVerilog)
                    {
                        let identifier = arg.identifier.as_ref();

                        let namespace = Self::get_struct_namespace(&symbol.found);
                        let path: SymbolPathNamespace = (identifier, &namespace).into();

                        self.check_simple_identifier(
                            &path,
                            None,
                            &identifier.into(),
                            Some(&symbol.found.token),
                        );
                    }
                }
                ReferenceCandidate::NamedArgument { arg, function } => {
                    if let Ok(symbol) = symbol_table::resolve(function) {
                        let func_symbol =
                            if let SymbolKind::ModportFunctionMember(x) = symbol.found.kind {
                                symbol_table::get(x.function).unwrap()
                            } else {
                                symbol.found
                            };
                        let namespace = func_symbol.inner_namespace();
                        let path: SymbolPath = arg.into();
                        let path: SymbolPathNamespace = (&path, &namespace).into();
                        self.check_simple_identifier(
                            &path,
                            None,
                            &arg.into(),
                            Some(&func_symbol.token),
                        );
                    }
                }
            }
        }

        self.errors.drain(0..).collect()
    }

    fn get_struct_namespace(symbol: &Symbol) -> Namespace {
        match &symbol.kind {
            SymbolKind::TypeDef(x) => {
                let namespace = Some(&symbol.namespace);
                if let Some((_, Some(symbol))) = x.r#type.trace_user_defined(namespace) {
                    return Self::get_struct_namespace(&symbol);
                }
            }
            SymbolKind::ProtoTypeDef(x) => {
                if let Some(r#type) = &x.r#type {
                    let namespace = Some(&symbol.namespace);
                    if let Some((_, Some(symbol))) = r#type.trace_user_defined(namespace) {
                        return Self::get_struct_namespace(&symbol);
                    }
                }
            }
            _ => {}
        }
        symbol.inner_namespace()
    }
}

thread_local!(static REFERENCE_TABLE: RefCell<ReferenceTable> = RefCell::new(ReferenceTable::new()));

pub fn add(cand: ReferenceCandidate) {
    REFERENCE_TABLE.with(|f| f.borrow_mut().add(cand))
}

pub fn apply() -> Vec<AnalyzerError> {
    REFERENCE_TABLE.with(|f| f.borrow_mut().apply())
}
