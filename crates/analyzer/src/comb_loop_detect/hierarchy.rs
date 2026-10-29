//! Module-instance traversal used by bottom-up summary construction.

use crate::ir::{Component, Declaration, InstDeclaration, Module};

/// The modules `module` instantiates directly.
pub(super) fn child_modules(module: &Module) -> impl Iterator<Item = &Module> {
    walk_insts(module).filter_map(|inst| match inst.component.as_ref() {
        Component::Module(child) => Some(child),
        _ => None,
    })
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
