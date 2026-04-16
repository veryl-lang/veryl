//! Inferred declaration types keyed by the declaration identifier
//! `TokenId`. Populated only for declarations without a type
//! annotation; the emitter falls back to AST walking otherwise.

use crate::HashMap;
use crate::ir;
use std::cell::RefCell;
use veryl_parser::resource_table::TokenId;

thread_local!(static RESOLVED_TYPE_TABLE: RefCell<HashMap<TokenId, ir::Type>> = RefCell::new(HashMap::default()));

pub fn insert(id: TokenId, value: ir::Type) {
    RESOLVED_TYPE_TABLE.with(|f| f.borrow_mut().insert(id, value));
}

pub fn get(id: &TokenId) -> Option<ir::Type> {
    RESOLVED_TYPE_TABLE.with(|f| f.borrow().get(id).cloned())
}

pub fn clear() {
    RESOLVED_TYPE_TABLE.with(|f| f.borrow_mut().clear());
}
