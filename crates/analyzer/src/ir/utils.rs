use crate::attribute::{AllowItem, Attribute, CondTypeItem};
use crate::attribute_table;
use veryl_parser::token_range::TokenRange;

pub fn has_cond_type(token: &TokenRange) -> bool {
    let mut attrs = attribute_table::get(&token.beg);
    attrs.reverse();
    for attr in attrs {
        match attr {
            Attribute::CondType(CondTypeItem::None) => return false,
            Attribute::CondType(_) => return true,
            _ => (),
        }
    }
    false
}

pub fn allow_missing_reset_statement(token: &TokenRange) -> bool {
    attribute_table::contains(
        &token.beg,
        Attribute::Allow(AllowItem::MissingResetStatement),
    )
}
