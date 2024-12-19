use std::cell::RefCell;
use std::collections::HashMap;
use veryl_parser::resource_table::TokenId;

#[derive(Clone, Default, Debug)]
pub struct MsbTable {
    table: HashMap<TokenId, usize>,
}

impl MsbTable {
    pub fn insert(&mut self, id: TokenId, dimension_number: usize) {
        self.table.insert(id, dimension_number);
    }

    pub fn get(&self, id: TokenId) -> Option<&usize> {
        self.table.get(&id)
    }

    pub fn clear(&mut self) {
        self.table.clear()
    }
}

thread_local!(static MSB_TABLE: RefCell<MsbTable> = RefCell::new(MsbTable::default()));

pub fn insert(id: TokenId, dimension_number: usize) {
    MSB_TABLE.with(|f| f.borrow_mut().insert(id, dimension_number))
}

pub fn get(id: TokenId) -> Option<usize> {
    MSB_TABLE.with(|f| f.borrow().get(id).cloned())
}

pub fn clear() {
    MSB_TABLE.with(|f| f.borrow_mut().clear())
}
