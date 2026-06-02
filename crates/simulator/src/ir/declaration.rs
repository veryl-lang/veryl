use crate::backend::inst::try_compile_inst_chunks;
use crate::ir::context::{Context, Conv, ScopeContext};
use crate::ir::expression::{ExpressionContext, build_dynamic_bit_select};
use crate::ir::opt::multi_write_analysis::analyze_multi_write;
use crate::ir::statement::ProtoAssignStatement;
use crate::ir::variable::{
    ModuleVariableMeta, VarOffset, align_up_64, create_variable_meta, ff_cacheline_pad_enabled,
};
use crate::ir::{Event, ProtoExpression, ProtoStatement};
use crate::simulator_error::SimulatorError;
use crate::{HashMap, HashSet};
use std::collections::VecDeque;
use veryl_analyzer::ir as air;
use veryl_parser::token_range::TokenRange;

/// Stable topological sort of comb statements using Kahn's algorithm (BFS/FIFO).
///
/// Builds Read-After-Write (RAW) dependency edges: for each variable written by
/// statement A and read by statement B (where B != A), add edge A → B.
/// Self-references (a statement that both reads and writes the same variable)
/// are skipped to avoid false cycles.
///
/// Falls back to source order if a cycle is detected.
pub(crate) fn stable_topo_sort(statements: Vec<ProtoStatement>) -> Vec<ProtoStatement> {
    let n = statements.len();
    if n <= 1 {
        return statements;
    }

    // Gather per-statement inputs and outputs (variable offsets).
    let mut stmt_inputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    let mut stmt_outputs: Vec<Vec<VarOffset>> = Vec::with_capacity(n);
    for s in &statements {
        let mut ins = vec![];
        let mut outs = vec![];
        s.gather_variable_offsets(&mut ins, &mut outs);
        stmt_inputs.push(ins);
        stmt_outputs.push(outs);
    }

    // Build a map: variable → set of statement indices that write it.
    // Dedup per stmt so repeated writes (e.g. multiple case arms touching
    // the same var) don't inflate the downstream in-degree count.
    let mut writers: HashMap<VarOffset, Vec<usize>> = HashMap::default();
    for (i, outs) in stmt_outputs.iter().enumerate() {
        let mut unique_outs: HashSet<VarOffset> = HashSet::default();
        for &key in outs {
            if unique_outs.insert(key) {
                writers.entry(key).or_default().push(i);
            }
        }
    }

    // Build adjacency list and in-degree count for Kahn's algorithm.
    // Edge: writer_stmt → reader_stmt (RAW dependency).
    // For variables with multiple writers (sequential reassignment from inlined
    // functions), only the most recent writer before the reader is relevant.
    let mut adj: Vec<HashSet<usize>> = vec![HashSet::default(); n];
    let mut in_degree: Vec<usize> = vec![0; n];

    for (reader_idx, ins) in stmt_inputs.iter().enumerate() {
        for key in ins {
            if let Some(writer_indices) = writers.get(key) {
                if writer_indices.len() == 1 {
                    let writer_idx = writer_indices[0];
                    if writer_idx != reader_idx && adj[writer_idx].insert(reader_idx) {
                        in_degree[reader_idx] += 1;
                    }
                } else if let Some(&writer_idx) =
                    writer_indices.iter().rev().find(|&&w| w < reader_idx)
                    && adj[writer_idx].insert(reader_idx)
                {
                    in_degree[reader_idx] += 1;
                }
            }
        }
    }

    // WAW ordering: chain consecutive writers of the same variable so that
    // bit-select assigns to a packed variable keep source order.
    // Skip when next already reaches prev (would create a cycle).
    for writer_indices in writers.values() {
        for pair in writer_indices.windows(2) {
            let (prev, next) = (pair[0], pair[1]);
            let mut reachable = false;
            let mut stack = vec![next];
            let mut visited = HashSet::default();
            while let Some(node) = stack.pop() {
                if node == prev {
                    reachable = true;
                    break;
                }
                if visited.insert(node) {
                    for &succ in &adj[node] {
                        stack.push(succ);
                    }
                }
            }
            if !reachable && adj[prev].insert(next) {
                in_degree[next] += 1;
            }
        }
    }

    // Kahn's algorithm with FIFO queue (VecDeque) for stable ordering.
    // Initialize queue with zero-in-degree nodes in source order.
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, &deg) in in_degree.iter().enumerate() {
        if deg == 0 {
            queue.push_back(i);
        }
    }

    let mut sorted_indices: Vec<usize> = Vec::with_capacity(n);
    while let Some(idx) = queue.pop_front() {
        sorted_indices.push(idx);
        // Collect successors in index order for determinism
        let mut successors: Vec<usize> = adj[idx].iter().cloned().collect();
        successors.sort_unstable();
        for succ in successors {
            in_degree[succ] -= 1;
            if in_degree[succ] == 0 {
                queue.push_back(succ);
            }
        }
    }

    // If not all nodes were processed, a cycle was detected — fall back to source order.
    if sorted_indices.len() != n {
        return statements;
    }

    // Reconstruct statement list in sorted order.
    let mut result: Vec<Option<ProtoStatement>> = statements.into_iter().map(Some).collect();
    sorted_indices
        .into_iter()
        .map(|i| result[i].take().unwrap())
        .collect()
}

pub struct ProtoDeclaration {
    pub event_statements: HashMap<Event, Vec<ProtoStatement>>,
    pub comb_statements: Vec<ProtoStatement>,
    /// Post-comb functions: child comb-only JIT functions for pre-event eval.
    pub post_comb_fns: Vec<ProtoStatement>,
    pub child_modules: Vec<ModuleVariableMeta>,
}

impl Conv<&air::Declaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::Declaration) -> Result<Self, SimulatorError> {
        match src {
            air::Declaration::Comb(x) => {
                let mut comb_statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    comb_statements.extend(stmts);
                }
                let comb_statements = if comb_statements.len() > 1 {
                    vec![ProtoStatement::SequentialBlock(comb_statements)]
                } else {
                    comb_statements
                };
                Ok(ProtoDeclaration {
                    event_statements: HashMap::default(),
                    comb_statements,
                    post_comb_fns: vec![],
                    child_modules: vec![],
                })
            }
            air::Declaration::Ff(x) => {
                let mut statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    statements.extend(stmts);
                }

                let clock_event = Event::Clock(x.clock.id);
                let mut event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();

                if let Some(reset) = &x.reset {
                    let reset_event = Event::Reset(reset.id);
                    let head = statements.remove(0);
                    let (mut true_side, mut false_side) = head.split_if_reset().unwrap();
                    // Statements after `if_reset` run on both reset and clock
                    // edges, so append to both branches instead of dropping them.
                    true_side.extend(statements.iter().cloned());
                    false_side.extend(statements);
                    event_statements.insert(reset_event, true_side);
                    event_statements.insert(clock_event, false_side);
                } else {
                    event_statements.insert(clock_event, statements);
                }

                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                })
            }
            air::Declaration::Inst(x) => Conv::conv(context, x.as_ref()),
            air::Declaration::Initial(x) => {
                context.in_initial = true;
                let mut initial_statements = vec![];
                let mut conv_err = None;
                for stmt in &x.statements {
                    match Conv::conv(context, stmt) {
                        Ok(stmts) => {
                            let stmts: Vec<ProtoStatement> = stmts;
                            initial_statements.extend(stmts);
                        }
                        Err(e) => {
                            conv_err = Some(e);
                            break;
                        }
                    }
                }
                context.in_initial = false;
                if let Some(e) = conv_err {
                    return Err(e);
                }
                let mut event_statements = HashMap::default();
                event_statements.insert(Event::Initial, initial_statements);
                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                })
            }
            air::Declaration::Final(x) => {
                let mut final_statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    final_statements.extend(stmts);
                }
                let mut event_statements = HashMap::default();
                event_statements.insert(Event::Final, final_statements);
                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                })
            }
            air::Declaration::Unsupported(token) => {
                Err(SimulatorError::unsupported_description(token))
            }
            air::Declaration::Null => Ok(ProtoDeclaration {
                event_statements: HashMap::default(),
                comb_statements: vec![],
                post_comb_fns: vec![],
                child_modules: vec![],
            }),
        }
    }
}

impl Conv<&air::InstDeclaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::InstDeclaration) -> Result<Self, SimulatorError> {
        let air::Component::Module(child_module) = src.component.as_ref() else {
            panic!("InstDeclaration for non-Module component");
        };

        let mut child_analyzer_context = veryl_analyzer::conv::Context::default();
        child_analyzer_context.variables = child_module.variables.clone();
        child_analyzer_context.functions = child_module.functions.clone();
        let mut child_ff_table = child_module.ff_table.clone();
        if context.config.disable_ff_opt {
            child_ff_table.force_all_ff();
        }

        // Comb-to-FF hoist: clone child declarations and mutate them —
        // move comb-side `let` writes into the consuming FF block, then
        // rebuild the FfTable on the hoisted form.
        let mut hoisted_child_decls = child_module.declarations.clone();
        {
            let plans = veryl_analyzer::ir::comb_to_ff_hoist::plan_hoists(
                &hoisted_child_decls,
                &child_ff_table,
                &child_module.variables,
            );
            veryl_analyzer::ir::comb_to_ff_hoist::apply_hoists(
                &mut hoisted_child_decls,
                &plans,
                &child_module.variables,
            );
            child_ff_table = air::FfTable::default();
            for (i, x) in hoisted_child_decls.iter().enumerate() {
                x.gather_ff(&mut child_analyzer_context, &mut child_ff_table, i);
            }
            child_ff_table.update_is_ff();
            if context.config.disable_ff_opt {
                child_ff_table.force_all_ff();
            }
        }
        let child_decls: &[air::Declaration] = &hoisted_child_decls;

        let mut ff_start = context.ff_total_bytes as isize;
        if ff_cacheline_pad_enabled() {
            ff_start = align_up_64(ff_start);
            context.ff_total_bytes = ff_start as usize;
        }
        let comb_start = context.comb_total_bytes as isize;

        // Analyzer-IR pre-pass to identify multi-RMW FFs.  Same as
        // ProtoModule::conv but for child module.
        let multi_rmw_set = analyze_multi_write(
            child_decls,
            &mut child_analyzer_context,
            context.config.disable_ff_opt,
        );

        let (mut child_variable_meta, child_ff_count, child_comb_count) = create_variable_meta(
            &child_module.variables,
            &child_ff_table,
            &multi_rmw_set,
            context.config.use_4state,
            ff_start,
            comb_start,
        )
        .unwrap();

        context.ff_total_bytes += child_ff_count;
        context.comb_total_bytes += child_comb_count;

        // Alias child input/output port storage to the parent slot it's
        // wired to, eliminating the inter-module copy at settle_comb.
        let alias_enabled = std::env::var("VERYL_DISABLE_PORT_ALIAS").ok().as_deref() != Some("1");
        let mut aliased_input_ids: HashSet<air::VarId> = HashSet::default();
        if alias_enabled {
            for input in &src.inputs {
                let air::Expression::Term(factor) = &input.expr else {
                    continue;
                };
                let air::Factor::Variable(parent_id, idx, sel, _) = factor.as_ref() else {
                    continue;
                };
                if !idx.0.is_empty() || !sel.is_empty() {
                    continue;
                }
                // Reading parent storage directly during an event would
                // race the parent's own NBA commit; reject when the
                // parent has any always_ff writer.
                let parent_scope = context.scope();
                let parent_has_ff_writer = parent_scope
                    .ff_table
                    .table
                    .iter()
                    .any(|((vid, _), entry)| *vid == *parent_id && entry.assigned.is_some());
                if parent_has_ff_writer {
                    continue;
                }
                let parent_meta = match parent_scope.variable_meta.get(parent_id) {
                    Some(m) => m.clone(),
                    None => continue,
                };
                let child_meta = match child_variable_meta.get(&input.id) {
                    Some(m) => m,
                    None => continue,
                };
                if parent_meta.elements.len() != child_meta.elements.len()
                    || parent_meta.width != child_meta.width
                    || parent_meta.native_bytes != child_meta.native_bytes
                {
                    continue;
                }
                let entry = child_variable_meta.get_mut(&input.id).unwrap();
                for (i, parent_elem) in parent_meta.elements.iter().enumerate() {
                    entry.elements[i].current = parent_elem.current;
                    entry.elements[i].next_offset = parent_elem.next_offset;
                }
                // Drop initial_values so fill_buffers_recursive doesn't
                // overwrite parent storage with the child port's default.
                entry.initial_values.clear();
                aliased_input_ids.insert(input.id);
            }
        }

        // Output ports are aliased comb→comb only.  An FF on either side
        // has a separate next-slot write that would not reach the comb
        // slot the alias targets.
        let mut aliased_output_ids: HashSet<air::VarId> = HashSet::default();
        let mut already_aliased_parent_ids: HashSet<air::VarId> = HashSet::default();
        if alias_enabled {
            for output in &src.outputs {
                let Some(parent_dst) = output.dst.first() else {
                    continue;
                };
                if !parent_dst.index.0.is_empty() || !parent_dst.select.is_empty() {
                    continue;
                }
                let child_output_is_ff = child_ff_table
                    .table
                    .iter()
                    .any(|((vid, _), entry)| *vid == output.id && entry.assigned.is_some());
                if child_output_is_ff {
                    continue;
                }
                let parent_scope = context.scope();
                let parent_has_ff_writer = parent_scope
                    .ff_table
                    .table
                    .iter()
                    .any(|((vid, _), entry)| *vid == parent_dst.id && entry.assigned.is_some());
                if parent_has_ff_writer {
                    continue;
                }
                let parent_meta = match parent_scope.variable_meta.get(&parent_dst.id) {
                    Some(m) => m.clone(),
                    None => continue,
                };
                if parent_meta.elements.iter().any(|e| e.is_ff()) {
                    continue;
                }
                let child_meta = match child_variable_meta.get(&output.id) {
                    Some(m) => m,
                    None => continue,
                };
                if parent_meta.elements.len() != child_meta.elements.len()
                    || parent_meta.width != child_meta.width
                    || parent_meta.native_bytes != child_meta.native_bytes
                {
                    continue;
                }
                if already_aliased_parent_ids.contains(&parent_dst.id) {
                    continue;
                }
                let entry = child_variable_meta.get_mut(&output.id).unwrap();
                for (i, parent_elem) in parent_meta.elements.iter().enumerate() {
                    entry.elements[i].current = parent_elem.current;
                    entry.elements[i].next_offset = parent_elem.next_offset;
                }
                entry.initial_values.clear();
                aliased_output_ids.insert(output.id);
                already_aliased_parent_ids.insert(parent_dst.id);
            }
        }

        let child_scope = ScopeContext {
            variable_meta: child_variable_meta.clone(),
            analyzer_context: child_analyzer_context,
            ff_table: child_ff_table.clone(),
        };
        context.scope_contexts.push(child_scope);

        let mut all_event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        let mut all_comb_statements: Vec<ProtoStatement> = vec![];
        let mut all_post_comb_fns: Vec<ProtoStatement> = vec![];
        let mut all_child_modules: Vec<ModuleVariableMeta> = vec![];

        for decl in child_decls {
            let proto_decl: ProtoDeclaration = Conv::conv(context, decl)?;

            for (event, mut stmts) in proto_decl.event_statements {
                all_event_statements
                    .entry(event)
                    .and_modify(|v| v.append(&mut stmts))
                    .or_insert(stmts);
            }
            all_comb_statements.append(&mut proto_decl.comb_statements.clone());
            all_post_comb_fns.extend(proto_decl.post_comb_fns);
            all_child_modules.extend(proto_decl.child_modules);
        }

        context.scope_contexts.pop();

        try_compile_inst_chunks(
            context,
            src,
            ff_start,
            comb_start,
            alias_enabled,
            &mut all_event_statements,
            &mut all_comb_statements,
        );

        // Input ports: parent expr → child port var
        for input in &src.inputs {
            if aliased_input_ids.contains(&input.id) {
                continue;
            }
            let child_meta = child_variable_meta.get(&input.id).unwrap();

            // Array port with a simple variable expression: expand per-element
            if child_meta.elements.len() > 1
                && let air::Expression::Term(factor) = &input.expr
                && let air::Factor::Variable(parent_id, index, select, _) = factor.as_ref()
                && index.0.is_empty()
                && select.is_empty()
            {
                let parent_scope = context.scope();
                let parent_meta = parent_scope.variable_meta.get(parent_id).unwrap();
                for i in 0..child_meta.elements.len() {
                    let child_element = &child_meta.elements[i];
                    let parent_element = &parent_meta.elements[i];
                    let parent_expr = ProtoExpression::Variable {
                        var_offset: parent_element.current,
                        select: None,
                        dynamic_select: None,
                        width: child_meta.width,
                        var_full_width: child_meta.width,
                        expr_context: ExpressionContext {
                            width: child_meta.width,
                            signed: false,
                        },
                    };
                    all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                        dst: child_element.current,
                        dst_width: child_meta.width,
                        select: None,
                        dynamic_select: None,
                        rhs_select: None,
                        expr: parent_expr,
                        dst_ff_current_offset: 0, // not FF
                        token: TokenRange::default(),
                    }));
                }
                continue;
            }

            let proto_expr: ProtoExpression = Conv::conv(context, &input.expr)?;
            let element = &child_meta.elements[0];
            all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                dst: element.current,
                dst_width: child_meta.width,
                select: None,
                dynamic_select: None,
                rhs_select: None,
                expr: proto_expr.clone(),
                dst_ff_current_offset: 0, // not FF
                token: TokenRange::default(),
            }));
        }

        // Output ports: child port var → parent dst
        for output in &src.outputs {
            if aliased_output_ids.contains(&output.id) {
                continue;
            }
            if let Some(parent_dst) = output.dst.first() {
                let child_meta = child_variable_meta.get(&output.id).unwrap();

                let (
                    parent_index,
                    parent_select,
                    parent_width,
                    parent_need_dynamic,
                    parent_width_shape,
                    parent_kind_width,
                ) = {
                    let parent_scope = context.scope();
                    let parent_meta = parent_scope.variable_meta.get(&parent_dst.id).unwrap();
                    let parent_index = parent_dst
                        .index
                        .eval_value(&mut parent_scope.analyzer_context)
                        .unwrap();

                    let select = if !parent_dst.select.is_empty() {
                        parent_dst.select.eval_value(
                            &mut parent_scope.analyzer_context,
                            &parent_dst.comptime.r#type,
                            false,
                        )
                    } else {
                        None
                    };
                    let need_dynamic =
                        !parent_dst.select.is_empty() && !parent_dst.select.is_const();
                    let select = if need_dynamic { None } else { select };
                    let width = parent_meta.width;
                    let width_shape = parent_meta.r#type.width().clone();
                    let kind_width = parent_meta.r#type.kind.width().unwrap_or(1);
                    (
                        parent_index,
                        select,
                        width,
                        need_dynamic,
                        width_shape,
                        kind_width,
                    )
                };

                let parent_dynamic_select = if parent_need_dynamic {
                    Some(build_dynamic_bit_select(
                        context,
                        &parent_width_shape,
                        &parent_dst.select,
                        parent_kind_width,
                    )?)
                } else {
                    None
                };

                let parent_scope = context.scope();
                let parent_meta = parent_scope.variable_meta.get(&parent_dst.id).unwrap();

                // Determine which parent elements to connect.
                // When the parent destination has no index and the variable is an
                // array, connect each element individually (array-to-array port).
                let parent_element_indices: Vec<usize> = if let Some(idx) =
                    parent_meta.r#type.array.calc_index(&parent_index)
                {
                    vec![idx]
                } else if parent_index.is_empty() && !parent_meta.r#type.array.is_empty() {
                    (0..parent_meta.elements.len()).collect()
                } else {
                    panic!(
                        "calc_index failed for output port destination (index {:?}, array {:?})",
                        parent_index, parent_meta.r#type.array,
                    );
                };

                for (elem_idx, &parent_elem_idx) in parent_element_indices.iter().enumerate() {
                    let child_element = &child_meta.elements[elem_idx];
                    let parent_element = &parent_meta.elements[parent_elem_idx];

                    let child_expr = ProtoExpression::Variable {
                        var_offset: child_element.current,
                        select: None,
                        dynamic_select: None,
                        width: child_meta.width,
                        var_full_width: child_meta.width,
                        expr_context: ExpressionContext {
                            width: child_meta.width,
                            signed: false,
                        },
                    };

                    let dst_var = if parent_element.is_ff() {
                        VarOffset::Ff(parent_element.next_offset)
                    } else {
                        VarOffset::Comb(parent_element.current_offset())
                    };

                    let stmt = ProtoStatement::Assign(ProtoAssignStatement {
                        dst: dst_var,
                        dst_width: parent_width,
                        select: parent_select,
                        dynamic_select: parent_dynamic_select.clone(),
                        rhs_select: None,
                        expr: child_expr,
                        dst_ff_current_offset: parent_element.current_offset(),
                        token: TokenRange::default(),
                    });

                    all_comb_statements.push(stmt);
                }
            }
        }

        // Remap child event keys (clock/reset) to parent VarIds via input port connections
        let mut child_to_parent_var: HashMap<air::VarId, air::VarId> = HashMap::default();
        for input in &src.inputs {
            if let air::Expression::Term(factor) = &input.expr
                && let air::Factor::Variable(parent_var_id, _, _, _) = factor.as_ref()
            {
                child_to_parent_var.insert(input.id, *parent_var_id);
            }
        }

        let mut remapped_events: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        for (event, stmts) in all_event_statements {
            let new_event = match &event {
                Event::Clock(child_id) => {
                    if let Some(parent_id) = child_to_parent_var.get(child_id) {
                        Event::Clock(*parent_id)
                    } else {
                        event.clone()
                    }
                }
                Event::Reset(child_id) => {
                    if let Some(parent_id) = child_to_parent_var.get(child_id) {
                        Event::Reset(*parent_id)
                    } else {
                        event.clone()
                    }
                }
                _ => event.clone(),
            };
            remapped_events
                .entry(new_event)
                .and_modify(|v| v.extend(stmts.clone()))
                .or_insert(stmts);
        }

        let child_module_meta = ModuleVariableMeta {
            name: src.name,
            variable_meta: child_variable_meta,
            children: all_child_modules,
        };

        Ok(ProtoDeclaration {
            event_statements: remapped_events,
            comb_statements: all_comb_statements,
            post_comb_fns: all_post_comb_fns,
            child_modules: vec![child_module_meta],
        })
    }
}
