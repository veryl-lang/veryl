use std::cell::RefCell;
use std::fmt;
use veryl_parser::resource_table::{self, StrId};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Unsafe {
    Cdc,
}

impl fmt::Display for Unsafe {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            Unsafe::Cdc => "cdc".to_string(),
        };
        text.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub enum UnsafeError {
    UnknownUnsafe,
}

struct Pattern {
    pub cdc: StrId,
}

impl Pattern {
    fn new() -> Self {
        Self {
            cdc: resource_table::insert_str("cdc"),
        }
    }
}

thread_local!(static PAT: RefCell<Pattern> = RefCell::new(Pattern::new()));

impl TryFrom<&veryl_parser::veryl_grammar_trait::UnsafeBlock> for Unsafe {
    type Error = UnsafeError;

    fn try_from(
        value: &veryl_parser::veryl_grammar_trait::UnsafeBlock,
    ) -> Result<Self, Self::Error> {
        PAT.with_borrow(|pat| match value.identifier.identifier_token.token.text {
            x if x == pat.cdc => Ok(Unsafe::Cdc),
            _ => Err(UnsafeError::UnknownUnsafe),
        })
    }
}
