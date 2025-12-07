use crate::HashMap;
use crate::literal::Literal;
use std::cell::RefCell;
use veryl_parser::resource_table::TokenId;

thread_local!(static LITERAL_TABLE: RefCell<HashMap<TokenId, Literal>> = RefCell::new(HashMap::default()));

pub fn insert(id: TokenId, value: Literal) {
    LITERAL_TABLE.with(|f| f.borrow_mut().insert(id, value));
}

pub fn get(id: &TokenId) -> Option<Literal> {
    LITERAL_TABLE.with(|f| f.borrow().get(id).cloned())
}
