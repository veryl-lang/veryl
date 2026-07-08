mod connect;
mod r#enum;
mod function;
mod msb;

use crate::analyzer_error::DuplicatedIdentifierKind;
use crate::namespace::{DefineContext, Namespace};
use crate::scope;
use crate::sv_system_function;
use crate::symbol::{
    ConnectTarget, Direction, DocComment, GenericBoundKind, GenericMap, GenericTable,
    GenericTables, InstanceProperty, ModportDefault, ModportFunctionMemberProperty,
    ModportProperty, ModportVariableMemberProperty, Symbol, SymbolId, SymbolKind, TestProperty,
    TypeKind,
};
use crate::symbol_path::{
    GenericSymbol, GenericSymbolPath, GenericSymbolPathNamespace, SymbolPath, SymbolPathNamespace,
};
use crate::tb_component;
use crate::wavedrom::{self, DocTestTarget};
use crate::{AnalyzerError, HashMap, SVec};
use connect::check_connect;
use log::trace;
use msb::check_msb;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::rc::Rc;
use veryl_parser::resource_table::{self, PathId, StrId};
use veryl_parser::token_collector::TokenCollector;
use veryl_parser::veryl_grammar_trait::{Expression, ExpressionIdentifier, HierarchicalIdentifier};
use veryl_parser::veryl_token::{Token, TokenSource};
use veryl_parser::veryl_walker::VerylWalker;

#[derive(Clone, Debug)]
pub struct ResolveResult {
    pub found: std::rc::Rc<Symbol>,
    pub full_path: Vec<SymbolId>,
    pub imported: bool,
    pub generic_tables: GenericTables,
}

#[derive(Clone, Debug)]
pub struct ResolveError {
    pub last_found: Option<Box<Symbol>>,
    pub cause: ResolveErrorCause,
}

#[derive(Clone, Debug)]
pub enum ResolveErrorCause {
    NotFound(StrId),
    Private,
    Invisible,
    /// The name resolved to two or more distinct symbols imported at equal
    /// priority (e.g. the same name from two wildcard-imported packages).
    Ambiguous(StrId),
}

impl ResolveError {
    pub fn new(last_found: Option<&Rc<Symbol>>, cause: ResolveErrorCause) -> Self {
        Self {
            last_found: last_found.map(|x| Box::new((**x).clone())),
            cause,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Import {
    pub path: GenericSymbolPathNamespace,
    pub namespace: Namespace,
    pub wildcard: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bind {
    pub token: Token,
    pub target: GenericSymbolPathNamespace,
    pub doc_comment: DocComment,
    pub property: InstanceProperty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Msb {
    pub token: Token,
    pub path: SymbolPathNamespace,
    pub dimension: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Connect {
    Statement(ExpressionIdentifier, Expression),
    Declaration(HierarchicalIdentifier, Expression),
}

/// Pending-list lengths captured before one file's pass1; delimits the
/// file's additions for fragment caching.
#[derive(Clone, Copy, Debug)]
pub struct PendingWatermark {
    import: usize,
    bind: usize,
    msb: usize,
    connect: usize,
}

/// Outcome of a lexical `lookup_name` over the scope tree.
enum LexicalLookup {
    Found {
        symbol: SymbolId,
        imported: bool,
    },
    /// Two or more distinct, simultaneously-active bindings of the name in the
    /// nearest scope that binds it (e.g. a local and an explicit import, or two
    /// wildcard imports). The reference must be qualified.
    Ambiguous,
    NotFound,
}

/// One candidate binding gathered for a single scope during `lookup_name`.
struct LookupCandidate {
    /// Definition site (`token` source/line/column); generic instantiation
    /// clones one declaration into several `SymbolId`s, so candidates are
    /// deduplicated by this rather than by id.
    site: (TokenSource, u32, u32),
    /// Define context that gates this binding's activeness (the declaration's
    /// or the import statement's ifdef).
    define_context: DefineContext,
    depth: usize,
    id: SymbolId,
    imported: bool,
    /// Whether the symbol can hold `::member`, used to prefer a container over a
    /// same-named leaf when this candidate fills a non-final path segment.
    can_have_member: bool,
}

/// Resolves the candidates gathered within one scope (one precedence tier).
/// Ambiguous when two distinct definition sites could be active together;
/// otherwise picks the deepest binding (last wins ties) among generic clones
/// and ifdef variants. When `prefer_container` (a non-final path segment), a
/// container outranks a deeper same-named leaf.
fn resolve_tier(candidates: &[LookupCandidate], prefer_container: bool) -> LexicalLookup {
    let mut distinct: Vec<&LookupCandidate> = Vec::new();
    for candidate in candidates {
        if !distinct.iter().any(|x| x.site == candidate.site) {
            distinct.push(candidate);
        }
    }
    let ambiguous = distinct.iter().enumerate().any(|(i, ci)| {
        distinct
            .iter()
            .skip(i + 1)
            .any(|cj| !ci.define_context.exclusive(&cj.define_context))
    });
    if ambiguous {
        return LexicalLookup::Ambiguous;
    }

    let rank = |c: &LookupCandidate| (prefer_container && c.can_have_member, c.depth);
    let mut best = &candidates[0];
    for candidate in &candidates[1..] {
        if rank(candidate) >= rank(best) {
            best = candidate;
        }
    }
    LexicalLookup::Found {
        symbol: best.id,
        imported: best.imported,
    }
}

/// Everything one file's pass1 wrote into the symbol table, in a
/// serializable form. Part of the fragment cache payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolTableFragment {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub binds: Vec<Bind>,
    pub msbs: Vec<Msb>,
    pub connects: Vec<Connect>,
    pub reference_functions: Vec<(SymbolId, Vec<GenericSymbolPath>)>,
    pub references: Vec<(SymbolId, Vec<Token>)>,
}

#[derive(Clone, Default, Debug)]
pub struct SymbolTable {
    name_table: HashMap<StrId, Vec<SymbolId>>,
    symbol_table: HashMap<SymbolId, Rc<Symbol>>,
    namespace_index: HashMap<SVec<StrId>, Vec<SymbolId>>,
    project_local_table: HashMap<StrId, HashMap<StrId, StrId>>,
    import_list: Vec<Import>,
    bind_list: Vec<Bind>,
    msb_list: Vec<Msb>,
    connect_list: Vec<Connect>,
    reference_func_table: HashMap<SymbolId, Vec<GenericSymbolPath>>,
    reference_table: HashMap<SymbolId, Vec<Token>>,
    suppress_cache_clear: bool,
    skip_generic_args: bool,
}

impl SymbolTable {
    pub fn new() -> Self {
        let mut ret = Self::default();

        // add builtin symbols to "" namespace
        let namespace = Namespace::new();

        for func in DEFINED_NAMESPACES {
            let token = Token::new(func, 0, 0, 0, 0, TokenSource::Builtin);
            let symbol = Symbol::new(
                &token,
                SymbolKind::Namespace,
                &namespace,
                true,
                DocComment::default(),
            );
            let _ = ret.insert(&token, symbol);
        }

        sv_system_function::insert_symbols(&mut ret, &namespace);
        tb_component::insert_symbols(&mut ret, &namespace);

        ret
    }

    pub fn insert(&mut self, token: &Token, symbol: Symbol) -> Option<SymbolId> {
        let entry = self
            .name_table
            .entry(resource_table::canonical_str_id(token.text))
            .or_default();
        for id in entry.iter() {
            let item = self.symbol_table.get(id).unwrap();
            let symbol = &symbol.namespace;
            let item = &item.namespace;

            let same_namespace = symbol.paths == item.paths;
            let define_exclusive = symbol.define_context.exclusive(&item.define_context);

            let conflict = same_namespace && !define_exclusive;
            if conflict {
                return None;
            }
        }
        let id = symbol.id;
        let scope = symbol.scope;
        let ns_paths = symbol.namespace.paths.clone();
        entry.push(id);
        self.symbol_table.insert(id, Rc::new(symbol));
        self.namespace_index.entry(ns_paths).or_default().push(id);
        // Mirror the binding into the scope tree so lexical lookup sees every
        // inserted symbol regardless of which path created it.
        scope::add_local(scope, token.text, id);
        Some(id)
    }

    pub fn get(&self, id: SymbolId) -> Option<Symbol> {
        self.symbol_table.get(&id).map(|x| (**x).clone())
    }

    pub fn get_rc(&self, id: SymbolId) -> Option<Rc<Symbol>> {
        self.symbol_table.get(&id).cloned()
    }

    pub fn update(&mut self, symbol: Symbol) {
        let id = symbol.id;
        self.symbol_table.insert(id, Rc::new(symbol));
    }

    fn match_nested_generic_instance(&self, context: &ResolveContext, found: &Symbol) -> bool {
        if let Some(last_found) = context.last_found
            && let (SymbolKind::GenericInstance(_), SymbolKind::GenericInstance(_)) =
                (&last_found.kind, &found.kind)
        {
            let inner = scope::inner_scope(last_found.scope, last_found.token.text);
            return inner == found.scope
                && !last_found
                    .namespace
                    .define_context
                    .exclusive(&found.namespace.define_context);
        }
        false
    }

    fn trace_type_kind<'a>(
        &self,
        mut context: ResolveContext<'a>,
        kind: &TypeKind,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let TypeKind::UserDefined(x) = kind {
            let mut path = x.path.clone();
            path.resolve_imported(context.scope, &context.define_context, None);
            path.unalias(None);
            let symbol = self.resolve(
                &path.generic_path(),
                &path.generic_arguments(),
                context.push(),
            )?;

            // cyclic-type guard: stop tracing if this type is already being
            // traced (a self- or mutually-referential type definition).
            let key = (symbol.found.id, ResolvePhase::TypeTrace);
            if context.visiting.contains(&key) {
                return Ok(context);
            }
            context.visiting.push(key);

            match &symbol.found.kind {
                SymbolKind::SystemVerilog => context.sv_member = true,
                SymbolKind::Parameter(x) if !x.is_proto => {
                    if matches!(x.r#type.kind, TypeKind::Type) {
                        let value = x.value.as_ref().unwrap();
                        return self.trace_type_parameter(context, value, &symbol.found);
                    }
                }
                SymbolKind::TypeDef(x) => {
                    if let Some(r#type) = &x.r#type {
                        context.set_namespace(
                            symbol.found.scope,
                            symbol.found.namespace.define_context.clone(),
                        );
                        return self.trace_type_kind(context, &r#type.kind);
                    }
                }
                SymbolKind::GenericParameter(x) => {
                    if matches!(x.bound, GenericBoundKind::Type) {
                        return self.trace_generic_parameter(context.clone(), &symbol.found);
                    }
                }
                _ => (),
            }
            context.set_inner(&symbol.found);
            context.last_found_type = Some(symbol.found.id);
            context.inner = true;
            context.generic_tables = symbol.generic_tables;
        } else {
            // assign a new empty namespace becuase
            // factor types and abstruct interface type have no members.
            context.set_namespace(scope::ScopeId::default(), DefineContext::default());
            context.inner = true;
        }
        Ok(context)
    }

    fn trace_type_path<'a>(
        &self,
        context: ResolveContext<'a>,
        path: &GenericSymbolPath,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        let (mut context, found) = self.expand_alias(context, path)?;
        match &found.kind {
            SymbolKind::GenericInstance(_) => self.trace_generic_instance(context, &found),
            SymbolKind::GenericParameter(_) => self.trace_generic_parameter(context, &found),
            _ => {
                if matches!(found.kind, SymbolKind::SystemVerilog) {
                    context.sv_member = true;
                }
                context.set_inner(&found);
                context.last_found_type = Some(found.id);
                context.inner = true;
                Ok(context)
            }
        }
    }

    /// Follows an alias chain (`alias module`/`interface`/`package`) to its
    /// non-alias target, resolving each link in `context` and returning the
    /// terminal symbol. `context.visiting` breaks cyclic aliases used as a type.
    fn expand_alias<'a>(
        &self,
        mut context: ResolveContext<'a>,
        path: &GenericSymbolPath,
    ) -> Result<(ResolveContext<'a>, Rc<Symbol>), ResolveError> {
        let symbol = self.resolve(
            &path.generic_path(),
            &path.generic_arguments(),
            context.push(),
        )?;
        context.generic_tables = symbol.generic_tables.clone();

        // cyclic-alias guard (alias used as a type): report unresolvable if revisited.
        if matches!(
            symbol.found.kind,
            SymbolKind::AliasModule(_)
                | SymbolKind::AliasInterface(_)
                | SymbolKind::AliasPackage(_)
        ) {
            let key = (symbol.found.id, ResolvePhase::Alias);
            if context.visiting.contains(&key) {
                return Err(ResolveError::new(
                    Some(&symbol.found),
                    ResolveErrorCause::NotFound(symbol.found.token.text),
                ));
            }
            context.visiting.push(key);
        }

        match &symbol.found.kind {
            SymbolKind::AliasModule(x) => self.expand_alias(context, &x.target),
            SymbolKind::AliasInterface(x) => self.expand_alias(context, &x.target),
            SymbolKind::AliasPackage(x) => self.expand_alias(context, &x.target),
            _ => Ok((context, Rc::clone(&symbol.found))),
        }
    }

    fn trace_generic_instance<'a>(
        &self,
        mut context: ResolveContext<'a>,
        found: &Symbol,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let SymbolKind::GenericInstance(x) = &found.kind {
            let base = self.symbol_table.get(&x.base).unwrap();

            // The scope-tree delegation built at instance creation must point
            // the instance's member scope at the same base member scope that
            // `set_inner(base)` below redirects to.
            #[cfg(debug_assertions)]
            {
                let inst_inner = scope::inner_scope(found.scope, found.token.text);
                let base_inner = scope::inner_scope(base.scope, base.token.text);
                debug_assert_eq!(
                    scope::generic_delegation(inst_inner),
                    Some(base_inner),
                    "generic instance delegation must match the base redirect"
                );
            }

            let prev_inst = context.inst_scope;
            context.set_inner(base);
            // Shadow cursor moves to the instance-side (mangled) scope instead of
            // the base, so the reconstructed namespace carries the mangled name.
            // For nested instances the mangled scope is rebased onto the outer
            // instance cursor; otherwise it is the registered instance scope.
            let base_inner = context.scope;
            context.inst_scope = if scope::generic_delegation(prev_inst) == Some(found.scope) {
                scope::register_generic_child(prev_inst, found.token.text, base_inner)
            } else {
                scope::inner_scope(found.scope, found.token.text)
            };
            context.last_found_type = Some(base.id);
            context.inner = true;
        }
        Ok(context)
    }

    fn trace_generic_parameter<'a>(
        &self,
        mut context: ResolveContext<'a>,
        found: &Symbol,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let SymbolKind::GenericParameter(x) = &found.kind {
            if let Some(x) = self.resolve_generic_param_const(context.clone(), found) {
                return x;
            }

            let symbol = match &x.bound {
                GenericBoundKind::Type if x.default_value.is_some() => {
                    let path = x.default_value.as_ref().unwrap().generic_path();
                    let mut ctxt = ResolveContext::new(&found.namespace);
                    ctxt.depth = context.depth + 1;
                    &self.resolve(&path, &[], ctxt)?.found
                }
                GenericBoundKind::Inst(proto) => {
                    let mut ctxt = ResolveContext::new(&found.namespace);
                    ctxt.depth = context.depth + 1;
                    &self
                        .resolve(&proto.generic_path(), &proto.generic_arguments(), ctxt)?
                        .found
                }
                GenericBoundKind::Proto(proto) => {
                    if let Some(x) = proto.get_user_defined() {
                        let mut ctxt = ResolveContext::new(&found.namespace);
                        ctxt.depth = context.depth + 1;
                        &self.resolve(&x.path.generic_path(), &[], ctxt)?.found
                    } else {
                        found
                    }
                }
                _ => found,
            };

            context.set_inner(symbol);
            context.last_found_type = Some(symbol.id);
            context.inner = true;
        }
        Ok(context)
    }

    fn trace_generic_const<'a>(
        &self,
        mut context: ResolveContext<'a>,
        found: &Symbol,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let SymbolKind::GenericConst(x) = &found.kind {
            if let Some(x) = self.resolve_generic_param_const(context.clone(), found) {
                return x;
            }

            let symbol = match &x.bound {
                GenericBoundKind::Type => {
                    if let Some(identifier) = x.value.unwrap_identifier() {
                        let path: GenericSymbolPath = identifier.into();
                        let mut ctxt = ResolveContext::new(&found.namespace);
                        ctxt.depth = context.depth + 1;
                        &self.resolve(&path.generic_path(), &[], ctxt)?.found
                    } else {
                        found
                    }
                }
                GenericBoundKind::Proto(proto) => {
                    if let Some(x) = proto.get_user_defined() {
                        let mut ctxt = ResolveContext::new(&found.namespace);
                        ctxt.depth = context.depth + 1;
                        &self.resolve(&x.path.generic_path(), &[], ctxt)?.found
                    } else {
                        found
                    }
                }
                _ => found,
            };

            context.set_inner(symbol);
            context.last_found_type = Some(symbol.id);
            context.inner = true;
        }
        Ok(context)
    }

    fn resolve_generic_param_const<'a>(
        &self,
        mut context: ResolveContext<'a>,
        found: &Symbol,
    ) -> Option<Result<ResolveContext<'a>, ResolveError>> {
        match &found.kind {
            SymbolKind::GenericParameter(_) | SymbolKind::GenericConst(_) => {
                if let Some(generic_table) = context
                    .generic_tables
                    .get(&(found.scope, found.namespace.define_context.clone()))
                    && let Some(path) = generic_table.get(&found.token.text)
                {
                    let result = self.resolve(
                        &path.generic_path(),
                        &path.generic_arguments(),
                        context.push(),
                    );
                    if let Ok(symbol) = result {
                        context.set_inner(&symbol.found);
                        context.last_found_type = Some(symbol.found.id);
                        context.inner = true;
                        return Some(Ok(context));
                    } else {
                        return result.err().map(Err);
                    }
                }
            }
            _ => {}
        }

        None
    }

    fn trace_type_parameter<'a>(
        &self,
        mut context: ResolveContext<'a>,
        expression: &Expression,
        found: &Symbol,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let Some(identifier) = expression.unwrap_identifier() {
            let mut ctxt = ResolveContext::new(&found.namespace);
            ctxt.depth = context.depth + 1;

            let symbol = self.resolve(&identifier.into(), &[], ctxt)?;
            match &symbol.found.kind {
                SymbolKind::Parameter(x) if !x.is_proto => {
                    if matches!(x.r#type.kind, TypeKind::Type) {
                        let value = x.value.as_ref().unwrap();
                        return self.trace_type_parameter(context, value, &symbol.found);
                    }
                }
                SymbolKind::TypeDef(x) => {
                    if let Some(ref r#type) = x.r#type {
                        return self.trace_type_kind(context, &r#type.kind);
                    }
                }
                SymbolKind::GenericParameter(x) => {
                    if matches!(x.bound, GenericBoundKind::Type)
                        && let Some(x) =
                            self.resolve_generic_param_const(context.clone(), &symbol.found)
                    {
                        return x;
                    }
                }
                SymbolKind::GenericConst(x) => {
                    if matches!(x.bound, GenericBoundKind::Type)
                        && let Some(x) =
                            self.resolve_generic_param_const(context.clone(), &symbol.found)
                    {
                        return x;
                    }
                }
                _ => {}
            }

            context.set_inner(&symbol.found);
            context.last_found_type = Some(symbol.found.id);
            context.inner = true;
            context.generic_tables = symbol.generic_tables;
        }

        Ok(context)
    }

    /// Advances the cursor from a non-final path segment `found` to the scope
    /// holding the members the next segment resolves against. Dispatches on the
    /// symbol kind: typed members trace to their type's definition, aliases
    /// follow their target, generics instantiate, and containers descend into
    /// their inner scope.
    fn navigate_member<'a>(
        &self,
        mut context: ResolveContext<'a>,
        found: &Rc<Symbol>,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        match &found.kind {
            SymbolKind::Variable(x) => {
                context = self.trace_type_kind(context, &x.r#type.kind)?;
            }
            SymbolKind::StructMember(x) => {
                context = self.trace_type_kind(context, &x.r#type.kind)?;
            }
            SymbolKind::UnionMember(x) => {
                context = self.trace_type_kind(context, &x.r#type.kind)?;
            }
            SymbolKind::Parameter(x) if x.is_proto => {
                context = self.trace_type_kind(context, &x.r#type.kind)?;
            }
            SymbolKind::Parameter(x) => {
                if matches!(x.r#type.kind, TypeKind::Type) {
                    let value = x.value.as_ref().unwrap();
                    context = self.trace_type_parameter(context, value, found)?;
                } else {
                    context = self.trace_type_kind(context, &x.r#type.kind)?;
                }
            }
            SymbolKind::TypeDef(x) => {
                if let Some(ref r#type) = x.r#type {
                    context = self.trace_type_kind(context, &r#type.kind)?;
                }
            }
            SymbolKind::Port(x) => {
                context = self.trace_type_kind(context, &x.r#type.kind)?;
            }
            SymbolKind::ModportVariableMember(_) => {
                let path = SymbolPath::new(&[found.token.text]);
                context.set_namespace(
                    scope::parent(found.scope).unwrap_or_default(),
                    found.namespace.define_context.clone(),
                );
                let symbol = self.resolve(&path, &[], context.push())?;
                if let SymbolKind::Variable(x) = &symbol.found.kind {
                    context = self.trace_type_kind(context, &x.r#type.kind)?;
                }
            }
            // proto module has no inner item to trace
            SymbolKind::Module(x) if x.is_proto => (),
            SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package(_) => {
                context.set_inner(found);
                context.inner = true;
            }
            SymbolKind::AliasModule(x) => {
                context = self.trace_type_path(context, &x.target)?;
            }
            SymbolKind::AliasInterface(x) => {
                context = self.trace_type_path(context, &x.target)?;
            }
            SymbolKind::AliasPackage(x) => {
                context = self.trace_type_path(context, &x.target)?;
            }
            SymbolKind::Enum(_) | SymbolKind::Namespace | SymbolKind::TbComponent(_) => {
                context.set_inner(found);
                context.inner = true;
            }
            SymbolKind::SystemVerilog => {
                context.set_inner(found);
                context.inner = true;
                context.sv_member = true;
            }
            SymbolKind::Instance(x) => {
                let mut type_name = x.type_name.clone();
                type_name.resolve_imported(context.scope, &context.define_context, None);
                type_name.unalias(None);
                context = self.trace_type_path(context, &type_name)?;
            }
            SymbolKind::GenericInstance(_) => {
                context = self.trace_generic_instance(context, found)?;
            }
            SymbolKind::GenericParameter(_) => {
                context = self.trace_generic_parameter(context, found)?;
            }
            SymbolKind::GenericConst(_) => {
                context = self.trace_generic_const(context, found)?;
            }
            // don't trace inner item
            SymbolKind::Function(_)
            | SymbolKind::Struct(_)
            | SymbolKind::Union(_)
            | SymbolKind::Modport(_)
            | SymbolKind::ModportFunctionMember(_)
            | SymbolKind::EnumMember(_)
            | SymbolKind::EnumMemberMangled
            | SymbolKind::Block
            | SymbolKind::SystemFunction(_)
            | SymbolKind::Genvar
            | SymbolKind::ClockDomain
            | SymbolKind::Test(_)
            | SymbolKind::Embed
            | SymbolKind::ProjectProperty(_) => (),
        }
        Ok(context)
    }

    /// Checks that `found` is reachable from the current cursor: public across a
    /// project boundary (`is_public`) and visible through the container just
    /// navigated (`is_visible`).
    fn check_access(&self, context: &ResolveContext, found: &Symbol) -> Result<(), ResolveError> {
        if !self.is_public(context, found) {
            Err(ResolveError::new(context.found, ResolveErrorCause::Private))
        } else if !self.is_visible(context, found) {
            Err(ResolveError::new(
                context.found,
                ResolveErrorCause::Invisible,
            ))
        } else {
            Ok(())
        }
    }

    fn is_public(&self, context: &ResolveContext, found: &Symbol) -> bool {
        match found.kind {
            SymbolKind::Module(_)
            | SymbolKind::AliasModule(_)
            | SymbolKind::Interface(_)
            | SymbolKind::AliasInterface(_)
            | SymbolKind::Package(_)
            | SymbolKind::AliasPackage(_) => !context.other_prj || found.public,
            SymbolKind::Namespace => !context.root_prj || found.public,
            _ => true,
        }
    }

    fn is_visible(&self, context: &ResolveContext, found: &Symbol) -> bool {
        if context.last_found.is_none() || matches!(found.kind, SymbolKind::SystemVerilog) {
            return true;
        }

        let last_found = context.last_found.unwrap();
        let last_found_type = context.last_found_type.map(|x| {
            let symbol = self.symbol_table.get(&x).unwrap();
            symbol.kind.clone()
        });
        let via_modport = match &last_found.kind {
            SymbolKind::Port(x) => matches!(x.direction, Direction::Modport | Direction::Interface),
            _ => false,
        };
        let via_interface_instance = match &last_found.kind {
            SymbolKind::Instance(_) => matches!(
                last_found_type,
                Some(SymbolKind::Interface(_)) | Some(SymbolKind::AliasInterface(_))
            ),
            SymbolKind::GenericParameter(x) => {
                matches!(&x.bound, GenericBoundKind::Inst(_))
                    && matches!(last_found_type, Some(SymbolKind::Interface(_)))
            }
            _ => false,
        };
        let via_interface = match &last_found.kind {
            SymbolKind::Interface(_) | SymbolKind::AliasInterface(_) => true,
            SymbolKind::GenericInstance(_) => {
                matches!(last_found_type, Some(SymbolKind::Interface(ref x)) if !x.is_proto)
            }
            SymbolKind::GenericParameter(x) => {
                matches!(&x.bound, GenericBoundKind::Proto(_))
                    && matches!(last_found_type, Some(SymbolKind::Interface(ref x)) if x.is_proto)
            }
            _ => false,
        };
        let via_pacakge = match &last_found.kind {
            SymbolKind::Package(_) | SymbolKind::AliasPackage(_) => true,
            SymbolKind::GenericInstance(_) => {
                matches!(last_found_type, Some(SymbolKind::Package(_)))
            }
            SymbolKind::GenericParameter(_) => {
                matches!(last_found_type, Some(SymbolKind::Package(_)))
            }
            _ => false,
        };
        let via_enum = match &last_found.kind {
            SymbolKind::Enum(_) => true,
            SymbolKind::TypeDef(x) => {
                x.r#type.is_some() && matches!(last_found_type, Some(SymbolKind::Enum(_)))
            }
            _ => false,
        };
        let via_namespace = matches!(last_found.kind, SymbolKind::Namespace);
        let via_tb_component = matches!(last_found_type, Some(SymbolKind::TbComponent(_)));

        match &found.kind {
            SymbolKind::Variable(_)
            | SymbolKind::ModportFunctionMember(_)
            | SymbolKind::ModportVariableMember(_) => via_modport || via_interface_instance,
            SymbolKind::StructMember(_) | SymbolKind::UnionMember(_) => matches!(
                last_found.kind,
                SymbolKind::Port(_)
                    | SymbolKind::ModportVariableMember(_)
                    | SymbolKind::Variable(_)
                    | SymbolKind::Parameter(_)
                    | SymbolKind::StructMember(_)
                    | SymbolKind::UnionMember(_)
                    | SymbolKind::GenericParameter(_)
            ),
            SymbolKind::Parameter(_)
            | SymbolKind::TypeDef(_)
            | SymbolKind::Enum(_)
            | SymbolKind::Struct(_)
            | SymbolKind::Union(_)
            | SymbolKind::AliasModule(_)
            | SymbolKind::AliasInterface(_)
            | SymbolKind::AliasPackage(_) => via_pacakge,
            SymbolKind::Function(x) if x.is_proto => via_pacakge,
            SymbolKind::Function(x) => {
                via_modport
                    || via_interface_instance
                    || via_pacakge
                    || via_tb_component
                    || via_namespace && x.is_global()
            }
            SymbolKind::EnumMember(_) | SymbolKind::EnumMemberMangled => via_enum,
            SymbolKind::Modport(_) => via_interface || via_interface_instance,
            SymbolKind::GenericInstance(_) => {
                // A generic instance in this context is for generic type or function
                // defined in a packge or for generic component defined in other project
                via_pacakge || via_namespace
            }
            _ => via_namespace,
        }
    }

    /// Member lookup (`a.b`): among the symbols sharing the segment's name,
    /// picks the one declared directly in the cursor scope — or a matching
    /// nested generic instance — with the deepest namespace winning. Unlike
    /// `lookup_name` it does not walk parent scopes.
    fn lookup_member<'a>(
        &'a self,
        context: &ResolveContext,
        name: StrId,
    ) -> Option<&'a Rc<Symbol>> {
        // For nested generic instances the member lives in the base instance's
        // inner scope, so include those locals too.
        let mut candidates = scope::locals_get(context.scope, name);
        if let Some(last_found) = context.last_found
            && matches!(last_found.kind, SymbolKind::GenericInstance(_))
        {
            let inner = scope::inner_scope(last_found.scope, last_found.token.text);
            candidates.extend(scope::locals_get(inner, name));
        }

        let mut max_depth = 0;
        let mut found = None;
        for id in candidates {
            let symbol = self.symbol_table.get(&id).unwrap();
            let matched = self.match_nested_generic_instance(context, symbol)
                || (context.scope == symbol.scope
                    && !context
                        .define_context
                        .exclusive(&symbol.namespace.define_context));
            if matched && symbol.namespace.depth() >= max_depth {
                found = Some(symbol);
                max_depth = symbol.namespace.depth();
            }
        }

        if found.is_some() {
            return found;
        }

        if context
            .last_found_type
            .map(|x| self.symbol_table.get(&x).unwrap().kind.has_ancestors())
            .unwrap_or(false)
        {
            for inherited_interface in scope::inherited_interface_get(context.scope) {
                if context
                    .define_context
                    .exclusive(&inherited_interface.define_context)
                {
                    continue;
                }
                for id in scope::locals_get(inherited_interface.source, name) {
                    let symbol = self.symbol_table.get(&id).unwrap();
                    if !symbol
                        .namespace
                        .define_context
                        .exclusive(&inherited_interface.source_define_context)
                    {
                        return Some(symbol);
                    }
                }
            }
        }

        None
    }

    /// Lexical name lookup over the scope tree: walk from `scope` up the parent
    /// chain and resolve `name` in the nearest scope that binds it. Within a
    /// scope, explicit bindings (local declarations and explicit imports) take
    /// precedence over wildcard imports, matching SystemVerilog import rules; a
    /// binding in an inner scope shadows everything above it. Candidates whose
    /// define context is mutually exclusive with the query are not visible.
    fn lookup_name(
        &self,
        scope: scope::ScopeId,
        name: StrId,
        query_dctx: &DefineContext,
        prefer_container: bool,
    ) -> LexicalLookup {
        let candidate = |id: SymbolId, symbol: &Symbol, define_context, imported| LookupCandidate {
            site: (symbol.token.source, symbol.token.line, symbol.token.column),
            define_context,
            depth: symbol.namespace.depth(),
            id,
            imported,
            can_have_member: symbol.kind.can_have_path_member(),
        };

        // A non-final segment prefers a container: a nearer same-named leaf is
        // kept only as a fallback while the walk continues outward for a
        // container (the first one found is the deepest).
        let mut fallback: Option<LexicalLookup> = None;
        let mut current = Some(scope);
        while let Some(scope) = current {
            // A generic-instance scope holds no members of its own: resolve names
            // in the base template it delegates to, and continue up the base's
            // parent chain. For non-instance scopes this is the scope itself.
            let scope = scope::generic_delegation(scope).unwrap_or(scope);

            // Tier 1: explicit bindings — local declarations and explicit imports.
            let mut tier = Vec::new();
            for id in scope::locals_get(scope, name) {
                if let Some(symbol) = self.symbol_table.get(&id)
                    && !query_dctx.exclusive(&symbol.namespace.define_context)
                {
                    let dctx = symbol.namespace.define_context.clone();
                    tier.push(candidate(id, symbol, dctx, false));
                }
            }
            for binding in scope::imports_get(scope, name) {
                if !query_dctx.exclusive(&binding.define_context)
                    && let Some(symbol) = self.symbol_table.get(&binding.symbol)
                {
                    tier.push(candidate(
                        binding.symbol,
                        symbol,
                        binding.define_context,
                        true,
                    ));
                }
            }
            if !tier.is_empty()
                && let Some(result) = self.container_or_stash(
                    resolve_tier(&tier, prefer_container),
                    prefer_container,
                    &mut fallback,
                )
            {
                return result;
            }

            // Tier 2: wildcard imports, only when no explicit binding shadows them.
            let mut tier = Vec::new();
            for wildcard in scope::wildcards_get(scope) {
                if query_dctx.exclusive(&wildcard.define_context) {
                    continue;
                }
                for id in scope::locals_get(wildcard.source, name) {
                    if let Some(symbol) = self.symbol_table.get(&id)
                        && !symbol
                            .namespace
                            .define_context
                            .exclusive(&wildcard.source_define_context)
                    {
                        let dctx = symbol.namespace.define_context.clone();
                        tier.push(candidate(id, symbol, dctx, true));
                    }
                }
            }
            if !tier.is_empty()
                && let Some(result) = self.container_or_stash(
                    resolve_tier(&tier, prefer_container),
                    prefer_container,
                    &mut fallback,
                )
            {
                return result;
            }

            // Tier 3: inherited interfaces
            let mut tier = Vec::new();
            for inherited_interface in scope::inherited_interface_get(scope) {
                if query_dctx.exclusive(&inherited_interface.define_context) {
                    continue;
                }
                for id in scope::locals_get(inherited_interface.source, name) {
                    if let Some(symbol) = self.symbol_table.get(&id)
                        && !symbol
                            .namespace
                            .define_context
                            .exclusive(&inherited_interface.source_define_context)
                    {
                        let dctx = symbol.namespace.define_context.clone();
                        tier.push(candidate(id, symbol, dctx, true));
                    }
                }
            }
            if !tier.is_empty()
                && let Some(result) = self.container_or_stash(
                    resolve_tier(&tier, prefer_container),
                    prefer_container,
                    &mut fallback,
                )
            {
                return result;
            }

            current = scope::parent(scope);
        }
        fallback.unwrap_or(LexicalLookup::NotFound)
    }

    /// For a non-final path segment (`prefer_container`), a same-named leaf that
    /// is nearer than a container must not shadow it: such a leaf is stashed as
    /// the fallback (the first/nearest one) and the caller keeps walking outward.
    /// A container result, or any result for a final segment, is returned as-is.
    fn container_or_stash(
        &self,
        result: LexicalLookup,
        prefer_container: bool,
        fallback: &mut Option<LexicalLookup>,
    ) -> Option<LexicalLookup> {
        if prefer_container
            && let LexicalLookup::Found { symbol, .. } = &result
            && !self
                .symbol_table
                .get(symbol)
                .is_some_and(|s| s.kind.can_have_path_member())
        {
            if fallback.is_none() {
                *fallback = Some(result);
            }
            return None;
        }
        Some(result)
    }

    /// Registers the generic instantiation for `found` built from this segment's
    /// generic arguments, after normalizing them into the base component's
    /// namespace.
    fn instantiate_generic(
        &self,
        context: &mut ResolveContext,
        found: &Symbol,
        mut arguments: Vec<GenericSymbolPath>,
        namespace_generic_map: &NamespaceGenericMap,
        entry_scope: scope::ScopeId,
        entry_define_context: &DefineContext,
    ) {
        for arg in &mut arguments {
            // Generic arguments will be resolved in the namespace of base component.
            // Therefore, generic parameters given as generic arguments should be replaced
            // with their types.
            // See: https://github.com/veryl-lang/veryl/issues/1714#issuecomment-2967149726
            if let Some(map) = namespace_generic_map {
                arg.apply_map(map.as_slice());
            }

            // Path to generic arg should have its project prefix to make it visible from
            // the namespace of base component. An empty query namespace carries no project
            // to prepend, so there is nothing to append.
            if let Some(prj) = scope::project_of(entry_scope)
                && Some(prj) != scope::project_of(context.scope)
            {
                self.append_project_path(arg, entry_scope, entry_define_context);
            }
        }

        context.generic_tables.insert(
            (
                scope::inner_scope(found.scope, found.token.text),
                found.namespace.define_context.clone(),
            ),
            found.generic_table(&arguments),
        );
    }

    fn resolve<'a>(
        &'a self,
        path: &SymbolPath,
        generic_arguments: &[Vec<GenericSymbolPath>],
        mut context: ResolveContext<'a>,
    ) -> Result<ResolveResult, ResolveError> {
        let entry_scope = context.scope;
        let entry_define_context = context.define_context.clone();
        let mut path = path.clone();

        // Replace project-local names. An empty namespace carries no project
        // context (e.g. resolving a numeric or builtin-type generic argument),
        // so there is no project-local aliasing to apply.
        if let Some(prj) = scope::project_of(entry_scope) {
            let path_head = path.0[0];
            if let Some(map) = self.project_local_table.get(&prj) {
                context.root_prj = false;
                if let Some(id) = map.get(&path_head) {
                    path.0[0] = *id;
                }
            }
        }

        trace!("symbol_table: {}resolve   '{}'", context.indent(), path);

        let namespace_generic_map = if self.skip_generic_args {
            None
        } else {
            self.get_namespace_generic_map(entry_scope, &context.define_context)
        };
        // Recursion entries pushed while navigating one segment's type must not
        // leak into the next segment: each segment's traversal starts from this
        // base (the outer call-path carried in via `push`).
        let visiting_base = context.visiting.len();
        for (i, name) in path.as_slice().iter().enumerate() {
            // For a non-final path segment, prefer a container (a kind that can
            // hold `::member`) over a same-named leaf, so an item imported under
            // its package's name cannot shadow the package as a prefix.
            let prefer_container = (i + 1) != path.len();
            context.found = None;

            let generic_argument = if self.skip_generic_args {
                None
            } else {
                generic_arguments.get(i).cloned()
            };

            if context.sv_member {
                let token = Token::new(&name.to_string(), 0, 0, 0, 0, TokenSource::External);
                let symbol = Symbol::new(
                    &token,
                    SymbolKind::SystemVerilog,
                    &context.namespace(),
                    false,
                    DocComment::default(),
                );
                return Ok(ResolveResult {
                    found: std::rc::Rc::new(symbol),
                    full_path: context.full_path,
                    imported: context.imported,
                    generic_tables: context.generic_tables,
                });
            }

            if self
                .name_table
                .contains_key(&resource_table::canonical_str_id(*name))
            {
                if context.inner {
                    if let Some(symbol) = self.lookup_member(&context, *name) {
                        context.found = Some(symbol);
                        context.imported = false;
                    }
                } else {
                    match self.lookup_name(
                        context.scope,
                        *name,
                        &context.define_context,
                        prefer_container,
                    ) {
                        LexicalLookup::Found { symbol, imported } => {
                            context.found = self.symbol_table.get(&symbol);
                            context.imported = imported;
                        }
                        LexicalLookup::Ambiguous => {
                            trace!("symbol_table: {}ambiguous '{}'", context.indent(), path);
                            return Err(ResolveError::new(
                                None,
                                ResolveErrorCause::Ambiguous(*name),
                            ));
                        }
                        LexicalLookup::NotFound => {}
                    }
                }

                if let Some(found) = context.found {
                    self.check_access(&context, found).map_err(|err| {
                        let kind = if matches!(err.cause, ResolveErrorCause::Private) {
                            "private"
                        } else {
                            "invisible"
                        };
                        trace!("symbol_table: {}{} '{}'", context.indent(), kind, path);
                        err
                    })?;

                    if let Some(arguments) = generic_argument {
                        self.instantiate_generic(
                            &mut context,
                            found,
                            arguments,
                            &namespace_generic_map,
                            entry_scope,
                            &entry_define_context,
                        );
                    }

                    context.last_found = context.found;
                    context.full_path.push(found.id);

                    trace!(
                        "symbol_table: {}- path    '{}' : {} @ {}",
                        context.indent(),
                        name,
                        found.kind,
                        context.namespace(),
                    );

                    if (i + 1) < path.len() {
                        context = self.navigate_member(context, found)?;
                        context.visiting.truncate(visiting_base);
                    }
                } else {
                    trace!(
                        "symbol_table: {}not found '{}' @ {}",
                        context.indent(),
                        path,
                        context.namespace()
                    );

                    return Err(ResolveError::new(
                        context.last_found,
                        ResolveErrorCause::NotFound(*name),
                    ));
                }
            } else {
                // If symbol is not found, the name is treated as namespace
                context.set_namespace(
                    scope::inner_scope(scope::ScopeId::default(), *name),
                    DefineContext::default(),
                );
                context.inner = true;
                context.other_prj = true;
            }
        }
        if let Some(found) = context.found {
            // A member that physically lives in the base template the instance
            // cursor delegates to takes the instance's mangled namespace,
            // reconstructed from the instance-side scope path. Symbols reached
            // from elsewhere (aliases, imports, the instance's own base) keep
            // their own (table-owned) namespace.
            let found = if scope::generic_delegation(context.inst_scope) == Some(found.scope) {
                let mut found = (**found).clone();
                found.namespace = scope::namespace(context.inst_scope, &context.define_context);
                Rc::new(found)
            } else {
                Rc::clone(found)
            };

            trace!(
                "symbol_table: {}found     '{}' : {}",
                context.indent(),
                path,
                found.kind
            );

            Ok(ResolveResult {
                found,
                full_path: context.full_path,
                imported: context.imported,
                generic_tables: context.generic_tables,
            })
        } else {
            trace!("symbol_table: {}not found '{}'", context.indent(), path);

            let cause = ResolveErrorCause::NotFound(
                scope::name_path(context.scope).last().copied().unwrap(),
            );
            Err(ResolveError::new(context.last_found, cause))
        }
    }

    /// Structural counterpart of [`Self::resolve`] for a fully-formed generic
    /// instance path. Walks the segments reusing the same member-lookup and
    /// navigation machinery, but resolves a generic segment by looking up its
    /// *un-mangled* base template and navigating to the instance through the
    /// structural index — never constructing a mangled name. Returns a
    /// [`ResolveResult`] carrying the fields generic-instance callers consume;
    /// `generic_tables` is empty, matching the mangled resolution of an instance
    /// path (which passes no generic arguments, so it instantiates none). Returns
    /// `None` for cases this walk does not model (synthesized SystemVerilog
    /// members, pre-mangled degenerate paths), where the caller falls back to the
    /// mangled resolution.
    fn resolve_generic_structural<'a>(
        &'a self,
        path: &GenericSymbolPath,
        scope: scope::ScopeId,
        define_context: DefineContext,
    ) -> Option<ResolveResult> {
        let mut context = ResolveContext::from_scope(scope, define_context);
        let visiting_base = context.visiting.len();
        let segments = &path.paths;
        for (i, segment) in segments.iter().enumerate() {
            context.found = None;
            let prefer_container = (i + 1) != segments.len();

            let mut name = segment.base();
            if i == 0
                && let Some(prj) = scope::project_of(scope)
                && let Some(map) = self.project_local_table.get(&prj)
            {
                context.root_prj = false;
                if let Some(id) = map.get(&name) {
                    name = *id;
                }
            }

            // Synthesized SystemVerilog members carry a freshly allocated id each
            // time they are built, so they are neither structurally indexed nor
            // id-comparable. Defer such paths to the mangled-name fallback.
            if context.sv_member {
                return None;
            }

            if !self
                .name_table
                .contains_key(&resource_table::canonical_str_id(name))
            {
                context.set_namespace(
                    scope::inner_scope(scope::ScopeId::default(), name),
                    DefineContext::default(),
                );
                context.inner = true;
                context.other_prj = true;
                continue;
            }

            // Look up the (un-mangled) base name with the same lexical/member rules.
            let base: &'a Rc<Symbol> = if context.inner {
                let member = self.lookup_member(&context, name)?;
                context.imported = false;
                member
            } else {
                match self.lookup_name(
                    context.scope,
                    name,
                    &context.define_context,
                    prefer_container,
                ) {
                    LexicalLookup::Found { symbol, imported } => {
                        context.imported = imported;
                        self.symbol_table.get(&symbol)?
                    }
                    LexicalLookup::Ambiguous | LexicalLookup::NotFound => return None,
                }
            };
            self.check_access(&context, base).ok()?;

            // A segment whose arguments do not mangle to a distinct name (none, or
            // unresolved generic parameters) carries no instantiation — the mangled
            // walk resolves its base, so the structural walk does too. Otherwise
            // navigate to the instance through the structural index keyed by
            // (enclosing scope, base template, arguments), the identity the mangled
            // name encodes.
            let found: &'a Rc<Symbol> = if segment.mangled() == segment.base() {
                base
            } else {
                // When the base template is reached through a generic-instance
                // delegation (a nested instance, e.g. `Pkg::<1>::Struct::<2>`), the
                // enclosing scope keying the index is the parent instance's rebased
                // scope (the instance-side cursor), not the template's declaration
                // scope — mirroring the namespace rebasing applied to the final
                // found. Otherwise it is the base template's own enclosing scope.
                let enclosing = if scope::generic_delegation(context.inst_scope) == Some(base.scope)
                {
                    context.inst_scope
                } else {
                    scope::intern_namespace(&base.namespace)
                };
                let key = generic_instance_key(enclosing, base.id, &segment.arguments);
                let inst = GENERIC_INSTANCE_INDEX.with(|f| f.borrow().get(&key).copied())?;
                self.symbol_table.get(&inst)?
            };

            context.found = Some(found);
            context.last_found = context.found;
            context.full_path.push(found.id);

            if (i + 1) < segments.len() {
                context = self.navigate_member(context, found).ok()?;
                context.visiting.truncate(visiting_base);
            }
        }

        let found = context.found?;
        let found = if scope::generic_delegation(context.inst_scope) == Some(found.scope) {
            let mut found = (**found).clone();
            found.namespace = scope::namespace(context.inst_scope, &context.define_context);
            Rc::new(found)
        } else {
            Rc::clone(found)
        };
        Some(ResolveResult {
            found,
            full_path: context.full_path,
            imported: context.imported,
            generic_tables: context.generic_tables,
        })
    }

    fn get_namespace_generic_map(
        &self,
        scope: scope::ScopeId,
        define_context: &DefineContext,
    ) -> Option<Rc<Vec<GenericMap>>> {
        if scope::depth(scope) <= 1 {
            return None;
        }
        let key = (scope, define_context.clone());
        if let Some(cached) = NS_GENERIC_MAP_CACHE.with(|f| f.borrow().get(&key).cloned()) {
            return cached;
        }
        let namespace = scope::namespace(scope, define_context);
        let ret = self
            .get_namespace_generic_map_inner(&namespace)
            .map(Rc::new);
        NS_GENERIC_MAP_CACHE.with(|f| f.borrow_mut().insert(key, ret.clone()));
        ret
    }

    fn get_namespace_generic_map_inner(&self, namespace: &Namespace) -> Option<Vec<GenericMap>> {
        let mut namespace = namespace.clone();
        namespace.strip_anonymous_path();

        let path = namespace.pop().map(|x| SymbolPath::new(&[x]))?;
        let context = ResolveContext::new(&namespace);
        let param_list = self
            .resolve(&path, &[], context)
            .map(|x| self.collect_generic_bounds(&x.found))
            .ok()?;

        if param_list.is_empty() {
            return None;
        }

        let mut maps = Vec::new();
        for params in param_list {
            let mut map = GenericTable::default();
            for (key, bound) in params {
                if let GenericBoundKind::Proto(x) = bound {
                    let TypeKind::UserDefined(x) = x.kind else {
                        continue;
                    };
                    map.insert(key, x.path);
                }
            }

            maps.push(GenericMap { id: None, map });
        }

        Some(maps)
    }

    fn collect_generic_bounds(&self, symbol: &Symbol) -> Vec<Vec<(StrId, GenericBoundKind)>> {
        fn collect_generic_params(
            symbol_table: &SymbolTable,
            generic_params: &[SymbolId],
        ) -> Vec<(StrId, GenericBoundKind)> {
            generic_params
                .iter()
                .map(|param| {
                    let symbol = symbol_table.get(*param).unwrap();
                    let SymbolKind::GenericParameter(x) = symbol.kind else {
                        unreachable!();
                    };
                    (symbol.token.text, x.bound)
                })
                .collect()
        }

        // Namespace::get_symbol (used in Symbol::get_parent) uses symbol_table::resolve.
        // This causes 'RefCell already mutably borrowed' error.
        // The function below is to prevent this error.
        fn get_parent_symbol(symbol_table: &SymbolTable, symbol: &Symbol) -> Option<Symbol> {
            let mut namespace = symbol.namespace.clone();
            namespace.strip_anonymous_path();

            if let Some(path) = namespace.pop()
                && namespace.depth() >= 1
            {
                let context = ResolveContext::new(&namespace);
                let symbol_path = SymbolPath::new(&[path]);
                symbol_table
                    .resolve(&symbol_path, &[], context)
                    .map(|x| (*x.found).clone())
                    .ok()
            } else {
                None
            }
        }

        match &symbol.kind {
            SymbolKind::Module(x) => vec![collect_generic_params(self, &x.generic_parameters)],
            SymbolKind::Interface(x) => vec![collect_generic_params(self, &x.generic_parameters)],
            SymbolKind::Package(x) if !x.is_proto => {
                vec![collect_generic_params(self, &x.generic_parameters)]
            }
            SymbolKind::GenericInstance(x) => {
                let symbol = self.get(x.base).unwrap();
                self.collect_generic_bounds(&symbol)
            }
            SymbolKind::Function(x) if !x.is_proto => {
                let mut bounds = if let Some(parent) = get_parent_symbol(self, symbol) {
                    self.collect_generic_bounds(&parent)
                } else {
                    vec![]
                };
                bounds.push(collect_generic_params(self, &x.generic_parameters));
                bounds
            }
            SymbolKind::Struct(x) => {
                let mut bounds = if let Some(parent) = get_parent_symbol(self, symbol) {
                    self.collect_generic_bounds(&parent)
                } else {
                    vec![]
                };
                bounds.push(collect_generic_params(self, &x.generic_parameters));
                bounds
            }
            SymbolKind::Union(x) => {
                let mut bounds = if let Some(parent) = get_parent_symbol(self, symbol) {
                    self.collect_generic_bounds(&parent)
                } else {
                    vec![]
                };
                bounds.push(collect_generic_params(self, &x.generic_parameters));
                bounds
            }
            SymbolKind::Block => {
                if let Some(parent) = get_parent_symbol(self, symbol) {
                    self.collect_generic_bounds(&parent)
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    fn append_project_path(
        &self,
        path: &mut GenericSymbolPath,
        scope: scope::ScopeId,
        define_context: &DefineContext,
    ) {
        let namespace = scope::namespace(scope, define_context);
        let context = ResolveContext::new(&namespace);
        let Ok(symbol) = self.resolve(&path.base_path(0), &[], context) else {
            return;
        };
        if !matches!(
            symbol.found.kind,
            SymbolKind::Module(_)
                | SymbolKind::AliasModule(_)
                | SymbolKind::Interface(_)
                | SymbolKind::AliasInterface(_)
                | SymbolKind::Package(_)
                | SymbolKind::AliasPackage(_)
        ) {
            return;
        }

        if let Some(prj) = scope::project_of(scope)
            && let Some(project_symbol) = self.find_project_symbol(prj)
        {
            let project_path = GenericSymbol {
                base: project_symbol.token,
                arguments: vec![],
            };
            path.paths.insert(0, project_path);
        }
    }

    pub fn get_all(&self) -> Vec<Symbol> {
        let mut ret = Vec::new();
        for symbol in self.symbol_table.values() {
            ret.push((**symbol).clone());
        }
        ret
    }

    pub fn dump(&self) -> String {
        format!("{self}")
    }

    pub fn drop(&mut self, file_path: PathId) {
        let drop_list: Vec<_> = self
            .symbol_table
            .iter()
            .filter(|x| x.1.token.source == file_path)
            .map(|x| *x.0)
            .collect();

        for id in &drop_list {
            if let Some(symbol) = self.symbol_table.get(id)
                && let Some(ids) = self.namespace_index.get_mut(&symbol.namespace.paths)
            {
                ids.retain(|x| x != id);
            }
            self.symbol_table.remove(id);
            self.reference_table.remove(id);
        }

        for symbols in self.name_table.values_mut() {
            symbols.retain(|x| !drop_list.contains(x));
        }

        for tokens in self.reference_table.values_mut() {
            tokens.retain(|x| x.source != file_path);
        }
    }

    pub fn add_reference(&mut self, target: SymbolId, token: &Token) {
        if self.symbol_table.contains_key(&target) {
            self.reference_table
                .entry(target)
                .or_default()
                .push(token.to_owned());
        }
    }

    /// Records the pending-list lengths before one file's pass1, so the
    /// file's additions can be exported afterwards for fragment caching.
    pub fn pending_watermark(&self) -> PendingWatermark {
        PendingWatermark {
            import: self.import_list.len(),
            bind: self.bind_list.len(),
            msb: self.msb_list.len(),
            connect: self.connect_list.len(),
        }
    }

    /// Exports everything one file's pass1 wrote into the symbol table:
    /// the symbols in the file's ID window plus the pending entries added
    /// since the watermark. Must be called before `analyze_post_pass1`
    /// (which drains the pending lists and mutates symbols).
    pub fn export_fragment(
        &self,
        symbol_window_start: usize,
        symbol_window_end: usize,
        watermark: &PendingWatermark,
    ) -> SymbolTableFragment {
        let in_window = |id: &SymbolId| id.0 > symbol_window_start && id.0 <= symbol_window_end;

        let mut symbols: Vec<Symbol> = self
            .symbol_table
            .iter()
            .filter(|(id, _)| in_window(id))
            .map(|(_, symbol)| (**symbol).clone())
            .collect();
        symbols.sort_unstable_by_key(|x| x.id);

        let mut reference_functions: Vec<_> = self
            .reference_func_table
            .iter()
            .filter(|(id, _)| in_window(id))
            .map(|(id, x)| (*id, x.clone()))
            .collect();
        reference_functions.sort_unstable_by_key(|(id, _)| *id);

        let mut references: Vec<_> = self
            .reference_table
            .iter()
            .filter(|(id, _)| in_window(id))
            .map(|(id, x)| (*id, x.clone()))
            .collect();
        references.sort_unstable_by_key(|(id, _)| *id);

        SymbolTableFragment {
            symbols,
            imports: self.import_list[watermark.import..].to_vec(),
            binds: self.bind_list[watermark.bind..].to_vec(),
            msbs: self.msb_list[watermark.msb..].to_vec(),
            connects: self.connect_list[watermark.connect..].to_vec(),
            reference_functions,
            references,
        }
    }

    /// Re-inserts a previously exported fragment. Symbol IDs must already
    /// be rebased onto fresh ranges by the fragment codec. Fails if any
    /// symbol conflicts with an existing one; the caller is expected to
    /// fall back to a regular parse + pass1 after `drop`ping the file.
    pub fn restore_fragment(&mut self, fragment: SymbolTableFragment) -> Result<(), Box<Symbol>> {
        for mut symbol in fragment.symbols {
            // `scope` is not serialized (runtime-only intern handle); re-derive
            // it from the namespace so the restored binding lands in the right
            // scope instead of the default root.
            symbol.scope = scope::intern_namespace(&symbol.namespace);
            let token = symbol.token;
            let id = symbol.id;
            let scope = symbol.scope;
            let owned_scope_kind = scope::scope_kind_of(&symbol.kind);
            if self.insert(&token, symbol.clone()).is_none() {
                return Err(Box::new(symbol));
            }
            // Re-establish the owned inner scope's kind/owner, which pass1 sets
            // in `insert_symbol` but the serialized fragment does not carry.
            // Without this, `scope::owner_of` is `None` for restored
            // declarations and structural navigation falls back to name
            // resolution on warm builds.
            if let Some(kind) = owned_scope_kind {
                let owned = scope::intern_child(scope, token.text, kind);
                scope::set_kind_owner(owned, kind, id);
            }
        }
        self.import_list.extend(fragment.imports);
        self.bind_list.extend(fragment.binds);
        self.msb_list.extend(fragment.msbs);
        self.connect_list.extend(fragment.connects);
        for (id, functions) in fragment.reference_functions {
            self.reference_func_table.insert(id, functions);
        }
        for (id, tokens) in fragment.references {
            self.reference_table.entry(id).or_default().extend(tokens);
        }
        Ok(())
    }

    pub fn add_generic_instance(&mut self, target: SymbolId, instance: SymbolId) {
        if let Some(symbol) = self.symbol_table.get_mut(&target).map(Rc::make_mut)
            && !symbol.generic_instances.contains(&instance)
        {
            symbol.generic_instances.push(instance);
        }
    }

    pub fn update_generic_instance_affiliation(&mut self, target: SymbolId, instance: &Symbol) {
        let Some(target_inst_ids) = self
            .symbol_table
            .get(&target)
            .map(|x| x.generic_instances.clone())
        else {
            return;
        };
        for target_inst_id in &target_inst_ids {
            if let Some(target_inst_symbol) =
                self.symbol_table.get_mut(target_inst_id).map(Rc::make_mut)
                && target_inst_symbol.token.text == instance.token.text
            {
                let SymbolKind::GenericInstance(target_inst) = &mut target_inst_symbol.kind else {
                    unreachable!();
                };
                let SymbolKind::GenericInstance(inst) = &instance.kind else {
                    unreachable!();
                };
                if let Some(affiliation_symbol) = inst.affiliation_symbols.first()
                    && !target_inst.affiliation_symbols.contains(affiliation_symbol)
                {
                    target_inst.affiliation_symbols.push(*affiliation_symbol);
                }
                break;
            }
        }
    }

    pub fn add_import(&mut self, import: Import) {
        self.import_list.push(import);
    }

    pub fn apply_import(&mut self) {
        // Set skip_generic_args to true to skip applying generic args during resolving the path.
        // symbol_table::get is used when applying generic args but it is forbidden in
        // apply import which borrows `&mut self`.
        self.skip_generic_args = true;

        let import_list: Vec<_> = self.import_list.drain(0..).collect();
        for import in &import_list {
            let context = ResolveContext::new(&import.path.1);
            let Ok((symbol, imported)) = self
                .resolve(&import.path.0.generic_path(), &[], context)
                .map(|x| ((*x.found).clone(), x.imported))
            else {
                continue;
            };

            // The import target scope records the binding; resolution and
            // imported-path expansion read it from the scope tree.
            let target_scope = scope::intern_namespace(&import.namespace);
            let define_context = import.namespace.define_context.clone();

            if import.wildcard {
                let wildcard_target = if matches!(symbol.kind, SymbolKind::Enum(_)) {
                    Some(symbol.clone())
                } else {
                    self.get_package(&symbol, false, imported)
                };
                if let Some(target_symbol) = wildcard_target {
                    let target = target_symbol.inner_namespace();
                    let source_scope = scope::intern_namespace(&target);
                    scope::add_wildcard(
                        target_scope,
                        source_scope,
                        define_context,
                        target.define_context.clone(),
                        import.path.0.clone(),
                    );
                }
            } else if !matches!(symbol.kind, SymbolKind::SystemVerilog) {
                let mut package_path = import.path.0.clone();
                package_path.paths.pop();
                scope::add_import(
                    target_scope,
                    symbol.token.text,
                    symbol.id,
                    define_context,
                    package_path,
                );
            }
        }

        self.skip_generic_args = false;
    }

    fn get_package(
        &self,
        symbol: &Symbol,
        include_proto: bool,
        include_proto_alias: bool,
    ) -> Option<Symbol> {
        match &symbol.kind {
            SymbolKind::Package(x) if x.is_proto => {
                if include_proto {
                    return Some(symbol.clone());
                }
            }
            SymbolKind::Package(_) => return Some(symbol.clone()),
            SymbolKind::AliasPackage(x) if !x.is_proto || include_proto_alias => {
                let context = ResolveContext::new(&symbol.namespace);
                if let Ok(symbol) = self.resolve(&x.target.generic_path(), &[], context) {
                    let include_proto = if x.is_proto { true } else { include_proto };
                    return self.get_package(&symbol.found, include_proto, false);
                }
            }
            SymbolKind::GenericInstance(x) => {
                let symbol = self.get(x.base).unwrap();
                return self.get_package(&symbol, false, false);
            }
            SymbolKind::GenericParameter(x) => {
                if let GenericBoundKind::Proto(proto) = &x.bound
                    && let Some(x) = proto.get_user_defined()
                {
                    let context = ResolveContext::new(&symbol.namespace);
                    if let Ok(symbol) = self.resolve(&x.path.generic_path(), &[], context) {
                        return self.get_package(&symbol.found, true, false);
                    }
                }
            }
            _ => {}
        }

        None
    }

    pub fn resolve_interfaces(&mut self) -> Vec<AnalyzerError> {
        let mut errors = Vec::new();
        let mut visited = Vec::new();

        let interface_list: Vec<_> = self
            .symbol_table
            .values()
            .filter_map(|x| {
                if matches!(x.kind, SymbolKind::Interface(_)) {
                    Some(x.id)
                } else {
                    None
                }
            })
            .collect();

        self.skip_generic_args = true;
        for id in interface_list {
            let if_symbol = self.get(id).unwrap();
            self.resolve_interface(&if_symbol, &mut errors, &mut visited);
        }
        self.skip_generic_args = false;

        errors
    }

    fn resolve_interface(
        &mut self,
        if_symbol: &Symbol,
        errors: &mut Vec<AnalyzerError>,
        visited: &mut Vec<SymbolId>,
    ) {
        if visited.contains(&if_symbol.id) {
            // This interface has already been resolved.
            return;
        }
        visited.push(if_symbol.id);

        let mut members = BTreeMap::default();
        let mut modports = BTreeMap::default();

        self.collect_interface_members(if_symbol, true, &mut members, &mut modports);
        self.apply_interface_inheritances(if_symbol, &mut members, &mut modports, errors, visited);

        for (modport, is_own_member) in modports.values() {
            if !is_own_member {
                continue;
            }
            self.link_modport_members(*modport, &members);
        }
        self.link_modports(if_symbol, &members, &modports);
    }

    fn collect_interface_members(
        &self,
        symbol: &Symbol,
        is_own_member: bool,
        members: &mut BTreeMap<StrId, SymbolId>,
        modports: &mut BTreeMap<StrId, (SymbolId, bool)>,
    ) {
        if let SymbolKind::Interface(x) = &symbol.kind {
            for id in &x.members {
                let symbol = self.symbol_table.get(id).unwrap();
                members.insert(symbol.token.text, symbol.id);
                if matches!(symbol.kind, SymbolKind::Modport(_)) {
                    modports.insert(symbol.token.text, (symbol.id, is_own_member));
                }
            }
        }
    }

    fn apply_interface_inheritances(
        &mut self,
        symbol: &Symbol,
        members: &mut BTreeMap<StrId, SymbolId>,
        modports: &mut BTreeMap<StrId, (SymbolId, bool)>,
        errors: &mut Vec<AnalyzerError>,
        visited: &mut Vec<SymbolId>,
    ) {
        let ancestor_paths = if let SymbolKind::Interface(x) = &symbol.kind {
            &x.ancestors
        } else {
            return;
        };

        let if_namespace = symbol.inner_namespace();
        for ancestor_path in ancestor_paths {
            let Some(ancestor_symbol) =
                self.resolve_ancestor_interface(ancestor_path, &if_namespace)
            else {
                continue;
            };

            if !self.is_valid_ancestor_interface(symbol, &ancestor_symbol, members, errors) {
                continue;
            }

            let target_scope = scope::intern_namespace(&if_namespace);
            let target_define_context = if_namespace.define_context.clone();
            let source_namespace = ancestor_symbol.inner_namespace();
            let source_scope = scope::intern_namespace(&source_namespace);
            scope::add_inherited_interface(
                target_scope,
                source_scope,
                target_define_context,
                source_namespace.define_context.clone(),
                ancestor_path.clone(),
            );

            // The ancestor interface needs to be resolved before collecting its members.
            self.resolve_interface(&ancestor_symbol, errors, visited);
            self.collect_interface_members(&ancestor_symbol, false, members, modports);
        }
    }

    fn resolve_ancestor_interface(
        &self,
        ancestor_path: &GenericSymbolPath,
        namespace: &Namespace,
    ) -> Option<Rc<Symbol>> {
        let Ok(symbol) = self.resolve_generic_path(ancestor_path, namespace) else {
            return None;
        };
        if let SymbolKind::AliasInterface(x) = &symbol.found.kind {
            self.resolve_ancestor_interface(&x.target, namespace)
        } else {
            Some(symbol.found)
        }
    }

    fn is_valid_ancestor_interface(
        &self,
        if_symbol: &Symbol,
        ancestor_symbol: &Symbol,
        members: &BTreeMap<StrId, SymbolId>,
        errors: &mut Vec<AnalyzerError>,
    ) -> bool {
        if matches!(ancestor_symbol.kind, SymbolKind::GenericParameter(_))
            || !ancestor_symbol.is_interface(false)
        {
            errors.push(AnalyzerError::invalid_inheritance(
                &ancestor_symbol.token.to_string(),
                "it is not interface",
                &ancestor_symbol.token.into(),
            ));
            return false;
        }

        if ancestor_symbol.id == if_symbol.id {
            errors.push(AnalyzerError::invalid_inheritance(
                &ancestor_symbol.token.to_string(),
                "it inherits itself",
                &ancestor_symbol.token.into(),
            ));
            return false;
        }

        if ancestor_symbol.kind.has_parameters() {
            errors.push(AnalyzerError::invalid_inheritance(
                &ancestor_symbol.token.to_string(),
                "it is parameterized",
                &ancestor_symbol.token.into(),
            ));
            return false;
        }

        let SymbolKind::Interface(ancestor_prop) = &ancestor_symbol.kind else {
            unreachable!();
        };

        if !ancestor_prop.ancestors.is_empty() {
            errors.push(AnalyzerError::invalid_inheritance(
                &ancestor_symbol.token.to_string(),
                "it inherits from other interfaces",
                &ancestor_symbol.token.into(),
            ));
            return false;
        }

        for member in &ancestor_prop.members {
            let member_symbol = self.symbol_table.get(member).unwrap();
            if members.contains_key(&member_symbol.token.text) {
                errors.push(AnalyzerError::invalid_inheritance(
                    &ancestor_symbol.token.to_string(),
                    &format!("{} is already defined", member_symbol.token.text),
                    &ancestor_symbol.token.into(),
                ));
                return false;
            }
        }

        true
    }

    fn link_modport_members(&mut self, modport: SymbolId, members: &BTreeMap<StrId, SymbolId>) {
        let mp_symbol = self.get(modport).unwrap();
        let SymbolKind::Modport(mp) = &mp_symbol.kind else {
            unreachable!();
        };

        for mp_member_id in &mp.members {
            let mut mp_member = self.get(*mp_member_id).unwrap();
            let Some(target_id) = members.get(&mp_member.token.text) else {
                continue;
            };
            match mp_member.kind {
                SymbolKind::ModportFunctionMember(_) => {
                    mp_member.kind =
                        SymbolKind::ModportFunctionMember(ModportFunctionMemberProperty {
                            function: *target_id,
                        });
                    self.update(mp_member);
                }
                SymbolKind::ModportVariableMember(x) => {
                    mp_member.kind =
                        SymbolKind::ModportVariableMember(ModportVariableMemberProperty {
                            variable: *target_id,
                            direction: x.direction,
                        });
                    self.update(mp_member);
                }
                _ => unreachable!(),
            }
        }
    }

    fn link_modports(
        &mut self,
        if_symbol: &Symbol,
        members: &BTreeMap<StrId, SymbolId>,
        modports: &BTreeMap<StrId, (SymbolId, bool)>,
    ) {
        for (mp_id, is_own_member) in modports.values() {
            if !is_own_member {
                continue;
            }

            if let Some(mut mp_symbol) = self.get(*mp_id)
                && let SymbolKind::Modport(mp) = &mp_symbol.kind
            {
                let mut default_members =
                    self.collect_modport_default_members(&mp_symbol, members, modports);

                let mut members = mp.members.clone();
                members.append(&mut default_members);

                mp_symbol.kind = SymbolKind::Modport(ModportProperty {
                    interface: if_symbol.id,
                    members,
                    default: mp.default.clone(),
                });
                self.update(mp_symbol);
            }
        }
    }

    fn collect_modport_default_members(
        &mut self,
        modport_symbol: &Symbol,
        members: &BTreeMap<StrId, SymbolId>,
        modports: &BTreeMap<StrId, (SymbolId, bool)>,
    ) -> Vec<SymbolId> {
        let symbol_table = self;

        let mut ret = Vec::new();

        if let SymbolKind::Modport(x) = &modport_symbol.kind
            && let Some(default) = &x.default
        {
            let default_member_directions =
                symbol_table.collect_modport_default_member_target_directions(default, modports);
            let explicit_members: HashSet<_> = x
                .members
                .iter()
                .map(|x| symbol_table.get(*x).unwrap().token.text)
                .collect();
            let mut default_members: Vec<_> = members
                .iter()
                .filter(|(text, id)| {
                    if explicit_members.contains(text) {
                        false
                    } else if matches!(default, ModportDefault::Same(_)) {
                        true
                    } else {
                        symbol_table
                            .get(**id)
                            .map(|x| matches!(x.kind, SymbolKind::Variable(_)))
                            .unwrap_or(false)
                    }
                })
                .collect();
            // Sort by SymbolId to keep inserting order as the same as definition order
            default_members.sort_by(|x, y| x.1.cmp(y.1));

            let namespace = modport_symbol.inner_namespace();
            for (text, id) in default_members {
                let direction = match default {
                    ModportDefault::Input => Some(Direction::Input),
                    ModportDefault::Output => Some(Direction::Output),
                    ModportDefault::Same(_) => default_member_directions.get(text).copied(),
                    ModportDefault::Converse(_) => {
                        default_member_directions.get(text).map(|x| x.converse())
                    }
                };
                let Some(direction) = direction else {
                    continue;
                };

                let source_path = modport_symbol.token.source.get_path().unwrap();
                let token = Token::generate(*text, source_path);
                scope::insert_token(token.id, source_path, &namespace);
                symbol_table.add_reference(*id, &token);

                let kind = if matches!(direction, Direction::Import) {
                    SymbolKind::ModportFunctionMember(ModportFunctionMemberProperty {
                        function: *id,
                    })
                } else {
                    SymbolKind::ModportVariableMember(ModportVariableMemberProperty {
                        direction,
                        variable: *id,
                    })
                };
                let symbol = Symbol::new(&token, kind, &namespace, false, DocComment::default());
                if let Some(id) = symbol_table.insert(&token, symbol) {
                    ret.push(id);
                }
            }
        }

        ret
    }

    fn collect_modport_default_member_target_directions(
        &self,
        default: &ModportDefault,
        modports: &BTreeMap<StrId, (SymbolId, bool)>,
    ) -> HashMap<StrId, Direction> {
        let mut ret = HashMap::default();

        match default {
            ModportDefault::Same(targets) | ModportDefault::Converse(targets) => {
                for target in targets {
                    let Some((mp_id, _)) = modports.get(&target.text) else {
                        continue;
                    };
                    let Some(mp_symbol) = self.get(*mp_id) else {
                        continue;
                    };
                    let SymbolKind::Modport(mp) = mp_symbol.kind else {
                        continue;
                    };

                    for member in &mp.members {
                        let Some(member_symbol) = self.get(*member) else {
                            continue;
                        };

                        if let SymbolKind::ModportVariableMember(member) = member_symbol.kind {
                            ret.insert(member_symbol.token.text, member.direction);
                        } else if matches!(default, ModportDefault::Same(_))
                            && matches!(member_symbol.kind, SymbolKind::ModportFunctionMember(_))
                        {
                            ret.insert(member_symbol.token.text, Direction::Import);
                        }
                    }
                }
            }
            _ => {}
        }

        ret
    }

    pub fn add_bind(&mut self, bind: Bind) {
        self.bind_list.push(bind);
    }

    pub fn apply_bind(&mut self) -> Vec<AnalyzerError> {
        let mut errors = Vec::new();

        let bind_list: Vec<Bind> = self.bind_list.drain(0..).collect();
        for bind in bind_list {
            let Ok(target) = self.resolve_generic_path(&bind.target.0, &bind.target.1) else {
                continue;
            };

            let Some(namespace) = self.resolve_bind_target_namespace(&target.found) else {
                continue;
            };
            let symbol = Symbol::new(
                &bind.token,
                SymbolKind::Instance(bind.property.clone()),
                &namespace,
                false,
                bind.doc_comment.clone(),
            );

            if self.insert(&bind.token, symbol).is_some() {
                if let TokenSource::File { path, .. } = bind.token.source {
                    scope::insert_token(bind.token.id, path, &namespace);

                    for target in bind.property.parameter_connects.values() {
                        Self::update_connect_target_namespace(target, path, &namespace);
                    }
                    for target in bind.property.port_connects.values() {
                        Self::update_connect_target_namespace(target, path, &namespace);
                    }
                }
            } else {
                errors.push(AnalyzerError::duplicated_identifier(
                    &bind.token.to_string(),
                    DuplicatedIdentifierKind::Normal,
                    &bind.token.into(),
                ));
            }
        }

        errors
    }

    fn add_msb(&mut self, msb: Msb) {
        self.msb_list.push(msb);
    }

    fn get_msb(&mut self) -> Vec<Msb> {
        self.msb_list.drain(0..).collect()
    }

    fn add_connect(&mut self, connect: Connect) {
        self.connect_list.push(connect);
    }

    fn get_connect(&mut self) -> Vec<Connect> {
        self.connect_list.drain(0..).collect()
    }

    fn resolve_generic_path(
        &self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
    ) -> Result<ResolveResult, ResolveError> {
        let context = ResolveContext::new(namespace);
        self.resolve(&path.generic_path(), &[], context)
    }

    fn resolve_bind_target_namespace(&mut self, target: &Symbol) -> Option<Namespace> {
        match &target.kind {
            SymbolKind::Module(_) => Some(target.inner_namespace()),
            SymbolKind::AliasModule(x) => {
                let Ok(target) = self.resolve_generic_path(&x.target, &target.namespace) else {
                    return None;
                };
                self.resolve_bind_target_namespace(&target.found)
            }
            SymbolKind::Interface(_) => Some(target.inner_namespace()),
            SymbolKind::AliasInterface(x) => {
                let Ok(target) = self.resolve_generic_path(&x.target, &target.namespace) else {
                    return None;
                };
                self.resolve_bind_target_namespace(&target.found)
            }
            _ => None,
        }
    }

    fn update_connect_target_namespace(
        target: &ConnectTarget,
        path: PathId,
        namespace: &Namespace,
    ) {
        let mut collector = TokenCollector::new(false);
        collector.expression(&target.expression);

        for token in &collector.tokens {
            scope::insert_token(token.id, path, namespace);
        }
    }

    pub fn resolve_user_defined(&self) -> Vec<(SymbolId, SymbolId)> {
        let mut resolved = Vec::new();
        for symbol in self.symbol_table.values() {
            if let Some(x) = symbol.kind.get_type()
                && let TypeKind::UserDefined(x) = &x.kind
            {
                let context = ResolveContext::new(&symbol.namespace);
                if let Ok(type_symbol) =
                    self.resolve(&x.path.generic_path(), &x.path.generic_arguments(), context)
                {
                    resolved.push((symbol.id, type_symbol.found.id));
                }
            }
        }
        resolved
    }

    pub fn set_user_defined(&mut self, resolved: Vec<(SymbolId, SymbolId)>) {
        for (id, type_id) in resolved {
            let symbol = Rc::make_mut(self.symbol_table.get_mut(&id).unwrap());
            if let Some(x) = symbol.kind.get_type_mut()
                && let TypeKind::UserDefined(x) = &mut x.kind
            {
                x.symbol = Some(type_id);
            }
        }
    }

    pub fn get_function_symbols(&self) -> Vec<Symbol> {
        self.symbol_table
            .values()
            .filter_map(|symbol| {
                if matches!(symbol.kind, SymbolKind::Function(_)) {
                    Some((**symbol).clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_enum_symbols(&self) -> Vec<Symbol> {
        self.symbol_table
            .values()
            .filter_map(|symbol| {
                if matches!(symbol.kind, SymbolKind::Enum(_)) {
                    Some((**symbol).clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn find_project_symbol(&self, prj: StrId) -> Option<Symbol> {
        for symbol in self.symbol_table.values() {
            if matches!(symbol.kind, SymbolKind::Namespace) && symbol.token.text == prj {
                return Some((**symbol).clone());
            }
        }

        None
    }

    pub fn add_project_local(&mut self, prj: StrId, from: StrId, to: StrId) {
        self.project_local_table
            .entry(prj)
            .and_modify(|x| {
                x.insert(from, to);
            })
            .or_insert(HashMap::from_iter([(from, to)]));
    }

    pub fn add_reference_functions(&mut self, id: SymbolId, functions: Vec<GenericSymbolPath>) {
        self.reference_func_table.insert(id, functions);
    }

    pub fn get_reference_functions(&self, id: SymbolId) -> Option<Vec<GenericSymbolPath>> {
        self.reference_func_table.get(&id).cloned()
    }

    pub fn get_references(&self, id: SymbolId) -> Option<Vec<Token>> {
        self.reference_table.get(&id).cloned()
    }

    pub fn clear(&mut self) {
        // The scope arena must be reset before `Self::new` re-registers builtin
        // symbols (which intern their scopes), so the two tables stay in sync.
        scope::clear();
        self.clone_from(&Self::new());
    }

    fn check_unused_variable(&self) -> Vec<AnalyzerError> {
        let mut ret = vec![];
        for symbol in self.symbol_table.values() {
            if let SymbolKind::Variable(_) = symbol.kind
                && self
                    .reference_table
                    .get(&symbol.id)
                    .is_none_or(|v| v.is_empty())
                && !symbol.allow_unused
            {
                let name = symbol.token.to_string();
                if !name.starts_with('_') {
                    ret.push(AnalyzerError::unused_variable(
                        &symbol.token.to_string(),
                        &symbol.token.into(),
                    ));
                }
            }
        }
        ret
    }

    fn get_tests(&self, project_name: &str) -> Vec<(StrId, TestProperty)> {
        self.symbol_table
            .values()
            .filter(|s| s.namespace.to_string() == project_name)
            .filter_map(|symbol| match &symbol.kind {
                SymbolKind::Module(x) if x.test.is_some() => {
                    Some((symbol.token.text, x.test.clone().unwrap()))
                }
                SymbolKind::Test(x) => Some((symbol.token.text, x.clone())),
                _ => None,
            })
            .collect()
    }

    fn get_doc_tests(&self, project_name: &str) -> Vec<DocTestTarget> {
        self.symbol_table
            .values()
            .filter(|s| s.namespace.to_string() == project_name)
            .filter_map(|symbol| {
                if let SymbolKind::Module(ref module_prop) = symbol.kind {
                    let wavedrom_json = symbol.doc_comment.extract_wavedrom_test()?;
                    let ports = module_prop
                        .ports
                        .iter()
                        .map(|p| {
                            let prop = p.property();
                            let name = veryl_parser::resource_table::get_str_value(p.name())
                                .unwrap_or_default();
                            let dir = match prop.direction {
                                Direction::Input => "input",
                                Direction::Output => "output",
                                Direction::Inout => "inout",
                                _ => "unknown",
                            };
                            (name, dir.to_string())
                        })
                        .collect();
                    let path = symbol.token.source.get_path()?;
                    Some(DocTestTarget {
                        module_name: symbol.token.text,
                        wavedrom_json,
                        ports,
                        path,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    fn check_wavedrom(&self) -> Vec<AnalyzerError> {
        let mut ret = vec![];
        for symbol in self.symbol_table.values() {
            if let SymbolKind::Module(ref module_prop) = symbol.kind {
                let port_names: Vec<String> = module_prop
                    .ports
                    .iter()
                    .map(|p| {
                        veryl_parser::resource_table::get_str_value(p.name()).unwrap_or_default()
                    })
                    .collect();
                ret.extend(wavedrom::check_wavedrom(
                    &symbol.token,
                    symbol.doc_comment.extract_wavedrom_block(false),
                    symbol.doc_comment.extract_wavedrom_block(true),
                    &port_names,
                ));
            }
        }
        ret
    }
}

impl fmt::Display for SymbolTable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "SymbolTable [")?;
        let mut symbol_width = 0;
        let mut namespace_width = 0;
        let mut reference_width = 0;
        let mut vec: Vec<_> = self.name_table.iter().collect();
        vec.sort_by(|x, y| format!("{}", x.0).cmp(&format!("{}", y.0)));
        for (k, v) in &vec {
            symbol_width = symbol_width.max(format!("{k}").len());
            for id in *v {
                let symbol = self.symbol_table.get(id).unwrap();
                namespace_width = namespace_width.max(format!("{}", symbol.namespace).len());
                reference_width = reference_width
                    .max(format!("{}", self.reference_table.get(id).map_or(0, |v| v.len())).len());
            }
        }
        for (k, v) in &vec {
            for id in *v {
                let symbol = self.symbol_table.get(id).unwrap();
                writeln!(
                    f,
                    "    {:symbol_width$} @ {:namespace_width$} {{ref: {:reference_width$}}}: {},",
                    k,
                    symbol.namespace,
                    self.reference_table.get(id).map_or(0, |v| v.len()),
                    symbol.kind,
                    symbol_width = symbol_width,
                    namespace_width = namespace_width,
                    reference_width = reference_width,
                )?;
            }
        }
        writeln!(f, "]")?;

        Ok(())
    }
}

/// Kind of recursion a symbol is being re-entered through, paired with its
/// `SymbolId` on `ResolveContext::visiting` to terminate cycles. Distinct
/// phases let the same symbol legitimately appear once per phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ResolvePhase {
    /// Following an alias (`alias module`/`interface`/`package`) to its target.
    Alias,
    /// Tracing a user-defined type to the scope holding its members.
    TypeTrace,
}

#[derive(Clone)]
struct ResolveContext<'a> {
    found: Option<&'a Rc<Symbol>>,
    last_found: Option<&'a Rc<Symbol>>,
    last_found_type: Option<SymbolId>,
    full_path: Vec<SymbolId>,
    /// Scope-tree cursor: the scope reference resolution searches from. Derived
    /// structurally (a symbol's `scope`/`inner_scope`) rather than by carrying a
    /// `Namespace`, which the cursor used to hold.
    scope: scope::ScopeId,
    /// Instance-side counterpart of `scope`: members are looked up in `scope`
    /// (where the base template's members physically live), but their emission
    /// namespace is reconstructed from `inst_scope`, which carries the mangled
    /// generic-instance prefix through navigation. This is the delegating view —
    /// members are not cloned per instance.
    inst_scope: scope::ScopeId,
    /// ifdef context of the cursor, kept alongside `scope` because it is not
    /// part of the scope's interned identity.
    define_context: DefineContext,
    generic_tables: GenericTables,
    /// Symbols currently being resolved through (alias expansion / type
    /// tracing). Re-entering a `(SymbolId, ResolvePhase)` already present is a
    /// cycle. Entries are pushed while descending and truncated back to the
    /// segment's base in the resolve loop, giving call-path (not monotonic)
    /// scope. Carried into nested resolves via `push`.
    visiting: SVec<(SymbolId, ResolvePhase)>,
    inner: bool,
    other_prj: bool,
    root_prj: bool,
    sv_member: bool,
    imported: bool,
    depth: usize,
}

impl ResolveContext<'_> {
    fn from_scope(scope: scope::ScopeId, define_context: DefineContext) -> Self {
        Self {
            found: None,
            last_found: None,
            last_found_type: None,
            full_path: vec![],
            scope,
            inst_scope: scope,
            define_context,
            generic_tables: GenericTables::default(),
            visiting: SVec::new(),
            inner: false,
            other_prj: false,
            root_prj: true,
            sv_member: false,
            imported: false,
            depth: 0,
        }
    }

    fn new(namespace: &Namespace) -> Self {
        Self::from_scope(
            scope::intern_namespace(namespace),
            namespace.define_context.clone(),
        )
    }

    fn push(&self) -> Self {
        // A nested resolve does not inherit the instance context: `inst_scope`
        // starts from the (base-side) cursor, scoped to a single resolve call.
        let mut ret = Self::from_scope(self.scope, self.define_context.clone());
        ret.generic_tables = self.generic_tables.clone();
        ret.visiting = self.visiting.clone();
        ret.depth = self.depth + 1;
        ret
    }

    /// Moves the cursor to `scope` under `define_context`. These jumps (type
    /// tracing, name-as-namespace) do not preserve a generic-instance context, so
    /// the shadow cursor follows `scope` directly.
    fn set_namespace(&mut self, scope: scope::ScopeId, define_context: DefineContext) {
        self.scope = scope;
        self.inst_scope = scope;
        self.define_context = define_context;
    }

    /// Moves the cursor into a symbol's inner scope (where its members live).
    fn set_inner(&mut self, symbol: &Symbol) {
        let base_inner = scope::inner_scope(symbol.scope, symbol.token.text);
        // Shadow cursor: rebase onto the instance-side cursor only when the symbol
        // is a direct member of the delegated base scope (it really lives in the
        // instance's template). Symbols reached from elsewhere (e.g. imports) keep
        // their own home; only the base segment is renamed.
        self.inst_scope = if scope::generic_delegation(self.inst_scope) == Some(symbol.scope) {
            scope::register_generic_child(self.inst_scope, symbol.token.text, base_inner)
        } else {
            base_inner
        };
        self.scope = base_inner;
        self.define_context = symbol.namespace.define_context.clone();
    }

    /// Reconstructs the cursor namespace from the scope tree on demand. Used by
    /// the few consumers that still need a `Namespace` value (import expansion,
    /// synthesized SystemVerilog members, trace logging).
    fn namespace(&self) -> Namespace {
        scope::namespace(self.scope, &self.define_context)
    }

    fn indent(&self) -> String {
        "  ".repeat(self.depth)
    }
}

const DEFINED_NAMESPACES: [&str; 3] = ["$sv", "$std", "$tb"];

// Refer IEEE Std 1800-2023 Table B.1 - Reserved keywords
// This list must be sorted to enable binary search
const SYSTEMVERILOG_KEYWORDS: [&str; 248] = [
    "accept_on",
    "alias",
    "always",
    "always_comb",
    "always_ff",
    "always_latch",
    "and",
    "assert",
    "assign",
    "assume",
    "automatic",
    "before",
    "begin",
    "bind",
    "bins",
    "binsof",
    "bit",
    "break",
    "buf",
    "bufif0",
    "bufif1",
    "byte",
    "case",
    "casex",
    "casez",
    "cell",
    "chandle",
    "checker",
    "class",
    "clocking",
    "cmos",
    "config",
    "const",
    "constraint",
    "context",
    "continue",
    "cover",
    "covergroup",
    "coverpoint",
    "cross",
    "deassign",
    "default",
    "defparam",
    "design",
    "disable",
    "dist",
    "do",
    "edge",
    "else",
    "end",
    "endcase",
    "endchecker",
    "endclass",
    "endclocking",
    "endconfig",
    "endfunction",
    "endgenerate",
    "endgroup",
    "endinterface",
    "endmodule",
    "endpackage",
    "endprimitive",
    "endprogram",
    "endproperty",
    "endspecify",
    "endsequence",
    "endtable",
    "endtask",
    "enum",
    "event",
    "eventually",
    "expect",
    "export",
    "extends",
    "extern",
    "final",
    "first_match",
    "for",
    "force",
    "foreach",
    "forever",
    "fork",
    "forkjoin",
    "function",
    "generate",
    "genvar",
    "global",
    "highz0",
    "highz1",
    "if",
    "iff",
    "ifnone",
    "ignore_bins",
    "illegal_bins",
    "implements",
    "implies",
    "import",
    "incdir",
    "include",
    "initial",
    "inout",
    "input",
    "inside",
    "instance",
    "int",
    "integer",
    "interconnect",
    "interface",
    "intersect",
    "join",
    "join_any",
    "join_none",
    "large",
    "let",
    "liblist",
    "library",
    "local",
    "localparam",
    "logic",
    "longint",
    "macromodule",
    "matches",
    "medium",
    "modport",
    "module",
    "nand",
    "negedge",
    "nettype",
    "new",
    "nexttime",
    "nmos",
    "nor",
    "noshowcancelled",
    "not",
    "notif0",
    "notif1",
    "null",
    "or",
    "output",
    "package",
    "packed",
    "parameter",
    "pmos",
    "posedge",
    "primitive",
    "priority",
    "program",
    "property",
    "protected",
    "pull0",
    "pull1",
    "pulldown",
    "pullup",
    "pulsestyle_ondetect",
    "pulsestyle_onevent",
    "pure",
    "rand",
    "randc",
    "randcase",
    "randsequence",
    "rcmos",
    "real",
    "realtime",
    "ref",
    "reg",
    "reject_on",
    "release",
    "repeat",
    "restrict",
    "return",
    "rnmos",
    "rpmos",
    "rtran",
    "rtranif0",
    "rtranif1",
    "s_always",
    "s_eventually",
    "s_nexttime",
    "s_until",
    "s_until_with",
    "scalared",
    "sequence",
    "shortint",
    "shortreal",
    "showcancelled",
    "signed",
    "small",
    "soft",
    "solve",
    "specify",
    "specparam",
    "static",
    "string",
    "strong",
    "strong0",
    "strong1",
    "struct",
    "super",
    "supply0",
    "supply1",
    "sync_accept_on",
    "sync_reject_on",
    "table",
    "tagged",
    "task",
    "this",
    "throughout",
    "time",
    "timeprecision",
    "timeunit",
    "tran",
    "tranif0",
    "tranif1",
    "tri",
    "tri0",
    "tri1",
    "triand",
    "trior",
    "trireg",
    "type",
    "typedef",
    "union",
    "unique",
    "unique0",
    "unsigned",
    "until",
    "until_with",
    "untyped",
    "use",
    "uwire",
    "var",
    "vectored",
    "virtual",
    "void",
    "wait",
    "wait_order",
    "wand",
    "weak",
    "weak0",
    "weak1",
    "while",
    "wildcard",
    "wire",
    "with",
    "within",
    "wor",
    "xnor",
    "xor",
];

pub fn is_sv_keyword(s: &str) -> bool {
    SYSTEMVERILOG_KEYWORDS.binary_search(&s).is_ok()
}

thread_local!(static SYMBOL_TABLE: RefCell<SymbolTable> = RefCell::new(SymbolTable::new()));
// Resolve caches are keyed by the resolution start (scope + ifdef context)
// rather than a materialized namespace, so a reference's scope is enough to hit
// the cache without reconstructing its namespace path.
type ResolveCacheKey = (SymbolPath, scope::ScopeId, DefineContext);
thread_local!(static SYMBOL_CACHE: RefCell<HashMap<ResolveCacheKey, Rc<ResolveResult>>> = RefCell::new(HashMap::default()));

// Cache for get_namespace_generic_map; its result only depends on the symbol
// table state, so it is invalidated together with SYMBOL_CACHE.
type NamespaceGenericMap = Option<Rc<Vec<GenericMap>>>;
thread_local!(static NS_GENERIC_MAP_CACHE: RefCell<HashMap<(scope::ScopeId, DefineContext), NamespaceGenericMap>> = RefCell::new(HashMap::default()));

// Cache for resolve failures. A new symbol can turn a cached NotFound into a
// successful resolution, so `insert` invalidates this cache even while cache
// clear is suppressed.
thread_local!(static SYMBOL_ERR_CACHE: RefCell<HashMap<ResolveCacheKey, ResolveError>> = RefCell::new(HashMap::default()));

// Structural identity index for generic instances. An instance is keyed by its
// enclosing scope, its base template symbol, and its argument signature instead
// of the mangled string `__Foo__8`. The enclosing scope distinguishes nested
// instances of the same `base::<args>` living under different parent instances
// (e.g. `Func::<1>` in `PkgB::<PkgA::<1>>` vs `PkgB::<PkgA::<2>>`), which the
// mangled identity encodes via the namespace. Built during `insert_generic_instance`
// (instances are never restored from fragments — they are post-pass1, outside the
// pass1 capture window — so the index is complete on cold and warm builds) and
// queried by `resolve_generic_structural` to reach an instance without resolving
// its mangled name.
type GenericInstanceKey = (scope::ScopeId, SymbolId, Vec<SymbolPath>);
thread_local!(static GENERIC_INSTANCE_INDEX: RefCell<HashMap<GenericInstanceKey, SymbolId>> = RefCell::new(HashMap::default()));

fn generic_instance_key(
    enclosing: scope::ScopeId,
    base: SymbolId,
    arguments: &[GenericSymbolPath],
) -> GenericInstanceKey {
    (
        enclosing,
        base,
        arguments.iter().map(|a| a.mangled_path()).collect(),
    )
}

/// Registers a generic instance under its structural identity key, asserting the
/// key is injective: no two distinct instances may share
/// `(enclosing_scope, base, args)`.
pub fn index_generic_instance(
    enclosing: scope::ScopeId,
    base: SymbolId,
    arguments: &[GenericSymbolPath],
    instance: SymbolId,
) {
    let key = generic_instance_key(enclosing, base, arguments);
    GENERIC_INSTANCE_INDEX.with(|f| {
        let mut index = f.borrow_mut();
        if let Some(prev) = index.get(&key) {
            debug_assert_eq!(
                *prev, instance,
                "structural generic-instance key collides with a distinct instance"
            );
        }
        index.insert(key, instance);
    });
}

fn clear_resolve_caches() {
    SYMBOL_CACHE.with(|f| f.borrow_mut().clear());
    SYMBOL_ERR_CACHE.with(|f| f.borrow_mut().clear());
    NS_GENERIC_MAP_CACHE.with(|f| f.borrow_mut().clear());
}

/// Suppress resolve-cache invalidation for bulk mutations that modify symbol
/// metadata without affecting name resolution results. Must be paired with
/// `resume_cache_clear`. `insert` still invalidates the caches.
pub fn suppress_cache_clear() {
    SYMBOL_TABLE.with(|f| f.borrow_mut().suppress_cache_clear = true);
}

/// Resume resolve-cache invalidation after `suppress_cache_clear`.
pub fn resume_cache_clear() {
    SYMBOL_TABLE.with(|f| f.borrow_mut().suppress_cache_clear = false);
}

fn clear_cache() {
    let suppress = SYMBOL_TABLE.with(|f| f.borrow().suppress_cache_clear);
    if !suppress {
        clear_resolve_caches();
    }
}

pub fn insert(token: &Token, symbol: Symbol) -> Option<SymbolId> {
    let ret = SYMBOL_TABLE.with(|f| f.borrow_mut().insert(token, symbol));
    if ret.is_some() {
        clear_resolve_caches();
    }
    ret
}

pub fn get(id: SymbolId) -> Option<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get(id))
}

pub fn get_rc(id: SymbolId) -> Option<Rc<Symbol>> {
    SYMBOL_TABLE.with(|f| f.borrow().get_rc(id))
}

pub fn update(symbol: Symbol) {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().update(symbol))
}

pub fn resolve<T: Into<SymbolPathNamespace>>(path: T) -> Result<Rc<ResolveResult>, ResolveError> {
    let path: SymbolPathNamespace = path.into();
    let scope = path.2.unwrap_or_else(|| scope::intern_namespace(&path.1));
    let define_context = path.1.define_context.clone();
    let key: ResolveCacheKey = (path.0, scope, define_context.clone());

    if let Some(x) = SYMBOL_CACHE.with(|f| f.borrow().get(&key).cloned()) {
        Ok(x)
    } else if let Some(e) = SYMBOL_ERR_CACHE.with(|f| f.borrow().get(&key).cloned()) {
        Err(e)
    } else {
        match SYMBOL_TABLE.with(|f| {
            f.borrow().resolve(
                &key.0,
                &[],
                ResolveContext::from_scope(scope, define_context),
            )
        }) {
            Ok(x) => {
                let x = Rc::new(x);
                SYMBOL_CACHE.with(|f| f.borrow_mut().insert(key, Rc::clone(&x)));
                Ok(x)
            }
            Err(e) => {
                SYMBOL_ERR_CACHE.with(|f| f.borrow_mut().insert(key, e.clone()));
                Err(e)
            }
        }
    }
}

/// How a structural generic resolution is rooted: at an explicit scope, or at a
/// token or namespace it is derived from. Mirrors the general [`resolve`]
/// accepting an `Into<SymbolPathNamespace>`.
pub enum StructuralRoot {
    Scope(scope::ScopeId, DefineContext),
    Token(resource_table::TokenId),
}

impl From<(scope::ScopeId, DefineContext)> for StructuralRoot {
    fn from((scope, define_context): (scope::ScopeId, DefineContext)) -> Self {
        StructuralRoot::Scope(scope, define_context)
    }
}

impl From<resource_table::TokenId> for StructuralRoot {
    fn from(token: resource_table::TokenId) -> Self {
        StructuralRoot::Token(token)
    }
}

impl From<&Namespace> for StructuralRoot {
    fn from(namespace: &Namespace) -> Self {
        StructuralRoot::Scope(
            scope::intern_namespace(namespace),
            namespace.define_context.clone(),
        )
    }
}

impl StructuralRoot {
    /// The scope (and ifdef context) the structural walk starts from, if known.
    fn structural_scope(&self) -> Option<(scope::ScopeId, DefineContext)> {
        match self {
            StructuralRoot::Scope(scope, define_context) => Some((*scope, define_context.clone())),
            StructuralRoot::Token(token) => scope::token_scope(*token),
        }
    }

    /// The mangled-name path the structural walk falls back to when it declines.
    fn fallback_path(&self, path: SymbolPath) -> SymbolPathNamespace {
        match self {
            StructuralRoot::Scope(scope, define_context) => {
                SymbolPathNamespace::from_scope(path, *scope, define_context.clone())
            }
            StructuralRoot::Token(token) => SymbolPathNamespace::from_token(path, *token),
        }
    }
}

/// Resolve a generic instance path structurally — navigating generic segments
/// through the scope-tree index instead of a mangled name — rooted at a scope,
/// token, or namespace. Falls back to the mangled resolution for the paths the
/// structural walk does not model (synthesized SystemVerilog members). General
/// resolve sites pass arbitrary (possibly non-generic or already-mangled) paths;
/// a degenerate path keeps the mangled behaviour (e.g. double-mangle → not found)
/// these sites' diagnostics rely on.
pub fn resolve_generic_structural(
    path: &GenericSymbolPath,
    root: impl Into<StructuralRoot>,
) -> Result<Rc<ResolveResult>, ResolveError> {
    let root = root.into();
    let structural = root.structural_scope().and_then(|(scope, define_context)| {
        SYMBOL_TABLE.with(|f| {
            f.borrow()
                .resolve_generic_structural(path, scope, define_context)
        })
    });
    match structural {
        Some(structural) => Ok(Rc::new(structural)),
        None => resolve(root.fallback_path(path.mangled_path())),
    }
}

/// Resolve the symbol at depth `i` of `path` — segment `i` taken as a template
/// (its arguments ignored), reached by navigating the prefix `[0..i)` through the
/// instance index, never constructing a mangled prefix name. The structural
/// replacement for the `resolve(path.base_path(i), ..)` idiom: `base_path(i)`
/// mangled the prefix and dropped segment `i`'s arguments, which this reproduces
/// by slicing to `[0..=i]` and clearing the last segment's arguments.
pub fn resolve_base_path(
    path: &GenericSymbolPath,
    i: usize,
    root: impl Into<StructuralRoot>,
) -> Result<Rc<ResolveResult>, ResolveError> {
    let mut sliced = path.slice(i);
    if let Some(last) = sliced.paths.last_mut() {
        last.arguments.clear();
    }
    resolve_generic_structural(&sliced, root)
}

/// Resolve the symbol that `namespace` points to (its deepest segment resolved
/// in the enclosing namespace).
pub fn get_namespace_symbol(namespace: &Namespace) -> Option<Symbol> {
    let mut namespace = namespace.clone();
    namespace.strip_anonymous_path();

    if let Some(path) = namespace.pop()
        && namespace.depth() >= 1
    {
        resolve((path, &namespace)).map(|x| (*x.found).clone()).ok()
    } else {
        None
    }
}

pub fn get_all() -> Vec<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get_all())
}

pub fn dump() -> String {
    SYMBOL_TABLE.with(|f| f.borrow().dump())
}

pub fn drop(file_path: PathId) {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().drop(file_path))
}

pub fn add_reference(target: SymbolId, token: &Token) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_reference(target, token))
}

pub fn add_generic_instance(target: SymbolId, instance: SymbolId) {
    clear_cache();
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_generic_instance(target, instance))
}

pub fn update_generic_instance_affiliation(target: SymbolId, instance: &Symbol) {
    clear_cache();
    SYMBOL_TABLE.with(|f| {
        f.borrow_mut()
            .update_generic_instance_affiliation(target, instance)
    })
}

pub fn add_import(import: Import) {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_import(import))
}

pub fn apply_import() {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().apply_import());
}

pub fn resolve_interfaces() -> Vec<AnalyzerError> {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().resolve_interfaces())
}

pub fn add_bind(bind: Bind) {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_bind(bind))
}

pub fn apply_bind() -> Vec<AnalyzerError> {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().apply_bind())
}

pub fn add_msb(msb: Msb) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_msb(msb))
}

pub fn apply_msb() -> Vec<AnalyzerError> {
    let msb_list = SYMBOL_TABLE.with(|f| f.borrow_mut().get_msb());
    check_msb(msb_list)
}

pub fn add_connect(connect: Connect) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_connect(connect))
}

pub fn apply_connect() -> Vec<AnalyzerError> {
    let connect_list = SYMBOL_TABLE.with(|f| f.borrow_mut().get_connect());
    check_connect(connect_list)
}

pub fn resolve_user_defined() {
    clear_resolve_caches();
    let resolved = SYMBOL_TABLE.with(|f| f.borrow().resolve_user_defined());
    SYMBOL_TABLE.with(|f| f.borrow_mut().set_user_defined(resolved));
}

pub fn resolve_function() {
    let list = SYMBOL_TABLE.with(|f| f.borrow().get_function_symbols());
    function::resolve_function(&list);
}

pub fn resolve_enum() -> Vec<AnalyzerError> {
    let list = SYMBOL_TABLE.with(|f| f.borrow().get_enum_symbols());
    r#enum::resolve_enum(&list)
}

pub fn add_project_local(prj: StrId, from: StrId, to: StrId) {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_project_local(prj, from, to))
}

pub fn add_reference_functions(id: SymbolId, functions: Vec<GenericSymbolPath>) {
    clear_resolve_caches();
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_reference_functions(id, functions))
}

pub fn get_reference_functions(id: SymbolId) -> Option<Vec<GenericSymbolPath>> {
    SYMBOL_TABLE.with(|f| f.borrow().get_reference_functions(id))
}

pub fn get_references(id: SymbolId) -> Option<Vec<Token>> {
    SYMBOL_TABLE.with(|f| f.borrow().get_references(id))
}

/// See [`SymbolTable::pending_watermark`].
pub fn pending_watermark() -> PendingWatermark {
    SYMBOL_TABLE.with(|f| f.borrow().pending_watermark())
}

/// See [`SymbolTable::export_fragment`].
pub fn export_fragment(
    symbol_window_start: usize,
    symbol_window_end: usize,
    watermark: &PendingWatermark,
) -> SymbolTableFragment {
    SYMBOL_TABLE.with(|f| {
        f.borrow()
            .export_fragment(symbol_window_start, symbol_window_end, watermark)
    })
}

/// See [`SymbolTable::restore_fragment`].
pub fn restore_fragment(fragment: SymbolTableFragment) -> Result<(), Box<Symbol>> {
    let ret = SYMBOL_TABLE.with(|f| f.borrow_mut().restore_fragment(fragment));
    clear_resolve_caches();
    ret
}

pub fn clear() {
    clear_resolve_caches();
    GENERIC_INSTANCE_INDEX.with(|f| f.borrow_mut().clear());
    SYMBOL_TABLE.with(|f| f.borrow_mut().clear())
}

pub fn check_unused_variable() -> Vec<AnalyzerError> {
    SYMBOL_TABLE.with(|f| f.borrow().check_unused_variable())
}

pub fn get_tests(project_name: &str) -> Vec<(StrId, TestProperty)> {
    SYMBOL_TABLE.with(|f| f.borrow().get_tests(project_name))
}

pub fn get_doc_tests(project_name: &str) -> Vec<DocTestTarget> {
    SYMBOL_TABLE.with(|f| f.borrow().get_doc_tests(project_name))
}

pub fn check_wavedrom() -> Vec<AnalyzerError> {
    SYMBOL_TABLE.with(|f| f.borrow().check_wavedrom())
}

#[cfg(test)]
mod tests {
    use crate::namespace::Namespace;
    use crate::symbol_table::{ResolveError, ResolveResult, SymbolPath};
    use crate::{Analyzer, symbol_table};
    use veryl_metadata::Metadata;
    use veryl_parser::{Parser, resource_table};

    const CODE: &str = r##"
    module ModuleA #(
        param paramA: u32 = 1,
        param paramB: PackageA::StructA = 1,
    ) (
        portA: input logic<10>,
        portB: modport InterfaceA::modportA,
    ) {
        const localA: u32 = 1;
        const localB: PackageA::StructA = 1;

        type TypeA = PackageA::StructA;

        var memberA: logic;
        var memberB: PackageA::StructA;
        var memberC: TypeA;
        var memberD: $sv::SvTypeA;
        var memberE: PackageA::UnionA;

        inst instA: InterfaceA;
    }

    interface InterfaceA #(
        param paramA: u32 = 1,
        param paramB: PackageA::StructA = 1,
    ) {
        const localA: u32 = 1;
        const localB: PackageA::StructA = 1;

        type TypeA = PackageA::StructA;

        var memberA: logic;
        var memberB: PackageA::StructA;
        var memberC: TypeA;

        modport modportA {
            memberA: input,
            memberB: output,
            memberC: output,
        }
    }

    package PackageA {
        const localA: u32 = 1;

        struct StructA {
            memberA: logic,
            memberB: StructB,
        }

        struct StructB {
            memberA: logic,
        }

        enum EnumA: logic<2> {
            memberA,
        }

        union UnionA {
            memberA: logic<2>,
            memberB: EnumA,
        }
    }
    "##;

    fn parse() {
        let metadata = Metadata::create_default("prj").unwrap();
        let parser = Parser::parse(CODE, &"").unwrap();
        let analyzer = Analyzer::new(&metadata);
        analyzer.analyze_pass1("prj", &parser.veryl);
    }

    #[track_caller]
    fn check_found(result: Result<std::rc::Rc<ResolveResult>, ResolveError>, expect: &str) {
        assert_eq!(format!("{}", result.unwrap().found.namespace), expect);
    }

    #[track_caller]
    fn check_not_found(result: Result<std::rc::Rc<ResolveResult>, ResolveError>) {
        assert!(result.is_err());
    }

    fn create_path(paths: &[&str]) -> SymbolPath {
        let mut ret = SymbolPath::default();

        for path in paths {
            ret.push(resource_table::insert_str(path));
        }

        ret
    }

    fn create_namespace(paths: &[&str]) -> Namespace {
        // The fixture is analyzed under project "prj", so anchor queries at that
        // project root, matching the namespace pass1 builds for its symbols.
        let mut ret = Namespace::new();
        ret.push(resource_table::insert_str("prj"));

        for path in paths {
            ret.push(resource_table::insert_str(path));
        }

        ret
    }

    fn resolve(
        paths: &[&str],
        namespace: &[&str],
    ) -> Result<std::rc::Rc<ResolveResult>, ResolveError> {
        let path = create_path(paths);
        let namespace = create_namespace(namespace);
        symbol_table::resolve((&path, &namespace))
    }

    #[test]
    fn module() {
        parse();

        let symbol = resolve(&["ModuleA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn interface() {
        parse();

        let symbol = resolve(&["InterfaceA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn package() {
        parse();

        let symbol = resolve(&["PackageA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn param() {
        parse();

        let symbol = resolve(&["paramA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["paramA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["paramA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["paramA"], &["PackageA"]);
        check_not_found(symbol);

        let symbol = resolve(&["paramB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["paramB", "memberB"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["paramB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["paramB", "memberB", "memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn local() {
        parse();

        let symbol = resolve(&["localA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["localA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["localA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["localA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");

        let symbol = resolve(&["localB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["localB", "memberB"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["localB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["localB", "memberB", "memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn port() {
        parse();

        let symbol = resolve(&["portA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["portA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["portA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["portA"], &["PackageA"]);
        check_not_found(symbol);
    }

    #[test]
    fn variable() {
        parse();

        let symbol = resolve(&["memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["memberA"], &["PackageA"]);
        check_not_found(symbol);
    }

    #[test]
    fn r#struct() {
        parse();

        let symbol = resolve(&["StructA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn struct_member() {
        parse();

        let symbol = resolve(&["memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberA"], &["PackageA", "StructA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberB", "memberX"], &["ModuleA"]);
        check_not_found(symbol);
    }

    #[test]
    fn r#enum() {
        parse();

        let symbol = resolve(&["EnumA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn enum_member() {
        parse();

        let symbol = resolve(&["EnumA", "memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA::EnumA");
    }

    #[test]
    fn union() {
        parse();

        let symbol = resolve(&["UnionA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn union_member() {
        parse();

        let symbol = resolve(&["memberE"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberE"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberE", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::UnionA");

        let symbol = resolve(&["memberE", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::UnionA");
    }

    #[test]
    fn modport() {
        parse();

        let symbol = resolve(&["portB"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["portB"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["portB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["portB", "memberC"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberC", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberC", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberC", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn typedef() {
        parse();

        let symbol = resolve(&["memberC"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberC"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberC", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberC", "memberX"], &["ModuleA"]);
        check_not_found(symbol);
    }

    #[test]
    fn sv_member() {
        parse();

        let symbol = resolve(&["memberD"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberD"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberD", "memberA"], &["ModuleA"]);
        check_found(symbol, "$sv::SvTypeA");

        let symbol = resolve(&["memberD", "memberA", "memberA", "memberA"], &["ModuleA"]);
        check_found(symbol, "$sv::SvTypeA");
    }

    #[test]
    fn inst() {
        parse();

        let symbol = resolve(&["instA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["instA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["instA", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["instA", "memberB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    const IMPORT_PRECEDENCE_CODE: &str = r##"
    package PkgA {
        enum EnumA: u32 {
            val = 100,
        }
    }
    package PkgB {
        const val: u32 = 200;
    }
    module ModLocal {
        import PkgA::EnumA::*;
        const val: u32 = 7;
    }
    module ModExplicit {
        import PkgA::EnumA::*;
        import PkgB::val;
    }
    module ModAmbiguous {
        import PkgA::EnumA::*;
        import PkgB::*;
    }
    "##;

    /// SystemVerilog import precedence: a local declaration or an explicit
    /// import shadows a wildcard import in the same scope without error, while
    /// two wildcard imports of the same name are ambiguous.
    #[test]
    fn import_precedence() {
        let handle = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let metadata = Metadata::create_default("prj").unwrap();
                let parser = Parser::parse(IMPORT_PRECEDENCE_CODE, &"").unwrap();
                let analyzer = Analyzer::new(&metadata);
                analyzer.analyze_pass1("prj", &parser.veryl);
                Analyzer::analyze_post_pass1();

                // Local declaration wins over the wildcard-imported enum member.
                check_found(resolve(&["val"], &["ModLocal"]), "prj::ModLocal");
                // Explicit import wins over the wildcard import.
                check_found(resolve(&["val"], &["ModExplicit"]), "prj::PkgB");
                // Two wildcard imports of the same name are ambiguous.
                check_not_found(resolve(&["val"], &["ModAmbiguous"]));
            })
            .unwrap();
        handle.join().unwrap();
    }
}
