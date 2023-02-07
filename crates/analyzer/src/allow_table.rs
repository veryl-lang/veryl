use std::cell::RefCell;
use veryl_parser::resource_table::{self, StrId};

#[derive(Clone, Debug, Default)]
pub struct AllowTable {
    table: Vec<StrId>,
}

impl AllowTable {
    pub fn push(&mut self, id: StrId) {
        self.table.push(id);
    }

    pub fn pop(&mut self) {
        self.table.pop();
    }

    pub fn contains(&self, id: StrId) -> bool {
        self.table.contains(&id)
    }
}

thread_local!(static ALLOW_TABLE: RefCell<AllowTable> = RefCell::new(AllowTable::default()));

pub fn push(id: StrId) {
    ALLOW_TABLE.with(|f| f.borrow_mut().push(id))
}

pub fn pop() {
    ALLOW_TABLE.with(|f| f.borrow_mut().pop())
}

pub fn contains(text: &str) -> bool {
    if let Some(id) = resource_table::get_str_id(text.to_string()) {
        ALLOW_TABLE.with(|f| f.borrow().contains(id))
    } else {
        false
    }
}
