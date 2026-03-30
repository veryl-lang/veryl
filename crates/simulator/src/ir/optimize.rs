use crate::HashMap;
use crate::HashSet;
use crate::ir::expression::ProtoExpression;
use crate::ir::statement::{
    ProtoAssignStatement, ProtoIfStatement, ProtoStatement, ProtoSystemFunctionCall,
};
use crate::ir::variable::VarOffset;

type CombKey = VarOffset;

/// Count how many times each variable offset is read within an expression.
fn count_expr_reads(expr: &ProtoExpression, counts: &mut HashMap<CombKey, usize>) {
    match expr {
        ProtoExpression::Variable { var_offset, .. } => {
            *counts.entry(*var_offset).or_insert(0) += 1;
        }
        ProtoExpression::Value { .. } => {}
        ProtoExpression::Unary { x, .. } => count_expr_reads(x, counts),
        ProtoExpression::Binary { x, y, .. } => {
            count_expr_reads(x, counts);
            count_expr_reads(y, counts);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (expr, _, _) in elements {
                count_expr_reads(expr, counts);
            }
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            count_expr_reads(cond, counts);
            count_expr_reads(true_expr, counts);
            count_expr_reads(false_expr, counts);
        }
        ProtoExpression::DynamicVariable { index_expr, .. } => {
            count_expr_reads(index_expr, counts);
            // Dynamic variable reads are not trackable at compile time
        }
    }
}

/// Count reads in a statement (recursive for If blocks).
fn count_stmt_reads(stmt: &ProtoStatement, counts: &mut HashMap<CombKey, usize>) {
    match stmt {
        ProtoStatement::Assign(x) => {
            count_expr_reads(&x.expr, counts);
            // If select is present, the dst is also read (read-modify-write)
            if x.select.is_some() {
                *counts.entry(x.dst).or_insert(0) += 1;
            }
        }
        ProtoStatement::AssignDynamic(x) => {
            count_expr_reads(&x.dst_index_expr, counts);
            count_expr_reads(&x.expr, counts);
        }
        ProtoStatement::If(x) => {
            if let Some(cond) = &x.cond {
                count_expr_reads(cond, counts);
            }
            for s in &x.true_side {
                count_stmt_reads(s, counts);
            }
            for s in &x.false_side {
                count_stmt_reads(s, counts);
            }
        }
        ProtoStatement::SystemFunctionCall(x) => match x {
            ProtoSystemFunctionCall::Display { args, .. }
            | ProtoSystemFunctionCall::Write { args, .. } => {
                for arg in args {
                    count_expr_reads(arg, counts);
                }
            }
            ProtoSystemFunctionCall::Readmemh { .. } => {}
            ProtoSystemFunctionCall::Assert { condition, .. } => {
                count_expr_reads(condition, counts);
            }
            ProtoSystemFunctionCall::Finish => {}
        },
        ProtoStatement::CompiledBlock(x) => {
            for off in &x.input_offsets {
                *counts.entry(*off).or_insert(0) += 1;
            }
        }
        ProtoStatement::TbMethodCall { .. } => {}
    }
}

/// Replace Variable references in an expression using the inline map.
fn substitute_expr(
    expr: ProtoExpression,
    inline_map: &HashMap<CombKey, ProtoExpression>,
) -> ProtoExpression {
    match expr {
        ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select,
            width,
            expr_context,
        } => {
            if let Some(inlined) = inline_map.get(&var_offset) {
                if select.is_none() && dynamic_select.is_none() {
                    // Direct substitution
                    inlined.clone()
                } else {
                    // Cannot inline if the consumer applies a bit-select;
                    // keep the original variable reference
                    ProtoExpression::Variable {
                        var_offset,
                        select,
                        dynamic_select,
                        width,
                        expr_context,
                    }
                }
            } else {
                ProtoExpression::Variable {
                    var_offset,
                    select,
                    dynamic_select,
                    width,
                    expr_context,
                }
            }
        }
        ProtoExpression::Value { .. } => expr,
        ProtoExpression::Unary {
            op,
            x,
            width,
            expr_context,
        } => ProtoExpression::Unary {
            op,
            x: Box::new(substitute_expr(*x, inline_map)),
            width,
            expr_context,
        },
        ProtoExpression::Binary {
            x,
            op,
            y,
            width,
            expr_context,
        } => ProtoExpression::Binary {
            x: Box::new(substitute_expr(*x, inline_map)),
            op,
            y: Box::new(substitute_expr(*y, inline_map)),
            width,
            expr_context,
        },
        ProtoExpression::Concatenation {
            elements,
            width,
            expr_context,
        } => ProtoExpression::Concatenation {
            elements: elements
                .into_iter()
                .map(|(e, repeat, ew)| (Box::new(substitute_expr(*e, inline_map)), repeat, ew))
                .collect(),
            width,
            expr_context,
        },
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            width,
            expr_context,
        } => ProtoExpression::Ternary {
            cond: Box::new(substitute_expr(*cond, inline_map)),
            true_expr: Box::new(substitute_expr(*true_expr, inline_map)),
            false_expr: Box::new(substitute_expr(*false_expr, inline_map)),
            width,
            expr_context,
        },
        ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            index_expr,
            num_elements,
            select,
            dynamic_select,
            width,
            expr_context,
        } => ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            index_expr: Box::new(substitute_expr(*index_expr, inline_map)),
            num_elements,
            select,
            dynamic_select,
            width,
            expr_context,
        },
    }
}

/// Apply substitution to a statement.
fn substitute_stmt(
    stmt: ProtoStatement,
    inline_map: &HashMap<CombKey, ProtoExpression>,
) -> ProtoStatement {
    match stmt {
        ProtoStatement::Assign(x) => ProtoStatement::Assign(ProtoAssignStatement {
            expr: substitute_expr(x.expr, inline_map),
            ..x
        }),
        ProtoStatement::If(x) => ProtoStatement::If(ProtoIfStatement {
            cond: x.cond.map(|c| substitute_expr(c, inline_map)),
            true_side: x
                .true_side
                .into_iter()
                .map(|s| substitute_stmt(s, inline_map))
                .collect(),
            false_side: x
                .false_side
                .into_iter()
                .map(|s| substitute_stmt(s, inline_map))
                .collect(),
        }),
        ProtoStatement::AssignDynamic(x) => {
            let mut x = x;
            x.dst_index_expr = substitute_expr(x.dst_index_expr, inline_map);
            x.expr = substitute_expr(x.expr, inline_map);
            ProtoStatement::AssignDynamic(x)
        }
        _ => stmt,
    }
}

/// Optimize comb statements for a merged comb+event JIT function.
/// Inlines single-use comb variables into event statements,
/// eliminating intermediate stores and loads in the generated code.
///
/// `comb_stmts` must be in topological (dependency) order.
/// `event_stmts` are the event statements that will follow comb in the merged function.
/// `external_reads` are comb offsets read externally (e.g., by output port connections).
pub fn optimize_merged(
    comb_stmts: Vec<ProtoStatement>,
    event_stmts: Vec<ProtoStatement>,
    external_reads: &HashSet<isize>,
) -> (Vec<ProtoStatement>, Vec<ProtoStatement>) {
    // Count reads across ALL statements (comb + event)
    let mut read_counts: HashMap<CombKey, usize> = HashMap::default();
    for stmt in comb_stmts.iter().chain(event_stmts.iter()) {
        count_stmt_reads(stmt, &mut read_counts);
    }

    // Collect comb offsets that are written by non-Assign statements
    // (If/Case blocks). These offsets must not be inlined because an
    // Assign to the same offset is a "default" that gets overridden
    // by the conditional statement at runtime.
    let mut multi_write_offsets: HashSet<isize> = HashSet::default();
    for stmt in &comb_stmts {
        match stmt {
            ProtoStatement::Assign(_) => {}
            _ => {
                let mut outs = vec![];
                let mut ins = vec![];
                stmt.gather_variable_offsets(&mut ins, &mut outs);
                for off in outs {
                    if !off.is_ff() {
                        multi_write_offsets.insert(off.raw());
                    }
                }
            }
        }
    }

    // Process comb statements: DCE + inline
    let mut inline_map: HashMap<CombKey, ProtoExpression> = HashMap::default();
    let mut result_comb: Vec<ProtoStatement> = Vec::new();
    let mut dce_count = 0usize;
    let mut inline_count = 0usize;

    for stmt in comb_stmts {
        let stmt = substitute_stmt(stmt, &inline_map);

        match &stmt {
            ProtoStatement::Assign(x) if !x.dst.is_ff() && x.select.is_none() => {
                let key: CombKey = x.dst;
                let is_external = external_reads.contains(&x.dst.raw());
                let has_override = multi_write_offsets.contains(&x.dst.raw());
                let count = read_counts.get(&key).copied().unwrap_or(0);

                if count == 0 && !is_external && !has_override {
                    dce_count += 1;
                    continue;
                }

                if count == 1 && !is_external && !has_override {
                    inline_map.insert(key, x.expr.clone());
                    inline_count += 1;
                    continue;
                }

                result_comb.push(stmt);
            }
            _ => {
                result_comb.push(stmt);
            }
        }
    }

    // Apply inlining to event statements
    let result_events: Vec<ProtoStatement> = event_stmts
        .into_iter()
        .map(|s| substitute_stmt(s, &inline_map))
        .collect();

    if dce_count > 0 || inline_count > 0 {
        log::debug!(
            "Merged optimize: {} dead eliminated, {} inlined ({} → {} comb stmts)",
            dce_count,
            inline_count,
            dce_count + inline_count + result_comb.len(),
            result_comb.len()
        );
    }

    (result_comb, result_events)
}
