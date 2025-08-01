use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{DocComment, GenericInstanceProperty, GenericMap, Symbol, SymbolKind};
use crate::symbol_table;
use crate::{SVec, svec};
use std::cmp::Ordering;
use std::fmt;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::{Token, TokenSource};

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

impl From<(&SVec<StrId>, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&SVec<StrId>, &Namespace)) -> Self {
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

impl From<&GenericSymbolPathNamesapce> for SymbolPathNamespace {
    fn from(value: &GenericSymbolPathNamesapce) -> Self {
        SymbolPathNamespace(value.0.generic_path(), value.1.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericSymbolPathNamesapce(pub GenericSymbolPath, pub Namespace);

impl From<(&GenericSymbolPath, &Namespace)> for GenericSymbolPathNamesapce {
    fn from(value: (&GenericSymbolPath, &Namespace)) -> Self {
        let (path, namespace) = value;
        GenericSymbolPathNamesapce(path.clone(), namespace.clone())
    }
}

#[derive(Copy, Debug, Clone, PartialEq, Eq)]
pub enum GenericSymbolPathKind {
    Identifier,
    FixedType,
    IntegerBased,
    IntegerBaseLess,
    IntegerAllBit,
    RealExponent,
    RealFixedPoint,
    Boolean(bool),
}

impl fmt::Display for GenericSymbolPathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            GenericSymbolPathKind::Identifier => "identifier".to_string(),
            GenericSymbolPathKind::FixedType => "fixed type".to_string(),
            GenericSymbolPathKind::IntegerBased => "integer based".to_string(),
            GenericSymbolPathKind::IntegerBaseLess => "integer base less".to_string(),
            GenericSymbolPathKind::IntegerAllBit => "integer all bit".to_string(),
            GenericSymbolPathKind::RealExponent => "real exponent".to_string(),
            GenericSymbolPathKind::RealFixedPoint => "read fixed point".to_string(),
            GenericSymbolPathKind::Boolean(x) => format!("{x}"),
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
                    text.push_str(&format!("_{a}"));
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

    pub fn replace_generic_argument(&mut self, index: usize, value: GenericSymbolPath) {
        if index < self.arguments.len() {
            self.arguments[index] = value;
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

    pub fn generic_arguments(&self) -> Vec<Vec<GenericSymbolPath>> {
        let path: Vec<_> = self.paths.iter().map(|x| x.arguments.clone()).collect();
        path
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

    pub fn may_be_generic_reference(&self) -> bool {
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

        for path in &self.paths {
            if path.arguments.iter().any(|a| a.is_resolvable()) {
                return true;
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
        if let Ok(symbol) = symbol_table::resolve((&self.generic_path(), namespace)) {
            if matches!(symbol.found.kind, SymbolKind::EnumMember(_)) {
                // The parent enum declaration is imported but not the enum member.
                // Therefore, we need to execute `resolve_imported` to the parent enum declaration.
                // see:
                // https://github.com/veryl-lang/veryl/issues/1721#issuecomment-2986758880
                let member_path = self.paths.pop().unwrap();
                if namespace.matched(&symbol.found.namespace) {
                    // For case that the given namespace is matched with the enum declaration
                    let mut namespace = namespace.clone();
                    namespace.pop();
                    self.resolve_imported(&namespace, generic_maps);
                } else {
                    self.resolve_imported(namespace, generic_maps);
                }
                self.paths.push(member_path);
            } else if symbol.imported {
                let self_namespace = namespace_table::get(self.range.beg.id).unwrap();
                let TokenSource::File {
                    path: self_file_path,
                    ..
                } = self.range.beg.source
                else {
                    return;
                };

                if let Ok(symbol) = symbol_table::resolve((&self.generic_path(), &self_namespace)) {
                    if let Some(parent) = symbol.found.get_parent_package() {
                        let package_symbol = if parent.is_package(false) {
                            parent
                        } else if let Some(maps) = generic_maps {
                            // replace proto package with actual package
                            let mut package_path = symbol
                                .found
                                .imported
                                .iter()
                                .find(|(_, x)| namespace.included(x))
                                .map(|(x, _)| x)
                                .unwrap()
                                .clone();
                            package_path.apply_map(maps);
                            symbol_table::resolve((&package_path.mangled_path(), namespace))
                                .map(|x| x.found)
                                .unwrap()
                        } else {
                            parent
                        };

                        // If symbol belongs Package, it can be expanded
                        let mut package_namespace = package_symbol.inner_namespace();
                        package_namespace.strip_prefix(&namespace_table::get_default());

                        for (i, path) in package_namespace.paths.iter().enumerate() {
                            let token = Token::generate(*path, self_file_path);
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
        for path in &mut self.paths {
            for arg in &mut path.arguments {
                arg.resolve_imported(namespace, generic_maps);
            }
        }
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
        let token = match value {
            syntax_tree::FixedType::U8(x) => x.u8.u8_token.token,
            syntax_tree::FixedType::U16(x) => x.u16.u16_token.token,
            syntax_tree::FixedType::U32(x) => x.u32.u32_token.token,
            syntax_tree::FixedType::U64(x) => x.u64.u64_token.token,
            syntax_tree::FixedType::I8(x) => x.i8.i8_token.token,
            syntax_tree::FixedType::I16(x) => x.i16.i16_token.token,
            syntax_tree::FixedType::I32(x) => x.i32.i32_token.token,
            syntax_tree::FixedType::I64(x) => x.i64.i64_token.token,
            syntax_tree::FixedType::F32(x) => x.f32.f32_token.token,
            syntax_tree::FixedType::F64(x) => x.f64.f64_token.token,
            syntax_tree::FixedType::Bool(x) => x.bool.bool_token.token,
            syntax_tree::FixedType::Strin(x) => x.strin.string_token.token,
        };

        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: token,
                arguments: Vec::new(),
            }],
            kind: GenericSymbolPathKind::FixedType,
            range: token.into(),
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

impl From<&syntax_tree::BooleanLiteral> for GenericSymbolPath {
    fn from(value: &syntax_tree::BooleanLiteral) -> Self {
        let (token, value) = match value {
            syntax_tree::BooleanLiteral::True(x) => (x.r#true.true_token.token, true),
            syntax_tree::BooleanLiteral::False(x) => (x.r#false.false_token.token, false),
        };
        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: token,
                arguments: Vec::new(),
            }],
            kind: GenericSymbolPathKind::Boolean(value),
            range: token.into(),
        }
    }
}

impl From<&syntax_tree::WithGenericArgumentItem> for GenericSymbolPath {
    fn from(value: &syntax_tree::WithGenericArgumentItem) -> Self {
        match value {
            syntax_tree::WithGenericArgumentItem::ExpressionIdentifier(x) => {
                x.expression_identifier.as_ref().into()
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
                            let arg: GenericSymbolPath = x.into();
                            arguments.push(arg);
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
                        let arg: GenericSymbolPath = x.into();
                        arguments.push(arg);
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

impl fmt::Display for GenericSymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        for path in &self.paths {
            text.push_str(&format!("{} ", path.mangled()));
        }
        text.fmt(f)
    }
}
