//! AIG-based structural rewrite pipeline (opt-in via the `aig` Cargo feature).
//!
//! - [`graph`] — hash-consed And-Inverter Graph data structure.
//! - [`npn4`] — 4-variable truth-table NPN canonicalisation + library
//!   minimal-area pattern tables.
//! - [`convert`] — lower / raise between `GateModule` and AIG.
//! - [`rewrite`] — cut enumeration + NPN4 pattern replacement.
//! - [`techmap`] — map the rewritten AIG back to compound library cells.
//!
//! Flow when the feature is active (see `conv::convert_module`):
//! `aigify` → `rewrite` → `aig_to_cells_techmap` → worklist re-converge.

pub mod convert;
pub mod graph;
pub mod npn4;
pub mod rewrite;
pub mod techmap;
