use crate::ir::{Value, VarPath};
use std::fmt;

#[derive(Clone, Debug)]
pub struct Variable {
    pub path: VarPath,
    pub width: usize,
    pub current_values: Vec<*mut Value>,
    pub next_values: Vec<*mut Value>,
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (i, value) in self.current_values.iter().enumerate() {
            let value = unsafe { &**value };
            ret.push_str(&format!("{}[{}] = {:x};\n", self.path, i, value));
        }

        ret.trim_end().fmt(f)
    }
}

#[derive(Clone)]
pub struct FfValue {
    pub current: Value,
    pub next: Value,
}

impl FfValue {
    pub fn as_ptr(&self) -> *const Value {
        &self.current
    }

    pub fn as_mut_ptr(&mut self) -> *mut Value {
        &mut self.current
    }

    pub fn as_next_ptr(&self) -> *const Value {
        &self.next
    }

    pub fn as_next_mut_ptr(&mut self) -> *mut Value {
        &mut self.next
    }

    pub fn swap(&mut self) {
        std::mem::swap(&mut self.current, &mut self.next);
    }
}

#[derive(Clone)]
pub struct CombValue(pub Value);

impl CombValue {
    pub fn as_ptr(&self) -> *const Value {
        &self.0
    }

    pub fn as_mut_ptr(&mut self) -> *mut Value {
        &mut self.0
    }
}
