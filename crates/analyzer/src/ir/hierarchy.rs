//! Module-instance traversal over concrete specializations.

use crate::HashSet;
use crate::ir::{Component, Declaration, InstDeclaration, Ir, Module, Signature};

impl Ir {
    /// Actual instantiated specializations in children-before-parents order.
    /// Unevaluable generic templates are not stable bodies and therefore do
    /// not claim the same signature as a concrete default specialization.
    pub fn module_postorder(&self) -> Vec<&Module> {
        fn visit<'a>(
            module: &'a Module,
            visited: &mut HashSet<Signature>,
            active: &mut HashSet<Signature>,
            order: &mut Vec<&'a Module>,
        ) {
            if module.suppress_unassigned
                || visited.contains(&module.signature)
                || !active.insert(module.signature.clone())
            {
                return;
            }
            for child in module.child_modules() {
                visit(child, visited, active, order);
            }
            active.remove(&module.signature);
            visited.insert(module.signature.clone());
            order.push(module);
        }

        let mut visited = HashSet::default();
        let mut active = HashSet::default();
        let mut order = Vec::new();
        for component in &self.components {
            if let Component::Module(module) = component {
                visit(module, &mut visited, &mut active, &mut order);
            }
        }
        order
    }
}

impl Module {
    /// The modules this module instantiates directly.
    pub fn child_modules(&self) -> impl Iterator<Item = &Module> {
        self.insts()
            .filter_map(|inst| match inst.component.as_ref() {
                Component::Module(child) => Some(child),
                _ => None,
            })
    }

    pub fn insts(&self) -> impl Iterator<Item = &InstDeclaration> {
        self.declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Inst(inst) => Some(inst.as_ref()),
                _ => None,
            })
    }
}
