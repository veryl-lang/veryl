//! Simulator-IR form of user-defined component instances
//! (`inst x: $comp::<name>`).
//!
//! Port directions are unknown until the component library is loaded, so a
//! connection carries both the readable expression (input use) and, for
//! plain variable connections, the variable id (output use). The runtime
//! resolves outputs through `ModuleVariables`, and turns input expressions
//! into pointer-bound `Expression`s at `instantiate` time.

use crate::ir::expression::{Expression, ProtoExpression};
use crate::ir::variable::VarOffset;
use veryl_analyzer::ir as air;
use veryl_analyzer::ir::ExternalParamValue;
use veryl_parser::resource_table::StrId;
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
    /// The variable that a clock/reset event fires on. Matches `output`
    /// for plain connections; modport input members carry it separately.
    pub event_var: Option<air::VarId>,
    pub is_clock: bool,
    pub is_reset: bool,
    pub width: u32,
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
                event_var: c.event_var,
                is_clock: c.is_clock,
                is_reset: c.is_reset,
                width: c.width,
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
