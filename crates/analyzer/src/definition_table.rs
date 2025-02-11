use std::cell::RefCell;
use std::collections::HashMap;
use veryl_parser::veryl_grammar_trait::{InterfaceDeclaration, ModuleDeclaration};

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
    Module {
        text: String,
        decl: ModuleDeclaration,
    },
    Interface {
        text: String,
        decl: InterfaceDeclaration,
    },
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

    // TODO drop for language server
}

thread_local!(static DEFINITION_TABLE: RefCell<DefinitionTable> = RefCell::new(DefinitionTable::default()));

pub fn insert(definition: Definition) -> DefinitionId {
    DEFINITION_TABLE.with(|f| f.borrow_mut().insert(definition))
}

pub fn get(id: DefinitionId) -> Option<Definition> {
    DEFINITION_TABLE.with(|f| f.borrow().get(id))
}
