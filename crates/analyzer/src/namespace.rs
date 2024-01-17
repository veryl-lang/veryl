use crate::namespace_table;
use crate::symbol_table::SymbolPath;
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Debug, Clone, PartialEq)]
pub struct Namespace {
    pub paths: Vec<StrId>,
}

impl Namespace {
    pub fn new() -> Self {
        Self { paths: Vec::new() }
    }

    pub fn push(&mut self, path: StrId) {
        self.paths.push(path);
    }

    pub fn pop(&mut self) {
        self.paths.pop();
    }

    pub fn depth(&self) -> usize {
        self.paths.len()
    }

    pub fn included(&self, x: &Namespace) -> bool {
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

    pub fn matched(&self, x: &Namespace) -> bool {
        if self.paths.len() != x.paths.len() {
            false
        } else {
            self.included(x)
        }
    }
}

impl Default for Namespace {
    fn default() -> Self {
        namespace_table::get_default()
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        if let Some(first) = self.paths.first() {
            text.push_str(&format!("{first}"));
            for path in &self.paths[1..] {
                text.push_str(&format!("::{path}"));
            }
        }
        text.fmt(f)
    }
}

impl From<&SymbolPath> for Namespace {
    fn from(value: &SymbolPath) -> Self {
        let mut paths = Vec::new();
        for x in value.as_slice() {
            paths.push(x.clone());
        }
        Namespace { paths }
    }
}
