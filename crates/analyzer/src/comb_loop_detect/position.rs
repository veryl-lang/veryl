//! Position relations between two regions.
//!
//! Positions have an array and a packed coordinate. A relation states, for
//! each destination coordinate, which source positions can reach it:
//!
//! - `Link::Unlinked`: every source position reaches every destination
//!   coordinate on this axis;
//! - `Link::Strided`: every source position reaches every destination
//!   coordinate congruent to `residue` modulo `modulus` (at least 2) on this
//!   axis, such as a scalar written to every other element;
//! - `Link::Map`: the coordinate is an affine function of one source
//!   coordinate (the same axis, or the other one when `crossed`) on an
//!   arithmetic progression: `source = residue + modulus * t` reaches
//!   `destination = base + step * t` for every integer `t`;
//! - `Link::Never`: no source position reaches this axis, so the relation is
//!   empty.
//!
//! A translation by `k` is `{ residue: 0, modulus: 1, base: k, step: 1 }`.
//! Reversal (`step < 0`), widening (`step > 1`), narrowing (`modulus > 1`),
//! a fixed destination (`step == 0`) and a transfer between the array and
//! packed axes are all maps, and composition stays a map: substituting one
//! progression into another solves a linear congruence. Two destination axes
//! that read the same source axis are composed independently, which forgets
//! only their mutual correlation and therefore over-approximates.
//!
//! Mapping every coordinate, or a strided set of them, gives a strided set
//! again, so composition keeps the congruence too.
//!
//! A consumer that only understands translations may treat every other link
//! as `Unlinked`, because that is a superset of its positions.

/// Coordinate axis of a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Axis {
    Array,
    Packed,
}

impl Axis {
    pub(super) const BOTH: [Self; 2] = [Self::Array, Self::Packed];

    pub(super) fn other(self) -> Self {
        match self {
            Self::Array => Self::Packed,
            Self::Packed => Self::Array,
        }
    }
}

/// Affine dependency of one destination coordinate on one source coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Map {
    /// Reads the other source axis instead of the same one.
    pub(super) crossed: bool,
    /// Source coordinates `residue + modulus * t`, with `modulus >= 1` and
    /// `0 <= residue < modulus`.
    pub(super) modulus: isize,
    pub(super) residue: isize,
    /// Destination coordinate `base + step * t`.
    pub(super) base: isize,
    pub(super) step: isize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Link {
    Never,
    Map(Map),
    Strided { modulus: isize, residue: isize },
    Unlinked,
}

impl Map {
    pub(super) const fn translation(offset: isize) -> Self {
        Self {
            crossed: false,
            modulus: 1,
            residue: 0,
            base: offset,
            step: 1,
        }
    }

    /// `destination = (source * numerator + offset) / denominator` on the
    /// source positions where the division is exact.
    pub(super) fn scaled(
        crossed: bool,
        numerator: isize,
        offset: isize,
        denominator: isize,
    ) -> Link {
        if denominator <= 0 {
            return Link::Unlinked;
        }
        // Solve numerator * s + offset = 0 (mod denominator) for s.
        let Some(progression) = solve_congruence(numerator, offset.checked_neg(), denominator)
        else {
            return Link::Unlinked;
        };
        let Some((residue, modulus)) = progression else {
            return Link::Never;
        };
        // s = residue + modulus * t; destination = (numerator * s + offset) / denominator.
        let base = numerator
            .checked_mul(residue)
            .and_then(|x| x.checked_add(offset))
            .map(|x| x / denominator);
        let step = numerator.checked_mul(modulus).map(|x| x / denominator);
        match (base, step) {
            (Some(base), Some(step)) => Link::Map(Self {
                crossed,
                modulus,
                residue,
                base,
                step,
            }),
            _ => Link::Unlinked,
        }
    }

    /// The map on the source coordinates congruent to `residue` modulo
    /// `modulus` only. `None` on arithmetic overflow.
    pub(super) fn restricted(self, modulus: isize, residue: isize) -> Option<Link> {
        // residue + modulus * u = self.residue + self.modulus * t
        let Some((first, period)) =
            solve_congruence(self.modulus, residue.checked_sub(self.residue), modulus)?
        else {
            return Some(Link::Never);
        };
        // t = first + period * v
        let source = self.residue.checked_add(self.modulus.checked_mul(first)?)?;
        let source_modulus = self.modulus.checked_mul(period)?;
        let source_residue = source.rem_euclid(source_modulus);
        // Renumber v so that it starts at the smallest source coordinate.
        let shift = (source - source_residue) / source_modulus;
        let step = self.step.checked_mul(period)?;
        let base = self
            .base
            .checked_add(self.step.checked_mul(first)?)?
            .checked_sub(step.checked_mul(shift)?)?;
        Some(Link::Map(Self {
            modulus: source_modulus,
            residue: source_residue,
            base,
            step,
            ..self
        }))
    }

    pub(super) fn translation_offset(self) -> Option<isize> {
        (!self.crossed && self.modulus == 1 && self.step == 1).then_some(self.base)
    }

    /// Parameters `t` whose source coordinate lies in `[start, end)`.
    pub(super) fn source_parameters(self, start: isize, end: isize) -> Option<(isize, isize)> {
        let first = div_ceil(start.checked_sub(self.residue)?, self.modulus)?;
        let last = div_floor(end.checked_sub(1)?.checked_sub(self.residue)?, self.modulus)?;
        (first <= last).then_some((first, last))
    }

    /// Parameters `t` whose destination coordinate lies in `[start, end)`.
    pub(super) fn destination_parameters(self, start: isize, end: isize) -> Option<(isize, isize)> {
        let last_value = end.checked_sub(1)?;
        match self.step.cmp(&0) {
            std::cmp::Ordering::Equal => {
                (start <= self.base && self.base <= last_value).then_some((isize::MIN, isize::MAX))
            }
            std::cmp::Ordering::Greater => {
                let first = div_ceil(start.checked_sub(self.base)?, self.step)?;
                let last = div_floor(last_value.checked_sub(self.base)?, self.step)?;
                (first <= last).then_some((first, last))
            }
            std::cmp::Ordering::Less => {
                let first = div_ceil(last_value.checked_sub(self.base)?, self.step)?;
                let last = div_floor(start.checked_sub(self.base)?, self.step)?;
                (first <= last).then_some((first, last))
            }
        }
    }

    /// Smallest half-open range of destination coordinates for parameters in
    /// `[first, last]`.
    pub(super) fn destination_hull(self, first: isize, last: isize) -> Option<(isize, isize)> {
        let a = self.base.checked_add(self.step.checked_mul(first)?)?;
        let b = self.base.checked_add(self.step.checked_mul(last)?)?;
        Some((a.min(b), a.max(b).checked_add(1)?))
    }

    /// Smallest half-open range of source coordinates for parameters in
    /// `[first, last]`.
    pub(super) fn source_hull(self, first: isize, last: isize) -> Option<(isize, isize)> {
        let a = self.residue.checked_add(self.modulus.checked_mul(first)?)?;
        let b = self.residue.checked_add(self.modulus.checked_mul(last)?)?;
        Some((a.min(b), a.max(b).checked_add(1)?))
    }

    /// `next` applied after `self`, where `next` reads the coordinate that
    /// `self` produces. `None` means overflow; `Some(None)` means no source
    /// coordinate reaches through both maps.
    pub(super) fn then(self, next: Self) -> Option<Option<Self>> {
        // self:  s = r1 + m1 t1  ->  y = c1 + k1 t1
        // next:  y = r2 + m2 t2  ->  d = c2 + k2 t2
        let Some((t0, period)) =
            solve_congruence(self.step, next.residue.checked_sub(self.base), next.modulus)?
        else {
            return Some(None);
        };
        // t1 = t0 + period * u.
        let modulus = self.modulus.checked_mul(period)?;
        let start = self.residue.checked_add(self.modulus.checked_mul(t0)?)?;
        let residue = start.rem_euclid(modulus);
        let shift = (start - residue) / modulus;
        // y(u) = c1 + k1 t0 + k1 period u = r2 + m2 t2.
        let intermediate = self.base.checked_add(self.step.checked_mul(t0)?)?;
        let first = intermediate.checked_sub(next.residue)? / next.modulus;
        let ratio = self.step.checked_mul(period)? / next.modulus;
        // t2 = first + ratio * u, u = v - shift.
        let step = next.step.checked_mul(ratio)?;
        let base = next
            .base
            .checked_add(next.step.checked_mul(first)?)?
            .checked_sub(step.checked_mul(shift)?)?;
        Some(Some(Self {
            crossed: self.crossed ^ next.crossed,
            modulus,
            residue,
            base,
            step,
        }))
    }
}

impl Link {
    pub(super) const IDENTITY: Self = Self::Map(Map::translation(0));

    pub(super) const fn translation(offset: isize) -> Self {
        Self::Map(Map::translation(offset))
    }

    /// A translation by `offset`, or every position when it is unknown.
    pub(super) fn from_offset(offset: Option<isize>) -> Self {
        offset.map_or(Self::Unlinked, Self::translation)
    }

    /// Every coordinate congruent to `residue` modulo `modulus`.
    pub(super) fn strided(modulus: isize, residue: isize) -> Self {
        let modulus = modulus.saturating_abs();
        if modulus <= 1 {
            Self::Unlinked
        } else {
            Self::Strided {
                modulus,
                residue: residue.rem_euclid(modulus),
            }
        }
    }

    /// The offset of a translation along the same axis.
    pub(super) fn translation_offset(self) -> Option<isize> {
        match self {
            Self::Map(map) => map.translation_offset(),
            Self::Never | Self::Strided { .. } | Self::Unlinked => None,
        }
    }

    /// Whether the destination coordinate does not depend on the source.
    pub(super) fn is_unlinked(self) -> bool {
        matches!(self, Self::Unlinked | Self::Strided { .. })
    }

    /// The destination coordinates that `self` reaches from every source
    /// coordinate in `class` (`(modulus, residue)`; modulus 1 is every
    /// coordinate) when it reads that coordinate.
    fn image_of_class(self, class: (isize, isize)) -> Self {
        let (modulus, residue) = class;
        match self {
            Self::Never => Self::Never,
            Self::Unlinked => Self::Unlinked,
            Self::Strided { .. } => self,
            Self::Map(map) => {
                // map.residue + map.modulus * t = residue (mod modulus)
                let Some(solution) =
                    solve_congruence(map.modulus, residue.checked_sub(map.residue), modulus)
                else {
                    return Self::Unlinked;
                };
                let Some((first, period)) = solution else {
                    return Self::Never;
                };
                if map.step == 0 {
                    // One coordinate, which a congruence cannot single out.
                    return Self::Unlinked;
                }
                let base = map
                    .step
                    .checked_mul(first)
                    .and_then(|offset| map.base.checked_add(offset));
                match (base, map.step.checked_mul(period)) {
                    (Some(base), Some(step)) => Self::strided(step, base),
                    _ => Self::Unlinked,
                }
            }
        }
    }

    /// Whether the destination coordinate can lie in `[start, end)`.
    fn class_meets(modulus: isize, residue: isize, start: isize, end: isize) -> bool {
        let Some(offset) = residue.checked_sub(start) else {
            return true;
        };
        start
            .checked_add(offset.rem_euclid(modulus))
            .is_none_or(|first| first < end)
    }
}

/// Dependency of a destination position on a source position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Relation {
    pub(super) array: Link,
    pub(super) packed: Link,
}

impl Default for Relation {
    fn default() -> Self {
        Self::identity()
    }
}

impl Relation {
    pub(super) const WHOLE: Self = Self {
        array: Link::Unlinked,
        packed: Link::Unlinked,
    };

    pub(super) const fn identity() -> Self {
        Self {
            array: Link::IDENTITY,
            packed: Link::IDENTITY,
        }
    }

    pub(super) const fn whole() -> Self {
        Self::WHOLE
    }

    pub(super) const fn translation(array: isize, packed: isize) -> Self {
        Self {
            array: Link::translation(array),
            packed: Link::translation(packed),
        }
    }

    pub(super) fn link(self, axis: Axis) -> Link {
        match axis {
            Axis::Array => self.array,
            Axis::Packed => self.packed,
        }
    }

    pub(super) fn link_mut(&mut self, axis: Axis) -> &mut Link {
        match axis {
            Axis::Array => &mut self.array,
            Axis::Packed => &mut self.packed,
        }
    }

    pub(super) fn is_empty(self) -> bool {
        self.array == Link::Never || self.packed == Link::Never
    }

    pub(super) fn exact_offset(self) -> Option<(isize, isize)> {
        self.array
            .translation_offset()
            .zip(self.packed.translation_offset())
    }

    /// Forget every correspondence on the destination axis.
    pub(super) fn forget(mut self, axis: Axis) -> Self {
        if !self.is_empty() {
            *self.link_mut(axis) = Link::Unlinked;
        }
        self
    }

    #[cfg(test)]
    pub(super) fn union(self, other: Self) -> Self {
        let union = |left: Link, right: Link| {
            if left == right {
                left
            } else if left == Link::Never {
                right
            } else if right == Link::Never {
                left
            } else {
                Link::Unlinked
            }
        };
        Self {
            array: union(self.array, other.array),
            packed: union(self.packed, other.packed),
        }
    }

    /// Whether some source position in the half-open `source` box can reach
    /// the half-open destination range on `axis`. A strided image is
    /// approximated by its hull, so `true` may be conservative.
    pub(super) fn may_reach(
        self,
        axis: Axis,
        source: [(isize, isize); 2],
        destination: (isize, isize),
    ) -> bool {
        match self.link(axis) {
            Link::Never => false,
            Link::Unlinked => true,
            Link::Strided { modulus, residue } => {
                Link::class_meets(modulus, residue, destination.0, destination.1)
            }
            Link::Map(map) => {
                let read = if map.crossed { axis.other() } else { axis };
                let (start, end) = match read {
                    Axis::Array => source[0],
                    Axis::Packed => source[1],
                };
                let Some((first, last)) = map.source_parameters(start, end) else {
                    return false;
                };
                map.destination_hull(first, last)
                    .is_none_or(|(low, high)| low < destination.1 && destination.0 < high)
            }
        }
    }

    /// Whether `source` reaches `destination` through this relation.
    #[cfg(test)]
    pub(super) fn relates(self, source: (isize, isize), destination: (isize, isize)) -> bool {
        let reaches = |link: Link, axis: Axis, destination: isize| match link {
            Link::Never => false,
            Link::Unlinked => true,
            Link::Strided { modulus, residue } => destination.rem_euclid(modulus) == residue,
            Link::Map(map) => {
                let read = if map.crossed { axis.other() } else { axis };
                let coordinate = match read {
                    Axis::Array => source.0,
                    Axis::Packed => source.1,
                };
                let offset = coordinate - map.residue;
                offset.rem_euclid(map.modulus) == 0
                    && map.base + map.step * offset.div_euclid(map.modulus) == destination
            }
        };
        !self.is_empty()
            && reaches(self.array, Axis::Array, destination.0)
            && reaches(self.packed, Axis::Packed, destination.1)
    }

    /// `next` applied after `self`.
    pub(super) fn compose(self, next: Self) -> Self {
        if self.is_empty() || next.is_empty() {
            return Self {
                array: Link::Never,
                packed: Link::Never,
            };
        }
        let mut result = Self::WHOLE;
        for axis in Axis::BOTH {
            let link = match next.link(axis) {
                Link::Never => Link::Never,
                link @ (Link::Unlinked | Link::Strided { .. }) => link,
                Link::Map(map) => {
                    let read = if map.crossed { axis.other() } else { axis };
                    match self.link(read) {
                        Link::Never => Link::Never,
                        Link::Unlinked => next.link(axis).image_of_class((1, 0)),
                        Link::Strided { modulus, residue } => {
                            next.link(axis).image_of_class((modulus, residue))
                        }
                        Link::Map(first) => match first.then(map) {
                            Some(Some(map)) => Link::Map(map),
                            Some(None) => Link::Never,
                            None => Link::Unlinked,
                        },
                    }
                }
            };
            *result.link_mut(axis) = link;
        }
        if result.is_empty() {
            result.array = Link::Never;
            result.packed = Link::Never;
        }
        result
    }
}

fn div_floor(numerator: isize, denominator: isize) -> Option<isize> {
    if denominator == 0 {
        return None;
    }
    let quotient = numerator.checked_div(denominator)?;
    if numerator % denominator != 0 && ((numerator < 0) != (denominator < 0)) {
        quotient.checked_sub(1)
    } else {
        Some(quotient)
    }
}

fn div_ceil(numerator: isize, denominator: isize) -> Option<isize> {
    if denominator == 0 {
        return None;
    }
    let quotient = numerator.checked_div(denominator)?;
    if numerator % denominator != 0 && ((numerator < 0) == (denominator < 0)) {
        quotient.checked_add(1)
    } else {
        Some(quotient)
    }
}

/// Solutions of `coefficient * x = constant (mod modulus)` as
/// `x = first + period * u`, `None` when there is no solution. The outer
/// `None` means arithmetic overflow.
#[allow(clippy::option_option)]
pub(super) fn solve_congruence(
    coefficient: isize,
    constant: Option<isize>,
    modulus: isize,
) -> Option<Option<(isize, isize)>> {
    let constant = constant?;
    if modulus <= 0 {
        return None;
    }
    let coefficient = coefficient.rem_euclid(modulus) as i128;
    let constant = constant.rem_euclid(modulus) as i128;
    let modulus = modulus as i128;
    let (gcd, inverse) = gcd_inverse(coefficient, modulus);
    if constant % gcd != 0 {
        return Some(None);
    }
    let period = modulus / gcd;
    let first = ((constant / gcd) * inverse).rem_euclid(period);
    Some(Some((first as isize, period as isize)))
}

pub(super) fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

/// `(gcd, x)` with `a * x = gcd (mod b)` for `0 <= a < b`.
fn gcd_inverse(a: i128, b: i128) -> (i128, i128) {
    let (mut old_r, mut r) = (a, b);
    let (mut old_s, mut s) = (1i128, 0i128);
    while r != 0 {
        let quotient = old_r / r;
        (old_r, r) = (r, old_r - quotient * r);
        (old_s, s) = (s, old_s - quotient * s);
    }
    (old_r, old_s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGE: std::ops::Range<isize> = -12..12;

    fn reaches(link: Link, axis: Axis, source: (isize, isize), destination: isize) -> bool {
        match link {
            Link::Never => false,
            Link::Unlinked => true,
            Link::Strided { modulus, residue } => destination.rem_euclid(modulus) == residue,
            Link::Map(map) => {
                let coordinate = match (axis, map.crossed) {
                    (Axis::Array, false) | (Axis::Packed, true) => source.0,
                    (Axis::Packed, false) | (Axis::Array, true) => source.1,
                };
                let offset = coordinate - map.residue;
                offset.rem_euclid(map.modulus) == 0
                    && map.base + map.step * offset.div_euclid(map.modulus) == destination
            }
        }
    }

    fn relates(relation: Relation, source: (isize, isize), destination: (isize, isize)) -> bool {
        !relation.is_empty()
            && reaches(relation.array, Axis::Array, source, destination.0)
            && reaches(relation.packed, Axis::Packed, source, destination.1)
    }

    fn sample_links() -> Vec<Link> {
        let mut links = vec![
            Link::Unlinked,
            Link::Never,
            Link::strided(2, 1),
            Link::strided(3, 0),
            Link::strided(4, 2),
        ];
        for crossed in [false, true] {
            for numerator in [-2, -1, 0, 1, 3] {
                for denominator in [1, 2, 3] {
                    for offset in [-1, 0, 2] {
                        links.push(Map::scaled(crossed, numerator, offset, denominator));
                    }
                }
            }
        }
        links.sort();
        links.dedup();
        links
    }

    #[test]
    fn scaled_maps_match_exact_division() {
        for crossed in [false, true] {
            for numerator in -3..=3 {
                for denominator in 1..=4 {
                    for offset in -3..=3 {
                        let link = Map::scaled(crossed, numerator, offset, denominator);
                        let relation = Relation {
                            array: link,
                            packed: Link::Unlinked,
                        };
                        for array in RANGE {
                            for packed in RANGE {
                                let source = if crossed { packed } else { array };
                                let value = numerator * source + offset;
                                for destination in RANGE {
                                    let expected = value % denominator == 0
                                        && value / denominator == destination;
                                    assert_eq!(
                                        relates(relation, (array, packed), (destination, 0)),
                                        expected,
                                        "{link:?} {array} {packed} -> {destination}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn composition_covers_every_path() {
        let links = sample_links();
        let mut relations = Vec::new();
        for &array in &links {
            for &packed in &links {
                relations.push(Relation { array, packed });
            }
        }
        // A deterministic sample keeps the reference walk small.
        let mut seed = 17u64;
        let mut pick = |len: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % len
        };
        for _ in 0..400 {
            let first = relations[pick(relations.len())];
            let second = relations[pick(relations.len())];
            let composed = first.compose(second);
            let translations = first.exact_offset().is_some() && second.exact_offset().is_some();
            for source in [(-3, 2), (0, 0), (1, -2), (4, 5), (-5, -5), (2, 3)] {
                for array in -8..8 {
                    for packed in -8..8 {
                        let destination = (array, packed);
                        let expected = RANGE.clone().any(|ma| {
                            RANGE.clone().any(|mp| {
                                relates(first, source, (ma, mp))
                                    && relates(second, (ma, mp), destination)
                            })
                        });
                        let actual = relates(composed, source, destination);
                        if expected {
                            assert!(
                                actual,
                                "{first:?} then {second:?} lost {source:?} -> {destination:?}"
                            );
                        }
                        if translations {
                            assert_eq!(actual, expected, "{first:?} then {second:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn single_axis_composition_is_exact() {
        let links = sample_links();
        for &first in &links {
            for &second in &links {
                let (Link::Map(a), Link::Map(b)) = (first, second) else {
                    continue;
                };
                if a.crossed || b.crossed {
                    continue;
                }
                let composed = match a.then(b) {
                    Some(Some(map)) => Link::Map(map),
                    Some(None) => Link::Never,
                    None => continue,
                };
                for source in -10..10 {
                    for destination in -10..10 {
                        let expected = (-400..400).any(|middle| {
                            reaches(first, Axis::Array, (source, 0), middle)
                                && reaches(second, Axis::Array, (middle, 0), destination)
                        });
                        assert_eq!(
                            reaches(composed, Axis::Array, (source, 0), destination),
                            expected,
                            "{a:?} then {b:?}: {source} -> {destination}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn strided_composition_is_exact() {
        // A strided or unlinked coordinate followed by a map on one axis.
        let links = sample_links();
        for &first in &links {
            if !first.is_unlinked() {
                continue;
            }
            for &second in &links {
                let Link::Map(map) = second else {
                    continue;
                };
                if map.crossed {
                    continue;
                }
                let first_relation = Relation {
                    array: first,
                    packed: Link::IDENTITY,
                };
                let second_relation = Relation {
                    array: second,
                    packed: Link::IDENTITY,
                };
                let composed = first_relation.compose(second_relation);
                if composed.array == Link::Unlinked && map.step == 0 {
                    // One coordinate is widened to every coordinate.
                    continue;
                }
                for destination in -10..10 {
                    let expected = (-400..400).any(|middle| {
                        reaches(first, Axis::Array, (0, 0), middle)
                            && reaches(second, Axis::Array, (middle, 0), destination)
                    });
                    assert_eq!(
                        reaches(composed.array, Axis::Array, (0, 0), destination),
                        expected,
                        "{first:?} then {second:?}: {destination}"
                    );
                }
            }
        }
    }

    #[test]
    fn congruences_have_all_solutions() {
        for coefficient in -6..=6 {
            for constant in -6..=6 {
                for modulus in 1..=7 {
                    let solutions = solve_congruence(coefficient, Some(constant), modulus).unwrap();
                    for x in -20..20 {
                        let holds = (coefficient * x - constant).rem_euclid(modulus) == 0;
                        let listed = solutions
                            .is_some_and(|(first, period)| (x - first).rem_euclid(period) == 0);
                        assert_eq!(
                            holds, listed,
                            "{coefficient} x = {constant} mod {modulus}, x={x}"
                        );
                    }
                }
            }
        }
    }
}
