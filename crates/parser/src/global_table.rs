use bimap::BiMap;
use std::cell::RefCell;
use std::hash::Hash;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct GlobalTable<T: Hash + Eq> {
    table: BiMap<T, usize>,
    last: usize,
}

impl<T: Hash + Eq> GlobalTable<T> {
    pub fn insert(&mut self, value: T) -> usize {
        if let Some(id) = self.table.get_by_left(&value) {
            *id
        } else {
            let id = self.last;
            self.table.insert(value, id);
            self.last += 1;
            id
        }
    }

    pub fn get_value(&self, id: usize) -> Option<&T> {
        self.table.get_by_right(&id)
    }

    pub fn get_id<U: AsRef<T>>(&self, value: U) -> Option<usize> {
        self.table.get_by_left(value.as_ref()).copied()
    }
}

thread_local!(static STRING_TABLE: RefCell<GlobalTable<String>> = RefCell::new(GlobalTable::default()));
thread_local!(static PATHBUF_TABLE: RefCell<GlobalTable<PathBuf>> = RefCell::new(GlobalTable::default()));

pub fn insert_str(value: &str) -> usize {
    STRING_TABLE.with(|f| f.borrow_mut().insert(value.to_owned()))
}

pub fn insert_path(value: &Path) -> usize {
    PATHBUF_TABLE.with(|f| f.borrow_mut().insert(value.to_owned()))
}

pub fn get_str_value(id: usize) -> Option<String> {
    STRING_TABLE.with(|f| f.borrow().get_value(id).map(|x| x.to_owned()))
}

pub fn get_path_value(id: usize) -> Option<PathBuf> {
    PATHBUF_TABLE.with(|f| f.borrow().get_value(id).map(|x| x.to_owned()))
}

pub fn get_str_id<T: AsRef<String>>(value: T) -> Option<usize> {
    STRING_TABLE.with(|f| f.borrow().get_id(value).map(|x| x.to_owned()))
}

pub fn get_path_id<T: AsRef<PathBuf>>(value: T) -> Option<usize> {
    PATHBUF_TABLE.with(|f| f.borrow().get_id(value).map(|x| x.to_owned()))
}
