use crate::ir::VarPath;
use crate::literal::Literal;
use crate::literal_table;
use crate::namespace::{DefineContext, Namespace};
use crate::scope;
use crate::symbol::{
    DocComment, GenericInstanceProperty, GenericMap, Symbol, SymbolId, SymbolKind,
};
use crate::symbol_table;
use crate::{SVec, svec};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use veryl_parser::resource_table::{self, PathId, StrId, TokenId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::{Token, TokenSource, is_anonymous_token};

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolPath(pub SVec<StrId>);

impl SymbolPath {
    pub fn new(x: &[StrId]) -> Self {
        Self(
            x.iter()
                .map(|id| resource_table::canonical_str_id(*id))
                .collect(),
        )
    }

    pub fn push(&mut self, x: StrId) {
        self.0.push(resource_table::canonical_str_id(x))
    }

    pub fn pop(&mut self) -> Option<StrId> {
        self.0.pop()
    }

    pub fn clear(&mut self) {
        self.0.clear()
    }

    pub fn as_slice(&self) -> &[StrId] {
        self.0.as_slice()
    }

    pub fn to_vec(self) -> SVec<StrId> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for SymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        for path in self.as_slice() {
            text.push_str(&format!("{path} "));
        }
        text.fmt(f)
    }
}

impl From<&[Token]> for SymbolPath {
    fn from(value: &[Token]) -> Self {
        let mut ret = SymbolPath::default();
        for x in value {
            ret.push(x.text);
        }
        ret
    }
}

impl From<&Token> for SymbolPath {
    fn from(value: &Token) -> Self {
        let path = svec![resource_table::canonical_str_id(value.text)];
        SymbolPath(path)
    }
}

impl From<&syntax_tree::Identifier> for SymbolPath {
    fn from(value: &syntax_tree::Identifier) -> Self {
        let path = svec![resource_table::canonical_str_id(
            value.identifier_token.token.text
        )];
        SymbolPath(path)
    }
}

impl From<&[syntax_tree::Identifier]> for SymbolPath {
    fn from(value: &[syntax_tree::Identifier]) -> Self {
        let mut ret = SymbolPath::default();
        for x in value {
            ret.push(x.identifier_token.token.text);
        }
        ret
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        let mut ret = SymbolPath::default();
        ret.push(value.identifier.identifier_token.token.text);
        for x in &value.hierarchical_identifier_list0 {
            ret.push(x.identifier.identifier_token.token.text);
        }
        ret
    }
}

impl From<&syntax_tree::ScopedIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::ScopedIdentifier) -> Self {
        let path: GenericSymbolPath = value.into();
        path.generic_path()
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let mut path: SymbolPath = value.scoped_identifier.as_ref().into();
        for x in &value.expression_identifier_list0 {
            path.push(x.identifier.identifier_token.token.text);
        }
        path
    }
}

impl From<&str> for SymbolPath {
    fn from(value: &str) -> Self {
        let mut ret = SymbolPath::default();
        for x in value.split("::") {
            ret.push(x.into());
        }
        ret
    }
}

/// A path plus the scope to resolve it from. The third field is a runtime-only
/// scope hint carried by token-derived paths so resolution skips reconstructing
/// the namespace; persisted/explicit paths leave it `None` and resolution
/// interns the namespace.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolPathNamespace(
    pub SymbolPath,
    pub Namespace,
    #[serde(skip)] pub Option<scope::ScopeId>,
);

impl SymbolPathNamespace {
    pub fn pop_namespace(&mut self) -> Option<StrId> {
        if let Some(scope) = self.2.as_mut() {
            *scope = scope::parent(*scope).unwrap_or_default();
        }
        self.1.pop()
    }

    /// Builds a path rooted at a token's scope without materializing its
    /// namespace path; the resolution start is carried as a scope hint and the
    /// namespace holds only the token's ifdef context.
    pub(crate) fn from_token(path: SymbolPath, token: TokenId) -> Self {
        match scope::token_scope(token) {
            Some((scope, define_context)) => SymbolPathNamespace(
                path,
                Namespace {
                    paths: SVec::new(),
                    define_context,
                },
                Some(scope),
            ),
            None => SymbolPathNamespace(path, Namespace::default(), None),
        }
    }

    /// Builds a path rooted at an explicit scope without materializing its
    /// namespace path; the resolution start is carried as a scope hint and the
    /// namespace holds only the given ifdef context.
    pub fn from_scope(
        path: SymbolPath,
        scope: scope::ScopeId,
        define_context: DefineContext,
    ) -> Self {
        SymbolPathNamespace(
            path,
            Namespace {
                paths: SVec::new(),
                define_context,
            },
            Some(scope),
        )
    }
}

impl From<&Token> for SymbolPathNamespace {
    fn from(value: &Token) -> Self {
        SymbolPathNamespace::from_token(value.into(), value.id)
    }
}

impl From<&Vec<Token>> for SymbolPathNamespace {
    fn from(value: &Vec<Token>) -> Self {
        let path: Vec<_> = value.iter().map(|x| x.text).collect();
        let path = SymbolPath::new(&path);
        SymbolPathNamespace::from_token(path, value[0].id)
    }
}

impl From<&SymbolPathNamespace> for SymbolPathNamespace {
    fn from(value: &SymbolPathNamespace) -> Self {
        value.clone()
    }
}

impl From<(&SymbolPath, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&SymbolPath, &Namespace)) -> Self {
        SymbolPathNamespace(value.0.clone(), value.1.clone(), None)
    }
}

impl From<(&Token, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&Token, &Namespace)) -> Self {
        let path = SymbolPath::new(&[value.0.text]);
        SymbolPathNamespace(path, value.1.clone(), None)
    }
}

impl From<(&Vec<StrId>, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&Vec<StrId>, &Namespace)) -> Self {
        let path = SymbolPath::new(value.0);
        SymbolPathNamespace(path, value.1.clone(), None)
    }
}

impl From<(StrId, &Namespace)> for SymbolPathNamespace {
    fn from(value: (StrId, &Namespace)) -> Self {
        let path = SymbolPath::new(&[value.0]);
        SymbolPathNamespace(path, value.1.clone(), None)
    }
}

impl From<&syntax_tree::Identifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::Identifier) -> Self {
        SymbolPathNamespace::from_token(value.into(), value.identifier_token.token.id)
    }
}

impl From<(&syntax_tree::Identifier, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&syntax_tree::Identifier, &Namespace)) -> Self {
        let (identifier, namespace) = value;
        SymbolPathNamespace(identifier.into(), namespace.clone(), None)
    }
}

impl From<(&syntax_tree::Identifier, Option<&Namespace>)> for SymbolPathNamespace {
    fn from(value: (&syntax_tree::Identifier, Option<&Namespace>)) -> Self {
        let (identifier, namespace) = value;
        if let Some(namespace) = namespace {
            (identifier, namespace).into()
        } else {
            identifier.into()
        }
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        SymbolPathNamespace::from_token(value.into(), value.identifier.identifier_token.token.id)
    }
}

impl From<&syntax_tree::ScopedIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ScopedIdentifier) -> Self {
        SymbolPathNamespace::from_token(value.into(), value.identifier().token.id)
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        SymbolPathNamespace::from_token(value.into(), value.identifier().token.id)
    }
}

impl From<&GenericSymbolPathNamespace> for SymbolPathNamespace {
    fn from(value: &GenericSymbolPathNamespace) -> Self {
        SymbolPathNamespace(value.0.generic_path(), value.1.clone(), None)
    }
}

impl From<&GenericSymbolPath> for SymbolPathNamespace {
    fn from(value: &GenericSymbolPath) -> Self {
        SymbolPathNamespace::from_token(value.generic_path(), value.paths[0].base.id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericSymbolPathNamespace(pub GenericSymbolPath, pub Namespace);

impl From<(&GenericSymbolPath, &Namespace)> for GenericSymbolPathNamespace {
    fn from(value: (&GenericSymbolPath, &Namespace)) -> Self {
        let (path, namespace) = value;
        GenericSymbolPathNamespace(path.clone(), namespace.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GenericSymbolPathKind {
    Identifier,
    TypeLiteral,
    VariableType(Vec<usize>),
    ValueLiteral,
}

impl fmt::Display for GenericSymbolPathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            GenericSymbolPathKind::Identifier => "identifier".to_string(),
            GenericSymbolPathKind::TypeLiteral => "type".to_string(),
            GenericSymbolPathKind::VariableType(_) => "type".to_string(),
            GenericSymbolPathKind::ValueLiteral => "value".to_string(),
        };
        text.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GenericSymbolPath {
    pub paths: Vec<GenericSymbol>,
    pub kind: GenericSymbolPathKind,
    pub range: TokenRange,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GenericSymbol {
    pub base: Token,
    pub arguments: Vec<GenericSymbolPath>,
}

impl GenericSymbol {
    pub fn base(&self) -> StrId {
        self.base.text
    }

    pub fn mangled(&self) -> StrId {
        if self.arguments.is_empty() {
            self.base()
        } else {
            // If arguments have unresolved generic parameter, return base path
            for arg in &self.arguments {
                if !arg.is_resolvable() {
                    continue;
                }
                let head = &arg.paths[0];
                if let Ok(symbol) = symbol_table::resolve(&head.base)
                    && matches!(
                        symbol.found.kind,
                        SymbolKind::GenericParameter(_) | SymbolKind::GenericConst(_)
                    )
                {
                    return self.base();
                }
            }

            let mut text = format!("__{}", self.base);
            for a in &self.arguments {
                text.push('_');
                for a in a.mangled_path().0.as_slice() {
                    text.push_str(&format!("_{a}"));
                }
            }
            resource_table::insert_str(&text)
        }
    }

    pub fn get_generic_instance(
        &self,
        base: &Symbol,
        affiliation_symbol: Option<&Symbol>,
    ) -> Option<(Token, Symbol)> {
        if self.arguments.is_empty() {
            None
        } else {
            let affiliation_symbols = if let Some(symbol) = affiliation_symbol {
                vec![symbol.id]
            } else if let Some(symbol) = base.get_parent() {
                vec![symbol.id]
            } else {
                vec![]
            };
            let property = GenericInstanceProperty {
                base: base.id,
                arguments: self.arguments.clone(),
                affiliation_symbols,
            };
            let kind = SymbolKind::GenericInstance(property);
            let token = &self.base;
            let token = Token::new(
                &self.mangled().to_string(),
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

impl GenericSymbolPath {
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn base_path(&self, i: usize) -> SymbolPath {
        let path: Vec<_> = self
            .paths
            .iter()
            .enumerate()
            .filter_map(|(j, x)| match i.cmp(&j) {
                Ordering::Greater => Some(x.mangled()),
                Ordering::Equal => Some(x.base()),
                Ordering::Less => None,
            })
            .collect();
        SymbolPath::new(&path)
    }

    pub fn slice(&self, i: usize) -> GenericSymbolPath {
        let paths: Vec<_> = self.paths.clone().drain(0..=i).collect();
        let range = TokenRange {
            beg: paths.first().map(|x| x.base).unwrap(),
            end: paths.last().map(|x| x.base).unwrap(),
        };
        GenericSymbolPath {
            paths,
            kind: self.kind.clone(),
            range,
        }
    }

    pub fn mangled_path(&self) -> SymbolPath {
        let mut path: Vec<_> = self.paths.iter().map(|x| x.mangled()).collect();

        if let GenericSymbolPathKind::VariableType(width) = &self.kind
            && let Some(x) = path.pop()
        {
            let mut text = x.to_string();
            for w in width {
                text.push_str(&format!("_{}", w));
            }

            let id = resource_table::insert_str(&text);
            path.push(id);
        }

        SymbolPath::new(&path)
    }

    pub fn generic_path(&self) -> SymbolPath {
        let path: Vec<_> = self.paths.iter().map(|x| x.base()).collect();
        SymbolPath::new(&path)
    }

    pub fn generic_arguments(&self) -> Vec<Vec<GenericSymbolPath>> {
        let path: Vec<_> = self.paths.iter().map(|x| x.arguments.clone()).collect();
        path
    }

    pub fn file_path(&self) -> Option<PathId> {
        if let TokenSource::File { path, .. } = self.range.beg.source {
            Some(path)
        } else {
            None
        }
    }

    pub fn unalias(&mut self, generic_maps: Option<&Vec<GenericMap>>) {
        self.unalias_inner(generic_maps, &mut Vec::new());
    }

    fn unalias_inner(
        &mut self,
        generic_maps: Option<&Vec<GenericMap>>,
        visited: &mut Vec<SymbolId>,
    ) {
        if !self.is_resolvable() {
            return;
        }

        let Some((scope, define_context)) = scope::token_scope(self.paths[0].base.id) else {
            return;
        };

        let mut generic_maps = generic_maps.cloned().unwrap_or_default();
        for i in 0..self.len() {
            let symbol = symbol_table::resolve_generic_structural(
                &self.slice(i),
                (scope, define_context.clone()),
            );

            if let Ok(ref symbol) = symbol
                && let Some(mut alias_target) = symbol.found.alias_target(false)
            {
                // cyclic-alias guard: stop if this alias was already visited.
                if visited.contains(&symbol.found.id) {
                    return;
                }
                visited.push(symbol.found.id);

                alias_target.resolve_imported(scope, &define_context, Some(&generic_maps));
                alias_target.apply_map(&generic_maps);
                if (i + 1) < self.len() {
                    for j in (i + 1)..self.len() {
                        alias_target.paths.push(self.paths[j].clone());
                    }
                }
                alias_target.unalias_inner(Some(&generic_maps), visited);

                self.paths = alias_target.paths;
                self.kind = alias_target.kind;
                self.range = alias_target.range;
                return;
            }

            if let Some(path) = self.paths.get_mut(i) {
                for arg in path.arguments.iter_mut() {
                    arg.unalias(Some(&generic_maps));
                }

                if let Ok(ref symbol) = symbol
                    && (matches!(&symbol.found.kind, SymbolKind::GenericInstance(_))
                        || symbol.found.has_generic_consts())
                {
                    let map = symbol
                        .found
                        .generic_map(Some(symbol.found.id), &path.arguments);
                    generic_maps.push(map);
                }
            }
        }
    }

    pub fn to_literal(&self) -> Option<Literal> {
        literal_table::get(&self.paths[0].base.id)
    }

    pub fn to_generic_maps(&self) -> Vec<GenericMap> {
        let mut ret = vec![];

        let mut full_path = vec![];
        for path in &self.paths {
            full_path.push(path.base);
            if let Ok(symbol) = symbol_table::resolve(&full_path) {
                let mut map = GenericMap::default();

                if let SymbolKind::GenericInstance(inst) = &symbol.found.kind {
                    let params = inst.base_symbol().generic_parameters();
                    for (i, (name, _)) in params.iter().enumerate() {
                        map.map.insert(*name, inst.arguments[i].clone());
                    }
                } else {
                    let params = symbol.found.generic_parameters();
                    for (i, (name, value)) in params.into_iter().enumerate() {
                        if let Some(x) = path.arguments.get(i) {
                            map.map.insert(name, x.clone());
                        } else if let Some(x) = value.default_value {
                            map.map.insert(name, x);
                        }
                    }
                }

                ret.push(map);
            }
        }

        ret
    }

    pub fn is_anonymous(&self) -> bool {
        self.paths.len() == 1
            && self.paths[0].arguments.is_empty()
            && is_anonymous_token(&self.paths[0].base)
    }

    pub fn is_resolvable(&self) -> bool {
        self.kind == GenericSymbolPathKind::Identifier
    }

    pub fn is_generic(&self) -> bool {
        for path in &self.paths {
            if !path.arguments.is_empty() {
                return true;
            }
        }
        false
    }

    pub fn is_generic_reference(&self) -> bool {
        // path starts with generic parameter
        if !self.is_resolvable() {
            return false;
        }

        let head = &self.paths[0];
        if let Ok(symbol) = symbol_table::resolve(&head.base) {
            let is_generic = matches!(
                symbol.found.kind,
                SymbolKind::GenericParameter(_) | SymbolKind::GenericConst(_)
            ) || matches!(symbol.found.kind, SymbolKind::Package(ref x) if x.is_proto)
                || (symbol.imported
                    && symbol
                        .found
                        .get_parent_package()
                        .map(|x| matches!(x.kind, SymbolKind::Package(ref y) if y.is_proto))
                        .unwrap_or(false));
            if is_generic {
                return true;
            }
        }
        // path contains generic parameter as generic argument
        for path in &self.paths {
            for arg in &path.arguments {
                if arg.is_generic_reference() {
                    return true;
                }
            }
        }

        false
    }

    pub fn apply_map(&mut self, maps: &[GenericMap]) {
        for map in maps.iter().rev() {
            let head = &self.paths[0];
            if let Some(x) = map.map.get(&head.base()) {
                let mut paths: Vec<_> = self.paths.drain(1..).collect();
                self.paths.clone_from(&x.paths);
                self.paths.append(&mut paths);
                self.kind = x.kind.clone();
                self.range = x.range;
                break;
            }
        }

        for path in &mut self.paths {
            for arg in &mut path.arguments {
                arg.apply_map(maps);
            }
        }
    }

    /// Resolve and expand path if the path is imported at declaration
    pub fn resolve_imported(
        &mut self,
        scope: scope::ScopeId,
        define_context: &DefineContext,
        generic_maps: Option<&Vec<GenericMap>>,
    ) {
        if !self.is_resolvable() {
            return;
        }

        // Qualification targets the head. For a multi-segment path the head is a
        // prefix, so it must resolve as the enclosing package, not a same-named
        // imported item that would re-qualify this already-qualified path.
        let head = self.slice(0).generic_path();
        let head_alone_imported = || {
            symbol_table::resolve(SymbolPathNamespace::from_scope(
                head.clone(),
                scope,
                define_context.clone(),
            ))
            .map(|x| x.imported)
            .unwrap_or(false)
        };
        // A multi-segment path whose head resolves (as a prefix, container
        // preferred) to a package reached without import is already qualified and
        // must not be re-qualified — otherwise an item imported under its
        // package's name would re-qualify it endlessly. Any other head uses the
        // head-alone import test.
        let head_already_qualified = self.paths.len() >= 2
            && symbol_table::resolve(SymbolPathNamespace::from_scope(
                self.generic_path(),
                scope,
                define_context.clone(),
            ))
            .ok()
            .and_then(|r| r.full_path.first().copied())
            .and_then(symbol_table::get)
            .is_some_and(|head| {
                head.is_package(false)
                    && !scope::is_imported(
                        &scope::namespace(scope, define_context),
                        head.id,
                        head.token.text,
                        &head.namespace.define_context,
                    )
            });
        if !head_already_qualified && head_alone_imported() {
            let (self_scope, self_define_context) = scope::token_scope(self.range.beg.id).unwrap();
            let Some(self_file_path) = self.file_path() else {
                return;
            };

            if let Ok(head_symbol) = symbol_table::resolve(SymbolPathNamespace::from_scope(
                head.clone(),
                self_scope,
                self_define_context.clone(),
            )) && head_symbol.found.get_parent_package().is_some()
                && let Some(mut package_path) = scope::import_package_path(
                    scope,
                    head_symbol.found.id,
                    head_symbol.found.token.text,
                )
            {
                if let Some(maps) = generic_maps {
                    package_path.apply_map(maps);
                }
                package_path.unalias(None);

                if let Ok(package_symbol) = symbol_table::resolve(SymbolPathNamespace::from_scope(
                    package_path.generic_path(),
                    scope,
                    define_context.clone(),
                )) && package_symbol.imported
                    && matches!(package_symbol.found.kind, SymbolKind::AliasPackage(_))
                {
                    // 'package_path' points imported alias package or proto alias package.
                    package_path.resolve_imported(scope, define_context, generic_maps);
                }

                for (i, path) in package_path.paths.iter().enumerate() {
                    let token = Token::generate(path.base.text, self_file_path);
                    scope::insert_token_scope(
                        token.id,
                        self_file_path,
                        self_scope,
                        &self_define_context,
                    );

                    let mut path = path.clone();
                    path.base = token;
                    self.paths.insert(i, path);
                }
            }
        }

        for path in &mut self.paths {
            for arg in &mut path.arguments {
                arg.resolve_imported(scope, define_context, generic_maps);
            }
        }
    }

    pub fn append_namespace_path(&mut self, namespace: &Namespace, target_namespace: &Namespace) {
        fn is_defined_in_package(symbol: &Symbol) -> bool {
            if matches!(
                symbol.kind,
                SymbolKind::GenericParameter(_) | SymbolKind::GenericConst(_)
            ) {
                // Generic parameter and const are not visible from other namespace
                return false;
            }

            symbol_table::get_namespace_symbol(&symbol.namespace)
                .map(|x| x.is_package(true))
                .unwrap_or(false)
        }

        fn start_with_project_name(namespace: &Namespace) -> bool {
            namespace.depth() >= 1 && scope::match_project_name(namespace.paths[0])
        }

        fn add_root_project_name(namespace: &Namespace) -> Namespace {
            let mut ret = namespace.clone();
            if !start_with_project_name(&ret) {
                ret.paths.insert(0, scope::root_project_name());
            }
            ret
        }

        // The head resolves directly from the (possibly mangled) namespace:
        // delegation-aware lexical lookup descends into the base template, so no
        // demangling is needed.
        if let Ok(head_symbol) = symbol_table::resolve((&self.base_path(0), namespace))
            && let Some(file_path) = self.file_path()
        {
            let head_symbol = &head_symbol.found;
            let head_namespace = &head_symbol.namespace;

            let mut namespace = add_root_project_name(namespace);
            if start_with_project_name(head_namespace) {
                namespace.paths.drain(head_namespace.depth()..);
            } else {
                namespace.paths.drain((head_namespace.depth() + 1)..);
            }

            let target_namespace = add_root_project_name(target_namespace);

            if !namespace.matched(&target_namespace) && is_defined_in_package(head_symbol) {
                // The given path is referened from the `target_namespace`
                // but it is not visible because it has no package paths.
                for i in 1..namespace.depth() {
                    let token = Token::generate(namespace.paths[i], file_path);
                    scope::insert_token(token.id, file_path, &namespace);

                    let symbol_path = GenericSymbol {
                        base: token,
                        arguments: vec![],
                    };
                    self.paths.insert(i - 1, symbol_path);
                }
            }

            if namespace.paths[0] != target_namespace.paths[0] && head_symbol.is_component(true) {
                // Append the project prefix to the path.
                let token = Token::generate(namespace.paths[0], file_path);
                scope::insert_token(token.id, file_path, &namespace);

                let symbol_path = GenericSymbol {
                    base: token,
                    arguments: vec![],
                };
                self.paths.insert(0, symbol_path);
            }
        }

        for i in 0..self.len() {
            if !self.paths[i].arguments.is_empty()
                && let Ok(path_symbol) =
                    symbol_table::resolve((&self.slice(i).generic_path(), namespace))
            {
                for arg in &mut self.paths[i].arguments {
                    arg.append_namespace_path(namespace, &path_symbol.found.namespace);
                }
            }
        }
    }

    pub fn to_var_path(&self) -> Option<VarPath> {
        let mut ret = VarPath::default();
        for path in &self.paths {
            if path.arguments.is_empty() {
                ret.push(path.base());
            } else {
                return None;
            }
        }
        Some(ret)
    }
}

impl From<&Token> for GenericSymbolPath {
    fn from(value: &Token) -> Self {
        let path = GenericSymbol {
            base: *value,
            arguments: vec![],
        };
        GenericSymbolPath {
            paths: vec![path],
            kind: GenericSymbolPathKind::Identifier,
            range: value.into(),
        }
    }
}

impl From<&syntax_tree::FixedType> for GenericSymbolPath {
    fn from(value: &syntax_tree::FixedType) -> Self {
        let token: TokenRange = value.into();
        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: token.beg,
                arguments: Vec::new(),
            }],
            kind: GenericSymbolPathKind::TypeLiteral,
            range: token,
        }
    }
}

impl From<(&syntax_tree::VariableType, &Vec<usize>)> for GenericSymbolPath {
    fn from(value: (&syntax_tree::VariableType, &Vec<usize>)) -> Self {
        let token: TokenRange = value.0.into();
        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: token.beg,
                arguments: Vec::new(),
            }],
            kind: GenericSymbolPathKind::VariableType(value.1.clone()),
            range: token,
        }
    }
}

impl From<&syntax_tree::Number> for GenericSymbolPath {
    fn from(value: &syntax_tree::Number) -> Self {
        let token = match value {
            syntax_tree::Number::IntegralNumber(x) => match x.integral_number.as_ref() {
                syntax_tree::IntegralNumber::Based(x) => x.based.based_token.token,
                syntax_tree::IntegralNumber::AllBit(x) => x.all_bit.all_bit_token.token,
                syntax_tree::IntegralNumber::BaseLess(x) => x.base_less.base_less_token.token,
            },
            syntax_tree::Number::RealNumber(x) => match x.real_number.as_ref() {
                syntax_tree::RealNumber::Exponent(x) => x.exponent.exponent_token.token,
                syntax_tree::RealNumber::FixedPoint(x) => x.fixed_point.fixed_point_token.token,
            },
        };

        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: token,
                arguments: Vec::new(),
            }],
            kind: GenericSymbolPathKind::ValueLiteral,
            range: token.into(),
        }
    }
}

impl From<&syntax_tree::BooleanLiteral> for GenericSymbolPath {
    fn from(value: &syntax_tree::BooleanLiteral) -> Self {
        let token = match value {
            syntax_tree::BooleanLiteral::True(x) => x.r#true.true_token.token,
            syntax_tree::BooleanLiteral::False(x) => x.r#false.false_token.token,
        };
        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: token,
                arguments: Vec::new(),
            }],
            kind: GenericSymbolPathKind::ValueLiteral,
            range: token.into(),
        }
    }
}

impl From<&syntax_tree::WithGenericArgumentItem> for GenericSymbolPath {
    fn from(value: &syntax_tree::WithGenericArgumentItem) -> Self {
        match value {
            syntax_tree::WithGenericArgumentItem::GenericArgIdentifier(x) => {
                x.generic_arg_identifier.as_ref().into()
            }
            syntax_tree::WithGenericArgumentItem::FixedType(x) => x.fixed_type.as_ref().into(),
            syntax_tree::WithGenericArgumentItem::Number(x) => x.number.as_ref().into(),
            syntax_tree::WithGenericArgumentItem::BooleanLiteral(x) => {
                x.boolean_literal.as_ref().into()
            }
        }
    }
}

impl From<&syntax_tree::Identifier> for GenericSymbolPath {
    fn from(value: &syntax_tree::Identifier) -> Self {
        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: value.identifier_token.token,
                arguments: Vec::new(),
            }],
            kind: GenericSymbolPathKind::Identifier,
            range: value.into(),
        }
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for GenericSymbolPath {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        let mut path: Self = value.identifier.as_ref().into();

        for x in &value.hierarchical_identifier_list0 {
            path.paths.push(GenericSymbol {
                base: x.identifier.identifier_token.token,
                arguments: vec![],
            });
        }

        path.range = value.into();
        path
    }
}

impl From<&syntax_tree::ScopedIdentifier> for GenericSymbolPath {
    fn from(value: &syntax_tree::ScopedIdentifier) -> Self {
        let mut paths = Vec::new();
        match value.scoped_identifier_group.as_ref() {
            syntax_tree::ScopedIdentifierGroup::DollarIdentifier(x) => {
                let base = x.dollar_identifier.dollar_identifier_token.token;
                paths.push(GenericSymbol {
                    base,
                    arguments: Vec::new(),
                });
            }
            syntax_tree::ScopedIdentifierGroup::IdentifierScopedIdentifierOpt(x) => {
                let base = x.identifier.identifier_token.token;
                let mut arguments = Vec::new();

                if let Some(ref x) = x.scoped_identifier_opt
                    && let Some(ref x) = x.with_generic_argument.with_generic_argument_opt
                {
                    let list: Vec<_> = x.with_generic_argument_list.as_ref().into();
                    for x in &list {
                        let arg: GenericSymbolPath = (*x).into();
                        arguments.push(arg);
                    }
                }

                paths.push(GenericSymbol { base, arguments });
            }
        }

        for x in &value.scoped_identifier_list {
            let base = x.identifier.identifier_token.token;
            let mut arguments = Vec::new();

            if let Some(ref x) = x.scoped_identifier_opt0
                && let Some(ref x) = x.with_generic_argument.with_generic_argument_opt
            {
                let list: Vec<_> = x.with_generic_argument_list.as_ref().into();
                for x in &list {
                    let arg: GenericSymbolPath = (*x).into();
                    arguments.push(arg);
                }
            }

            paths.push(GenericSymbol { base, arguments });
        }

        GenericSymbolPath {
            paths,
            kind: GenericSymbolPathKind::Identifier,
            range: value.into(),
        }
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for GenericSymbolPath {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let mut path: GenericSymbolPath = value.scoped_identifier.as_ref().into();

        for base in value
            .expression_identifier_list0
            .iter()
            .map(|x| x.identifier.identifier_token.token)
        {
            path.paths.push(GenericSymbol {
                base,
                arguments: vec![],
            });
        }

        path.range = value.into();
        path
    }
}

impl From<&syntax_tree::GenericArgIdentifier> for GenericSymbolPath {
    fn from(value: &syntax_tree::GenericArgIdentifier) -> Self {
        let mut path: Self = value.scoped_identifier.as_ref().into();

        for x in &value.generic_arg_identifier_list {
            path.paths.push(GenericSymbol {
                base: x.identifier.identifier_token.token,
                arguments: vec![],
            });
        }

        path.range = value.into();
        path
    }
}

impl fmt::Display for GenericSymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        for (i, path) in self.paths.iter().enumerate() {
            if i == 0 {
                text.push_str(&format!("{}", path.mangled()));
            } else {
                text.push_str(&format!(" {}", path.mangled()));
            }
        }

        if let GenericSymbolPathKind::VariableType(width) = &self.kind {
            for w in width {
                text.push_str(&format!("_{}", w));
            }
        }

        text.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veryl_parser::veryl_token::{Token, TokenSource};

    fn tok(name: &str) -> Token {
        Token::new(name, 0, 0, 0, 0, TokenSource::External)
    }

    fn seg(name: &str, arguments: Vec<GenericSymbolPath>) -> GenericSymbol {
        GenericSymbol {
            base: tok(name),
            arguments,
        }
    }

    fn gpath(paths: Vec<GenericSymbol>) -> GenericSymbolPath {
        let range = TokenRange {
            beg: paths.first().unwrap().base,
            end: paths.last().unwrap().base,
        };
        GenericSymbolPath {
            paths,
            kind: GenericSymbolPathKind::Identifier,
            range,
        }
    }

    fn names(path: &SymbolPath) -> Vec<String> {
        path.0.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn generic_path_drops_arguments() {
        // `A::<1>::B` reduces to the base names `[A, B]` (arguments stripped).
        let one = gpath(vec![seg("1", vec![])]);
        let path = gpath(vec![seg("A", vec![one]), seg("B", vec![])]);
        assert_eq!(names(&path.generic_path()), vec!["A", "B"]);
    }

    #[test]
    fn slice_is_inclusive_prefix() {
        let path = gpath(vec![seg("A", vec![]), seg("B", vec![]), seg("c", vec![])]);
        assert_eq!(path.slice(0).len(), 1);
        assert_eq!(path.slice(1).len(), 2);
        assert_eq!(names(&path.slice(1).generic_path()), vec!["A", "B"]);
    }

    #[test]
    fn base_path_keeps_non_generic_prefix_and_drops_suffix() {
        // No generic arguments: prefix segments pass through unmangled, segment `i`
        // is the base, trailing segments are dropped.
        let path = gpath(vec![seg("A", vec![]), seg("B", vec![]), seg("c", vec![])]);
        assert_eq!(names(&path.base_path(0)), vec!["A"]);
        assert_eq!(names(&path.base_path(1)), vec!["A", "B"]);
    }

    #[test]
    fn base_path_mangles_generic_prefix() {
        // A fresh table makes the literal argument unresolvable, so `mangled()`
        // takes the structural (non-parameter) branch deterministically.
        crate::symbol_table::clear();
        // `A::<1>::B::c` at i=1: prefix `A::<1>` is mangled, `B` is the base, `c`
        // is dropped.
        let one = gpath(vec![seg("1", vec![])]);
        let path = gpath(vec![
            seg("A", vec![one]),
            seg("B", vec![]),
            seg("c", vec![]),
        ]);
        let base_path = names(&path.base_path(1));
        assert_eq!(base_path.len(), 2);
        assert_eq!(base_path[0], "__A__1");
        assert_eq!(base_path[1], "B");
    }
}
