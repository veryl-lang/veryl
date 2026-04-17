use crate::HashMap;
use crate::namespace::Namespace;
use std::cell::RefCell;
use std::fmt;
use veryl_parser::resource_table::{PathId, StrId, TokenId};

#[derive(Clone, Debug)]
pub struct NamespaceTable {
    default: Namespace,
    project_names: Vec<StrId>,
    root_project: Option<StrId>,
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
        format!("{self}")
    }

    pub fn drop(&mut self, file_path: PathId) {
        self.table.retain(|_, x| x.1 != file_path);
    }

    pub fn set_project(&mut self, project_name: StrId, is_root: bool) {
        if !self.project_names.contains(&project_name) {
            self.project_names.push(project_name);
            if is_root {
                self.root_project = Some(project_name);
            }
        }
        self.set_default(&[project_name]);
    }

    pub fn match_project_name(&self, name: StrId) -> bool {
        self.project_names.contains(&name)
    }

    pub fn root_project_name(&self) -> StrId {
        self.root_project.unwrap()
    }

    pub fn set_default(&mut self, id: &[StrId]) {
        let mut namespace = Namespace::new();
        for id in id {
            namespace.push(*id);
        }
        self.default = namespace;
    }

    pub fn get_default(&self) -> Namespace {
        self.default.clone()
    }

    pub fn clear(&mut self) {
        self.project_names.clear();
        self.root_project = None;
        self.table.clear()
    }
}

impl Default for NamespaceTable {
    fn default() -> Self {
        Self {
            default: Namespace::new(),
            project_names: vec![],
            root_project: None,
            table: HashMap::default(),
        }
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
            id_witdh = id_witdh.max(format!("{k}").len());
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

pub fn set_project(project_name: StrId, is_root: bool) {
    NAMESPACE_TABLE.with(|f| f.borrow_mut().set_project(project_name, is_root))
}

pub fn match_project_name(name: StrId) -> bool {
    NAMESPACE_TABLE.with(|f| f.borrow().match_project_name(name))
}

pub fn root_project_name() -> StrId {
    NAMESPACE_TABLE.with(|f| f.borrow().root_project_name())
}

pub fn set_default(id: &[StrId]) {
    NAMESPACE_TABLE.with(|f| f.borrow_mut().set_default(id))
}

pub fn get_default() -> Namespace {
    NAMESPACE_TABLE.with(|f| f.borrow().get_default())
}

pub fn clear() {
    NAMESPACE_TABLE.with(|f| f.borrow_mut().clear())
}
