use bimap::BiMap;
use std::cell::RefCell;
use std::hash::Hash;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct GlobalTable<T, U>
where
    T: Hash + Eq,
    U: Hash + Eq,
{
    table: BiMap<T, U>,
    last: U,
}

impl<T, U> GlobalTable<T, U>
where
    T: Hash + Eq,
    U: Hash + Eq + Copy + Incrementable,
{
    pub fn insert(&mut self, value: T) -> U {
        if let Some(id) = self.table.get_by_left(&value) {
            *id
        } else {
            let id = self.last;
            self.table.insert(value, id);
            self.last.inc();
            id
        }
    }

    pub fn get_value(&self, id: U) -> Option<&T> {
        self.table.get_by_right(&id)
    }

    pub fn get_id<V: AsRef<T>>(&self, value: V) -> Option<U> {
        self.table.get_by_left(value.as_ref()).copied()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StrId(usize);
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathId(usize);
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenId(usize);

pub trait Incrementable {
    fn inc(&mut self);
}

impl Incrementable for StrId {
    fn inc(&mut self) {
        self.0 += 1;
    }
}

impl Incrementable for PathId {
    fn inc(&mut self) {
        self.0 += 1;
    }
}

thread_local!(static STRING_TABLE: RefCell<GlobalTable<String, StrId>> = RefCell::new(GlobalTable::default()));
thread_local!(static PATHBUF_TABLE: RefCell<GlobalTable<PathBuf, PathId>> = RefCell::new(GlobalTable::default()));
thread_local!(static TOKEN_ID: RefCell<usize> = RefCell::new(0));

pub fn insert_str(value: &str) -> StrId {
    STRING_TABLE.with(|f| f.borrow_mut().insert(value.to_owned()))
}

pub fn insert_path(value: &Path) -> PathId {
    PATHBUF_TABLE.with(|f| f.borrow_mut().insert(value.to_owned()))
}

pub fn get_str_value(id: StrId) -> Option<String> {
    STRING_TABLE.with(|f| f.borrow().get_value(id).map(|x| x.to_owned()))
}

pub fn get_path_value(id: PathId) -> Option<PathBuf> {
    PATHBUF_TABLE.with(|f| f.borrow().get_value(id).map(|x| x.to_owned()))
}

pub fn get_str_id<T: AsRef<String>>(value: T) -> Option<StrId> {
    STRING_TABLE.with(|f| f.borrow().get_id(value).map(|x| x.to_owned()))
}

pub fn get_path_id<T: AsRef<PathBuf>>(value: T) -> Option<PathId> {
    PATHBUF_TABLE.with(|f| f.borrow().get_id(value).map(|x| x.to_owned()))
}

pub fn get_token_id() -> TokenId {
    TOKEN_ID.with(|f| {
        let mut ret = f.borrow_mut();
        *ret += 1;
        TokenId(*ret)
    })
}
