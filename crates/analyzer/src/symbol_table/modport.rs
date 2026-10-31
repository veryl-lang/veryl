use super::{ResolveContext, SymbolTable};
use crate::analyzer_error::{AnalyzerError, MissingModportDependencyKind};
use crate::namespace::Namespace;
use crate::scope;
use crate::symbol::{Direction, FunctionCallSite, FunctionProperty, Symbol, SymbolId, SymbolKind};
use crate::symbol_path::{GenericSymbolPath, SymbolPathNamespace};
use crate::{HashMap, HashSet};
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug, PartialEq, Eq)]
struct MissingDependency {
    kind: MissingModportDependencyKind,
    variable: SymbolId,
}

fn push_unique(list: &mut Vec<MissingDependency>, x: MissingDependency) {
    if !list.contains(&x) {
        list.push(x);
    }
}

impl SymbolTable {
    pub(super) fn check_modport_function(
        &self,
        modports: &[SymbolId],
        errors: &mut Vec<AnalyzerError>,
    ) {
        for modport in modports {
            let Some(modport) = self.get(*modport) else {
                continue;
            };
            let SymbolKind::Modport(x) = &modport.kind else {
                continue;
            };

            let exports = self.modport_exports(&modport);
            for id in &x.members {
                let Some(member) = self.get(*id) else {
                    continue;
                };
                let SymbolKind::ModportFunctionMember(x) = &member.kind else {
                    continue;
                };
                // Members copied by `..same` are checked at the original modport
                if x.generated_from.is_some() {
                    continue;
                }
                let Some(function) = self.get(x.function) else {
                    continue;
                };

                for missing in self.missing_dependencies(&function, &exports) {
                    let Some(variable) = self.get(missing.variable) else {
                        continue;
                    };
                    let token: TokenRange = (&member.token).into();
                    errors.push(AnalyzerError::missing_modport_dependency(
                        missing.kind,
                        &function.token.to_string(),
                        &variable.token.to_string(),
                        &modport.token.to_string(),
                        &token,
                    ));
                }
            }
        }
    }

    /// Maps the variables exported by `modport` to their directions.
    fn modport_exports(&self, modport: &Symbol) -> HashMap<SymbolId, Direction> {
        let mut ret = HashMap::default();
        if let SymbolKind::Modport(x) = &modport.kind {
            for id in &x.members {
                if let Some(member) = self.get(*id)
                    && let SymbolKind::ModportVariableMember(x) = &member.kind
                {
                    ret.insert(x.variable, x.direction);
                }
            }
        }
        ret
    }

    /// Lists interface variables which `function` (including interface functions
    /// called by it) depends on but which are not exported by the modport with the
    /// required direction. The function can be imported by the modport if the
    /// returned list is empty.
    fn missing_dependencies(
        &self,
        function: &Symbol,
        exports: &HashMap<SymbolId, Direction>,
    ) -> Vec<MissingDependency> {
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        self.collect_dependencies(function, &mut reads, &mut writes, &mut HashSet::default());

        let mut ret = Vec::new();
        for variable in reads {
            if !exports.contains_key(&variable) {
                push_unique(
                    &mut ret,
                    MissingDependency {
                        kind: MissingModportDependencyKind::NotExported,
                        variable,
                    },
                );
            }
        }
        for variable in writes {
            if matches!(
                exports.get(&variable),
                Some(direction) if !matches!(direction, Direction::Output | Direction::Inout)
            ) {
                push_unique(
                    &mut ret,
                    MissingDependency {
                        kind: MissingModportDependencyKind::NotWritable,
                        variable,
                    },
                );
            }
        }
        ret
    }

    fn collect_dependencies(
        &self,
        function: &Symbol,
        reads: &mut Vec<SymbolId>,
        writes: &mut Vec<SymbolId>,
        visited: &mut HashSet<SymbolId>,
    ) {
        if !visited.insert(function.id) {
            return;
        }
        let SymbolKind::Function(func) = &function.kind else {
            return;
        };
        if func.is_proto {
            return;
        }

        let namespace = function.inner_namespace();
        for path in &func.reference_paths {
            if let Some(id) = self.external_variable(path, &namespace) {
                reads.push(id);
            }
        }
        for path in &func.write_paths {
            if let Some(id) = self.external_variable(path, &namespace) {
                writes.push(id);
            }
        }

        for call in &func.call_sites {
            let Some(callee) = self.called_function(&call.callee, function) else {
                continue;
            };
            if let SymbolKind::Function(callee_func) = &callee.kind {
                for path in self.output_arguments(call, callee_func) {
                    if let Some(id) = self.external_variable(path, &namespace) {
                        writes.push(id);
                    }
                }
            }
            self.collect_dependencies(&callee, reads, writes, visited);
        }
    }

    /// Resolves `path` at the scope where it is written.
    fn resolve_written_path(&self, path: &GenericSymbolPath) -> Option<Symbol> {
        let SymbolPathNamespace(path, namespace, scope) = path.into();
        let scope = scope.unwrap_or_else(|| scope::intern_namespace(&namespace));
        let context = ResolveContext::from_scope(scope, namespace.define_context);
        let symbol = self.resolve(&path, &[], context).ok()?;
        Some(symbol.found.as_ref().clone())
    }

    /// Resolves the root of `path` to a variable defined outside of the function.
    fn external_variable(
        &self,
        path: &GenericSymbolPath,
        namespace: &Namespace,
    ) -> Option<SymbolId> {
        let mut root = path.clone();
        root.paths.truncate(1);
        let symbol = self.resolve_written_path(&root)?;
        if matches!(symbol.kind, SymbolKind::Variable(_)) && !symbol.namespace.included(namespace) {
            Some(symbol.id)
        } else {
            None
        }
    }

    /// Resolves the callee to a function defined in the same scope as the caller.
    fn called_function(&self, path: &GenericSymbolPath, caller: &Symbol) -> Option<Symbol> {
        let symbol = self.resolve_written_path(path)?;
        if matches!(symbol.kind, SymbolKind::Function(_))
            && symbol.namespace.included(&caller.namespace)
        {
            Some(symbol)
        } else {
            None
        }
    }

    fn output_arguments<'a>(
        &self,
        call: &'a FunctionCallSite,
        callee: &FunctionProperty,
    ) -> Vec<&'a GenericSymbolPath> {
        let positional = call.arguments.iter().all(|x| x.name.is_none());
        let mut ret = Vec::new();
        for (i, port) in callee.ports.iter().enumerate() {
            let Some(SymbolKind::Port(x)) = self.get(port.symbol).map(|x| x.kind) else {
                continue;
            };
            if !matches!(x.direction, Direction::Output | Direction::Inout) {
                continue;
            }
            let name = port.token.token.text;
            let argument = if positional {
                call.arguments.get(i)
            } else {
                call.arguments.iter().find(|x| x.name == Some(name))
            };
            if let Some(argument) = argument {
                ret.extend(argument.targets.iter());
            }
        }
        ret
    }
}
