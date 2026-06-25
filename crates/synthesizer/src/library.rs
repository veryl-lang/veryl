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

/// Analytical SRAM macro model for inferred RAM blocks. A bit-array macro is
/// far denser and faster than the equivalent flip-flops plus an address
/// decode/mux tree, so modelling it directly keeps area/timing realistic for
/// memory-heavy designs (caches, register files). All figures are per-PDK
/// scaled and intentionally coarse — relative cost, not signoff.
#[derive(Clone, Copy, Debug)]
pub struct SramModel {
    /// Silicon area per stored bit (um²), amortising the bit cell plus its
    /// share of row/column periphery.
    pub bit_area: f64,
    /// Static leakage per stored bit (nW).
    pub bit_leakage: f64,
    /// Fixed access-time floor (ns) before the depth-dependent term.
    pub access_base: f64,
    /// Added access time per doubling of depth (ns × log2(depth)).
    pub access_per_log2_depth: f64,
    /// Dynamic energy per accessed data bit, per read (pJ).
    pub read_energy_per_bit: f64,
    /// Dynamic energy per accessed data bit, per write (pJ).
    pub write_energy_per_bit: f64,
}

impl SramModel {
    /// Access time for a `depth`-entry macro: `base + slope · log2(depth)`.
    pub fn access_delay(&self, depth: usize) -> f64 {
        let log2 = (depth.max(2) as f64).log2();
        self.access_base + self.access_per_log2_depth * log2
    }
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

    /// SRAM macro figures. The default derives them from this PDK's flip-flop
    /// metrics so every library gets a self-consistent model without separate
    /// memory characterization: a 6T bit cell is roughly a fifth of a DFF and
    /// leaks far less, and an access is a couple of FF setup times plus a
    /// shallow depth term. PDKs with real macro data can override this.
    fn sram_model(&self) -> SramModel {
        let ff_area = self.ff_area();
        let ff_leak = self.ff_leakage();
        let ff_setup = self.ff_setup().max(0.02);
        let ff_energy = self.ff_internal_energy();
        SramModel {
            bit_area: ff_area * 0.18,
            bit_leakage: ff_leak * 0.08,
            access_base: ff_setup * 2.0,
            access_per_log2_depth: ff_setup * 0.25,
            read_energy_per_bit: ff_energy * 0.04,
            write_energy_per_bit: ff_energy * 0.06,
        }
    }
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
