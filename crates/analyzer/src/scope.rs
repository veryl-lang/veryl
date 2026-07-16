use crate::HashMap;
use crate::SVec;
use crate::namespace::{DefineContext, Namespace};
use crate::symbol::{SymbolId, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use std::cell::RefCell;
use std::rc::Rc;
use veryl_parser::resource_table::{self, PathId, StrId, TokenId};

/// Runtime-only handle into the [`ScopeArena`]. `ScopeId(0)` is always the root.
///
/// This is intentionally not serializable: fragment persistence keeps the
/// name-based `Namespace`, and a scope is recovered by re-interning its
/// structural identity rather than by replaying a raw id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ScopeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Root,
    Project,
    Package,
    Module,
    Interface,
    Function,
    Modport,
    Enum,
    StructUnion,
    Block,
    SystemVerilog,
    /// A generic instance (`Foo::<8>`). Its member scope holds no members of its
    /// own; lexical lookups delegate to the base template's member scope (see
    /// [`ScopeArena::generic_delegations`]).
    GenericInstance,
    Unknown,
}

/// The kind of inner scope a symbol owns, or `None` for symbols that do not
/// open a scope. Both pass1 symbol insertion and fragment restore use this to
/// stamp the owned scope's kind/owner, so they must agree.
pub fn scope_kind_of(kind: &SymbolKind) -> Option<ScopeKind> {
    match kind {
        SymbolKind::Module(_) => Some(ScopeKind::Module),
        SymbolKind::Interface(_) => Some(ScopeKind::Interface),
        SymbolKind::Package(_) => Some(ScopeKind::Package),
        SymbolKind::Function(_) => Some(ScopeKind::Function),
        SymbolKind::Struct(_) | SymbolKind::Union(_) => Some(ScopeKind::StructUnion),
        SymbolKind::Enum(_) => Some(ScopeKind::Enum),
        SymbolKind::Modport(_) => Some(ScopeKind::Modport),
        SymbolKind::Block => Some(ScopeKind::Block),
        _ => None,
    }
}

/// An explicit `import pkg::name` binding local to a scope.
#[derive(Debug, Clone)]
pub struct ImportBinding {
    pub symbol: SymbolId,
    /// ifdef context of the `import` statement for ifdef-aware visibility.
    pub define_context: DefineContext,
    /// As-written package qualifier of the import statement, used to expand an
    /// imported reference to its fully qualified form (`resolve_imported`).
    /// `Rc` so the lexical-lookup hot path can clone import/wildcard entries
    /// without deep-copying this path it never reads.
    pub package_path: Rc<GenericSymbolPath>,
}

/// A `import pkg::*` (or enum-member wildcard) brought into a scope. The
/// wildcard exposes every direct member of `source` under its own name.
#[derive(Debug, Clone)]
pub struct WildcardImport {
    pub source: ScopeId,
    /// ifdef context of the `import` statement; gates the wildcard against the
    /// querying scope.
    pub define_context: DefineContext,
    /// ifdef context of the source container; gates which members the wildcard
    /// exposes against the referencing symbol's own ifdef context.
    pub source_define_context: DefineContext,
    /// As-written package qualifier of the import statement, used to expand an
    /// imported reference to its fully qualified form (`resolve_imported`).
    /// `Rc` so the lexical-lookup hot path can clone import/wildcard entries
    /// without deep-copying this path it never reads.
    pub package_path: Rc<GenericSymbolPath>,
}

#[derive(Debug, Clone)]
pub struct Mixin {
    pub source: ScopeId,
    pub define_context: DefineContext,
    pub source_define_context: DefineContext,
    pub source_path: Rc<GenericSymbolPath>,
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    pub owner: Option<SymbolId>,
    pub name: Option<StrId>,
    pub locals: HashMap<StrId, SVec<SymbolId>>,
    /// Explicit `import pkg::name` bindings local to this scope.
    pub imports: HashMap<StrId, SVec<ImportBinding>>,
    /// Wildcard imports local to this scope.
    pub wildcards: SVec<WildcardImport>,
    pub mixins: SVec<Mixin>,
}

/// Explicit-parent scope tree built during pass1; `current` is the sole pass1
/// cursor since the parallel `Namespace` cursor was retired.
///
/// Scopes are interned by structural identity `(parent, name)` so the same
/// logical scope (e.g. a shared project root) maps to one `ScopeId` across
/// files. `current` advances as pass1 walks the tree; a namespace value is
/// reconstructed from it on demand.
pub struct ScopeArena {
    scopes: Vec<Scope>,
    intern: HashMap<(u32, StrId), u32>,
    current: ScopeId,
    /// Scope at which each token appears, the authoritative starting point for
    /// reference resolution. Replaces the per-token `Namespace` snapshot the
    /// analyzer used to keep: the structural path is recovered from the
    /// interned `ScopeId`, the token's own ifdef context is kept alongside, and
    /// `PathId` drives incremental drop/export.
    token_scopes: HashMap<TokenId, (ScopeId, DefineContext, PathId)>,
    /// The root (non-dependency) project, identified by name. Projects are the
    /// root scope's children of kind [`ScopeKind::Project`].
    root_project: Option<StrId>,
    /// Member-scope delegation for generic instances: the inner scope of an
    /// instance (`__Foo__8`) maps to the inner scope of its base template
    /// (`Foo`), where the actual members live. The authority resolution uses:
    /// delegation-aware lexical lookup descends into the base scope and the
    /// structural generic walk navigates members through it.
    generic_delegations: HashMap<ScopeId, ScopeId>,
}

impl ScopeArena {
    fn new() -> Self {
        let root = Scope {
            id: ScopeId(0),
            parent: None,
            kind: ScopeKind::Root,
            owner: None,
            name: None,
            locals: HashMap::default(),
            imports: HashMap::default(),
            wildcards: SVec::new(),
            mixins: SVec::new(),
        };
        Self {
            scopes: vec![root],
            intern: HashMap::default(),
            current: ScopeId(0),
            token_scopes: HashMap::default(),
            root_project: None,
            generic_delegations: HashMap::default(),
        }
    }

    /// Registers an instance's member scope as delegating to its base template's
    /// member scope, and marks the instance scope kind. Both inner scopes are
    /// children of `base_enclosing` (the namespace the template and its
    /// instances share). Returns the instance's inner scope.
    fn register_generic_instance(
        &mut self,
        base_enclosing: ScopeId,
        base_name: StrId,
        mangled_name: StrId,
    ) -> ScopeId {
        let inst_inner =
            self.intern_child(base_enclosing, mangled_name, ScopeKind::GenericInstance);
        let base_inner = self.intern_child(base_enclosing, base_name, ScopeKind::Unknown);
        self.generic_delegations.insert(inst_inner, base_inner);
        inst_inner
    }

    fn generic_delegation(&self, scope: ScopeId) -> Option<ScopeId> {
        self.generic_delegations.get(&scope).copied()
    }

    fn get_mixin_target_scope(&self, query_scope: ScopeId, source: ScopeId) -> Option<ScopeId> {
        fn is_mixin_source(scope: &Scope, source: ScopeId) -> bool {
            scope.mixins.iter().any(|m| m.source == source)
        }

        let mut current = Some(query_scope);
        while let Some(id) = current
            && let Some(scope) = self.scopes.get(id.0 as usize)
        {
            if is_mixin_source(scope, source) {
                return current;
            }
            current = scope.parent;
        }

        None
    }

    fn intern_child(&mut self, parent: ScopeId, name: StrId, kind: ScopeKind) -> ScopeId {
        let name = resource_table::canonical_str_id(name);
        if let Some(&id) = self.intern.get(&(parent.0, name)) {
            let id = ScopeId(id);
            // Upgrade a placeholder kind once the owning declaration is known.
            if kind != ScopeKind::Unknown {
                self.scopes[id.0 as usize].kind = kind;
            }
            id
        } else {
            let id = ScopeId(self.scopes.len() as u32);
            self.scopes.push(Scope {
                id,
                parent: Some(parent),
                kind,
                owner: None,
                name: Some(name),
                locals: HashMap::default(),
                imports: HashMap::default(),
                wildcards: SVec::new(),
                mixins: SVec::new(),
            });
            self.intern.insert((parent.0, name), id.0);
            id
        }
    }

    fn intern_namespace(&mut self, namespace: &Namespace) -> ScopeId {
        let mut current = ScopeId(0);
        for name in &namespace.paths {
            current = self.intern_child(current, *name, ScopeKind::Unknown);
        }
        current
    }

    fn enter(&mut self, name: StrId) -> ScopeId {
        let id = self.intern_child(self.current, name, ScopeKind::Unknown);
        self.current = id;
        id
    }

    fn exit(&mut self) -> ScopeId {
        let exited = self.current;
        if let Some(parent) = self.scopes[exited.0 as usize].parent {
            self.current = parent;
        }
        exited
    }

    fn add_local(&mut self, scope: ScopeId, name: StrId, symbol: SymbolId) {
        let name = resource_table::canonical_str_id(name);
        self.scopes[scope.0 as usize]
            .locals
            .entry(name)
            .or_default()
            .push(symbol);
    }

    fn add_import(
        &mut self,
        scope: ScopeId,
        name: StrId,
        symbol: SymbolId,
        define_context: DefineContext,
        package_path: GenericSymbolPath,
    ) {
        let name = resource_table::canonical_str_id(name);
        self.scopes[scope.0 as usize]
            .imports
            .entry(name)
            .or_default()
            .push(ImportBinding {
                symbol,
                define_context,
                package_path: Rc::new(package_path),
            });
    }

    fn add_wildcard(
        &mut self,
        scope: ScopeId,
        source: ScopeId,
        define_context: DefineContext,
        source_define_context: DefineContext,
        package_path: GenericSymbolPath,
    ) {
        self.scopes[scope.0 as usize]
            .wildcards
            .push(WildcardImport {
                source,
                define_context,
                source_define_context,
                package_path: Rc::new(package_path),
            });
    }

    fn add_mixin_source(
        &mut self,
        scope: ScopeId,
        source: ScopeId,
        define_context: DefineContext,
        source_define_context: DefineContext,
        source_path: GenericSymbolPath,
    ) {
        self.scopes[scope.0 as usize].mixins.push(Mixin {
            source,
            define_context,
            source_define_context,
            source_path: Rc::new(source_path),
        });
    }

    fn set_kind_owner(&mut self, scope: ScopeId, kind: ScopeKind, owner: SymbolId) {
        let scope = &mut self.scopes[scope.0 as usize];
        if kind != ScopeKind::Unknown {
            scope.kind = kind;
        }
        scope.owner = Some(owner);
    }

    fn depth(&self, scope: ScopeId) -> usize {
        let mut n = 0;
        let mut current = Some(scope);
        while let Some(id) = current {
            let s = &self.scopes[id.0 as usize];
            if s.name.is_some() {
                n += 1;
            }
            current = s.parent;
        }
        n
    }

    fn project_of(&self, scope: ScopeId) -> Option<StrId> {
        let mut current = scope;
        loop {
            let s = self.scopes.get(current.0 as usize)?;
            match s.parent {
                // A root-level child is a project scope; its name is the project.
                Some(p) if p.0 == 0 => return s.name,
                Some(p) => current = p,
                None => return None,
            }
        }
    }

    fn name_path(&self, scope: ScopeId) -> Vec<StrId> {
        let mut ret = Vec::new();
        let mut current = Some(scope);
        while let Some(id) = current {
            let scope = &self.scopes[id.0 as usize];
            if let Some(name) = scope.name {
                ret.push(name);
            }
            current = scope.parent;
        }
        ret.reverse();
        ret
    }

    fn namespace(&self, scope: ScopeId, define_context: &DefineContext) -> Namespace {
        Namespace {
            paths: self.name_path(scope).into(),
            define_context: define_context.clone(),
        }
    }

    fn insert_token(&mut self, token: TokenId, file_path: PathId, namespace: &Namespace) {
        let scope = self.intern_namespace(namespace);
        self.insert_token_scope(token, file_path, scope, &namespace.define_context);
    }

    fn insert_token_scope(
        &mut self,
        token: TokenId,
        file_path: PathId,
        scope: ScopeId,
        define_context: &DefineContext,
    ) {
        self.token_scopes
            .insert(token, (scope, define_context.clone(), file_path));
    }

    fn drop_tokens(&mut self, file_path: PathId) {
        self.token_scopes.retain(|_, (_, _, p)| *p != file_path);
    }

    fn export_tokens_by_path(&self, file_path: PathId) -> Vec<(TokenId, Namespace)> {
        let mut ret: Vec<_> = self
            .token_scopes
            .iter()
            .filter(|(_, (_, _, p))| *p == file_path)
            .map(|(id, (scope, define_context, _))| (*id, self.namespace(*scope, define_context)))
            .collect();
        ret.sort_unstable_by_key(|(id, _)| *id);
        ret
    }

    fn dump_tokens(&self) -> String {
        use std::fmt::Write;
        let mut entries: Vec<_> = self
            .token_scopes
            .iter()
            .map(|(id, (scope, define_context, path))| {
                (*id, self.namespace(*scope, define_context), *path)
            })
            .collect();
        entries.sort_by_key(|(id, _, _)| *id);

        let mut id_width = 0;
        let mut namespace_width = 0;
        for (id, namespace, _) in &entries {
            id_width = id_width.max(format!("{id}").len());
            namespace_width = namespace_width.max(format!("{namespace}").len());
        }

        let mut ret = String::from("NamespaceTable [\n");
        for (id, namespace, path) in &entries {
            writeln!(
                ret,
                "    {id:id_width$}: {namespace:namespace_width$} @ {path},"
            )
            .unwrap();
        }
        ret.push_str("]\n");
        ret
    }

    /// Dumps the owning declaration scopes keyed by structural name path, with
    /// kind and owner. Keyed by path (not `ScopeId`) so the result is
    /// independent of interning order, letting a cold run and a
    /// fragment-restored run be compared. Only owner-bearing scopes are
    /// included: a walk also interns owner-less scratch scopes for not-yet-known
    /// or cross-project names (the "treat as namespace" resolve fallback), which
    /// a restore legitimately does not reproduce.
    #[cfg(test)]
    fn dump_owned_scopes(&self) -> String {
        let mut lines: Vec<String> = self
            .scopes
            .iter()
            .filter(|s| s.owner.is_some())
            .map(|s| {
                let path = self
                    .name_path(s.id)
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join("::");
                format!("{path} : {:?} : {:?}", s.kind, s.owner)
            })
            .collect();
        lines.sort();
        lines.join("\n")
    }

    fn set_project(&mut self, project_name: StrId, is_root: bool) {
        let name = resource_table::canonical_str_id(project_name);
        self.intern_child(ScopeId(0), name, ScopeKind::Project);
        if is_root {
            self.root_project = Some(name);
        }
    }

    fn match_project_name(&self, name: StrId) -> bool {
        let name = resource_table::canonical_str_id(name);
        self.intern
            .get(&(0, name))
            .is_some_and(|&id| self.scopes[id as usize].kind == ScopeKind::Project)
    }

    fn root_project_name(&self) -> StrId {
        self.root_project.unwrap()
    }
}

thread_local!(static SCOPE_ARENA: RefCell<ScopeArena> = RefCell::new(ScopeArena::new()));

pub fn clear() {
    SCOPE_ARENA.with(|f| *f.borrow_mut() = ScopeArena::new())
}

pub fn intern_namespace(namespace: &Namespace) -> ScopeId {
    SCOPE_ARENA.with(|f| f.borrow_mut().intern_namespace(namespace))
}

pub fn intern_child(parent: ScopeId, name: StrId, kind: ScopeKind) -> ScopeId {
    SCOPE_ARENA.with(|f| f.borrow_mut().intern_child(parent, name, kind))
}

/// Scope a symbol's members live in: the child of the symbol's enclosing scope
/// named after the symbol. Equivalent to `intern_namespace(symbol.inner_namespace())`
/// since `symbol.scope == intern_namespace(symbol.namespace)`.
pub fn inner_scope(symbol_scope: ScopeId, name: StrId) -> ScopeId {
    intern_child(symbol_scope, name, ScopeKind::Unknown)
}

/// Registers a generic instance's member-scope delegation to its base template.
/// `base_enclosing` is the shared namespace scope of the template and its
/// instances. Returns the instance's inner (member) scope.
pub fn register_generic_instance(
    base_enclosing: ScopeId,
    base_name: StrId,
    mangled_name: StrId,
) -> ScopeId {
    SCOPE_ARENA.with(|f| {
        f.borrow_mut()
            .register_generic_instance(base_enclosing, base_name, mangled_name)
    })
}

/// The base template's member scope that a generic instance's member scope
/// delegates to, or `None` if `scope` is not a registered generic instance.
pub fn generic_delegation(scope: ScopeId) -> Option<ScopeId> {
    SCOPE_ARENA.with(|f| f.borrow().generic_delegation(scope))
}

pub fn get_mixin_target_scope(query_scope: ScopeId, source: ScopeId) -> Option<ScopeId> {
    SCOPE_ARENA.with(|f| f.borrow().get_mixin_target_scope(query_scope, source))
}

/// Read-only structural child lookup: the existing scope named `name` directly
/// under `parent`, or `None` if absent. Unlike `intern_child`, never creates a
/// scope.
pub fn child(parent: ScopeId, name: StrId) -> Option<ScopeId> {
    SCOPE_ARENA.with(|f| {
        let arena = f.borrow();
        let name = resource_table::canonical_str_id(name);
        arena.intern.get(&(parent.0, name)).copied().map(ScopeId)
    })
}

/// The symbol whose members live in `scope` — the scope's owning declaration —
/// or `None` for scopes with no owning symbol (root, project, or a synthetic
/// SystemVerilog interop scope).
pub fn owner_of(scope: ScopeId) -> Option<SymbolId> {
    SCOPE_ARENA.with(|f| f.borrow().scopes[scope.0 as usize].owner)
}

/// Interns an instance-side member scope (`name` under `inst_scope`, an instance
/// cursor) and records its delegation to the base template's member scope
/// `base_inner`, where the members actually live. This extends the top-level
/// delegation recursively so navigating into a member keeps the mangled
/// generic-instance prefix on the cursor path.
pub fn register_generic_child(inst_scope: ScopeId, name: StrId, base_inner: ScopeId) -> ScopeId {
    SCOPE_ARENA.with(|f| {
        let mut arena = f.borrow_mut();
        let child = arena.intern_child(inst_scope, name, ScopeKind::GenericInstance);
        arena.generic_delegations.insert(child, base_inner);
        child
    })
}

pub fn enter(name: StrId) -> ScopeId {
    SCOPE_ARENA.with(|f| f.borrow_mut().enter(name))
}

pub fn exit() -> ScopeId {
    SCOPE_ARENA.with(|f| f.borrow_mut().exit())
}

pub fn current() -> ScopeId {
    SCOPE_ARENA.with(|f| f.borrow().current)
}

pub fn set_current(scope: ScopeId) {
    SCOPE_ARENA.with(|f| f.borrow_mut().current = scope)
}

pub fn add_local(scope: ScopeId, name: StrId, symbol: SymbolId) {
    SCOPE_ARENA.with(|f| f.borrow_mut().add_local(scope, name, symbol))
}

pub fn set_kind_owner(scope: ScopeId, kind: ScopeKind, owner: SymbolId) {
    SCOPE_ARENA.with(|f| f.borrow_mut().set_kind_owner(scope, kind, owner))
}

pub fn add_import(
    scope: ScopeId,
    name: StrId,
    symbol: SymbolId,
    define_context: DefineContext,
    package_path: GenericSymbolPath,
) {
    SCOPE_ARENA.with(|f| {
        f.borrow_mut()
            .add_import(scope, name, symbol, define_context, package_path)
    })
}

pub fn add_wildcard(
    scope: ScopeId,
    source: ScopeId,
    define_context: DefineContext,
    source_define_context: DefineContext,
    package_path: GenericSymbolPath,
) {
    SCOPE_ARENA.with(|f| {
        f.borrow_mut().add_wildcard(
            scope,
            source,
            define_context,
            source_define_context,
            package_path,
        )
    })
}

pub fn add_mixin_source(
    scope: ScopeId,
    source: ScopeId,
    define_context: DefineContext,
    source_define_context: DefineContext,
    source_path: GenericSymbolPath,
) {
    SCOPE_ARENA.with(|f| {
        f.borrow_mut().add_mixin_source(
            scope,
            source,
            define_context,
            source_define_context,
            source_path,
        )
    })
}

pub fn name_path(scope: ScopeId) -> Vec<StrId> {
    SCOPE_ARENA.with(|f| f.borrow().name_path(scope))
}

/// The `Namespace` a scope corresponds to: its structural name path plus the
/// given ifdef context.
pub fn namespace(scope: ScopeId, define_context: &DefineContext) -> Namespace {
    SCOPE_ARENA.with(|f| f.borrow().namespace(scope, define_context))
}

/// The project a scope belongs to: the name of its root-level ancestor (the
/// first path element of its namespace), or `None` for the root scope.
pub fn project_of(scope: ScopeId) -> Option<StrId> {
    SCOPE_ARENA.with(|f| f.borrow().project_of(scope))
}

/// Number of named scopes from `scope` up to the root, i.e. the length of the
/// equivalent namespace path.
pub fn depth(scope: ScopeId) -> usize {
    SCOPE_ARENA.with(|f| f.borrow().depth(scope))
}

pub fn insert_token(token: TokenId, file_path: PathId, namespace: &Namespace) {
    SCOPE_ARENA.with(|f| f.borrow_mut().insert_token(token, file_path, namespace))
}

/// Records a token's resolution start directly from a scope, skipping the
/// namespace interning that [`insert_token`] performs. Use where the caller
/// already holds the scope rather than a materialized namespace.
pub fn insert_token_scope(
    token: TokenId,
    file_path: PathId,
    scope: ScopeId,
    define_context: &DefineContext,
) {
    SCOPE_ARENA.with(|f| {
        f.borrow_mut()
            .insert_token_scope(token, file_path, scope, define_context)
    })
}

/// The scope and ifdef context at which `token` appears, the resolution start
/// point, or `None` if the token was never recorded. The namespace path can be
/// reconstructed on demand with [`namespace`].
pub fn token_scope(token: TokenId) -> Option<(ScopeId, DefineContext)> {
    SCOPE_ARENA.with(|f| {
        f.borrow()
            .token_scopes
            .get(&token)
            .map(|(scope, define_context, _)| (*scope, define_context.clone()))
    })
}

pub fn drop_tokens(file_path: PathId) {
    SCOPE_ARENA.with(|f| f.borrow_mut().drop_tokens(file_path))
}

/// Exports all token entries belonging to one file, sorted by token ID.
/// Used by fragment caching.
pub fn export_tokens_by_path(file_path: PathId) -> Vec<(TokenId, Namespace)> {
    SCOPE_ARENA.with(|f| f.borrow().export_tokens_by_path(file_path))
}

pub fn dump_tokens() -> String {
    SCOPE_ARENA.with(|f| f.borrow().dump_tokens())
}

#[cfg(test)]
pub(crate) fn dump_owned_scopes() -> String {
    SCOPE_ARENA.with(|f| f.borrow().dump_owned_scopes())
}

/// Registers `project_name` as a project (a root-level scope of kind
/// [`ScopeKind::Project`]), recording the root project when `is_root`.
pub fn set_project(project_name: StrId, is_root: bool) {
    SCOPE_ARENA.with(|f| f.borrow_mut().set_project(project_name, is_root))
}

pub fn match_project_name(name: StrId) -> bool {
    SCOPE_ARENA.with(|f| f.borrow().match_project_name(name))
}

pub fn root_project_name() -> StrId {
    SCOPE_ARENA.with(|f| f.borrow().root_project_name())
}

#[cfg(test)]
pub(crate) fn get(scope: ScopeId) -> Option<Scope> {
    SCOPE_ARENA.with(|f| f.borrow().scopes.get(scope.0 as usize).cloned())
}

#[cfg(test)]
pub(crate) fn count() -> usize {
    SCOPE_ARENA.with(|f| f.borrow().scopes.len())
}

pub fn parent(scope: ScopeId) -> Option<ScopeId> {
    SCOPE_ARENA.with(|f| {
        f.borrow()
            .scopes
            .get(scope.0 as usize)
            .and_then(|s| s.parent)
    })
}

/// Returns the symbols bound to `name` directly in `scope` (not its parents),
/// in declaration order. Empty if the name is unbound there.
pub fn locals_get(scope: ScopeId, name: StrId) -> SVec<SymbolId> {
    let name = resource_table::canonical_str_id(name);
    SCOPE_ARENA.with(|f| {
        f.borrow()
            .scopes
            .get(scope.0 as usize)
            .and_then(|s| s.locals.get(&name).cloned())
            .unwrap_or_default()
    })
}

/// Returns the explicit `import` bindings of `name` directly in `scope`.
pub fn imports_get(scope: ScopeId, name: StrId) -> SVec<ImportBinding> {
    let name = resource_table::canonical_str_id(name);
    SCOPE_ARENA.with(|f| {
        f.borrow()
            .scopes
            .get(scope.0 as usize)
            .and_then(|s| s.imports.get(&name).cloned())
            .unwrap_or_default()
    })
}

/// As-written package qualifier of the import that brings `symbol` (named
/// `name`) into `query_namespace`. Walks the scope tree from the query scope
/// toward the root, returning the nearest enclosing import. Used to expand an
/// imported reference to its fully qualified form.
pub fn import_package_path(
    query_scope: ScopeId,
    symbol: SymbolId,
    name: StrId,
) -> Option<GenericSymbolPath> {
    let mut current = Some(query_scope);
    while let Some(scope) = current {
        for binding in imports_get(scope, name) {
            if binding.symbol == symbol {
                return Some((*binding.package_path).clone());
            }
        }
        for wildcard in wildcards_get(scope) {
            if locals_get(wildcard.source, name).contains(&symbol) {
                return Some((*wildcard.package_path).clone());
            }
        }
        current = parent(scope);
    }
    None
}

/// Whether `symbol` (named `name`, declared under `symbol_define_context`) is
/// imported directly into `namespace`, matching its exact paths and ifdef
/// context. Wildcard members are additionally gated by the source container's
/// ifdef context, mirroring the resolver's `Tier 2` wildcard visibility so an
/// ifdef-excluded member is not treated as imported.
pub fn is_imported(
    namespace: &Namespace,
    symbol: SymbolId,
    name: StrId,
    symbol_define_context: &DefineContext,
) -> bool {
    let scope = intern_namespace(namespace);
    let dctx = &namespace.define_context;
    imports_get(scope, name)
        .iter()
        .any(|b| b.symbol == symbol && &b.define_context == dctx)
        || wildcards_get(scope).iter().any(|w| {
            &w.define_context == dctx
                && !symbol_define_context.exclusive(&w.source_define_context)
                && locals_get(w.source, name).contains(&symbol)
        })
}

/// Returns the wildcard imports declared directly in `scope`.
pub fn wildcards_get(scope: ScopeId) -> SVec<WildcardImport> {
    SCOPE_ARENA.with(|f| {
        f.borrow()
            .scopes
            .get(scope.0 as usize)
            .map(|s| s.wildcards.clone())
            .unwrap_or_default()
    })
}

pub fn mixin_get(scope: ScopeId) -> SVec<Mixin> {
    SCOPE_ARENA.with(|f| {
        f.borrow()
            .scopes
            .get(scope.0 as usize)
            .map(|s| s.mixins.clone())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use veryl_parser::resource_table;

    fn name(s: &str) -> StrId {
        resource_table::insert_str(s)
    }

    #[test]
    fn intern_child_is_idempotent_and_structural() {
        clear();
        let root = ScopeId(0);
        // Same (parent, name) interns to one scope; the kind upgrades in place.
        let a = intern_child(root, name("a"), ScopeKind::Unknown);
        let a_again = intern_child(root, name("a"), ScopeKind::Package);
        assert_eq!(a, a_again);
        // Distinct names are distinct scopes.
        assert_ne!(a, intern_child(root, name("b"), ScopeKind::Unknown));
        // Structural path is recoverable.
        assert_eq!(parent(a), Some(root));
        assert_eq!(name_path(a), vec![name("a")]);
    }

    #[test]
    fn child_is_a_non_creating_lookup() {
        clear();
        let root = ScopeId(0);
        let before = count();
        assert_eq!(child(root, name("ghost")), None);
        assert_eq!(count(), before, "child() must not create scopes");
        let a = intern_child(root, name("a"), ScopeKind::Unknown);
        assert_eq!(child(root, name("a")), Some(a));
    }

    #[test]
    fn owner_roundtrip() {
        clear();
        let a = intern_child(ScopeId(0), name("a"), ScopeKind::Package);
        assert_eq!(owner_of(a), None);
        set_kind_owner(a, ScopeKind::Unknown, SymbolId(42));
        assert_eq!(owner_of(a), Some(SymbolId(42)));
    }

    #[test]
    fn intern_namespace_builds_and_dedups_the_chain() {
        clear();
        let mut ns = Namespace::new();
        ns.push(name("prj"));
        ns.push(name("pkg"));
        let scope = intern_namespace(&ns);
        assert_eq!(name_path(scope), vec![name("prj"), name("pkg")]);
        assert_eq!(depth(scope), 2);
        assert_eq!(intern_namespace(&ns), scope, "interning is idempotent");
    }

    #[test]
    fn generic_delegation_roundtrip() {
        clear();
        let enclosing = intern_child(ScopeId(0), name("prj"), ScopeKind::Project);
        let inst = register_generic_instance(enclosing, name("Foo"), name("__Foo__8"));
        let base_inner = intern_child(enclosing, name("Foo"), ScopeKind::Unknown);
        // The instance's member scope delegates to the base template's.
        assert_eq!(generic_delegation(inst), Some(base_inner));
        // A non-instance scope has no delegation.
        assert_eq!(generic_delegation(base_inner), None);
    }
}
