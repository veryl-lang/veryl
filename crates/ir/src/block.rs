use crate::variable::{VarId, Variable};
use std::collections::HashMap;
use veryl_parser::resource_table::StrId;

pub struct Block {
    name: StrId,
    variables: HashMap<VarId, Variable>,
}
