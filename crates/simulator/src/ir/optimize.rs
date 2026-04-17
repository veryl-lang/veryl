use crate::HashMap;
use crate::HashSet;
use crate::ir::expression::ProtoExpression;
use crate::ir::statement::{
    ProtoAssignStatement, ProtoIfStatement, ProtoStatement, ProtoSystemFunctionCall,
};
use crate::ir::variable::VarOffset;

type CombKey = VarOffset;

/// Returns true if the expression compiles to ~1-2 JIT instructions,
/// making duplication cheaper than store + multiple loads.
fn is_cheap_expr(expr: &ProtoExpression) -> bool {
    fn is_leaf(e: &ProtoExpression) -> bool {
        matches!(
            e,
            ProtoExpression::Variable {
                select: None,
                dynamic_select: None,
                ..
            } | ProtoExpression::Value { .. }
        )
    }
    match expr {
        // Variable reference or constant: 0-1 instructions
        _ if is_leaf(expr) => true,
        // Unary op on leaf: 1 instruction
        ProtoExpression::Unary { x, .. } => is_leaf(x),
        // Binary op on two leaves: 1-2 instructions
        ProtoExpression::Binary { x, y, .. } => is_leaf(x) && is_leaf(y),
        _ => false,
    }
}

/// Collect variable offsets that are read in a non-substitutable way
/// (with select or dynamic_select, or as DynamicVariable base).
/// These variables must NOT be inlined because substitute_expr will
/// leave the original Variable reference in place.
fn collect_non_substitutable_reads(expr: &ProtoExpression, pinned: &mut HashSet<CombKey>) {
    match expr {
        ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select,
            ..
        } => {
            if select.is_some() || dynamic_select.is_some() {
                pinned.insert(*var_offset);
            }
        }
        ProtoExpression::Value { .. } => {}
        ProtoExpression::Unary { x, .. } => collect_non_substitutable_reads(x, pinned),
        ProtoExpression::Binary { x, y, .. } => {
            collect_non_substitutable_reads(x, pinned);
            collect_non_substitutable_reads(y, pinned);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (expr, _, _) in elements {
                collect_non_substitutable_reads(expr, pinned);
            }
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            collect_non_substitutable_reads(cond, pinned);
            collect_non_substitutable_reads(true_expr, pinned);
            collect_non_substitutable_reads(false_expr, pinned);
        }
        ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            index_expr,
            num_elements,
            ..
        } => {
            collect_non_substitutable_reads(index_expr, pinned);
            // DynamicVariable base offset is a struct field, not a Variable node,
            // so substitute_expr cannot replace it. Pin the base (and last element)
            // to prevent inlining of assignments to these offsets.
            pinned.insert(*base_offset);
            if *num_elements > 1 {
                let last = VarOffset::new(
                    base_offset.is_ff(),
                    base_offset.raw() + *stride * (*num_elements as isize - 1),
                );
                pinned.insert(last);
            }
        }
    }
}

/// Collect non-substitutable reads from a statement (recursive for If blocks).
fn collect_stmt_non_substitutable(stmt: &ProtoStatement, pinned: &mut HashSet<CombKey>) {
    match stmt {
        ProtoStatement::Assign(x) => {
            collect_non_substitutable_reads(&x.expr, pinned);
            // If select is present, dst is read (read-modify-write) and can't be substituted
            if x.select.is_some() {
                pinned.insert(x.dst);
            }
        }
        ProtoStatement::AssignDynamic(x) => {
            collect_non_substitutable_reads(&x.dst_index_expr, pinned);
            collect_non_substitutable_reads(&x.expr, pinned);
        }
        ProtoStatement::If(x) => {
            if let Some(cond) = &x.cond {
                collect_non_substitutable_reads(cond, pinned);
            }
            for s in &x.true_side {
                collect_stmt_non_substitutable(s, pinned);
            }
            for s in &x.false_side {
                collect_stmt_non_substitutable(s, pinned);
            }
        }
        ProtoStatement::SystemFunctionCall(x) => match x {
            ProtoSystemFunctionCall::Display { args, .. }
            | ProtoSystemFunctionCall::Write { args, .. } => {
                for arg in args {
                    collect_non_substitutable_reads(arg, pinned);
                }
            }
            ProtoSystemFunctionCall::Readmemh { .. } => {}
            ProtoSystemFunctionCall::Assert { condition, .. } => {
                collect_non_substitutable_reads(condition, pinned);
            }
            ProtoSystemFunctionCall::Finish => {}
        },
        ProtoStatement::CompiledBlock(x) => {
            // All CompiledBlock reads are non-substitutable (fixed offsets in JIT code)
            for off in &x.input_offsets {
                pinned.insert(*off);
            }
        }
        ProtoStatement::For(x) => {
            for s in &x.body {
                collect_stmt_non_substitutable(s, pinned);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                collect_stmt_non_substitutable(s, pinned);
            }
        }
        ProtoStatement::Break => {}
        ProtoStatement::TbMethodCall { .. } => {}
    }
}

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
        ProtoStatement::For(x) => {
            for s in &x.body {
                count_stmt_reads(s, counts);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                count_stmt_reads(s, counts);
            }
        }
        ProtoStatement::TbMethodCall { .. } => {}
        ProtoStatement::Break => {}
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
                } else if let ProtoExpression::Variable {
                    var_offset: src_offset,
                    select: None,
                    dynamic_select: None,
                    ..
                } = inlined
                {
                    // Inlined source is a simple Variable with no select/dynamic_select.
                    // Safe to substitute var_offset while keeping the consumer's select.
                    // This covers FF-copy inlines (e.g. child.i_a = parent.a_reg)
                    // where downstream reads child.i_a[63:52].
                    ProtoExpression::Variable {
                        var_offset: *src_offset,
                        select,
                        dynamic_select,
                        width,
                        expr_context,
                    }
                } else {
                    // Cannot inline if the consumer applies a bit-select
                    // and the inlined expression is complex.
                    // Keep the original variable reference.
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
        ProtoStatement::SequentialBlock(body) => ProtoStatement::SequentialBlock(
            body.into_iter()
                .map(|s| substitute_stmt(s, inline_map))
                .collect(),
        ),
        ProtoStatement::SystemFunctionCall(x) => {
            let x = match x {
                ProtoSystemFunctionCall::Display { format_str, args } => {
                    ProtoSystemFunctionCall::Display {
                        format_str,
                        args: args
                            .into_iter()
                            .map(|a| substitute_expr(a, inline_map))
                            .collect(),
                    }
                }
                ProtoSystemFunctionCall::Write { format_str, args } => {
                    ProtoSystemFunctionCall::Write {
                        format_str,
                        args: args
                            .into_iter()
                            .map(|a| substitute_expr(a, inline_map))
                            .collect(),
                    }
                }
                ProtoSystemFunctionCall::Assert { condition, message } => {
                    ProtoSystemFunctionCall::Assert {
                        condition: substitute_expr(condition, inline_map),
                        message,
                    }
                }
                other => other,
            };
            ProtoStatement::SystemFunctionCall(x)
        }
        ProtoStatement::For(x) => {
            let mut x = x;
            x.body = x
                .body
                .into_iter()
                .map(|s| substitute_stmt(s, inline_map))
                .collect();
            ProtoStatement::For(x)
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
        log::info!(
            "Merged optimize: {} dead eliminated, {} inlined ({} → {} comb stmts, {} event stmts)",
            dce_count,
            inline_count,
            dce_count + inline_count + result_comb.len(),
            result_comb.len(),
            result_events.len(),
        );
    }

    (result_comb, result_events)
}

/// Optimize unified comb statements: DCE and expression inlining.
/// `event_reads` contains comb offsets that are read by event statements
/// (these must NOT be eliminated or inlined away).
pub fn optimize_unified(
    comb_stmts: Vec<ProtoStatement>,
    event_reads: &HashSet<isize>,
) -> Vec<ProtoStatement> {
    // Count reads within comb statements only
    let mut read_counts: HashMap<CombKey, usize> = HashMap::default();
    for stmt in &comb_stmts {
        count_stmt_reads(stmt, &mut read_counts);
    }

    // Comb offsets written by non-Assign (If/Case) or by Assign with select
    // (partial bit-write). These must not be inlined away because the full
    // assignment (select: None) and partial bit-writes (select: Some) must
    // all execute to produce the correct final value.
    let mut multi_write_offsets: HashSet<isize> = HashSet::default();
    for stmt in &comb_stmts {
        match stmt {
            ProtoStatement::Assign(x) if x.select.is_some() && !x.dst.is_ff() => {
                // Partial bit-write to a comb variable — mark as multi-write
                multi_write_offsets.insert(x.dst.raw());
            }
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

    // Collect offsets read with select or dynamic_select (non-substitutable).
    // These can only be inlined if the inlined expression is a simple Variable
    // (substitute_expr can forward var_offset with select). Complex expressions
    // (Binary, Unary) can't have select applied, so the original reference is
    // kept but the assignment is removed → undefined value.
    let mut non_substitutable: HashSet<CombKey> = HashSet::default();
    for stmt in &comb_stmts {
        collect_stmt_non_substitutable(stmt, &mut non_substitutable);
    }

    let mut inline_map: HashMap<CombKey, ProtoExpression> = HashMap::default();
    let mut result: Vec<ProtoStatement> = Vec::new();
    let mut dce_count = 0usize;
    let mut inline_count = 0usize;

    for stmt in comb_stmts {
        let stmt = substitute_stmt(stmt, &inline_map);

        match &stmt {
            ProtoStatement::Assign(x) if !x.dst.is_ff() && x.select.is_none() => {
                let key: CombKey = x.dst;
                let is_event_read = event_reads.contains(&x.dst.raw());
                let has_override = multi_write_offsets.contains(&x.dst.raw());
                let count = read_counts.get(&key).copied().unwrap_or(0);

                // DCE: no readers at all (and not read by events)
                if count == 0 && !is_event_read && !has_override {
                    dce_count += 1;
                    continue;
                }

                // Inline: single reader in comb, not read by events
                if count == 1 && !is_event_read && !has_override {
                    inline_map.insert(key, x.expr.clone());
                    inline_count += 1;
                    continue;
                }

                // Inline trivial copies (expr is a Variable with no select)
                // even with multiple readers. FF sources are always safe
                // (immutable during comb eval). Comb sources are safe in
                // single-pass eval (each var written once in dependency order).
                if !is_event_read
                    && !has_override
                    && let ProtoExpression::Variable {
                        var_offset,
                        select: None,
                        dynamic_select: None,
                        ..
                    } = &x.expr
                    && (var_offset.is_ff() || !multi_write_offsets.contains(&var_offset.raw()))
                {
                    inline_map.insert(key, x.expr.clone());
                    inline_count += 1;
                    continue;
                }

                // Inline cheap expressions (1-2 JIT instructions) with up to 3
                // readers. Skip if any reader has select/dynamic_select
                // (substitute_expr can't apply select to complex expressions).
                if count <= 3
                    && !is_event_read
                    && !has_override
                    && !non_substitutable.contains(&key)
                    && is_cheap_expr(&x.expr)
                {
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

    // Count remaining multi-use and event-read variables
    let mut multi_use = 0usize;
    let mut event_blocked = 0usize;
    let mut override_blocked = 0usize;
    for stmt in &result {
        if let ProtoStatement::Assign(x) = stmt
            && !x.dst.is_ff()
            && x.select.is_none()
        {
            let key = x.dst;
            let count = read_counts.get(&key).copied().unwrap_or(0);
            if event_reads.contains(&x.dst.raw()) {
                event_blocked += 1;
            } else if multi_write_offsets.contains(&x.dst.raw()) {
                override_blocked += 1;
            } else if count > 1 {
                multi_use += 1;
            }
        }
    }

    if dce_count > 0 || inline_count > 0 {
        log::info!(
            "Unified comb optimize: {} dead, {} inlined ({} → {} stmts) [remaining: {} multi-use, {} event-read, {} override]",
            dce_count,
            inline_count,
            dce_count + inline_count + result.len(),
            result.len(),
            multi_use,
            event_blocked,
            override_blocked,
        );
    }

    result
}

/// Top-level comb+event optimization in two safe phases:
///
/// Phase 1: `optimize_unified` for comb-only DCE/inlining (event_reads protected).
/// Phase 2: Inline comb vars that are read ONLY by events (comb_count=0) into
///          event expressions, then DCE them from comb. This is safe because
///          these vars have no comb readers — removing them from comb_values
///          only requires that events get the substituted expression.
///
/// Returns optimized comb stmts and event stmts (with comb vars inlined).
pub fn optimize_top_level(
    comb_stmts: Vec<ProtoStatement>,
    event_stmts_grouped: Vec<(usize, Vec<ProtoStatement>)>,
    event_reads: &HashSet<isize>,
) -> (Vec<ProtoStatement>, Vec<(usize, Vec<ProtoStatement>)>) {
    // Phase 1: comb-only optimization (proven correct)
    let comb_stmts = optimize_unified(comb_stmts, event_reads);

    // Phase 2: inline event-only comb vars into events
    // Find comb vars with 0 comb readers that are read by events.
    // These can be inlined into events and removed from comb.

    // Count comb-only reads (excluding events)
    let mut comb_read_counts: HashMap<CombKey, usize> = HashMap::default();
    for stmt in &comb_stmts {
        count_stmt_reads(stmt, &mut comb_read_counts);
    }

    // Collect non-substitutable reads in events
    let mut event_pinned: HashSet<CombKey> = HashSet::default();
    for (_, stmts) in &event_stmts_grouped {
        for s in stmts {
            collect_stmt_non_substitutable(s, &mut event_pinned);
        }
    }

    // Collect comb offsets written by non-Assign (can't inline defaults)
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

    // Collect all comb offsets that are written by Phase 1 output.
    // Only these offsets exist in comb_values at runtime.
    let mut available_comb: HashSet<isize> = HashSet::default();
    for stmt in &comb_stmts {
        let mut ins = vec![];
        let mut outs = vec![];
        stmt.gather_variable_offsets(&mut ins, &mut outs);
        for off in outs {
            if !off.is_ff() {
                available_comb.insert(off.raw());
            }
        }
    }

    // Check if all comb variable references in an expression exist in available_comb.
    fn expr_deps_available(
        expr: &ProtoExpression,
        available: &HashSet<isize>,
        inline_map: &HashMap<CombKey, ProtoExpression>,
    ) -> bool {
        match expr {
            ProtoExpression::Variable { var_offset, .. } => {
                // If this var is already in inline_map, it will be substituted
                if inline_map.contains_key(var_offset) {
                    return true;
                }
                if var_offset.is_ff() {
                    // FF reads are always available
                    true
                } else {
                    // Comb read: must be in available_comb
                    available.contains(&var_offset.raw())
                }
            }
            ProtoExpression::Value { .. } => true,
            ProtoExpression::Unary { x, .. } => expr_deps_available(x, available, inline_map),
            ProtoExpression::Binary { x, y, .. } => {
                expr_deps_available(x, available, inline_map)
                    && expr_deps_available(y, available, inline_map)
            }
            ProtoExpression::Concatenation { elements, .. } => elements
                .iter()
                .all(|(e, _, _)| expr_deps_available(e, available, inline_map)),
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                expr_deps_available(cond, available, inline_map)
                    && expr_deps_available(true_expr, available, inline_map)
                    && expr_deps_available(false_expr, available, inline_map)
            }
            ProtoExpression::DynamicVariable { index_expr, .. } => {
                // DynamicVariable base_offset is pinned separately
                expr_deps_available(index_expr, available, inline_map)
            }
        }
    }

    // Build inline map: comb vars with 0 comb readers, read by events,
    // not pinned, not overridden, and whose expr deps are all available.
    let mut event_inline_map: HashMap<CombKey, ProtoExpression> = HashMap::default();
    let mut event_inline_count = 0usize;
    let mut deps_missing = 0usize;

    // Process in order (topological) so cascading substitution works
    let mut result_comb: Vec<ProtoStatement> = Vec::new();
    for stmt in comb_stmts {
        let stmt = substitute_stmt(stmt, &event_inline_map);

        match &stmt {
            ProtoStatement::Assign(x) if !x.dst.is_ff() && x.select.is_none() => {
                let key: CombKey = x.dst;
                let comb_count = comb_read_counts.get(&key).copied().unwrap_or(0);
                let is_event_read = event_reads.contains(&x.dst.raw());
                let is_pinned = event_pinned.contains(&key);
                let has_override = multi_write_offsets.contains(&x.dst.raw());

                // Only inline vars that have NO comb readers (only event readers)
                if comb_count == 0 && is_event_read && !is_pinned && !has_override {
                    // Verify all comb deps in the expression are available
                    if expr_deps_available(&x.expr, &available_comb, &event_inline_map) {
                        event_inline_map.insert(key, x.expr.clone());
                        // Remove from available_comb since it won't be written anymore
                        available_comb.remove(&key.raw());
                        event_inline_count += 1;
                        continue;
                    } else {
                        deps_missing += 1;
                    }
                }

                result_comb.push(stmt);
            }
            _ => {
                result_comb.push(stmt);
            }
        }
    }

    // Apply event inline map to all event statements
    let result_events: Vec<(usize, Vec<ProtoStatement>)> = event_stmts_grouped
        .into_iter()
        .map(|(key, stmts)| {
            let stmts = stmts
                .into_iter()
                .map(|s| substitute_stmt(s, &event_inline_map))
                .collect();
            (key, stmts)
        })
        .collect();

    if event_inline_count > 0 || deps_missing > 0 {
        log::info!(
            "Event inline: {} comb vars inlined into events, {} skipped (deps missing) ({} → {} comb stmts)",
            event_inline_count,
            deps_missing,
            event_inline_count + result_comb.len(),
            result_comb.len(),
        );
    }

    (result_comb, result_events)
}

/// Check if an expression is safe to evaluate unconditionally (for select).
/// DynamicVariable array accesses with runtime indices are unsafe because
/// the index might be out of range when the branch isn't taken.
fn is_safe_for_select(expr: &ProtoExpression) -> bool {
    match expr {
        ProtoExpression::Variable {
            dynamic_select: None,
            ..
        } => true,
        ProtoExpression::Variable { .. } => false, // has dynamic_select
        ProtoExpression::Value { .. } => true,
        // Complex expressions (Unary, Binary, etc.) are excluded for now.
        // Binary expressions involving bit-select reads can produce incorrect
        // values when evaluated unconditionally in select context.
        ProtoExpression::Unary { .. }
        | ProtoExpression::Binary { .. }
        | ProtoExpression::Concatenation { .. }
        | ProtoExpression::Ternary { .. } => false,
        ProtoExpression::DynamicVariable { .. } => false, // array access
    }
}

/// Convert eligible If blocks to Ternary expressions (conditional select).
/// This eliminates basic block splits in JIT, avoiding load_cache clears
/// at If boundaries (3 clears per If → 0 with select).
///
/// Pattern 1: Default Assign + If override (empty false side)
///   Assign(X, default) ; If(cond, [Assign(X, override)], [])
///   → Assign(X, Ternary(cond, override, default))
///
/// Pattern 2: If with both sides assigning single matching offset
///   If(cond, [Assign(X, t)], [Assign(X, f)])
///   → Assign(X, Ternary(cond, t, f))
pub fn flatten_if_to_select(stmts: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    use crate::ir::expression::ExpressionContext;

    let mut result: Vec<ProtoStatement> = Vec::with_capacity(stmts.len());
    let mut converted = 0usize;

    let mut i = 0;
    while i < stmts.len() {
        // Pattern 1: Assign(X, default) followed by If(cond, [Assign(X, override)], [])
        if i + 1 < stmts.len()
            && let (ProtoStatement::Assign(default_assign), ProtoStatement::If(if_stmt)) =
                (&stmts[i], &stmts[i + 1])
            && let Some(cond) = &if_stmt.cond
            && if_stmt.false_side.is_empty()
            && if_stmt.true_side.len() == 1
            && default_assign.select.is_none()
            && default_assign.dynamic_select.is_none()
            && let ProtoStatement::Assign(override_assign) = &if_stmt.true_side[0]
        {
            let both_safe = is_safe_for_select(&override_assign.expr)
                && is_safe_for_select(&default_assign.expr);
            if override_assign.dst == default_assign.dst
                && override_assign.select.is_none()
                && override_assign.dynamic_select.is_none()
                && override_assign.dst_width == default_assign.dst_width
                && both_safe
            {
                let ternary = ProtoExpression::Ternary {
                    cond: Box::new(cond.clone()),
                    true_expr: Box::new(override_assign.expr.clone()),
                    false_expr: Box::new(default_assign.expr.clone()),
                    width: default_assign.dst_width,
                    expr_context: ExpressionContext {
                        width: default_assign.dst_width,
                        signed: false,
                    },
                };
                result.push(ProtoStatement::Assign(ProtoAssignStatement {
                    expr: ternary,
                    ..default_assign.clone()
                }));
                converted += 1;
                i += 2;
                continue;
            }
        }

        // Pattern 2: If with matching single Assigns on both sides
        if let ProtoStatement::If(if_stmt) = &stmts[i]
            && let Some(cond) = &if_stmt.cond
            && if_stmt.true_side.len() == 1
            && if_stmt.false_side.len() == 1
            && let (ProtoStatement::Assign(true_assign), ProtoStatement::Assign(false_assign)) =
                (&if_stmt.true_side[0], &if_stmt.false_side[0])
        {
            // Only convert when both expressions are side-effect free
            // and won't produce problematic values when evaluated
            // unconditionally (e.g., no DynamicVariable array access
            // with potentially out-of-range indices).
            let both_safe =
                is_safe_for_select(&true_assign.expr) && is_safe_for_select(&false_assign.expr);
            if true_assign.dst == false_assign.dst
                && true_assign.select.is_none()
                && false_assign.select.is_none()
                && true_assign.dynamic_select.is_none()
                && false_assign.dynamic_select.is_none()
                && true_assign.dst_width == false_assign.dst_width
                && both_safe
            {
                let ternary = ProtoExpression::Ternary {
                    cond: Box::new(cond.clone()),
                    true_expr: Box::new(true_assign.expr.clone()),
                    false_expr: Box::new(false_assign.expr.clone()),
                    width: true_assign.dst_width,
                    expr_context: ExpressionContext {
                        width: true_assign.dst_width,
                        signed: false,
                    },
                };
                result.push(ProtoStatement::Assign(ProtoAssignStatement {
                    expr: ternary,
                    ..true_assign.clone()
                }));
                converted += 1;
                i += 1;
                continue;
            }
        }

        result.push(stmts[i].clone());
        i += 1;
    }

    if converted > 0 {
        log::info!(
            "If→select: {} If blocks converted to Ternary ({} → {} stmts)",
            converted,
            stmts.len(),
            result.len(),
        );
    }

    result
}
