use crate::ir::{Value, VarPath};
use std::fmt;

#[derive(Clone)]
pub struct Variable {
    pub path: VarPath,
    pub width: usize,
    pub value: Vec<Value>,
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (i, value) in self.value.iter().enumerate() {
            ret.push_str(&format!("{}[{}] = 'h{:x};\n", self.path, i, value));
        }

        ret.trim_end().fmt(f)
    }
}
