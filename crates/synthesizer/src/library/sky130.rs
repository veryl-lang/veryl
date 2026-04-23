//! SKY130 (SkyWater 130nm planar CMOS).
//!
//! Values: sky130_fd_sc_hd, drive strength 1, tt_025C_1v80 corner, FO4 load.
//! Hand-curated typical values rather than a single NLDM extraction point.
//!
//! Source: https://github.com/google/skywater-pdk (Apache 2.0).
use super::{CellInfo, CellLibrary};
use crate::ir::CellKind;

pub struct Sky130;

impl CellLibrary for Sky130 {
    fn banner(&self) -> &'static str {
        "SKY130 (SkyWater 130nm) / sky130_fd_sc_hd / tt_025C_1v80 \
         (approximate, FO4 no wire load)"
    }

    fn info(&self, kind: CellKind) -> CellInfo {
        match kind {
            // Buf is elided before reporting; zero so leftovers don't inflate numbers.
            CellKind::Buf => CellInfo {
                area: 0.0,
                delay: 0.0,
                leakage: 0.0,
                internal_energy: 0.0,
            },
            // sky130_fd_sc_hd__inv_1.
            CellKind::Not => CellInfo {
                area: 2.50,
                delay: 0.02,
                leakage: 0.06,
                internal_energy: 0.0018,
            },
            // sky130_fd_sc_hd__nand2_1.
            CellKind::Nand2 => CellInfo {
                area: 3.75,
                delay: 0.05,
                leakage: 0.11,
                internal_energy: 0.0038,
            },
            // sky130_fd_sc_hd__nor2_1.
            CellKind::Nor2 => CellInfo {
                area: 3.75,
                delay: 0.06,
                leakage: 0.10,
                internal_energy: 0.0045,
            },
            // sky130_fd_sc_hd__and2_1.
            CellKind::And2 => CellInfo {
                area: 5.00,
                delay: 0.08,
                leakage: 0.13,
                internal_energy: 0.0048,
            },
            // sky130_fd_sc_hd__or2_1.
            CellKind::Or2 => CellInfo {
                area: 5.00,
                delay: 0.09,
                leakage: 0.12,
                internal_energy: 0.0055,
            },
            // sky130_fd_sc_hd__xor2_1.
            CellKind::Xor2 => CellInfo {
                area: 8.75,
                delay: 0.12,
                leakage: 0.44,
                internal_energy: 0.0120,
            },
            // sky130_fd_sc_hd__xnor2_1.
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
            // sky130_fd_sc_hd__nand3_1.
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
            // sky130_fd_sc_hd__a21o_1: (A & B) | C.
            CellKind::Ao21 => CellInfo {
                area: 7.50,
                delay: 0.08,
                leakage: 0.20,
                internal_energy: 0.0085,
            },
            // sky130_fd_sc_hd__a21oi_1.
            CellKind::Aoi21 => CellInfo {
                area: 5.00,
                delay: 0.04,
                leakage: 0.15,
                internal_energy: 0.0070,
            },
            // sky130_fd_sc_hd__o21a_1: (A | B) & C.
            CellKind::Oa21 => CellInfo {
                area: 7.50,
                delay: 0.10,
                leakage: 0.19,
                internal_energy: 0.0088,
            },
            // sky130_fd_sc_hd__o21ai_0.
            CellKind::Oai21 => CellInfo {
                area: 5.00,
                delay: 0.04,
                leakage: 0.14,
                internal_energy: 0.0072,
            },
            // sky130_fd_sc_hd__a31o_1: (A & B & C) | D.
            CellKind::Ao31 => CellInfo {
                area: 8.75,
                delay: 0.10,
                leakage: 0.24,
                internal_energy: 0.0095,
            },
            // sky130_fd_sc_hd__a31oi_1: !((A & B & C) | D).
            CellKind::Aoi31 => CellInfo {
                area: 6.25,
                delay: 0.06,
                leakage: 0.20,
                internal_energy: 0.0082,
            },
            // sky130_fd_sc_hd__a22o_1: (A & B) | (C & D).
            CellKind::Ao22 => CellInfo {
                area: 11.25,
                delay: 0.10,
                leakage: 0.28,
                internal_energy: 0.0090,
            },
            // sky130_fd_sc_hd__a22oi_1: !((A & B) | (C & D)).
            CellKind::Aoi22 => CellInfo {
                area: 7.50,
                delay: 0.08,
                leakage: 0.24,
                internal_energy: 0.0082,
            },
            // sky130_fd_sc_hd__o22ai_1: !((A | B) & (C | D)).
            CellKind::Oai22 => CellInfo {
                area: 7.50,
                delay: 0.08,
                leakage: 0.22,
                internal_energy: 0.0085,
            },
            // sky130_fd_sc_hd__mux2_1.
            CellKind::Mux2 => CellInfo {
                area: 10.00,
                delay: 0.15,
                leakage: 0.35,
                internal_energy: 0.0130,
            },
        }
    }

    // sky130_fd_sc_hd__dfrtp_1 — async-reset positive-edge D-FF at tt_025C_1v80.
    fn ff_setup(&self) -> f64 {
        0.15
    }

    fn ff_area(&self) -> f64 {
        22.50
    }

    fn ff_leakage(&self) -> f64 {
        2.80
    }

    fn ff_internal_energy(&self) -> f64 {
        0.0280
    }
}
