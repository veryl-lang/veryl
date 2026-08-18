//! Module-instance traversal used by bottom-up summary construction.

use crate::{
    HashSet,
    ir::{Component, Declaration, InstDeclaration, Ir, Module, Signature},
};

/// Actual instantiated specializations in children-before-parents order.
/// Unevaluable generic templates are not stable bodies and therefore do not
/// claim the same signature as a concrete default specialization.
pub(super) fn module_postorder(ir: &Ir) -> Vec<&Module> {
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
        for inst in walk_insts(module) {
            if let Component::Module(child) = inst.component.as_ref() {
                visit(child, visited, active, order);
            }
        }
        active.remove(&module.signature);
        visited.insert(module.signature.clone());
        order.push(module);
    }

    let mut visited = HashSet::default();
    let mut active = HashSet::default();
    let mut order = Vec::new();
    for component in &ir.components {
        if let Component::Module(module) = component {
            visit(module, &mut visited, &mut active, &mut order);
        }
    }
    order
}

pub(super) fn walk_insts(module: &Module) -> impl Iterator<Item = &InstDeclaration> {
    module
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Inst(inst) => Some(inst.as_ref()),
            _ => None,
        })
}
