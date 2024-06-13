use crate::r#unsafe::Unsafe;
use crate::range_table::RangeTable;
use std::cell::RefCell;
use veryl_parser::veryl_token::{Token, TokenRange};

thread_local!(static UNSAFE_TABLE: RefCell<RangeTable<Unsafe>> = RefCell::new(RangeTable::default()));

pub fn insert(range: TokenRange, value: Unsafe) {
    UNSAFE_TABLE.with(|f| f.borrow_mut().insert(range, value))
}

pub fn begin(token: Token, value: Option<Unsafe>) {
    UNSAFE_TABLE.with(|f| f.borrow_mut().begin(token, value))
}

pub fn end(token: Token) {
    UNSAFE_TABLE.with(|f| f.borrow_mut().end(token))
}

pub fn get(token: &Token) -> Vec<Unsafe> {
    UNSAFE_TABLE.with(|f| f.borrow().get(token))
}

pub fn contains(token: &Token, value: Unsafe) -> bool {
    UNSAFE_TABLE.with(|f| f.borrow().contains(token, &value))
}

pub fn dump() -> String {
    UNSAFE_TABLE.with(|f| format!("UnsafeTable {}", f.borrow().dump()))
}

pub fn get_all() -> Vec<(TokenRange, Unsafe)> {
    UNSAFE_TABLE.with(|f| f.borrow().get_all())
}

pub fn clear() {
    UNSAFE_TABLE.with(|f| f.borrow_mut().clear())
}
