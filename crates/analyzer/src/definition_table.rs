use std::cell::RefCell;
use std::collections::HashMap;
use veryl_parser::resource_table::PathId;
use veryl_parser::veryl_grammar_trait::{InterfaceDeclaration, ModuleDeclaration};
use veryl_parser::veryl_token::TokenSource;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinitionId(pub usize);

thread_local!(static DEFINITION_ID: RefCell<usize> = const { RefCell::new(0) });

pub fn new_definition_id() -> DefinitionId {
    DEFINITION_ID.with(|f| {
        let mut ret = f.borrow_mut();
        *ret += 1;
        DefinitionId(*ret)
    })
}

#[derive(Clone, Debug)]
pub enum Definition {
    Module(ModuleDeclaration),
    Interface(InterfaceDeclaration),
}

impl Definition {
    fn get_path(&self) -> Option<PathId> {
        match self {
            Definition::Module(x) => {
                if let TokenSource::File { path, .. } = x.module.module_token.token.source {
                    Some(path)
                } else {
                    None
                }
            }
            Definition::Interface(x) => {
                if let TokenSource::File { path, .. } = x.interface.interface_token.token.source {
                    Some(path)
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct DefinitionTable {
    table: HashMap<DefinitionId, Definition>,
}

impl DefinitionTable {
    pub fn insert(&mut self, definition: Definition) -> DefinitionId {
        let id = new_definition_id();
        self.table.insert(id, definition);
        id
    }

    pub fn get(&self, id: DefinitionId) -> Option<Definition> {
        self.table.get(&id).cloned()
    }

    pub fn drop(&mut self, path: PathId) {
        self.table.retain(|_, x| x.get_path() == Some(path));
    }
}

thread_local!(static DEFINITION_TABLE: RefCell<DefinitionTable> = RefCell::new(DefinitionTable::default()));

pub fn insert(definition: Definition) -> DefinitionId {
    DEFINITION_TABLE.with(|f| f.borrow_mut().insert(definition))
}

pub fn get(id: DefinitionId) -> Option<Definition> {
    DEFINITION_TABLE.with(|f| f.borrow().get(id))
}

pub fn drop(path: PathId) {
    DEFINITION_TABLE.with(|f| f.borrow_mut().drop(path))
}
