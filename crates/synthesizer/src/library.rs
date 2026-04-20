use crate::ir::CellKind;

/// Illustrative values only — area is relative to NAND2 = 1.0, delay is a
/// pseudo-ns value. The library is fixed; Liberty loading is deferred.
#[derive(Clone, Copy, Debug)]
pub struct CellInfo {
    pub area: f64,
    pub delay: f64,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct BuiltinLibrary;

impl BuiltinLibrary {
    pub fn new() -> Self {
        Self
    }

    pub fn info(&self, kind: CellKind) -> CellInfo {
        match kind {
            CellKind::Buf => CellInfo {
                area: 0.0,
                delay: 0.0,
            },
            CellKind::Not => CellInfo {
                area: 0.8,
                delay: 0.03,
            },
            CellKind::Nand2 => CellInfo {
                area: 1.0,
                delay: 0.04,
            },
            CellKind::Nor2 => CellInfo {
                area: 1.0,
                delay: 0.04,
            },
            CellKind::And2 => CellInfo {
                area: 1.5,
                delay: 0.05,
            },
            CellKind::Or2 => CellInfo {
                area: 1.5,
                delay: 0.05,
            },
            CellKind::Xor2 => CellInfo {
                area: 2.5,
                delay: 0.08,
            },
            CellKind::Xnor2 => CellInfo {
                area: 2.5,
                delay: 0.08,
            },
            CellKind::Mux2 => CellInfo {
                area: 3.0,
                delay: 0.08,
            },
        }
    }

    pub fn ff_setup(&self) -> f64 {
        0.10
    }

    pub fn ff_area(&self) -> f64 {
        6.0
    }
}
