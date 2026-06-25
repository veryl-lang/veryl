use indent::indent_all_by;
use std::fmt;
use veryl_analyzer::symbol::ClockDomain;
use veryl_parser::resource_table::StrId;

pub type NetId = u32;

// Net 0 and 1 are reserved for constant GND / VDD tie-offs.
pub const NET_CONST0: NetId = 0;
pub const NET_CONST1: NetId = 1;
pub const RESERVED_NETS: u32 = 2;

#[derive(Clone, Default)]
pub struct GateIr {
    pub module: GateModule,
}

impl fmt::Display for GateIr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.module.fmt(f)
    }
}

#[derive(Clone, Default)]
pub struct GateModule {
    pub name: Option<StrId>,
    pub ports: Vec<GatePort>,
    pub nets: Vec<NetInfo>,
    pub cells: Vec<Cell>,
    pub ffs: Vec<FfCell>,
    /// Memory arrays inferred as RAM macros instead of expanded to per-bit
    /// flip-flops plus address mux/decode logic. Empty unless RAM inference
    /// fires (large, single-write-port arrays — see `conv::ram`).
    pub ram_blocks: Vec<RamBlock>,
}

#[derive(Clone)]
pub struct GatePort {
    /// Display name — the first segment of `path`, used in SV emit and tests.
    pub name: StrId,
    /// Full hierarchical path (e.g. `["m_if", "ready"]` for a modport member).
    /// Used to disambiguate ports whose `name` alone would collide.
    pub path: Vec<StrId>,
    pub dir: PortDir,
    pub nets: Vec<NetId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortDir {
    Input,
    Output,
    Inout,
}

impl fmt::Display for PortDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortDir::Input => "input".fmt(f),
            PortDir::Output => "output".fmt(f),
            PortDir::Inout => "inout".fmt(f),
        }
    }
}

#[derive(Clone, Debug)]
pub enum NetDriver {
    /// Only used for NET_CONST0 / NET_CONST1.
    Const(bool),
    PortInput,
    /// Index into `module.cells`.
    Cell(usize),
    /// Index into `module.ffs`.
    FfQ(usize),
    /// A RAM read-port data output: `(ram_block_idx, read_port_idx, bit)`.
    RamRead(usize, usize, usize),
    Undriven,
}

#[derive(Clone, Debug)]
pub struct NetInfo {
    pub driver: NetDriver,
    /// (variable name, bit index) for display; None for synthesized scratch nets.
    pub origin: Option<(StrId, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellKind {
    Buf,
    Not,
    And2,
    Or2,
    Nand2,
    Nor2,
    Xor2,
    Xnor2,
    And3,
    Or3,
    Nand3,
    Nor3,
    /// `(A & B) | C`.
    Ao21,
    /// `!((A & B) | C)`.
    Aoi21,
    /// `(A | B) & C`.
    Oa21,
    /// `!((A | B) & C)`.
    Oai21,
    /// `(A & B & C) | D`.
    Ao31,
    /// `!((A & B & C) | D)`.
    Aoi31,
    /// `(A & B) | (C & D)`.
    Ao22,
    /// `!((A & B) | (C & D))`.
    Aoi22,
    /// `!((A | B) & (C | D))`.
    Oai22,
    /// inputs = [sel, d_when_sel_0, d_when_sel_1]
    Mux2,
}

impl CellKind {
    pub fn arity(self) -> usize {
        match self {
            CellKind::Buf | CellKind::Not => 1,
            CellKind::And2
            | CellKind::Or2
            | CellKind::Nand2
            | CellKind::Nor2
            | CellKind::Xor2
            | CellKind::Xnor2 => 2,
            CellKind::And3
            | CellKind::Or3
            | CellKind::Nand3
            | CellKind::Nor3
            | CellKind::Ao21
            | CellKind::Aoi21
            | CellKind::Oa21
            | CellKind::Oai21
            | CellKind::Mux2 => 3,
            CellKind::Ao31
            | CellKind::Aoi31
            | CellKind::Ao22
            | CellKind::Aoi22
            | CellKind::Oai22 => 4,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            CellKind::Buf => "buf",
            CellKind::Not => "not",
            CellKind::And2 => "and2",
            CellKind::Or2 => "or2",
            CellKind::Nand2 => "nand2",
            CellKind::Nor2 => "nor2",
            CellKind::Xor2 => "xor2",
            CellKind::Xnor2 => "xnor2",
            CellKind::And3 => "and3",
            CellKind::Or3 => "or3",
            CellKind::Nand3 => "nand3",
            CellKind::Nor3 => "nor3",
            CellKind::Ao21 => "ao21",
            CellKind::Aoi21 => "aoi21",
            CellKind::Oa21 => "oa21",
            CellKind::Oai21 => "oai21",
            CellKind::Ao31 => "ao31",
            CellKind::Aoi31 => "aoi31",
            CellKind::Ao22 => "ao22",
            CellKind::Aoi22 => "aoi22",
            CellKind::Oai22 => "oai22",
            CellKind::Mux2 => "mux2",
        }
    }
}

impl fmt::Display for CellKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.symbol().fmt(f)
    }
}

#[derive(Clone)]
pub struct Cell {
    pub kind: CellKind,
    pub inputs: Vec<NetId>,
    pub output: NetId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockEdge {
    Posedge,
    Negedge,
}

impl fmt::Display for ClockEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClockEdge::Posedge => "posedge".fmt(f),
            ClockEdge::Negedge => "negedge".fmt(f),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetPolarity {
    ActiveHigh,
    ActiveLow,
}

impl fmt::Display for ResetPolarity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResetPolarity::ActiveHigh => "high".fmt(f),
            ResetPolarity::ActiveLow => "low".fmt(f),
        }
    }
}

#[derive(Clone)]
pub struct ResetSpec {
    pub net: NetId,
    pub polarity: ResetPolarity,
    pub sync: bool,
}

#[derive(Clone)]
pub struct FfCell {
    pub clock: NetId,
    pub clock_edge: ClockEdge,
    pub reset: Option<ResetSpec>,
    pub d: NetId,
    pub q: NetId,
    pub reset_value: bool,
    pub clock_domain: ClockDomain,
    pub origin: Option<(StrId, usize)>,
}

/// One synchronous write port of a [`RamBlock`]. `data`/`addr`/`enable` are
/// driven by surrounding logic; the write commits on the RAM's clock edge when
/// `enable` is high. `addr`/`data` are LSB-first.
#[derive(Clone)]
pub struct RamWritePort {
    pub addr: Vec<NetId>,
    pub data: Vec<NetId>,
    pub enable: NetId,
}

/// One read port of a [`RamBlock`]. `data` nets are *outputs* — the RAM drives
/// them (their `NetDriver` is `RamRead`). `sync = false` models an
/// asynchronous (combinational) read whose delay is the access time; `true`
/// models a registered read (data valid the cycle after the address).
#[derive(Clone)]
pub struct RamReadPort {
    pub addr: Vec<NetId>,
    pub data: Vec<NetId>,
    pub sync: bool,
}

/// A memory array represented as a single RAM macro rather than expanded into
/// `depth × width` flip-flops plus address decode/mux trees. Inferred for
/// large, memory-like arrays (see `conv::ram`). Area / timing / power are
/// modelled analytically via [`crate::library::SramModel`].
#[derive(Clone)]
pub struct RamBlock {
    pub name: StrId,
    pub depth: usize,
    pub width: usize,
    pub clock: NetId,
    pub clock_edge: ClockEdge,
    pub read_ports: Vec<RamReadPort>,
    pub write_ports: Vec<RamWritePort>,
}

impl RamBlock {
    /// Total stored bits — the basis for area and leakage.
    pub fn bits(&self) -> usize {
        self.depth * self.width
    }
}

impl GateModule {
    /// Nets the RAM blocks *consume* — clock, write addr/data/enable, read addr.
    /// These are DCE roots, fusion consumers, and alias-remap targets; the clock
    /// is handled like an FF clock so a Buf-aliased/gated RAM clock is resolved,
    /// not left dangling. Read-data nets are RAM *outputs* and excluded. The
    /// mutable counterpart visits the same nets in the same order, so
    /// collect-then-reapply stays aligned.
    pub fn for_each_ram_input_net(&self, mut f: impl FnMut(NetId)) {
        for ram in &self.ram_blocks {
            f(ram.clock);
            for wp in &ram.write_ports {
                wp.addr.iter().for_each(|&n| f(n));
                wp.data.iter().for_each(|&n| f(n));
                f(wp.enable);
            }
            for rp in &ram.read_ports {
                rp.addr.iter().for_each(|&n| f(n));
            }
        }
    }

    /// Mutable counterpart of [`Self::for_each_ram_input_net`], for rewriting
    /// consumed nets when a simplify pass aliases them away. Visits nets in the
    /// identical order to the shared (immutable) iterator.
    pub fn for_each_ram_input_net_mut(&mut self, mut f: impl FnMut(&mut NetId)) {
        for ram in &mut self.ram_blocks {
            f(&mut ram.clock);
            for wp in &mut ram.write_ports {
                wp.addr.iter_mut().for_each(&mut f);
                wp.data.iter_mut().for_each(&mut f);
                f(&mut wp.enable);
            }
            for rp in &mut ram.read_ports {
                rp.addr.iter_mut().for_each(&mut f);
            }
        }
    }
}

impl fmt::Display for GateModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();
        let name = match self.name {
            Some(n) => n.to_string(),
            None => "<unnamed>".into(),
        };
        ret.push_str(&format!("gate_module {} {{\n", name));

        for p in &self.ports {
            let nets: Vec<String> = p.nets.iter().map(|n| format!("n{}", n)).collect();
            ret.push_str(&indent_all_by(
                2,
                format!("{} {} : {{{}}};\n", p.dir, p.name, nets.join(", ")),
            ));
        }
        if !self.ports.is_empty() {
            ret.push('\n');
        }

        for (i, cell) in self.cells.iter().enumerate() {
            let ins: Vec<String> = cell.inputs.iter().map(|n| format!("n{}", n)).collect();
            ret.push_str(&indent_all_by(
                2,
                format!(
                    "cell{} {}: n{} = {}({});\n",
                    i,
                    cell.kind,
                    cell.output,
                    cell.kind,
                    ins.join(", ")
                ),
            ));
        }
        if !self.cells.is_empty() {
            ret.push('\n');
        }

        for (i, ff) in self.ffs.iter().enumerate() {
            let origin = match ff.origin {
                Some((name, bit)) => format!(" ({}[{}])", name, bit),
                None => String::new(),
            };
            let reset = match &ff.reset {
                Some(r) => format!(
                    ", reset n{} {} {}",
                    r.net,
                    r.polarity,
                    if r.sync { "sync" } else { "async" }
                ),
                None => String::new(),
            };
            ret.push_str(&indent_all_by(
                2,
                format!(
                    "ff{}{} : n{} <= ({} n{}{}) n{} (rst_value={});\n",
                    i, origin, ff.q, ff.clock_edge, ff.clock, reset, ff.d, ff.reset_value as u8,
                ),
            ));
        }
        if !self.ffs.is_empty() && !self.ram_blocks.is_empty() {
            ret.push('\n');
        }

        for (i, ram) in self.ram_blocks.iter().enumerate() {
            ret.push_str(&indent_all_by(
                2,
                format!(
                    "ram{} ({}) : {}×{}b, {} ({} write, {} read):\n",
                    i,
                    ram.name,
                    ram.depth,
                    ram.width,
                    ram.clock_edge,
                    ram.write_ports.len(),
                    ram.read_ports.len(),
                ),
            ));
            for (w, wp) in ram.write_ports.iter().enumerate() {
                ret.push_str(&indent_all_by(
                    4,
                    format!(
                        "w{}: addr[{}] data[{}] we=n{}\n",
                        w,
                        wp.addr.len(),
                        wp.data.len(),
                        wp.enable,
                    ),
                ));
            }
            for (r, rp) in ram.read_ports.iter().enumerate() {
                ret.push_str(&indent_all_by(
                    4,
                    format!(
                        "r{}: addr[{}] data[{}] {}\n",
                        r,
                        rp.addr.len(),
                        rp.data.len(),
                        if rp.sync { "sync" } else { "async" },
                    ),
                ));
            }
        }

        ret.push('}');
        ret.fmt(f)
    }
}
