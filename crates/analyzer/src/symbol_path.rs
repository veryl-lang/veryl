use crate::ir::VarPath;
use crate::literal::Literal;
use crate::literal_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{DocComment, GenericInstanceProperty, GenericMap, Symbol, SymbolKind};
use crate::symbol_table;
use crate::{SVec, svec};
use std::cmp::Ordering;
use std::fmt;
use veryl_parser::resource_table::{self, PathId, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::{Token, TokenSource, is_anonymous_token};

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct SymbolPath(pub SVec<StrId>);

impl SymbolPath {
    pub fn new(x: &[StrId]) -> Self {
        Self(x.into())
    }

    pub fn push(&mut self, x: StrId) {
        self.0.push(x)
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
        let mut path = SVec::new();
        for x in value {
            path.push(x.text);
        }
        SymbolPath(path)
    }
}

impl From<&Token> for SymbolPath {
    fn from(value: &Token) -> Self {
        let path = svec![value.text];
        SymbolPath(path)
    }
}

impl From<&syntax_tree::Identifier> for SymbolPath {
    fn from(value: &syntax_tree::Identifier) -> Self {
        let path = svec![value.identifier_token.token.text];
        SymbolPath(path)
    }
}

impl From<&[syntax_tree::Identifier]> for SymbolPath {
    fn from(value: &[syntax_tree::Identifier]) -> Self {
        let mut path = SVec::new();
        for x in value {
            path.push(x.identifier_token.token.text);
        }
        SymbolPath(path)
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        let mut path = SVec::new();
        path.push(value.identifier.identifier_token.token.text);
        for x in &value.hierarchical_identifier_list0 {
            path.push(x.identifier.identifier_token.token.text);
        }
        SymbolPath(path)
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
        let mut path = SVec::new();
        for x in value.split("::") {
            path.push(x.into());
        }
        SymbolPath(path)
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct SymbolPathNamespace(pub SymbolPath, pub Namespace);

impl SymbolPathNamespace {
    pub fn pop_namespace(&mut self) -> Option<StrId> {
        self.1.pop()
    }
}

impl From<&Token> for SymbolPathNamespace {
    fn from(value: &Token) -> Self {
        let namespace = namespace_table::get(value.id).unwrap_or_default();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&Vec<Token>> for SymbolPathNamespace {
    fn from(value: &Vec<Token>) -> Self {
        let namespace = namespace_table::get(value[0].id).unwrap_or_default();
        let path: Vec<_> = value.iter().map(|x| x.text).collect();
        let path = SymbolPath::new(&path);
        SymbolPathNamespace(path, namespace)
    }
}

impl From<&SymbolPathNamespace> for SymbolPathNamespace {
    fn from(value: &SymbolPathNamespace) -> Self {
        value.clone()
    }
}

impl From<(&SymbolPath, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&SymbolPath, &Namespace)) -> Self {
        SymbolPathNamespace(value.0.clone(), value.1.clone())
    }
}

impl From<(&Token, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&Token, &Namespace)) -> Self {
        let path = SymbolPath::new(&[value.0.text]);
        SymbolPathNamespace(path, value.1.clone())
    }
}

impl From<(&Vec<Token>, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&Vec<Token>, &Namespace)) -> Self {
        let path: Vec<_> = value.0.iter().map(|x| x.text).collect();
        let path = SymbolPath::new(&path);
        SymbolPathNamespace(path, value.1.clone())
    }
}

impl From<(&Vec<StrId>, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&Vec<StrId>, &Namespace)) -> Self {
        let path = SymbolPath::new(value.0);
        SymbolPathNamespace(path, value.1.clone())
    }
}

impl From<(&SVec<StrId>, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&SVec<StrId>, &Namespace)) -> Self {
        let path = SymbolPath::new(value.0);
        SymbolPathNamespace(path, value.1.clone())
    }
}

impl From<(StrId, &Namespace)> for SymbolPathNamespace {
    fn from(value: (StrId, &Namespace)) -> Self {
        let path = SymbolPath::new(&[value.0]);
        SymbolPathNamespace(path, value.1.clone())
    }
}

impl From<&syntax_tree::Identifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::Identifier) -> Self {
        let namespace = namespace_table::get(value.identifier_token.token.id).unwrap_or_default();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<(&syntax_tree::Identifier, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&syntax_tree::Identifier, &Namespace)) -> Self {
        let (identifier, namespace) = value;
        SymbolPathNamespace(identifier.into(), namespace.clone())
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

impl From<&[syntax_tree::Identifier]> for SymbolPathNamespace {
    fn from(value: &[syntax_tree::Identifier]) -> Self {
        let namespace =
            namespace_table::get(value[0].identifier_token.token.id).unwrap_or_default();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        let namespace =
            namespace_table::get(value.identifier.identifier_token.token.id).unwrap_or_default();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::ScopedIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ScopedIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier().token.id).unwrap_or_default();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier().token.id).unwrap_or_default();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&GenericSymbolPathNamespace> for SymbolPathNamespace {
    fn from(value: &GenericSymbolPathNamespace) -> Self {
        SymbolPathNamespace(value.0.generic_path(), value.1.clone())
    }
}

impl From<&GenericSymbolPath> for SymbolPathNamespace {
    fn from(value: &GenericSymbolPath) -> Self {
        let namespace = namespace_table::get(value.paths[0].base.id).unwrap_or_default();
        SymbolPathNamespace(value.generic_path(), namespace)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericSymbolPathNamespace(pub GenericSymbolPath, pub Namespace);

impl From<(&GenericSymbolPath, &Namespace)> for GenericSymbolPathNamespace {
    fn from(value: (&GenericSymbolPath, &Namespace)) -> Self {
        let (path, namespace) = value;
        GenericSymbolPathNamespace(path.clone(), namespace.clone())
    }
}

#[derive(Copy, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GenericSymbolPathKind {
    Identifier,
    TypeLiteral,
    ValueLiteral,
}

impl fmt::Display for GenericSymbolPathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            GenericSymbolPathKind::Identifier => "identifier".to_string(),
            GenericSymbolPathKind::TypeLiteral => "type".to_string(),
            GenericSymbolPathKind::ValueLiteral => "value".to_string(),
        };
        text.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenericSymbolPath {
    pub paths: Vec<GenericSymbol>,
    pub kind: GenericSymbolPathKind,
    pub range: TokenRange,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
                    && matches!(symbol.found.kind, SymbolKind::GenericParameter(_))
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
            let affiliation_id = if let Some(symbol) = affiliation_symbol {
                Some(symbol.id)
            } else {
                base.get_parent().map(|parent| parent.id)
            };
            let property = GenericInstanceProperty {
                base: base.id,
                arguments: self.arguments.clone(),
                affiliation_symbol: affiliation_id,
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
            kind: self.kind,
            range,
        }
    }

    pub fn mangled_path(&self) -> SymbolPath {
        let path: Vec<_> = self.paths.iter().map(|x| x.mangled()).collect();
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

    pub fn unalias(&mut self) {
        if !self.is_resolvable() {
            return;
        }

        let Some(namespace) = namespace_table::get(self.paths[0].base.id) else {
            return;
        };
        let mut generic_maps: Vec<_> = Vec::new();

        for i in 0..self.len() {
            let symbol = symbol_table::resolve((&self.slice(i).mangled_path(), &namespace));

            if let Ok(ref symbol) = symbol
                && let Some(mut alias_target) = symbol.found.alias_target(false)
            {
                alias_target.resolve_imported(&namespace, Some(&generic_maps));
                alias_target.apply_map(&generic_maps);
                if (i + 1) < self.len() {
                    for j in (i + 1)..self.len() {
                        alias_target.paths.push(self.paths[j].clone());
                    }
                }
                alias_target.unalias();

                self.paths = alias_target.paths;
                self.kind = alias_target.kind;
                self.range = alias_target.range;
                return;
            }

            if let Some(path) = self.paths.get_mut(i) {
                for arg in path.arguments.iter_mut() {
                    arg.unalias();
                }

                if let Ok(ref symbol) = symbol
                    && matches!(&symbol.found.kind, SymbolKind::GenericInstance(_))
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
                let params = symbol.found.generic_parameters();

                let mut map = GenericMap::default();

                for (i, (name, value)) in params.into_iter().enumerate() {
                    if let Some(x) = path.arguments.get(i) {
                        map.map.insert(name, x.clone());
                    } else if let Some(x) = value.default_value {
                        map.map.insert(name, x);
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
                SymbolKind::GenericParameter(_) | SymbolKind::ProtoPackage(_)
            ) || (symbol.imported
                && symbol
                    .found
                    .get_parent_package()
                    .map(|x| matches!(x.kind, SymbolKind::ProtoPackage(_)))
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
                self.kind = x.kind;
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
        namespace: &Namespace,
        generic_maps: Option<&Vec<GenericMap>>,
    ) {
        if !self.is_resolvable() {
            return;
        }

        // Import process is performed against the head symbol of the given path.
        let head = self.slice(0).generic_path();
        if let Ok(head_symbol) = symbol_table::resolve((&head, namespace))
            && head_symbol.imported
        {
            let self_namespace = namespace_table::get(self.range.beg.id).unwrap();
            let Some(self_file_path) = self.file_path() else {
                return;
            };

            if let Ok(head_symbol) = symbol_table::resolve((&head, &self_namespace))
                && head_symbol.found.get_parent_package().is_some()
                && let Some(import) = head_symbol
                    .found
                    .imported
                    .iter()
                    .find(|x| namespace.included(&x.namespace))
            {
                let mut package_path = import.path.0.clone();
                if !import.wildcard {
                    package_path.paths.pop();
                }
                if let Some(maps) = generic_maps {
                    package_path.apply_map(maps);
                }
                package_path.unalias();

                for (i, path) in package_path.paths.iter().enumerate() {
                    let token = Token::generate(path.base.text, self_file_path);
                    namespace_table::insert(token.id, self_file_path, &self_namespace);

                    let mut path = path.clone();
                    path.base = token;
                    self.paths.insert(i, path);
                }
            }
        }

        for path in &mut self.paths {
            for arg in &mut path.arguments {
                arg.resolve_imported(namespace, generic_maps);
            }
        }
    }

    pub fn append_namespace_path(&mut self, namespace: &Namespace, target_namespace: &Namespace) {
        fn is_defined_in_package(path: &GenericSymbolPath, namespace: &Namespace) -> bool {
            if !namespace
                .get_symbol()
                .map(|x| x.is_package(true))
                .unwrap_or(false)
            {
                // The given namespace does not point a package
                return false;
            }

            symbol_table::resolve((&path.base_path(0), namespace))
                .map(|symbol| {
                    // Generic parameter is not visible from other namespace.
                    !matches!(symbol.found.kind, SymbolKind::GenericParameter(_))
                        && symbol.found.namespace.matched(namespace)
                })
                .unwrap_or(false)
        }

        fn is_component_path(path: &GenericSymbolPath, namespace: &Namespace) -> bool {
            symbol_table::resolve((&path.base_path(0), namespace))
                .map(|x| x.found.is_component(true))
                .unwrap_or(false)
        }

        if let Some(file_path) = self.file_path() {
            if !namespace.matched(target_namespace) && is_defined_in_package(self, namespace) {
                // The given path is referened from the `target_namespace`
                // but it is not visible because it has no package paths.
                for i in 1..namespace.depth() {
                    let token = Token::generate(namespace.paths[i], file_path);
                    namespace_table::insert(token.id, file_path, namespace);

                    let symbol_path = GenericSymbol {
                        base: token,
                        arguments: vec![],
                    };
                    self.paths.insert(i - 1, symbol_path);
                }
            }

            if namespace.paths[0] != target_namespace.paths[0] && is_component_path(self, namespace)
            {
                // Append the project prefix to the path.
                let token = Token::generate(namespace.paths[0], file_path);
                namespace_table::insert(token.id, file_path, namespace);

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
        text.fmt(f)
    }
}
