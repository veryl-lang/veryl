use crate::resource_table::TokenId;
use crate::veryl_token::Token;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Default, Debug)]
pub struct TokenTable {
    table: HashMap<TokenId, Token>,
}

impl TokenTable {
    pub fn insert(&mut self, id: TokenId, token: Token) {
        self.table.insert(id, token);
    }

    pub fn get(&self, id: TokenId) -> Option<Token> {
        self.table.get(&id).cloned()
    }
}

thread_local!(static TOKEN_TABLE: RefCell<TokenTable> = RefCell::new(TokenTable::default()));

pub fn insert(id: TokenId, token: Token) {
    TOKEN_TABLE.with(|f| f.borrow_mut().insert(id, token))
}

pub fn get(id: TokenId) -> Token {
    TOKEN_TABLE.with(|f| f.borrow().get(id)).unwrap_or_default()
}
