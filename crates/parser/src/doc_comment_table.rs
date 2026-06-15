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

    pub fn export_by_path(&self, path: PathId) -> Vec<(u32, StrId)> {
        let mut ret: Vec<_> = self
            .table
            .iter()
            .filter(|((p, _), _)| *p == path)
            .map(|((_, line), text)| (*line, *text))
            .collect();
        ret.sort_unstable_by_key(|(line, _)| *line);
        ret
    }

    pub fn clear(&mut self) {
        self.table.clear();
    }
}

thread_local!(static DOC_COMMENT_TABLE: RefCell<DocCommentTable> = RefCell::new(DocCommentTable::default()));

pub fn insert(path: PathId, line: u32, text: StrId) {
    DOC_COMMENT_TABLE.with(|f| f.borrow_mut().insert(path, line, text))
}

pub fn get(path: PathId, line: u32) -> Option<StrId> {
    DOC_COMMENT_TABLE.with(|f| f.borrow().get(path, line))
}

/// Exports all doc comments belonging to one file, sorted by line.
/// Used by fragment caching.
pub fn export_by_path(path: PathId) -> Vec<(u32, StrId)> {
    DOC_COMMENT_TABLE.with(|f| f.borrow().export_by_path(path))
}

pub fn clear() {
    DOC_COMMENT_TABLE.with(|f| f.borrow_mut().clear())
}
