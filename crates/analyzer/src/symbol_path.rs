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
        let namespace = namespace_table::get(value.id).unwrap_or_default();
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
pub enum FixedTypeKind {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Bool,
    String,
}

impl fmt::Display for FixedTypeKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Bool => "bool",
            Self::String => "string",
        };
        text.fmt(f)
    }
}

impl FixedTypeKind {
    pub fn to_sv_string(&self) -> String {
        let text = match self {
            Self::U8 => "byte unsigned",
            Self::U16 => "shortint unsigned",
            Self::U32 => "int unsigned",
            Self::U64 => "longint unsigned",
            Self::I8 => "byte signed",
            Self::I16 => "shortint signed",
            Self::I32 => "int signed",
            Self::I64 => "longint signed",
            Self::F32 => "shortreal",
            Self::F64 => "real",
            Self::Bool => "logic",
            Self::String => "string",
        };
        text.to_string()
    }
}

impl From<&syntax_tree::U8> for FixedTypeKind {
    fn from(_value: &syntax_tree::U8) -> Self {
        Self::U8
    }
}

impl From<&syntax_tree::U16> for FixedTypeKind {
    fn from(_value: &syntax_tree::U16) -> Self {
        Self::U16
    }
}

impl From<&syntax_tree::U32> for FixedTypeKind {
    fn from(_value: &syntax_tree::U32) -> Self {
        Self::U32
    }
}

impl From<&syntax_tree::U64> for FixedTypeKind {
    fn from(_value: &syntax_tree::U64) -> Self {
        Self::U64
    }
}

impl From<&syntax_tree::I8> for FixedTypeKind {
    fn from(_value: &syntax_tree::I8) -> Self {
        Self::I8
    }
}

impl From<&syntax_tree::I16> for FixedTypeKind {
    fn from(_value: &syntax_tree::I16) -> Self {
        Self::I16
    }
}

impl From<&syntax_tree::I32> for FixedTypeKind {
    fn from(_value: &syntax_tree::I32) -> Self {
        Self::I32
    }
}

impl From<&syntax_tree::I64> for FixedTypeKind {
    fn from(_value: &syntax_tree::I64) -> Self {
        Self::I64
    }
}

impl From<&syntax_tree::F32> for FixedTypeKind {
    fn from(_value: &syntax_tree::F32) -> Self {
        Self::F32
    }
}

impl From<&syntax_tree::F64> for FixedTypeKind {
    fn from(_value: &syntax_tree::F64) -> Self {
        Self::F64
    }
}

impl From<&syntax_tree::Bool> for FixedTypeKind {
    fn from(_value: &syntax_tree::Bool) -> Self {
        Self::Bool
    }
}

impl From<&syntax_tree::Strin> for FixedTypeKind {
    fn from(_value: &syntax_tree::Strin) -> Self {
        Self::String
    }
}

impl From<&syntax_tree::FixedType> for FixedTypeKind {
    fn from(value: &syntax_tree::FixedType) -> Self {
        match value {
            syntax_tree::FixedType::U8(x) => x.u8.as_ref().into(),
            syntax_tree::FixedType::U16(x) => x.u16.as_ref().into(),
            syntax_tree::FixedType::U32(x) => x.u32.as_ref().into(),
            syntax_tree::FixedType::U64(x) => x.u64.as_ref().into(),
            syntax_tree::FixedType::I8(x) => x.i8.as_ref().into(),
            syntax_tree::FixedType::I16(x) => x.i16.as_ref().into(),
            syntax_tree::FixedType::I32(x) => x.i32.as_ref().into(),
            syntax_tree::FixedType::I64(x) => x.i64.as_ref().into(),
            syntax_tree::FixedType::F32(x) => x.f32.as_ref().into(),
            syntax_tree::FixedType::F64(x) => x.f64.as_ref().into(),
            syntax_tree::FixedType::Bool(x) => x.bool.as_ref().into(),
            syntax_tree::FixedType::Strin(x) => x.strin.as_ref().into(),
        }
    }
}

#[derive(Copy, Debug, Clone, PartialEq, Eq)]
pub enum GenericSymbolPathKind {
    Identifier,
    FixedType(FixedTypeKind),
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
            GenericSymbolPathKind::FixedType(x) => x.to_string(),
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

    pub fn unaliased_path(&self) -> Option<GenericSymbolPath> {
        if !self.is_resolvable() {
            return None;
        }

        let mut ret = GenericSymbolPath {
            paths: Vec::new(),
            kind: self.kind,
            range: self.range,
        };

        let namespace = namespace_table::get(self.paths[0].base.id)?;
        let mut generic_maps: Vec<_> = Vec::new();

        for (i, path_item) in self.paths.iter().enumerate() {
            ret.paths.push(path_item.clone());
            let symbol = symbol_table::resolve((&ret.generic_path(), &namespace));

            if let Ok(ref symbol) = symbol
                && let Some(mut alias_target) = symbol.found.alias_target()
            {
                alias_target.apply_map(&generic_maps);
                alias_target.resolve_imported(&namespace, Some(&generic_maps));
                if (i + 1) < self.paths.len() {
                    for j in (i + 1)..self.paths.len() {
                        alias_target.paths.push(self.paths[j].clone());
                    }
                }
                return alias_target.unaliased_path();
            }

            if let Some(path) = ret.paths.last_mut() {
                for arg in path.arguments.iter_mut() {
                    if let Some(x) = arg.unaliased_path() {
                        *arg = x;
                    }
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

        Some(ret)
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
        if let Ok(symbol) = symbol_table::resolve(&head.base)
            && matches!(symbol.found.kind, SymbolKind::GenericParameter(_))
        {
            return true;
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
            if self.len() > 1
                && matches!(
                    symbol.found.kind,
                    SymbolKind::EnumMember(_)
                        | SymbolKind::StructMember(_)
                        | SymbolKind::UnionMember(_)
                )
            {
                // The parent enum declaration is imported but not the enum member.
                // Therefore, we need to execute `resolve_imported` to the parent enum declaration.
                // see:
                // https://github.com/veryl-lang/veryl/issues/1721#issuecomment-2986758880
                //
                // This is also applied for struct/union member.
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

                if let Ok(symbol) = symbol_table::resolve((&self.generic_path(), &self_namespace))
                    && let Some(_) = symbol.found.get_parent_package()
                    && let Some(import) = symbol
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

                    if let Some(path) = package_path.unaliased_path() {
                        self.append_package_path(&path, self_file_path, &self_namespace);
                    } else {
                        self.append_package_path(&package_path, self_file_path, &self_namespace);
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

    fn append_package_path(
        &mut self,
        package_path: &GenericSymbolPath,
        file_path: PathId,
        self_namespace: &Namespace,
    ) {
        for (i, path) in package_path.paths.iter().enumerate() {
            let token = Token::generate(path.base.text, file_path);
            namespace_table::insert(token.id, file_path, self_namespace);

            self.paths.insert(i, path.clone());
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
        let token: TokenRange = value.into();
        let kind: FixedTypeKind = value.into();
        GenericSymbolPath {
            paths: vec![GenericSymbol {
                base: token.beg,
                arguments: Vec::new(),
            }],
            kind: GenericSymbolPathKind::FixedType(kind),
            range: token,
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
                    let list: Vec<syntax_tree::WithGenericArgumentItem> =
                        x.with_generic_argument_list.as_ref().into();
                    for x in &list {
                        let arg: GenericSymbolPath = x.into();
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
                let list: Vec<syntax_tree::WithGenericArgumentItem> =
                    x.with_generic_argument_list.as_ref().into();
                for x in &list {
                    let arg: GenericSymbolPath = x.into();
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
        for path in &self.paths {
            text.push_str(&format!("{} ", path.mangled()));
        }
        text.fmt(f)
    }
}
