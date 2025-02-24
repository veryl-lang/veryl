use crate::attribute::Attribute;
use crate::range_table::RangeTable;
use std::cell::RefCell;
use veryl_parser::resource_table::PathId;
use veryl_parser::veryl_token::{Token, TokenRange};

thread_local!(static ATTRIBUTE_TABLE: RefCell<RangeTable<Attribute>> = RefCell::new(RangeTable::default()));

pub fn insert(range: TokenRange, value: Attribute) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().insert(range, value))
}

pub fn begin(token: Token, value: Option<Attribute>) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().begin(token, value))
}

pub fn end(token: Token) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().end(token))
}

pub fn get(token: &Token) -> Vec<Attribute> {
    ATTRIBUTE_TABLE.with(|f| f.borrow().get(token))
}

pub fn contains(token: &Token, value: Attribute) -> bool {
    ATTRIBUTE_TABLE.with(|f| f.borrow().contains(token, &value))
}

pub fn dump() -> String {
    ATTRIBUTE_TABLE.with(|f| format!("AttributeTable {}", f.borrow().dump()))
}

pub fn get_all() -> Vec<(TokenRange, Attribute)> {
    ATTRIBUTE_TABLE.with(|f| f.borrow().get_all())
}

pub fn clear() {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().clear())
}

pub fn drop(path: PathId) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().drop(path))
}
