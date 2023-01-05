use crate::namespace::Namespace;
use crate::symbol::{Symbol, SymbolKind};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use veryl_parser::resource_table::{PathId, StrId};
use veryl_parser::veryl_token::Token;

#[derive(Debug, Clone, PartialEq)]
pub enum Name {
    Hierarchical(Vec<StrId>),
    Scoped(Vec<StrId>),
}

impl Name {
    pub fn as_slice(&self) -> &[StrId] {
        match self {
            Name::Hierarchical(x) => x.as_slice(),
            Name::Scoped(x) => x.as_slice(),
        }
    }
}

impl Default for Name {
    fn default() -> Self {
        Name::Hierarchical(vec![])
    }
}

impl From<&veryl_parser::veryl_grammar_trait::HierarchicalIdentifier> for Name {
    fn from(value: &veryl_parser::veryl_grammar_trait::HierarchicalIdentifier) -> Self {
        let mut paths = Vec::new();
        paths.push(value.identifier.identifier_token.token.text);
        for x in &value.hierarchical_identifier_list0 {
            paths.push(x.identifier.identifier_token.token.text);
        }
        Name::Hierarchical(paths)
    }
}

impl From<&veryl_parser::veryl_grammar_trait::ScopedIdentifier> for Name {
    fn from(value: &veryl_parser::veryl_grammar_trait::ScopedIdentifier) -> Self {
        let mut paths = Vec::new();
        paths.push(value.identifier.identifier_token.token.text);
        for x in &value.scoped_identifier_list {
            paths.push(x.identifier.identifier_token.token.text);
        }
        Name::Scoped(paths)
    }
}

impl From<&veryl_parser::veryl_grammar_trait::ScopedOrHierIdentifier> for Name {
    fn from(value: &veryl_parser::veryl_grammar_trait::ScopedOrHierIdentifier) -> Self {
        let mut paths = Vec::new();
        paths.push(value.identifier.identifier_token.token.text);
        match &*value.scoped_or_hier_identifier_group {
            veryl_parser::veryl_grammar_trait::ScopedOrHierIdentifierGroup::ColonColonIdentifierScopedOrHierIdentifierGroupList(x) => {
                paths.push(x.identifier.identifier_token.token.text);
                for x in &x.scoped_or_hier_identifier_group_list {
                    paths.push(x.identifier.identifier_token.token.text);
                }
                Name::Scoped(paths)
            },
            veryl_parser::veryl_grammar_trait::ScopedOrHierIdentifierGroup::ScopedOrHierIdentifierGroupList0ScopedOrHierIdentifierGroupList1(x) => {
                for x in &x.scoped_or_hier_identifier_group_list1 {
                    paths.push(x.identifier.identifier_token.token.text);
                }
                Name::Hierarchical(paths)
            },
        }
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

    pub fn get(&self, name: &Name, namespace: &Namespace) -> Option<&Symbol> {
        match name {
            Name::Hierarchical(x) => self.get_hierarchical(x, namespace),
            Name::Scoped(_) => todo!(),
        }
    }

    fn get_hierarchical(&self, paths: &[StrId], namespace: &Namespace) -> Option<&Symbol> {
        let mut ret = None;
        let mut namespace = namespace.clone();
        for name in paths {
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
                    if let SymbolKind::Instance { ref type_name } = ret.kind {
                        namespace = Namespace::default();
                        namespace.push(*type_name);
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

pub fn get(name: &Name, namespace: &Namespace) -> Option<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get(name, namespace).cloned())
}

pub fn dump() -> String {
    SYMBOL_TABLE.with(|f| f.borrow().dump())
}

pub fn drop(file_path: PathId) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().drop(file_path))
}
