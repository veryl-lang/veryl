// Cell area/delay/power figures approximated from the SkyWater SKY130 open
// PDK, specifically the sky130_fd_sc_hd (high-density) standard-cell library,
// drive strength 1, typical process corner (tt_025C_1v80), fanout-of-4 load.
// Source: https://github.com/google/skywater-pdk (Apache License 2.0).
//
// The numbers embedded below are coarse-grained engineering approximations
// derived from the published Liberty (.lib) characterization data; they are
// factual measurements (not copyrightable expression) and are used here only
// as reference ratios for area / timing / power estimation. We are not
// redistributing any Liberty source, schematic, layout, or other copyrighted
// material from the PDK — see the SkyWater project for full license terms
// and primary data.
//
// Because input slew / output load / corner selection all materially affect
// real Liberty numbers, these values should be read as a self-consistent
// calibration of "relative cost" (not bit-accurate delays). Tune them if
// your target PDK differs.
use crate::ir::CellKind;

/// Per-cell area (um²), intrinsic delay (ns), leakage power (nW), and
/// internal switching energy (pJ per output transition).
#[derive(Clone, Copy, Debug)]
pub struct CellInfo {
    pub area: f64,
    pub delay: f64,
    pub leakage: f64,
    pub internal_energy: f64,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct BuiltinLibrary;

impl BuiltinLibrary {
    pub fn new() -> Self {
        Self
    }

    /// Human-readable banner summarising the calibration source, printed in
    /// reports so readers can trace the numeric basis.
    pub fn banner(&self) -> &'static str {
        "SKY130 (SkyWater 130nm) / sky130_fd_sc_hd / tt_025C_1v80 FO4 \
         (approximate, no wire load)"
    }

    pub fn info(&self, kind: CellKind) -> CellInfo {
        match kind {
            // Buf is elided before reporting; cost is zeroed out so dangling
            // leftovers don't inflate numbers.
            CellKind::Buf => CellInfo {
                area: 0.0,
                delay: 0.0,
                leakage: 0.0,
                internal_energy: 0.0,
            },
            // sky130_fd_sc_hd__inv_1 — smallest cell; fast PMOS/NMOS stacks.
            CellKind::Not => CellInfo {
                area: 2.50,
                delay: 0.02,
                leakage: 0.06,
                internal_energy: 0.0018,
            },
            // sky130_fd_sc_hd__nand2_1 — reference for "NAND2-equivalent".
            CellKind::Nand2 => CellInfo {
                area: 3.75,
                delay: 0.05,
                leakage: 0.11,
                internal_energy: 0.0038,
            },
            // sky130_fd_sc_hd__nor2_1 — same area as NAND2, slightly slower
            // due to PMOS series stack.
            CellKind::Nor2 => CellInfo {
                area: 3.75,
                delay: 0.06,
                leakage: 0.10,
                internal_energy: 0.0045,
            },
            // sky130_fd_sc_hd__and2_1 ≈ NAND2 + INV internally.
            CellKind::And2 => CellInfo {
                area: 5.00,
                delay: 0.08,
                leakage: 0.13,
                internal_energy: 0.0048,
            },
            // sky130_fd_sc_hd__or2_1 ≈ NOR2 + INV internally.
            CellKind::Or2 => CellInfo {
                area: 5.00,
                delay: 0.09,
                leakage: 0.12,
                internal_energy: 0.0055,
            },
            // sky130_fd_sc_hd__xor2_1 — complex transmission-gate XOR,
            // noticeably larger and slower than the primary NAND/NOR.
            CellKind::Xor2 => CellInfo {
                area: 8.75,
                delay: 0.12,
                leakage: 0.44,
                internal_energy: 0.0120,
            },
            // sky130_fd_sc_hd__xnor2_1 — same topology as XOR2.
            CellKind::Xnor2 => CellInfo {
                area: 8.75,
                delay: 0.12,
                leakage: 0.42,
                internal_energy: 0.0115,
            },
            // sky130_fd_sc_hd__and3_1.
            CellKind::And3 => CellInfo {
                area: 6.25,
                delay: 0.10,
                leakage: 0.18,
                internal_energy: 0.0075,
            },
            // sky130_fd_sc_hd__or3_1.
            CellKind::Or3 => CellInfo {
                area: 6.25,
                delay: 0.12,
                leakage: 0.17,
                internal_energy: 0.0080,
            },
            // sky130_fd_sc_hd__nand3_1 — cheaper than nand2 + and2 cascade.
            CellKind::Nand3 => CellInfo {
                area: 5.00,
                delay: 0.06,
                leakage: 0.15,
                internal_energy: 0.0060,
            },
            // sky130_fd_sc_hd__nor3_1.
            CellKind::Nor3 => CellInfo {
                area: 5.00,
                delay: 0.08,
                leakage: 0.14,
                internal_energy: 0.0065,
            },
            // sky130_fd_sc_hd__a21oi_1 — single cell realizing
            // `!((A&B)|C)`; crucial for mux-heavy decoders since AOI
            // compound cells are how sky130 compresses mux chains.
            CellKind::Aoi21 => CellInfo {
                area: 5.00,
                delay: 0.04,
                leakage: 0.15,
                internal_energy: 0.0070,
            },
            // sky130_fd_sc_hd__o21ai_0.
            CellKind::Oai21 => CellInfo {
                area: 5.00,
                delay: 0.04,
                leakage: 0.14,
                internal_energy: 0.0072,
            },
            // sky130_fd_sc_hd__mux2_1 — 2:1 select with transmission-gate
            // implementation; heavier on select→out than data→out path.
            CellKind::Mux2 => CellInfo {
                area: 10.00,
                delay: 0.15,
                leakage: 0.35,
                internal_energy: 0.0130,
            },
        }
    }

    /// FF D-pin setup requirement, derived from sky130_fd_sc_hd__dfrtp_1
    /// setup_rising under typical corner.
    pub fn ff_setup(&self) -> f64 {
        0.15
    }

    /// sky130_fd_sc_hd__dfrtp_1 area (async-reset positive-edge FF).
    pub fn ff_area(&self) -> f64 {
        22.50
    }

    /// FF leakage (nW), sky130_fd_sc_hd__dfrtp_1 typical.
    pub fn ff_leakage(&self) -> f64 {
        2.80
    }

    /// FF internal energy (pJ per clock edge). Accounts for both
    /// clock-to-Q switching and CLK pin capacitance per toggle.
    pub fn ff_internal_energy(&self) -> f64 {
        0.0280
    }
}
