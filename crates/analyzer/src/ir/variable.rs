use crate::analyzer_error::{AnalyzerError, InvalidSelectKind};
use crate::conv::Context;
use crate::conv::utils::eval_width_select;
use crate::ir::{AssignDestination, Expression, Factor, Op, Shape, ShapeRef, Type, TypeKind};
use crate::symbol::Affiliation;
use crate::value::{Value, gen_mask};
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

#[derive(Clone, Debug)]
pub struct VarPathSelect(pub VarPath, pub VarSelect, pub TokenRange);

impl VarPathSelect {
    pub fn to_assign_destination(
        self,
        context: &mut Context,
        ignore_error: bool,
    ) -> Option<AssignDestination> {
        let (path, select, token) = self.into();

        if let Some((id, mut comptime)) = context.find_path(&path) {
            if let Some(part_select) = &comptime.part_select {
                comptime.r#type = part_select.base.clone();
            }

            let (array_select, width_select) = select.split(comptime.r#type.array.dims());
            comptime.r#type.array.drain(0..array_select.dimension());

            if let Some(variable) = context.variables.get(&id)
                && !variable.is_assignable()
                && !ignore_error
            {
                context.insert_error(AnalyzerError::invalid_assignment(
                    &path.to_string(),
                    &variable.kind.description(),
                    &token,
                ));
            }

            let width_select = if let Some(part_select) = &comptime.part_select {
                part_select.to_base_select(context, &width_select)?
            } else {
                eval_width_select(context, &path, &comptime.r#type, width_select)?
            };

            // TODO invalid_select

            if array_select.is_range() {
                // TODO
                None
            } else {
                Some(AssignDestination {
                    id,
                    path,
                    index: array_select.to_index(),
                    select: width_select,
                    comptime,
                    token,
                })
            }
        } else {
            // If base path is SystemVerilog, valid AssignDestination should be generated.
            let base = VarPath::new(path.first());
            if let Some((id, mut comptime)) = context.find_path(&base)
                && comptime.r#type.kind == TypeKind::SystemVerilog
            {
                let (array_select, _) = select.split(comptime.r#type.array.dims());
                comptime.r#type.array.drain(0..array_select.dimension());

                Some(AssignDestination {
                    id,
                    path,
                    index: array_select.to_index(),
                    select: VarSelect::default(),
                    comptime,
                    token,
                })
            } else {
                None
            }
        }
    }

    pub fn to_expression(self, context: &Context) -> Option<Expression> {
        let (path, select, token) = self.into();

        if let Some((id, mut comptime)) = context.find_path(&path) {
            let (array_select, width_select) = select.split(comptime.r#type.array.dims());
            comptime.r#type.array.drain(0..array_select.dimension());

            let src = Factor::Variable(id, array_select.to_index(), width_select, comptime, token);
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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
    pub fn append(&mut self, x: &[StrId]) {
        for x in x {
            self.0.push(*x)
        }
    }
    pub fn add_prelude(&mut self, x: &[StrId]) {
        let mut ret = x.to_vec();
        ret.append(&mut self.0);
        self.0 = ret;
    }
    pub fn remove_prelude(&mut self, x: &[StrId]) {
        if self.starts_with(x) {
            for _ in 0..x.len() {
                self.0.remove(0);
            }
        }
    }
    pub fn starts_with(&self, x: &[StrId]) -> bool {
        self.0.starts_with(x)
    }
    pub fn first(&self) -> StrId {
        self.0[0]
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
    pub fn from_index(index: usize, array: &ShapeRef) -> Self {
        let mut remaining = index;
        let mut ret = vec![];
        for a in array.iter().rev() {
            let a = a.unwrap_or(1);
            ret.push(remaining % a);
            remaining /= a;
        }

        let token = TokenRange::default();
        let ret: Vec<_> = ret
            .into_iter()
            .rev()
            .map(|x| Expression::create_value(x.into(), 32, token))
            .collect();
        Self(ret)
    }

    pub fn push(&mut self, x: Expression) {
        self.0.push(x)
    }

    pub fn dimension(&self) -> usize {
        self.0.len()
    }

    pub fn add_prelude(&mut self, x: &VarIndex) {
        let mut x = x.clone();
        for e in self.0.drain(..) {
            x.push(e);
        }
        self.0 = x.0;
    }

    pub fn append(&mut self, x: &VarIndex) {
        for x in &x.0 {
            self.0.push(x.clone());
        }
    }

    pub fn is_const(&self, context: &mut Context) -> bool {
        let mut ret = true;

        for x in &self.0 {
            let mut expr = x.clone();
            let comptime = expr.eval_comptime(context, None);
            ret &= comptime.is_const;
        }

        ret
    }

    pub fn eval_value(&self, context: &mut Context) -> Option<Vec<usize>> {
        let mut ret = vec![];
        for x in &self.0 {
            if let Some(x) = x.eval_value(context, None) {
                ret.push(x.to_usize())
            } else {
                return None;
            }
        }
        Some(ret)
    }

    pub fn to_select(self) -> VarSelect {
        VarSelect(self.0, None)
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
    pub fn eval_expr(&self, beg: &Expression, end: &Expression) -> (Expression, Expression) {
        match self {
            VarSelectOp::Colon => (beg.clone(), end.clone()),
            VarSelectOp::PlusColon => {
                let expr =
                    Expression::Binary(Box::new(beg.clone()), Op::Add, Box::new(end.clone()));
                let minus_one = Expression::Unary(
                    Op::Sub,
                    Box::new(Expression::create_value(
                        1u32.into(),
                        32,
                        TokenRange::default(),
                    )),
                );
                let expr = Expression::Binary(Box::new(expr), Op::Add, Box::new(minus_one));
                (expr, beg.clone())
            }
            VarSelectOp::MinusColon => {
                let expr = Expression::Unary(Op::Sub, Box::new(end.clone()));
                let expr = Expression::Binary(Box::new(beg.clone()), Op::Add, Box::new(expr));
                let expr = Expression::Binary(
                    Box::new(expr),
                    Op::Add,
                    Box::new(Expression::create_value(
                        1u32.into(),
                        32,
                        TokenRange::default(),
                    )),
                );
                (beg.clone(), expr)
            }
            VarSelectOp::Step => {
                let mul = Expression::Binary(Box::new(beg.clone()), Op::Mul, Box::new(end.clone()));
                let expr =
                    Expression::Binary(Box::new(mul.clone()), Op::Add, Box::new(end.clone()));
                let minus_one = Expression::Unary(
                    Op::Sub,
                    Box::new(Expression::create_value(
                        1u32.into(),
                        32,
                        TokenRange::default(),
                    )),
                );
                let expr = Expression::Binary(Box::new(expr), Op::Add, Box::new(minus_one));
                (expr, mul)
            }
        }
    }

    pub fn eval_value(&self, beg: usize, end: usize) -> (usize, usize) {
        match self {
            VarSelectOp::Colon => (beg, end),
            VarSelectOp::PlusColon => ((beg + end).saturating_sub(1), beg),
            VarSelectOp::MinusColon => (beg, beg.saturating_sub(end) + 1),
            VarSelectOp::Step => ((beg * end + end).saturating_sub(1), beg * end),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VarSelect(pub Vec<Expression>, pub Option<(VarSelectOp, Expression)>);

impl VarSelect {
    pub fn set_index(&mut self, index: &VarIndex) {
        for x in &mut self.0 {
            x.set_index(index);
        }

        if let Some((_, x)) = &mut self.1 {
            x.set_index(index);
        }
    }

    pub fn push(&mut self, x: Expression) {
        self.0.push(x)
    }

    pub fn append(&mut self, mut x: VarSelect) {
        self.0.append(&mut x.0);
        self.1 = x.1;
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

    pub fn is_const(&self, context: &mut Context) -> bool {
        let mut ret = true;

        for x in &self.0 {
            let mut expr = x.clone();
            let comptime = expr.eval_comptime(context, None);
            ret &= comptime.is_const;
        }

        ret
    }

    pub fn to_index(self) -> VarIndex {
        VarIndex(self.0)
    }

    pub fn token_range(&self) -> TokenRange {
        if let Some(x) = self.0.first() {
            let beg = x.token_range();

            let end = if let Some(x) = &self.1 {
                x.1.token_range()
            } else {
                self.0.last().unwrap().token_range()
            };
            TokenRange {
                beg: beg.beg,
                end: end.end,
            }
        } else {
            TokenRange::default()
        }
    }

    pub fn eval_comptime(
        &self,
        context: &mut Context,
        r#type: &Type,
        is_array: bool,
    ) -> Option<Shape> {
        if r#type.is_unknown() {
            None
        } else {
            let r#type = if is_array {
                &r#type.array
            } else {
                &r#type.width
            };

            if self.is_empty() {
                Some(r#type.to_owned())
            } else {
                let dim = self.dimension();
                let beg = self.0.last().unwrap();
                let mut range = beg.token_range();
                let beg = beg.eval_value(context, None);

                let beg = if let Some(beg) = beg {
                    beg.to_usize()
                } else {
                    // Even if beg is unknown, single select can be determined
                    if self.1.is_none() {
                        let mut ret = r#type.to_owned();
                        ret.drain(0..dim);
                        let ret = if !is_array && ret.is_empty() {
                            Some(Shape::new(vec![Some(1)]))
                        } else {
                            Some(ret)
                        };
                        return ret;
                    } else {
                        return None;
                    }
                };

                let (beg, end) = if let Some((op, x)) = &self.1 {
                    range.set_end(x.token_range());
                    let end = x.eval_value(context, None)?.to_usize();
                    op.eval_value(beg, end)
                } else {
                    (beg, beg)
                };

                if r#type.dims() < dim {
                    if dim == 1 && r#type.dims() == 0 && !is_array {
                        let width = beg - end + 1;
                        return Some(Shape::new(vec![Some(width)]));
                    } else {
                        context.insert_error(AnalyzerError::invalid_select(
                            &InvalidSelectKind::OutOfDimension {
                                dim,
                                size: r#type.dims(),
                            },
                            &range,
                            &[],
                        ));
                        return None;
                    }
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
                    if let Some(size) = r#type.get(i)
                        && let Some(size) = size
                    {
                        let size = *size;
                        let beg = beg.eval_value(context, None)?;

                        if beg.is_xz() {
                            // skip out_of_range check
                            continue;
                        }

                        let beg = beg.to_usize();
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

                let mut ret = r#type.to_owned();

                if width == 1 {
                    ret.drain(0..dim);
                } else {
                    ret.drain(0..(dim - 1));
                    let first = ret.first_mut().unwrap();
                    *first = Some(width);
                }

                if !is_array && ret.is_empty() {
                    Some(Shape::new(vec![Some(1)]))
                } else {
                    Some(ret)
                }
            }
        }
    }

    pub fn eval_value(
        &self,
        context: &mut Context,
        r#type: &Type,
        is_array: bool,
    ) -> Option<(usize, usize)> {
        if self.0.is_empty() {
            let total_width: usize = if is_array {
                r#type.total_array()?
            } else {
                r#type.total_width()?
            };
            return Some((total_width.saturating_sub(1), 0));
        }

        let r#type = if is_array {
            &r#type.array
        } else {
            &r#type.width
        };

        let mut beg = 0;
        let mut end = 0;
        let mut base = 1;

        let dim = self.dimension();
        if r#type.dims() < dim {
            if dim == 1 && r#type.dims() == 0 && !is_array {
                let x = self.0[0].eval_value(context, None)?.to_usize();
                let (x, y) = if let Some((op, y)) = &self.1 {
                    let y = y.eval_value(context, None)?.to_usize();
                    op.eval_value(x, y)
                } else {
                    (x, x)
                };
                return Some((x, y));
            } else {
                return None;
            }
        }
        let skip = r#type.dims() - dim;
        for (i, w) in r#type.iter().rev().enumerate() {
            if let Some(w) = w {
                if i == skip {
                    let x = self.0.get(dim - (i - skip) - 1)?;
                    let x = x.eval_value(context, None)?.to_usize();

                    let (x, y) = if let Some((op, y)) = &self.1 {
                        let y = y.eval_value(context, None)?.to_usize();
                        op.eval_value(x, y)
                    } else {
                        (x, x)
                    };

                    if is_array {
                        beg += x * base;
                        end += (y + 1) * base - 1;
                    } else {
                        beg += (x + 1) * base - 1;
                        end += y * base;
                    }
                } else if i > skip {
                    let x = self.0.get(dim - (i - skip) - 1)?;
                    let x = x.eval_value(context, None)?.to_usize();

                    beg += x * base;
                    end += x * base;
                }
                base *= w;
            } else {
                return None;
            }
        }

        let token = self.token_range();
        let beg = context.check_size(beg, token)?;
        let end = context.check_size(end, token)?;

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
    Let,
}

impl VarKind {
    pub fn is_param(&self) -> bool {
        matches!(self, VarKind::Param)
    }

    pub fn is_port(&self) -> bool {
        matches!(self, VarKind::Input | VarKind::Output | VarKind::Inout)
    }

    pub fn description(&self) -> String {
        let ret = match self {
            VarKind::Param => "parameter",
            VarKind::Const => "constant",
            VarKind::Input => "input",
            VarKind::Output => "output",
            VarKind::Inout => "inout",
            VarKind::Variable => "variable",
            VarKind::Let => "let-bounded variable",
        };
        ret.to_string()
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
            VarKind::Let => "let",
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

        Self {
            id,
            path,
            kind,
            r#type,
            value,
            assigned,
            affiliation,
            token: *token,
        }
    }

    pub fn get_value(&self, index: &[usize]) -> Option<&Value> {
        let index = self.r#type.array.calc_index(index)?;
        self.value.get(index)
    }

    pub fn set_value(
        &mut self,
        index: &[usize],
        mut value: Value,
        range: Option<(usize, usize)>,
    ) -> bool {
        let Some(index) = self.r#type.array.calc_index(index) else {
            return false;
        };
        if let Some(total_width) = self.total_width() {
            value.trunc(total_width);
            if let Some(x) = self.value.get_mut(index) {
                if let Some((beg, end)) = range {
                    x.assign(value, beg, end);
                } else {
                    *x = value;
                }
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn set_assigned(&mut self, index: usize, value: BigUint) -> bool {
        if let Some(x) = self.assigned.get_mut(index) {
            *x = value;
            true
        } else {
            false
        }
    }

    pub fn unassigned(&self) -> Vec<usize> {
        let mut ret = vec![];
        if let Some(total_width) = self.total_width() {
            let mask = gen_mask(total_width);

            for (i, assigned) in self.assigned.iter().enumerate() {
                if *assigned != mask {
                    ret.push(i);
                }
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

    pub fn total_width(&self) -> Option<usize> {
        self.r#type.total_width()
    }

    pub fn prepend_array(&mut self, array: &ShapeRef) {
        if !array.is_empty()
            && let Some(total_array) = array.total()
        {
            let value = self.value.clone();
            let assigned = self.assigned.clone();
            for _ in 0..total_array.saturating_sub(1) {
                self.value.append(&mut value.clone());
                self.assigned.append(&mut assigned.clone());
            }
            self.r#type.prepend_array(array);
        }
    }
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        // adjust type format
        let mut r#type = self.r#type.clone();
        if &r#type.width == ShapeRef::new(&[Some(1)]) {
            r#type.width.clear();
        }
        r#type.array.clear();

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
            ret.push_str(&format!("{}", r#type));

            ret.push_str(&format!(" = 'h{:x};\n", value));
        }
        ret.trim_end().fmt(f)
    }
}

#[derive(Clone)]
pub struct VariableInfo {
    pub id: VarId,
    pub path: VarPath,
    pub kind: VarKind,
    pub r#type: Type,
    pub affiliation: Affiliation,
    pub token: TokenRange,
}

impl VariableInfo {
    pub fn new(x: &Variable) -> Self {
        Self {
            id: x.id,
            path: x.path.clone(),
            kind: x.kind,
            r#type: x.r#type.clone(),
            affiliation: x.affiliation,
            token: x.token,
        }
    }

    pub fn total_width(&self) -> Option<usize> {
        self.r#type.total_width()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::TypeKind;
    use veryl_parser::token_range::TokenRange;

    fn gen_var_select(x: &[u32], y: Option<u32>) -> VarSelect {
        let mut ret = VarSelect::default();

        for x in x {
            let token = TokenRange::default();
            let expr = Expression::create_value((*x).into(), 8, token);
            ret.push(expr);
        }

        if let Some(y) = y {
            let token = TokenRange::default();
            let expr = Expression::create_value(y.into(), 8, token);
            let op = VarSelectOp::Colon;
            ret.1 = Some((op, expr));
        }

        ret
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

        let array = Shape::new(vec![Some(4), Some(5), Some(6)]);

        let r#type = Type {
            kind: TypeKind::Logic,
            array: array.clone(),
            ..Default::default()
        };

        let y0 = x0.eval_comptime(&mut context, &r#type, true).unwrap();
        let y1 = x1.eval_comptime(&mut context, &r#type, true).unwrap();
        let y2 = x2.eval_comptime(&mut context, &r#type, true).unwrap();
        let y3 = x3.eval_comptime(&mut context, &r#type, true).unwrap();
        let y4 = x4.eval_comptime(&mut context, &r#type, true).unwrap();
        let y5 = x5.eval_comptime(&mut context, &r#type, true).unwrap();

        assert_eq!(&y0, ShapeRef::new(&[]));
        assert_eq!(&y1, ShapeRef::new(&[Some(2)]));
        assert_eq!(&y2, ShapeRef::new(&[Some(6)]));
        assert_eq!(&y3, ShapeRef::new(&[Some(2), Some(6)]));
        assert_eq!(&y4, ShapeRef::new(&[Some(5), Some(6)]));
        assert_eq!(&y5, ShapeRef::new(&[Some(2), Some(5), Some(6)]));

        let mut context = Context::default();

        let z0 = x0.eval_value(&mut context, &r#type, true).unwrap();
        let z1 = x1.eval_value(&mut context, &r#type, true).unwrap();
        let z2 = x2.eval_value(&mut context, &r#type, true).unwrap();
        let z3 = x3.eval_value(&mut context, &r#type, true).unwrap();
        let z4 = x4.eval_value(&mut context, &r#type, true).unwrap();
        let z5 = x5.eval_value(&mut context, &r#type, true).unwrap();

        assert_eq!(z0, (82, 82));
        assert_eq!(z1, (82, 83));
        assert_eq!(z2, (78, 83));
        assert_eq!(z3, (78, 89));
        assert_eq!(z4, (60, 89));
        assert_eq!(z5, (60, 119));
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

        let width = Shape::new(vec![Some(4), Some(5), Some(6)]);

        let r#type = Type {
            kind: TypeKind::Logic,
            width: width.clone(),
            ..Default::default()
        };

        let y0 = x0.eval_comptime(&mut context, &r#type, false).unwrap();
        let y1 = x1.eval_comptime(&mut context, &r#type, false).unwrap();
        let y2 = x2.eval_comptime(&mut context, &r#type, false).unwrap();
        let y3 = x3.eval_comptime(&mut context, &r#type, false).unwrap();
        let y4 = x4.eval_comptime(&mut context, &r#type, false).unwrap();
        let y5 = x5.eval_comptime(&mut context, &r#type, false).unwrap();
        let y6 = x6.eval_comptime(&mut context, &r#type, false).unwrap();

        assert_eq!(&y0, ShapeRef::new(&[Some(1)]));
        assert_eq!(&y1, ShapeRef::new(&[Some(2)]));
        assert_eq!(&y2, ShapeRef::new(&[Some(6)]));
        assert_eq!(&y3, ShapeRef::new(&[Some(2), Some(6)]));
        assert_eq!(&y4, ShapeRef::new(&[Some(5), Some(6)]));
        assert_eq!(&y5, ShapeRef::new(&[Some(2), Some(5), Some(6)]));
        assert_eq!(&y6, ShapeRef::new(&[Some(4), Some(5), Some(6)]));

        let mut context = Context::default();

        let z0 = x0.eval_value(&mut context, &r#type, false).unwrap();
        let z1 = x1.eval_value(&mut context, &r#type, false).unwrap();
        let z2 = x2.eval_value(&mut context, &r#type, false).unwrap();
        let z3 = x3.eval_value(&mut context, &r#type, false).unwrap();
        let z4 = x4.eval_value(&mut context, &r#type, false).unwrap();
        let z5 = x5.eval_value(&mut context, &r#type, false).unwrap();
        let z6 = x6.eval_value(&mut context, &r#type, false).unwrap();

        assert_eq!(z0, (82, 82));
        assert_eq!(z1, (83, 82));
        assert_eq!(z2, (83, 78));
        assert_eq!(z3, (89, 78));
        assert_eq!(z4, (89, 60));
        assert_eq!(z5, (119, 60));
        assert_eq!(z6, (119, 0));
    }

    #[test]
    fn var_index_from_index() {
        let array = Shape::new(vec![Some(2), Some(3), Some(4)]);

        let x00 = VarIndex::from_index(0, &array);
        let x01 = VarIndex::from_index(1, &array);
        let x02 = VarIndex::from_index(2, &array);
        let x03 = VarIndex::from_index(3, &array);
        let x04 = VarIndex::from_index(4, &array);
        let x05 = VarIndex::from_index(5, &array);
        let x06 = VarIndex::from_index(6, &array);
        let x07 = VarIndex::from_index(7, &array);
        let x08 = VarIndex::from_index(8, &array);
        let x09 = VarIndex::from_index(9, &array);
        let x10 = VarIndex::from_index(10, &array);
        let x11 = VarIndex::from_index(11, &array);
        let x12 = VarIndex::from_index(12, &array);
        let x13 = VarIndex::from_index(13, &array);
        let x14 = VarIndex::from_index(14, &array);
        let x15 = VarIndex::from_index(15, &array);
        let x16 = VarIndex::from_index(16, &array);
        let x17 = VarIndex::from_index(17, &array);
        let x18 = VarIndex::from_index(18, &array);
        let x19 = VarIndex::from_index(19, &array);
        let x20 = VarIndex::from_index(20, &array);
        let x21 = VarIndex::from_index(21, &array);
        let x22 = VarIndex::from_index(22, &array);
        let x23 = VarIndex::from_index(23, &array);

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
