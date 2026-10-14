//! Exact symbolic relations between an anchor position and the current node.
//!
//! # Correctness
//!
//! For an axis range `I`, let `L(k, I) = {(x, x + k) | x in I}` and
//! `U(I, J) = I x J`; an absent range denotes all integer positions.
//! `Linked` and `Unlinked` represent `L` and `U` respectively. Relational
//! composition stays in these two forms:
//!
//! ```text
//! L(a, I); L(b, J) = L(a + b, I intersect (J - a))
//! L(a, I); U(J, K) = U(I intersect (J - a), K)
//! U(I, J); L(b, K) = U(I, (J intersect K) + b)
//! U(I, J); U(K, L) = U(I, L), if J intersects K
//! ```
//!
//! These are the four cases in `compose_axis`. `extend_axis` is the same
//! composition specialized to one dependency edge, with its result restricted
//! to the destination domain. Array and packed relations form a Cartesian
//! product, and composition distributes over the union of `RelationPiece`s.
//! Consequently, induction over a path proves that `PositionRelationSet`
//! contains exactly the reachable `(anchor, current)` position pairs.
//!
//! `axis_intersects_identity` is exactly the test for an `L` or `U` relation
//! to contain `(x, x)`. A successful `piecewise_covers` test implies semantic
//! set inclusion; the converse is not required for pruning. Normalization
//! removes only duplicates or pieces covered by that implication, so it
//! preserves the represented relation.
//!
//! Domain endpoints and every intermediate translation, negation, sum, and
//! repeated product are required to be representable in `isize`; composition
//! fails the construction invariant instead of weakening overflow to WHOLE.

use super::{FeasiblePosition, SearchBudget};
use crate::comb_loop_detect::model::BitDependency;
use crate::comb_loop_detect::ssa::{PathCondition, PositionDomain};
use std::collections::{BTreeMap, BTreeSet};

type AxisRange = Option<(isize, isize)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum AxisRelation {
    /// `current = start + offset` for every position in `start`.
    Linked { offset: isize, start: AxisRange },
    /// `start` and `current` vary independently in their respective ranges.
    Unlinked {
        start: AxisRange,
        current: AxisRange,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct RelationPiece {
    array: AxisRelation,
    packed: AxisRelation,
}

/// A union of rectangular products of per-axis binary relations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct PositionRelationSet {
    pieces: Vec<RelationPiece>,
}

impl PositionRelationSet {
    pub(super) fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    pub(super) fn identity(domains: &[PositionDomain], budget: &mut SearchBudget) -> Self {
        let pieces = if domains.is_empty() {
            vec![RelationPiece {
                array: AxisRelation::Linked {
                    offset: 0,
                    start: None,
                },
                packed: AxisRelation::Linked {
                    offset: 0,
                    start: None,
                },
            }]
        } else {
            domains
                .iter()
                .filter_map(|domain| {
                    Some(RelationPiece {
                        array: AxisRelation::Linked {
                            offset: 0,
                            start: finite_range(domain.array_start, domain.array_length)?,
                        },
                        packed: AxisRelation::Linked {
                            offset: 0,
                            start: finite_range(domain.packed_start, domain.packed_length)?,
                        },
                    })
                })
                .collect()
        };
        Self::normalized(pieces, budget)
    }

    pub(super) fn then_dependency(
        &self,
        dependency: BitDependency,
        destination: &[PositionDomain],
        budget: &mut SearchBudget,
    ) -> Self {
        let domains = if destination.is_empty() {
            vec![(None, None)]
        } else {
            destination
                .iter()
                .filter_map(|domain| {
                    Some((
                        finite_range(domain.array_start, domain.array_length)?,
                        finite_range(domain.packed_start, domain.packed_length)?,
                    ))
                })
                .collect()
        };
        let mut pieces = Vec::new();
        for piece in &self.pieces {
            for &(array_domain, packed_domain) in &domains {
                let Some(array) = extend_axis(piece.array, dependency.array, array_domain) else {
                    continue;
                };
                let Some(packed) = extend_axis(piece.packed, dependency.packed, packed_domain)
                else {
                    continue;
                };
                pieces.push(RelationPiece { array, packed });
            }
        }
        Self::normalized(pieces, budget)
    }

    pub(super) fn then(&self, next: &Self, budget: &mut SearchBudget) -> Self {
        let mut pieces = Vec::new();
        for left in &self.pieces {
            for right in &next.pieces {
                let Some(array) = compose_axis(left.array, right.array) else {
                    continue;
                };
                let Some(packed) = compose_axis(left.packed, right.packed) else {
                    continue;
                };
                pieces.push(RelationPiece { array, packed });
            }
        }
        Self::normalized(pieces, budget)
    }

    pub(super) fn intersects_identity(&self) -> bool {
        self.pieces.iter().any(|piece| {
            axis_intersects_identity(piece.array) && axis_intersects_identity(piece.packed)
        })
    }

    /// Per piece, bounds of its anchors and of the current positions it
    /// reaches (`None` when unbounded).
    pub(super) fn piece_bounds(&self) -> Vec<[[AxisRange; 2]; 2]> {
        self.pieces
            .iter()
            .map(|piece| {
                let anchor = [piece.array, piece.packed].map(axis_start);
                let current = [piece.array, piece.packed].map(axis_current);
                [anchor, current]
            })
            .collect()
    }

    /// Per axis, bounds of the anchors that this relation can map onto
    /// themselves (`None` when unbounded), or `None` when there are none.
    pub(super) fn identity_anchor_bounds(&self) -> Option<[AxisRange; 2]> {
        let mut hull: Option<[AxisRange; 2]> = None;
        for piece in &self.pieces {
            let Some(bounds) = piece_identity_anchor_bounds(piece) else {
                continue;
            };
            hull = Some(match hull {
                None => bounds,
                Some(hull) => [0, 1].map(|axis| match (hull[axis], bounds[axis]) {
                    (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
                    _ => None,
                }),
            });
        }
        hull
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    pub(super) fn piecewise_covers(&self, other: &Self) -> bool {
        other.pieces.iter().all(|inner| {
            self.pieces
                .iter()
                .any(|outer| piece_contains(*outer, *inner))
        })
    }

    pub(super) fn exact_translation(&self) -> Option<(BitDependency, Vec<FeasiblePosition>)> {
        let mut offset = None;
        let mut feasible = Vec::new();
        for piece in &self.pieces {
            let (
                AxisRelation::Linked {
                    offset: array,
                    start: array_start,
                },
                AxisRelation::Linked {
                    offset: packed,
                    start: packed_start,
                },
            ) = (piece.array, piece.packed)
            else {
                return None;
            };
            let current = (array, packed);
            if offset.is_some_and(|offset| offset != current) {
                return None;
            }
            offset = Some(current);
            feasible.push(FeasiblePosition {
                array: array_start,
                packed: packed_start,
            });
        }
        let (array, packed) = offset?;
        feasible.sort_unstable();
        feasible.dedup();
        Some((
            BitDependency {
                array: Some(array),
                packed: Some(packed),
            },
            feasible,
        ))
    }

    /// Checks `self ; translation^n` for some positive `n` without walking
    /// once per position. This accelerates WHOLE paths followed by a regular
    /// shift back into their starting range.
    pub(super) fn closes_after_repeating_translation(
        &self,
        offset: (isize, isize),
        guards: &[FeasiblePosition],
        budget: &mut SearchBudget,
    ) -> bool {
        for piece in &self.pieces {
            for &guard in guards {
                if !budget.spend(1) {
                    return false;
                }
                let mut exact_count = None;
                if !linked_repetition_count(piece.array, offset.0, &mut exact_count)
                    || !linked_repetition_count(piece.packed, offset.1, &mut exact_count)
                {
                    continue;
                }
                if let Some(count) = exact_count {
                    if self.closes_after_translation_count(offset, guard, count, budget) {
                        return true;
                    }
                    continue;
                }

                let mut bounds = (1, isize::MAX);
                if !unlinked_repetition_bounds(piece.array, offset.0, guard.array, &mut bounds)
                    || !unlinked_repetition_bounds(
                        piece.packed,
                        offset.1,
                        guard.packed,
                        &mut bounds,
                    )
                    || bounds.0 > bounds.1
                {
                    continue;
                }
                if self.closes_after_translation_count(offset, guard, bounds.0, budget) {
                    return true;
                }
            }
        }
        false
    }

    fn closes_after_translation_count(
        &self,
        offset: (isize, isize),
        guard: FeasiblePosition,
        count: isize,
        budget: &mut SearchBudget,
    ) -> bool {
        let Some(repetitions) = count.checked_sub(1) else {
            return false;
        };
        let (Some(array_shift), Some(packed_shift)) = (
            offset.0.checked_mul(repetitions),
            offset.1.checked_mul(repetitions),
        ) else {
            return false;
        };
        let (Some(array_start), Some(packed_start)) = (
            repeat_range(guard.array, array_shift),
            repeat_range(guard.packed, packed_shift),
        ) else {
            return false;
        };
        let (Some(array_offset), Some(packed_offset)) =
            (offset.0.checked_mul(count), offset.1.checked_mul(count))
        else {
            return false;
        };
        let translation = Self {
            pieces: vec![RelationPiece {
                array: AxisRelation::Linked {
                    offset: array_offset,
                    start: array_start,
                },
                packed: AxisRelation::Linked {
                    offset: packed_offset,
                    start: packed_start,
                },
            }],
        };
        budget.spend_pieces(self.piece_count(), translation.piece_count())
            && self.then(&translation, budget).intersects_identity()
    }

    /// Merge, deduplicate and drop covered pieces. The comparisons are
    /// charged first; when the budget cannot pay for them, the pieces are
    /// kept as they are, which is the same relation.
    fn normalized(pieces: Vec<RelationPiece>, budget: &mut SearchBudget) -> Self {
        if pieces.len() < 2 || !budget.spend(pieces.len()) {
            return Self { pieces };
        }
        let mut pieces = coalesce_anchors(coalesce_points(pieces));
        pieces.sort_unstable();
        pieces.dedup();
        // A piece can only cover another whose linked axes have the same
        // offset, so index pieces by their offsets instead of comparing all
        // pairs.
        let mut groups: crate::HashMap<PieceKey, Vec<usize>> = crate::HashMap::default();
        for (index, piece) in pieces.iter().enumerate() {
            groups.entry(piece_key(piece)).or_default().push(index);
        }
        let comparisons = pieces
            .iter()
            .flat_map(|piece| covering_keys(piece_key(piece)))
            .map(|candidate| groups.get(&candidate).map_or(0, Vec::len))
            .fold(0usize, usize::saturating_add);
        if !budget.spend(comparisons) {
            return Self { pieces };
        }
        let mut retained = Vec::new();
        for (index, piece) in pieces.iter().copied().enumerate() {
            // Distinct pieces cover each other only if they are equal, which
            // `dedup` already removed.
            if covering_keys(piece_key(&piece)).iter().any(|candidate| {
                groups.get(candidate).is_some_and(|group| {
                    group
                        .iter()
                        .any(|&outer| outer != index && piece_contains(pieces[outer], piece))
                })
            }) {
                continue;
            }
            retained.push(piece);
        }
        Self { pieces: retained }
    }

    pub(super) fn union_all(
        sets: impl IntoIterator<Item = Self>,
        budget: &mut SearchBudget,
    ) -> Self {
        Self::normalized(
            sets.into_iter().flat_map(|set| set.pieces).collect(),
            budget,
        )
    }

    /// Each piece as a relation of its own.
    pub(super) fn into_pieces(self) -> impl Iterator<Item = Self> {
        self.pieces.into_iter().map(|piece| Self {
            pieces: vec![piece],
        })
    }

    /// Split into sets that each keep one translation, so a union of walks
    /// with different displacements still reaches the exact translation
    /// solver. Other pieces stay together.
    pub(super) fn split_translations(self) -> Vec<Self> {
        let mut translations: BTreeMap<(isize, isize), Vec<RelationPiece>> = BTreeMap::new();
        let mut others = Vec::new();
        for piece in self.pieces {
            match piece_key(&piece) {
                [Some(array), Some(packed)] => {
                    translations.entry((array, packed)).or_default().push(piece)
                }
                _ => others.push(piece),
            }
        }
        let mut sets = translations
            .into_values()
            .map(|pieces| Self { pieces })
            .collect::<Vec<_>>();
        if !others.is_empty() {
            sets.push(Self { pieces: others });
        }
        sets
    }
}

fn linked_repetition_count(
    relation: AxisRelation,
    translation: isize,
    count: &mut Option<isize>,
) -> bool {
    let AxisRelation::Linked { offset, .. } = relation else {
        return true;
    };
    if translation == 0 {
        return offset == 0;
    }
    let Some(required) = offset.checked_neg() else {
        return false;
    };
    if required % translation != 0 {
        return false;
    }
    let required = required / translation;
    if required < 1 || count.is_some_and(|count| count != required) {
        return false;
    }
    *count = Some(required);
    true
}

fn unlinked_repetition_bounds(
    relation: AxisRelation,
    translation: isize,
    guard: AxisRange,
    bounds: &mut (isize, isize),
) -> bool {
    let AxisRelation::Unlinked { start, current } = relation else {
        return true;
    };
    let mut lowers = Vec::new();
    let mut uppers = Vec::new();
    if let Some((start, end)) = current {
        lowers.push((0, start));
        uppers.push((0, end));
    }
    if let Some((start, end)) = guard {
        if translation >= 0 {
            let Some(intercept) = end.checked_add(translation) else {
                return false;
            };
            lowers.push((0, start));
            uppers.push((-translation, intercept));
        } else {
            let Some(slope) = translation.checked_neg() else {
                return false;
            };
            let Some(intercept) = start.checked_add(translation) else {
                return false;
            };
            lowers.push((slope, intercept));
            uppers.push((0, end));
        }
    }
    if let Some((start, end)) = start {
        let Some(slope) = translation.checked_neg() else {
            return false;
        };
        lowers.push((slope, start));
        uppers.push((slope, end));
    }
    lowers.iter().all(|&lower| {
        uppers
            .iter()
            .all(|&upper| constrain_strict_inequality(lower, upper, bounds))
    })
}

/// Restricts positive integer `n` so `lower(n) < upper(n)`.
fn constrain_strict_inequality(
    lower: (isize, isize),
    upper: (isize, isize),
    bounds: &mut (isize, isize),
) -> bool {
    let (Some(slope), Some(intercept)) =
        (lower.0.checked_sub(upper.0), upper.1.checked_sub(lower.1))
    else {
        return false;
    };
    match slope.cmp(&0) {
        std::cmp::Ordering::Equal => intercept > 0,
        std::cmp::Ordering::Greater => {
            let Some(numerator) = intercept.checked_sub(1) else {
                return false;
            };
            bounds.1 = bounds.1.min(numerator.div_euclid(slope));
            bounds.0 <= bounds.1
        }
        std::cmp::Ordering::Less => {
            let (Some(divisor), Some(numerator)) = (slope.checked_neg(), intercept.checked_neg())
            else {
                return false;
            };
            let Some(lower) = numerator.div_euclid(divisor).checked_add(1) else {
                return false;
            };
            bounds.0 = bounds.0.max(lower);
            bounds.0 <= bounds.1
        }
    }
}

fn repeat_range(range: AxisRange, total_shift: isize) -> Option<AxisRange> {
    let Some((start, end)) = range else {
        return Some(None);
    };
    let repeated = if total_shift >= 0 {
        (start, end.checked_sub(total_shift)?)
    } else {
        (start.checked_sub(total_shift)?, end)
    };
    (repeated.0 < repeated.1).then_some(Some(repeated))
}

fn extend_axis(
    relation: AxisRelation,
    dependency: Option<isize>,
    destination: AxisRange,
) -> Option<AxisRelation> {
    match (relation, dependency) {
        (AxisRelation::Linked { offset, start }, Some(next)) => {
            let offset = offset
                .checked_add(next)
                .expect("composed position offset must fit in isize");
            let allowed = translate_range(
                destination,
                offset
                    .checked_neg()
                    .expect("reversed position offset must fit in isize"),
            );
            Some(AxisRelation::Linked {
                offset,
                start: intersect_range(start, allowed)?,
            })
        }
        (AxisRelation::Unlinked { start, current }, Some(offset)) => {
            let current = translate_range(current, offset);
            Some(AxisRelation::Unlinked {
                start,
                current: intersect_range(current, destination)?,
            })
        }
        (AxisRelation::Linked { start, .. }, None)
        | (AxisRelation::Unlinked { start, .. }, None) => Some(AxisRelation::Unlinked {
            start,
            current: destination,
        }),
    }
}

fn compose_axis(left: AxisRelation, right: AxisRelation) -> Option<AxisRelation> {
    match (left, right) {
        (
            AxisRelation::Linked {
                offset: left_offset,
                start: left_start,
            },
            AxisRelation::Linked {
                offset: right_offset,
                start: right_start,
            },
        ) => {
            let right_start = translate_range(
                right_start,
                left_offset
                    .checked_neg()
                    .expect("reversed position offset must fit in isize"),
            );
            Some(AxisRelation::Linked {
                offset: left_offset
                    .checked_add(right_offset)
                    .expect("composed position offset must fit in isize"),
                start: intersect_range(left_start, right_start)?,
            })
        }
        (
            AxisRelation::Linked {
                offset,
                start: left_start,
            },
            AxisRelation::Unlinked {
                start: right_start,
                current,
            },
        ) => {
            let right_start = translate_range(
                right_start,
                offset
                    .checked_neg()
                    .expect("reversed position offset must fit in isize"),
            );
            Some(AxisRelation::Unlinked {
                start: intersect_range(left_start, right_start)?,
                current,
            })
        }
        (
            AxisRelation::Unlinked {
                start,
                current: left_current,
            },
            AxisRelation::Linked {
                offset,
                start: right_start,
            },
        ) => {
            let middle = intersect_range(left_current, right_start)?;
            Some(AxisRelation::Unlinked {
                start,
                current: translate_range(middle, offset),
            })
        }
        (
            AxisRelation::Unlinked {
                start,
                current: left_current,
            },
            AxisRelation::Unlinked {
                start: right_start,
                current,
            },
        ) => {
            intersect_range(left_current, right_start)?;
            Some(AxisRelation::Unlinked { start, current })
        }
    }
}

fn axis_intersects_identity(relation: AxisRelation) -> bool {
    match relation {
        AxisRelation::Linked { offset, .. } => offset == 0,
        AxisRelation::Unlinked { start, current } => intersect_range(start, current).is_some(),
    }
}

fn piece_contains(outer: RelationPiece, inner: RelationPiece) -> bool {
    axis_contains_relation(outer.array, inner.array)
        && axis_contains_relation(outer.packed, inner.packed)
}

fn axis_contains_relation(outer: AxisRelation, inner: AxisRelation) -> bool {
    match (outer, inner) {
        (
            AxisRelation::Linked {
                offset: outer_offset,
                start: outer_start,
            },
            AxisRelation::Linked {
                offset: inner_offset,
                start: inner_start,
            },
        ) => outer_offset == inner_offset && range_contains(outer_start, inner_start),
        (
            AxisRelation::Unlinked {
                start: outer_start,
                current: outer_current,
            },
            AxisRelation::Unlinked {
                start: inner_start,
                current: inner_current,
            },
        ) => {
            range_contains(outer_start, inner_start) && range_contains(outer_current, inner_current)
        }
        (
            AxisRelation::Unlinked {
                start: outer_start,
                current: outer_current,
            },
            AxisRelation::Linked { offset, start },
        ) => {
            range_contains(outer_start, start)
                && range_contains(outer_current, translate_range(start, offset))
        }
        (AxisRelation::Linked { .. }, AxisRelation::Unlinked { .. }) => false,
    }
}

fn finite_range(start: usize, length: usize) -> Option<AxisRange> {
    let start = isize::try_from(start).expect("position domain start must fit in isize");
    let end = start
        .checked_add_unsigned(length)
        .expect("position domain end must fit in isize");
    (start < end).then_some(Some((start, end)))
}

fn translate_range(range: AxisRange, offset: isize) -> AxisRange {
    range.map(|(start, end)| {
        (
            start
                .checked_add(offset)
                .expect("translated range start must fit in isize"),
            end.checked_add(offset)
                .expect("translated range end must fit in isize"),
        )
    })
}

fn intersect_range(left: AxisRange, right: AxisRange) -> Option<AxisRange> {
    match (left, right) {
        (None, right) | (right, None) => Some(right),
        (Some(left), Some(right)) => {
            let intersection = (left.0.max(right.0), left.1.min(right.1));
            (intersection.0 < intersection.1).then_some(Some(intersection))
        }
    }
}

fn range_contains(outer: AxisRange, inner: AxisRange) -> bool {
    match (outer, inner) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(outer), Some(inner)) => outer.0 <= inner.0 && inner.1 <= outer.1,
    }
}

fn axis_start(relation: AxisRelation) -> AxisRange {
    match relation {
        AxisRelation::Linked { start, .. } | AxisRelation::Unlinked { start, .. } => start,
    }
}

fn axis_current(relation: AxisRelation) -> AxisRange {
    match relation {
        AxisRelation::Linked { offset, start } => translate_range(start, offset),
        AxisRelation::Unlinked { current, .. } => current,
    }
}

fn with_start(relation: AxisRelation, start: AxisRange) -> AxisRelation {
    match relation {
        AxisRelation::Linked { offset, .. } => AxisRelation::Linked { offset, start },
        AxisRelation::Unlinked { current, .. } => AxisRelation::Unlinked { start, current },
    }
}

impl RelationPiece {
    fn axis(&self, axis: usize) -> AxisRelation {
        [self.array, self.packed][axis]
    }

    fn with_axis(mut self, axis: usize, relation: AxisRelation) -> Self {
        match axis {
            0 => self.array = relation,
            _ => self.packed = relation,
        }
        self
    }

    /// The union of two pieces that differ only in overlapping or adjacent
    /// start ranges of one axis.
    fn union_contiguous(&self, other: &Self) -> Self {
        let mut union = *self;
        for axis in 0..2 {
            if let (Some(left), Some(right)) =
                (axis_start(self.axis(axis)), axis_start(other.axis(axis)))
                && left != right
            {
                union = union.with_axis(
                    axis,
                    with_start(
                        self.axis(axis),
                        Some((left.0.min(right.0), left.1.max(right.1))),
                    ),
                );
            }
        }
        union
    }
}

/// Per axis, bounds of the anchors of `piece` that it relates to themselves,
/// or `None` when it relates none.
fn piece_identity_anchor_bounds(piece: &RelationPiece) -> Option<[AxisRange; 2]> {
    let axis = |relation: AxisRelation| match relation {
        AxisRelation::Linked { offset: 0, start } => Some(start),
        AxisRelation::Linked { .. } => None,
        AxisRelation::Unlinked { start, current } => intersect_range(start, current),
    };
    Some([axis(piece.array)?, axis(piece.packed)?])
}

/// The offsets of the linked axes of a piece.
type PieceKey = [Option<isize>; 2];

fn piece_key(piece: &RelationPiece) -> PieceKey {
    [piece.array, piece.packed].map(|relation| match relation {
        AxisRelation::Linked { offset, .. } => Some(offset),
        AxisRelation::Unlinked { .. } => None,
    })
}

/// The keys of the pieces that may contain a piece with `key`: a linked axis
/// is contained in a linked axis with the same offset or in an unlinked one.
fn covering_keys(key: PieceKey) -> Vec<PieceKey> {
    let [array, packed] = key;
    let mut keys = vec![[array, packed], [None, packed], [array, None], [None, None]];
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// The current range of `axis` when it does not depend on the anchor: an
/// unlinked range, or a translation of a single anchor position.
fn point_image(piece: &RelationPiece, axis: usize) -> Option<(isize, isize)> {
    match piece.axis(axis) {
        AxisRelation::Unlinked { current, .. } => current,
        AxisRelation::Linked { offset, start } => {
            let (start, end) = start?;
            if end.checked_sub(start)? != 1 {
                return None;
            }
            Some((start.checked_add(offset)?, end.checked_add(offset)?))
        }
    }
}

/// An anchor-independent current range and the piece that has it.
type PointImage = ((isize, isize), RelationPiece);

/// Pieces that differ only in an anchor-independent current range relate the
/// same anchors to the union of those ranges. Merge contiguous ranges into one
/// unlinked range; for a single anchor position this is exactly the union of
/// its separate images, so a run of translated copies of one position costs
/// one piece. Isolated pieces keep their form for the translation solver.
fn coalesce_points(mut pieces: Vec<RelationPiece>) -> Vec<RelationPiece> {
    for axis in 0..2 {
        if pieces.len() < 2 {
            break;
        }
        let mut groups: BTreeMap<RelationPiece, Vec<PointImage>> = BTreeMap::new();
        let mut result = Vec::with_capacity(pieces.len());
        for piece in pieces {
            match point_image(&piece, axis) {
                Some(image) => {
                    let start = axis_start(piece.axis(axis));
                    let key = piece.with_axis(
                        axis,
                        AxisRelation::Unlinked {
                            start,
                            current: None,
                        },
                    );
                    groups.entry(key).or_default().push((image, piece));
                }
                None => result.push(piece),
            }
        }
        for (key, mut images) in groups {
            if images.len() == 1 {
                result.push(images[0].1);
                continue;
            }
            images.sort_unstable();
            let mut index = 0;
            while index < images.len() {
                let ((start, mut end), first) = images[index];
                let mut next = index + 1;
                while next < images.len() && images[next].0.0 <= end {
                    end = end.max(images[next].0.1);
                    next += 1;
                }
                if next == index + 1 {
                    result.push(first);
                } else {
                    let anchor = axis_start(key.axis(axis));
                    result.push(key.with_axis(
                        axis,
                        AxisRelation::Unlinked {
                            start: anchor,
                            current: Some((start, end)),
                        },
                    ));
                }
                index = next;
            }
        }
        pieces = result;
    }
    pieces
}

/// Pieces that differ only in the start range of one axis relate each anchor
/// by the same offsets and ranges, so contiguous ranges merge into one piece
/// with exactly their union. Copies of one translation written element by
/// element then cost one piece.
fn coalesce_anchors(mut pieces: Vec<RelationPiece>) -> Vec<RelationPiece> {
    for axis in 0..2 {
        if pieces.len() < 2 {
            break;
        }
        let mut groups: BTreeMap<RelationPiece, Vec<(isize, isize)>> = BTreeMap::new();
        let mut result = Vec::with_capacity(pieces.len());
        for piece in pieces {
            match axis_start(piece.axis(axis)) {
                Some(range) => {
                    let key = piece.with_axis(axis, with_start(piece.axis(axis), None));
                    groups.entry(key).or_default().push(range);
                }
                None => result.push(piece),
            }
        }
        for (key, mut ranges) in groups {
            ranges.sort_unstable();
            let mut index = 0;
            while index < ranges.len() {
                let (start, mut end) = ranges[index];
                index += 1;
                while index < ranges.len() && ranges[index].0 <= end {
                    end = end.max(ranges[index].1);
                    index += 1;
                }
                result.push(key.with_axis(axis, with_start(key.axis(axis), Some((start, end)))));
            }
        }
        pieces = result;
    }
    pieces
}

/// The relation pieces that reach one node, each under the conditions of the
/// paths that reach it. A piece under a condition means: for every valuation
/// of the condition, every pair of the piece is realized by a path whose
/// guards that valuation admits. Unions of pieces preserve this, and so does
/// the exact union of two conditions of one piece; conditions whose union is
/// not exact stay separate. Only the entries added since the node was last
/// expanded are propagated again.
#[derive(Default)]
pub(super) struct PieceStates {
    pieces: crate::HashMap<RelationPiece, Vec<PathCondition>>,
    groups: crate::HashMap<PieceKey, BTreeSet<RelationPiece>>,
    /// Per axis, the bounded start ranges of the pieces that agree on
    /// everything else and share a condition. Contiguous pieces merge, so
    /// the ranges of one entry are disjoint and not adjacent.
    runs: [crate::HashMap<(RelationPiece, PathCondition), BTreeMap<isize, isize>>; 2],
    delta: BTreeSet<(RelationPiece, PathCondition)>,
}

impl PieceStates {
    /// Add `relation` under `condition`. Returns `None` when the budget is
    /// exhausted, otherwise whether some piece needs propagation.
    pub(super) fn insert(
        &mut self,
        relation: &PositionRelationSet,
        condition: &PathCondition,
        budget: &mut SearchBudget,
    ) -> Option<bool> {
        let mut changed = false;
        'piece: for piece in &relation.pieces {
            if !budget.spend(1) {
                return None;
            }
            // A recorded piece containing this one under a covering condition
            // has every continuation of it.
            for candidate in covering_keys(piece_key(piece)) {
                for outer in self.groups.get(&candidate).into_iter().flatten() {
                    if !budget.spend(1) {
                        return None;
                    }
                    if !piece_contains(*outer, *piece) {
                        continue;
                    }
                    for recorded in &self.pieces[outer] {
                        if !budget.spend_guard_comparison(recorded, condition) {
                            return None;
                        }
                        if recorded.covers(condition) {
                            continue 'piece;
                        }
                    }
                }
            }
            // Merge the piece with an equal piece under the exact union of
            // their conditions, and with contiguous pieces under the same
            // condition, until neither applies.
            let mut piece = *piece;
            let mut condition = condition.clone();
            'merge: loop {
                let recorded = self.pieces.get(&piece).cloned().unwrap_or_default();
                for existing in recorded {
                    if !budget.spend_guard_comparison(&existing, &condition) {
                        return None;
                    }
                    if condition.covers(&existing) {
                        self.remove(&piece, &existing);
                        continue;
                    }
                    if let Some(union) = existing.disjoin_exact(&condition) {
                        self.remove(&piece, &existing);
                        condition = union;
                        continue 'merge;
                    }
                }
                let Some(neighbour) = self.contiguous(&piece, &condition) else {
                    break;
                };
                if !budget.spend(1) {
                    return None;
                }
                self.remove(&neighbour, &condition);
                piece = piece.union_contiguous(&neighbour);
            }
            self.add(piece, condition);
            changed = true;
        }
        Some(changed)
    }

    /// A recorded piece under `condition` that differs from `piece` only in
    /// an overlapping or adjacent start range.
    fn contiguous(
        &self,
        piece: &RelationPiece,
        condition: &PathCondition,
    ) -> Option<RelationPiece> {
        (0..2).find_map(|axis| {
            let (start, end) = axis_start(piece.axis(axis))?;
            let key = piece.with_axis(axis, with_start(piece.axis(axis), None));
            let runs = self.runs[axis].get(&(key, condition.clone()))?;
            let (&low, &high) = runs.range(..=end).next_back()?;
            (high >= start)
                .then(|| key.with_axis(axis, with_start(key.axis(axis), Some((low, high)))))
        })
    }

    fn add(&mut self, piece: RelationPiece, condition: PathCondition) {
        self.groups
            .entry(piece_key(&piece))
            .or_default()
            .insert(piece);
        for axis in 0..2 {
            if let Some((start, end)) = axis_start(piece.axis(axis)) {
                let key = piece.with_axis(axis, with_start(piece.axis(axis), None));
                self.runs[axis]
                    .entry((key, condition.clone()))
                    .or_default()
                    .insert(start, end);
            }
        }
        self.pieces
            .entry(piece)
            .or_default()
            .push(condition.clone());
        self.delta.insert((piece, condition));
    }

    fn remove(&mut self, piece: &RelationPiece, condition: &PathCondition) {
        let conditions = self
            .pieces
            .get_mut(piece)
            .expect("only recorded pieces are removed");
        conditions.retain(|recorded| recorded != condition);
        if conditions.is_empty() {
            self.pieces.remove(piece);
            if let Some(group) = self.groups.get_mut(&piece_key(piece)) {
                group.remove(piece);
            }
        }
        for axis in 0..2 {
            if let Some((start, _)) = axis_start(piece.axis(axis)) {
                let key = piece.with_axis(axis, with_start(piece.axis(axis), None));
                if let Some(runs) = self.runs[axis].get_mut(&(key, condition.clone())) {
                    runs.remove(&start);
                }
            }
        }
        self.delta.remove(&(*piece, condition.clone()));
    }

    /// The pieces to propagate, grouped by condition.
    pub(super) fn take_delta(
        &mut self,
        budget: &mut SearchBudget,
    ) -> Vec<(PathCondition, PositionRelationSet)> {
        let mut grouped: BTreeMap<PathCondition, Vec<RelationPiece>> = BTreeMap::new();
        for (piece, condition) in std::mem::take(&mut self.delta) {
            grouped.entry(condition).or_default().push(piece);
        }
        grouped
            .into_iter()
            .map(|(condition, pieces)| (condition, PositionRelationSet::normalized(pieces, budget)))
            .collect()
    }
}

/// The relation pieces that reach one node along recorded paths, each with
/// the conditions of those paths. Unlike `PieceStates`, neither pieces nor
/// conditions are merged, so every entry is realized by the one path that
/// recorded it, and a path search can return that path.
#[derive(Default)]
pub(super) struct PathPieces {
    conditions: crate::HashMap<RelationPiece, Vec<PathCondition>>,
    groups: crate::HashMap<PieceKey, Vec<RelationPiece>>,
}

impl PathPieces {
    /// Record a path that reaches the single piece of `relation` under
    /// `condition`, unless a recorded piece contains it under a covering
    /// condition: every continuation of that path then continues the
    /// recorded one. Returns `None` when the budget is exhausted, otherwise
    /// whether the path was recorded.
    pub(super) fn insert(
        &mut self,
        relation: &PositionRelationSet,
        condition: &PathCondition,
        budget: &mut SearchBudget,
    ) -> Option<bool> {
        let [piece] = relation.pieces.as_slice() else {
            unreachable!("paths are recorded one piece at a time");
        };
        for candidate in covering_keys(piece_key(piece)) {
            for outer in self.groups.get(&candidate).into_iter().flatten() {
                if !budget.spend(1) {
                    return None;
                }
                if !piece_contains(*outer, *piece) {
                    continue;
                }
                for recorded in &self.conditions[outer] {
                    if !budget.spend_guard_comparison(recorded, condition) {
                        return None;
                    }
                    if recorded.covers(condition) {
                        return Some(false);
                    }
                }
            }
        }
        let existing = self.conditions.entry(*piece).or_default();
        if existing.is_empty() {
            self.groups
                .entry(piece_key(piece))
                .or_default()
                .push(*piece);
        }
        existing.push(condition.clone());
        Some(true)
    }
}
