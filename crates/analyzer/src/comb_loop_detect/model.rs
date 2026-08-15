//! Shared dependency and module-summary data model.

use super::region::{ArraySpan, PackedSpan};
use super::ssa::PathCondition;
use crate::HashMap;
use crate::ir::VarId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct SummaryRegion {
    pub(super) id: VarId,
    pub(super) array: ArraySpan,
    pub(super) packed: PackedSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct BitDependency {
    /// `None` means that every source coordinate on this axis may affect the
    /// destination region. `Some(C)` preserves `source + C = destination`.
    pub(super) array: Option<isize>,
    pub(super) packed: Option<isize>,
}

impl BitDependency {
    pub(super) const WHOLE: Self = Self {
        array: None,
        packed: None,
    };

    pub(super) fn exact_offset(self) -> Option<(isize, isize)> {
        self.array.zip(self.packed)
    }

    pub(super) fn has_position(self) -> bool {
        self.array.is_some() || self.packed.is_some()
    }

    pub(super) fn compose(self, next: Self) -> Self {
        Self {
            array: self
                .array
                .zip(next.array)
                .and_then(|(left, right)| left.checked_add(right)),
            packed: self
                .packed
                .zip(next.packed)
                .and_then(|(left, right)| left.checked_add(right)),
        }
    }

    pub(super) fn union(self, other: Self) -> Self {
        Self {
            array: (self.array == other.array).then_some(self.array).flatten(),
            packed: (self.packed == other.packed)
                .then_some(self.packed)
                .flatten(),
        }
    }
}

/// Sparse region-to-region reachability across a module boundary. Endpoints
/// include ordinary ports and interface members captured by imported modport
/// functions.
#[derive(Clone, Debug, Default)]
pub(super) struct ModuleCombSummary {
    pub(super) feedthrough: HashMap<SummaryRegion, Vec<SummaryDependency>>,
    pub(super) complete: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SummaryDependency {
    pub(super) destination: SummaryRegion,
    pub(super) kind: BitDependency,
    pub(super) condition: PathCondition,
}
