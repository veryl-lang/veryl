use crate::evaluator::Evaluated;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{Direction, Symbol, SymbolKind, TypeKind};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use veryl_parser::resource_table::{PathId, StrId, TokenId};
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::Token;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SymbolPath(Vec<StrId>);

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
        let mut path = Vec::new();
        path.push(value.identifier.identifier_token.token.text);
        for x in &value.scoped_identifier_list {
            path.push(x.identifier.identifier_token.token.text);
        }
        SymbolPath(path)
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let mut path = Vec::new();
        if let Some(ref x) = value.expression_identifier_opt {
            path.push(x.dollar.dollar_token.token.text);
        }
        path.push(value.identifier.identifier_token.token.text);
        match &*value.expression_identifier_group {
            syntax_tree::ExpressionIdentifierGroup::ColonColonIdentifierExpressionIdentifierGroupListExpressionIdentifierGroupList0(x) => {
                path.push(x.identifier.identifier_token.token.text);
                for x in &x.expression_identifier_group_list {
                    path.push(x.identifier.identifier_token.token.text);
                }
            },
            syntax_tree::ExpressionIdentifierGroup::ExpressionIdentifierGroupList1ExpressionIdentifierGroupList2(x) => {
                for x in &x.expression_identifier_group_list2 {
                    path.push(x.identifier.identifier_token.token.text);
                }
            },
        }
        SymbolPath(path)
    }
}

#[derive(Clone, Default)]
pub struct SymbolPathNamespace(pub SymbolPath, pub Namespace);

impl From<&Token> for SymbolPathNamespace {
    fn from(value: &Token) -> Self {
        let namespace = namespace_table::get(value.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
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
        let namespace = namespace_table::get(value.identifier.identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier.identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

#[derive(Clone, Debug)]
pub struct ResolveResult {
    pub found: Option<Symbol>,
    pub full_path: Vec<Symbol>,
}

#[derive(Clone, Debug)]
pub struct ResolveError {
    pub last_found: Symbol,
    pub not_found: StrId,
}

impl ResolveError {
    pub fn new(last_found: &Symbol, not_found: &StrId) -> Self {
        Self {
            last_found: last_found.clone(),
            not_found: *not_found,
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct SymbolTable {
    table: HashMap<StrId, Vec<Symbol>>,
    project_local_table: HashMap<StrId, HashMap<StrId, StrId>>,
}

impl SymbolTable {
    pub fn insert(&mut self, token: &Token, symbol: Symbol) -> bool {
        let entry = self.table.entry(token.text).or_default();
        for item in entry.iter() {
            if symbol.namespace == item.namespace {
                return false;
            }
        }
        entry.push(symbol);
        true
    }

    pub fn get(
        &self,
        path: &SymbolPath,
        namespace: &Namespace,
    ) -> Result<ResolveResult, ResolveError> {
        let mut ret = None;
        let mut last_found = None;
        let mut full_path = Vec::new();
        let mut namespace = namespace.clone();
        let mut inner = false;

        let mut path = path.clone();

        // replace project local name
        let prj = namespace.paths[0];
        let path_head = path.0[0];
        if let Some(map) = self.project_local_table.get(&prj) {
            if let Some(id) = map.get(&path_head) {
                path.0[0] = *id;
            }
        }

        for name in path.as_slice() {
            let mut max_depth = 0;
            ret = None;
            if let Some(symbols) = self.table.get(name) {
                for symbol in symbols {
                    let included = if inner {
                        namespace.matched(&symbol.namespace)
                    } else {
                        namespace.included(&symbol.namespace)
                    };
                    if included && symbol.namespace.depth() >= max_depth {
                        symbol.evaluate();
                        ret = Some(symbol);
                        last_found = Some(symbol);
                        max_depth = symbol.namespace.depth();
                    }
                }

                if let Some(ret) = ret {
                    full_path.push(ret.clone());
                    match &ret.kind {
                        SymbolKind::Variable(x) => {
                            if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                                let path = SymbolPath::new(x);
                                if let Ok(symbol) = self.get(&path, &namespace) {
                                    if let Some(found) = symbol.found {
                                        namespace = Namespace::new();
                                        for path in &found.namespace.paths {
                                            namespace.push(*path);
                                        }
                                        namespace.push(found.token.text);
                                        inner = true;
                                    }
                                }
                            }
                        }
                        SymbolKind::Interface(_) => {
                            namespace = Namespace::default();
                            namespace.push(ret.token.text);
                            inner = true;
                        }
                        SymbolKind::Package => {
                            namespace = Namespace::default();
                            namespace.push(ret.token.text);
                            inner = true;
                        }
                        SymbolKind::Enum(_) => {
                            namespace = ret.namespace.clone();
                            namespace.push(ret.token.text);
                            inner = true;
                        }
                        SymbolKind::Instance(ref x) => {
                            namespace = Namespace::default();
                            for x in &x.type_name {
                                namespace.push(*x);
                            }
                            inner = true;
                        }
                        SymbolKind::Port(ref x) if x.direction == Direction::Modport => {
                            if let Some(ref x) = x.r#type {
                                if let TypeKind::UserDefined(ref x) = x.kind {
                                    namespace = Namespace::default();
                                    for x in x {
                                        namespace.push(*x);
                                    }
                                    inner = true;
                                }
                            }
                        }
                        _ => (),
                    }
                } else if let Some(last_found) = last_found {
                    return Err(ResolveError::new(last_found, name));
                } else {
                    return Ok(ResolveResult {
                        found: None,
                        full_path,
                    });
                }
            } else if let Some(last_found) = last_found {
                return Err(ResolveError::new(last_found, name));
            } else {
                // If symbol is not found, the name is treated as namespace
                namespace = Namespace::new();
                namespace.push(*name);
                inner = true;
            }
        }
        Ok(ResolveResult {
            found: ret.cloned(),
            full_path,
        })
    }

    pub fn get_all(&self) -> Vec<Symbol> {
        let mut ret = Vec::new();
        for value in self.table.values() {
            for symbol in value {
                symbol.evaluate();
            }
            let mut value = value.clone();
            ret.append(&mut value);
        }
        ret
    }

    pub fn dump(&self) -> String {
        for value in self.table.values() {
            for symbol in value {
                symbol.evaluate();
            }
        }
        format!("{self}")
    }

    pub fn drop(&mut self, file_path: PathId) {
        for (_, symbols) in self.table.iter_mut() {
            symbols.retain(|x| x.token.file_path != file_path);
            for symbol in symbols.iter_mut() {
                symbol.references.retain(|x| x.file_path != file_path);
            }
        }
    }

    pub fn add_reference(&mut self, target: TokenId, token: &Token) {
        for (_, symbols) in self.table.iter_mut() {
            for symbol in symbols.iter_mut() {
                if symbol.token.id == target {
                    symbol.references.push(token.to_owned());
                }
            }
        }
    }

    pub fn add_project_local(&mut self, prj: StrId, from: StrId, to: StrId) {
        self.project_local_table
            .entry(prj)
            .and_modify(|x| {
                x.insert(from, to);
            })
            .or_insert(HashMap::from([(from, to)]));
    }

    pub fn get_project_local(&self, prj: StrId) -> Option<HashMap<StrId, StrId>> {
        self.project_local_table.get(&prj).cloned()
    }
}

impl fmt::Display for SymbolTable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "SymbolTable [")?;
        let mut symbol_width = 0;
        let mut namespace_width = 0;
        let mut reference_width = 0;
        let mut vec: Vec<_> = self.table.iter().collect();
        vec.sort_by(|x, y| x.0.cmp(y.0));
        for (k, v) in &vec {
            symbol_width = symbol_width.max(format!("{k}").len());
            for symbol in *v {
                namespace_width = namespace_width.max(format!("{}", symbol.namespace).len());
                reference_width = reference_width.max(format!("{}", symbol.references.len()).len());
            }
        }
        for (k, v) in &vec {
            for symbol in *v {
                let evaluated = if let Some(evaluated) = symbol.evaluated.get() {
                    match evaluated {
                        Evaluated::Unknown => "".to_string(),
                        _ => format!(" ( {evaluated:?} )"),
                    }
                } else {
                    "".to_string()
                };
                writeln!(
                    f,
                    "    {:symbol_width$} @ {:namespace_width$} {{ refs: {:reference_width$} }}: {}{},",
                    k,
                    symbol.namespace,
                    symbol.references.len(),
                    symbol.kind,
                    evaluated,
                    symbol_width = symbol_width,
                    namespace_width = namespace_width,
                    reference_width = reference_width
                )?;
            }
        }
        writeln!(f, "]")?;
        Ok(())
    }
}

thread_local!(static SYMBOL_TABLE: RefCell<SymbolTable> = RefCell::new(SymbolTable::default()));

pub fn insert(token: &Token, symbol: Symbol) -> bool {
    SYMBOL_TABLE.with(|f| f.borrow_mut().insert(token, symbol))
}

pub fn get(path: &SymbolPath, namespace: &Namespace) -> Result<ResolveResult, ResolveError> {
    SYMBOL_TABLE.with(|f| f.borrow().get(path, namespace))
}

pub fn resolve<T: Into<SymbolPathNamespace>>(path: T) -> Result<ResolveResult, ResolveError> {
    let SymbolPathNamespace(path, namespace) = path.into();
    SYMBOL_TABLE.with(|f| f.borrow().get(&path, &namespace))
}

pub fn get_all() -> Vec<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get_all())
}

pub fn dump() -> String {
    SYMBOL_TABLE.with(|f| f.borrow().dump())
}

pub fn drop(file_path: PathId) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().drop(file_path))
}

pub fn add_reference(target: TokenId, token: &Token) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_reference(target, token))
}

pub fn add_project_local(prj: StrId, from: StrId, to: StrId) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_project_local(prj, from, to))
}

pub fn get_project_local(prj: StrId) -> Option<HashMap<StrId, StrId>> {
    SYMBOL_TABLE.with(|f| f.borrow().get_project_local(prj))
}

#[cfg(test)]
mod tests {
    use crate::namespace::Namespace;
    use crate::symbol_table::SymbolPath;
    use crate::{symbol_table, Analyzer};
    use veryl_metadata::Metadata;
    use veryl_parser::{resource_table, Parser};

    const CODE: &str = r##"
    module ModuleA #(
        parameter paramA: u32 = 1,
    ) (
        portA: input logic<10>,
    ) {
        localparam paramB: u32 = 1;

        var memberA: logic;
        var memberB: PackageA::StructA;
    }

    interface InterfaceA #(
        parameter paramA: u32 = 1,
    ) {
        localparam paramB: u32 = 1;

        var memberA: logic;

        modport modportA {
            memberA: input,
        }
    }

    package PackageA {
        localparam paramB: u32 = 1;

        struct StructA {
            memberA: logic,
        }

        enum EnumA: logic<2> {
            memberA,
        }
    }
    "##;

    fn parse() {
        let metadata: Metadata = toml::from_str(&Metadata::create_default_toml("")).unwrap();
        let parser = Parser::parse(&CODE, &"").unwrap();
        let analyzer = Analyzer::new(&metadata);
        analyzer.analyze_pass1(&"prj", &CODE, &"", &parser.veryl);
    }

    #[test]
    fn module() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");
    }

    #[test]
    fn interface() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");
    }

    #[test]
    fn package() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("PackageA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj");
    }

    #[test]
    fn parameter() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("paramA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::InterfaceA");
    }

    #[test]
    fn localparam() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("paramB".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::InterfaceA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::PackageA");
    }

    #[test]
    fn port() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("portA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());
    }

    #[test]
    fn variable() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::InterfaceA");
    }

    #[test]
    fn r#struct() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("StructA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "prj::PackageA");
    }

    #[test]
    fn struct_member() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        namespace.push(resource_table::get_str_id("StructA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(
            format!("{}", symbol.unwrap().namespace),
            "prj::PackageA::StructA"
        );

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberB".to_string()).unwrap());
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace).unwrap().found;

        assert!(symbol.is_some());
        assert_eq!(
            format!("{}", symbol.unwrap().namespace),
            "prj::PackageA::StructA"
        );

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberB".to_string()).unwrap());
        symbol_path.push(resource_table::get_str_id("memberB".to_string()).unwrap());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_err());
    }
}
