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
    for (event, stmts) in event_statements.iter_mut() {
        if !(event.is_initial() || *event == Event::Final) {
            continue;
        }
        resolve_stmts(stmts, context, children)?;
    }
    Ok(())
}

/// Walk a statement list. A `HierAssign` can expand into more than one store
/// (a dual-slot FF takes both its current and its next), so the list is
/// spliced rather than mapped one-to-one.
fn resolve_stmts(
    stmts: &mut Vec<ProtoStatement>,
    context: &mut Context,
    children: &[ModuleVariableMeta],
) -> Result<(), SimulatorError> {
    let mut i = 0;
    while i < stmts.len() {
        if matches!(stmts[i], ProtoStatement::HierAssign(_)) {
            let ProtoStatement::HierAssign(hier) =
                std::mem::replace(&mut stmts[i], ProtoStatement::Break)
            else {
                unreachable!("just matched")
            };
            let expanded = resolve_hier_assign(*hier, context, children)?;
            let n = expanded.len();
            stmts.splice(i..=i, expanded);
            i += n;
        } else {
            resolve_stmt(&mut stmts[i], context, children)?;
            i += 1;
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
            resolve_stmts(&mut x.true_side, context, children)?;
            resolve_stmts(&mut x.false_side, context, children)?;
        }
        ProtoStatement::Case(x) => {
            for arm in &mut x.arms {
                resolve_expr(&mut arm.cond, context, children)?;
                resolve_stmts(&mut arm.body, context, children)?;
            }
            resolve_stmts(&mut x.default, context, children)?;
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
            resolve_stmts(&mut x.body, context, children)?;
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
            ProtoSystemFunctionCall::Readmemh {
                elements,
                width,
                hier,
                ..
            } => {
                if let Some(target) = hier.take() {
                    let Some(meta) = find_target(children, &target.inst_path, &target.var_path)
                    else {
                        return Err(SimulatorError::unsupported_description(&target.token));
                    };
                    *width = meta.width;
                    *elements = crate::ir::statement::readmemh_elements(meta);
                }
            }
            ProtoSystemFunctionCall::Finish => {}
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
            crate::ir::statement::ProtoTbMethodKind::RandomSeed { value } => {
                resolve_expr(value, context, children)?;
            }
            crate::ir::statement::ProtoTbMethodKind::RandomGetRange { min, max, .. } => {
                resolve_expr(min, context, children)?;
                resolve_expr(max, context, children)?;
            }
            crate::ir::statement::ProtoTbMethodKind::FileOpen { .. }
            | crate::ir::statement::ProtoTbMethodKind::FileClose
            | crate::ir::statement::ProtoTbMethodKind::FileFlush
            | crate::ir::statement::ProtoTbMethodKind::RandomGet { .. }
            | crate::ir::statement::ProtoTbMethodKind::RandomGetSeed { .. } => {}
        },
        ProtoStatement::SequentialBlock(stmts) => {
            resolve_stmts(stmts, context, children)?;
        }
        // Compiled blocks come from child-module internals, which cannot
        // contain hierarchical references.
        ProtoStatement::CompiledBlock(_) | ProtoStatement::Break => {}
        // Expanded by `resolve_stmts`, which owns the enclosing list.
        ProtoStatement::HierAssign(_) => unreachable!("handled by resolve_stmts"),
    }
    Ok(())
}

/// Rewrite a testbench write into a child instance to plain assignments on the
/// target's own storage. The read-side twin lives in [`resolve_expr`], and the
/// two share `find_target`.
///
/// Returns two statements when an `initial` block preloads an FF: the local
/// path gets its next-value copy from `append_ff_next_copies`, which runs long
/// before the instance tree gives this destination an offset.
fn resolve_hier_assign(
    hier: crate::ir::statement::ProtoHierAssign,
    context: &mut Context,
    children: &[ModuleVariableMeta],
) -> Result<Vec<ProtoStatement>, SimulatorError> {
    use crate::ir::statement::{ProtoAssignDynamicStatement, ProtoAssignStatement};

    let token = &hier.token;
    let Some(meta) = find_target(children, &hier.inst_path, &hier.var_path) else {
        return Err(SimulatorError::unsupported_description(token));
    };
    let dst_width = meta.width;

    // Evaluating a runtime select eagerly resolves the unknown index to 0,
    // silently writing the wrong bits; the read side branches the same way.
    let need_dynamic_select = !hier.select.is_empty() && !hier.select.is_const();
    let select = {
        let scope = context.scope();
        if hier.select.is_empty() || need_dynamic_select {
            None
        } else {
            Some(
                hier.select
                    .eval_value(&mut scope.analyzer_context, &meta.r#type, false)
                    .ok_or_else(|| SimulatorError::unsupported_description(token))?,
            )
        }
    };
    let dynamic_select = if need_dynamic_select {
        let width_shape = meta.r#type.width().clone();
        let kind_width = meta.r#type.kind.width().unwrap_or(1);
        let mut sel = crate::ir::expression::build_dynamic_bit_select(
            context,
            &width_shape,
            &hier.select,
            kind_width,
            !meta.r#type.kind.is_enum(),
        )?;
        // The select may itself reach into the hierarchy (`dut.mem[0][dut.i]`).
        resolve_expr(&mut sel.index_expr, context, children)?;
        Some(sel)
    } else {
        None
    };

    // This node sits behind the walk that would have descended into it, so the
    // RHS is resolved here (`dut.u_ram.mem[0] = dut.u_other.sig`).
    let mut expr = hier.expr;
    resolve_expr(&mut expr, context, children)?;
    crate::ir::statement::size_literal_rhs(
        &mut expr,
        select,
        dynamic_select.as_ref().map(|d| d.window),
        dst_width,
    );

    // A hierarchical write only ever comes from a testbench block, never from
    // an `always_ff`, so it is a blocking store into the target's CURRENT
    // value, not a next-value update. A dual-slot element gets its `next`
    // written too, which is what `append_ff_next_copies` does for a local
    // destination in the same block, and `$readmemh` for a preload: between
    // events the two slots hold one value.
    if hier.index.is_const() {
        let element = {
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
            meta.elements
                .get(elem_index)
                .cloned()
                .ok_or_else(|| SimulatorError::unsupported_description(token))?
        };

        let is_ff = element.is_ff();
        let current_offset = element.current_offset();
        let assign = |dst: VarOffset, expr: ProtoExpression| {
            ProtoStatement::Assign(ProtoAssignStatement {
                dst,
                dst_width,
                select,
                dynamic_select: dynamic_select.clone(),
                rhs_select: None,
                expr,
                dst_ff_current_offset: if is_ff { current_offset } else { 0 },
                comb_direct: false,
                token: *token,
            })
        };

        if !is_ff {
            return Ok(vec![assign(VarOffset::Comb(current_offset), expr)]);
        }
        // A packed FF keeps `next` as a sentinel equal to `current`; writing
        // it twice would just be the same store.
        if element.next_offset == current_offset {
            return Ok(vec![assign(VarOffset::Ff(current_offset), expr)]);
        }
        Ok(vec![
            assign(VarOffset::Ff(current_offset), expr.clone()),
            assign(VarOffset::Ff(element.next_offset), expr),
        ])
    } else {
        let (base_current, base_next, stride, is_ff) = meta
            .dynamic_index_info()
            .ok_or_else(|| SimulatorError::unsupported_description(token))?;
        let num_elements = meta.elements.len();
        let array_shape = meta.r#type.array.clone();
        let mut index_expr =
            crate::ir::expression::build_linear_index_expr(context, &array_shape, &hier.index)?;
        // Likewise for a hierarchical reference nested in the index
        // (`dut.mem[dut.other.sig] = x`).
        resolve_expr(&mut index_expr, context, children)?;

        let assign = |dst_base: VarOffset, expr: ProtoExpression| {
            ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
                dst_base,
                dst_stride: stride,
                dst_num_elements: num_elements,
                dst_index_expr: index_expr.clone(),
                dst_width,
                select,
                dynamic_select: dynamic_select.clone(),
                rhs_select: None,
                expr,
                dst_ff_current_base_offset: if is_ff { base_current } else { 0 },
                comb_direct: false,
            })
        };

        if !is_ff {
            return Ok(vec![assign(VarOffset::Comb(base_current), expr)]);
        }
        if base_next == base_current {
            return Ok(vec![assign(VarOffset::Ff(base_current), expr)]);
        }
        Ok(vec![
            assign(VarOffset::Ff(base_current), expr.clone()),
            assign(VarOffset::Ff(base_next), expr),
        ])
    }
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

            // Evaluating a runtime select eagerly resolves the unknown index
            // to 0, silently reading the wrong element.
            let need_dynamic_select = !hier.select.is_empty() && !hier.select.is_const();
            let select_val = {
                let scope = context.scope();
                if hier.select.is_empty() || need_dynamic_select {
                    None
                } else {
                    Some(
                        hier.select
                            .eval_value(&mut scope.analyzer_context, &meta.r#type, false)
                            .ok_or_else(|| SimulatorError::unsupported_description(token))?,
                    )
                }
            };
            let dynamic_select = if need_dynamic_select {
                let width_shape = meta.r#type.width().clone();
                let kind_width = meta.r#type.kind.width().unwrap_or(1);
                let mut sel = crate::ir::expression::build_dynamic_bit_select(
                    context,
                    &width_shape,
                    &hier.select,
                    kind_width,
                    !meta.r#type.kind.is_enum(),
                )?;
                // The index may itself reach into the hierarchy (`dut.a[dut.i]`).
                resolve_expr(&mut sel.index_expr, context, children)?;
                Some(sel)
            } else {
                None
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
                    dynamic_select,
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
                    dynamic_select,
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
