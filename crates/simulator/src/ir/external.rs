//! Simulator-IR form of user-defined component instances
//! (`inst x: $comp::<name>`).
//!
//! Port directions are unknown until the component library is loaded, so a
//! connection carries both the readable expression (input use) and, for
//! plain variable connections, the variable id (output use). The runtime
//! resolves outputs through `ModuleVariables`, and turns input expressions
//! into pointer-bound `Expression`s at `instantiate` time.

use crate::HashMap;
use crate::ir::Event;
use crate::ir::expression::{Expression, ProtoExpression};
use crate::ir::statement::{
    ProtoComponentArg, ProtoComponentRef, ProtoComponentRefMember, ProtoStatement,
    ProtoTbMethodKind,
};
use crate::ir::variable::{VarOffset, native_bytes as calc_native_bytes};
use crate::simulator_error::SimulatorError;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::ExternalParamValue;
use veryl_component_sys::{PortDesc, PortDescDir};
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug)]
pub struct ProtoExternalComponent {
    /// Instance name in the testbench.
    pub name: StrId,
    /// Component export name (`$comp::<name>`).
    pub component: StrId,
    pub params: Vec<(StrId, ExternalParamValue)>,
    pub connects: Vec<ProtoExternalConnect>,
    /// True for the method-only `var` form; checked against the
    /// component's declared kind at load time.
    pub is_var_form: bool,
    pub token: TokenRange,
}

#[derive(Clone, Debug)]
pub struct ProtoExternalConnect {
    pub port: StrId,
    pub expr: ProtoExpression,
    /// The connected variable when the connection is a plain (unindexed,
    /// unselected) variable reference — the only form usable as a
    /// component output.
    pub output: Option<ExternalOutput>,
    /// Whether the port is offered to the component as an input (false
    /// for modport output members).
    pub input: bool,
    /// Unused-port check group for modport-expanded members.
    pub group: Option<StrId>,
    /// The interface member name of a modport-expanded connection; the
    /// component's ports bind to it by the manifest's (group, member)
    /// record.
    pub member: Option<StrId>,
    /// The variable, and its element, that a clock/reset event fires on.
    /// Matches `output` for plain connections; modport input members carry
    /// it separately.
    pub event_var: Option<(air::VarId, u32)>,
    pub is_clock: bool,
    pub is_reset: bool,
    pub width: u32,
    /// For a variable a method call passes by reference: what the component
    /// is told about the port. It is bound when the component is built and
    /// handed to the component, its clock or reset firing it, from the call.
    pub deferred: Option<Box<PortDesc>>,
    pub token: TokenRange,
}

/// The variable a component output drives, located by storage rather than
/// by id: a component of a nested test module drives a variable of a child
/// scope, whose id means nothing at the top.
#[derive(Clone, Debug)]
pub struct ExternalOutput {
    pub offset: VarOffset,
    pub native_bytes: usize,
    pub width: usize,
    /// Hierarchical variable name, for diagnostics.
    pub name: StrId,
}

/// `instantiate`d form of [`ExternalOutput`].
#[derive(Clone, Debug)]
pub struct OutputTarget {
    pub offset: VarOffset,
    pub ptr: *mut u8,
    pub native_bytes: usize,
    pub width: usize,
    pub name: StrId,
}

/// `instantiate`d form: input expressions are pointer-bound.
pub struct ExternalComponentInst {
    pub name: StrId,
    pub component: StrId,
    pub params: Vec<(StrId, ExternalParamValue)>,
    pub connects: Vec<ExternalConnectInst>,
    pub is_var_form: bool,
    pub token: TokenRange,
}

pub struct ExternalConnectInst {
    pub port: StrId,
    pub expr: Expression,
    pub output: Option<OutputTarget>,
    pub input: bool,
    pub group: Option<StrId>,
    pub member: Option<StrId>,
    pub event_var: Option<air::VarId>,
    pub is_clock: bool,
    pub is_reset: bool,
    pub width: u32,
    pub deferred: Option<Box<PortDesc>>,
    pub token: TokenRange,
}

impl ProtoExternalComponent {
    /// # Safety
    /// Same contract as `ProtoExpression::apply_values_ptr`: the pointers
    /// must reference live buffers of the given lengths.
    pub unsafe fn instantiate(
        &self,
        ff_ptr: *mut u8,
        ff_len: usize,
        comb_ptr: *mut u8,
        comb_len: usize,
        use_4state: bool,
    ) -> ExternalComponentInst {
        let connects = self
            .connects
            .iter()
            .map(|c| ExternalConnectInst {
                port: c.port,
                expr: unsafe {
                    c.expr
                        .apply_values_ptr(ff_ptr, ff_len, comb_ptr, comb_len, use_4state)
                },
                output: c.output.as_ref().map(|x| OutputTarget {
                    offset: x.offset,
                    ptr: unsafe {
                        let base = if x.offset.is_ff() { ff_ptr } else { comb_ptr };
                        base.add(x.offset.raw() as usize)
                    },
                    native_bytes: x.native_bytes,
                    width: x.width,
                    name: x.name,
                }),
                input: c.input,
                group: c.group,
                member: c.member,
                // An element other than the first is left unresolved, so the
                // load reports the connection as having no event source.
                event_var: c
                    .event_var
                    .and_then(|(id, element)| (element == 0).then_some(id)),
                is_clock: c.is_clock,
                is_reset: c.is_reset,
                width: c.width,
                deferred: c.deferred.clone(),
                token: c.token,
            })
            .collect();
        ExternalComponentInst {
            name: self.name,
            component: self.component,
            params: self.params.clone(),
            connects,
            is_var_form: self.is_var_form,
            token: self.token,
        }
    }
}

/// Turns the reference arguments of the component method calls in `events`
/// into deferred connections of the called components. References to one
/// variable with one description share a connection, so calls passing the
/// same output, wherever they are, hand over one driver.
pub fn bind_component_refs(
    events: &mut HashMap<Event, Vec<ProtoStatement>>,
    components: &mut [ProtoExternalComponent],
) -> Result<(), SimulatorError> {
    let mut result = Ok(());
    for (event, stmts) in events {
        if !event.is_initial() && *event != Event::Final {
            continue;
        }
        crate::ir::nested_tb::walk_mut(stmts, &mut |inst, method| {
            let ProtoTbMethodKind::Component { args, .. } = method else {
                return;
            };
            for arg in args {
                let ProtoComponentArg::Ref(r) = arg else {
                    continue;
                };
                let ProtoComponentRef::Unbound {
                    kind,
                    members,
                    token,
                } = r.as_mut()
                else {
                    continue;
                };
                let Some(component) = components.iter_mut().find(|c| c.name == *inst) else {
                    result = Err(SimulatorError::unsupported_description(token));
                    continue;
                };
                let connects = std::mem::take(members)
                    .into_iter()
                    .map(|m| component.deferred_connect(m, *token))
                    .collect();
                **r = ProtoComponentRef::Bound {
                    kind: *kind,
                    connects,
                };
            }
        });
    }
    result
}

impl ProtoExternalComponent {
    fn deferred_connect(&mut self, member: ProtoComponentRefMember, token: TokenRange) -> u32 {
        let dir = member.desc.dir;
        let offset = plain_offset(&member.expr);
        if offset.is_some()
            && let Some(idx) = self.connects.iter().position(|c| {
                c.deferred.as_deref() == Some(&member.desc) && plain_offset(&c.expr) == offset
            })
        {
            return idx as u32;
        }
        let output = match (dir, &member.expr) {
            (
                PortDescDir::Output,
                ProtoExpression::Variable {
                    var_offset,
                    var_full_width,
                    ..
                },
            ) => Some(ExternalOutput {
                offset: *var_offset,
                native_bytes: calc_native_bytes(*var_full_width),
                width: *var_full_width,
                name: resource_table::insert_str(&member.desc.path),
            }),
            _ => None,
        };
        self.connects.push(ProtoExternalConnect {
            port: StrId::default(),
            expr: member.expr,
            output,
            input: dir != PortDescDir::Output,
            group: None,
            member: None,
            event_var: member.event_var.map(|id| (id, 0)),
            is_clock: dir == PortDescDir::Clock,
            is_reset: dir == PortDescDir::Reset,
            width: member.desc.width,
            deferred: Some(Box::new(member.desc)),
            token,
        });
        (self.connects.len() - 1) as u32
    }
}

fn plain_offset(expr: &ProtoExpression) -> Option<VarOffset> {
    match expr {
        ProtoExpression::Variable {
            var_offset,
            select: None,
            dynamic_select: None,
            ..
        } => Some(*var_offset),
        _ => None,
    }
}
