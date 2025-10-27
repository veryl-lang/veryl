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

pub fn calc_index(index: &[usize], array: &[usize]) -> Option<usize> {
    if array.is_empty() || (array.len() == 1 && array[0] == 1 && index.is_empty()) {
        Some(0)
    } else if index.len() != array.len() {
        None
    } else {
        let mut ret = 0;
        let mut base = 1;
        for (i, x) in array.iter().enumerate().rev() {
            ret += index[i] * base;
            base *= x;
        }
        Some(ret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_index() {
        assert_eq!(calc_index(&[0, 0, 0], &[2, 3, 4]), Some(0));
        assert_eq!(calc_index(&[0, 0, 1], &[2, 3, 4]), Some(1));
        assert_eq!(calc_index(&[0, 0, 2], &[2, 3, 4]), Some(2));
        assert_eq!(calc_index(&[0, 0, 3], &[2, 3, 4]), Some(3));
        assert_eq!(calc_index(&[0, 1, 0], &[2, 3, 4]), Some(4));
        assert_eq!(calc_index(&[0, 1, 1], &[2, 3, 4]), Some(5));
        assert_eq!(calc_index(&[0, 1, 2], &[2, 3, 4]), Some(6));
        assert_eq!(calc_index(&[0, 1, 3], &[2, 3, 4]), Some(7));
        assert_eq!(calc_index(&[0, 2, 0], &[2, 3, 4]), Some(8));
        assert_eq!(calc_index(&[0, 2, 1], &[2, 3, 4]), Some(9));
        assert_eq!(calc_index(&[0, 2, 2], &[2, 3, 4]), Some(10));
        assert_eq!(calc_index(&[0, 2, 3], &[2, 3, 4]), Some(11));
        assert_eq!(calc_index(&[1, 0, 0], &[2, 3, 4]), Some(12));
        assert_eq!(calc_index(&[1, 0, 1], &[2, 3, 4]), Some(13));
        assert_eq!(calc_index(&[1, 0, 2], &[2, 3, 4]), Some(14));
        assert_eq!(calc_index(&[1, 0, 3], &[2, 3, 4]), Some(15));
        assert_eq!(calc_index(&[1, 1, 0], &[2, 3, 4]), Some(16));
        assert_eq!(calc_index(&[1, 1, 1], &[2, 3, 4]), Some(17));
        assert_eq!(calc_index(&[1, 1, 2], &[2, 3, 4]), Some(18));
        assert_eq!(calc_index(&[1, 1, 3], &[2, 3, 4]), Some(19));
        assert_eq!(calc_index(&[1, 2, 0], &[2, 3, 4]), Some(20));
        assert_eq!(calc_index(&[1, 2, 1], &[2, 3, 4]), Some(21));
        assert_eq!(calc_index(&[1, 2, 2], &[2, 3, 4]), Some(22));
        assert_eq!(calc_index(&[1, 2, 3], &[2, 3, 4]), Some(23));
    }
}
