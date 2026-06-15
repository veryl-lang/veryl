use crate::HashMap;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::Arc;
use veryl_parser::resource_table::PathId;
use veryl_parser::veryl_grammar_trait::{
    FunctionDeclaration, InterfaceDeclaration, ModuleDeclaration, ProtoFunctionDeclaration,
    ProtoModuleDeclaration,
};
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

/// Returns the last issued definition ID value. Used to delimit per-file ID
/// windows for fragment caching.
pub fn peek_definition_id() -> usize {
    DEFINITION_ID.with(|f| *f.borrow())
}

/// Reserves `count` consecutive definition IDs and returns the value the
/// counter had before the reservation. The reserved IDs are
/// `base+1..=base+count`.
pub fn reserve_definition_ids(count: usize) -> usize {
    DEFINITION_ID.with(|f| {
        let mut ret = f.borrow_mut();
        let base = *ret;
        *ret += count;
        base
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Definition {
    Module(ModuleDeclaration),
    Interface(InterfaceDeclaration),
    Function(FunctionDeclaration),
    ProtoFunction(ProtoFunctionDeclaration),
    ProtoModule(ProtoModuleDeclaration),
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
            Definition::Function(x) => {
                if let TokenSource::File { path, .. } = x.function.function_token.token.source {
                    Some(path)
                } else {
                    None
                }
            }
            Definition::ProtoFunction(x) => {
                if let TokenSource::File { path, .. } = x.function.function_token.token.source {
                    Some(path)
                } else {
                    None
                }
            }
            Definition::ProtoModule(x) => {
                if let TokenSource::File { path, .. } = x.module.module_token.token.source {
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
    table: HashMap<DefinitionId, Arc<Definition>>,
}

impl DefinitionTable {
    pub fn insert(&mut self, definition: Definition) -> DefinitionId {
        let id = new_definition_id();
        self.table.insert(id, Arc::new(definition));
        id
    }

    pub fn get(&self, id: DefinitionId) -> Option<Arc<Definition>> {
        self.table.get(&id).cloned()
    }

    pub fn insert_with_id(&mut self, id: DefinitionId, definition: Definition) {
        self.table.insert(id, Arc::new(definition));
    }

    pub fn export_in_window(&self, start: usize, end: usize) -> Vec<(DefinitionId, Definition)> {
        let mut ret: Vec<_> = self
            .table
            .iter()
            .filter(|(id, _)| id.0 > start && id.0 <= end)
            .map(|(id, definition)| (*id, (**definition).clone()))
            .collect();
        ret.sort_unstable_by_key(|(id, _)| *id);
        ret
    }

    pub fn drop(&mut self, path: PathId) {
        self.table.retain(|_, x| x.get_path() != Some(path));
    }
}

thread_local!(static DEFINITION_TABLE: RefCell<DefinitionTable> = RefCell::new(DefinitionTable::default()));

pub fn insert(definition: Definition) -> DefinitionId {
    DEFINITION_TABLE.with(|f| f.borrow_mut().insert(definition))
}

pub fn get(id: DefinitionId) -> Option<Arc<Definition>> {
    DEFINITION_TABLE.with(|f| f.borrow().get(id))
}

/// Inserts a definition under a caller-provided (reserved) ID without
/// allocating a new one. Used by fragment restore.
pub fn insert_with_id(id: DefinitionId, definition: Definition) {
    DEFINITION_TABLE.with(|f| f.borrow_mut().insert_with_id(id, definition))
}

/// Exports all definitions whose ID lies in the window `(start, end]`,
/// sorted by ID. Used by fragment caching.
pub fn export_in_window(start: usize, end: usize) -> Vec<(DefinitionId, Definition)> {
    DEFINITION_TABLE.with(|f| f.borrow().export_in_window(start, end))
}

pub fn drop(path: PathId) {
    DEFINITION_TABLE.with(|f| f.borrow_mut().drop(path))
}
