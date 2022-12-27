use std::collections::HashMap;
use std::path::PathBuf;
use veryl_parser::ParolLocation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Variable,
    Module,
    Interface,
    Function,
    Parameter,
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct NameSpace {
    pub paths: Vec<String>,
}

impl NameSpace {
    pub fn push(&mut self, path: &str) {
        self.paths.push(path.to_owned());
    }

    pub fn pop(&mut self) {
        self.paths.pop();
    }

    pub fn depth(&self) -> usize {
        self.paths.len()
    }

    pub fn included(&self, x: &NameSpace) -> bool {
        for (i, x) in x.paths.iter().enumerate() {
            if let Some(path) = self.paths.get(i) {
                if path != x {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct Location {
    pub line: usize,
    pub column: usize,
    pub file_name: PathBuf,
}

impl From<&ParolLocation> for Location {
    fn from(x: &ParolLocation) -> Self {
        Self {
            line: x.line,
            column: x.column,
            file_name: PathBuf::from(&*x.file_name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub name_space: NameSpace,
    pub location: Location,
}

impl Symbol {
    pub fn new(name: &str, kind: SymbolKind, name_space: &NameSpace, location: &Location) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            name_space: name_space.to_owned(),
            location: location.to_owned(),
        }
    }
}

#[derive(Default, Debug)]
pub struct SymbolTable {
    table: HashMap<String, Vec<Symbol>>,
}

impl SymbolTable {
    pub fn insert(&mut self, name: &str, symbol: Symbol) -> bool {
        let entry = self.table.entry(name.to_owned()).or_default();
        for item in entry.iter() {
            if symbol.name_space == item.name_space {
                return false;
            }
        }
        entry.push(symbol);
        true
    }

    pub fn get(&self, name: &str, kind: SymbolKind, name_space: &NameSpace) -> Option<&Symbol> {
        let mut ret = None;
        let mut max_depth = 0;
        if let Some(symbols) = self.table.get(name) {
            for symbol in symbols {
                if name_space.included(&symbol.name_space) && symbol.name_space.depth() > max_depth
                {
                    ret = Some(symbol);
                    max_depth = symbol.name_space.depth();
                }
            }
        }
        ret
    }
}
