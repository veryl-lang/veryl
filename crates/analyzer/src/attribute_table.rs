use crate::attribute::{AlignItem, Attribute, ExpandItem, FormatItem};
use crate::range_table::RangeTable;
use std::cell::RefCell;
use veryl_parser::resource_table::PathId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::Token;

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

pub fn get(token: &TokenRange) -> Vec<Attribute> {
    ATTRIBUTE_TABLE.with(|f| f.borrow().get(token))
}

pub fn is_align(token: &TokenRange, item: AlignItem) -> bool {
    let attrs = ATTRIBUTE_TABLE.with(|f| f.borrow().get(token));
    attrs.iter().any(|x| x.is_align(item))
}

pub fn is_format(token: &TokenRange, item: FormatItem) -> bool {
    let attrs = ATTRIBUTE_TABLE.with(|f| f.borrow().get(token));
    attrs.iter().any(|x| x.is_format(item))
}

pub fn is_expand(token: &TokenRange, item: ExpandItem) -> bool {
    let attrs = ATTRIBUTE_TABLE.with(|f| f.borrow().get(token));
    attrs.iter().any(|x| x.is_expand(item))
}

pub fn contains(token: &TokenRange, value: Attribute) -> bool {
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
