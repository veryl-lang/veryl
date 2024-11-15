use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{DocComment, GenericInstanceProperty, GenericMap, Symbol, SymbolKind};
use crate::symbol_table;
use std::cmp::Ordering;
use std::fmt;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::{Token, TokenRange, TokenSource};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SymbolPath(pub Vec<StrId>);

impl SymbolPath {
    pub fn new(x: &[StrId]) -> Self {
        Self(x.to_vec())
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

    pub fn to_vec(self) -> Vec<StrId> {
        self.0
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
        let mut path = Vec::new();
        for x in value {
            path.push(x.text);
        }
        SymbolPath(path)
    }
}

impl From<&Token> for SymbolPath {
    fn from(value: &Token) -> Self {
        let path = vec![value.text];
        SymbolPath(path)
    }
}

impl From<&syntax_tree::Identifier> for SymbolPath {
    fn from(value: &syntax_tree::Identifier) -> Self {
        let path = vec![value.identifier_token.token.text];
        SymbolPath(path)
    }
}

impl From<&[syntax_tree::Identifier]> for SymbolPath {
    fn from(value: &[syntax_tree::Identifier]) -> Self {
        let mut path = Vec::new();
        for x in value {
            path.push(x.identifier_token.token.text);
        }
        SymbolPath(path)
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        let mut path = Vec::new();
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
        path.mangled_path()
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

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct SymbolPathNamespace(pub SymbolPath, pub Namespace);

impl From<&Token> for SymbolPathNamespace {
    fn from(value: &Token) -> Self {
        let namespace = namespace_table::get(value.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
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

impl From<(&Vec<StrId>, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&Vec<StrId>, &Namespace)) -> Self {
        let path = SymbolPath::new(value.0);
        SymbolPathNamespace(path, value.1.clone())
    }
}

impl From<&syntax_tree::Identifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::Identifier) -> Self {
        let namespace = namespace_table::get(value.identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&[syntax_tree::Identifier]> for SymbolPathNamespace {
    fn from(value: &[syntax_tree::Identifier]) -> Self {
        let namespace = namespace_table::get(value[0].identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier.identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::ScopedIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ScopedIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier().token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier().token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

#[derive(Copy, Debug, Clone, PartialEq, Eq)]
pub enum GenericSymbolPathKind {
    Identifier,
    IntegerBased,
    IntegerBaseLess,
    IntegerAllBit,
    RealExponent,
    RealFixedPoint,
}

impl fmt::Display for GenericSymbolPathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            GenericSymbolPathKind::Identifier => "identifier".to_string(),
            GenericSymbolPathKind::IntegerBased => "integer based".to_string(),
            GenericSymbolPathKind::IntegerBaseLess => "integer base less".to_string(),
            GenericSymbolPathKind::IntegerAllBit => "integer all bit".to_string(),
            GenericSymbolPathKind::RealExponent => "real exponent".to_string(),
            GenericSymbolPathKind::RealFixedPoint => "read fixed point".to_string(),
        };
        text.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericSymbolPath {
    pub paths: Vec<GenericSymbol>,
    pub kind: GenericSymbolPathKind,
    pub range: TokenRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
                if let Ok(symbol) = symbol_table::resolve(&head.base) {
                    if matches!(symbol.found.kind, SymbolKind::GenericParameter(_)) {
                        return self.base();
                    }
                }
            }

            let mut text = format!("__{}", self.base);
            for a in &self.arguments {
                text.push('_');
                for a in a.mangled_path().0.as_slice() {
                    text.push_str(&format!("_{}", a));
                }
            }
            resource_table::insert_str(&text)
        }
    }

    pub fn get_generic_instance(&self, base: &Symbol) -> Option<(Token, Symbol)> {
        if self.arguments.is_empty() {
            None
        } else {
            let property = GenericInstanceProperty {
                base: base.id,
                arguments: self.arguments.clone(),
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

    pub fn mangled_path(&self) -> SymbolPath {
        let path: Vec<_> = self.paths.iter().map(|x| x.mangled()).collect();
        SymbolPath::new(&path)
    }

    pub fn generic_path(&self) -> SymbolPath {
        let path: Vec<_> = self.paths.iter().map(|x| x.base()).collect();
        SymbolPath::new(&path)
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
            if matches!(symbol.found.kind, SymbolKind::GenericParameter(_)) {
                return true;
            }
        }

        // path contains generic parameter as generic argument
        for path in &self.paths {
            for arg in &path.arguments {
                if !arg.is_resolvable() {
                    continue;
                }
                let head = &arg.paths[0];
                if let Ok(symbol) = symbol_table::resolve(&head.base) {
                    if matches!(symbol.found.kind, SymbolKind::GenericParameter(_)) {
                        return true;
                    }
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
                for map in maps.iter().rev() {
                    let head = &arg.paths[0];
                    if let Some(x) = map.map.get(&head.base()) {
                        let mut paths: Vec<_> = arg.paths.drain(1..).collect();
                        arg.paths.clone_from(&x.paths);
                        arg.paths.append(&mut paths);
                        arg.kind = x.kind;
                        arg.range = x.range;
                        break;
                    }
                }
            }
        }
    }

    /// Resolve and expand path if the path is imported at declaration
    pub fn resolve_imported(&mut self, namespace: &Namespace) {
        if !self.is_resolvable() {
            return;
        }
        if let Ok(symbol) = symbol_table::resolve((&self.generic_path(), namespace)) {
            if symbol.imported {
                let self_namespace = namespace_table::get(self.range.beg.id).unwrap();
                let TokenSource::File(self_file_path) = self.range.beg.source else {
                    return;
                };
                if let Ok(symbol) = symbol_table::resolve((&self.generic_path(), &self_namespace)) {
                    let mut parent = symbol.found.namespace.clone();
                    parent.strip_prefix(&namespace_table::get_default());

                    if parent.depth() == 0 {
                        return;
                    }

                    // If symbol belongs Package, it can be expanded
                    if let Ok(parent_symbol) =
                        symbol_table::resolve((&parent.paths, &self_namespace))
                    {
                        if matches!(parent_symbol.found.kind, SymbolKind::Package(_)) {
                            for (i, path) in parent.paths.iter().enumerate() {
                                let token = Token::generate(*path);
                                namespace_table::insert(token.id, self_file_path, &self_namespace);
                                let generic_symbol = GenericSymbol {
                                    base: token,
                                    arguments: vec![],
                                };
                                self.paths.insert(i, generic_symbol);
                            }
                        }
                    }
                }
            }
        }
        for path in &mut self.paths {
            for arg in &mut path.arguments {
                arg.resolve_imported(namespace);
            }
        }
    }
}

impl From<&syntax_tree::Number> for GenericSymbolPath {
    fn from(value: &syntax_tree::Number) -> Self {
        let (token, kind) = match value {
            syntax_tree::Number::IntegralNumber(x) => match x.integral_number.as_ref() {
                syntax_tree::IntegralNumber::Based(x) => (
                    x.based.based_token.token,
                    GenericSymbolPathKind::IntegerBased,
                ),
                syntax_tree::IntegralNumber::AllBit(x) => (
                    x.all_bit.all_bit_token.token,
                    GenericSymbolPathKind::IntegerAllBit,
                ),
                syntax_tree::IntegralNumber::BaseLess(x) => (
                    x.base_less.base_less_token.token,
                    GenericSymbolPathKind::IntegerBaseLess,
                ),
            },
            syntax_tree::Number::RealNumber(x) => match x.real_number.as_ref() {
                syntax_tree::RealNumber::Exponent(x) => (
                    x.exponent.exponent_token.token,
                    GenericSymbolPathKind::RealExponent,
                ),
                syntax_tree::RealNumber::FixedPoint(x) => (
                    x.fixed_point.fixed_point_token.token,
                    GenericSymbolPathKind::RealFixedPoint,
                ),
            },
        };

        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: token,
                arguments: Vec::new(),
            }],
            kind,
            range: token.into(),
        }
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

                if let Some(ref x) = x.scoped_identifier_opt {
                    if let Some(ref x) = x.with_generic_argument.with_generic_argument_opt {
                        let list: Vec<syntax_tree::WithGenericArgumentItem> =
                            x.with_generic_argument_list.as_ref().into();
                        for x in &list {
                            match x {
                                syntax_tree::WithGenericArgumentItem::ScopedIdentifier(x) => {
                                    arguments.push(x.scoped_identifier.as_ref().into());
                                }
                                syntax_tree::WithGenericArgumentItem::Number(x) => {
                                    arguments.push(x.number.as_ref().into());
                                }
                            }
                        }
                    }
                }

                paths.push(GenericSymbol { base, arguments });
            }
        }

        for x in &value.scoped_identifier_list {
            let base = x.identifier.identifier_token.token;
            let mut arguments = Vec::new();

            if let Some(ref x) = x.scoped_identifier_opt0 {
                if let Some(ref x) = x.with_generic_argument.with_generic_argument_opt {
                    let list: Vec<syntax_tree::WithGenericArgumentItem> =
                        x.with_generic_argument_list.as_ref().into();
                    for x in &list {
                        match x {
                            syntax_tree::WithGenericArgumentItem::ScopedIdentifier(x) => {
                                arguments.push(x.scoped_identifier.as_ref().into());
                            }
                            syntax_tree::WithGenericArgumentItem::Number(x) => {
                                arguments.push(x.number.as_ref().into());
                            }
                        }
                    }
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

impl fmt::Display for GenericSymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        for path in &self.paths {
            text.push_str(&format!("{} ", path.mangled()));
        }
        text.fmt(f)
    }
}
