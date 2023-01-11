use crate::namespace::Namespace;
use crate::symbol::{Symbol, SymbolKind};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use veryl_parser::resource_table::{PathId, StrId};
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

    pub fn as_slice(&self) -> &[StrId] {
        self.0.as_slice()
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

impl From<&syntax_tree::ScopedOrHierIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::ScopedOrHierIdentifier) -> Self {
        let mut path = Vec::new();
        path.push(value.identifier.identifier_token.token.text);
        match &*value.scoped_or_hier_identifier_group {
            syntax_tree::ScopedOrHierIdentifierGroup::ColonColonIdentifierScopedOrHierIdentifierGroupList(x) => {
                path.push(x.identifier.identifier_token.token.text);
                for x in &x.scoped_or_hier_identifier_group_list {
                    path.push(x.identifier.identifier_token.token.text);
                }
            },
            syntax_tree::ScopedOrHierIdentifierGroup::ScopedOrHierIdentifierGroupList0ScopedOrHierIdentifierGroupList1(x) => {
                for x in &x.scoped_or_hier_identifier_group_list1 {
                    path.push(x.identifier.identifier_token.token.text);
                }
            },
        }
        SymbolPath(path)
    }
}

#[derive(Clone, Default, Debug)]
pub struct SymbolTable {
    table: HashMap<StrId, Vec<Symbol>>,
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

    pub fn get(&self, path: &SymbolPath, namespace: &Namespace) -> Option<&Symbol> {
        let mut ret = None;
        let mut namespace = namespace.clone();
        for name in path.as_slice() {
            let mut max_depth = 0;
            ret = None;
            if let Some(symbols) = self.table.get(name) {
                for symbol in symbols {
                    if namespace.included(&symbol.namespace)
                        && symbol.namespace.depth() >= max_depth
                    {
                        ret = Some(symbol);
                        max_depth = symbol.namespace.depth();
                    }
                }

                if let Some(ret) = ret {
                    if let SymbolKind::Instance(ref x) = ret.kind {
                        namespace = Namespace::default();
                        namespace.push(x.type_name);
                    }
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
        ret
    }

    pub fn get_all(&self) -> Vec<Symbol> {
        let mut ret = Vec::new();
        for value in self.table.values() {
            let mut value = value.clone();
            ret.append(&mut value);
        }
        ret
    }

    pub fn dump(&self) -> String {
        format!("{}", self)
    }

    pub fn drop(&mut self, file_path: PathId) {
        for (_, symbols) in self.table.iter_mut() {
            symbols.retain(|x| x.token.file_path != file_path);
        }
    }
}

impl fmt::Display for SymbolTable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "SymbolTable [")?;
        let mut symbol_width = 0;
        let mut namespace_width = 0;
        let mut vec: Vec<_> = self.table.iter().collect();
        vec.sort_by(|x, y| x.0.cmp(y.0));
        for (k, v) in &vec {
            symbol_width = symbol_width.max(format!("{}", k).len());
            for symbol in *v {
                namespace_width = namespace_width.max(format!("{}", symbol.namespace).len());
            }
        }
        for (k, v) in &vec {
            for symbol in *v {
                writeln!(
                    f,
                    "    {:symbol_width$} @ {:namespace_width$}: {},",
                    k,
                    symbol.namespace,
                    symbol.kind,
                    symbol_width = symbol_width,
                    namespace_width = namespace_width
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

pub fn get(path: &SymbolPath, namespace: &Namespace) -> Option<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get(path, namespace).cloned())
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

#[cfg(test)]
mod tests {
    use crate::namespace::Namespace;
    use crate::symbol_table::SymbolPath;
    use crate::{symbol_table, Analyzer};
    use veryl_parser::{resource_table, Parser};

    const CODE: &str = r##"
    module ModuleA #(
        parameter paramA: u32 = 1,
    ) (
        portA: input logic [10],
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

        enum EnumA: logic [2] {
            memberA,
        }
    }
    "##;

    fn parse() {
        let parser = Parser::parse(&CODE, &"").unwrap();
        let mut analyzer = Analyzer::new(&CODE);
        analyzer.analyze(&parser.veryl);
    }

    #[test]
    fn module() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");
    }

    #[test]
    fn interface() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");
    }

    #[test]
    fn package() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("PackageA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "");
    }

    #[test]
    fn parameter() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("paramA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "InterfaceA");
    }

    #[test]
    fn localparam() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("paramB".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "InterfaceA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "PackageA");
    }

    #[test]
    fn port() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("portA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());
    }

    #[test]
    fn variable() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "InterfaceA");
    }

    #[test]
    fn r#struct() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("StructA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "PackageA");
    }

    #[test]
    fn struct_member() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_none());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        namespace.push(resource_table::get_str_id("StructA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(
            format!("{}", symbol.unwrap().namespace),
            "PackageA::StructA"
        );

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberB".to_string()).unwrap());
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::get(&symbol_path, &namespace);

        assert!(symbol.is_some());
        assert_eq!(format!("{}", symbol.unwrap().namespace), "ModuleA");
    }
}
