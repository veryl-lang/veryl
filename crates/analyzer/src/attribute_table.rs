use crate::attribute::{AlignItem, Attribute, ExpandItem, FormatItem, IfdefCondition};
use crate::range_table::RangeTable;
use std::cell::{Cell, RefCell};
use veryl_parser::resource_table::PathId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::Token;

thread_local!(static ATTRIBUTE_TABLE: RefCell<RangeTable<Attribute>> = RefCell::new(RangeTable::default()));

// Lets the emitter skip its `always_ff` scan in a project that uses none.
// `drop` leaves it set: a stale `true` only costs a scan that finds nothing.
thread_local!(static NON_PORTABLE_ALLOW: Cell<bool> = const { Cell::new(false) });

fn note_non_portable(value: &Attribute) {
    if let Attribute::Allow(x) = value
        && x.is_non_portable()
    {
        NON_PORTABLE_ALLOW.set(true);
    }
}

pub fn has_non_portable_allow() -> bool {
    NON_PORTABLE_ALLOW.get()
}

pub fn insert(range: TokenRange, value: Attribute) {
    note_non_portable(&value);
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().insert(range, value))
}

pub fn begin(token: Token, value: Option<Attribute>) {
    if let Some(x) = &value {
        note_non_portable(x);
    }
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().begin(token, value))
}

pub fn end(token: Token) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().end(token))
}

pub fn get(token: &Token) -> Vec<Attribute> {
    ATTRIBUTE_TABLE.with(|f| f.borrow().get(token))
}

pub fn ifdef_conditions(token: &Token) -> Vec<IfdefCondition> {
    get(token)
        .iter()
        .flat_map(|x| x.ifdef_conditions())
        .collect()
}

pub fn is_align(token: &Token, item: AlignItem) -> bool {
    let attrs = ATTRIBUTE_TABLE.with(|f| f.borrow().get(token));
    attrs.iter().any(|x| x.is_align(item))
}

pub fn is_format(token: &Token, item: FormatItem) -> bool {
    let attrs = ATTRIBUTE_TABLE.with(|f| f.borrow().get(token));
    attrs.iter().any(|x| x.is_format(item))
}

pub fn is_expand(token: &Token, item: ExpandItem) -> bool {
    let attrs = ATTRIBUTE_TABLE.with(|f| f.borrow().get(token));
    attrs.iter().any(|x| x.is_expand(item))
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

/// Exports all entries belonging to one file. Used by fragment caching.
pub fn export_by_path(path: PathId) -> Vec<(TokenRange, Attribute)> {
    ATTRIBUTE_TABLE.with(|f| f.borrow().export_by_path(path))
}

pub fn clear() {
    NON_PORTABLE_ALLOW.set(false);
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().clear())
}

pub fn drop(path: PathId) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().drop(path))
}
