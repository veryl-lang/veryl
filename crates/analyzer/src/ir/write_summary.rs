//! Regions written by each driving process of a module specialization.
//!
//! The summary is built once at the end of a module's conversion, so it
//! survives even when the body itself is not retained, and parents lift the
//! processes of their children from it. Every `always_comb` / `assign`,
//! `always_ff`, `initial` and `final` declaration is one process, and so is
//! each output port connection of an instance and each process of a child
//! lifted through a direction-free interface binding. Writes inside one
//! process never conflict with each other: the last one wins.
//!
//! A procedural `for` loop is emitted as an SV loop, so its iterator is not a
//! static index: a write inside it covers what the constant prefix of its
//! destination selects, as in the longest static prefix rule. Elements of
//! that region the iterations never reach are included, so the region is
//! what the process owns rather than what it certainly writes.
//!
//! Regions are intervals, so the summary is independent of array sizes and
//! bit widths.

use crate::conv::Context;
use crate::ir::span::{ArraySpan, PackedSpan, dst_writes};
use crate::ir::{
    AssignDestination, Component, Declaration, Expression, Factor, FunctionCall, InstDeclaration,
    InstInterfaceBinding, Module, Statement, SystemFunctionCall, SystemFunctionKind, VarId,
    Variable,
};
use crate::{BigUint, HashMap};
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug, Default)]
pub struct WriteSummary {
    processes: Vec<ProcessWrites>,
    /// `(process, write)` of the writes to each variable.
    by_id: HashMap<VarId, Vec<(usize, usize)>>,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessWrites {
    pub writes: Vec<RegionWrite>,
}

#[derive(Clone, Debug)]
pub struct RegionWrite {
    pub id: VarId,
    pub array: ArraySpan,
    pub packed: PackedSpan,
    /// False for writes that do not drive their region at run time:
    /// connections of unknown direction, which may only read, and
    /// initialization exempted by `#[allow(initial_assign)]`.
    pub driving: bool,
    pub token: TokenRange,
}

impl WriteSummary {
    pub fn new(context: &mut Context, declarations: &[Declaration]) -> Self {
        context.begin_analysis_transaction();
        let mut processes = Vec::new();
        for declaration in declarations {
            match declaration {
                Declaration::Comb(x) => {
                    processes.push(Collector::process(context, &x.statements));
                }
                Declaration::Ff(x) => {
                    processes.push(Collector::process(context, &x.statements));
                }
                Declaration::Final(x) => {
                    processes.push(Collector::process(context, &x.statements));
                }
                Declaration::Initial(x) => {
                    let mut process = Collector::process(context, &x.statements);
                    for write in &mut process.writes {
                        write.driving = !context
                            .variables
                            .get(&write.id)
                            .is_some_and(Variable::allow_initial_assign);
                    }
                    processes.push(process);
                }
                Declaration::Inst(x) => instance_processes(context, x, &mut processes),
                Declaration::External(x) => {
                    let mut collector = Collector::new(false);
                    for dst in x.connects.iter().filter_map(|x| x.output.as_ref()) {
                        collector.add(context, dst);
                    }
                    processes.push(collector.finish());
                }
                Declaration::Unsupported(_) | Declaration::Null => {}
            }
        }
        context.rollback_analysis_transaction();
        processes.retain(|x| !x.writes.is_empty());
        let mut by_id: HashMap<_, Vec<_>> = HashMap::default();
        for (process, x) in processes.iter().enumerate() {
            for (write, x) in x.writes.iter().enumerate() {
                by_id.entry(x.id).or_default().push((process, write));
            }
        }
        Self { processes, by_id }
    }

    pub fn processes(&self) -> &[ProcessWrites] {
        &self.processes
    }

    /// Writes to `id`, driving or not, with the index of their process.
    fn process_writes(&self, id: VarId) -> impl Iterator<Item = (usize, &RegionWrite)> {
        self.by_id
            .get(&id)
            .into_iter()
            .flatten()
            .map(|&(process, write)| (process, &self.processes[process].writes[write]))
    }

    /// Writes to `id`, driving or not.
    pub fn writes(&self, id: VarId) -> impl Iterator<Item = &RegionWrite> {
        self.process_writes(id).map(|(_, x)| x)
    }

    /// Per array element of `id`, out of `elements`, the packed bits some
    /// process drives.
    pub fn driven_masks(&self, id: VarId, elements: usize) -> Vec<BigUint> {
        element_masks(self.writes(id).filter(|x| x.driving), elements)
    }

    /// Per array element of `id`, out of `elements`, the packed bits some
    /// process writes, including writes that do not drive it.
    pub fn written_masks(&self, id: VarId, elements: usize) -> Vec<BigUint> {
        element_masks(self.writes(id), elements)
    }
}

fn element_masks<'a>(
    writes: impl Iterator<Item = &'a RegionWrite>,
    elements: usize,
) -> Vec<BigUint> {
    let mut ret = vec![BigUint::default(); elements];
    for write in writes {
        let end = write.array.end().unwrap_or(usize::MAX).min(elements);
        let mask = write.packed.mask();
        for x in ret.iter_mut().take(end).skip(write.array.start) {
            *x |= &mask;
        }
    }
    ret
}

/// Walks the statements of one process.
struct Collector {
    writes: Vec<RegionWrite>,
    driving: bool,
}

impl Collector {
    fn new(driving: bool) -> Self {
        Self {
            writes: Vec::new(),
            driving,
        }
    }

    fn process(context: &mut Context, statements: &[Statement]) -> ProcessWrites {
        let mut collector = Self::new(true);
        collector.add_statements(context, statements);
        collector.finish()
    }

    fn finish(self) -> ProcessWrites {
        ProcessWrites {
            writes: self.writes,
        }
    }
    fn add(&mut self, context: &mut Context, dst: &AssignDestination) {
        for (array, packed) in dst_writes(dst, context) {
            self.writes.push(RegionWrite {
                id: dst.id,
                array,
                packed,
                driving: self.driving,
                token: dst.token,
            });
        }
    }

    fn add_statements(&mut self, context: &mut Context, statements: &[Statement]) {
        for statement in statements {
            match statement {
                Statement::Assign(x) => {
                    self.add_expression(context, &x.expr);
                    for dst in &x.dst {
                        self.add(context, dst);
                    }
                }
                Statement::If(x) => {
                    self.add_expression(context, &x.cond);
                    // An untaken constant branch is dead code.
                    if x.cond.comptime().is_const
                        && let Some(value) = x.cond.eval_value(context)
                    {
                        let side = if value.is_true() {
                            &x.true_side
                        } else {
                            &x.false_side
                        };
                        self.add_statements(context, side);
                    } else {
                        self.add_statements(context, &x.true_side);
                        self.add_statements(context, &x.false_side);
                    }
                }
                Statement::IfReset(x) => {
                    self.add_statements(context, &x.true_side);
                    self.add_statements(context, &x.false_side);
                }
                Statement::Case(x) => {
                    self.add_expression(context, &x.case_target);
                    for arm in &x.arms {
                        self.add_statements(context, &arm.body);
                    }
                    self.add_statements(context, &x.default);
                }
                // The iterator is not a static index in any write.
                Statement::For(x) => self.add_statements(context, &x.body),
                Statement::FunctionCall(x) => self.add_function_call(context, x),
                Statement::TbMethodCall(x) => {
                    if let Some(dst) = &x.ret {
                        self.add(context, dst);
                    }
                }
                Statement::SystemFunctionCall(x) => {
                    self.add_system_function_call(context, x);
                }
                Statement::Break | Statement::Unsupported(_) | Statement::Null => {}
            }
        }
    }

    /// Output arguments of function calls and `$readmemh` inside an expression.
    fn add_expression(&mut self, context: &mut Context, expr: &Expression) {
        match expr {
            Expression::Term(x) => match x.as_ref() {
                Factor::Variable(_, index, _, _) => {
                    for x in index.expressions() {
                        self.add_expression(context, x);
                    }
                }
                Factor::FunctionCall(x) => self.add_function_call(context, x),
                Factor::SystemFunctionCall(x) => {
                    self.add_system_function_call(context, x);
                }
                _ => {}
            },
            Expression::Unary(_, x, _) => self.add_expression(context, x),
            Expression::Binary(x, _, y, _) => {
                self.add_expression(context, x);
                self.add_expression(context, y);
            }
            Expression::Ternary(x, y, z, _) => {
                self.add_expression(context, x);
                self.add_expression(context, y);
                self.add_expression(context, z);
            }
            Expression::Concatenation(x, _) => {
                for (x, y) in x {
                    self.add_expression(context, x);
                    if let Some(y) = y {
                        self.add_expression(context, y);
                    }
                }
            }
            Expression::StructConstructor(_, exprs, _) => {
                for (_, x) in exprs {
                    self.add_expression(context, x);
                }
            }
            // Expanded during conversion.
            Expression::ArrayLiteral(_, _) => {}
        }
    }

    fn add_function_call(&mut self, context: &mut Context, call: &FunctionCall) {
        for x in call.inputs.values() {
            self.add_expression(context, x);
        }
        for dst in call.outputs.values().flatten() {
            self.add(context, dst);
        }
    }

    fn add_system_function_call(&mut self, context: &mut Context, call: &SystemFunctionCall) {
        if let SystemFunctionKind::Readmemh(_, x) = &call.kind {
            for dst in x.local() {
                self.add(context, dst);
            }
        }
    }
}

fn instance_processes(
    context: &mut Context,
    inst: &InstDeclaration,
    processes: &mut Vec<ProcessWrites>,
) {
    // Each output port is driven by its own continuous assignment.
    for output in &inst.outputs {
        let mut collector = Collector::new(true);
        for dst in &output.dst {
            collector.add(context, dst);
        }
        processes.push(collector.finish());
    }

    match inst.component.as_ref() {
        Component::SystemVerilog(x) => {
            let mut collector = Collector::new(false);
            for dst in &x.connects {
                collector.add(context, dst);
            }
            processes.push(collector.finish());
        }
        Component::Module(_) => processes.extend(lifted_processes(context, inst, |_, _| true)),
        Component::Interface(_) => {}
    }
}

/// The processes of a child module mapped through the interface bindings of
/// its instance onto the parent variables, for the child variables `members`
/// accepts. Child processes stay separate: two of them may drive the same
/// parent region through aliased bindings.
pub(crate) fn lifted_processes(
    context: &mut Context,
    inst: &InstDeclaration,
    members: impl Fn(&Module, VarId) -> bool,
) -> Vec<ProcessWrites> {
    let Component::Module(child) = inst.component.as_ref() else {
        return vec![];
    };
    let bindings: Vec<_> = inst
        .interface_bindings
        .iter()
        .filter(|x| members(child, x.child))
        .collect();
    let summary = &child.write_summary;
    let mut processes = vec![ProcessWrites::default(); summary.processes.len()];
    for binding in &bindings {
        for (process, write) in summary.process_writes(binding.child) {
            lift(context, binding, write, inst.token, &mut processes[process]);
        }
    }
    processes
}

/// Map a child write through a binding onto the parent variable.
fn lift(
    context: &mut Context,
    binding: &InstInterfaceBinding,
    write: &RegionWrite,
    token: TokenRange,
    process: &mut ProcessWrites,
) {
    let Some(parent) = context.get_variable_info(binding.parent) else {
        return;
    };
    let array = &parent.r#type.array;
    let Some((_, shift)) = binding.select.eval_value(context, &parent.r#type, false) else {
        return;
    };
    let Some(packed) = write.packed.translated(0, shift) else {
        return;
    };
    let selection = if binding.index.is_const() {
        binding.index.eval_selection(context, array)
    } else {
        None
    };
    let target = if let Some(selection) = selection {
        let selected = ArraySpan {
            start: selection.result_start,
            length: selection.length,
        };
        write
            .array
            .intersection(selected)
            .and_then(|x| x.translated(selection.result_start, selection.source_start))
    } else {
        // A dynamic binding may select any element of its range.
        binding
            .index
            .read_range(context, array)
            .and_then(|(start, end)| {
                Some(ArraySpan {
                    start,
                    length: end.checked_sub(start)?.checked_add(1)?,
                })
            })
    };
    if let Some(array) = target {
        process.writes.push(RegionWrite {
            id: binding.parent,
            array,
            packed,
            driving: write.driving,
            token,
        });
    }
}
