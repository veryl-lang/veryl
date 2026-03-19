use crate::HashMap;
use crate::HashSet;
use crate::cranelift;
use crate::ir::context::{Context, Conv, JitCacheEntry, JitCachedFunc, ScopeContext};
use crate::ir::expression::ExpressionContext;
use crate::ir::statement::{CompiledBlockStatement, ProtoAssignStatement};
use crate::ir::variable::{ModuleVariableMeta, create_variable_meta};
use crate::ir::{Event, ProtoExpression, ProtoStatement};
use crate::simulator_error::SimulatorError;
use veryl_analyzer::ir as air;
use veryl_parser::token_range::TokenRange;

/// Collect variable offsets from statements, filtering out internal variables
/// (those that appear in both inputs and outputs) to avoid dependency cycles
/// when the compiled block is used in analyze_dependency.
type VarOffsets = Vec<(bool, isize)>;

fn gather_external_offsets(stmts: &[ProtoStatement]) -> (VarOffsets, VarOffsets) {
    let mut all_inputs = vec![];
    let mut all_outputs = vec![];
    for s in stmts {
        s.gather_variable_offsets(&mut all_inputs, &mut all_outputs);
    }

    let input_set: HashSet<(bool, isize)> = all_inputs.iter().cloned().collect();
    let output_set: HashSet<(bool, isize)> = all_outputs.iter().cloned().collect();
    let internal: HashSet<(bool, isize)> = input_set.intersection(&output_set).cloned().collect();

    all_inputs.retain(|x| !internal.contains(x));
    all_outputs.retain(|x| !internal.contains(x));
    all_inputs.dedup();
    all_outputs.dedup();

    (all_inputs, all_outputs)
}

pub struct ProtoDeclaration {
    pub event_statements: HashMap<Event, Vec<ProtoStatement>>,
    pub comb_statements: Vec<ProtoStatement>,
    /// Post-comb functions: child comb-only JIT functions for pre-event eval.
    pub post_comb_fns: Vec<ProtoStatement>,
    pub child_modules: Vec<ModuleVariableMeta>,
    /// Full internal comb statements (before merge optimization removed them).
    /// Present only when merged comb+event functions are used.
    pub full_internal_comb: Option<Vec<ProtoStatement>>,
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
                Ok(ProtoDeclaration {
                    event_statements: HashMap::default(),
                    comb_statements,
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    full_internal_comb: None,
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
                    let (true_side, false_side) = head.split_if_reset().unwrap();
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
                    full_internal_comb: None,
                })
            }
            air::Declaration::Inst(x) => Conv::conv(context, x.as_ref()),
            air::Declaration::Initial(x) => {
                let mut initial_statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    initial_statements.extend(stmts);
                }
                let mut event_statements = HashMap::default();
                event_statements.insert(Event::Initial, initial_statements);
                Ok(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    post_comb_fns: vec![],
                    child_modules: vec![],
                    full_internal_comb: None,
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
                    full_internal_comb: None,
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
                full_internal_comb: None,
            }),
        }
    }
}

impl Conv<&air::InstDeclaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::InstDeclaration) -> Result<Self, SimulatorError> {
        let air::Component::Module(child_module) = &src.component else {
            panic!("InstDeclaration for non-Module component");
        };

        let mut child_analyzer_context = veryl_analyzer::conv::Context::default();
        child_analyzer_context.variables = child_module.variables.clone();
        child_analyzer_context.functions = child_module.functions.clone();
        let mut child_ff_table = air::FfTable::default();
        child_module.gather_ff(&mut child_analyzer_context, &mut child_ff_table);
        child_ff_table.update_is_ff();

        let ff_start = context.ff_total_bytes as isize;
        let comb_start = context.comb_total_bytes as isize;
        let (child_variable_meta, child_ff_count, child_comb_count) = create_variable_meta(
            &child_module.variables,
            &child_ff_table,
            context.config.use_4state,
            ff_start,
            comb_start,
        )
        .unwrap();

        context.ff_total_bytes += child_ff_count;
        context.comb_total_bytes += child_comb_count;

        let child_scope = ScopeContext {
            variable_meta: child_variable_meta.clone(),
            analyzer_context: child_analyzer_context,
        };
        context.scope_contexts.push(child_scope);

        let mut all_event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        let mut all_comb_statements: Vec<ProtoStatement> = vec![];
        let mut all_post_comb_fns: Vec<ProtoStatement> = vec![];
        let mut all_child_modules: Vec<ModuleVariableMeta> = vec![];

        for decl in &child_module.declarations {
            let proto_decl: ProtoDeclaration = match Conv::conv(context, decl) {
                Ok(d) => d,
                Err(_) => continue,
            };

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

        // JIT cache: reuse compiled code across instances of the same module type.
        // ff_start and comb_start are already byte offsets.
        let mut full_internal_comb: Option<Vec<ProtoStatement>> = None;
        if context.config.use_jit {
            let ff_start_bytes = ff_start;
            let comb_start_bytes = comb_start;
            let module_name = child_module.name;

            if let Some(cache_entry) = context.jit_cache.get(&module_name) {
                // Cache hit: replace internal logic with CompiledBlocks using delta
                let ff_delta = ff_start_bytes - cache_entry.ref_ff_start_bytes;
                let comb_delta = comb_start_bytes - cache_entry.ref_comb_start_bytes;

                let adjust = |offsets: &[(bool, isize)]| -> Vec<(bool, isize)> {
                    offsets
                        .iter()
                        .map(|(is_ff, off)| {
                            (*is_ff, off + if *is_ff { ff_delta } else { comb_delta })
                        })
                        .collect()
                };

                for (event, stmts) in all_event_statements.iter_mut() {
                    // Prefer merged function (comb+event combined) over event-only
                    let cached = cache_entry
                        .merged_funcs
                        .get(event)
                        .or_else(|| cache_entry.event_funcs.get(event));
                    if let Some(cached) = cached {
                        *stmts = vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                            func: cached.func,
                            ff_delta_bytes: ff_delta,
                            comb_delta_bytes: comb_delta,
                            input_offsets: adjust(&cached.input_offsets),
                            output_offsets: adjust(&cached.output_offsets),
                        })];
                    }
                }

                full_internal_comb = if !cache_entry.merged_funcs.is_empty() {
                    let full = all_comb_statements.clone();
                    all_comb_statements.clear();
                    Some(full)
                } else {
                    None
                };

                if !cache_entry.merged_funcs.is_empty() {
                    // Internal comb already cleared above.
                    // Add comb-only JIT function to post_comb_fns so child comb
                    // is evaluated before events fire (without going through
                    // analyze_dependency on the parent level).
                    if let Some(cached) = &cache_entry.comb_func {
                        all_post_comb_fns.push(ProtoStatement::CompiledBlock(
                            CompiledBlockStatement {
                                func: cached.func,
                                ff_delta_bytes: ff_delta,
                                comb_delta_bytes: comb_delta,
                                input_offsets: adjust(&cached.input_offsets),
                                output_offsets: adjust(&cached.output_offsets),
                            },
                        ));
                    }
                } else if let Some(cached) = &cache_entry.comb_func {
                    all_comb_statements =
                        vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                            func: cached.func,
                            ff_delta_bytes: ff_delta,
                            comb_delta_bytes: comb_delta,
                            input_offsets: adjust(&cached.input_offsets),
                            output_offsets: adjust(&cached.output_offsets),
                        })];
                }
            } else {
                // Cache miss: save originals before individual compilation
                let original_comb = all_comb_statements.clone();
                let original_events: HashMap<Event, Vec<ProtoStatement>> =
                    all_event_statements.clone();

                // Compile events individually
                let mut event_funcs = HashMap::default();
                for (event, stmts) in all_event_statements.iter_mut() {
                    if stmts.iter().all(|s| s.can_build_binary())
                        && !stmts.is_empty()
                        && let Some(func) = cranelift::build_binary(context, stmts.clone())
                    {
                        let (input_offsets, output_offsets) = gather_external_offsets(stmts);

                        event_funcs.insert(
                            event.clone(),
                            JitCachedFunc {
                                func,
                                input_offsets: input_offsets.clone(),
                                output_offsets: output_offsets.clone(),
                            },
                        );

                        *stmts = vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                            func,
                            ff_delta_bytes: 0,
                            comb_delta_bytes: 0,
                            input_offsets,
                            output_offsets,
                        })];
                    }
                }

                // Compile comb individually
                let comb_func = if all_comb_statements.iter().all(|s| s.can_build_binary())
                    && !all_comb_statements.is_empty()
                {
                    if let Some(func) =
                        cranelift::build_binary(context, all_comb_statements.clone())
                    {
                        let (input_offsets, output_offsets) =
                            gather_external_offsets(&all_comb_statements);

                        all_comb_statements =
                            vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                                func,
                                ff_delta_bytes: 0,
                                comb_delta_bytes: 0,
                                input_offsets: input_offsets.clone(),
                                output_offsets: output_offsets.clone(),
                            })];

                        Some(JitCachedFunc {
                            func,
                            input_offsets,
                            output_offsets,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Compile merged comb+event functions using saved originals.
                // The merged function computes comb then event in one JIT call,
                // allowing load_cache to forward comb stores to event loads.
                let comb_jittable =
                    !original_comb.is_empty() && original_comb.iter().all(|s| s.can_build_binary());
                let mut merged_funcs = HashMap::default();

                if comb_jittable {
                    // Sort comb for inlining optimization.
                    // Combinational loops in child modules will be caught by the
                    // parent-level analyze_dependency call, so we can safely skip
                    // the merge optimization here.
                    let sorted_comb = super::module::analyze_dependency(original_comb.clone()).ok();

                    // Compute external reads: output port comb offsets that are
                    // read by port connections after the merged function returns
                    let mut external_reads = HashSet::default();
                    for output in &src.outputs {
                        for child_var_id in &output.id {
                            if let Some(child_meta) = child_variable_meta.get(child_var_id) {
                                let element = &child_meta.elements[0];
                                if !element.is_ff {
                                    external_reads.insert(element.current_offset);
                                }
                            }
                        }
                    }

                    for (event, orig_stmts) in &original_events {
                        if orig_stmts.is_empty() || !orig_stmts.iter().all(|s| s.can_build_binary())
                        {
                            continue;
                        }

                        // Inline single-use comb variables into event statements
                        let (opt_comb, opt_events) = if let Some(sorted) = &sorted_comb {
                            super::optimize::optimize_merged(
                                sorted.clone(),
                                orig_stmts.clone(),
                                &external_reads,
                            )
                        } else {
                            (original_comb.clone(), orig_stmts.clone())
                        };

                        // Check that optimized statements are still jittable
                        let all_jittable = opt_comb
                            .iter()
                            .chain(opt_events.iter())
                            .all(|s| s.can_build_binary());
                        if !all_jittable {
                            continue;
                        }

                        // Compute store elimination set: internal comb offsets
                        // that are not externally read (port connections, etc.)
                        let mut store_elim = HashSet::default();
                        for s in &opt_comb {
                            if let ProtoStatement::Assign(a) = s
                                && !a.dst_is_ff
                                && a.select.is_none()
                                && !external_reads.contains(&a.dst_offset)
                            {
                                store_elim.insert((a.dst_is_ff, a.dst_offset as i32));
                            }
                        }

                        let mut merged = opt_comb;
                        merged.extend(opt_events);

                        if let Some(func) = cranelift::build_binary_with_store_elim(
                            context,
                            merged.clone(),
                            store_elim,
                        ) {
                            let (input_offsets, output_offsets) = gather_external_offsets(&merged);

                            // Replace event_statements with merged CompiledBlock
                            all_event_statements.insert(
                                event.clone(),
                                vec![ProtoStatement::CompiledBlock(CompiledBlockStatement {
                                    func,
                                    ff_delta_bytes: 0,
                                    comb_delta_bytes: 0,
                                    input_offsets: input_offsets.clone(),
                                    output_offsets: output_offsets.clone(),
                                })],
                            );

                            merged_funcs.insert(
                                event.clone(),
                                JitCachedFunc {
                                    func,
                                    input_offsets,
                                    output_offsets,
                                },
                            );
                        }
                    }
                }

                // If any merged functions were compiled, save full internal comb
                // and clear it from comb_statements. Port connections are added
                // after this block so they remain in comb_statements.
                // The full comb is needed by get()/dump() for correctness.
                full_internal_comb = if !merged_funcs.is_empty() {
                    let full = all_comb_statements.clone();
                    all_comb_statements.clear();
                    // When merged comb+event is used, add the comb-only JIT function
                    // to post_comb_fns so child comb is evaluated before events fire.
                    if let Some(ref cf) = comb_func {
                        all_post_comb_fns.push(ProtoStatement::CompiledBlock(
                            CompiledBlockStatement {
                                func: cf.func,
                                ff_delta_bytes: 0,
                                comb_delta_bytes: 0,
                                input_offsets: cf.input_offsets.clone(),
                                output_offsets: cf.output_offsets.clone(),
                            },
                        ));
                    }
                    Some(full)
                } else {
                    None
                };

                context.jit_cache.insert(
                    module_name,
                    JitCacheEntry {
                        ref_ff_start_bytes: ff_start_bytes,
                        ref_comb_start_bytes: comb_start_bytes,
                        event_funcs,
                        comb_func,
                        merged_funcs,
                    },
                );
            }
        }

        // Input ports: parent expr → child port var
        for input in &src.inputs {
            let proto_expr: ProtoExpression = Conv::conv(context, &input.expr)?;

            for child_var_id in &input.id {
                let child_meta = child_variable_meta.get(child_var_id).unwrap();
                let element = &child_meta.elements[0];
                all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst_offset: element.current_offset,
                    dst_is_ff: false,
                    dst_width: child_meta.width,
                    select: None,
                    rhs_select: None,
                    expr: proto_expr.clone(),
                    dst_ff_current_offset: 0, // not FF
                    token: TokenRange::default(),
                }));
            }
        }

        // Output ports: child port var → parent dst
        // When merged functions exist, also add output port connections to
        // post_comb_fns so that child comb values (computed by post_comb)
        // propagate to parent variables before events fire.
        let needs_post_comb_propagation =
            full_internal_comb.is_some() || !all_post_comb_fns.is_empty();
        for output in &src.outputs {
            for (child_var_id, parent_dst) in output.id.iter().zip(output.dst.iter()) {
                let child_meta = child_variable_meta.get(child_var_id).unwrap();

                let parent_scope = context.scope();
                let parent_meta = parent_scope.variable_meta.get(&parent_dst.id).unwrap();
                let parent_index = parent_dst
                    .index
                    .eval_value(&mut parent_scope.analyzer_context)
                    .unwrap();

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
                        offset: child_element.current_offset,
                        is_ff: child_element.is_ff,
                        select: None,
                        width: child_meta.width,
                        expr_context: ExpressionContext {
                            width: child_meta.width,
                            signed: false,
                        },
                    };

                    let (dst_offset, dst_is_ff) = if parent_element.is_ff {
                        (parent_element.next_offset, true)
                    } else {
                        (parent_element.current_offset, false)
                    };

                    let stmt = ProtoStatement::Assign(ProtoAssignStatement {
                        dst_offset,
                        dst_is_ff,
                        dst_width: parent_meta.width,
                        select: None,
                        rhs_select: None,
                        expr: child_expr,
                        dst_ff_current_offset: parent_element.current_offset,
                        token: TokenRange::default(),
                    });

                    all_comb_statements.push(stmt.clone());

                    // When this module has merged functions, also add comb
                    // output port connections to post_comb_fns so that child
                    // comb values propagate to parent before events fire.
                    if needs_post_comb_propagation && !dst_is_ff {
                        all_post_comb_fns.push(stmt);
                    }
                }
            }
        }

        // Remap child event keys (clock/reset) to parent VarIds via input port connections
        let mut child_to_parent_var: HashMap<air::VarId, air::VarId> = HashMap::default();
        for input in &src.inputs {
            if let air::Expression::Term(factor) = &input.expr
                && let air::Factor::Variable(parent_var_id, _, _, _) = factor.as_ref()
            {
                for child_var_id in &input.id {
                    child_to_parent_var.insert(*child_var_id, *parent_var_id);
                }
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
            full_internal_comb,
        })
    }
}
