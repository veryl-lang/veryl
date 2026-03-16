use crate::HashMap;
use crate::HashSet;
use crate::ir::expression::ProtoExpression;
use crate::ir::statement::{
    ProtoAssignStatement, ProtoIfStatement, ProtoStatement, ProtoSystemFunctionCall,
};

type CombKey = (bool, isize); // (is_ff, offset)

/// Count how many times each variable offset is read within an expression.
fn count_expr_reads(expr: &ProtoExpression, counts: &mut HashMap<CombKey, usize>) {
    match expr {
        ProtoExpression::Variable { offset, is_ff, .. } => {
            *counts.entry((*is_ff, *offset)).or_insert(0) += 1;
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
                *counts.entry((x.dst_is_ff, x.dst_offset)).or_insert(0) += 1;
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
            ProtoSystemFunctionCall::Display { args, .. } => {
                for arg in args {
                    count_expr_reads(arg, counts);
                }
            }
            ProtoSystemFunctionCall::Readmemh { .. } => {}
        },
        ProtoStatement::CompiledBlock(x) => {
            for (is_ff, off) in &x.input_offsets {
                *counts.entry((*is_ff, *off)).or_insert(0) += 1;
            }
        }
    }
}

/// Replace Variable references in an expression using the inline map.
fn substitute_expr(
    expr: ProtoExpression,
    inline_map: &HashMap<CombKey, ProtoExpression>,
) -> ProtoExpression {
    match expr {
        ProtoExpression::Variable {
            offset,
            is_ff,
            select,
            width,
            expr_context,
        } => {
            if let Some(inlined) = inline_map.get(&(is_ff, offset)) {
                if select.is_none() {
                    // Direct substitution
                    inlined.clone()
                } else {
                    // Cannot inline if the consumer applies a bit-select;
                    // keep the original variable reference
                    ProtoExpression::Variable {
                        offset,
                        is_ff,
                        select,
                        width,
                        expr_context,
                    }
                }
            } else {
                ProtoExpression::Variable {
                    offset,
                    is_ff,
                    select,
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
            is_ff,
            index_expr,
            num_elements,
            select,
            width,
            expr_context,
        } => ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            is_ff,
            index_expr: Box::new(substitute_expr(*index_expr, inline_map)),
            num_elements,
            select,
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

/// Optimize combinational statements using dead code elimination and expression inlining.
///
/// `comb_stmts` must be in topological (dependency) order.
/// `event_stmts` are the event statements that consume comb values.
/// `observable_comb` are comb offsets that are externally visible (ports, named variables)
/// and must NOT be eliminated or inlined away.
pub fn optimize_comb(
    comb_stmts: Vec<ProtoStatement>,
    event_stmts: &HashMap<super::Event, Vec<ProtoStatement>>,
    observable_comb: &HashSet<isize>,
) -> Vec<ProtoStatement> {
    // Step 1: Count reads of each variable across all comb and event statements
    let mut read_counts: HashMap<CombKey, usize> = HashMap::default();
    for stmt in &comb_stmts {
        count_stmt_reads(stmt, &mut read_counts);
    }
    for stmts in event_stmts.values() {
        for stmt in stmts {
            count_stmt_reads(stmt, &mut read_counts);
        }
    }

    // Step 2: Collect comb offsets that are read by CompiledBlock (native code
    // that reads directly from the buffer, so we cannot remove the store).
    let mut compiled_block_reads = HashSet::default();
    for stmt in &comb_stmts {
        if let ProtoStatement::CompiledBlock(x) = stmt {
            for (is_ff, off) in &x.input_offsets {
                if !*is_ff {
                    compiled_block_reads.insert(*off);
                }
            }
        }
    }
    for stmts in event_stmts.values() {
        for stmt in stmts {
            if let ProtoStatement::CompiledBlock(x) = stmt {
                for (is_ff, off) in &x.input_offsets {
                    if !*is_ff {
                        compiled_block_reads.insert(*off);
                    }
                }
            }
        }
    }

    // Step 3: Process comb statements in topological order.
    // - Apply inlined expressions to each statement
    // - If output is never read AND not observable, eliminate it (DCE)
    // - If output is single-use within comb AND not observable, inline it
    let mut inline_map: HashMap<CombKey, ProtoExpression> = HashMap::default();
    let mut result: Vec<ProtoStatement> = Vec::new();
    let mut dce_count = 0usize;
    let mut inline_count = 0usize;

    for stmt in comb_stmts {
        // Apply pending substitutions
        let stmt = substitute_stmt(stmt, &inline_map);

        match &stmt {
            ProtoStatement::Assign(x) if !x.dst_is_ff && x.select.is_none() => {
                let key: CombKey = (false, x.dst_offset);
                let is_observable = observable_comb.contains(&x.dst_offset)
                    || compiled_block_reads.contains(&x.dst_offset);
                let count = read_counts.get(&key).copied().unwrap_or(0);

                if count == 0 && !is_observable {
                    // Dead code: output never read and not externally visible
                    dce_count += 1;
                    continue;
                }

                if count == 1 && !is_observable {
                    // Single-use, not externally visible: inline the expression
                    inline_map.insert(key, x.expr.clone());
                    inline_count += 1;
                    continue;
                }

                result.push(stmt);
            }
            _ => {
                result.push(stmt);
            }
        }
    }

    if dce_count > 0 || inline_count > 0 {
        eprintln!(
            "DFG optimize: {} dead eliminated, {} inlined ({} → {} stmts)",
            dce_count,
            inline_count,
            dce_count + inline_count + result.len(),
            result.len()
        );
    }

    result
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

    // Process comb statements: DCE + inline
    let mut inline_map: HashMap<CombKey, ProtoExpression> = HashMap::default();
    let mut result_comb: Vec<ProtoStatement> = Vec::new();
    let mut dce_count = 0usize;
    let mut inline_count = 0usize;

    for stmt in comb_stmts {
        let stmt = substitute_stmt(stmt, &inline_map);

        match &stmt {
            ProtoStatement::Assign(x) if !x.dst_is_ff && x.select.is_none() => {
                let key: CombKey = (false, x.dst_offset);
                let is_external = external_reads.contains(&x.dst_offset);
                let count = read_counts.get(&key).copied().unwrap_or(0);

                if count == 0 && !is_external {
                    dce_count += 1;
                    continue;
                }

                if count == 1 && !is_external {
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
        eprintln!(
            "Merged optimize: {} dead eliminated, {} inlined ({} → {} comb stmts)",
            dce_count,
            inline_count,
            dce_count + inline_count + result_comb.len(),
            result_comb.len()
        );
    }

    (result_comb, result_events)
}
