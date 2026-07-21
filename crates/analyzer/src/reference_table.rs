use crate::AnalyzerError;
use crate::generic_inference_table;
use crate::namespace::{DefineContext, Namespace};
use crate::scope;
use crate::symbol::{Direction, GenericMap, Symbol, SymbolId, SymbolKind, TbComponentKind};
use crate::symbol_path::{GenericSymbol, GenericSymbolPath, SymbolPath, SymbolPathNamespace};
use crate::symbol_table;
use crate::symbol_table::{ResolveError, ResolveErrorCause};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use veryl_parser::resource_table::TokenId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::{
    ExpressionIdentifier, GenericArgIdentifier, HierarchicalIdentifier, Identifier,
    InstParameterItem, InstPortItem, ModportItem, MultipleImportItem, ScopedIdentifier,
    StructConstructorItem,
};
use veryl_parser::veryl_token::{Token, TokenSource, is_anonymous_text};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ReferenceCandidate {
    Identifier {
        arg: Identifier,
    },
    HierarchicalIdentifier {
        arg: HierarchicalIdentifier,
    },
    ScopedIdentifier {
        arg: ScopedIdentifier,
        in_import_declaration: bool,
    },
    ExpressionIdentifier {
        arg: ExpressionIdentifier,
    },
    GenericArgIdentifier {
        arg: GenericArgIdentifier,
    },
    ImportItem {
        base: ScopedIdentifier,
        arg: Identifier,
    },
    ModportItem {
        arg: ModportItem,
    },
    InstParameterItem {
        arg: InstParameterItem,
    },
    InstPortItem {
        arg: InstPortItem,
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
        Self::Identifier { arg: value.clone() }
    }
}

impl From<&HierarchicalIdentifier> for ReferenceCandidate {
    fn from(value: &HierarchicalIdentifier) -> Self {
        Self::HierarchicalIdentifier { arg: value.clone() }
    }
}

impl From<(&ScopedIdentifier, bool)> for ReferenceCandidate {
    fn from(value: (&ScopedIdentifier, bool)) -> Self {
        Self::ScopedIdentifier {
            arg: value.0.clone(),
            in_import_declaration: value.1,
        }
    }
}

impl From<&ExpressionIdentifier> for ReferenceCandidate {
    fn from(value: &ExpressionIdentifier) -> Self {
        Self::ExpressionIdentifier { arg: value.clone() }
    }
}

impl From<&GenericArgIdentifier> for ReferenceCandidate {
    fn from(value: &GenericArgIdentifier) -> Self {
        Self::GenericArgIdentifier { arg: value.clone() }
    }
}

impl From<(&ScopedIdentifier, &MultipleImportItem)> for ReferenceCandidate {
    fn from(value: (&ScopedIdentifier, &MultipleImportItem)) -> Self {
        let (base, item) = value;
        Self::ImportItem {
            base: base.clone(),
            arg: item.identifier.as_ref().clone(),
        }
    }
}

impl From<&ModportItem> for ReferenceCandidate {
    fn from(value: &ModportItem) -> Self {
        Self::ModportItem { arg: value.clone() }
    }
}

impl From<&InstParameterItem> for ReferenceCandidate {
    fn from(value: &InstParameterItem) -> Self {
        Self::InstParameterItem { arg: value.clone() }
    }
}

impl From<&InstPortItem> for ReferenceCandidate {
    fn from(value: &InstPortItem) -> Self {
        Self::InstPortItem { arg: value.clone() }
    }
}

#[derive(Default, Debug)]
pub struct ReferenceTable {
    candidates: Vec<ReferenceCandidate>,
    errors: Vec<AnalyzerError>,
    /// Alias symbols currently being expanded, to break cyclic alias chains.
    alias_stack: Vec<SymbolId>,
    /// Begin token of the expression-position candidate path currently
    /// being resolved. A hierarchical testbench reference is tolerated for
    /// that exact path only, not for aliases or generic arguments resolved
    /// recursively under it.
    testbench_hier_root: Option<TokenId>,
}

/// A path whose first segment is a module instance inside a `#[test]` module
/// is a hierarchical testbench reference.
fn is_testbench_hier_ref(
    path: &GenericSymbolPath,
    scope: scope::ScopeId,
    define_context: &DefineContext,
) -> bool {
    let Ok(symbol) = symbol_table::resolve_base_path(path, 0, (scope, define_context.clone()))
    else {
        return false;
    };
    if !matches!(symbol.found.kind, SymbolKind::Instance(_)) {
        return false;
    }
    if let Some(ns_symbol) = symbol_table::get_namespace_symbol(&symbol.found.namespace)
        && let SymbolKind::Module(x) = &ns_symbol.kind
    {
        x.test.is_some()
    } else {
        false
    }
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
                ResolveErrorCause::Ambiguous(ambiguous) => {
                    let name = format!("{ambiguous}");
                    self.errors
                        .push(AnalyzerError::ambiguous_identifier(&name, token));
                }
            }
        } else if let ResolveErrorCause::Ambiguous(ambiguous) = err.cause {
            let name = format!("{ambiguous}");
            self.errors
                .push(AnalyzerError::ambiguous_identifier(&name, token));
        } else if let ResolveErrorCause::NotFound(not_found) = err.cause {
            let name = format!("{not_found}");
            if let Some(generics_token) = generics_token {
                self.errors
                    .push(AnalyzerError::unresolvable_generic_expression(
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
        if !matches!(symbol.kind, SymbolKind::Package(ref x) if !x.is_proto) {
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
        scope: scope::ScopeId,
        define_context: &DefineContext,
        in_import_declaration: bool,
        generics_token: Option<&Token>,
        generic_maps: Option<&Vec<GenericMap>>,
    ) {
        let mut path = path.clone();
        let mut generic_maps = generic_maps.cloned().unwrap_or_default();

        let orig_len = path.len();
        path.resolve_imported(scope, define_context, Some(&generic_maps));

        // Prefix paths added by `resolve_imported` have already been resolved.
        // They should be skipped.
        let prefix_len = path.len() - orig_len;
        for i in prefix_len..path.len() {
            match symbol_table::resolve_base_path(&path, i, (scope, define_context.clone())) {
                Ok(symbol) => {
                    self.check_pacakge_reference(&symbol.found, &path.range);
                    symbol_table::add_reference(symbol.found.id, &path.paths[0].base);

                    // A user-defined component takes its parameters as
                    // generic arguments in the `var` form; they are
                    // validated against the interface manifest, not against
                    // generic parameter declarations (it has none). The
                    // builtin `$tb` components take no generic arguments and
                    // keep the arity check.
                    if matches!(
                        symbol.found.kind,
                        SymbolKind::TbComponent(ref x)
                            if matches!(x.kind, TbComponentKind::External(_))
                    ) {
                        continue;
                    }

                    // Check number of arguments
                    let params = symbol.found.generic_parameters();

                    let mut inference_attempted_failed = false;
                    if i + 1 == path.paths.len() {
                        use generic_inference_table::InferredApply;
                        match generic_inference_table::apply_inferred_args(&mut path, &symbol.found)
                        {
                            InferredApply::Missing => inference_attempted_failed = true,
                            InferredApply::Applied | InferredApply::NotApplicable => {}
                        }
                    }

                    let n_args = path.paths[i].arguments.len();

                    if in_import_declaration
                        && !params.is_empty()
                        && (matches!(
                            symbol.found.kind,
                            SymbolKind::Struct(_) | SymbolKind::Union(_)
                        ) || matches!(symbol.found.kind, SymbolKind::Function(ref x) if !x.is_proto))
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
                        if inference_attempted_failed {
                            self.errors.push(AnalyzerError::generic_inference_failed(
                                &path.paths[i].base.to_string(),
                                &path.range,
                            ));
                        } else {
                            self.errors.push(AnalyzerError::mismatch_generics_arity(
                                &path.paths[i].base.to_string(),
                                params.len(),
                                n_args,
                                &path.range,
                            ));
                        }
                        continue;
                    }

                    if let Some(mut alias_target) = symbol.found.alias_target(true)
                        && !self.alias_stack.contains(&symbol.found.id)
                    {
                        // If the alias is in a generic instance namespace, include its maps
                        // so generic parameters in the alias target can be correctly resolved.
                        let mut alias_maps = generic_maps.clone();
                        if let Some(ns_sym) =
                            symbol_table::get_namespace_symbol(&symbol.found.namespace)
                            && matches!(ns_sym.kind, SymbolKind::GenericInstance(_))
                        {
                            alias_maps.extend(ns_sym.generic_maps());
                        }
                        alias_target.apply_map(&alias_maps);
                        // remember this alias while expanding it (cyclic-alias guard)
                        self.alias_stack.push(symbol.found.id);
                        self.generic_symbol_path(
                            &alias_target,
                            scope::intern_namespace(&symbol.found.namespace),
                            &symbol.found.namespace.define_context,
                            false,
                            None,
                            Some(&alias_maps),
                        );
                        self.alias_stack.pop();
                    }

                    if (params.len() + n_args) == 0 {
                        if symbol.found.is_global_function() {
                            // A non-generic global function may call generic global functions.
                            // `insert_subordinate_generic_instances` is used to connect the current namespace
                            // and such functions.
                            let namespace = scope::namespace(scope, define_context);
                            Self::insert_subordinate_generic_instances(
                                &namespace,
                                &symbol.found,
                                None,
                                &generic_maps,
                                None,
                            );
                        }
                        continue;
                    }

                    let target_symbol = Rc::clone(&symbol.found);

                    // Namespace expansion below needs the query namespace; reconstruct
                    // it from the scope cursor only on this generic branch.
                    let namespace = scope::namespace(scope, define_context);

                    let mut args: Vec<_> = path.paths[i].arguments.drain(0..).collect();
                    for param in params.iter().skip(n_args) {
                        //  apply default value
                        args.push(param.1.default_value.as_ref().unwrap().clone());
                    }

                    for arg in args.iter_mut() {
                        // To ensure generic instances are emitted in the correct order,
                        // generic args must be processed before the base component is processed.
                        self.generic_symbol_path(arg, scope, define_context, false, None, None);

                        arg.unalias(None);
                        // Global function is emitted into the caller namespace.
                        // So namespace expansion is not needed.
                        if !target_symbol.is_global_function() {
                            arg.append_namespace_path(&namespace, &target_symbol.namespace);
                        }
                    }

                    path.paths[i].arguments.append(&mut args);
                    if path.is_generic_reference() {
                        Self::add_generic_reference(&target_symbol, &namespace, &path, i);
                    } else {
                        Self::insert_generic_instance(
                            &path,
                            i,
                            &namespace,
                            &target_symbol,
                            &mut generic_maps,
                            None,
                        );
                    }
                }
                Err(err) => {
                    let single_path = path.paths.len() == 1;
                    if single_path && !path.is_resolvable() {
                        return;
                    }

                    // Hierarchical testbench references (`dut.u_core.pc`) cross
                    // instance boundaries, which resolution reports as Invisible.
                    // They are validated by the IR conversion instead.
                    if self.testbench_hier_root == Some(path.range.beg.id)
                        && matches!(err.cause, ResolveErrorCause::Invisible)
                        && is_testbench_hier_ref(&path, scope, define_context)
                    {
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
        generic_maps: &mut Vec<GenericMap>,
        affiliation_symbol: Option<&Symbol>,
    ) {
        let instance_path = &path.paths[ith];
        let Some((token, symbol)) =
            Self::create_generic_instance(instance_path, target, namespace, affiliation_symbol)
        else {
            return;
        };

        if let Some(ref id) = symbol_table::insert(&token, symbol.clone()) {
            symbol_table::add_generic_instance(target.id, *id);
            // Register the instance in the structural identity index (queried by
            // `resolve_generic_structural`). `symbol.scope` is the instance's
            // enclosing scope, which encodes the parent instance for nested generics.
            symbol_table::index_generic_instance(
                symbol.scope,
                target.id,
                &instance_path.arguments,
                *id,
            );
            // Record the instance scope's owner so structural namespace
            // recovery can map an instance namespace segment back to its symbol
            // without resolve-by-name.
            let inst_inner = scope::inner_scope(symbol.scope, symbol.token.text);
            scope::set_kind_owner(inst_inner, scope::ScopeKind::Unknown, *id);
        } else {
            symbol_table::update_generic_instance_affiliation(target.id, &symbol);
        }

        // Register the instance's scope-tree node, whose member scope delegates
        // to the base template's — the delegation resolution navigates. The
        // mangled instance symbol persists only as the SV emission name and the
        // structural-index key, not as a resolution mechanism.
        scope::register_generic_instance(symbol.scope, target.token.text, symbol.token.text);

        generic_maps.push(GenericMap {
            id: None,
            map: target.generic_table(&instance_path.arguments),
        });
        Self::insert_subordinate_generic_instances(
            namespace,
            target,
            Some(&symbol),
            generic_maps,
            affiliation_symbol,
        );
    }

    fn create_generic_instance(
        symbol: &GenericSymbol,
        target: &Symbol,
        namespace: &Namespace,
        affiliation_symbol: Option<&Symbol>,
    ) -> Option<(Token, Symbol)> {
        if !target.is_global_function() {
            symbol.get_generic_instance(target, affiliation_symbol)
        } else if let Some(affiliation_symbol) = affiliation_symbol {
            if affiliation_symbol.is_component(true) {
                symbol.get_generic_instance(target, Some(affiliation_symbol))
            } else {
                symbol.get_generic_instance(
                    target,
                    affiliation_symbol.get_parent_component().as_ref(),
                )
            }
        } else if let Some(namespace_symbol) = symbol_table::get_namespace_symbol(namespace) {
            if namespace_symbol.is_component(true) {
                symbol.get_generic_instance(target, Some(&namespace_symbol))
            } else {
                symbol
                    .get_generic_instance(target, namespace_symbol.get_parent_component().as_ref())
            }
        } else {
            symbol.get_generic_instance(target, None)
        }
    }

    fn insert_subordinate_generic_instances(
        namespace: &Namespace,
        target: &Symbol,
        inst_symbol: Option<&Symbol>,
        generic_maps: &[GenericMap],
        affiliation_symbol: Option<&Symbol>,
    ) {
        let is_global_func = target.is_global_function();
        let mut subordinate_paths = if !is_global_func {
            target.generic_references()
        } else if let Some(paths) = symbol_table::get_reference_functions(target.id) {
            paths
        } else {
            return;
        };

        if subordinate_paths.is_empty() {
            return;
        }

        let target_namespace = &target.inner_namespace();
        for path in &mut subordinate_paths {
            // check recursive reference
            if path.paths[0].base.text == target.token.text {
                continue;
            }

            path.apply_map(generic_maps);
            path.unalias(None);
            path.append_namespace_path(namespace, &target.namespace);

            if let Ok(path_symbol) = symbol_table::resolve((&path.generic_path(), target_namespace))
            {
                let ith = path.len() - 1;
                if is_global_func {
                    Self::insert_generic_instance(
                        path,
                        ith,
                        namespace,
                        &path_symbol.found,
                        &mut generic_maps.to_vec(),
                        affiliation_symbol,
                    );
                } else {
                    Self::insert_generic_instance(
                        path,
                        ith,
                        target_namespace,
                        &path_symbol.found,
                        &mut generic_maps.to_vec(),
                        inst_symbol,
                    );
                }
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
            let target = symbol_table::get_namespace_symbol(namespace)?;
            if target.has_generic_paramters() || target.has_generic_consts() {
                Some(target)
            } else {
                get_parent_generic_component(&target.namespace)
            }
        }

        let mut namespace = namespace.clone();
        namespace.strip_anonymous_path();

        let mut target = if let Some(target) = get_parent_generic_component(&namespace)
            && !target.is_global_function()
        {
            target
        } else {
            return;
        };
        let path = path.slice(ith);

        // existing generic maps means that the target symbol has
        // already been processed.
        // For this case, need to insert generic instance generated from
        // the given path explicitly.
        let generic_maps = target.generic_maps();
        for map in generic_maps {
            let affiliation_symbol = map.id.map(|id| symbol_table::get(id).unwrap());
            let mut path = path.clone();
            let ith = path.len() - 1;

            let mut maps = vec![map];
            path.apply_map(&maps);
            path.unalias(None);
            path.append_namespace_path(&namespace, &symbol.namespace);

            Self::insert_generic_instance(
                &path,
                ith,
                &namespace,
                symbol,
                &mut maps,
                affiliation_symbol.as_ref(),
            );
        }

        let kind = match target.kind {
            SymbolKind::Function(mut x) if !x.is_proto => {
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
            SymbolKind::Package(mut x) if !x.is_proto => {
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
        token: &TokenRange,
        struct_token: Option<&Token>,
    ) {
        match symbol_table::resolve(path) {
            Ok(symbol) => {
                for id in symbol.full_path.iter().copied() {
                    symbol_table::add_reference(id, &token.beg);
                }
            }
            Err(err) => {
                self.push_resolve_error(err, token, None, struct_token);
            }
        }
    }

    fn check_array_member_access(
        &mut self,
        path: &GenericSymbolPath,
        token: &Token,
        selects_per_component: &[usize],
    ) {
        for i in 0..path.len().saturating_sub(1) {
            if let Ok(symbol) = symbol_table::resolve_base_path(path, i, token.id)
                && let Some(r#type) = symbol.found.kind.get_type()
            {
                let array_dims = r#type.array.len();
                let applied_selects = selects_per_component.get(i).copied().unwrap_or(0);
                if applied_selects < array_dims {
                    let member_name = if i + 1 < path.len() {
                        path.paths[i + 1].base.to_string()
                    } else {
                        String::new()
                    };
                    self.errors.push(AnalyzerError::member_access_on_array(
                        &symbol.found.token.to_string(),
                        &member_name,
                        array_dims,
                        &path.range,
                    ));
                    return;
                }
            }
        }
    }

    fn check_complex_identifier(
        &mut self,
        path: &GenericSymbolPath,
        token: &Token,
        in_import_declaration: bool,
    ) {
        let (scope, define_context) = scope::token_scope(token.id).unwrap();
        self.generic_symbol_path(
            path,
            scope,
            &define_context,
            in_import_declaration,
            None,
            None,
        );
    }

    pub fn apply(&mut self) -> Vec<AnalyzerError> {
        symbol_table::suppress_cache_clear();
        let candidates: Vec<_> = self.candidates.drain(0..).collect();

        for x in &candidates {
            match x {
                ReferenceCandidate::Identifier { arg } => {
                    self.check_simple_identifier(&arg.into(), &arg.into(), None);
                }
                ReferenceCandidate::HierarchicalIdentifier { arg } => {
                    let token = arg.identifier.identifier_token.token;
                    let path: GenericSymbolPath = arg.into();
                    self.check_complex_identifier(&path, &token, false);
                    if !arg.hierarchical_identifier_list0.is_empty() {
                        let mut selects = vec![0usize; path.len()];
                        selects[0] = arg.hierarchical_identifier_list.len();
                        for (j, member) in arg.hierarchical_identifier_list0.iter().enumerate() {
                            if 1 + j < selects.len() {
                                selects[1 + j] = member.hierarchical_identifier_list0_list.len();
                            }
                        }
                        self.check_array_member_access(&path, &token, &selects);
                    }
                }
                ReferenceCandidate::ScopedIdentifier {
                    arg,
                    in_import_declaration,
                } => {
                    let token = arg.identifier().token;
                    self.check_complex_identifier(&arg.into(), &token, *in_import_declaration);
                }
                ReferenceCandidate::ExpressionIdentifier { arg } => {
                    let token = arg.scoped_identifier.identifier().token;
                    let path: GenericSymbolPath = arg.into();
                    self.testbench_hier_root = Some(path.range.beg.id);
                    self.check_complex_identifier(&path, &token, false);
                    self.testbench_hier_root = None;
                    if !arg.expression_identifier_list0.is_empty() {
                        let scoped_len = arg.scoped_identifier.scoped_identifier_list.len() + 1;
                        let mut selects = vec![0usize; path.len()];
                        if scoped_len <= selects.len() {
                            selects[scoped_len - 1] = arg.expression_identifier_list.len();
                        }
                        for (j, member) in arg.expression_identifier_list0.iter().enumerate() {
                            if scoped_len + j < selects.len() {
                                selects[scoped_len + j] =
                                    member.expression_identifier_list0_list.len();
                            }
                        }
                        self.check_array_member_access(&path, &token, &selects);
                    }
                }
                ReferenceCandidate::GenericArgIdentifier { arg } => {
                    let token = arg.scoped_identifier.identifier().token;
                    self.check_complex_identifier(&arg.into(), &token, false);
                }
                ReferenceCandidate::ImportItem { base, arg } => {
                    let Ok(base_symbol) = symbol_table::resolve(base) else {
                        continue;
                    };

                    let path: SymbolPathNamespace =
                        (arg, &base_symbol.found.inner_namespace()).into();
                    self.check_simple_identifier(&path, &arg.into(), None);
                }
                ReferenceCandidate::ModportItem { arg } => {
                    let mut path: SymbolPathNamespace = arg.identifier.as_ref().into();
                    path.pop_namespace();
                    self.check_simple_identifier(&path, &arg.into(), None);
                }
                ReferenceCandidate::InstParameterItem { arg } => {
                    if arg.inst_parameter_item_opt.is_none() {
                        // implicit port connection by name
                        let identifier = arg.identifier.as_ref();
                        self.check_simple_identifier(&identifier.into(), &identifier.into(), None);
                    }
                }
                ReferenceCandidate::InstPortItem { arg } => {
                    if arg.inst_port_item_opt.is_none() {
                        // implicit port connection by name
                        let identifier = arg.identifier.as_ref();
                        self.check_simple_identifier(&identifier.into(), &identifier.into(), None);
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
                            &identifier.into(),
                            Some(&symbol.found.token),
                        );
                    }
                }
                ReferenceCandidate::NamedArgument { arg, function } => {
                    if let Ok(symbol) = symbol_table::resolve(function) {
                        let func_symbol =
                            if let SymbolKind::ModportFunctionMember(x) = &symbol.found.kind {
                                symbol_table::get(x.function).unwrap()
                            } else {
                                (*symbol.found).clone()
                            };
                        let namespace = func_symbol.inner_namespace();
                        let path: SymbolPath = arg.into();
                        let path: SymbolPathNamespace = (&path, &namespace).into();
                        self.check_simple_identifier(&path, &arg.into(), Some(&func_symbol.token));
                    }
                }
            }
        }

        symbol_table::resume_cache_clear();
        self.errors.drain(0..).collect()
    }

    fn get_struct_namespace(symbol: &Symbol) -> Namespace {
        if let SymbolKind::TypeDef(x) = &symbol.kind
            && let Some(r#type) = &x.r#type
        {
            let namespace = Some(&symbol.namespace);
            if let Some((_, Some(symbol))) = r#type.trace_user_defined(namespace) {
                return Self::get_struct_namespace(&symbol);
            }
        }
        symbol.inner_namespace()
    }
}

thread_local!(static REFERENCE_TABLE: RefCell<ReferenceTable> = RefCell::new(ReferenceTable::new()));

pub fn add(cand: ReferenceCandidate) {
    REFERENCE_TABLE.with(|f| f.borrow_mut().add(cand))
}

/// Returns the current number of pending candidates. Used as a watermark
/// to delimit one file's pass1 additions for fragment caching.
pub fn candidates_len() -> usize {
    REFERENCE_TABLE.with(|f| f.borrow().candidates.len())
}

/// Exports the candidates added since the given watermark.
pub fn export_candidates_since(watermark: usize) -> Vec<ReferenceCandidate> {
    REFERENCE_TABLE.with(|f| f.borrow().candidates[watermark..].to_vec())
}

pub fn apply() -> Vec<AnalyzerError> {
    REFERENCE_TABLE.with(|f| f.borrow_mut().apply())
}
