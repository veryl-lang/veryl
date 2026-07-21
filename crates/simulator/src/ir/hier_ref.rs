//! Resolution of hierarchical testbench references (`dut.u_core.pc`):
//! rewrites `ProtoExpression::HierVariable` placeholders into plain
//! `Variable`s once the `ModuleVariableMeta` tree is assembled, so every
//! later stage (optimization, DCE, backends, runtime) sees only ordinary
//! variables.

use crate::HashMap;
use crate::ir::ProtoStatement;
use crate::ir::context::Context;
use crate::ir::event::Event;
use crate::ir::expression::ProtoExpression;
use crate::ir::statement::{ProtoForBound, ProtoForRange, ProtoSystemFunctionCall};
use crate::ir::variable::{ModuleVariableMeta, VarOffset, VariableMeta};
use crate::simulator_error::SimulatorError;
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::StrId;

pub fn resolve_hier_refs(
    context: &mut Context,
    event_statements: &mut HashMap<Event, Vec<ProtoStatement>>,
    children: &[ModuleVariableMeta],
) -> Result<(), SimulatorError> {
    // The analyzer emits hierarchical references only inside initial/final
    // blocks; skipping RTL events also keeps this recursive walk away from
    // arbitrarily deep synthesizable expressions.
    for event in [Event::Initial, Event::Final] {
        if let Some(stmts) = event_statements.get_mut(&event) {
            for stmt in stmts.iter_mut() {
                resolve_stmt(stmt, context, children)?;
            }
        }
    }
    Ok(())
}

fn find_target<'a>(
    children: &'a [ModuleVariableMeta],
    inst_path: &[StrId],
    var_path: &air::VarPath,
) -> Option<&'a VariableMeta> {
    let mut level = children;
    let mut module: Option<&ModuleVariableMeta> = None;
    // Walk inst_path, consuming each node's qualified path (prefix then name).
    let mut i = 0;
    while i < inst_path.len() {
        let mut consumed = 0;
        let found = level.iter().find(|m| {
            match air::qualified_prefix_len(&m.hierarchy, m.name, &inst_path[i..]) {
                Some(n) => {
                    consumed = n;
                    true
                }
                None => false,
            }
        })?;
        i += consumed;
        module = Some(found);
        level = &found.children;
    }
    module?.variable_meta.values().find(|m| m.path == *var_path)
}

fn resolve_stmt(
    stmt: &mut ProtoStatement,
    context: &mut Context,
    children: &[ModuleVariableMeta],
) -> Result<(), SimulatorError> {
    match stmt {
        ProtoStatement::Assign(x) => {
            resolve_expr(&mut x.expr, context, children)?;
            if let Some(dyn_sel) = &mut x.dynamic_select {
                resolve_expr(&mut dyn_sel.index_expr, context, children)?;
            }
        }
        ProtoStatement::AssignDynamic(x) => {
            resolve_expr(&mut x.dst_index_expr, context, children)?;
            resolve_expr(&mut x.expr, context, children)?;
            if let Some(dyn_sel) = &mut x.dynamic_select {
                resolve_expr(&mut dyn_sel.index_expr, context, children)?;
            }
        }
        ProtoStatement::If(x) => {
            if let Some(cond) = &mut x.cond {
                resolve_expr(cond, context, children)?;
            }
            for s in &mut x.true_side {
                resolve_stmt(s, context, children)?;
            }
            for s in &mut x.false_side {
                resolve_stmt(s, context, children)?;
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &mut x.arms {
                resolve_expr(&mut arm.cond, context, children)?;
                for s in &mut arm.body {
                    resolve_stmt(s, context, children)?;
                }
            }
            for s in &mut x.default {
                resolve_stmt(s, context, children)?;
            }
        }
        ProtoStatement::For(x) => {
            let (start, end) = match &mut x.range {
                ProtoForRange::Forward { start, end, .. }
                | ProtoForRange::Reverse { start, end, .. }
                | ProtoForRange::Stepped { start, end, .. } => (start, end),
            };
            for bound in [start, end] {
                if let ProtoForBound::Dynamic(expr) = bound {
                    resolve_expr(expr, context, children)?;
                }
            }
            for s in &mut x.body {
                resolve_stmt(s, context, children)?;
            }
        }
        ProtoStatement::SystemFunctionCall(x) => match x {
            ProtoSystemFunctionCall::Display { args, .. }
            | ProtoSystemFunctionCall::Write { args, .. } => {
                for arg in args {
                    resolve_expr(arg, context, children)?;
                }
            }
            ProtoSystemFunctionCall::Assert {
                condition, args, ..
            } => {
                resolve_expr(condition, context, children)?;
                for arg in args {
                    resolve_expr(arg, context, children)?;
                }
            }
            ProtoSystemFunctionCall::Readmemh { .. } | ProtoSystemFunctionCall::Finish => {}
        },
        ProtoStatement::TbMethodCall { method, .. } => match method {
            crate::ir::statement::ProtoTbMethodKind::ClockNext { count, period } => {
                for expr in [count, period].into_iter().flatten() {
                    resolve_expr(expr, context, children)?;
                }
            }
            crate::ir::statement::ProtoTbMethodKind::ResetAssert { duration, .. } => {
                if let Some(expr) = duration {
                    resolve_expr(expr, context, children)?;
                }
            }
            crate::ir::statement::ProtoTbMethodKind::FileWrite { args, .. } => {
                for arg in args {
                    resolve_expr(arg, context, children)?;
                }
            }
            crate::ir::statement::ProtoTbMethodKind::Component { args, .. } => {
                for arg in args {
                    if let crate::ir::statement::ProtoComponentArg::Expr(e) = arg {
                        resolve_expr(e, context, children)?;
                    }
                }
            }
            crate::ir::statement::ProtoTbMethodKind::FileOpen { .. }
            | crate::ir::statement::ProtoTbMethodKind::FileClose
            | crate::ir::statement::ProtoTbMethodKind::FileFlush => {}
        },
        ProtoStatement::SequentialBlock(stmts) => {
            for s in stmts {
                resolve_stmt(s, context, children)?;
            }
        }
        // Compiled blocks come from child-module internals, which cannot
        // contain hierarchical references.
        ProtoStatement::CompiledBlock(_) | ProtoStatement::Break => {}
    }
    Ok(())
}

pub(crate) fn resolve_expr(
    expr: &mut ProtoExpression,
    context: &mut Context,
    children: &[ModuleVariableMeta],
) -> Result<(), SimulatorError> {
    match expr {
        ProtoExpression::HierVariable(hier) => {
            let token = &hier.token;
            let Some(meta) = find_target(children, &hier.inst_path, &hier.var_path) else {
                return Err(SimulatorError::unsupported_description(token));
            };

            let kind_width = meta.r#type.kind.width().unwrap_or(1);
            let var_full_width = kind_width
                * meta
                    .r#type
                    .width()
                    .iter()
                    .map(|d| d.unwrap_or(1))
                    .product::<usize>();

            // A dynamic bit-select on a hierarchical reference is unsupported.
            let select_val = {
                let scope = context.scope();
                if hier.select.is_empty() {
                    None
                } else {
                    Some(
                        hier.select
                            .eval_value(&mut scope.analyzer_context, &meta.r#type, false)
                            .ok_or_else(|| SimulatorError::unsupported_description(token))?,
                    )
                }
            };

            if hier.index.is_const() {
                let scope = context.scope();
                let idx_vals = hier
                    .index
                    .eval_value(&mut scope.analyzer_context)
                    .ok_or_else(|| SimulatorError::unsupported_description(token))?;
                let elem_index = meta
                    .r#type
                    .array
                    .calc_index(&idx_vals)
                    .ok_or_else(|| SimulatorError::unsupported_description(token))?;
                let element = meta
                    .elements
                    .get(elem_index)
                    .ok_or_else(|| SimulatorError::unsupported_description(token))?;

                *expr = ProtoExpression::Variable {
                    var_offset: element.current,
                    select: select_val,
                    dynamic_select: None,
                    width: hier.width,
                    var_full_width,
                    expr_context: hier.expr_context,
                };
            } else {
                // Runtime index: mirror the non-hierarchical dynamic path.
                let (base_current, stride, is_ff) = meta
                    .dynamic_index_info()
                    .map(|(base_current, _base_next, stride, is_ff)| (base_current, stride, is_ff))
                    .ok_or_else(|| SimulatorError::unsupported_description(token))?;
                let num_elements = meta.elements.len();
                let element_native_bytes = meta.native_bytes;
                let array_shape = meta.r#type.array.clone();
                let mut index_proto = crate::ir::expression::build_linear_index_expr(
                    context,
                    &array_shape,
                    &hier.index,
                )?;
                // This node is already behind the walk, so a hierarchical
                // reference nested in the index (`mem[dut.other.sig]`) is
                // resolved explicitly here.
                resolve_expr(&mut index_proto, context, children)?;

                *expr = ProtoExpression::DynamicVariable {
                    base_offset: VarOffset::new(is_ff, base_current),
                    stride,
                    element_native_bytes,
                    index_expr: Box::new(index_proto),
                    num_elements,
                    select: select_val,
                    dynamic_select: None,
                    width: hier.width,
                    expr_context: hier.expr_context,
                };
            }
        }
        ProtoExpression::Variable { dynamic_select, .. } => {
            if let Some(dyn_sel) = dynamic_select {
                resolve_expr(&mut dyn_sel.index_expr, context, children)?;
            }
        }
        ProtoExpression::DynamicVariable {
            index_expr,
            dynamic_select,
            ..
        } => {
            resolve_expr(index_expr, context, children)?;
            if let Some(dyn_sel) = dynamic_select {
                resolve_expr(&mut dyn_sel.index_expr, context, children)?;
            }
        }
        ProtoExpression::Unary { x, .. } => {
            resolve_expr(x, context, children)?;
        }
        ProtoExpression::Binary { x, y, .. } => {
            resolve_expr(x, context, children)?;
            resolve_expr(y, context, children)?;
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            resolve_expr(cond, context, children)?;
            resolve_expr(true_expr, context, children)?;
            resolve_expr(false_expr, context, children)?;
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (e, _, _) in elements {
                resolve_expr(e, context, children)?;
            }
        }
        ProtoExpression::Value { .. } => {}
    }
    Ok(())
}
