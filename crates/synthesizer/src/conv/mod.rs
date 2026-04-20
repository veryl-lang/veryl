pub(crate) mod arith;
pub(crate) mod expression;
pub(crate) mod statement;

use std::collections::HashMap;

use veryl_analyzer::ir::{self as air, Declaration, Statement, VarKind};
use veryl_parser::resource_table::StrId;

use crate::error::SynthError;
use crate::ir::{
    Cell, CellKind, ClockEdge, FfCell, GateModule, GatePort, NetDriver, NetId, NetInfo, PortDir,
    RESERVED_NETS, ResetPolarity, ResetSpec,
};

pub(crate) use statement::process_statements;

pub fn convert_module(module: &air::Module) -> Result<GateModule, SynthError> {
    let mut ctx = ConvContext::new();
    ctx.allocate_variables(module)?;
    ctx.classify_drivers(module)?;

    // FF cells must be built before any declaration processes expressions,
    // so references to FF-driven variables from comb blocks can resolve to
    // the Q net.
    ctx.preallocate_ff_cells(module)?;

    for (idx, decl) in module.declarations.iter().enumerate() {
        ctx.process_declaration(idx, decl)?;
    }

    ctx.finalize(module)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VarDriverKind {
    None,
    Comb(usize),
    Ff(usize),
}

#[derive(Clone)]
pub(crate) struct VarSlot {
    pub nets: Vec<NetId>,
    pub width: usize,
    pub name: StrId,
    pub kind: VarKind,
    pub driver: VarDriverKind,
}

pub(crate) struct PreFf {
    // ff_indices[bit] = index into module.ffs
    pub ff_indices: Vec<usize>,
}

pub(crate) struct ConvContext {
    pub cells: Vec<Cell>,
    pub ffs: Vec<FfCell>,
    pub nets: Vec<NetInfo>,
    pub variables: HashMap<air::VarId, VarSlot>,
    pub ff_allocation: HashMap<air::VarId, PreFf>,
}

impl ConvContext {
    fn new() -> Self {
        let mut nets = Vec::with_capacity(16);
        nets.push(NetInfo {
            driver: NetDriver::Const(false),
            origin: None,
        });
        nets.push(NetInfo {
            driver: NetDriver::Const(true),
            origin: None,
        });
        assert_eq!(nets.len() as u32, RESERVED_NETS);
        Self {
            cells: Vec::new(),
            ffs: Vec::new(),
            nets,
            variables: HashMap::new(),
            ff_allocation: HashMap::new(),
        }
    }

    pub(crate) fn alloc_net(&mut self, origin: Option<(StrId, usize)>) -> NetId {
        let id = self.nets.len() as NetId;
        self.nets.push(NetInfo {
            driver: NetDriver::Undriven,
            origin,
        });
        id
    }

    pub(crate) fn add_cell(&mut self, kind: CellKind, inputs: Vec<NetId>) -> NetId {
        debug_assert_eq!(inputs.len(), kind.arity());
        let out_origin = inputs.first().and_then(|n| self.nets[*n as usize].origin);
        let output = self.alloc_net(out_origin);
        let idx = self.cells.len();
        self.cells.push(Cell {
            kind,
            inputs,
            output,
        });
        self.nets[output as usize].driver = NetDriver::Cell(idx);
        output
    }

    fn allocate_variables(&mut self, module: &air::Module) -> Result<(), SynthError> {
        // Deterministic net numbering simplifies diffing dump output.
        let mut vars: Vec<&air::Variable> = module.variables.values().collect();
        vars.sort_by_key(|v| v.id);
        for v in vars {
            if v.r#type.is_struct()
                || v.r#type.is_union()
                || matches!(
                    v.r#type.kind,
                    air::TypeKind::Enum(_)
                        | air::TypeKind::Module(_)
                        | air::TypeKind::Interface(_)
                        | air::TypeKind::Modport(_, _)
                        | air::TypeKind::Package(_)
                        | air::TypeKind::Instance(_, _)
                        | air::TypeKind::AbstractInterface(_)
                        | air::TypeKind::SystemVerilog
                )
            {
                return Err(SynthError::unsupported(format!(
                    "variable '{}' has unsupported type",
                    v.path
                )));
            }
            if v.r#type.is_array() {
                return Err(SynthError::unsupported(format!(
                    "array variable '{}' is not supported in Phase 1",
                    v.path
                )));
            }

            let width = v
                .r#type
                .total_width()
                .ok_or_else(|| SynthError::UnknownWidth {
                    what: v.path.to_string(),
                })?;
            if width == 0 {
                continue;
            }

            let name = v.path.first();
            let mut nets = Vec::with_capacity(width);
            for bit in 0..width {
                nets.push(self.alloc_net(Some((name, bit))));
            }

            self.variables.insert(
                v.id,
                VarSlot {
                    nets,
                    width,
                    name,
                    kind: v.kind,
                    driver: VarDriverKind::None,
                },
            );
        }
        Ok(())
    }

    fn classify_drivers(&mut self, module: &air::Module) -> Result<(), SynthError> {
        for (idx, decl) in module.declarations.iter().enumerate() {
            match decl {
                Declaration::Comb(x) => {
                    for st in &x.statements {
                        collect_assigned(st, &mut |vid| {
                            if let Some(slot) = self.variables.get_mut(&vid)
                                && slot.driver == VarDriverKind::None
                            {
                                slot.driver = VarDriverKind::Comb(idx);
                            }
                        });
                    }
                }
                Declaration::Ff(x) => {
                    for st in &x.statements {
                        collect_assigned(st, &mut |vid| {
                            if let Some(slot) = self.variables.get_mut(&vid)
                                && slot.driver == VarDriverKind::None
                            {
                                slot.driver = VarDriverKind::Ff(idx);
                            }
                        });
                    }
                }
                Declaration::Inst(_) => {
                    return Err(SynthError::unsupported(
                        "sub-module instantiation (Phase 2+)",
                    ));
                }
                Declaration::Initial(_)
                | Declaration::Final(_)
                | Declaration::Unsupported(_)
                | Declaration::Null => {}
            }
        }
        for slot in self.variables.values() {
            if matches!(slot.kind, VarKind::Input | VarKind::Inout) {
                for &n in &slot.nets {
                    self.nets[n as usize].driver = NetDriver::PortInput;
                }
            }
        }
        Ok(())
    }

    fn build_ports(&mut self, module: &air::Module) -> Vec<GatePort> {
        // module.ports is a HashMap; sort by VarId for deterministic order.
        let mut ports: Vec<(&air::VarPath, &air::VarId)> = module.ports.iter().collect();
        ports.sort_by_key(|p| *p.1);
        let mut result = Vec::new();
        for (_path, vid) in ports {
            let slot = match self.variables.get(vid) {
                Some(s) => s,
                None => continue,
            };
            let dir = match slot.kind {
                VarKind::Input => PortDir::Input,
                VarKind::Output => PortDir::Output,
                VarKind::Inout => PortDir::Inout,
                _ => continue,
            };
            result.push(GatePort {
                name: slot.name,
                dir,
                nets: slot.nets.clone(),
            });
        }
        result
    }

    fn preallocate_ff_cells(&mut self, module: &air::Module) -> Result<(), SynthError> {
        for decl in &module.declarations {
            if let Declaration::Ff(ff_decl) = decl {
                let clock_net = self.resolve_scalar_ref(ff_decl.clock.id, "clock")?;
                let clock_edge = match ff_decl.clock.comptime.r#type.kind {
                    air::TypeKind::ClockNegedge => ClockEdge::Negedge,
                    _ => ClockEdge::Posedge,
                };
                let reset_spec = if let Some(reset) = &ff_decl.reset {
                    let rn = self.resolve_scalar_ref(reset.id, "reset")?;
                    let (polarity, sync) = match reset.comptime.r#type.kind {
                        air::TypeKind::ResetAsyncHigh => (ResetPolarity::ActiveHigh, false),
                        air::TypeKind::ResetAsyncLow => (ResetPolarity::ActiveLow, false),
                        air::TypeKind::ResetSyncHigh => (ResetPolarity::ActiveHigh, true),
                        air::TypeKind::ResetSyncLow => (ResetPolarity::ActiveLow, true),
                        // `reset` without explicit polarity defaults to active-low async.
                        _ => (ResetPolarity::ActiveLow, false),
                    };
                    Some(ResetSpec {
                        net: rn,
                        polarity,
                        sync,
                    })
                } else {
                    None
                };
                let clock_domain = ff_decl.clock.comptime.clock_domain;

                let mut assigned: Vec<air::VarId> = Vec::new();
                for st in &ff_decl.statements {
                    collect_assigned(st, &mut |v| {
                        if !assigned.contains(&v) {
                            assigned.push(v);
                        }
                    });
                }
                for vid in assigned {
                    let width = match self.variables.get(&vid) {
                        Some(s) => s.width,
                        None => continue,
                    };
                    let name = self.variables[&vid].name;
                    let mut ff_indices = Vec::with_capacity(width);
                    for bit in 0..width {
                        let q = self.variables[&vid].nets[bit];
                        let ff_idx = self.ffs.len();
                        self.ffs.push(FfCell {
                            clock: clock_net,
                            clock_edge,
                            reset: reset_spec.clone(),
                            // D is wired up after the FF body is synthesized.
                            d: 0,
                            q,
                            reset_value: false,
                            clock_domain,
                            origin: Some((name, bit)),
                        });
                        self.nets[q as usize].driver = NetDriver::FfQ(ff_idx);
                        ff_indices.push(ff_idx);
                    }
                    self.ff_allocation.insert(vid, PreFf { ff_indices });
                }
            }
        }
        Ok(())
    }

    pub(crate) fn resolve_scalar_ref(
        &self,
        id: air::VarId,
        what: &str,
    ) -> Result<NetId, SynthError> {
        let slot = self
            .variables
            .get(&id)
            .ok_or_else(|| SynthError::Internal(format!("{} variable not found", what)))?;
        if slot.nets.len() != 1 {
            return Err(SynthError::unsupported(format!(
                "multi-bit {} signal is not supported in Phase 1",
                what
            )));
        }
        Ok(slot.nets[0])
    }

    fn process_declaration(
        &mut self,
        decl_idx: usize,
        decl: &Declaration,
    ) -> Result<(), SynthError> {
        match decl {
            Declaration::Comb(x) => {
                let mut current = init_current_comb(self, decl_idx);
                process_statements(self, &x.statements, &mut current)?;
                for (vid, nets) in current {
                    let slot = match self.variables.get(&vid) {
                        Some(s) => s.clone(),
                        None => continue,
                    };
                    if slot.driver != VarDriverKind::Comb(decl_idx) {
                        continue;
                    }
                    for (bit, &src) in nets.iter().take(slot.width).enumerate() {
                        let persistent = slot.nets[bit];
                        if persistent == src {
                            continue;
                        }
                        // Buf's output is the existing persistent net (not a
                        // fresh one) so reads through that net resolve to the
                        // computed value; `add_cell` can't do this.
                        let idx = self.cells.len();
                        self.cells.push(Cell {
                            kind: CellKind::Buf,
                            inputs: vec![src],
                            output: persistent,
                        });
                        self.nets[persistent as usize].driver = NetDriver::Cell(idx);
                    }
                }
                Ok(())
            }
            Declaration::Ff(x) => {
                let mut current = init_current_ff(self, x);
                let (reset_values, main_stmts) = split_if_reset(&x.statements);
                if let Some(reset_map) = reset_values {
                    for (vid, bits) in reset_map {
                        if let Some(pre) = self.ff_allocation.get(&vid) {
                            for (bit, v) in bits.iter().enumerate() {
                                if let Some(ff_idx) = pre.ff_indices.get(bit) {
                                    self.ffs[*ff_idx].reset_value = *v;
                                }
                            }
                        }
                    }
                }
                process_statements(self, &main_stmts, &mut current)?;
                for (vid, nets) in current {
                    let pre = match self.ff_allocation.get(&vid) {
                        Some(p) => p.ff_indices.clone(),
                        None => continue,
                    };
                    for (bit, ff_idx) in pre.iter().enumerate() {
                        let d = nets[bit];
                        self.ffs[*ff_idx].d = d;
                    }
                }
                Ok(())
            }
            Declaration::Inst(_) => Err(SynthError::unsupported(
                "sub-module instantiation (Phase 2+)",
            )),
            Declaration::Initial(_) | Declaration::Final(_) => Ok(()),
            Declaration::Unsupported(_) | Declaration::Null => Ok(()),
        }
    }

    fn finalize(mut self, module: &air::Module) -> Result<GateModule, SynthError> {
        let ports = self.build_ports(module);
        // Tie any undriven output/inout net to GND so downstream analysis
        // doesn't hit an Undriven in the middle of a path.
        for slot in self.variables.values() {
            if matches!(slot.kind, VarKind::Output | VarKind::Inout) {
                for &n in &slot.nets {
                    if matches!(self.nets[n as usize].driver, NetDriver::Undriven) {
                        let idx = self.cells.len();
                        self.cells.push(Cell {
                            kind: CellKind::Buf,
                            inputs: vec![crate::ir::NET_CONST0],
                            output: n,
                        });
                        self.nets[n as usize].driver = NetDriver::Cell(idx);
                    }
                }
            }
        }
        Ok(GateModule {
            name: Some(module.name),
            ports,
            nets: self.nets,
            cells: self.cells,
            ffs: self.ffs,
        })
    }
}

/// Walks nested statements, calling `f` once per `VarId` that appears as an
/// assignment destination (including inside `if` / `for` bodies).
pub(crate) fn collect_assigned(stmt: &Statement, f: &mut impl FnMut(air::VarId)) {
    match stmt {
        Statement::Assign(a) => {
            for d in &a.dst {
                f(d.id);
            }
        }
        Statement::If(i) => {
            for s in &i.true_side {
                collect_assigned(s, f);
            }
            for s in &i.false_side {
                collect_assigned(s, f);
            }
        }
        Statement::IfReset(i) => {
            for s in &i.true_side {
                collect_assigned(s, f);
            }
            for s in &i.false_side {
                collect_assigned(s, f);
            }
        }
        Statement::For(fs) => {
            for s in &fs.body {
                collect_assigned(s, f);
            }
        }
        _ => (),
    }
}

/// Per-bit "current value" map seeded with constant 0. Only variables
/// driven by THIS comb decl are included; otherwise finalize would emit a
/// Buf(0) alias that overwrites another decl's real driver.
fn init_current_comb(ctx: &ConvContext, decl_idx: usize) -> HashMap<air::VarId, Vec<NetId>> {
    let mut map = HashMap::new();
    for (vid, slot) in &ctx.variables {
        if slot.driver == VarDriverKind::Comb(decl_idx) {
            let nets = vec![crate::ir::NET_CONST0; slot.width];
            map.insert(*vid, nets);
        }
    }
    map
}

/// Per-bit "current value" map seeded with each variable's Q net. This
/// gives "hold" semantics: a bit that isn't reassigned in the FF body ends
/// up with D = Q.
fn init_current_ff(ctx: &ConvContext, ff: &air::FfDeclaration) -> HashMap<air::VarId, Vec<NetId>> {
    let mut map = HashMap::new();
    let mut assigned: Vec<air::VarId> = Vec::new();
    for st in &ff.statements {
        collect_assigned(st, &mut |v| {
            if !assigned.contains(&v) {
                assigned.push(v);
            }
        });
    }
    for vid in assigned {
        if let Some(slot) = ctx.variables.get(&vid) {
            map.insert(vid, slot.nets.clone());
        }
    }
    map
}

/// If the FF body is a single top-level `if_reset`, return the constant
/// reset values and the "normal clocked" statements from the else branch.
/// Otherwise return the body as-is with no reset values.
fn split_if_reset(stmts: &[Statement]) -> (Option<HashMap<air::VarId, Vec<bool>>>, Vec<Statement>) {
    if stmts.len() == 1
        && let Statement::IfReset(ifreset) = &stmts[0]
    {
        let mut reset_map: HashMap<air::VarId, Vec<bool>> = HashMap::new();
        if extract_constant_assigns(&ifreset.true_side, &mut reset_map).is_ok() {
            return (Some(reset_map), ifreset.false_side.clone());
        }
        // Non-constant reset expression: drop the reset branch; FFs keep
        // reset_value = 0.
        return (None, ifreset.false_side.clone());
    }
    (None, stmts.to_vec())
}

fn extract_constant_assigns(
    stmts: &[Statement],
    map: &mut HashMap<air::VarId, Vec<bool>>,
) -> Result<(), ()> {
    for s in stmts {
        match s {
            Statement::Assign(a) => {
                let width = a.width.unwrap_or(0);
                if width == 0 {
                    return Err(());
                }
                let value = eval_constant_bits(&a.expr, width).ok_or(())?;
                for d in &a.dst {
                    if !d.select.is_empty() || !d.index.0.is_empty() {
                        return Err(());
                    }
                    map.insert(d.id, value.clone());
                }
            }
            _ => return Err(()),
        }
    }
    Ok(())
}

fn eval_constant_bits(expr: &air::Expression, width: usize) -> Option<Vec<bool>> {
    use veryl_analyzer::ir::Factor;
    if let air::Expression::Term(factor) = expr
        && let Factor::Value(ct) = factor.as_ref()
    {
        let value = ct.get_value().ok()?;
        let n = value.to_u64()?;
        let mut bits = Vec::with_capacity(width);
        for i in 0..width {
            bits.push((n >> i) & 1 != 0);
        }
        return Some(bits);
    }
    None
}
