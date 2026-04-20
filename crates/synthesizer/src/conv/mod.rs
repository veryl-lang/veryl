pub(crate) mod arith;
pub(crate) mod expression;
pub(crate) mod statement;

use std::collections::{HashMap, HashSet};

use veryl_analyzer::ir::{self as air, Declaration, Function, Shape, Statement, VarKind};
use veryl_parser::resource_table::StrId;

use crate::ir::{
    Cell, CellKind, ClockEdge, FfCell, GateModule, GatePort, NetDriver, NetId, NetInfo, PortDir,
    RESERVED_NETS, ResetPolarity, ResetSpec,
};
use crate::synthesizer_error::{SynthesizerError, UnsupportedKind};

pub(crate) use statement::process_statements;

pub fn convert_module(module: &air::Module) -> Result<GateModule, SynthesizerError> {
    let functions: HashMap<air::VarId, Function> = module
        .functions
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    let mut ctx = ConvContext::new(functions);
    ctx.allocate_variables(module)?;
    ctx.classify_drivers(module)?;

    // FF cells must be built before any declaration processes expressions,
    // so references to FF-driven variables from comb blocks can resolve to
    // the Q net.
    ctx.preallocate_ff_cells(module)?;

    for (idx, decl) in module.declarations.iter().enumerate() {
        ctx.process_declaration(idx, decl, &module.token)?;
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
    /// Total number of bits (`scalar_width * shape.total()`).
    pub width: usize,
    /// Per-element bit width (without array multiplication).
    pub scalar_width: usize,
    /// Array dimensions. Empty shape = scalar.
    pub shape: Shape,
    pub name: StrId,
    pub kind: VarKind,
    pub driver: VarDriverKind,
}

pub(crate) struct PreFf {
    pub ff_indices: Vec<usize>,
}

pub(crate) struct ConvContext {
    pub cells: Vec<Cell>,
    pub ffs: Vec<FfCell>,
    pub nets: Vec<NetInfo>,
    pub variables: HashMap<air::VarId, VarSlot>,
    pub ff_allocation: HashMap<air::VarId, PreFf>,
    /// User-defined functions available for inline expansion at call sites.
    /// Cloned from `Module.functions` because expression synthesis needs
    /// access during `Factor::FunctionCall` handling and the module borrow
    /// wouldn't survive the recursive convert_module calls for child insts.
    pub functions: HashMap<air::VarId, Function>,
}

impl ConvContext {
    fn new(functions: HashMap<air::VarId, Function>) -> Self {
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
            functions,
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

    fn allocate_variables(&mut self, module: &air::Module) -> Result<(), SynthesizerError> {
        // Deterministic net numbering simplifies diffing dump output.
        let mut vars: Vec<&air::Variable> = module.variables.values().collect();
        vars.sort_by_key(|v| v.id);
        for v in vars {
            let meta_type_name = match &v.r#type.kind {
                air::TypeKind::Module(_) => Some("module"),
                air::TypeKind::Interface(_) => Some("interface"),
                air::TypeKind::Modport(_, _) => Some("modport"),
                air::TypeKind::Package(_) => Some("package"),
                air::TypeKind::Instance(_, _) => Some("instance"),
                air::TypeKind::AbstractInterface(_) => Some("abstract interface"),
                air::TypeKind::SystemVerilog => Some("SystemVerilog"),
                _ => None,
            };
            if let Some(type_kind) = meta_type_name {
                return Err(SynthesizerError::unsupported(
                    UnsupportedKind::UnsupportedVariableType {
                        path: v.path.to_string(),
                        type_kind: type_kind.to_string(),
                    },
                    &v.token,
                ));
            }
            let scalar_width = v.r#type.total_width().ok_or_else(|| {
                SynthesizerError::unknown_width(
                    format!(
                        "{} (width unresolved — likely an uninstantiated generic parameter)",
                        v.path
                    ),
                    &v.token,
                )
            })?;
            let shape = v.r#type.array.clone();
            let element_count = shape.total().ok_or_else(|| {
                SynthesizerError::unknown_width(
                    format!(
                        "{} (array dim unresolved — likely an uninstantiated generic parameter)",
                        v.path
                    ),
                    &v.token,
                )
            })?;
            let width = scalar_width * element_count;
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
                    scalar_width,
                    shape,
                    name,
                    kind: v.kind,
                    driver: VarDriverKind::None,
                },
            );
        }
        Ok(())
    }

    fn classify_drivers(&mut self, module: &air::Module) -> Result<(), SynthesizerError> {
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
                Declaration::Inst(x) => {
                    // Treat the Inst's output destinations as combinational
                    // drivers owned by this decl; finalize emits Buf aliases
                    // the same way as an always_comb block.
                    for output in &x.outputs {
                        for dst in &output.dst {
                            if let Some(slot) = self.variables.get_mut(&dst.id)
                                && slot.driver == VarDriverKind::None
                            {
                                slot.driver = VarDriverKind::Comb(idx);
                            }
                        }
                    }
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
        for (path, vid) in ports {
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
                path: path.0.clone(),
                dir,
                nets: slot.nets.clone(),
            });
        }
        result
    }

    fn preallocate_ff_cells(&mut self, module: &air::Module) -> Result<(), SynthesizerError> {
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
    ) -> Result<NetId, SynthesizerError> {
        let slot = self
            .variables
            .get(&id)
            .ok_or_else(|| SynthesizerError::internal(format!("{} variable not found", what)))?;
        // Clock / reset types are 1-bit scalars by analyzer construction
        // (TypeKind::ClockPosedge etc.), so a multi-bit signal here would be
        // an analyzer regression rather than an unsupported user construct.
        if slot.nets.len() != 1 {
            return Err(SynthesizerError::internal(format!(
                "{} signal is multi-bit ({} nets)",
                what,
                slot.nets.len()
            )));
        }
        Ok(slot.nets[0])
    }

    fn process_declaration(
        &mut self,
        decl_idx: usize,
        decl: &Declaration,
        module_token: &veryl_parser::token_range::TokenRange,
    ) -> Result<(), SynthesizerError> {
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
            Declaration::Inst(inst) => {
                let mut current = init_current_comb(self, decl_idx);
                self.flatten_inst(inst, &mut current, module_token)?;
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
            Declaration::Initial(_) | Declaration::Final(_) => Ok(()),
            Declaration::Unsupported(_) | Declaration::Null => Ok(()),
        }
    }

    fn flatten_inst(
        &mut self,
        inst: &air::InstDeclaration,
        current: &mut HashMap<air::VarId, Vec<NetId>>,
        module_token: &veryl_parser::token_range::TokenRange,
    ) -> Result<(), SynthesizerError> {
        let child_module = match &inst.component {
            air::Component::Module(m) => m,
            air::Component::Interface(_) => {
                // Analyzer flattens interface instantiations to Declaration::Null
                // before they reach us, so this branch should be unreachable.
                return Err(SynthesizerError::internal(
                    "Component::Interface unexpectedly reached synthesizer",
                ));
            }
            air::Component::SystemVerilog(_) => {
                return Err(SynthesizerError::unsupported(
                    UnsupportedKind::SystemVerilogBlackbox,
                    module_token,
                ));
            }
        };

        let child_gate = convert_module(child_module)?;
        let mut net_map: Vec<NetId> = Vec::with_capacity(child_gate.nets.len());
        net_map.push(crate::ir::NET_CONST0);
        net_map.push(crate::ir::NET_CONST1);
        for _ in (RESERVED_NETS as usize)..child_gate.nets.len() {
            net_map.push(self.alloc_net(None));
        }

        // Keyed by the full hierarchical path because modport expansion produces
        // multiple ports sharing the same `name` (e.g. `m_if.ready`, `m_if.valid`).
        let mut child_port: HashMap<Vec<StrId>, (PortDir, Vec<NetId>)> = HashMap::new();
        for port in &child_gate.ports {
            child_port.insert(port.path.clone(), (port.dir, port.nets.clone()));
        }

        for input in &inst.inputs {
            let input_token = &input.expr.comptime().token;
            if input.id.len() != 1 {
                return Err(SynthesizerError::unsupported(
                    UnsupportedKind::BundledInstInput,
                    input_token,
                ));
            }
            let vid = input.id[0];
            let port_path = child_module
                .variables
                .get(&vid)
                .map(|v| v.path.0.clone())
                .ok_or_else(|| {
                    SynthesizerError::internal(format!("inst input port {} not found", vid))
                })?;
            let (dir, child_nets) = child_port.get(&port_path).cloned().ok_or_else(|| {
                SynthesizerError::internal(format!("child port {:?} missing", port_path))
            })?;
            if !matches!(dir, PortDir::Input | PortDir::Inout) {
                return Err(SynthesizerError::internal(format!(
                    "port {:?} on child is not input-capable",
                    port_path
                )));
            }
            let expr_nets = crate::conv::expression::synthesize_expr(
                self,
                &input.expr,
                current,
                child_nets.len(),
            )?;
            for (cn, en) in child_nets.iter().zip(expr_nets.iter()) {
                net_map[*cn as usize] = *en;
            }
        }

        // Cells whose output maps to a parent-driven net (input-port
        // short-circuit) become no-ops, but elide_bufs will strip them later.
        for cell in &child_gate.cells {
            let new_inputs: Vec<NetId> = cell.inputs.iter().map(|n| net_map[*n as usize]).collect();
            let new_output = net_map[cell.output as usize];
            let new_idx = self.cells.len();
            self.cells.push(Cell {
                kind: cell.kind,
                inputs: new_inputs,
                output: new_output,
            });
            // Skip remapped ports — they already have upstream drivers.
            if matches!(self.nets[new_output as usize].driver, NetDriver::Undriven) {
                self.nets[new_output as usize].driver = NetDriver::Cell(new_idx);
            }
        }
        for ff in &child_gate.ffs {
            let clock = net_map[ff.clock as usize];
            let d = net_map[ff.d as usize];
            let q = net_map[ff.q as usize];
            let reset = ff.reset.as_ref().map(|r| ResetSpec {
                net: net_map[r.net as usize],
                polarity: r.polarity,
                sync: r.sync,
            });
            let new_ff_idx = self.ffs.len();
            self.ffs.push(FfCell {
                clock,
                clock_edge: ff.clock_edge,
                reset,
                d,
                q,
                reset_value: ff.reset_value,
                clock_domain: ff.clock_domain,
                origin: ff.origin,
            });
            if matches!(self.nets[q as usize].driver, NetDriver::Undriven) {
                self.nets[q as usize].driver = NetDriver::FfQ(new_ff_idx);
            }
        }

        for output in &inst.outputs {
            let output_token = output.dst.first().map(|d| &d.token).unwrap_or(module_token);
            if output.id.len() != 1 {
                return Err(SynthesizerError::unsupported(
                    UnsupportedKind::BundledInstOutput,
                    output_token,
                ));
            }
            if output.dst.is_empty() {
                // `y: _` — unconnected, child's output nets are discarded.
                continue;
            }
            let vid = output.id[0];
            let port_path = child_module
                .variables
                .get(&vid)
                .map(|v| v.path.0.clone())
                .ok_or_else(|| {
                    SynthesizerError::internal(format!("inst output port {} not found", vid))
                })?;
            let (dir, child_nets) = child_port.get(&port_path).cloned().ok_or_else(|| {
                SynthesizerError::internal(format!("child port {:?} missing", port_path))
            })?;
            if !matches!(dir, PortDir::Output | PortDir::Inout) {
                return Err(SynthesizerError::internal(format!(
                    "port {:?} on child is not output-capable",
                    port_path
                )));
            }
            let parent_nets: Vec<NetId> = child_nets.iter().map(|n| net_map[*n as usize]).collect();

            // Multi-dst (concat-LHS shape `y: {a, b}`) slices MSB-first so
            // dst[0] gets the high bits.
            let mut widths: Vec<usize> = Vec::with_capacity(output.dst.len());
            for dst in &output.dst {
                widths.push(statement::dst_slice_width(self, dst)?);
            }
            let total_dst_width: usize = widths.iter().sum();
            if total_dst_width > parent_nets.len() {
                return Err(SynthesizerError::internal(format!(
                    "inst output dst width {} exceeds child port width {}",
                    total_dst_width,
                    parent_nets.len()
                )));
            }
            let mut lo = 0;
            for (dst, w) in output.dst.iter().zip(widths.iter()).rev() {
                let slice = parent_nets[lo..lo + w].to_vec();
                statement::write_to_dst(self, dst, &slice, current)?;
                lo += w;
            }
        }

        Ok(())
    }

    fn finalize(mut self, module: &air::Module) -> Result<GateModule, SynthesizerError> {
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
        let mut gate = GateModule {
            name: Some(module.name),
            ports,
            nets: self.nets,
            cells: self.cells,
            ffs: self.ffs,
        };
        elide_bufs(&mut gate);
        Ok(gate)
    }
}

/// Removes `Buf` cells by routing their consumers directly to the Buf's input.
/// Chains of `Buf(Buf(x))` collapse to `x`. Buf cells are allocated during
/// comb finalize just to alias the "current computed value" back onto a
/// variable's persistent net; once gate IR construction is done they have no
/// physical meaning and only inflate area/timing reports.
fn elide_bufs(module: &mut GateModule) {
    let mut buf_alias: HashMap<NetId, NetId> = HashMap::new();
    for cell in &module.cells {
        if cell.kind == CellKind::Buf {
            buf_alias.insert(cell.output, cell.inputs[0]);
        }
    }
    if buf_alias.is_empty() {
        return;
    }

    for cell in module.cells.iter_mut() {
        if cell.kind == CellKind::Buf {
            continue;
        }
        for inp in cell.inputs.iter_mut() {
            *inp = resolve_buf(*inp, &buf_alias);
        }
    }
    for ff in module.ffs.iter_mut() {
        ff.d = resolve_buf(ff.d, &buf_alias);
        ff.clock = resolve_buf(ff.clock, &buf_alias);
        if let Some(r) = ff.reset.as_mut() {
            r.net = resolve_buf(r.net, &buf_alias);
        }
    }
    for port in module.ports.iter_mut() {
        for n in port.nets.iter_mut() {
            *n = resolve_buf(*n, &buf_alias);
        }
    }

    let old = std::mem::take(&mut module.cells);
    let mut index_map: Vec<Option<usize>> = vec![None; old.len()];
    let mut new_cells: Vec<Cell> = Vec::with_capacity(old.len());
    for (old_idx, cell) in old.into_iter().enumerate() {
        if cell.kind == CellKind::Buf {
            continue;
        }
        index_map[old_idx] = Some(new_cells.len());
        new_cells.push(cell);
    }
    module.cells = new_cells;

    for net in module.nets.iter_mut() {
        if let NetDriver::Cell(idx) = &mut net.driver {
            match index_map[*idx] {
                Some(new_idx) => *idx = new_idx,
                None => net.driver = NetDriver::Undriven,
            }
        }
    }
}

fn resolve_buf(net: NetId, buf_alias: &HashMap<NetId, NetId>) -> NetId {
    let mut cur = net;
    let mut seen: HashSet<NetId> = HashSet::new();
    while let Some(&next) = buf_alias.get(&cur) {
        if !seen.insert(cur) {
            break;
        }
        cur = next;
    }
    cur
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
