//! Testbench handles of a test module instantiated under another one.
//!
//! Testbench statements address `$tb` and `$comp` instances by name, and a
//! nested test module's `initial`/`final` blocks merge into the parent's.
//! Each instance boundary therefore prefixes the names its child uses with
//! the instance path (`u.clk`), so equal names in different instances stay
//! apart and the parent can address the child's components by that path.

use crate::HashMap;
use crate::ir::ProtoStatement;
use crate::ir::context::Context;
use crate::ir::event::Event;
use crate::ir::statement::{ProtoComponentArg, ProtoTbMethodKind};
use crate::ir::variable::{VarOffset, VariableMeta};
use std::sync::Arc;
use veryl_analyzer::ir as air;
use veryl_parser::resource_table::{self, StrId};

/// A variable of a nested test module that the testbench runtime reaches
/// by id: a `$tb` clock or reset, keyed by the id its events were re-keyed
/// to, or the destination of a method's return value. `meta.path` is its
/// hierarchical name.
#[derive(Clone, Debug)]
pub struct TbVariable {
    pub id: air::VarId,
    pub meta: VariableMeta,
    /// The hierarchical name testbench statements address a `$tb` clock or
    /// reset by.
    pub handle: Option<StrId>,
}

/// A nested reset's event identity, storage, and declared kind.
pub struct ResetNet {
    pub id: air::VarId,
    pub offset: VarOffset,
    pub kind: air::TypeKind,
}

/// The event a `$tb` clock or reset handle fires.
pub fn handle_event(id: air::VarId, r#type: &air::Type) -> Event {
    if r#type.is_clock() {
        Event::clock(id)
    } else {
        Event::reset(id)
    }
}

/// The clocks and resets among `variables`, by the storage their events
/// fire on.
pub fn signal_offsets<'a>(
    variables: impl IntoIterator<Item = (&'a air::VarId, &'a VariableMeta)>,
) -> Vec<(air::VarId, VarOffset)> {
    variables
        .into_iter()
        .filter(|(_, meta)| meta.r#type.is_clock() || meta.r#type.is_reset())
        .filter_map(|(id, meta)| Some((*id, meta.elements.first()?.current)))
        .collect()
}

/// `hierarchy` and `name` address an instance; `inner` is a name inside it.
pub fn hierarchical_name(hierarchy: &[StrId], name: StrId, inner: StrId) -> StrId {
    let mut text = String::new();
    for x in hierarchy.iter().chain([&name, &inner]) {
        if !text.is_empty() {
            text.push('.');
        }
        text.push_str(&resource_table::get_str_value(*x).unwrap_or_default());
    }
    resource_table::insert_str(&text)
}

/// Whether the subtree under `component` declares a `$tb` generator or a
/// `$comp` instance, or calls a testbench method. Such a subtree is
/// converted per instance: its handles are renamed by the instance path,
/// and its generators are collected for the testbench to drive.
pub fn has_testbench(context: &mut Context, component: &Arc<air::Component>) -> bool {
    let key = Arc::as_ptr(component);
    if let Some(x) = context.testbench_cache.get(&key) {
        return *x;
    }
    let ret = match component.as_ref() {
        air::Component::Module(module) => {
            // A testbench call can also sit in a function the initial calls.
            !module.tb_handles.generators.is_empty()
                || module
                    .functions
                    .values()
                    .any(|x| x.functions.iter().any(|x| calls_testbench(&x.statements)))
                || module.declarations.iter().any(|x| match x {
                    air::Declaration::External(_) => true,
                    air::Declaration::Initial(x) => calls_testbench(&x.statements),
                    air::Declaration::Final(x) => calls_testbench(&x.statements),
                    air::Declaration::Inst(x) => has_testbench(context, &x.component),
                    _ => false,
                })
        }
        _ => false,
    };
    context.testbench_cache.insert(key, ret);
    ret
}

fn calls_testbench(stmts: &[air::Statement]) -> bool {
    stmts.iter().any(|x| match x {
        air::Statement::TbMethodCall(_) => true,
        air::Statement::If(x) => calls_testbench(&x.true_side) || calls_testbench(&x.false_side),
        air::Statement::IfReset(x) => {
            calls_testbench(&x.true_side) || calls_testbench(&x.false_side)
        }
        air::Statement::Case(x) => {
            x.arms.iter().any(|x| calls_testbench(&x.body)) || calls_testbench(&x.default)
        }
        air::Statement::For(x) => calls_testbench(&x.body),
        _ => false,
    })
}

/// Prefixes every testbench handle name in `events` by the instance path.
pub fn rename_handles(
    events: &mut HashMap<Event, Vec<ProtoStatement>>,
    hierarchy: &[StrId],
    name: StrId,
) {
    for (event, stmts) in events.iter_mut() {
        if event.is_initial() || *event == Event::Final {
            walk_mut(stmts, &mut |inst, method| {
                *inst = hierarchical_name(hierarchy, name, *inst);
                if let ProtoTbMethodKind::ResetAssert { clock, .. } = method {
                    *clock = hierarchical_name(hierarchy, name, *clock);
                }
            });
        }
    }
}

/// Remaps the clock and reset subscriptions of component method arguments.
pub fn remap_reference_events(
    events: &mut HashMap<Event, Vec<ProtoStatement>>,
    mut remap: impl FnMut(air::VarId, u32) -> (air::VarId, u32),
) {
    for (event, stmts) in events {
        if event.is_initial() || *event == Event::Final {
            walk_mut(stmts, &mut |_, method| {
                if let ProtoTbMethodKind::Component { args, .. } = method {
                    for arg in args {
                        if let ProtoComponentArg::Ref(reference) = arg {
                            for member in &mut reference.members {
                                if let Some(Event::Clock(id, element) | Event::Reset(id, element)) =
                                    &mut member.event
                                {
                                    (*id, *element) = remap(*id, *element);
                                }
                            }
                        }
                    }
                }
            });
        }
    }
}

/// Replaces the return destination of every testbench method in `events`.
pub fn remap_returns(
    events: &mut HashMap<Event, Vec<ProtoStatement>>,
    mut f: impl FnMut(air::VarId) -> air::VarId,
) {
    for (event, stmts) in events.iter_mut() {
        if event.is_initial() || *event == Event::Final {
            walk_mut(stmts, &mut |_, method| {
                let ret = match method {
                    ProtoTbMethodKind::Component { ret, .. }
                    | ProtoTbMethodKind::RandomGet { ret, .. }
                    | ProtoTbMethodKind::RandomGetRange { ret, .. }
                    | ProtoTbMethodKind::RandomGetSeed { ret }
                    | ProtoTbMethodKind::Queue { ret, .. }
                    | ProtoTbMethodKind::RuntimeArg { ret, .. } => ret,
                    _ => return,
                };
                if let Some((id, _)) = ret {
                    *id = f(*id);
                }
            });
        }
    }
}

fn walk_mut(stmts: &mut [ProtoStatement], f: &mut impl FnMut(&mut StrId, &mut ProtoTbMethodKind)) {
    for stmt in stmts {
        match stmt {
            ProtoStatement::TbMethodCall { inst, method } => f(inst, method),
            ProtoStatement::If(x) => {
                walk_mut(&mut x.true_side, f);
                walk_mut(&mut x.false_side, f);
            }
            ProtoStatement::Case(x) => {
                for arm in &mut x.arms {
                    walk_mut(&mut arm.body, f);
                }
                walk_mut(&mut x.default, f);
            }
            ProtoStatement::For(x) => walk_mut(&mut x.body, f),
            ProtoStatement::SequentialBlock(x) => walk_mut(x, f),
            _ => {}
        }
    }
}
