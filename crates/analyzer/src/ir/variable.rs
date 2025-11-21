use crate::HashMap;
use crate::ir::{Expression, Select, Value};
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct VarId(u32);

impl VarId {
    pub fn inc(&mut self) {
        self.0 += 1;
    }
}

impl fmt::Display for VarId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = format!("var{}", self.0);
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct VarPathIndex(pub VarPath, pub VarIndex);

impl From<VarPathIndex> for (VarPath, VarIndex) {
    fn from(value: VarPathIndex) -> Self {
        (value.0, value.1)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct VarPath(pub Vec<StrId>);

impl VarPath {
    pub fn new(x: StrId) -> Self {
        Self(vec![x])
    }
    pub fn from_slice(x: &[StrId]) -> Self {
        Self(x.to_vec())
    }
    pub fn push(&mut self, x: StrId) {
        self.0.push(x)
    }
    pub fn add_prelude(&mut self, x: &[StrId]) {
        let mut ret = x.to_vec();
        ret.append(&mut self.0);
        self.0 = ret;
    }
}

impl fmt::Display for VarPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for id in &self.0 {
            ret.push('.');
            ret.push_str(&format!("{id}"));
        }

        ret[1..].fmt(f)
    }
}

#[derive(Clone, Debug, Default)]
pub struct VarIndex(pub Vec<Expression>, pub Option<Expression>);

impl VarIndex {
    pub fn push(&mut self, x: Expression) {
        self.0.push(x)
    }

    pub fn split(mut self, i: usize) -> (Self, Vec<Select>) {
        if self.0.len() <= i {
            (self, vec![])
        } else {
            let x = self.0.drain(0..i).collect();
            let mut y = vec![];
            let len = self.0.len();
            let end = self.1.take();
            for (i, expr) in self.0.into_iter().enumerate() {
                if i == (len - 1) {
                    y.push(Select {
                        beg: expr,
                        end: end.clone(),
                    });
                } else {
                    y.push(Select {
                        beg: expr,
                        end: None,
                    });
                }
            }
            (VarIndex(x, None), y)
        }
    }

    pub fn dimension(&self) -> usize {
        self.0.len()
    }

    pub fn eval(&self, map: &HashMap<VarId, Variable>) -> Option<Vec<usize>> {
        // ignore self.1 because it is part of Select
        let mut ret = vec![];
        for x in &self.0 {
            if let Some(x) = x.eval(None, map) {
                ret.push(x.to_usize())
            } else {
                return None;
            }
        }
        Some(ret)
    }
}

impl fmt::Display for VarIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();
        for i in &self.0 {
            ret.push_str(&format!("[{i}]"));
        }
        ret.fmt(f)
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum VarKind {
    Param,
    Const,
    Input,
    Output,
    Inout,
    Variable,
}

impl VarKind {
    pub fn is_param(&self) -> bool {
        matches!(self, VarKind::Param)
    }

    pub fn is_port(&self) -> bool {
        matches!(self, VarKind::Input | VarKind::Output | VarKind::Inout)
    }
}

impl fmt::Display for VarKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ret = match self {
            VarKind::Param => "param",
            VarKind::Const => "const",
            VarKind::Input => "input",
            VarKind::Output => "output",
            VarKind::Inout => "inout",
            VarKind::Variable => "var",
        };
        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct Variable {
    pub id: VarId,
    pub path: VarPath,
    pub kind: VarKind,
    pub value: Vec<Value>,
    pub array_base: Vec<usize>,
}

impl Variable {
    pub fn new(path: VarPath, kind: VarKind, value: Vec<Value>, array: &[usize]) -> Self {
        Self {
            // actual VarId is assigned at Context
            id: VarId::default(),
            path,
            kind,
            value,
            array_base: gen_array_base(array),
        }
    }

    pub fn get_value(&self, index: &[usize]) -> Option<Value> {
        let index = calc_index(index, &self.array_base);
        self.value.get(index).cloned()
    }

    pub fn set_value(&mut self, index: &[usize], value: Value) -> bool {
        let index = calc_index(index, &self.array_base);
        if let Some(x) = self.value.get_mut(index) {
            *x = value;
            true
        } else {
            false
        }
    }
}

/// Calc base offset of each dimension
fn gen_array_base(array: &[usize]) -> Vec<usize> {
    let mut array_base = vec![];
    let mut mul = 1;
    let len = array.len();
    for (i, x) in array.iter().rev().enumerate() {
        if i == (len - 1) {
            break;
        }
        mul *= x;
        array_base.push(mul);
    }
    array_base.reverse();
    array_base
}

fn calc_index(index: &[usize], array_base: &[usize]) -> usize {
    if index.is_empty() {
        0
    } else {
        assert!(index.len() == array_base.len() + 1);

        let mut ret = 0;
        for (i, x) in index.iter().enumerate() {
            ret += x * array_base.get(i).unwrap_or(&1);
        }
        ret
    }
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        let is_array = self.value.len() != 1;
        for (i, value) in self.value.iter().enumerate() {
            if is_array {
                ret.push_str(&format!(
                    "{} {}[{}]({}): ",
                    self.kind, self.id, i, self.path
                ));
            } else {
                ret.push_str(&format!("{} {}({}): ", self.kind, self.id, self.path));
            }
            if value.signed {
                ret.push_str("signed logic");
            } else {
                ret.push_str("logic");
            }
            if value.width != 1 {
                ret.push_str(&format!("<{}>", value.width));
            }

            ret.push_str(&format!(" = 'h{:x};\n", value));
        }
        ret.trim_end().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_base() {
        let array = vec![2, 3, 4];
        let array_base = gen_array_base(&array);
        assert_eq!(array_base, vec![12, 4]);

        let array = vec![1, 5, 7, 13];
        let array_base = gen_array_base(&array);
        assert_eq!(array_base, vec![455, 91, 13]);
    }
}
