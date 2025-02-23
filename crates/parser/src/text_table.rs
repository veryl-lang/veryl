use crate::resource_table::PathId;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextId(pub usize);

thread_local!(static TEXT_ID: RefCell<usize> = const { RefCell::new(0) });

pub fn new_text_id() -> TextId {
    TEXT_ID.with(|f| {
        let mut ret = f.borrow_mut();
        *ret += 1;
        TextId(*ret)
    })
}

#[derive(Clone, Debug)]
pub struct TextInfo {
    pub text: String,
    pub path: PathId,
}

#[derive(Clone, Default, Debug)]
pub struct TextTable {
    current_text: TextId,
    table: HashMap<TextId, TextInfo>,
}

impl TextTable {
    pub fn set_current_text(&mut self, info: TextInfo) -> TextId {
        let id = new_text_id();
        self.table.insert(id, info);
        self.current_text = id;
        id
    }

    pub fn get_current_text(&self) -> TextId {
        self.current_text
    }

    pub fn get(&self, id: TextId) -> Option<TextInfo> {
        self.table.get(&id).cloned()
    }

    // TODO drop for language server
}

thread_local!(static TEXT_TABLE: RefCell<TextTable> = RefCell::new(TextTable::default()));

pub fn set_current_text(info: TextInfo) -> TextId {
    TEXT_TABLE.with(|f| f.borrow_mut().set_current_text(info))
}

pub fn get_current_text() -> TextId {
    TEXT_TABLE.with(|f| f.borrow().get_current_text())
}

pub fn get(id: TextId) -> Option<TextInfo> {
    TEXT_TABLE.with(|f| f.borrow().get(id))
}
