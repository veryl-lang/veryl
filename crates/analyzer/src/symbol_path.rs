use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{DocComment, GenericInstanceProperty, Symbol, SymbolKind};
use crate::symbol_table;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::{Token, TokenRange};

#[derive(Debug, Default, Clone, PartialEq)]
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

#[derive(Clone, Default, Debug)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericSymbolPath {
    pub paths: Vec<GenericSymbol>,
    pub resolvable: bool,
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
        if !self.resolvable {
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
                if !arg.resolvable {
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

    pub fn apply_map(&mut self, map: &HashMap<StrId, GenericSymbolPath>) {
        let head = &self.paths[0];
        if let Some(x) = map.get(&head.base()) {
            let mut paths: Vec<_> = self.paths.drain(1..).collect();
            self.paths.clone_from(&x.paths);
            self.paths.append(&mut paths);
            self.resolvable = x.resolvable;
        }

        for path in &mut self.paths {
            for arg in &mut path.arguments {
                let head = &arg.paths[0];
                if let Some(x) = map.get(&head.base()) {
                    let mut paths: Vec<_> = arg.paths.drain(1..).collect();
                    arg.paths.clone_from(&x.paths);
                    arg.paths.append(&mut paths);
                }
            }
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
            resolvable: false,
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
            resolvable: true,
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
