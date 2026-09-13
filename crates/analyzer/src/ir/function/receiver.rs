//! Apply receiver coordinates only to storage and calls owned by that receiver.

use super::*;
use crate::ir::system_function::Output;
use crate::ir::{
    CasePattern, Factor, ForBound, ForRange, SystemFunctionCall, SystemFunctionKind, VarSelect,
};

pub(super) enum Receiver<'a> {
    Bind(&'a VarIndex, &'a HashMap<VarId, usize>),
    Prepend(usize, &'a HashSet<VarId>),
}

impl Receiver<'_> {
    pub(super) fn statements(&self, statements: &mut [Statement]) {
        for statement in statements {
            match statement {
                Statement::Assign(x) => {
                    for dst in &mut x.dst {
                        self.destination(dst);
                    }
                    self.expression(&mut x.expr);
                }
                Statement::If(x) => {
                    self.expression(&mut x.cond);
                    self.statements(&mut x.true_side);
                    self.statements(&mut x.false_side);
                }
                Statement::IfReset(x) => {
                    self.statements(&mut x.true_side);
                    self.statements(&mut x.false_side);
                }
                Statement::Case(x) => {
                    self.expression(&mut x.case_target);
                    for arm in &mut x.arms {
                        for pattern in &mut arm.patterns {
                            match pattern {
                                CasePattern::Eq(x) => self.expression(x),
                                CasePattern::Range { lo, hi, .. } => {
                                    self.expression(lo);
                                    self.expression(hi);
                                }
                            }
                        }
                        self.statements(&mut arm.body);
                    }
                    self.statements(&mut x.default);
                }
                Statement::For(x) => {
                    let (start, end) = match &mut x.range {
                        ForRange::Forward { start, end, .. }
                        | ForRange::Reverse { start, end, .. }
                        | ForRange::Stepped { start, end, .. } => (start, end),
                    };
                    for bound in [start, end] {
                        if let ForBound::Expression(x) = bound {
                            self.expression(x);
                        }
                    }
                    self.statements(&mut x.body);
                }
                Statement::FunctionCall(x) => self.call(x),
                Statement::SystemFunctionCall(x) => self.system_call(x),
                Statement::TbMethodCall(_)
                | Statement::Break
                | Statement::Unsupported(_)
                | Statement::Null => {}
            }
        }
    }

    fn expression(&self, expression: &mut Expression) {
        match expression {
            Expression::Term(factor) => match factor.as_mut() {
                Factor::Variable(id, index, select, _) => self.variable(*id, index, select),
                Factor::FunctionCall(x) => self.call(x),
                Factor::SystemFunctionCall(x) => self.system_call(x),
                _ => {}
            },
            Expression::Unary(_, x, _) => self.expression(x),
            Expression::Binary(x, _, y, _) => {
                self.expression(x);
                self.expression(y);
            }
            Expression::Ternary(x, y, z, _) => {
                self.expression(x);
                self.expression(y);
                self.expression(z);
            }
            Expression::Concatenation(items, _) => {
                for (item, repeat) in items {
                    self.expression(item);
                    if let Some(repeat) = repeat {
                        self.expression(repeat);
                    }
                }
            }
            Expression::StructConstructor(_, fields, _) => {
                for (_, field) in fields {
                    self.expression(field);
                }
            }
            Expression::ArrayLiteral(items, _) => {
                for item in items {
                    match item {
                        crate::ir::ArrayLiteralItem::Value(value, repeat) => {
                            self.expression(value);
                            if let Some(repeat) = repeat {
                                self.expression(repeat);
                            }
                        }
                        crate::ir::ArrayLiteralItem::Defaul(value) => self.expression(value),
                    }
                }
            }
        }
    }

    fn variable(&self, id: VarId, index: &mut VarIndex, select: &mut VarSelect) {
        for expression in index.0.iter_mut().chain(select.0.iter_mut()) {
            self.expression(expression);
        }
        if let Some((_, end)) = &mut select.1 {
            self.expression(end);
        }
        if let Self::Bind(receiver, prefixes) = self
            && let Some(dims) = prefixes.get(&id)
        {
            let prefix = receiver
                .0
                .get(..*dims)
                .expect("receiver owns the required coordinates");
            index.add_prelude(&VarIndex(prefix.to_vec()));
        }
    }

    fn destination(&self, dst: &mut AssignDestination) {
        self.variable(dst.id, &mut dst.index, &mut dst.select);
    }

    fn call(&self, call: &mut FunctionCall) {
        for expression in &mut call.receiver_index.0 {
            self.expression(expression);
        }
        match self {
            Self::Bind(receiver, _) => {
                if call.receiver_prefix_dims != 0 {
                    let prefix = receiver
                        .0
                        .get(..call.receiver_prefix_dims)
                        .expect("nested call owns the required receiver coordinates");
                    call.receiver_index.add_prelude(&VarIndex(prefix.to_vec()));
                }
                call.index = call
                    .receiver_index
                    .is_const()
                    .then(|| {
                        call.receiver_index
                            .0
                            .iter()
                            .map(|x| x.comptime().get_value().ok()?.to_usize())
                            .collect()
                    })
                    .flatten();
            }
            Self::Prepend(dims, functions) => {
                if functions.contains(&call.id) {
                    call.receiver_prefix_dims += dims;
                }
            }
        }
        for input in call.inputs.values_mut() {
            self.expression(input);
        }
        for outputs in call.outputs.values_mut() {
            for output in outputs {
                self.destination(output);
            }
        }
    }

    fn system_call(&self, call: &mut SystemFunctionCall) {
        match &mut call.kind {
            SystemFunctionKind::Bits(x)
            | SystemFunctionKind::Size(x)
            | SystemFunctionKind::Clog2(x)
            | SystemFunctionKind::Onehot(x)
            | SystemFunctionKind::Signed(x)
            | SystemFunctionKind::Unsigned(x) => self.expression(&mut x.0),
            SystemFunctionKind::Readmemh(input, output) => {
                self.expression(&mut input.0);
                if let Output::Local(outputs) = output {
                    for output in outputs {
                        self.destination(output);
                    }
                }
            }
            SystemFunctionKind::Display(inputs) | SystemFunctionKind::Write(inputs) => {
                for input in inputs {
                    self.expression(&mut input.0);
                }
            }
            SystemFunctionKind::Assert { cond, args, .. } => {
                self.expression(&mut cond.0);
                for input in args {
                    self.expression(&mut input.0);
                }
            }
            SystemFunctionKind::Finish => {}
        }
    }
}
