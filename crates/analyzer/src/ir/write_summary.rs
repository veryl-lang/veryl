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
//! Regions are intervals, so the summary is independent of array sizes and
//! bit widths.

use crate::conv::Context;
use crate::conv::checker::portability::allow_initial_assign;
use crate::ir::span::{ArraySpan, PackedSpan, dst_writes};
use crate::ir::{
    AssignDestination, Component, Declaration, Expression, Factor, FunctionCall, InstDeclaration,
    InstInterfaceBinding, Statement, SystemFunctionCall, SystemFunctionKind, VarId,
};
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug, Default)]
pub struct WriteSummary {
    pub processes: Vec<ProcessWrites>,
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
                    processes.push(ProcessWrites::of_statements(context, &x.statements, true));
                }
                Declaration::Ff(x) => {
                    processes.push(ProcessWrites::of_statements(context, &x.statements, true));
                }
                Declaration::Final(x) => {
                    processes.push(ProcessWrites::of_statements(context, &x.statements, true));
                }
                Declaration::Initial(x) => {
                    let mut process = ProcessWrites::of_statements(context, &x.statements, true);
                    for write in &mut process.writes {
                        write.driving = !allow_initial_assign(context, write.id);
                    }
                    processes.push(process);
                }
                Declaration::Inst(x) => instance_processes(context, x, &mut processes),
                Declaration::External(x) => {
                    let mut process = ProcessWrites::default();
                    for dst in x.connects.iter().filter_map(|x| x.output.as_ref()) {
                        process.add(context, dst, false);
                    }
                    processes.push(process);
                }
                Declaration::Unsupported(_) | Declaration::Null => {}
            }
        }
        context.rollback_analysis_transaction();
        processes.retain(|x| !x.writes.is_empty());
        Self { processes }
    }
}

impl ProcessWrites {
    fn of_statements(context: &mut Context, statements: &[Statement], driving: bool) -> Self {
        let mut process = Self::default();
        process.add_statements(context, statements, driving);
        process
    }

    fn add(&mut self, context: &mut Context, dst: &AssignDestination, driving: bool) {
        for (array, packed) in dst_writes(dst, context) {
            self.writes.push(RegionWrite {
                id: dst.id,
                array,
                packed,
                driving,
                token: dst.token,
            });
        }
    }

    fn add_statements(&mut self, context: &mut Context, statements: &[Statement], driving: bool) {
        for statement in statements {
            match statement {
                Statement::Assign(x) => {
                    self.add_expression(context, &x.expr, driving);
                    for dst in &x.dst {
                        self.add(context, dst, driving);
                    }
                }
                Statement::If(x) => {
                    self.add_expression(context, &x.cond, driving);
                    // An untaken constant branch is dead code.
                    if x.cond.comptime().is_const
                        && let Some(value) = x.cond.eval_value(context)
                    {
                        let side = if value.is_true() {
                            &x.true_side
                        } else {
                            &x.false_side
                        };
                        self.add_statements(context, side, driving);
                    } else {
                        self.add_statements(context, &x.true_side, driving);
                        self.add_statements(context, &x.false_side, driving);
                    }
                }
                Statement::IfReset(x) => {
                    self.add_statements(context, &x.true_side, driving);
                    self.add_statements(context, &x.false_side, driving);
                }
                Statement::Case(x) => {
                    self.add_expression(context, &x.case_target, driving);
                    for arm in &x.arms {
                        self.add_statements(context, &arm.body, driving);
                    }
                    self.add_statements(context, &x.default, driving);
                }
                Statement::For(x) => {
                    // Emitted as an SV `for` loop, the iterator is not a
                    // static index: a write covers what its constant prefix
                    // selects, as in the longest static prefix rule.
                    self.add_statements(context, &x.body, driving);
                }
                Statement::FunctionCall(x) => self.add_function_call(context, x, driving),
                Statement::TbMethodCall(x) => {
                    if let Some(dst) = &x.ret {
                        self.add(context, dst, driving);
                    }
                }
                Statement::SystemFunctionCall(x) => {
                    self.add_system_function_call(context, x, driving);
                }
                Statement::Break | Statement::Unsupported(_) | Statement::Null => {}
            }
        }
    }

    /// Output arguments of function calls and `$readmemh` inside an expression.
    fn add_expression(&mut self, context: &mut Context, expr: &Expression, driving: bool) {
        match expr {
            Expression::Term(x) => match x.as_ref() {
                Factor::Variable(_, index, _, _) => {
                    for x in index.expressions() {
                        self.add_expression(context, x, driving);
                    }
                }
                Factor::FunctionCall(x) => self.add_function_call(context, x, driving),
                Factor::SystemFunctionCall(x) => {
                    self.add_system_function_call(context, x, driving);
                }
                _ => {}
            },
            Expression::Unary(_, x, _) => self.add_expression(context, x, driving),
            Expression::Binary(x, _, y, _) => {
                self.add_expression(context, x, driving);
                self.add_expression(context, y, driving);
            }
            Expression::Ternary(x, y, z, _) => {
                self.add_expression(context, x, driving);
                self.add_expression(context, y, driving);
                self.add_expression(context, z, driving);
            }
            Expression::Concatenation(x, _) => {
                for (x, y) in x {
                    self.add_expression(context, x, driving);
                    if let Some(y) = y {
                        self.add_expression(context, y, driving);
                    }
                }
            }
            Expression::StructConstructor(_, exprs, _) => {
                for (_, x) in exprs {
                    self.add_expression(context, x, driving);
                }
            }
            // Expanded during conversion.
            Expression::ArrayLiteral(_, _) => {}
        }
    }

    fn add_function_call(&mut self, context: &mut Context, call: &FunctionCall, driving: bool) {
        for x in call.inputs.values() {
            self.add_expression(context, x, driving);
        }
        for dst in call.outputs.values().flatten() {
            self.add(context, dst, driving);
        }
    }

    fn add_system_function_call(
        &mut self,
        context: &mut Context,
        call: &SystemFunctionCall,
        driving: bool,
    ) {
        if let SystemFunctionKind::Readmemh(_, x) = &call.kind {
            for dst in x.local() {
                self.add(context, dst, driving);
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
        let mut process = ProcessWrites::default();
        for dst in &output.dst {
            process.add(context, dst, true);
        }
        processes.push(process);
    }

    match inst.component.as_ref() {
        Component::SystemVerilog(x) => {
            let mut process = ProcessWrites::default();
            for dst in &x.connects {
                process.add(context, dst, false);
            }
            processes.push(process);
        }
        Component::Module(child) => {
            // Child processes stay separate: two of them may drive the same
            // parent region through aliased bindings.
            for child_process in &child.write_summary.processes {
                let mut process = ProcessWrites::default();
                for binding in &inst.interface_bindings {
                    for write in child_process
                        .writes
                        .iter()
                        .filter(|x| x.id == binding.child)
                    {
                        lift(context, binding, write, inst.token, &mut process);
                    }
                }
                processes.push(process);
            }
        }
        Component::Interface(_) => {}
    }
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
