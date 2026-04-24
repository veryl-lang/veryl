//! ASAP7 (ASU 7nm predictive FinFET).
//!
//! Values: asap7sc7p5t RVT, drive strength 1, tt_0p7V corner,
//! NLDM at [input_slew=10ps, output_load=2.88fF].
//! Converted from native units (ps → ns, pW → nW, 0.35 fJ-units → pJ).
//! No dedicated Mux2 cell; approximated by AOI22x1.
//!
//! Source: https://github.com/The-OpenROAD-Project/asap7 (BSD 3-Clause).
use super::{CellInfo, CellLibrary};
use crate::ir::CellKind;

pub struct Asap7;

impl CellLibrary for Asap7 {
    fn banner(&self) -> &'static str {
        "ASAP7 (ASU 7nm predictive FinFET) / asap7sc7p5t RVT / tt_0p7V \
         (approximate, light-load)"
    }

    fn info(&self, kind: CellKind) -> CellInfo {
        match kind {
            CellKind::Buf => CellInfo {
                area: 0.0,
                delay: 0.0,
                leakage: 0.0,
                internal_energy: 0.0,
            },
            // asap7sc7p5t INVx1.
            CellKind::Not => CellInfo {
                area: 0.04374,
                delay: 0.016,
                leakage: 0.0512,
                internal_energy: 0.0000174,
            },
            // NAND2x1.
            CellKind::Nand2 => CellInfo {
                area: 0.08748,
                delay: 0.018,
                leakage: 0.0912,
                internal_energy: 0.0000477,
            },
            // NOR2x1.
            CellKind::Nor2 => CellInfo {
                area: 0.08748,
                delay: 0.019,
                leakage: 0.0821,
                internal_energy: 0.0000515,
            },
            // AND2x2 (smallest AND available).
            CellKind::And2 => CellInfo {
                area: 0.08748,
                delay: 0.030,
                leakage: 0.1498,
                internal_energy: 0.0001001,
            },
            // OR2x2.
            CellKind::Or2 => CellInfo {
                area: 0.08748,
                delay: 0.032,
                leakage: 0.1502,
                internal_energy: 0.0001030,
            },
            // XOR2x1.
            CellKind::Xor2 => CellInfo {
                area: 0.17496,
                delay: 0.029,
                leakage: 0.2321,
                internal_energy: 0.0000586,
            },
            // XNOR2x1.
            CellKind::Xnor2 => CellInfo {
                area: 0.17496,
                delay: 0.026,
                leakage: 0.2323,
                internal_energy: 0.0001181,
            },
            // AND3x1.
            CellKind::And3 => CellInfo {
                area: 0.08748,
                delay: 0.028,
                leakage: 0.1008,
                internal_energy: 0.0000692,
            },
            // OR3x1.
            CellKind::Or3 => CellInfo {
                area: 0.08748,
                delay: 0.030,
                leakage: 0.0988,
                internal_energy: 0.0000690,
            },
            // NAND3x1.
            CellKind::Nand3 => CellInfo {
                area: 0.16038,
                delay: 0.018,
                leakage: 0.1170,
                internal_energy: 0.0000429,
            },
            // NOR3x1.
            CellKind::Nor3 => CellInfo {
                area: 0.16038,
                delay: 0.019,
                leakage: 0.0937,
                internal_energy: 0.0000443,
            },
            // AO21x1: (A & B) | C.
            CellKind::Ao21 => CellInfo {
                area: 0.16200,
                delay: 0.020,
                leakage: 0.1500,
                internal_energy: 0.0000575,
            },
            // AOI21x1.
            CellKind::Aoi21 => CellInfo {
                area: 0.11664,
                delay: 0.013,
                leakage: 0.1270,
                internal_energy: 0.0000507,
            },
            // OA21x1: (A | B) & C.
            CellKind::Oa21 => CellInfo {
                area: 0.16200,
                delay: 0.023,
                leakage: 0.1520,
                internal_energy: 0.0000590,
            },
            // OAI21x1.
            CellKind::Oai21 => CellInfo {
                area: 0.11664,
                delay: 0.018,
                leakage: 0.1401,
                internal_energy: 0.0000514,
            },
            // AO31x1: (A & B & C) | D.
            CellKind::Ao31 => CellInfo {
                area: 0.18000,
                delay: 0.022,
                leakage: 0.1700,
                internal_energy: 0.0000630,
            },
            // AOI31x1: !((A & B & C) | D).
            CellKind::Aoi31 => CellInfo {
                area: 0.13122,
                delay: 0.015,
                leakage: 0.1420,
                internal_energy: 0.0000555,
            },
            // AO22x1: (A & B) | (C & D).
            CellKind::Ao22 => CellInfo {
                area: 0.21870,
                delay: 0.017,
                leakage: 0.2400,
                internal_energy: 0.0000920,
            },
            // AOI22x1: !((A & B) | (C & D)).
            CellKind::Aoi22 => CellInfo {
                area: 0.14580,
                delay: 0.014,
                leakage: 0.1700,
                internal_energy: 0.0000560,
            },
            // OAI22x1: !((A | B) & (C | D)).
            CellKind::Oai22 => CellInfo {
                area: 0.14580,
                delay: 0.019,
                leakage: 0.1780,
                internal_energy: 0.0000570,
            },
            // Mux2 approximated by AOI22x1 (asap7 lacks a dedicated Mux2).
            CellKind::Mux2 => CellInfo {
                area: 0.14580,
                delay: 0.018,
                leakage: 0.1763,
                internal_energy: 0.0000554,
            },
        }
    }

    // DFFHQNx1 — positive-edge D-FF at tt_0p7V.
    fn ff_setup(&self) -> f64 {
        0.050
    }

    fn ff_area(&self) -> f64 {
        0.29160
    }

    fn ff_leakage(&self) -> f64 {
        0.23
    }

    fn ff_internal_energy(&self) -> f64 {
        0.00017
    }
}
