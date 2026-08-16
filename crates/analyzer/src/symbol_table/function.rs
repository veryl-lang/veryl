use crate::namespace::{DefineContext, Namespace};
use crate::symbol::{Direction, FunctionProperty, FunctionWrite, Symbol, SymbolId, SymbolKind};
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
    symbol_table::update(symbol);

    visited.pop();

    constantable
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Effect {
    external_writes: HashSet<DefineContext>,
    formal_writes: HashSet<FunctionWrite>,
}

enum WriteTarget {
    None,
    External,
    Formal(GenericSymbolPath),
}

fn classify_write(path: &GenericSymbolPath, namespace: &Namespace) -> WriteTarget {
    let Some(_) = path.paths.first() else {
        return WriteTarget::None;
    };
    let mut root_path = path.clone();
    root_path.paths.truncate(1);
    let Ok(root) = symbol_table::resolve(&root_path) else {
        return WriteTarget::None;
    };

    if root.found.namespace.included(namespace) {
        match &root.found.kind {
            SymbolKind::Variable(_) => WriteTarget::None,
            SymbolKind::Port(port) => match port.direction {
                Direction::Output => WriteTarget::Formal(path.clone()),
                // A modport/inout actual is necessarily external to the process;
                // only an actual LHS reaching here is treated as a write.
                Direction::Modport | Direction::Inout => {
                    let Ok(target) = symbol_table::resolve(path) else {
                        return WriteTarget::None;
                    };
                    match &target.found.kind {
                        SymbolKind::ModportVariableMember(member)
                            if matches!(member.direction, Direction::Output | Direction::Inout) =>
                        {
                            WriteTarget::External
                        }
                        _ if matches!(port.direction, Direction::Inout) => WriteTarget::External,
                        _ => WriteTarget::None,
                    }
                }
                _ => WriteTarget::None,
            },
            _ => WriteTarget::None,
        }
    } else {
        if matches!(
            &root.found.kind,
            SymbolKind::Port(_) | SymbolKind::Variable(_)
        ) {
            return WriteTarget::External;
        }
        let Ok(target) = symbol_table::resolve(path) else {
            return WriteTarget::None;
        };
        match target.found.kind {
            SymbolKind::Port(_)
            | SymbolKind::Variable(_)
            | SymbolKind::ModportVariableMember(_)
            | SymbolKind::StructMember(_)
            | SymbolKind::UnionMember(_) => WriteTarget::External,
            _ => WriteTarget::None,
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
    match target {
        WriteTarget::None => {}
        WriteTarget::External => {
            effect.external_writes.insert(define_context);
        }
        WriteTarget::Formal(path) => {
            effect.formal_writes.insert(FunctionWrite {
                path,
                define_context,
            });
        }
    }
}

/// Finds one active external write for a diagnostic without retaining write
/// provenance in every function effect summary.
pub fn find_external_write<F>(
    symbol: &Symbol,
    defines: &HashSet<StrId>,
    mut resolve_path: F,
) -> Option<TokenRange>
where
    F: FnMut(GenericSymbolPath) -> GenericSymbolPath,
{
    fn visit<F>(
        symbol: &Symbol,
        defines: &HashSet<StrId>,
        resolve_path: &mut F,
        visited: &mut HashSet<SymbolId>,
    ) -> Option<TokenRange>
    where
        F: FnMut(GenericSymbolPath) -> GenericSymbolPath,
    {
        if !visited.insert(symbol.id) {
            return None;
        }
        let SymbolKind::Function(func) = &symbol.kind else {
            return None;
        };
        let namespace = symbol.inner_namespace();

        for path in &func.write_paths {
            if !path_define_context(path).is_active(defines) {
                continue;
            }
            let resolved = resolve_path(path.clone());
            if matches!(classify_write(&resolved, &namespace), WriteTarget::External) {
                return Some(path.range);
            }
        }

        for call in &func.call_sites {
            if !path_define_context(&call.callee).is_active(defines) {
                continue;
            }
            let callee_path = resolve_path(call.callee.clone());
            let Some(callee) = called_function(&callee_path) else {
                continue;
            };

            if let Some(write) = visit(&callee, defines, resolve_path, visited) {
                return Some(write);
            }

            let SymbolKind::Function(callee_func) = &callee.kind else {
                continue;
            };
            for formal_write in callee_func.written_output_paths(defines) {
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

                for actual in &argument.targets {
                    let mut mapped = actual.clone();
                    mapped
                        .paths
                        .extend(formal_write.paths.iter().skip(1).cloned());
                    let resolved = resolve_path(mapped);
                    if matches!(classify_write(&resolved, &namespace), WriteTarget::External) {
                        return Some(actual.range);
                    }
                }
            }
        }

        None
    }

    visit(symbol, defines, &mut resolve_path, &mut HashSet::default())
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
