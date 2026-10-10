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

    pub(super) fn compose(self, next: Self) -> Self {
        Self {
            array: compose_axis(self.array, next.array),
            packed: compose_axis(self.packed, next.packed),
        }
    }

    fn axis(self, axis: usize) -> Option<isize> {
        [self.array, self.packed][axis]
    }

    /// Whether a position of `source` can reach a position of `destination`
    /// on `axis` (0 for the array axis, 1 for the packed one).
    pub(super) fn may_reach(
        self,
        axis: usize,
        source: [AxisBounds; 2],
        destination: AxisBounds,
    ) -> bool {
        let Some(offset) = self.axis(axis) else {
            return true;
        };
        let (start, end) = source[axis];
        // Arithmetic beyond `isize` keeps the conservative answer.
        match (start.checked_add(offset), end.checked_add(offset)) {
            (Some(low), Some(high)) => low < destination.1 && destination.0 < high,
            _ => true,
        }
    }
}

// Half-open coordinate bounds of a box, per axis.
pub(super) type AxisBounds = (isize, isize);

pub(super) fn domain_bounds(domain: &PositionDomain) -> Option<[AxisBounds; 2]> {
    let range = |start: usize, length: usize| {
        let start = isize::try_from(start).ok()?;
        Some((start, start.checked_add_unsigned(length)?))
    };
    Some([
        range(domain.array_start, domain.array_length)?,
        range(domain.packed_start, domain.packed_length)?,
    ])
}

pub(super) fn bounds_domain(bounds: [AxisBounds; 2]) -> Option<PositionDomain> {
    let [array, packed] = bounds;
    if array.0 >= array.1 || packed.0 >= packed.1 {
        return None;
    }
    Some(PositionDomain {
        array_start: usize::try_from(array.0).ok()?,
        array_length: usize::try_from(array.1 - array.0).ok()?,
        packed_start: usize::try_from(packed.0).ok()?,
        packed_length: usize::try_from(packed.1 - packed.0).ok()?,
    })
}

/// Hull of the destination positions of a source box: `None` if unbounded
/// or beyond `isize`, `Some(None)` if no position is reached.
#[allow(clippy::option_option)]
pub(super) fn image(
    dependency: BitDependency,
    source: [AxisBounds; 2],
) -> Option<Option<[AxisBounds; 2]>> {
    let mut result = [(0, 0); 2];
    for (axis, bounds) in result.iter_mut().enumerate() {
        let offset = dependency.axis(axis)?;
        let (start, end) = source[axis];
        *bounds = (start.checked_add(offset)?, end.checked_add(offset)?);
    }
    Some(Some(result))
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
