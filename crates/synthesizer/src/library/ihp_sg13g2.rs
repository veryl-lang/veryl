//! IHP SG13G2 (IHP 130nm SiGe BiCMOS).
//!
//! Values: sg13g2_stdcell, drive strength 1, typ_1p20V_25C corner,
//! NLDM at [input_slew=0.174ns, output_load=0.0648pF].
//! 1.2 V operation.
//!
//! Source: https://github.com/IHP-GmbH/IHP-Open-PDK (Apache 2.0).
use super::{CellInfo, CellLibrary};
use crate::ir::CellKind;

pub struct IhpSg13g2;

impl CellLibrary for IhpSg13g2 {
    fn banner(&self) -> &'static str {
        "IHP SG13G2 (130nm SiGe BiCMOS) / sg13g2_stdcell / \
         typ_1p20V_25C (approximate, light-load)"
    }

    fn info(&self, kind: CellKind) -> CellInfo {
        match kind {
            CellKind::Buf => CellInfo {
                area: 0.0,
                delay: 0.0,
                leakage: 0.0,
                internal_energy: 0.0,
            },
            // sg13g2_inv_1.
            CellKind::Not => CellInfo {
                area: 5.443,
                delay: 0.273,
                leakage: 0.0630,
                internal_energy: 0.00173,
            },
            // sg13g2_nand2_1.
            CellKind::Nand2 => CellInfo {
                area: 7.258,
                delay: 0.333,
                leakage: 0.0812,
                internal_energy: 0.00221,
            },
            // sg13g2_nor2_1.
            CellKind::Nor2 => CellInfo {
                area: 7.258,
                delay: 0.368,
                leakage: 0.0829,
                internal_energy: 0.00310,
            },
            // sg13g2_and2_1.
            CellKind::And2 => CellInfo {
                area: 9.072,
                delay: 0.284,
                leakage: 0.1376,
                internal_energy: 0.00624,
            },
            // sg13g2_or2_1.
            CellKind::Or2 => CellInfo {
                area: 9.072,
                delay: 0.302,
                leakage: 0.1149,
                internal_energy: 0.00700,
            },
            // sg13g2_xor2_1.
            CellKind::Xor2 => CellInfo {
                area: 14.515,
                delay: 0.400,
                leakage: 0.1848,
                internal_energy: 0.00486,
            },
            // sg13g2_xnor2_1.
            CellKind::Xnor2 => CellInfo {
                area: 14.515,
                delay: 0.354,
                leakage: 0.1948,
                internal_energy: 0.00762,
            },
            // sg13g2_and3_1.
            CellKind::And3 => CellInfo {
                area: 12.701,
                delay: 0.305,
                leakage: 0.1467,
                internal_energy: 0.00676,
            },
            // sg13g2_or3_1.
            CellKind::Or3 => CellInfo {
                area: 12.701,
                delay: 0.335,
                leakage: 0.1219,
                internal_energy: 0.00874,
            },
            // sg13g2_nand3_1.
            CellKind::Nand3 => CellInfo {
                area: 9.072,
                delay: 0.396,
                leakage: 0.0872,
                internal_energy: 0.00287,
            },
            // sg13g2_nor3_1.
            CellKind::Nor3 => CellInfo {
                area: 9.072,
                delay: 0.476,
                leakage: 0.0951,
                internal_energy: 0.00490,
            },
            // sg13g2_a21oi_1.
            CellKind::Aoi21 => CellInfo {
                area: 9.072,
                delay: 0.434,
                leakage: 0.1145,
                internal_energy: 0.00372,
            },
            // sg13g2_o21ai_1.
            CellKind::Oai21 => CellInfo {
                area: 9.072,
                delay: 0.476,
                leakage: 0.1266,
                internal_energy: 0.00474,
            },
            // sg13g2_mux2_1.
            CellKind::Mux2 => CellInfo {
                area: 18.144,
                delay: 0.325,
                leakage: 0.2463,
                internal_energy: 0.00895,
            },
        }
    }

    // sg13g2_dfrbpq_1 — D-FF with async reset + Q output at 1.2V typical.
    fn ff_setup(&self) -> f64 {
        0.10
    }

    fn ff_area(&self) -> f64 {
        48.99
    }

    fn ff_leakage(&self) -> f64 {
        0.51
    }

    fn ff_internal_energy(&self) -> f64 {
        0.0256
    }
}
