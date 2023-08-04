use crate::resource_table::{PathId, StrId};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct DocCommentTable {
    table: HashMap<(PathId, u32), StrId>,
}

impl DocCommentTable {
    pub fn insert(&mut self, path: PathId, line: u32, text: StrId) {
        self.table.insert((path, line), text);
    }

    pub fn get(&self, path: PathId, line: u32) -> Option<StrId> {
        self.table.get(&(path, line)).cloned()
    }
}

thread_local!(static DOC_COMMENT_TABLE: RefCell<DocCommentTable> = RefCell::new(DocCommentTable::default()));

pub fn insert(path: PathId, line: u32, text: StrId) {
    DOC_COMMENT_TABLE.with(|f| f.borrow_mut().insert(path, line, text))
}

pub fn get(path: PathId, line: u32) -> Option<StrId> {
    DOC_COMMENT_TABLE.with(|f| f.borrow().get(path, line))
}
