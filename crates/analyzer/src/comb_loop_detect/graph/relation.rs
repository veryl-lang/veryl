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

use super::FeasiblePosition;
use crate::comb_loop_detect::model::BitDependency;
use crate::comb_loop_detect::ssa::PositionDomain;

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
    pub(super) fn identity(domains: &[PositionDomain]) -> Self {
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
        Self::normalized(pieces)
    }

    pub(super) fn then_dependency(
        &self,
        dependency: BitDependency,
        destination: &[PositionDomain],
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
        Self::normalized(pieces)
    }

    pub(super) fn then(&self, next: &Self) -> Self {
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
        Self::normalized(pieces)
    }

    pub(super) fn intersects_identity(&self) -> bool {
        self.pieces.iter().any(|piece| {
            axis_intersects_identity(piece.array) && axis_intersects_identity(piece.packed)
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// Whether zero or more repetitions of one axis-aligned translation can
    /// reach a source position accepted by `dependency` and `destination`.
    ///
    /// This projects away the anchor coordinate, so a `false` result proves
    /// that no continuation can leave the translating node through that edge.
    /// For integer intervals, all differences `destination - current` form an
    /// interval; checking whether it contains a non-negative multiple of the
    /// stride is therefore exact and independent of the interval width.
    pub(super) fn may_take_after_repeating_translation(
        &self,
        translation: (isize, isize),
        translation_domains: &[PositionDomain],
        dependency: BitDependency,
        destination_domains: &[PositionDomain],
    ) -> bool {
        let advances_array = translation.0 != 0 && translation.1 == 0;
        let advances_packed = translation.0 == 0 && translation.1 != 0;
        if !advances_array && !advances_packed {
            return true;
        }

        let translation_domains = axis_domains(translation_domains);
        let destination_domains = axis_domains(destination_domains);
        for piece in &self.pieces {
            let current_array = current_range(piece.array);
            let current_packed = current_range(piece.packed);
            for &(translation_array, translation_packed) in &translation_domains {
                for &(destination_array, destination_packed) in &destination_domains {
                    let Some(target_array) = dependency_source_range(
                        translation_array,
                        dependency.array,
                        destination_array,
                    ) else {
                        continue;
                    };
                    let Some(target_packed) = dependency_source_range(
                        translation_packed,
                        dependency.packed,
                        destination_packed,
                    ) else {
                        continue;
                    };
                    if repeated_axis_reaches(current_array, target_array, translation.0)
                        .unwrap_or(true)
                        && repeated_axis_reaches(current_packed, target_packed, translation.1)
                            .unwrap_or(true)
                    {
                        return true;
                    }
                }
            }
        }
        false
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
    ) -> bool {
        for piece in &self.pieces {
            for &guard in guards {
                let mut exact_count = None;
                if !linked_repetition_count(piece.array, offset.0, &mut exact_count)
                    || !linked_repetition_count(piece.packed, offset.1, &mut exact_count)
                {
                    continue;
                }
                if let Some(count) = exact_count {
                    if self.closes_after_translation_count(offset, guard, count) {
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
                if self.closes_after_translation_count(offset, guard, bounds.0) {
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
        let translation = Self::normalized(vec![RelationPiece {
            array: AxisRelation::Linked {
                offset: array_offset,
                start: array_start,
            },
            packed: AxisRelation::Linked {
                offset: packed_offset,
                start: packed_start,
            },
        }]);
        self.then(&translation).intersects_identity()
    }

    fn normalized(mut pieces: Vec<RelationPiece>) -> Self {
        pieces.sort_unstable();
        pieces.dedup();
        let mut retained = Vec::new();
        for (index, piece) in pieces.iter().copied().enumerate() {
            if pieces
                .iter()
                .enumerate()
                .any(|(outer, candidate)| outer != index && piece_contains(*candidate, piece))
            {
                continue;
            }
            retained.push(piece);
        }
        Self { pieces: retained }
    }
}

fn axis_domains(domains: &[PositionDomain]) -> Vec<(AxisRange, AxisRange)> {
    if domains.is_empty() {
        return vec![(None, None)];
    }
    domains
        .iter()
        .filter_map(|domain| {
            Some((
                finite_range(domain.array_start, domain.array_length)?,
                finite_range(domain.packed_start, domain.packed_length)?,
            ))
        })
        .collect()
}

fn current_range(relation: AxisRelation) -> AxisRange {
    match relation {
        AxisRelation::Linked { offset, start } => translate_range(start, offset),
        AxisRelation::Unlinked { current, .. } => current,
    }
}

fn dependency_source_range(
    translation_domain: AxisRange,
    dependency: Option<isize>,
    destination_domain: AxisRange,
) -> Option<AxisRange> {
    let accepted = if let Some(offset) = dependency {
        let Some(offset) = offset.checked_neg() else {
            return Some(None);
        };
        translate_range(destination_domain, offset)
    } else {
        None
    };
    intersect_range(translation_domain, accepted)
}

fn repeated_axis_reaches(current: AxisRange, target: AxisRange, step: isize) -> Option<bool> {
    if step == 0 {
        return Some(intersect_range(current, target).is_some());
    }
    let (Some((current_start, current_end)), Some((target_start, target_end))) = (current, target)
    else {
        return Some(true);
    };
    let current_last = current_end.checked_sub(1)?;
    let target_last = target_end.checked_sub(1)?;
    let difference_start = target_start.checked_sub(current_last)?;
    let difference_end = target_last.checked_sub(current_start)?;
    if step > 0 {
        interval_contains_nonnegative_multiple(difference_start, difference_end, step)
    } else {
        interval_contains_nonnegative_multiple(
            difference_end.checked_neg()?,
            difference_start.checked_neg()?,
            step.checked_neg()?,
        )
    }
}

fn interval_contains_nonnegative_multiple(start: isize, end: isize, step: isize) -> Option<bool> {
    debug_assert!(step > 0);
    let start = start.max(0);
    if start > end {
        return Some(false);
    }
    let quotient = start.div_euclid(step);
    let quotient = if start.rem_euclid(step) == 0 {
        quotient
    } else {
        quotient.checked_add(1)?
    };
    Some(quotient.checked_mul(step)? <= end)
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
