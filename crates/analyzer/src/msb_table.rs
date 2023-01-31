use std::cell::RefCell;
use std::collections::HashMap;
use veryl_parser::resource_table::TokenId;
use veryl_parser::veryl_grammar_trait::Expression;

#[derive(Clone, Debug)]
pub struct MsbTable {
    table: HashMap<TokenId, Expression>,
}

impl MsbTable {
    pub fn insert(&mut self, id: TokenId, expression: &Expression) {
        self.table.insert(id, expression.clone());
    }

    pub fn get(&self, id: TokenId) -> Option<&Expression> {
        self.table.get(&id)
    }
}

impl Default for MsbTable {
    fn default() -> Self {
        Self {
            table: HashMap::new(),
        }
    }
}

thread_local!(static MSB_TABLE: RefCell<MsbTable> = RefCell::new(MsbTable::default()));

pub fn insert(id: TokenId, expression: &Expression) {
    MSB_TABLE.with(|f| f.borrow_mut().insert(id, expression))
}

pub fn get(id: TokenId) -> Option<Expression> {
    MSB_TABLE.with(|f| f.borrow().get(id).cloned())
}
