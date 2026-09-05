//! Shared dependency and module-summary data model.

use super::region::{ArraySpan, PackedSpan};
use super::ssa::PathCondition;
use super::ssa::PositionDomain;
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

    pub(super) const fn identity() -> Self {
        Self {
            array: Some(0),
            packed: Some(0),
        }
    }

    pub(super) fn exact_offset(self) -> Option<(isize, isize)> {
        self.array.zip(self.packed)
    }

    pub(super) fn has_position(self) -> bool {
        self.array.is_some() || self.packed.is_some()
    }

    pub(super) fn compose(self, next: Self) -> Self {
        Self {
            array: compose_axis(self.array, next.array),
            packed: compose_axis(self.packed, next.packed),
        }
    }
}

fn compose_axis(left: Option<isize>, right: Option<isize>) -> Option<isize> {
    match (left, right) {
        (Some(left), Some(right)) => Some(
            left.checked_add(right)
                .expect("composed dependency offset must fit in isize"),
        ),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SummaryNodeKind {
    Input,
    Output,
    Interface,
    Internal,
}

#[derive(Clone, Debug)]
pub(super) struct SummaryNode {
    pub(super) region: SummaryRegion,
    pub(super) domains: Vec<PositionDomain>,
    pub(super) kind: SummaryNodeKind,
}

/// Finite dependency graph across a module boundary. Retaining graph structure
/// is essential: taking the transitive closure of a positional cycle such as
/// `x = x << 1` would otherwise enumerate one offset per declared bit.
#[derive(Clone, Debug, Default)]
pub(super) struct ModuleCombSummary {
    pub(super) nodes: Vec<SummaryNode>,
    pub(super) edges: Vec<SummaryDependency>,
    pub(super) complete: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SummaryDependency {
    pub(super) source: usize,
    pub(super) destination: usize,
    pub(super) kind: BitDependency,
    pub(super) condition: PathCondition,
}
