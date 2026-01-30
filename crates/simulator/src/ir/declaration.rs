use crate::HashMap;
use crate::ir::context::{Context, Conv, ScopeContext};
use crate::ir::expression::ExpressionContext;
use crate::ir::statement::ProtoAssignStatement;
use crate::ir::variable::{ModuleVariableMeta, create_variable_meta};
use crate::ir::{Event, ProtoExpression, ProtoStatement};
use veryl_analyzer::ir as air;

pub struct ProtoDeclaration {
    pub event_statements: HashMap<Event, Vec<ProtoStatement>>,
    pub comb_statements: Vec<ProtoStatement>,
    pub child_modules: Vec<ModuleVariableMeta>,
}

impl Conv<&air::Declaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::Declaration) -> Option<Self> {
        match src {
            air::Declaration::Comb(x) => {
                let mut comb_statements = vec![];
                for stmt in &x.statements {
                    let stmts: Vec<ProtoStatement> = Conv::conv(context, stmt)?;
                    comb_statements.extend(stmts);
                }
                Some(ProtoDeclaration {
                    event_statements: HashMap::default(),
                    comb_statements,
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
                    let (true_side, false_side) = head.split_if_reset()?;
                    event_statements.insert(reset_event, true_side);
                    event_statements.insert(clock_event, false_side);
                } else {
                    event_statements.insert(clock_event, statements);
                }

                Some(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    child_modules: vec![],
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
                Some(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
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
                Some(ProtoDeclaration {
                    event_statements,
                    comb_statements: vec![],
                    child_modules: vec![],
                })
            }
            _ => None,
        }
    }
}

impl Conv<&air::InstDeclaration> for ProtoDeclaration {
    fn conv(context: &mut Context, src: &air::InstDeclaration) -> Option<Self> {
        let air::Component::Module(child_module) = &src.component else {
            return None;
        };

        let mut child_analyzer_context = veryl_analyzer::conv::Context::default();
        child_analyzer_context.variables = child_module.variables.clone();
        child_analyzer_context.functions = child_module.functions.clone();
        let mut child_ff_table = air::FfTable::default();
        child_module.gather_ff(&mut child_analyzer_context, &mut child_ff_table);
        child_ff_table.update_is_ff();

        let ff_start = context.ff_total_count as isize;
        let comb_start = context.comb_total_count as isize;
        let (child_variable_meta, child_ff_count, child_comb_count) = create_variable_meta(
            &child_module.variables,
            &child_ff_table,
            context.config.use_4state,
            ff_start,
            comb_start,
        )?;

        context.ff_total_count += child_ff_count;
        context.comb_total_count += child_comb_count;

        let child_scope = ScopeContext {
            variable_meta: child_variable_meta.clone(),
            analyzer_context: child_analyzer_context,
        };
        context.scope_contexts.push(child_scope);

        let mut all_event_statements: HashMap<Event, Vec<ProtoStatement>> = HashMap::default();
        let mut all_comb_statements: Vec<ProtoStatement> = vec![];
        let mut all_child_modules: Vec<ModuleVariableMeta> = vec![];

        for decl in &child_module.declarations {
            let proto_decl: ProtoDeclaration = match Conv::conv(context, decl) {
                Some(d) => d,
                None => continue,
            };

            for (event, mut stmts) in proto_decl.event_statements {
                all_event_statements
                    .entry(event)
                    .and_modify(|v| v.append(&mut stmts))
                    .or_insert(stmts);
            }
            all_comb_statements.append(&mut proto_decl.comb_statements.clone());
            all_child_modules.extend(proto_decl.child_modules);
        }

        context.scope_contexts.pop();

        // Input ports: parent expr → child port var
        for input in &src.inputs {
            let proto_expr: ProtoExpression = Conv::conv(context, &input.expr)?;

            for child_var_id in &input.id {
                let child_meta = child_variable_meta.get(child_var_id)?;
                let element = &child_meta.elements[0];
                all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst_offset: element.current_offset,
                    dst_is_ff: false,
                    dst_width: child_meta.width,
                    select: None,
                    rhs_select: None,
                    expr: proto_expr.clone(),
                }));
            }
        }

        // Output ports: child port var → parent dst
        for output in &src.outputs {
            for (child_var_id, parent_dst) in output.id.iter().zip(output.dst.iter()) {
                let child_meta = child_variable_meta.get(child_var_id)?;
                let child_element = &child_meta.elements[0];

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

                let parent_scope = context.scope();
                let parent_meta = parent_scope.variable_meta.get(&parent_dst.id)?;
                let parent_index = parent_dst
                    .index
                    .eval_value(&mut parent_scope.analyzer_context)?;
                let parent_index = parent_meta.r#type.array.calc_index(&parent_index)?;
                let parent_element = &parent_meta.elements[parent_index];

                let (dst_offset, dst_is_ff) = if parent_element.is_ff {
                    (parent_element.next_offset, true)
                } else {
                    (parent_element.current_offset, false)
                };

                all_comb_statements.push(ProtoStatement::Assign(ProtoAssignStatement {
                    dst_offset,
                    dst_is_ff,
                    dst_width: parent_meta.width,
                    select: None,
                    rhs_select: None,
                    expr: child_expr,
                }));
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

        Some(ProtoDeclaration {
            event_statements: remapped_events,
            comb_statements: all_comb_statements,
            child_modules: vec![child_module_meta],
        })
    }
}
