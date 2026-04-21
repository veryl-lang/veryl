//! GF180MCU (GlobalFoundries 180nm MCU, 3.3V/6V planar CMOS).
//!
//! Values: gf180mcu_fd_sc_mcu7t5v0, drive strength 1, tt_025C_1v80 corner,
//! NLDM at [input_slew=0.1027ns, output_load=0.01686pF].
//! 1.8 V operation — high leakage by modern standards.
//!
//! Source: https://github.com/google/gf180mcu-pdk (Apache 2.0).
use super::{CellInfo, CellLibrary};
use crate::ir::CellKind;

pub struct Gf180mcu;

impl CellLibrary for Gf180mcu {
    fn banner(&self) -> &'static str {
        "GF180MCU (GlobalFoundries 180nm MCU) / gf180mcu_fd_sc_mcu7t5v0 / \
         tt_025C_1v80 (approximate, light-load)"
    }

    fn info(&self, kind: CellKind) -> CellInfo {
        match kind {
            CellKind::Buf => CellInfo {
                area: 0.0,
                delay: 0.0,
                leakage: 0.0,
                internal_energy: 0.0,
            },
            // inv_1.
            CellKind::Not => CellInfo {
                area: 8.78,
                delay: 0.542,
                leakage: 9.899,
                internal_energy: 0.00635,
            },
            // nand2_1.
            CellKind::Nand2 => CellInfo {
                area: 10.98,
                delay: 0.670,
                leakage: 14.118,
                internal_energy: 0.00747,
            },
            // nor2_1.
            CellKind::Nor2 => CellInfo {
                area: 13.17,
                delay: 0.618,
                leakage: 13.554,
                internal_energy: 0.00686,
            },
            // and2_1.
            CellKind::And2 => CellInfo {
                area: 17.56,
                delay: 1.174,
                leakage: 17.986,
                internal_energy: 0.01683,
            },
            // or2_1.
            CellKind::Or2 => CellInfo {
                area: 17.56,
                delay: 1.439,
                leakage: 17.064,
                internal_energy: 0.01733,
            },
            // xor2_1.
            CellKind::Xor2 => CellInfo {
                area: 26.34,
                delay: 0.704,
                leakage: 25.880,
                internal_energy: 0.02758,
            },
            // xnor2_1.
            CellKind::Xnor2 => CellInfo {
                area: 28.54,
                delay: 0.750,
                leakage: 26.314,
                internal_energy: 0.00980,
            },
            // and3_1.
            CellKind::And3 => CellInfo {
                area: 21.95,
                delay: 1.496,
                leakage: 22.273,
                internal_energy: 0.01882,
            },
            // or3_1.
            CellKind::Or3 => CellInfo {
                area: 21.95,
                delay: 1.953,
                leakage: 20.426,
                internal_energy: 0.01911,
            },
            // nand3_1.
            CellKind::Nand3 => CellInfo {
                area: 15.37,
                delay: 0.796,
                leakage: 18.266,
                internal_energy: 0.00917,
            },
            // nor3_1.
            CellKind::Nor3 => CellInfo {
                area: 17.56,
                delay: 0.709,
                leakage: 17.124,
                internal_energy: 0.00785,
            },
            // aoi21_1.
            CellKind::Aoi21 => CellInfo {
                area: 17.56,
                delay: 0.687,
                leakage: 18.866,
                internal_energy: 0.00822,
            },
            // oai21_1.
            CellKind::Oai21 => CellInfo {
                area: 17.56,
                delay: 0.712,
                leakage: 19.503,
                internal_energy: 0.00894,
            },
            // mux2_1.
            CellKind::Mux2 => CellInfo {
                area: 28.54,
                delay: 1.729,
                leakage: 25.187,
                internal_energy: 0.01965,
            },
        }
    }

    // dffq_1 — D-FF with Q output at tt_025C_1v80.
    fn ff_setup(&self) -> f64 {
        0.30
    }

    fn ff_area(&self) -> f64 {
        63.66
    }

    fn ff_leakage(&self) -> f64 {
        75.6
    }

    fn ff_internal_energy(&self) -> f64 {
        0.0284
    }
}
