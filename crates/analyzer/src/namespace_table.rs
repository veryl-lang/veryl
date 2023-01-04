use crate::namespace::Namespace;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use veryl_parser::global_table::{PathId, TokenId};

#[derive(Clone, Default, Debug)]
pub struct NamespaceTable {
    table: HashMap<TokenId, (Namespace, PathId)>,
}

impl NamespaceTable {
    pub fn insert(&mut self, id: TokenId, file_path: PathId, namespace: &Namespace) {
        self.table.insert(id, (namespace.clone(), file_path));
    }

    pub fn get(&self, id: TokenId) -> Option<&Namespace> {
        self.table.get(&id).map(|(x, _)| x)
    }

    pub fn dump(&self) -> String {
        format!("{}", self)
    }

    pub fn drop(&mut self, file_path: PathId) {
        self.table.retain(|_, x| x.1 != file_path);
    }
}

impl fmt::Display for NamespaceTable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "NamespaceTable [")?;
        let mut id_witdh = 0;
        let mut namespace_width = 0;
        let mut vec: Vec<_> = self.table.iter().collect();
        vec.sort_by(|x, y| x.0.cmp(y.0));
        for (k, v) in &vec {
            id_witdh = id_witdh.max(format!("{}", k).len());
            namespace_width = namespace_width.max(format!("{}", v.0).len());
        }
        for (k, v) in &vec {
            writeln!(
                f,
                "    {:id_witdh$}: {:namespace_width$} @ {},",
                k,
                v.0,
                v.1,
                id_witdh = id_witdh,
                namespace_width = namespace_width,
            )?;
        }
        writeln!(f, "]")?;
        Ok(())
    }
}

thread_local!(static NAMESPACE_TABLE: RefCell<NamespaceTable> = RefCell::new(NamespaceTable::default()));

pub fn insert(id: TokenId, file_path: PathId, namespace: &Namespace) {
    NAMESPACE_TABLE.with(|f| f.borrow_mut().insert(id, file_path, namespace))
}

pub fn get(id: TokenId) -> Option<Namespace> {
    NAMESPACE_TABLE.with(|f| f.borrow().get(id).cloned())
}

pub fn dump() -> String {
    NAMESPACE_TABLE.with(|f| f.borrow().dump())
}

pub fn drop(file_path: PathId) {
    NAMESPACE_TABLE.with(|f| f.borrow_mut().drop(file_path))
}
