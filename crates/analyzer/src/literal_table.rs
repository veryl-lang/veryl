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

/// Exports all literals whose token ID lies in the window `(start, end]`,
/// sorted by token ID. Used by fragment caching (the table has no per-file
/// index, so the per-file ID window delimits one file's entries).
pub fn export_in_window(start: usize, end: usize) -> Vec<(TokenId, Literal)> {
    LITERAL_TABLE.with(|f| {
        let mut ret: Vec<_> = f
            .borrow()
            .iter()
            .filter(|(id, _)| id.0 > start && id.0 <= end)
            .map(|(id, literal)| (*id, literal.clone()))
            .collect();
        ret.sort_unstable_by_key(|(id, _)| *id);
        ret
    })
}
