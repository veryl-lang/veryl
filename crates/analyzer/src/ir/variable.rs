use crate::HashMap;
use crate::analyzer_error::{AnalyzerError, InvalidSelectKind};
use crate::conv::Context;
use crate::ir::bigint::gen_mask;
use crate::ir::{AssignDestination, Expression, Factor, Type, TypedValue, Value};
use crate::symbol::Affiliation;
use num_bigint::BigUint;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

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
pub struct VarPathSelect(pub VarPath, pub VarSelect, pub TokenRange);

impl VarPathSelect {
    pub fn to_assign_destination(self, context: &Context) -> Option<AssignDestination> {
        let (path, select, token) = self.into();

        if let Some((id, mut typed_value)) = context.find_path(&path) {
            let (array_select, width_select) = select.split(typed_value.r#type.array.len());
            typed_value.r#type.array.drain(0..array_select.dimension());

            // TODO invalid_select

            if array_select.is_range() {
                // TODO
                None
            } else {
                Some(AssignDestination {
                    id,
                    index: array_select.to_index(),
                    select: width_select,
                    r#type: typed_value.r#type,
                    token,
                })
            }
        } else {
            None
        }
    }

    pub fn to_expression(self, context: &Context) -> Option<Expression> {
        let (path, select, token) = self.into();

        if let Some((id, typed_value)) = context.find_path(&path) {
            let src = Factor::Variable(id, VarIndex::default(), select, typed_value, token);
            Some(Expression::Term(Box::new(src)))
        } else {
            None
        }
    }
}

impl From<VarPathSelect> for (VarPath, VarSelect, TokenRange) {
    fn from(value: VarPathSelect) -> Self {
        (value.0, value.1, value.2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
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
    pub fn pop(&mut self) {
        self.0.pop();
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
pub struct VarIndex(pub Vec<Expression>);

impl VarIndex {
    pub fn from_index(index: usize, array: &[usize]) -> Self {
        let mut remaining = index;
        let mut ret = vec![];
        for a in array.iter().rev() {
            ret.push(remaining % a);
            remaining /= a;
        }

        let token = TokenRange::default();
        let ret: Vec<_> = ret
            .into_iter()
            .rev()
            .map(|x| {
                Expression::Term(Box::new(Factor::Value(
                    TypedValue::create_value(x.into(), 32),
                    token,
                )))
            })
            .collect();
        Self(ret)
    }

    pub fn push(&mut self, x: Expression) {
        self.0.push(x)
    }

    pub fn dimension(&self) -> usize {
        self.0.len()
    }

    pub fn eval(&self, map: &HashMap<VarId, Variable>) -> Option<Vec<usize>> {
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

#[derive(Clone, Debug)]
pub enum VarSelectOp {
    Colon,
    PlusColon,
    MinusColon,
    Step,
}

impl fmt::Display for VarSelectOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VarSelectOp::Colon => ":".fmt(f),
            VarSelectOp::PlusColon => "+:".fmt(f),
            VarSelectOp::MinusColon => "-:".fmt(f),
            VarSelectOp::Step => " step ".fmt(f),
        }
    }
}

impl VarSelectOp {
    pub fn eval(&self, beg: usize, end: usize) -> (usize, usize) {
        match self {
            VarSelectOp::Colon => (beg, end),
            VarSelectOp::PlusColon => (beg + end - 1, beg),
            VarSelectOp::MinusColon => (beg, beg - end + 1),
            VarSelectOp::Step => (beg * end + end - 1, beg * end),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VarSelect(pub Vec<Expression>, pub Option<(VarSelectOp, Expression)>);

impl VarSelect {
    pub fn push(&mut self, x: Expression) {
        self.0.push(x)
    }

    pub fn split(mut self, i: usize) -> (Self, Self) {
        if self.0.len() <= i {
            (self, VarSelect::default())
        } else {
            let x = self.0.drain(0..i).collect();
            (VarSelect(x, None), self)
        }
    }

    pub fn dimension(&self) -> usize {
        self.0.len()
    }

    pub fn is_range(&self) -> bool {
        self.1.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_index(self) -> VarIndex {
        VarIndex(self.0)
    }

    pub fn eval_type(
        &self,
        context: &mut Context,
        r#type: &[usize],
        is_array: bool,
    ) -> Option<Vec<usize>> {
        if self.is_empty() {
            Some(r#type.to_vec())
        } else {
            let dim = self.dimension();
            let beg = self.0.last().unwrap();
            let mut range = beg.token_range();
            let beg = beg.eval(None, &context.variables)?.to_usize();

            let (beg, end) = if let Some((op, x)) = &self.1 {
                range.set_end(x.token_range());
                let end = x.eval(None, &context.variables)?.to_usize();
                op.eval(beg, end)
            } else {
                (beg, beg)
            };

            if r#type.len() < dim {
                context.insert_error(AnalyzerError::invalid_select(
                    &InvalidSelectKind::OutOfDimension {
                        dim,
                        size: r#type.len(),
                    },
                    &range,
                    &[],
                ));
                return None;
            }

            let wrong_order = if is_array { beg > end } else { beg < end };
            if wrong_order {
                context.insert_error(AnalyzerError::invalid_select(
                    &InvalidSelectKind::WrongOrder { beg, end },
                    &range,
                    &[],
                ));
                return None;
            }

            for (i, beg) in self.0.iter().enumerate() {
                if let Some(size) = r#type.get(i) {
                    let size = *size;
                    let beg = beg.eval(None, &context.variables)?.to_usize();
                    let mut out_of_range = beg >= size;

                    if i == dim - 1 {
                        out_of_range |= end >= size;
                    }

                    if out_of_range {
                        context.insert_error(AnalyzerError::invalid_select(
                            &InvalidSelectKind::OutOfRange { beg, end, size },
                            &range,
                            &[],
                        ));
                        return None;
                    }
                }
            }

            let width = if is_array {
                end - beg + 1
            } else {
                beg - end + 1
            };

            let mut ret = r#type.to_vec();

            if width == 1 {
                ret.drain(0..dim);
            } else {
                ret.drain(0..(dim - 1));
                let first = ret.first_mut().unwrap();
                *first = width;
            }

            if !is_array && ret.is_empty() {
                Some(vec![1])
            } else {
                Some(ret)
            }
        }
    }

    pub fn eval(&self, map: &HashMap<VarId, Variable>, r#type: &[usize]) -> Option<(usize, usize)> {
        if self.0.is_empty() {
            let total_width: usize = r#type.iter().product();
            return Some((total_width - 1, 0));
        }

        let mut beg = 0;
        let mut end = 0;
        let mut base = 1;

        let dim = self.dimension();
        if r#type.len() < dim {
            return None;
        }
        let skip = r#type.len() - dim;
        for (i, w) in r#type.iter().rev().enumerate() {
            if i == skip {
                let x = self.0.get(dim - (i - skip) - 1)?;
                let x = x.eval(None, map)?.to_usize();

                let (x, y) = if let Some((op, y)) = &self.1 {
                    let y = y.eval(None, map)?.to_usize();
                    op.eval(x, y)
                } else {
                    (x, x)
                };

                beg += x * base;
                end += y * base;
            } else if i > skip {
                let x = self.0.get(dim - (i - skip) - 1)?;
                let x = x.eval(None, map)?.to_usize();

                beg += x * base;
                end += x * base;
            }
            base *= w;
        }

        Some((beg, end))
    }
}

impl fmt::Display for VarSelect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();
        let len = self.0.len();
        for (i, x) in self.0.iter().enumerate() {
            if i == len - 1 {
                if let Some((op, y)) = &self.1 {
                    ret.push_str(&format!("[{x}{op}{y}]"));
                } else {
                    ret.push_str(&format!("[{x}]"));
                }
            } else {
                ret.push_str(&format!("[{x}]"));
            }
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
    pub r#type: Type,
    pub value: Vec<Value>,
    pub array_base: Vec<usize>,
    pub assigned: Vec<BigUint>,
    pub affiliation: Affiliation,
    pub token: TokenRange,
}

impl Variable {
    pub fn new(
        id: VarId,
        path: VarPath,
        kind: VarKind,
        r#type: Type,
        value: Vec<Value>,
        affiliation: Affiliation,
        token: &TokenRange,
    ) -> Self {
        let mut assigned = vec![];
        for _ in 0..value.len() {
            assigned.push(0u32.into());
        }

        let array_base = gen_array_base(&r#type.array);

        Self {
            id,
            path,
            kind,
            r#type,
            value,
            array_base,
            assigned,
            affiliation,
            token: *token,
        }
    }

    pub fn get_value(&self, index: &[usize]) -> Option<Value> {
        let index = calc_index(index, &self.array_base)?;
        self.value.get(index).cloned()
    }

    pub fn set_value(&mut self, index: &[usize], value: Value) -> bool {
        let Some(index) = calc_index(index, &self.array_base) else {
            return false;
        };
        if let Some(x) = self.value.get_mut(index) {
            *x = value;
            true
        } else {
            false
        }
    }

    pub fn set_assigned(&mut self, index: &[usize], value: BigUint) -> bool {
        let Some(index) = calc_index(index, &self.array_base) else {
            return false;
        };
        if let Some(x) = self.assigned.get_mut(index) {
            *x = value;
            true
        } else {
            false
        }
    }

    pub fn unassigned(&self) -> Vec<usize> {
        let mut ret = vec![];
        let mask = gen_mask(self.total_width());

        for (i, assigned) in self.assigned.iter().enumerate() {
            if *assigned != mask {
                ret.push(i);
            }
        }

        ret
    }

    pub fn is_assignable(&self) -> bool {
        matches!(
            self.kind,
            VarKind::Output | VarKind::Inout | VarKind::Variable
        )
    }

    pub fn total_width(&self) -> usize {
        self.r#type.width.iter().product()
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

fn calc_index(index: &[usize], array_base: &[usize]) -> Option<usize> {
    if index.is_empty() {
        Some(0)
    } else {
        if index.len() != array_base.len() + 1 {
            return None;
        }

        let mut ret = 0;
        for (i, x) in index.iter().enumerate() {
            ret += x * array_base.get(i).unwrap_or(&1);
        }
        Some(ret)
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
    use crate::ir::{Factor, TypedValue};
    use veryl_parser::token_range::TokenRange;

    fn gen_var_select(x: &[u32], y: Option<u32>) -> VarSelect {
        let mut ret = VarSelect::default();

        for x in x {
            let value = TypedValue::create_value((*x).into(), 8);
            let range = TokenRange::default();
            let factor = Factor::Value(value, range);
            let expr = Expression::Term(Box::new(factor));
            ret.push(expr);
        }

        if let Some(y) = y {
            let value = TypedValue::create_value(y.into(), 8);
            let range = TokenRange::default();
            let factor = Factor::Value(value, range);
            let expr = Expression::Term(Box::new(factor));
            let op = VarSelectOp::Colon;
            ret.1 = Some((op, expr));
        }

        ret
    }

    #[test]
    fn array_base() {
        let array = vec![2, 3, 4];
        let array_base = gen_array_base(&array);
        assert_eq!(array_base, vec![12, 4]);

        let array = vec![1, 5, 7, 13];
        let array_base = gen_array_base(&array);
        assert_eq!(array_base, vec![455, 91, 13]);
    }

    #[test]
    fn var_select_array() {
        let mut context = Context::default();

        let x0 = gen_var_select(&[2, 3, 4], None);
        let x1 = gen_var_select(&[2, 3, 4], Some(5));
        let x2 = gen_var_select(&[2, 3], None);
        let x3 = gen_var_select(&[2, 3], Some(4));
        let x4 = gen_var_select(&[2], None);
        let x5 = gen_var_select(&[2], Some(3));

        assert_eq!(x0.to_string(), "[02][03][04]");
        assert_eq!(x1.to_string(), "[02][03][04:05]");
        assert_eq!(x2.to_string(), "[02][03]");
        assert_eq!(x3.to_string(), "[02][03:04]");
        assert_eq!(x4.to_string(), "[02]");
        assert_eq!(x5.to_string(), "[02:03]");

        let y0 = x0.eval_type(&mut context, &[4, 5, 6], true).unwrap();
        let y1 = x1.eval_type(&mut context, &[4, 5, 6], true).unwrap();
        let y2 = x2.eval_type(&mut context, &[4, 5, 6], true).unwrap();
        let y3 = x3.eval_type(&mut context, &[4, 5, 6], true).unwrap();
        let y4 = x4.eval_type(&mut context, &[4, 5, 6], true).unwrap();
        let y5 = x5.eval_type(&mut context, &[4, 5, 6], true).unwrap();

        assert_eq!(y0, &[]);
        assert_eq!(y1, &[2]);
        assert_eq!(y2, &[6]);
        assert_eq!(y3, &[2, 6]);
        assert_eq!(y4, &[5, 6]);
        assert_eq!(y5, &[2, 5, 6]);

        let z0 = x0.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z1 = x1.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z2 = x2.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z3 = x3.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z4 = x4.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z5 = x5.eval(&context.variables, &[4, 5, 6]).unwrap();

        assert_eq!(z0, (82, 82));
        assert_eq!(z1, (82, 83));
        assert_eq!(z2, (78, 78));
        assert_eq!(z3, (78, 84));
        assert_eq!(z4, (60, 60));
        assert_eq!(z5, (60, 90));
    }

    #[test]
    fn var_select_width() {
        let mut context = Context::default();

        let x0 = gen_var_select(&[2, 3, 4], None);
        let x1 = gen_var_select(&[2, 3, 5], Some(4));
        let x2 = gen_var_select(&[2, 3], None);
        let x3 = gen_var_select(&[2, 4], Some(3));
        let x4 = gen_var_select(&[2], None);
        let x5 = gen_var_select(&[3], Some(2));
        let x6 = gen_var_select(&[], None);

        assert_eq!(x0.to_string(), "[02][03][04]");
        assert_eq!(x1.to_string(), "[02][03][05:04]");
        assert_eq!(x2.to_string(), "[02][03]");
        assert_eq!(x3.to_string(), "[02][04:03]");
        assert_eq!(x4.to_string(), "[02]");
        assert_eq!(x5.to_string(), "[03:02]");
        assert_eq!(x6.to_string(), "");

        let y0 = x0.eval_type(&mut context, &[4, 5, 6], false).unwrap();
        let y1 = x1.eval_type(&mut context, &[4, 5, 6], false).unwrap();
        let y2 = x2.eval_type(&mut context, &[4, 5, 6], false).unwrap();
        let y3 = x3.eval_type(&mut context, &[4, 5, 6], false).unwrap();
        let y4 = x4.eval_type(&mut context, &[4, 5, 6], false).unwrap();
        let y5 = x5.eval_type(&mut context, &[4, 5, 6], false).unwrap();
        let y6 = x6.eval_type(&mut context, &[4, 5, 6], false).unwrap();

        assert_eq!(y0, &[1]);
        assert_eq!(y1, &[2]);
        assert_eq!(y2, &[6]);
        assert_eq!(y3, &[2, 6]);
        assert_eq!(y4, &[5, 6]);
        assert_eq!(y5, &[2, 5, 6]);
        assert_eq!(y6, &[4, 5, 6]);

        let z0 = x0.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z1 = x1.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z2 = x2.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z3 = x3.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z4 = x4.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z5 = x5.eval(&context.variables, &[4, 5, 6]).unwrap();
        let z6 = x6.eval(&context.variables, &[4, 5, 6]).unwrap();

        assert_eq!(z0, (82, 82));
        assert_eq!(z1, (83, 82));
        assert_eq!(z2, (78, 78));
        assert_eq!(z3, (84, 78));
        assert_eq!(z4, (60, 60));
        assert_eq!(z5, (90, 60));
        assert_eq!(z6, (119, 0));
    }

    #[test]
    fn var_index_from_index() {
        let x00 = VarIndex::from_index(0, &[2, 3, 4]);
        let x01 = VarIndex::from_index(1, &[2, 3, 4]);
        let x02 = VarIndex::from_index(2, &[2, 3, 4]);
        let x03 = VarIndex::from_index(3, &[2, 3, 4]);
        let x04 = VarIndex::from_index(4, &[2, 3, 4]);
        let x05 = VarIndex::from_index(5, &[2, 3, 4]);
        let x06 = VarIndex::from_index(6, &[2, 3, 4]);
        let x07 = VarIndex::from_index(7, &[2, 3, 4]);
        let x08 = VarIndex::from_index(8, &[2, 3, 4]);
        let x09 = VarIndex::from_index(9, &[2, 3, 4]);
        let x10 = VarIndex::from_index(10, &[2, 3, 4]);
        let x11 = VarIndex::from_index(11, &[2, 3, 4]);
        let x12 = VarIndex::from_index(12, &[2, 3, 4]);
        let x13 = VarIndex::from_index(13, &[2, 3, 4]);
        let x14 = VarIndex::from_index(14, &[2, 3, 4]);
        let x15 = VarIndex::from_index(15, &[2, 3, 4]);
        let x16 = VarIndex::from_index(16, &[2, 3, 4]);
        let x17 = VarIndex::from_index(17, &[2, 3, 4]);
        let x18 = VarIndex::from_index(18, &[2, 3, 4]);
        let x19 = VarIndex::from_index(19, &[2, 3, 4]);
        let x20 = VarIndex::from_index(20, &[2, 3, 4]);
        let x21 = VarIndex::from_index(21, &[2, 3, 4]);
        let x22 = VarIndex::from_index(22, &[2, 3, 4]);
        let x23 = VarIndex::from_index(23, &[2, 3, 4]);

        assert_eq!(x00.to_string(), "[00000000][00000000][00000000]");
        assert_eq!(x01.to_string(), "[00000000][00000000][00000001]");
        assert_eq!(x02.to_string(), "[00000000][00000000][00000002]");
        assert_eq!(x03.to_string(), "[00000000][00000000][00000003]");
        assert_eq!(x04.to_string(), "[00000000][00000001][00000000]");
        assert_eq!(x05.to_string(), "[00000000][00000001][00000001]");
        assert_eq!(x06.to_string(), "[00000000][00000001][00000002]");
        assert_eq!(x07.to_string(), "[00000000][00000001][00000003]");
        assert_eq!(x08.to_string(), "[00000000][00000002][00000000]");
        assert_eq!(x09.to_string(), "[00000000][00000002][00000001]");
        assert_eq!(x10.to_string(), "[00000000][00000002][00000002]");
        assert_eq!(x11.to_string(), "[00000000][00000002][00000003]");
        assert_eq!(x12.to_string(), "[00000001][00000000][00000000]");
        assert_eq!(x13.to_string(), "[00000001][00000000][00000001]");
        assert_eq!(x14.to_string(), "[00000001][00000000][00000002]");
        assert_eq!(x15.to_string(), "[00000001][00000000][00000003]");
        assert_eq!(x16.to_string(), "[00000001][00000001][00000000]");
        assert_eq!(x17.to_string(), "[00000001][00000001][00000001]");
        assert_eq!(x18.to_string(), "[00000001][00000001][00000002]");
        assert_eq!(x19.to_string(), "[00000001][00000001][00000003]");
        assert_eq!(x20.to_string(), "[00000001][00000002][00000000]");
        assert_eq!(x21.to_string(), "[00000001][00000002][00000001]");
        assert_eq!(x22.to_string(), "[00000001][00000002][00000002]");
        assert_eq!(x23.to_string(), "[00000001][00000002][00000003]");
    }
}
