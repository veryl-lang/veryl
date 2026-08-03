//! Sparse, IR-independent causal analysis.
//!
//! Front ends translate syntax into dense control-flow blocks, ordered memory
//! events and half-open regions.  The analysis deliberately owns no Veryl IR
//! types, global symbol tables or source locations, so one immutable input can
//! be analyzed independently on any worker thread.

pub mod cfg;
pub mod graph;
pub mod interval;
pub mod memory;
pub mod memory_ssa;
pub mod procedure;
pub mod region;
pub mod ssa;
