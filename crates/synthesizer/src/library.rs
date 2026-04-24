//! Cell area/delay/power figures approximated from open PDKs. See each
//! submodule for the specific source (SkyWater SKY130, ASAP7 predictive,
//! GF180MCU, IHP SG13G2) and its license. All values are extracted or
//! derived from public Liberty characterization data and used here as
//! reference ratios for area / timing / power estimation — not for signoff.
//! No Liberty source, schematic, or layout is redistributed.
//!
//! Input slew / output load / corner selection materially affect real
//! Liberty numbers; these tables should be read as self-consistent
//! calibrations of "relative cost" (not bit-accurate delays).

mod asap7;
mod gf180mcu;
mod ihp_sg13g2;
mod sky130;

use crate::ir::CellKind;
use veryl_metadata::Library;

/// Per-cell area (um²), intrinsic delay (ns), leakage power (nW), and
/// internal switching energy (pJ per output transition).
#[derive(Clone, Copy, Debug)]
pub struct CellInfo {
    pub area: f64,
    pub delay: f64,
    pub leakage: f64,
    pub internal_energy: f64,
}

/// The per-technology data a synthesis estimation needs: cell-kind
/// metrics, FF primitive parameters, and a banner line for reports.
/// One implementation per PDK, dispatched via [`library_for`].
pub trait CellLibrary {
    fn banner(&self) -> &'static str;
    fn info(&self, kind: CellKind) -> CellInfo;
    fn ff_setup(&self) -> f64;
    fn ff_area(&self) -> f64;
    fn ff_leakage(&self) -> f64;
    fn ff_internal_energy(&self) -> f64;
}

/// Static dispatcher: map a [`Library`] config enum to its concrete
/// [`CellLibrary`] implementation. Impls are zero-sized structs, so
/// returning a `&'static dyn` costs one pointer and no allocation.
pub fn library_for(lib: Library) -> &'static dyn CellLibrary {
    match lib {
        Library::Sky130 => &sky130::Sky130,
        Library::Asap7 => &asap7::Asap7,
        Library::Gf180mcu => &gf180mcu::Gf180mcu,
        Library::IhpSg13g2 => &ihp_sg13g2::IhpSg13g2,
    }
}
