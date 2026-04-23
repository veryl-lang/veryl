use crate::namespace::Namespace;
use crate::symbol::{Direction, FunctionProperty, Symbol, SymbolId, SymbolKind};
use crate::symbol_table;

pub fn resolve_function(list: &[Symbol]) {
    for symbol in list {
        resolve_constantable(symbol);
    }
}

fn resolve_constantable(symbol: &Symbol) -> bool {
    match &symbol.kind {
        SymbolKind::Function(func) | SymbolKind::ProtoFunction(func) => {
            if let Some(constantable) = func.constantable {
                return constantable;
            }
        }
        _ => {}
    }

    let namespace = symbol.inner_namespace();
    let mut symbol = symbol.clone();
    let func = match &mut symbol.kind {
        SymbolKind::Function(func) | SymbolKind::ProtoFunction(func) => func,
        _ => unreachable!(),
    };

    let constantable = is_constantable_function(func, symbol.id, &namespace);
    func.constantable = Some(constantable);
    func.reference_paths.clear();

    symbol_table::update(symbol);

    constantable
}

fn is_constantable_function(func: &FunctionProperty, id: SymbolId, namespace: &Namespace) -> bool {
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
            SymbolKind::Function(_) if !resolve_constantable(&symbol.found) => {
                return false;
            }
            SymbolKind::Instance(_) => return false,
            _ => {}
        }
    }

    true
}
