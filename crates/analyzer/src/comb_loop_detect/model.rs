//! Shared dependency and module-summary data model.

use super::ssa::PathCondition;
use super::ssa::PositionDomain;
pub(crate) use crate::procedural::region::SummaryRegion;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct BitDependency {
    /// `None` means that every source coordinate on this axis may affect the
    /// destination region. `Some(C)` preserves `source + C = destination`.
    pub(crate) array: Option<isize>,
    pub(crate) packed: Option<isize>,
}

impl BitDependency {
    pub(crate) const WHOLE: Self = Self {
        array: None,
        packed: None,
    };

    pub(crate) const fn identity() -> Self {
        Self {
            array: Some(0),
            packed: Some(0),
        }
    }

    pub(crate) fn exact_offset(self) -> Option<(isize, isize)> {
        self.array.zip(self.packed)
    }

    pub(crate) fn compose(self, next: Self) -> Self {
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
pub(crate) enum SummaryNodeKind {
    Input,
    Output,
    Interface,
    Internal,
}

#[derive(Clone, Debug)]
pub(crate) struct SummaryNode {
    pub(crate) region: SummaryRegion,
    pub(crate) domains: Vec<PositionDomain>,
    pub(crate) kind: SummaryNodeKind,
}

/// Finite dependency graph across a module boundary. Retaining graph structure
/// is essential: taking the transitive closure of a positional cycle such as
/// `x = x << 1` would otherwise enumerate one offset per declared bit.
#[derive(Clone, Debug, Default)]
pub(crate) struct ModuleCombSummary {
    pub(crate) nodes: Vec<SummaryNode>,
    pub(crate) edges: Vec<SummaryDependency>,
    pub(crate) complete: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SummaryDependency {
    pub(crate) source: usize,
    pub(crate) destination: usize,
    pub(crate) kind: BitDependency,
    pub(crate) condition: PathCondition,
}
