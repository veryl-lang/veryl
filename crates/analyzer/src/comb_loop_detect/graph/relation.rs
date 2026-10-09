//! Symbolic relations between an anchor position and the current node.
//!
//! # Correctness
//!
//! A `RelationPiece` is a set of `(anchor, current)` position pairs. The
//! anchor lies in a box of per-axis ranges (an absent range denotes all
//! integer positions). Each current coordinate is either
//!
//! - `Unlinked(C, J)`: any position of `J` in the congruence class `C`,
//!   independently of the anchor; or
//! - `Linked(m)`: the image of one anchor coordinate under the map `m` of
//!   `position.rs`, defined on the anchor coordinates of its progression.
//!
//! A translation `L(k, I)` of the previous formulation is `Linked` with a
//! translation map and anchor range `I`; `U(I, J)` is an anchor range `I`
//! with `Unlinked(J)`. Extending a piece by an edge composes each current
//! coordinate with the edge link that reads it and then restricts the anchor
//! so that the new coordinate lies in the destination domain. Composing two
//! pieces does the same with the second piece's anchor box as the domain.
//! For translations this is exactly the previous algebra, so every
//! statement about exact translation relations still holds.
//!
//! For other maps, composition through one intermediate coordinate is exact.
//! It over-approximates only when an intermediate coordinate is not read by
//! any later map (its progression constraint is dropped), when two current
//! coordinates read one intermediate coordinate (their correlation is
//! dropped). Mapping an unlinked range is exact: the image of the positions
//! of a class in a range is the positions of another class in the hull of
//! that image, except that a single image position keeps no class. A
//! coordinate that the previous formulation could represent only as
//! unlinked is therefore never less precise here.
//!
//! `intersects_identity` decides exactly whether a piece contains some
//! `(x, x)`: every case reduces to linear equations over at most two integer
//! parameters with interval bounds. A successful `piecewise_covers` test
//! implies semantic set inclusion; the converse is not required for pruning.
//! Normalization removes only duplicates or pieces covered by that
//! implication, so it preserves the represented relation.
//!
//! Every coordinate lies in a declared domain, so its arithmetic fits in
//! `isize`. An operation that would leave it reports `Overflow` instead of
//! an empty or a guessed result, and the search that meets it stops as
//! incomplete.

use super::{FeasiblePosition, SearchBudget};
use crate::comb_loop_detect::model::BitDependency;
use crate::comb_loop_detect::position::{
    Link, Map, Overflow, ceil_div_wide, checked, extended_gcd, first_in_class, floor_div_wide,
    intersect_classes, narrow,
};
use crate::comb_loop_detect::ssa::PositionDomain;

type AxisRange = Option<(isize, isize)>;

/// The coordinates congruent to `.1` modulo `.0`; modulus 1 is all of them.
type Class = (isize, isize);

const EVERY: Class = (1, 0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Current {
    Linked(Map),
    Unlinked(Class, AxisRange),
}

/// An unlinked coordinate with its range tightened to the first and last
/// positions of its class there, and a single position without a class, so
/// that equal sets compare equal. `None` when no position remains.
fn unlinked(class: Class, range: AxisRange) -> Result<Option<Current>, Overflow> {
    let (modulus, residue) = class;
    if modulus <= 1 {
        return Ok(Some(Current::Unlinked(EVERY, range)));
    }
    let residue = residue.rem_euclid(modulus);
    let Some((start, end)) = range else {
        return Ok(Some(Current::Unlinked((modulus, residue), None)));
    };
    let first = checked(first_in_class(residue, modulus, start))?;
    let last_value = checked(end.checked_sub(1))?;
    let offset = checked(last_value.checked_sub(residue))?.rem_euclid(modulus);
    let last = checked(last_value.checked_sub(offset))?;
    if first > last {
        return Ok(None);
    }
    let range = Some((first, checked(last.checked_add(1))?));
    Ok(Some(if first == last {
        Current::Unlinked(EVERY, range)
    } else {
        Current::Unlinked((modulus, residue), range)
    }))
}

/// Whether some coordinate of `class` lies in `range`.
fn class_meets(class: Class, range: AxisRange) -> Result<bool, Overflow> {
    let (modulus, residue) = class;
    match range {
        None => Ok(true),
        Some((start, end)) if modulus <= 1 => Ok(start < end),
        Some((start, end)) => progression_in_range(residue, modulus, start, end),
    }
}

/// Whether every coordinate of `inner` in `inner_range` belongs to `outer`.
fn class_contains(outer: Class, inner: Class, inner_range: AxisRange) -> bool {
    let (modulus, residue) = outer;
    if modulus <= 1 {
        return true;
    }
    if inner.0 % modulus == 0 && inner.1.rem_euclid(modulus) == residue {
        return true;
    }
    // A single position.
    inner_range.is_some_and(|(start, end)| {
        end.checked_sub(start) == Some(1) && start.rem_euclid(modulus) == residue
    })
}

/// The class and hull of the image of the coordinates of `class` in
/// `range` under `map`. `None` when no coordinate maps.
fn map_class_range(
    map: Map,
    class: Class,
    range: AxisRange,
) -> Result<Option<(Class, AxisRange)>, Overflow> {
    use crate::comb_loop_detect::position::solve_congruence;
    // map.residue + map.modulus * t = class.1 (mod class.0)
    let Some((first, period)) = checked(solve_congruence(
        map.modulus,
        class.1.checked_sub(map.residue),
        class.0,
    ))?
    else {
        return Ok(None);
    };
    let (low, high) = match range {
        Some((start, end)) => {
            let Some((low, high)) = map.source_parameters(start, end)? else {
                return Ok(None);
            };
            // The parameters `first + period * u` in `[low, high]`.
            let low = checked(first_in_class(first, period, low))?;
            let offset = checked(high.checked_sub(first))?.rem_euclid(period);
            let high = checked(high.checked_sub(offset))?;
            if low > high {
                return Ok(None);
            }
            (Some(low), Some(high))
        }
        None => (None, None),
    };
    let step = checked(map.step.checked_mul(period))?;
    let base = checked(
        map.step
            .checked_mul(first)
            .and_then(|offset| map.base.checked_add(offset)),
    )?;
    let class = if step == 0 {
        EVERY
    } else {
        (checked(step.checked_abs())?, base)
    };
    let hull = match (low, high) {
        (Some(low), Some(high)) => Some(map.destination_hull(low, high)?),
        _ if map.step == 0 => Some((map.base, checked(map.base.checked_add(1))?)),
        _ => None,
    };
    Ok(Some((class, hull)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct RelationPiece {
    anchor: [AxisRange; 2],
    current: [Current; 2],
}

/// The view of one axis used by translation repetition checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AxisRelation {
    /// `current = start + offset` for every position in `start`.
    Linked { offset: isize, start: AxisRange },
    /// `start` and `current` vary independently in their respective ranges.
    Unlinked {
        start: AxisRange,
        current: AxisRange,
    },
}

/// A union of pieces.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct PositionRelationSet {
    pieces: Vec<RelationPiece>,
}

fn read_axis(axis: usize, map: Map) -> usize {
    if map.crossed { 1 - axis } else { axis }
}

impl RelationPiece {
    fn translation_view(&self, axis: usize) -> Option<AxisRelation> {
        match self.current[axis] {
            Current::Linked(map) => Some(AxisRelation::Linked {
                offset: map.translation_offset()?,
                start: self.anchor[axis],
            }),
            // The repetition checks assume every position of the range.
            Current::Unlinked(class, current) => (class.0 <= 1).then_some(AxisRelation::Unlinked {
                start: self.anchor[axis],
                current,
            }),
        }
    }

    /// Restrict the anchor coordinate read by `map` so that the mapped
    /// coordinate lies in `range`. `None` means the piece becomes empty.
    fn restrict_linked(
        mut self,
        axis: usize,
        map: Map,
        range: AxisRange,
    ) -> Result<Option<Self>, Overflow> {
        let Some((start, end)) = range else {
            return Ok(Some(self));
        };
        let read = read_axis(axis, map);
        let Some((first, last)) = map.destination_parameters(start, end)? else {
            return Ok(None);
        };
        if first != isize::MIN || last != isize::MAX {
            let hull = map.source_hull(first, last)?;
            let Some(anchor) = intersect_range(self.anchor[read], Some(hull)) else {
                return Ok(None);
            };
            self.anchor[read] = anchor;
        }
        Ok(Some(self))
    }

    /// Compose a current coordinate with a link that reads it, restricting
    /// the result to `range`.
    fn extend(
        self,
        axis: usize,
        link: Link,
        range: AxisRange,
    ) -> Result<Option<(Self, Current)>, Overflow> {
        let with = |current: Option<Current>| current.map(|current| (self, current));
        match link {
            Link::Never => Ok(None),
            Link::Unlinked => Ok(with(unlinked(EVERY, range)?)),
            Link::Strided { modulus, residue } => Ok(with(unlinked((modulus, residue), range)?)),
            Link::Map(next) => {
                let read = read_axis(axis, next);
                match self.current[read] {
                    Current::Linked(first) => {
                        let Some(map) = checked(first.then(next))? else {
                            return Ok(None);
                        };
                        let piece = self.restrict_linked(axis, map, range)?;
                        Ok(piece.map(|piece| (piece, Current::Linked(map))))
                    }
                    Current::Unlinked(class, source) => {
                        let Some((class, image)) = map_class_range(next, class, source)? else {
                            return Ok(None);
                        };
                        let Some(range) = intersect_range(image, range) else {
                            return Ok(None);
                        };
                        Ok(with(unlinked(class, range)?))
                    }
                }
            }
        }
    }

    /// Drop empty pieces and canonicalize single-point maps so that equal
    /// relations compare equal.
    fn simplified(mut self) -> Result<Option<Self>, Overflow> {
        if self.anchor.iter().any(|range| is_empty_range(*range)) {
            return Ok(None);
        }
        for axis in 0..2 {
            match self.current[axis] {
                Current::Unlinked(class, range) => {
                    if is_empty_range(range) {
                        return Ok(None);
                    }
                    let Some(current) = unlinked(class, range)? else {
                        return Ok(None);
                    };
                    self.current[axis] = current;
                }
                Current::Linked(map) => {
                    let read = read_axis(axis, map);
                    let Some((start, end)) = self.anchor[read] else {
                        continue;
                    };
                    // No anchor value of the progression lies in range.
                    let Some((first, last)) = map.source_parameters(start, end)? else {
                        return Ok(None);
                    };
                    let hull = map.source_hull(first, last)?;
                    let Some(anchor) = intersect_range(self.anchor[read], Some(hull)) else {
                        return Ok(None);
                    };
                    self.anchor[read] = anchor;
                    if first == last && map.translation_offset().is_none() {
                        // One anchor value maps to one current value.
                        // Translations keep their form for the exact
                        // translation solver.
                        let (value, _) = map.destination_hull(first, last)?;
                        self.current[axis] = Current::Linked(Map {
                            crossed: map.crossed,
                            modulus: 1,
                            residue: 0,
                            base: value,
                            step: 0,
                        });
                    }
                }
            }
        }
        // Two maps reading one anchor coordinate need a common value.
        if let (Current::Linked(left), Current::Linked(right)) = (self.current[0], self.current[1])
            && read_axis(0, left) == read_axis(1, right)
        {
            let Some((residue, modulus)) = intersect_progressions(
                (left.residue, left.modulus),
                (right.residue, right.modulus),
            )?
            else {
                return Ok(None);
            };
            let read = read_axis(0, left);
            if let Some((start, end)) = self.anchor[read]
                && !progression_in_range(residue, modulus, start, end)?
            {
                return Ok(None);
            }
        }
        Ok(Some(self))
    }

    fn intersects_identity(&self) -> Result<bool, Overflow> {
        identity_solution(self)
    }

    fn contains(&self, inner: &Self) -> bool {
        (0..2).all(|axis| range_contains(self.anchor[axis], inner.anchor[axis]))
            && (0..2).all(|axis| match (self.current[axis], inner.current[axis]) {
                (Current::Linked(outer), Current::Linked(inner)) => outer == inner,
                (Current::Unlinked(outer_class, outer), Current::Unlinked(inner_class, inner)) => {
                    range_contains(outer, inner) && class_contains(outer_class, inner_class, inner)
                }
                (Current::Unlinked(class, outer), Current::Linked(map)) => {
                    let read = read_axis(axis, map);
                    let image = map_range(map, inner.anchor[read]);
                    // A range beyond `isize` is not known to be covered.
                    let image = image.ok().flatten();
                    // Every image position `base + step * t` is in the class.
                    let in_class = class.0 <= 1
                        || (map.step % class.0 == 0 && map.base.rem_euclid(class.0) == class.1);
                    in_class && image.is_some_and(|image| range_contains(outer, image))
                }
                (Current::Linked(_), Current::Unlinked(..)) => false,
            })
    }
}

impl PositionRelationSet {
    pub(super) fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    /// `self` without pairs whose array positions are equal, where pieces
    /// can be cut there; a piece that cannot keeps them.
    pub(super) fn without_array_diagonal(&self) -> Self {
        let mut pieces = Vec::new();
        for piece in &self.pieces {
            let Some((start, end)) = piece.anchor[0] else {
                pieces.push(*piece);
                continue;
            };
            // The anchor range without `position`.
            let mut without_anchor = |position: isize| {
                for range in [(start, position), (position + 1, end)] {
                    if range.0 < range.1 {
                        let mut cut = *piece;
                        cut.anchor[0] = Some(range);
                        pieces.push(cut);
                    }
                }
            };
            match piece.current[0] {
                // The other axis of the anchor.
                Current::Linked(map) if map.crossed => pieces.push(*piece),
                Current::Linked(map) => {
                    // a = residue + modulus * t = base + step * t.
                    let slope = map.step - map.modulus;
                    let offset = map.residue - map.base;
                    if slope == 0 {
                        if offset != 0 {
                            pieces.push(*piece);
                        }
                    } else if offset % slope == 0 {
                        let t = offset / slope;
                        match map.residue.checked_add(map.modulus.saturating_mul(t)) {
                            Some(position) if start <= position && position < end => {
                                without_anchor(position)
                            }
                            _ => pieces.push(*piece),
                        }
                    } else {
                        pieces.push(*piece);
                    }
                }
                Current::Unlinked(class, Some((low, high))) => {
                    let (first, last) = (start.max(low), end.min(high));
                    if first >= last {
                        pieces.push(*piece);
                    } else if end - start == 1 {
                        for range in [(low, start), (start + 1, high)] {
                            if let Ok(Some(current)) = unlinked(class, Some(range)) {
                                let mut cut = *piece;
                                cut.current[0] = current;
                                pieces.push(cut);
                            }
                        }
                    } else if high - low == 1 {
                        without_anchor(low);
                    } else {
                        pieces.push(*piece);
                    }
                }
                Current::Unlinked(..) => pieces.push(*piece),
            }
        }
        Self::normalized(pieces)
    }

    /// Whether each anchor position relates to at most one array position,
    /// as `array_within_function` can show of a part of a function.
    pub(super) fn array_is_determined(&self) -> bool {
        self.pieces.iter().all(|piece| {
            piece.anchor[0].is_some()
                && match piece.current[0] {
                    Current::Linked(map) => !map.crossed,
                    Current::Unlinked(_, Some((start, end))) => end.checked_sub(start) == Some(1),
                    Current::Unlinked(..) => false,
                }
        })
    }

    /// The one array position every pair of `self` reaches, if there is one.
    pub(super) fn single_array_position(&self) -> Option<isize> {
        let mut found = None;
        for piece in &self.pieces {
            let position = match piece.current[0] {
                Current::Unlinked(_, Some((start, end))) if end.checked_sub(start) == Some(1) => {
                    start
                }
                Current::Linked(map) if map.step == 0 => map.base,
                Current::Linked(map) => {
                    let (start, end) = piece.anchor[0]?;
                    let (first, last) = map.source_parameters(start, end).ok()??;
                    if first != last {
                        return None;
                    }
                    map.base.checked_add(map.step.checked_mul(first)?)?
                }
                Current::Unlinked(..) => return None,
            };
            if found.is_some_and(|found| found != position) {
                return None;
            }
            found = Some(position);
        }
        found
    }

    /// Whether every array pair of `self` is one of `function`, a set of
    /// array maps of the anchor: each anchor position relates only to the
    /// position `function` maps it to. `false` when that is not shown.
    pub(super) fn array_within_function(&self, function: &Self) -> bool {
        // The one map of `function` over all of `range`.
        let covering = |range: (isize, isize)| -> Option<Map> {
            let mut maps = function.pieces.iter().filter(|piece| {
                piece.anchor[0].is_none_or(|(start, end)| start <= range.0 && range.1 <= end)
            });
            let piece = maps.next()?;
            let Current::Linked(map) = piece.current[0] else {
                return None;
            };
            maps.next().is_none().then_some(map)
        };
        self.pieces.iter().all(|piece| {
            let Some(range) = piece.anchor[0] else {
                return false;
            };
            let Some(function) = covering(range) else {
                return false;
            };
            match piece.current[0] {
                Current::Linked(map) => {
                    let Ok(parameters) = map.source_parameters(range.0, range.1) else {
                        return false;
                    };
                    let Some((first, last)) = parameters else {
                        // No anchor position relates to any.
                        return true;
                    };
                    // Affine on the anchor positions of `map`, which
                    // `function` is defined on, they agree everywhere when
                    // they agree at the first and the last.
                    let defined = first == last
                        || (map.modulus % function.modulus == 0
                            && (map.residue - function.residue).rem_euclid(function.modulus) == 0);
                    defined
                        && [first, last].into_iter().all(|t| {
                            let agrees = || {
                                let anchor =
                                    map.residue.checked_add(map.modulus.checked_mul(t)?)?;
                                let current = map.base.checked_add(map.step.checked_mul(t)?)?;
                                Some(function.apply(anchor)? == current)
                            };
                            agrees() == Some(true)
                        })
                }
                // One position, the image of every anchor position.
                Current::Unlinked(_, Some((start, end))) if end.checked_sub(start) == Some(1) => {
                    if range.1.checked_sub(range.0) == Some(1) {
                        function.apply(range.0) == Some(start)
                    } else {
                        function.modulus == 1 && function.step == 0 && function.base == start
                    }
                }
                Current::Unlinked(..) => false,
            }
        })
    }

    pub(super) fn identity(domains: &[PositionDomain]) -> Self {
        let linked = [Current::Linked(Map::translation(0)); 2];
        let pieces = if domains.is_empty() {
            vec![RelationPiece {
                anchor: [None, None],
                current: linked,
            }]
        } else {
            domains
                .iter()
                .filter_map(|domain| {
                    Some(RelationPiece {
                        anchor: [
                            finite_range(domain.array_start, domain.array_length)?,
                            finite_range(domain.packed_start, domain.packed_length)?,
                        ],
                        current: linked,
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
    ) -> Result<Self, Overflow> {
        let domains = if destination.is_empty() {
            vec![[None, None]]
        } else {
            destination
                .iter()
                .filter_map(|domain| {
                    Some([
                        finite_range(domain.array_start, domain.array_length)?,
                        finite_range(domain.packed_start, domain.packed_length)?,
                    ])
                })
                .collect()
        };
        let mut pieces = Vec::new();
        for piece in &self.pieces {
            for domain in &domains {
                let mut next = *piece;
                let mut current = piece.current;
                let mut feasible = true;
                for axis in 0..2 {
                    // Each new coordinate reads the coordinates before this edge.
                    let source = RelationPiece {
                        anchor: next.anchor,
                        current: piece.current,
                    };
                    match source.extend(axis, dependency.link(axis_of(axis)), domain[axis])? {
                        Some((extended, coordinate)) => {
                            next.anchor = extended.anchor;
                            current[axis] = coordinate;
                        }
                        None => {
                            feasible = false;
                            break;
                        }
                    }
                }
                if !feasible {
                    continue;
                }
                next.current = current;
                if let Some(next) = next.simplified()? {
                    pieces.push(next);
                }
            }
        }
        Ok(Self::normalized(pieces))
    }

    pub(super) fn then(&self, next: &Self) -> Result<Self, Overflow> {
        let mut pieces = Vec::new();
        for left in &self.pieces {
            'right: for right in &next.pieces {
                // The intermediate position lies in the right anchor box.
                let mut middle = *left;
                for axis in 0..2 {
                    match left.current[axis] {
                        Current::Linked(map) => {
                            let Some(restricted) =
                                middle.restrict_linked(axis, map, right.anchor[axis])?
                            else {
                                continue 'right;
                            };
                            middle = restricted;
                        }
                        Current::Unlinked(class, range) => {
                            let Some(range) = intersect_range(range, right.anchor[axis]) else {
                                continue 'right;
                            };
                            let Some(current) = unlinked(class, range)? else {
                                continue 'right;
                            };
                            middle.current[axis] = current;
                        }
                    }
                }
                let mut composed = middle;
                for axis in 0..2 {
                    let link = match right.current[axis] {
                        Current::Linked(map) => Link::Map(map),
                        current @ Current::Unlinked(..) => {
                            composed.current[axis] = current;
                            continue;
                        }
                    };
                    let source = RelationPiece {
                        anchor: composed.anchor,
                        current: middle.current,
                    };
                    let Some((extended, coordinate)) = source.extend(axis, link, None)? else {
                        continue 'right;
                    };
                    composed.anchor = extended.anchor;
                    composed.current[axis] = coordinate;
                }
                if let Some(composed) = composed.simplified()? {
                    pieces.push(composed);
                }
            }
        }
        Ok(Self::normalized(pieces))
    }

    /// Whether some piece contains a position related to itself. A piece
    /// that overflows leaves the answer unknown unless another one closes.
    pub(super) fn intersects_identity(&self) -> Result<bool, Overflow> {
        let mut overflow = None;
        for piece in &self.pieces {
            match piece.intersects_identity() {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(error) => overflow = Some(error),
            }
        }
        overflow.map_or(Ok(false), Err)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    pub(super) fn piecewise_covers(&self, other: &Self) -> bool {
        other
            .pieces
            .iter()
            .all(|inner| self.pieces.iter().any(|outer| outer.contains(inner)))
    }

    pub(super) fn exact_translation(&self) -> Option<(BitDependency, Vec<FeasiblePosition>)> {
        let mut offset = None;
        let mut feasible = Vec::new();
        for piece in &self.pieces {
            let (
                Some(AxisRelation::Linked {
                    offset: array,
                    start: array_start,
                }),
                Some(AxisRelation::Linked {
                    offset: packed,
                    start: packed_start,
                }),
            ) = (piece.translation_view(0), piece.translation_view(1))
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
        Some((BitDependency::translation(array, packed), feasible))
    }

    /// Checks `self ; translation^n` for some positive `n` without walking
    /// once per position. This accelerates WHOLE paths followed by a regular
    /// shift back into their starting range. Pieces with other maps are left
    /// to the general search.
    pub(super) fn closes_after_repeating_translation(
        &self,
        offset: (isize, isize),
        guards: &[FeasiblePosition],
        budget: &mut SearchBudget,
    ) -> bool {
        let closes = self.closes_after_some_repetition(offset, guards, budget);
        budget.checked(closes).unwrap_or(false)
    }

    fn closes_after_some_repetition(
        &self,
        offset: (isize, isize),
        guards: &[FeasiblePosition],
        budget: &mut SearchBudget,
    ) -> Result<bool, Overflow> {
        for piece in &self.pieces {
            let (Some(array), Some(packed)) =
                (piece.translation_view(0), piece.translation_view(1))
            else {
                continue;
            };
            for &guard in guards {
                if !budget.spend(1) {
                    return Ok(false);
                }
                let mut exact_count = None;
                if !linked_repetition_count(array, offset.0, &mut exact_count)?
                    || !linked_repetition_count(packed, offset.1, &mut exact_count)?
                {
                    continue;
                }
                if let Some(count) = exact_count {
                    if self.closes_after_translation_count(offset, guard, count, budget)? {
                        return Ok(true);
                    }
                    continue;
                }

                let mut bounds = (1, isize::MAX);
                if !unlinked_repetition_bounds(array, offset.0, guard.array, &mut bounds)?
                    || !unlinked_repetition_bounds(packed, offset.1, guard.packed, &mut bounds)?
                    || bounds.0 > bounds.1
                {
                    continue;
                }
                if self.closes_after_translation_count(offset, guard, bounds.0, budget)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn closes_after_translation_count(
        &self,
        offset: (isize, isize),
        guard: FeasiblePosition,
        count: isize,
        budget: &mut SearchBudget,
    ) -> Result<bool, Overflow> {
        let repetitions = checked(count.checked_sub(1))?;
        let array_shift = checked(offset.0.checked_mul(repetitions))?;
        let packed_shift = checked(offset.1.checked_mul(repetitions))?;
        let (Some(array_start), Some(packed_start)) = (
            repeat_range(guard.array, array_shift)?,
            repeat_range(guard.packed, packed_shift)?,
        ) else {
            return Ok(false);
        };
        let array_offset = checked(offset.0.checked_mul(count))?;
        let packed_offset = checked(offset.1.checked_mul(count))?;
        let translation = Self::normalized(vec![RelationPiece {
            anchor: [array_start, packed_start],
            current: [
                Current::Linked(Map::translation(array_offset)),
                Current::Linked(Map::translation(packed_offset)),
            ],
        }]);
        if !budget.spend_product(self.piece_count(), translation.piece_count()) {
            return Ok(false);
        }
        self.then(&translation)?.intersects_identity()
    }

    fn normalized(mut pieces: Vec<RelationPiece>) -> Self {
        pieces.sort_unstable();
        pieces.dedup();
        let mut retained = Vec::new();
        for (index, piece) in pieces.iter().copied().enumerate() {
            // Distinct pieces cover each other only if they are equal, which
            // `dedup` already removed.
            if pieces
                .iter()
                .enumerate()
                .any(|(outer, candidate)| outer != index && candidate.contains(&piece))
            {
                continue;
            }
            retained.push(piece);
        }
        Self { pieces: retained }
    }
}

fn axis_of(axis: usize) -> crate::comb_loop_detect::position::Axis {
    if axis == 0 {
        crate::comb_loop_detect::position::Axis::Array
    } else {
        crate::comb_loop_detect::position::Axis::Packed
    }
}

/// Hull of the image of a range under a map, `None` when no position maps.
fn map_range(map: Map, range: AxisRange) -> Result<Option<AxisRange>, Overflow> {
    let Some((start, end)) = range else {
        return Ok(Some(if map.step == 0 {
            Some((map.base, checked(map.base.checked_add(1))?))
        } else {
            None
        }));
    };
    let Some((first, last)) = map.source_parameters(start, end)? else {
        return Ok(None);
    };
    Ok(Some(Some(map.destination_hull(first, last)?)))
}

/// `x = residue + modulus * k` with the smallest non-negative residue.
fn intersect_progressions(
    left: (isize, isize),
    right: (isize, isize),
) -> Result<Option<(isize, isize)>, Overflow> {
    checked(intersect_classes(left, right))
}

fn progression_in_range(
    residue: isize,
    modulus: isize,
    start: isize,
    end: isize,
) -> Result<bool, Overflow> {
    Ok(checked(first_in_class(residue, modulus, start))? < end)
}

/// Integer parameters `u` with `start <= value + slope * u < end`.
fn parameter_interval(value: isize, slope: isize, range: AxisRange) -> Option<(i128, i128)> {
    let Some((start, end)) = range else {
        return Some((i128::MIN, i128::MAX));
    };
    let (value, slope, start, end) = (value as i128, slope as i128, start as i128, end as i128 - 1);
    if slope == 0 {
        return (start <= value && value <= end).then_some((i128::MIN, i128::MAX));
    }
    let (first, last) = if slope > 0 {
        (
            ceil_div_wide(start - value, slope),
            floor_div_wide(end - value, slope),
        )
    } else {
        (
            ceil_div_wide(end - value, slope),
            floor_div_wide(start - value, slope),
        )
    };
    (first <= last).then_some((first, last))
}

/// Whether some integer `u` satisfies every `(value, slope, range)`.
fn parameter_exists(constraints: &[(isize, isize, AxisRange)]) -> bool {
    let mut low = i128::MIN;
    let mut high = i128::MAX;
    for &(value, slope, range) in constraints {
        let Some((first, last)) = parameter_interval(value, slope, range) else {
            return false;
        };
        low = low.max(first);
        high = high.min(last);
    }
    low <= high
}

/// Fixed points of a non-crossed map on one axis: `None` when there are
/// none, `Some((value, 0))` for one value, `Some((residue, modulus))` for a
/// progression.
fn self_fixed_points(map: Map) -> Result<Option<(isize, isize)>, Overflow> {
    // residue + modulus * t = base + step * t
    let slope = checked(map.modulus.checked_sub(map.step))?;
    let constant = checked(map.base.checked_sub(map.residue))?;
    if slope == 0 {
        return Ok((constant == 0).then_some((map.residue, map.modulus)));
    }
    if checked(constant.checked_rem(slope))? != 0 {
        return Ok(None);
    }
    let t = checked(constant.checked_div(slope))?;
    let value = checked(
        map.modulus
            .checked_mul(t)
            .and_then(|offset| map.residue.checked_add(offset)),
    )?;
    Ok(Some((value, 0)))
}

/// Whether a set of the form returned by `self_fixed_points` meets a range.
fn fixed_points_in_range(points: (isize, isize), range: AxisRange) -> Result<bool, Overflow> {
    let (value, modulus) = points;
    match range {
        None => Ok(true),
        Some((start, end)) if modulus == 0 => Ok(start <= value && value < end),
        Some((start, end)) => progression_in_range(value, modulus, start, end),
    }
}

/// Solutions `t = first + period * u` of `coefficient * t = constant`
/// restricted to the integer `t` with `value = base + slope * t` in a set of
/// the form returned by `self_fixed_points`.
fn parameters_hitting(
    base: isize,
    slope: isize,
    points: (isize, isize),
) -> Result<Option<(isize, isize)>, Overflow> {
    use crate::comb_loop_detect::position::solve_congruence;
    let (value, modulus) = points;
    if modulus == 0 {
        // base + slope * t = value
        if slope == 0 {
            return Ok((base == value).then_some((0, 1)));
        }
        let difference = checked(value.checked_sub(base))?;
        if checked(difference.checked_rem(slope))? != 0 {
            return Ok(None);
        }
        return Ok(Some((checked(difference.checked_div(slope))?, 0)));
    }
    // base + slope * t = value (mod modulus)
    checked(solve_congruence(slope, value.checked_sub(base), modulus))
}

fn identity_solution(piece: &RelationPiece) -> Result<bool, Overflow> {
    let anchor = piece.anchor;
    match (piece.current[0], piece.current[1]) {
        (Current::Unlinked(array_class, array), Current::Unlinked(packed_class, packed)) => {
            let (Some(array), Some(packed)) = (
                intersect_range(array, anchor[0]),
                intersect_range(packed, anchor[1]),
            ) else {
                return Ok(false);
            };
            Ok(class_meets(array_class, array)? && class_meets(packed_class, packed)?)
        }
        (Current::Linked(map), Current::Unlinked(class, range))
        | (Current::Unlinked(class, range), Current::Linked(map)) => {
            let linked = if matches!(piece.current[0], Current::Linked(_)) {
                0
            } else {
                1
            };
            let free = 1 - linked;
            let Some(free_range) = intersect_range(range, anchor[free]) else {
                return Ok(false);
            };
            if !map.crossed {
                let Some(points) = self_fixed_points(map)? else {
                    return Ok(false);
                };
                Ok(class_meets(class, free_range)?
                    && fixed_points_in_range(points, anchor[linked])?)
            } else {
                // anchor[linked] = base + step t, anchor[free] = residue + modulus t,
                // with anchor[free] in the class: t = first + period * u.
                use crate::comb_loop_detect::position::solve_congruence;
                let Some((first, period)) = checked(solve_congruence(
                    map.modulus,
                    class.1.checked_sub(map.residue),
                    class.0,
                ))?
                else {
                    return Ok(false);
                };
                // `base + slope * t` as a value and slope in `u`.
                let along = |base: isize, slope: isize| -> Result<(isize, isize), Overflow> {
                    let value = slope
                        .checked_mul(first)
                        .and_then(|offset| base.checked_add(offset));
                    Ok((checked(value)?, checked(slope.checked_mul(period))?))
                };
                let (linked_value, linked_slope) = along(map.base, map.step)?;
                let (free_value, free_slope) = along(map.residue, map.modulus)?;
                Ok(parameter_exists(&[
                    (linked_value, linked_slope, anchor[linked]),
                    (free_value, free_slope, free_range),
                ]))
            }
        }
        (Current::Linked(array), Current::Linked(packed)) => {
            match (array.crossed, packed.crossed) {
                (false, false) => {
                    let (Some(array_points), Some(packed_points)) =
                        (self_fixed_points(array)?, self_fixed_points(packed)?)
                    else {
                        return Ok(false);
                    };
                    Ok(fixed_points_in_range(array_points, anchor[0])?
                        && fixed_points_in_range(packed_points, anchor[1])?)
                }
                (true, false) | (false, true) => {
                    // One coordinate maps onto itself; the other reads it.
                    let (own, own_axis, reader, reader_axis) = if array.crossed {
                        (packed, 1, array, 0)
                    } else {
                        (array, 0, packed, 1)
                    };
                    let Some(points) = self_fixed_points(own)? else {
                        return Ok(false);
                    };
                    // The reader's parameter t gives own coordinate residue + modulus t.
                    let Some((first, period)) =
                        parameters_hitting(reader.residue, reader.modulus, points)?
                    else {
                        return Ok(false);
                    };
                    let slope = |coefficient: isize| {
                        if period == 0 {
                            Ok(0)
                        } else {
                            checked(coefficient.checked_mul(period))
                        }
                    };
                    let value = |base: isize, slope: isize| {
                        checked(
                            slope
                                .checked_mul(first)
                                .and_then(|offset| base.checked_add(offset)),
                        )
                    };
                    let own_value = value(reader.residue, reader.modulus)?;
                    let reader_value = value(reader.base, reader.step)?;
                    Ok(parameter_exists(&[
                        (own_value, slope(reader.modulus)?, anchor[own_axis]),
                        (reader_value, slope(reader.step)?, anchor[reader_axis]),
                    ]))
                }
                (true, true) => swapped_identity(array, packed, anchor),
            }
        }
    }
}

/// Both coordinates read the other anchor coordinate.
fn swapped_identity(array: Map, packed: Map, anchor: [AxisRange; 2]) -> Result<bool, Overflow> {
    let wide = |value: isize| value as i128;
    let add = |left: i128, right: i128| checked(left.checked_add(right));
    let sub = |left: i128, right: i128| checked(left.checked_sub(right));
    let mul = |left: i128, right: i128| checked(left.checked_mul(right));
    let divides = |divisor: i128, value: i128| -> Result<bool, Overflow> {
        Ok(checked(value.checked_rem(divisor))? == 0)
    };
    // anchor[0] = array.base + array.step * t0 = packed.residue + packed.modulus * t1
    // anchor[1] = array.residue + array.modulus * t0 = packed.base + packed.step * t1
    let (a, b, c) = (
        wide(array.step),
        -wide(packed.modulus),
        wide(packed.residue) - wide(array.base),
    );
    let (d, e, f) = (
        wide(array.modulus),
        -wide(packed.step),
        wide(packed.base) - wide(array.residue),
    );
    let check = |t0: i128| -> Result<bool, Overflow> {
        let first = add(wide(array.base), mul(wide(array.step), t0)?)?;
        let second = add(wide(array.residue), mul(wide(array.modulus), t0)?)?;
        let inside = |value: i128, range: AxisRange| {
            range.is_none_or(|(start, end)| wide(start) <= value && value < wide(end))
        };
        Ok(inside(first, anchor[0]) && inside(second, anchor[1]))
    };
    let determinant = sub(mul(a, e)?, mul(b, d)?)?;
    if determinant != 0 {
        let t0 = sub(mul(c, e)?, mul(b, f)?)?;
        let t1 = sub(mul(a, f)?, mul(c, d)?)?;
        if !divides(determinant, t0)? || !divides(determinant, t1)? {
            return Ok(false);
        }
        return check(t0 / determinant);
    }
    // Dependent equations: solve the first, then require the second.
    // a t0 + b t1 = c with b != 0 because every modulus is positive.
    let (gcd, x, _) = extended_gcd(a, b);
    if !divides(gcd, c)? {
        return Ok(false);
    }
    // t0 = t0p + (b / gcd) u, t1 = t1p - (a / gcd) u
    let t0p = mul(x, c / gcd)?;
    let t1p = sub(c, mul(a, t0p)?)? / b;
    let (s0, s1) = (b / gcd, -(a / gcd));
    // Second equation: d t0 + e t1 = f must hold for some u.
    let constant = sub(sub(f, mul(d, t0p)?)?, mul(e, t1p)?)?;
    let slope = add(mul(d, s0)?, mul(e, s1)?)?;
    if slope != 0 {
        if !divides(slope, constant)? {
            return Ok(false);
        }
        return check(add(t0p, mul(s0, constant / slope)?)?);
    }
    if constant != 0 {
        return Ok(false);
    }
    // Every u: anchor[0] and anchor[1] are linear in u.
    let base0 = narrow(add(wide(array.base), mul(wide(array.step), t0p)?)?)?;
    let slope0 = narrow(mul(wide(array.step), s0)?)?;
    let base1 = narrow(add(wide(array.residue), mul(wide(array.modulus), t0p)?)?)?;
    let slope1 = narrow(mul(wide(array.modulus), s0)?)?;
    Ok(parameter_exists(&[
        (base0, slope0, anchor[0]),
        (base1, slope1, anchor[1]),
    ]))
}

fn linked_repetition_count(
    relation: AxisRelation,
    translation: isize,
    count: &mut Option<isize>,
) -> Result<bool, Overflow> {
    let AxisRelation::Linked { offset, .. } = relation else {
        return Ok(true);
    };
    if translation == 0 {
        return Ok(offset == 0);
    }
    let required = checked(offset.checked_neg())?;
    if checked(required.checked_rem(translation))? != 0 {
        return Ok(false);
    }
    let required = checked(required.checked_div(translation))?;
    if required < 1 || count.is_some_and(|count| count != required) {
        return Ok(false);
    }
    *count = Some(required);
    Ok(true)
}

fn unlinked_repetition_bounds(
    relation: AxisRelation,
    translation: isize,
    guard: AxisRange,
    bounds: &mut (isize, isize),
) -> Result<bool, Overflow> {
    let AxisRelation::Unlinked { start, current } = relation else {
        return Ok(true);
    };
    let mut lowers = Vec::new();
    let mut uppers = Vec::new();
    if let Some((start, end)) = current {
        lowers.push((0, start));
        uppers.push((0, end));
    }
    if let Some((start, end)) = guard {
        if translation >= 0 {
            lowers.push((0, start));
            uppers.push((-translation, checked(end.checked_add(translation))?));
        } else {
            let slope = checked(translation.checked_neg())?;
            lowers.push((slope, checked(start.checked_add(translation))?));
            uppers.push((0, end));
        }
    }
    if let Some((start, end)) = start {
        let slope = checked(translation.checked_neg())?;
        lowers.push((slope, start));
        uppers.push((slope, end));
    }
    for &lower in &lowers {
        for &upper in &uppers {
            if !constrain_strict_inequality(lower, upper, bounds)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Restricts positive integer `n` so `lower(n) < upper(n)`.
fn constrain_strict_inequality(
    lower: (isize, isize),
    upper: (isize, isize),
    bounds: &mut (isize, isize),
) -> Result<bool, Overflow> {
    let slope = checked(lower.0.checked_sub(upper.0))?;
    let intercept = checked(upper.1.checked_sub(lower.1))?;
    match slope.cmp(&0) {
        std::cmp::Ordering::Equal => Ok(intercept > 0),
        std::cmp::Ordering::Greater => {
            let numerator = checked(intercept.checked_sub(1))?;
            bounds.1 = bounds.1.min(numerator.div_euclid(slope));
            Ok(bounds.0 <= bounds.1)
        }
        std::cmp::Ordering::Less => {
            let divisor = checked(slope.checked_neg())?;
            let numerator = checked(intercept.checked_neg())?;
            let lower = checked(numerator.div_euclid(divisor).checked_add(1))?;
            bounds.0 = bounds.0.max(lower);
            Ok(bounds.0 <= bounds.1)
        }
    }
}

fn repeat_range(range: AxisRange, total_shift: isize) -> Result<Option<AxisRange>, Overflow> {
    let Some((start, end)) = range else {
        return Ok(Some(None));
    };
    let repeated = if total_shift >= 0 {
        (start, checked(end.checked_sub(total_shift))?)
    } else {
        (checked(start.checked_sub(total_shift))?, end)
    };
    Ok((repeated.0 < repeated.1).then_some(Some(repeated)))
}

/// Whether a bounded range holds no position.
fn is_empty_range(range: AxisRange) -> bool {
    range.is_some_and(|(start, end)| start >= end)
}

fn finite_range(start: usize, length: usize) -> Option<AxisRange> {
    let start = isize::try_from(start).expect("position domain start must fit in isize");
    let end = start
        .checked_add_unsigned(length)
        .expect("position domain end must fit in isize");
    (start < end).then_some(Some((start, end)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_anchor_map_reports_a_value_beyond_isize() {
        let map = Map {
            crossed: false,
            modulus: 1,
            residue: 0,
            base: -10,
            step: isize::MAX,
        };
        let piece = RelationPiece {
            anchor: [Some((2, 3)), None],
            current: [Current::Linked(map), Current::Unlinked(EVERY, None)],
        };
        // The only anchor value maps past `isize::MAX`. Neither a bogus
        // constant nor an empty piece stands in for it.
        assert_eq!(piece.simplified(), Err(Overflow));
    }

    #[test]
    fn overflowing_identity_is_unknown_rather_than_absent() {
        // A fixed point beyond `isize` neither closes nor rules out a cycle.
        let map = Map {
            crossed: false,
            modulus: 2,
            residue: 1,
            base: isize::MIN,
            step: 1,
        };
        let piece = RelationPiece {
            anchor: [None, None],
            current: [Current::Linked(map), Current::Unlinked(EVERY, None)],
        };
        assert_eq!(identity_solution(&piece), Err(Overflow));
    }
}
