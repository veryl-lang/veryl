use crate::connect_operation_table;
use crate::conv::Context;
use crate::ir::Signature;
use crate::namespace::{DefineContext, Namespace};
use crate::symbol::{
    Direction, FunctionProperty, FunctionWrite, GenericMap, Symbol, SymbolId, SymbolKind,
};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use crate::{HashMap, HashSet, scope};
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

pub fn resolve_function(list: &[Symbol]) {
    for symbol in list {
        resolve_constantable(symbol, &mut Vec::new());
    }
    resolve_side_effects(list);
}

fn resolve_constantable(symbol: &Symbol, visited: &mut Vec<SymbolId>) -> bool {
    if let SymbolKind::Function(func) = &symbol.kind
        && let Some(constantable) = func.constantable
    {
        return constantable;
    }

    // Already in progress: a call cycle (mutual recursion `f0 -> f1 -> f0`),
    // not constant-evaluable and would overflow on re-entry. `type_dag` reports
    // the cycle; self-recursion is skipped earlier in `is_constantable_function`.
    if visited.contains(&symbol.id) {
        return false;
    }
    visited.push(symbol.id);

    let namespace = symbol.inner_namespace();
    let mut symbol = symbol.clone();
    let func = match &mut symbol.kind {
        SymbolKind::Function(func) => func,
        _ => unreachable!(),
    };

    let constantable = is_constantable_function(func, symbol.id, &namespace, visited);
    func.constantable = Some(constantable);
    func.reference_paths.clear();
    symbol_table::update(symbol);

    visited.pop();

    constantable
}

/// `f(o.a)` inside `f(o: output S)` appends one member per fixed-point round,
/// so the mapped path needs a bound to converge. Truncating keeps the root,
/// which is what `classify_write` keys on.
const FORMAL_WRITE_DEPTH_LIMIT: usize = 16;

#[derive(Clone, Default, PartialEq, Eq)]
struct Effect {
    external_writes: HashSet<DefineContext>,
    formal_writes: HashSet<FunctionWrite>,
}

#[derive(Default)]
struct WriteTarget {
    external: bool,
    formal: Option<GenericSymbolPath>,
}

impl WriteTarget {
    fn external() -> Self {
        Self {
            external: true,
            formal: None,
        }
    }

    fn formal(path: GenericSymbolPath) -> Self {
        Self {
            external: false,
            formal: Some(path),
        }
    }

    fn external_formal(path: GenericSymbolPath) -> Self {
        Self {
            external: true,
            formal: Some(path),
        }
    }
}

fn classify_connect(path: &GenericSymbolPath) -> Option<WriteTarget> {
    let operation = connect_operation_table::get(&path.range.beg)?;
    let has_write = if let Some((ports, _)) = operation.get_ports_with_expression() {
        !ports.is_empty()
    } else {
        !operation.get_connection_pairs().is_empty()
    };
    Some(if has_write {
        WriteTarget::external()
    } else {
        WriteTarget::default()
    })
}

fn classify_write(path: &GenericSymbolPath, namespace: &Namespace) -> WriteTarget {
    if let Some(target) = classify_connect(path) {
        return target;
    }
    let Some(_) = path.paths.first() else {
        return WriteTarget::default();
    };
    let mut root_path = path.clone();
    root_path.paths.truncate(1);
    let Ok(root) = symbol_table::resolve(&root_path) else {
        return WriteTarget::default();
    };

    if root.found.namespace.included(namespace) {
        match &root.found.kind {
            SymbolKind::Variable(_) => WriteTarget::default(),
            SymbolKind::Port(port) => match port.direction {
                Direction::Output => WriteTarget::formal(path.clone()),
                // A modport/inout actual is necessarily external to the process;
                // only an actual LHS reaching here is treated as a write.
                Direction::Modport | Direction::Inout => {
                    let Ok(target) = symbol_table::resolve(path) else {
                        return WriteTarget::default();
                    };
                    match &target.found.kind {
                        SymbolKind::ModportVariableMember(member)
                            if matches!(member.direction, Direction::Output | Direction::Inout) =>
                        {
                            WriteTarget::external_formal(path.clone())
                        }
                        _ if matches!(port.direction, Direction::Inout) => {
                            WriteTarget::external_formal(path.clone())
                        }
                        _ => WriteTarget::default(),
                    }
                }
                _ => WriteTarget::default(),
            },
            _ => WriteTarget::default(),
        }
    } else {
        if matches!(
            &root.found.kind,
            SymbolKind::Port(_) | SymbolKind::Variable(_)
        ) {
            return WriteTarget::external();
        }
        let Ok(target) = symbol_table::resolve(path) else {
            return WriteTarget::default();
        };
        match target.found.kind {
            SymbolKind::Port(_)
            | SymbolKind::Variable(_)
            | SymbolKind::ModportVariableMember(_)
            | SymbolKind::StructMember(_)
            | SymbolKind::UnionMember(_)
            | SymbolKind::SystemVerilog => WriteTarget::external(),
            _ => WriteTarget::default(),
        }
    }
}

fn called_function(path: &GenericSymbolPath) -> Option<Symbol> {
    let resolved = symbol_table::resolve(path).ok()?;
    match &resolved.found.kind {
        SymbolKind::Function(x) if !x.is_proto => Some(resolved.found.as_ref().clone()),
        SymbolKind::ModportFunctionMember(x) => symbol_table::get(x.function),
        _ => None,
    }
}

fn path_define_context(path: &GenericSymbolPath) -> DefineContext {
    path.paths
        .first()
        .and_then(|x| scope::token_scope(x.base.id))
        .map(|(_, x)| x)
        .unwrap_or_default()
}

fn add_target(effect: &mut Effect, target: WriteTarget, define_context: DefineContext) {
    if target.external {
        effect.external_writes.insert(define_context.clone());
    }
    if let Some(path) = target.formal {
        effect.formal_writes.insert(FunctionWrite {
            path,
            define_context,
        });
    }
}

fn add_copyout_targets(effect: &mut Effect, func: &FunctionProperty) {
    for port in &func.ports {
        let port_symbol = port.symbol();
        let SymbolKind::Port(property) = &port_symbol.kind else {
            unreachable!();
        };
        let root = GenericSymbolPath::from(&port.token.token);
        let port_context = path_define_context(&root);

        match property.direction {
            Direction::Output => {
                add_target(effect, WriteTarget::formal(root), port_context);
            }
            Direction::Inout => {
                add_target(effect, WriteTarget::external_formal(root), port_context);
            }
            Direction::Modport => {
                let Some((_, Some(modport_symbol))) = property
                    .r#type
                    .trace_user_defined(Some(&port_symbol.namespace))
                else {
                    continue;
                };
                let SymbolKind::Modport(modport) = &modport_symbol.kind else {
                    continue;
                };

                for member_id in &modport.members {
                    let Some(member_symbol) = symbol_table::get(*member_id) else {
                        continue;
                    };
                    let SymbolKind::ModportVariableMember(member) = &member_symbol.kind else {
                        continue;
                    };
                    if !matches!(member.direction, Direction::Output | Direction::Inout) {
                        continue;
                    }

                    let member_path = GenericSymbolPath::from(&member_symbol.token);
                    let Some(context) = port_context.conjoin(&path_define_context(&member_path))
                    else {
                        continue;
                    };
                    let mut path = root.clone();
                    path.append(&member_symbol.token, &[]);
                    add_target(effect, WriteTarget::external_formal(path), context);
                }
            }
            _ => {}
        }
    }
}

/// Reclassifies direct writes after applying the active generic specialization.
/// The fixed-point summary cannot classify a generic instance parameter until
/// its actual instance is known at a call site.
pub fn specialized_direct_effect(
    symbol: &Symbol,
    context: &Context,
) -> (bool, HashSet<GenericSymbolPath>) {
    let SymbolKind::Function(func) = &symbol.kind else {
        return (false, HashSet::default());
    };
    let namespace = symbol.inner_namespace();
    let mut external_write = false;
    let mut formal_writes = HashSet::default();

    for path in &func.write_paths {
        if !path_define_context(path).is_active(&context.config.defines) {
            continue;
        }
        let resolved = context.resolve_path(path.clone());
        let target = classify_write(&resolved, &namespace);
        external_write |= target.external;
        if let Some(path) = target.formal {
            formal_writes.insert(path);
        }
    }

    (external_write, formal_writes)
}

/// Finds one active external write for a diagnostic without retaining write
/// provenance in every function effect summary.
pub struct ExternalWriteTrace {
    pub write: TokenRange,
    pub calls: Vec<TokenRange>,
}

pub fn function_call_generic_maps(
    signature: &Signature,
    callee: &GenericSymbolPath,
) -> Vec<GenericMap> {
    let mut maps = signature.to_generic_map();
    let mut parent = callee.clone();
    parent.paths.pop();

    if !parent.is_empty()
        && let Ok(symbol) = symbol_table::resolve(&parent)
    {
        let mut parent_maps = match &symbol.found.kind {
            SymbolKind::Instance(instance) => instance.type_name.to_generic_maps(),
            SymbolKind::Port(port) => port
                .r#type
                .get_user_defined()
                .map(|ty| ty.path.to_generic_maps())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        parent_maps.append(&mut maps);
        maps = parent_maps;
    }

    maps
}

pub fn find_external_write(
    symbol: &Symbol,
    signature: &Signature,
    context: &mut Context,
) -> Option<ExternalWriteTrace> {
    #[derive(Clone, Default)]
    struct WriteOrigin {
        write: TokenRange,
        // Collected while unwinding, from the innermost call outwards.
        calls: Vec<TokenRange>,
    }

    #[derive(Default)]
    struct WriteOrigins {
        external: Option<WriteOrigin>,
        formal: Vec<(GenericSymbolPath, WriteOrigin)>,
    }

    fn visit(
        symbol: &Symbol,
        signature: &Signature,
        defines: &HashSet<StrId>,
        context: &mut Context,
        visiting: &mut HashSet<Signature>,
        depth: usize,
        depth_limit: usize,
    ) -> WriteOrigins {
        // Effect summaries are computed to a fixed point below. This search
        // only reconstructs one source-level witness for the diagnostic, so a
        // recursive edge can be cut while the caller continues with its other
        // call sites.
        if depth > depth_limit || !visiting.insert(signature.clone()) {
            return WriteOrigins::default();
        }
        let ret = collect(symbol, defines, context, visiting, depth, depth_limit);
        visiting.remove(signature);
        ret
    }

    fn collect(
        symbol: &Symbol,
        defines: &HashSet<StrId>,
        context: &mut Context,
        visiting: &mut HashSet<Signature>,
        depth: usize,
        depth_limit: usize,
    ) -> WriteOrigins {
        let SymbolKind::Function(func) = &symbol.kind else {
            return WriteOrigins::default();
        };
        let namespace = symbol.inner_namespace();
        let mut ret = WriteOrigins::default();

        for path in &func.write_paths {
            if !path_define_context(path).is_active(defines) {
                continue;
            }
            let resolved = context.resolve_path(path.clone());
            let target = classify_write(&resolved, &namespace);
            if target.external {
                ret.external = Some(WriteOrigin {
                    write: path.range,
                    calls: Vec::new(),
                });
                return ret;
            }
            if let Some(formal) = target.formal {
                ret.formal.push((
                    formal,
                    WriteOrigin {
                        write: path.range,
                        calls: Vec::new(),
                    },
                ));
            }
        }

        for call in &func.call_sites {
            if !path_define_context(&call.callee).is_active(defines) {
                continue;
            }
            let Some(mut callee_signature) = Signature::from_path(context, call.callee.clone())
            else {
                continue;
            };
            callee_signature.normalize();
            let Some(callee) = symbol_table::get(callee_signature.symbol) else {
                continue;
            };
            if !matches!(&callee.kind, SymbolKind::Function(func) if !func.is_proto) {
                continue;
            }

            let generic_map = function_call_generic_maps(&callee_signature, &call.callee);
            context.push_generic_map(generic_map);
            let callee_origins = visit(
                &callee,
                &callee_signature,
                defines,
                context,
                visiting,
                depth + 1,
                depth_limit,
            );
            context.pop_generic_map();
            if let Some(mut origin) = callee_origins.external {
                origin.calls.push(call.range);
                ret.external = Some(origin);
                return ret;
            }

            let SymbolKind::Function(callee_func) = &callee.kind else {
                continue;
            };
            for (formal_write, mut origin) in callee_origins.formal {
                let Some(formal_name) = formal_write.paths.first().map(|x| x.base.text) else {
                    continue;
                };
                let Some((formal_index, _)) = callee_func
                    .ports
                    .iter()
                    .enumerate()
                    .find(|(_, port)| port.token.token.text == formal_name)
                else {
                    continue;
                };
                let argument = call
                    .arguments
                    .iter()
                    .find(|argument| argument.name == Some(formal_name))
                    .or_else(|| {
                        call.arguments
                            .iter()
                            .all(|argument| argument.name.is_none())
                            .then(|| call.arguments.get(formal_index))
                            .flatten()
                    });
                let Some(argument) = argument else {
                    continue;
                };
                origin.calls.push(call.range);

                for actual in &argument.targets {
                    let mut mapped = actual.clone();
                    mapped
                        .paths
                        .extend(formal_write.paths.iter().skip(1).cloned());
                    let resolved = context.resolve_path(mapped);
                    let target = classify_write(&resolved, &namespace);
                    if target.external {
                        ret.external = Some(origin.clone());
                        return ret;
                    }
                    if let Some(formal) = target.formal {
                        ret.formal.push((formal, origin.clone()));
                    }
                }
            }
        }

        ret
    }

    let defines = context.config.defines.clone();
    let depth_limit = context.config.function_instance_depth_limit;
    visit(
        symbol,
        signature,
        &defines,
        context,
        &mut HashSet::default(),
        0,
        depth_limit,
    )
    .external
    .map(|mut origin| {
        origin.calls.reverse();
        ExternalWriteTrace {
            write: origin.write,
            calls: origin.calls,
        }
    })
}

fn resolve_side_effects(list: &[Symbol]) {
    let mut direct = HashMap::default();
    for original in list {
        let symbol = symbol_table::get(original.id).unwrap();
        let SymbolKind::Function(func) = &symbol.kind else {
            continue;
        };
        let namespace = symbol.inner_namespace();
        let mut effect = Effect::default();
        // Function output/inout arguments are copied back on return. This is a
        // write even when the function body never assigns the formal argument.
        add_copyout_targets(&mut effect, func);
        for path in &func.write_paths {
            add_target(
                &mut effect,
                classify_write(path, &namespace),
                path_define_context(path),
            );
        }
        direct.insert(symbol.id, effect);
    }

    let mut effects = direct.clone();
    loop {
        let mut next = direct.clone();
        for original in list {
            let symbol = symbol_table::get(original.id).unwrap();
            let SymbolKind::Function(func) = &symbol.kind else {
                continue;
            };
            let namespace = symbol.inner_namespace();
            let effect = next.entry(symbol.id).or_default();

            for call in &func.call_sites {
                let Some(callee) = called_function(&call.callee) else {
                    continue;
                };
                let Some(callee_effect) = effects.get(&callee.id) else {
                    continue;
                };
                let call_context = path_define_context(&call.callee);
                for callee_context in &callee_effect.external_writes {
                    if let Some(context) = call_context.conjoin(callee_context) {
                        effect.external_writes.insert(context);
                    }
                }

                let SymbolKind::Function(callee_func) = &callee.kind else {
                    continue;
                };
                for formal_write in &callee_effect.formal_writes {
                    let Some(context) = call_context.conjoin(&formal_write.define_context) else {
                        continue;
                    };
                    let Some(formal_name) = formal_write.path.paths.first().map(|x| x.base.text)
                    else {
                        continue;
                    };
                    let Some((formal_index, _)) = callee_func
                        .ports
                        .iter()
                        .enumerate()
                        .find(|(_, port)| port.token.token.text == formal_name)
                    else {
                        continue;
                    };
                    let argument = call
                        .arguments
                        .iter()
                        .find(|arg| arg.name == Some(formal_name))
                        .or_else(|| {
                            call.arguments
                                .iter()
                                .all(|arg| arg.name.is_none())
                                .then(|| call.arguments.get(formal_index))
                                .flatten()
                        });
                    let Some(argument) = argument else {
                        continue;
                    };

                    for actual in &argument.targets {
                        let mut mapped = actual.clone();
                        mapped
                            .paths
                            .extend(formal_write.path.paths.iter().skip(1).cloned());
                        mapped.paths.truncate(FORMAL_WRITE_DEPTH_LIMIT);
                        add_target(effect, classify_write(&mapped, &namespace), context.clone());
                    }
                }
            }
        }
        if next == effects {
            break;
        }
        effects = next;
    }

    for (id, effect) in effects {
        let mut symbol = symbol_table::get(id).unwrap();
        let SymbolKind::Function(func) = &mut symbol.kind else {
            unreachable!();
        };
        func.has_side_effect = !effect.external_writes.is_empty();
        func.conditional_effects.side_effect_contexts =
            effect.external_writes.into_iter().collect();
        func.conditional_effects.side_effect_contexts.sort();
        func.formal_writes = effect
            .formal_writes
            .iter()
            .map(|x| x.path.clone())
            .collect();
        func.formal_writes.sort();
        func.formal_writes.dedup();
        func.conditional_effects.formal_write_contexts = effect.formal_writes.into_iter().collect();
        func.conditional_effects.formal_write_contexts.sort();
        symbol_table::update(symbol);
    }
}

fn is_constantable_function(
    func: &FunctionProperty,
    id: SymbolId,
    namespace: &Namespace,
    visited: &mut Vec<SymbolId>,
) -> bool {
    if func.ret.is_none() {
        // constant function should have a return value.
        return false;
    }

    for port in &func.ports {
        let SymbolKind::Port(port) = symbol_table::get(port.symbol).unwrap().kind else {
            unreachable!();
        };

        // constant function has only input ports
        if !matches!(port.direction, Direction::Input) {
            return false;
        }
    }

    for path in &func.reference_paths {
        let Ok(symbol) = symbol_table::resolve(path) else {
            continue;
        };
        if symbol.found.id == id {
            continue;
        }

        match &symbol.found.kind {
            // port and variable should be defined in the given function
            SymbolKind::Port(_) | SymbolKind::Variable(_)
                if !symbol.found.namespace.included(namespace) =>
            {
                return false;
            }
            SymbolKind::Function(x)
                if !x.is_proto && !resolve_constantable(&symbol.found, visited) =>
            {
                return false;
            }
            SymbolKind::Instance(_) => return false,
            _ => {}
        }
    }

    true
}
