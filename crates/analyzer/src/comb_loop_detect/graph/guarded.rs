//! Guarded composition of non-zero positional cycles.
//!
//! # Correctness
//!
//! Every `GuardedCycle` is a first-return walk at one anchor. A sequence of
//! them is a closed positional walk exactly when their conditions have a
//! common branch valuation and their composed relation intersects identity.
//!
//! For exact translations, relation composition adds the two-dimensional
//! displacement while `compose_feasible_positions` computes exactly the
//! anchor positions for which the concatenation exists. If non-zero integer
//! vectors have a non-negative sum of zero, a support-minimal such sum in two
//! dimensions uses either an opposing pair or three vectors surrounding the
//! origin. Therefore `compatible_cycle_displacements_cancel` can safely reject
//! the exact-only case when it finds neither. The cyclic order of any closed
//! word also lies in one strongly connected component of the feasible
//! relation-to-relation transition graph, so components without a cancelling
//! displacement set are safely rejected. The pair and triple paths construct
//! feasible repetitions directly; the fallback worklist retains the exact
//! accumulated displacement, feasible positions, and condition. Hence it finds
//! every remaining feasible zero-displacement word.
//!
//! A closed word containing a non-translation relation can be rotated at its
//! concrete intermediate position to start with that relation. Thus
//! `guarded_relations_close` loses nothing by seeding only non-translations and
//! appending every first-return relation thereafter. Relational composition is
//! exact, and discarding a state only when a weaker condition carries a
//! superset relation is safe by monotonicity of composition. The specialized
//! repeated-translation checks only return after constructing a feasible
//! composition that intersects identity, so they are witness-preserving
//! accelerators rather than additional approximations.

use super::relation::PositionRelationSet;
use super::{FeasiblePosition, intersect_axis};
use crate::comb_loop_detect::model::BitDependency;
use crate::comb_loop_detect::ssa::PathCondition;
use crate::{HashMap, HashSet};
use daggy::petgraph::Graph;
use daggy::petgraph::algo::tarjan_scc;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct GuardedCycle {
    pub(super) relation: PositionRelationSet,
    pub(super) condition: PathCondition,
    /// This first-return cycle is exactly the anchor's explicitly tagged
    /// regular-transfer self-edge, not merely a relation with the same shape.
    pub(super) carrier: bool,
}

pub(super) fn guarded_cycle_displacements_cancel(cycles: &HashSet<GuardedCycle>) -> bool {
    let translations = cycles
        .iter()
        .filter_map(|cycle| {
            let (dependency, feasible) = cycle.relation.exact_translation()?;
            Some(GuardedTranslation {
                dependency,
                condition: cycle.condition.clone(),
                feasible,
                carrier: cycle.carrier,
            })
        })
        .collect::<Vec<_>>();

    if guarded_translations_cancel(&translations) {
        return true;
    }
    if translations.len() == cycles.len() {
        return false;
    }
    for cycle in cycles {
        for translation in &translations {
            if cycle
                .condition
                .conjoin_if_compatible(&translation.condition)
                .is_some()
                && translation.dependency.exact_offset().is_some_and(|offset| {
                    cycle
                        .relation
                        .closes_after_repeating_translation(offset, &translation.feasible)
                })
            {
                return true;
            }
        }
    }

    guarded_relations_close(cycles)
}

fn guarded_relations_close(cycles: &HashSet<GuardedCycle>) -> bool {
    let cycles = cycles.iter().collect::<Vec<_>>();
    let mut reached: Vec<(PositionRelationSet, PathCondition)> = Vec::new();
    let mut queue = VecDeque::new();
    // Exact-only closed walks were decided above. Any remaining closed walk
    // contains a non-translation relation and can be rotated to start there,
    // so do not enumerate arbitrarily long exact prefixes here.
    for cycle in cycles
        .iter()
        .filter(|cycle| cycle.relation.exact_translation().is_none())
    {
        insert_guarded_relation(
            &mut reached,
            &mut queue,
            cycle.relation.clone(),
            cycle.condition.clone(),
        );
    }
    while let Some((relation, condition)) = queue.pop_front() {
        for cycle in &cycles {
            let Some(next_condition) = condition.conjoin_if_compatible(&cycle.condition) else {
                continue;
            };
            let next_relation = relation.then(&cycle.relation);
            if next_relation.is_empty() {
                continue;
            }
            if next_relation.intersects_identity() {
                return true;
            }
            insert_guarded_relation(&mut reached, &mut queue, next_relation, next_condition);
        }
    }
    false
}

fn insert_guarded_relation(
    reached: &mut Vec<(PositionRelationSet, PathCondition)>,
    queue: &mut VecDeque<(PositionRelationSet, PathCondition)>,
    relation: PositionRelationSet,
    condition: PathCondition,
) {
    if reached
        .iter()
        .any(|(existing_relation, existing_condition)| {
            existing_relation.piecewise_covers(&relation) && existing_condition.covers(&condition)
        })
    {
        return;
    }
    reached.retain(|(existing_relation, existing_condition)| {
        !relation.piecewise_covers(existing_relation) || !condition.covers(existing_condition)
    });
    reached.push((relation.clone(), condition.clone()));
    queue.push_back((relation, condition));
}

#[derive(Clone, Debug)]
struct GuardedTranslation {
    dependency: BitDependency,
    condition: PathCondition,
    feasible: Vec<FeasiblePosition>,
    carrier: bool,
}

fn guarded_translations_cancel(cycles: &[GuardedTranslation]) -> bool {
    for cycles in guarded_transition_components_that_can_cancel(cycles) {
        // Large regular walks, such as +1 repeated 999_999 times followed by
        // one -999_999 wrap, are checked from their GCD-derived repetition
        // counts and interval endpoints. Runtime is independent of the
        // declared width.
        if guarded_opposing_pair_closes(&cycles) || guarded_three_cycle_closes(&cycles) {
            return true;
        }
        if let Some(closes) = tagged_carrier_with_singleton_resets_closes(&cycles) {
            if closes {
                return true;
            }
            continue;
        }
        // The remaining irregular cases retain the exact cumulative
        // translation. A state is reusable only when its branch condition and
        // valid starting positions cover the new state as well.
        let mut reached: HashMap<BitDependency, Vec<(PathCondition, Vec<FeasiblePosition>)>> =
            HashMap::default();
        let mut queue = VecDeque::new();
        for cycle in &cycles {
            insert_guarded_walk(
                &mut reached,
                &mut queue,
                cycle.dependency,
                cycle.condition.clone(),
                cycle.feasible.clone(),
            );
        }

        while let Some((dependency, condition, feasible)) = queue.pop_front() {
            for cycle in &cycles {
                let Some(next_condition) = condition.conjoin_if_compatible(&cycle.condition) else {
                    continue;
                };
                let offset = dependency
                    .exact_offset()
                    .expect("an exact guarded walk must retain its displacement");
                let next_feasible = compose_feasible_positions(&feasible, offset, &cycle.feasible);
                if next_feasible.is_empty() {
                    continue;
                }
                let next_dependency = dependency.compose(cycle.dependency);
                if next_dependency == BitDependency::identity() {
                    return true;
                }
                insert_guarded_walk(
                    &mut reached,
                    &mut queue,
                    next_dependency,
                    next_condition,
                    next_feasible,
                );
            }
        }
    }
    false
}

/// Exactly decides the finite-repeat shape with one explicitly tagged
/// carrier and singleton resets. Every word containing a reset can be rotated
/// to start at one. Between consecutive resets, the only available
/// non-singleton relation is `carrier^k`; the reset endpoints therefore fix
/// `k` by sign and divisibility. An edge in the finite reset graph exists iff
/// that exact segment is feasible. A directed cycle is consequently
/// equivalent to a concrete zero-displacement word. A carrier-only word
/// cannot close because its axis displacement is required to be non-zero.
fn tagged_carrier_with_singleton_resets_closes(cycles: &[&GuardedTranslation]) -> Option<bool> {
    if cycles
        .iter()
        .any(|cycle| !cycle.condition.is_unconditional())
    {
        return None;
    }
    let carriers = cycles
        .iter()
        .enumerate()
        .filter(|(_, cycle)| cycle.carrier)
        .collect::<Vec<_>>();
    let [(carrier_index, carrier)] = carriers.as_slice() else {
        return None;
    };
    if carrier.feasible.len() != 1 {
        return None;
    }
    let carrier_offset = carrier.dependency.exact_offset()?;
    let packed_axis = carrier_offset.0 == 0 && carrier_offset.1 != 0;
    let array_axis = carrier_offset.1 == 0 && carrier_offset.0 != 0;
    if !packed_axis && !array_axis {
        return None;
    }
    let active_offset = |offset: (isize, isize)| {
        if packed_axis { offset.1 } else { offset.0 }
    };
    let active_range = |position: FeasiblePosition| {
        if packed_axis {
            position.packed
        } else {
            position.array
        }
    };
    let inactive_range = |position: FeasiblePosition| {
        if packed_axis {
            position.array
        } else {
            position.packed
        }
    };
    let carrier_position = carrier.feasible[0];
    let carrier_active = active_range(carrier_position)?;
    let carrier_inactive = inactive_range(carrier_position);
    if carrier_active.0 >= carrier_active.1 {
        return None;
    }

    let resets = cycles
        .iter()
        .enumerate()
        .filter(|(index, _)| index != carrier_index)
        .collect::<Vec<_>>();
    if resets.is_empty() {
        return Some(false);
    }
    let mut reset_starts = Vec::with_capacity(resets.len());
    let mut reset_offsets = Vec::with_capacity(resets.len());
    for (_, reset) in &resets {
        if reset.carrier || reset.feasible.len() != 1 {
            return None;
        }
        let offset = reset.dependency.exact_offset()?;
        if (packed_axis && offset.0 != 0) || (array_axis && offset.1 != 0) {
            return None;
        }
        let position = reset.feasible[0];
        if inactive_range(position) != carrier_inactive {
            return None;
        }
        let (start, end) = active_range(position)?;
        if end != start.checked_add(1)? {
            return None;
        }
        reset_starts.push(start);
        reset_offsets.push(active_offset(offset));
    }

    let carrier_step = active_offset(carrier_offset);
    let mut transitions = Graph::<(), ()>::new();
    let nodes = (0..resets.len())
        .map(|_| transitions.add_node(()))
        .collect::<Vec<_>>();
    for (left_index, (_, left)) in resets.iter().enumerate() {
        let endpoint = reset_starts[left_index].checked_add(reset_offsets[left_index])?;
        for (right_index, (_, right)) in resets.iter().enumerate() {
            let Some(distance) = reset_starts[right_index].checked_sub(endpoint) else {
                continue;
            };
            let carrier_count = if distance == 0 {
                0
            } else {
                let Some(count) = distance.checked_div(carrier_step) else {
                    continue;
                };
                if distance % carrier_step != 0 || count <= 0 {
                    continue;
                }
                let Some(count) = usize::try_from(count).ok() else {
                    continue;
                };
                count
            };
            let segment = if carrier_count == 0 {
                (left.dependency, left.feasible.clone())
            } else {
                let Some(carrier_walk) = repeat_guarded_cycle(carrier, carrier_count) else {
                    continue;
                };
                let Some(segment) =
                    compose_guarded_walk(&(left.dependency, left.feasible.clone()), &carrier_walk)
                else {
                    continue;
                };
                segment
            };
            if compose_feasible_positions(&segment.1, segment.0.exact_offset()?, &right.feasible)
                .is_empty()
            {
                continue;
            }
            transitions.add_edge(nodes[left_index], nodes[right_index], ());
        }
    }
    Some(daggy::petgraph::algo::is_cyclic_directed(&transitions))
}

/// A closed word induces a directed cycle between the first-return relations
/// it uses: every relation must be able to hand its output position to the next
/// one. Its displacement vectors must therefore cancel inside one strongly
/// connected component of this coarse transition graph. Rejecting components
/// that cannot cancel avoids enumerating a declared width when opposing
/// translations are separated by a positional gap.
fn guarded_transition_components_that_can_cancel(
    cycles: &[GuardedTranslation],
) -> Vec<Vec<&GuardedTranslation>> {
    let mut transitions = Graph::<usize, ()>::new();
    let nodes = (0..cycles.len())
        .map(|cycle| transitions.add_node(cycle))
        .collect::<Vec<_>>();
    for (left_index, left) in cycles.iter().enumerate() {
        let offset = left
            .dependency
            .exact_offset()
            .expect("guarded translations must retain exact offsets");
        for (right_index, right) in cycles.iter().enumerate() {
            if left
                .condition
                .conjoin_if_compatible(&right.condition)
                .is_none()
                || compose_feasible_positions(&left.feasible, offset, &right.feasible).is_empty()
            {
                continue;
            }
            transitions.add_edge(nodes[left_index], nodes[right_index], ());
        }
    }

    tarjan_scc(&transitions)
        .into_iter()
        .filter_map(|component| {
            let cycles = component
                .into_iter()
                .map(|node| &cycles[transitions[node]])
                .collect::<Vec<_>>();
            let displacements = cycles
                .iter()
                .map(|cycle| (cycle.dependency, cycle.condition.clone()))
                .collect();
            compatible_cycle_displacements_cancel(&displacements).then_some(cycles)
        })
        .collect()
}

fn guarded_opposing_pair_closes(cycles: &[&GuardedTranslation]) -> bool {
    for left in 0..cycles.len() {
        for right in (left + 1)..cycles.len() {
            if cycles[left]
                .condition
                .conjoin_if_compatible(&cycles[right].condition)
                .is_none()
            {
                continue;
            }
            let (Some(left_offset), Some(right_offset)) = (
                cycles[left].dependency.exact_offset(),
                cycles[right].dependency.exact_offset(),
            ) else {
                continue;
            };
            let (Some(cross), Some(dot)) = (
                cross_product(left_offset, right_offset),
                dot_product(left_offset, right_offset),
            ) else {
                continue;
            };
            if cross != 0 || dot >= 0 {
                continue;
            }
            let Some((left_count, right_count)) =
                opposing_repetition_counts(left_offset, right_offset)
            else {
                continue;
            };
            let Some(left_walk) = repeat_guarded_cycle(cycles[left], left_count) else {
                continue;
            };
            let Some(right_walk) = repeat_guarded_cycle(cycles[right], right_count) else {
                continue;
            };
            debug_assert_eq!(left_walk.0.compose(right_walk.0), BitDependency::identity());
            if !compose_feasible_positions(
                &left_walk.1,
                left_walk
                    .0
                    .exact_offset()
                    .expect("an exact cycle stays exact"),
                &right_walk.1,
            )
            .is_empty()
                || !compose_feasible_positions(
                    &right_walk.1,
                    right_walk
                        .0
                        .exact_offset()
                        .expect("an exact cycle stays exact"),
                    &left_walk.1,
                )
                .is_empty()
            {
                return true;
            }
        }
    }
    false
}

fn guarded_three_cycle_closes(cycles: &[&GuardedTranslation]) -> bool {
    const ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for first in 0..cycles.len() {
        for second in (first + 1)..cycles.len() {
            let Some(condition) = cycles[first]
                .condition
                .conjoin_if_compatible(&cycles[second].condition)
            else {
                continue;
            };
            for third in (second + 1)..cycles.len() {
                if condition
                    .conjoin_if_compatible(&cycles[third].condition)
                    .is_none()
                {
                    continue;
                }
                let selected = [cycles[first], cycles[second], cycles[third]];
                let Some(offsets) = selected
                    .iter()
                    .map(|cycle| cycle.dependency.exact_offset())
                    .collect::<Option<Vec<_>>>()
                    .and_then(|offsets| <[(isize, isize); 3]>::try_from(offsets).ok())
                else {
                    continue;
                };
                let [Some(a), Some(b), Some(c)] = [
                    cross_product(offsets[1], offsets[2]),
                    cross_product(offsets[2], offsets[0]),
                    cross_product(offsets[0], offsets[1]),
                ] else {
                    continue;
                };
                let same_sign = [a, b, c].iter().all(|coefficient| *coefficient > 0)
                    || [a, b, c].iter().all(|coefficient| *coefficient < 0);
                if !same_sign {
                    continue;
                }
                let divisor = greatest_common_divisor(
                    greatest_common_divisor(a.unsigned_abs(), b.unsigned_abs()),
                    c.unsigned_abs(),
                );
                let counts = [
                    a.unsigned_abs() / divisor,
                    b.unsigned_abs() / divisor,
                    c.unsigned_abs() / divisor,
                ];
                let Some(walks) = selected
                    .iter()
                    .zip(counts)
                    .map(|(cycle, count)| repeat_guarded_cycle(cycle, count))
                    .collect::<Option<Vec<_>>>()
                    .and_then(|walks| <[_; 3]>::try_from(walks).ok())
                else {
                    continue;
                };
                for order in ORDERS {
                    let Some(combined) = compose_guarded_walk(&walks[order[0]], &walks[order[1]])
                        .and_then(|walk| compose_guarded_walk(&walk, &walks[order[2]]))
                    else {
                        continue;
                    };
                    if combined.0 == BitDependency::identity() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn compose_guarded_walk(
    left: &(BitDependency, Vec<FeasiblePosition>),
    right: &(BitDependency, Vec<FeasiblePosition>),
) -> Option<(BitDependency, Vec<FeasiblePosition>)> {
    let feasible = compose_feasible_positions(&left.1, left.0.exact_offset()?, &right.1);
    (!feasible.is_empty()).then_some((left.0.compose(right.0), feasible))
}

fn opposing_repetition_counts(
    left: (isize, isize),
    right: (isize, isize),
) -> Option<(usize, usize)> {
    let (left, right) = if left.0 != 0 {
        (left.0.unsigned_abs(), right.0.unsigned_abs())
    } else {
        (left.1.unsigned_abs(), right.1.unsigned_abs())
    };
    if left == 0 || right == 0 {
        return None;
    }
    let divisor = greatest_common_divisor(left, right);
    Some((right / divisor, left / divisor))
}

fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

fn repeat_guarded_cycle(
    cycle: &GuardedTranslation,
    count: usize,
) -> Option<(BitDependency, Vec<FeasiblePosition>)> {
    let offset = cycle.dependency.exact_offset()?;
    let repetitions = count.checked_sub(1)?;
    let repetitions = isize::try_from(repetitions).ok()?;
    let count = isize::try_from(count).ok()?;
    let array_shift = offset.0.checked_mul(repetitions)?;
    let packed_shift = offset.1.checked_mul(repetitions)?;
    let mut feasible = cycle
        .feasible
        .iter()
        .filter_map(|position| {
            Some(FeasiblePosition {
                array: repeat_axis(position.array, array_shift)?,
                packed: repeat_axis(position.packed, packed_shift)?,
            })
        })
        .collect::<Vec<_>>();
    feasible.sort_unstable();
    feasible.dedup();
    if feasible.is_empty() {
        return None;
    }
    Some((
        BitDependency {
            array: Some(offset.0.checked_mul(count)?),
            packed: Some(offset.1.checked_mul(count)?),
        },
        feasible,
    ))
}

fn repeat_axis(
    range: Option<(isize, isize)>,
    total_shift: isize,
) -> Option<Option<(isize, isize)>> {
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

fn insert_guarded_walk(
    reached: &mut HashMap<BitDependency, Vec<(PathCondition, Vec<FeasiblePosition>)>>,
    queue: &mut VecDeque<(BitDependency, PathCondition, Vec<FeasiblePosition>)>,
    dependency: BitDependency,
    condition: PathCondition,
    mut feasible: Vec<FeasiblePosition>,
) {
    feasible.sort_unstable();
    feasible.dedup();
    let states = reached.entry(dependency).or_default();
    if states
        .iter()
        .any(|(existing_condition, existing_feasible)| {
            existing_condition.covers(&condition)
                && feasible_positions_contain(existing_feasible, &feasible)
        })
    {
        return;
    }
    states.retain(|(existing_condition, existing_feasible)| {
        !condition.covers(existing_condition)
            || !feasible_positions_contain(&feasible, existing_feasible)
    });
    states.push((condition.clone(), feasible.clone()));
    queue.push_back((dependency, condition, feasible));
}

fn compose_feasible_positions(
    current: &[FeasiblePosition],
    current_offset: (isize, isize),
    next: &[FeasiblePosition],
) -> Vec<FeasiblePosition> {
    let mut result = Vec::new();
    for &current in current {
        for &next in next {
            let next_array = translate_axis_to_initial(next.array, current_offset.0);
            let next_packed = translate_axis_to_initial(next.packed, current_offset.1);
            let Some(array) = intersect_axis(current.array, next_array) else {
                continue;
            };
            let Some(packed) = intersect_axis(current.packed, next_packed) else {
                continue;
            };
            result.push(FeasiblePosition { array, packed });
        }
    }
    result.sort_unstable();
    result.dedup();
    result
}

fn translate_axis_to_initial(
    range: Option<(isize, isize)>,
    offset: isize,
) -> Option<(isize, isize)> {
    range.map(|(start, end)| {
        (
            start
                .checked_sub(offset)
                .expect("translated guard start must fit in isize"),
            end.checked_sub(offset)
                .expect("translated guard end must fit in isize"),
        )
    })
}

fn feasible_positions_contain(outer: &[FeasiblePosition], inner: &[FeasiblePosition]) -> bool {
    inner.iter().all(|inner| {
        outer.iter().any(|outer| {
            axis_contains(outer.array, inner.array) && axis_contains(outer.packed, inner.packed)
        })
    })
}

fn axis_contains(outer: Option<(isize, isize)>, inner: Option<(isize, isize)>) -> bool {
    match (outer, inner) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(outer), Some(inner)) => outer.0 <= inner.0 && inner.1 <= outer.1,
    }
}

pub(super) fn compatible_cycle_displacements_cancel(
    cycles: &HashSet<(BitDependency, PathCondition)>,
) -> bool {
    // A closed walk is a non-negative combination of its coarse cycles. In
    // two positional dimensions, a zero displacement needs at most three
    // cycle vectors. Keep the associated path conditions so mutually
    // exclusive cycles are never combined.
    let exact: Vec<_> = cycles
        .iter()
        .filter_map(|(dependency, condition)| {
            dependency.exact_offset().map(|offset| (offset, condition))
        })
        .collect();

    for left in 0..exact.len() {
        for right in (left + 1)..exact.len() {
            let Some(condition) = exact[left].1.conjoin_if_compatible(exact[right].1) else {
                continue;
            };
            if opposite_collinear(exact[left].0, exact[right].0) {
                return true;
            }
            for third in (right + 1)..exact.len() {
                if condition.conjoin_if_compatible(exact[third].1).is_some()
                    && origin_is_in_positive_cone(exact[left].0, exact[right].0, exact[third].0)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn opposite_collinear(left: (isize, isize), right: (isize, isize)) -> bool {
    let Some(cross) = cross_product(left, right) else {
        return true;
    };
    let Some(dot) = dot_product(left, right) else {
        return true;
    };
    cross == 0 && dot < 0
}

fn origin_is_in_positive_cone(a: (isize, isize), b: (isize, isize), c: (isize, isize)) -> bool {
    let [Some(first), Some(second), Some(third)] = [
        cross_product(b, c),
        cross_product(c, a),
        cross_product(a, b),
    ] else {
        return true;
    };
    let coefficients = [first, second, third];
    let has_positive = coefficients.iter().any(|&coefficient| coefficient > 0);
    let has_negative = coefficients.iter().any(|&coefficient| coefficient < 0);
    (has_positive || has_negative) && !(has_positive && has_negative)
}

fn cross_product(left: (isize, isize), right: (isize, isize)) -> Option<isize> {
    left.0
        .checked_mul(right.1)?
        .checked_sub(left.1.checked_mul(right.0)?)
}

fn dot_product(left: (isize, isize), right: (isize, isize)) -> Option<isize> {
    left.0
        .checked_mul(right.0)?
        .checked_add(left.1.checked_mul(right.1)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis_translation(
        packed_axis: bool,
        start: isize,
        length: isize,
        offset: isize,
        inactive: Option<(isize, isize)>,
        carrier: bool,
    ) -> GuardedTranslation {
        let (array, packed) = if packed_axis {
            (Some(0), Some(offset))
        } else {
            (Some(offset), Some(0))
        };
        let active = Some((start, start + length));
        GuardedTranslation {
            dependency: BitDependency { array, packed },
            condition: PathCondition::default(),
            feasible: vec![if packed_axis {
                FeasiblePosition {
                    array: inactive,
                    packed: active,
                }
            } else {
                FeasiblePosition {
                    array: active,
                    packed: inactive,
                }
            }],
            carrier,
        }
    }

    fn bounded_expansion_closes(cycles: &[GuardedTranslation], packed_axis: bool) -> bool {
        let mut graph = Graph::<(usize, isize), ()>::new();
        let mut nodes = HashMap::default();
        let mut get_node = |cycle: usize, position: isize, graph: &mut Graph<_, _>| {
            *nodes
                .entry((cycle, position))
                .or_insert_with(|| graph.add_node((cycle, position)))
        };
        for (left_index, left) in cycles.iter().enumerate() {
            let left_offset = left.dependency.exact_offset().unwrap();
            let left_offset = if packed_axis {
                left_offset.1
            } else {
                left_offset.0
            };
            let left_range = if packed_axis {
                left.feasible[0].packed
            } else {
                left.feasible[0].array
            }
            .unwrap();
            for source in left_range.0..left_range.1 {
                let endpoint = source + left_offset;
                for (right_index, right) in cycles.iter().enumerate() {
                    let right_range = if packed_axis {
                        right.feasible[0].packed
                    } else {
                        right.feasible[0].array
                    }
                    .unwrap();
                    if !(right_range.0..right_range.1).contains(&endpoint) {
                        continue;
                    }
                    let left_node = get_node(left_index, source, &mut graph);
                    let right_node = get_node(right_index, endpoint, &mut graph);
                    graph.add_edge(left_node, right_node, ());
                }
            }
        }
        daggy::petgraph::algo::is_cyclic_directed(&graph)
    }

    #[test]
    fn tagged_carrier_accelerator_matches_bounded_expansion() {
        for packed_axis in [false, true] {
            for carrier_start in -2..=1 {
                for carrier_length in 2..=5 {
                    for carrier_offset in [-2, -1, 1, 2] {
                        for first_start in -3..=4 {
                            for first_offset in -3..=3 {
                                for second_start in -3..=4 {
                                    for second_offset in -3..=3 {
                                        let cycles = vec![
                                            axis_translation(
                                                packed_axis,
                                                carrier_start,
                                                carrier_length,
                                                carrier_offset,
                                                Some((-1, 2)),
                                                true,
                                            ),
                                            axis_translation(
                                                packed_axis,
                                                first_start,
                                                1,
                                                first_offset,
                                                Some((-1, 2)),
                                                false,
                                            ),
                                            axis_translation(
                                                packed_axis,
                                                second_start,
                                                1,
                                                second_offset,
                                                Some((-1, 2)),
                                                false,
                                            ),
                                        ];
                                        let references = cycles.iter().collect::<Vec<_>>();
                                        assert_eq!(
                                            tagged_carrier_with_singleton_resets_closes(
                                                &references
                                            ),
                                            Some(bounded_expansion_closes(&cycles, packed_axis)),
                                            "axis={packed_axis}, carrier=({carrier_start}, {carrier_length}, {carrier_offset}), resets=({first_start}, {first_offset}), ({second_start}, {second_offset})"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn tagged_carrier_accelerator_matches_three_bounded_resets() {
        let mut state = 0x9e37_79b9_u32;
        let mut random = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        for packed_axis in [false, true] {
            for case in 0..50_000 {
                let carrier_start = random() as isize % 5 - 2;
                let carrier_length = 2 + random() as isize % 4;
                let carrier_offset = [-2, -1, 1, 2][random() as usize % 4];
                let mut cycles = vec![axis_translation(
                    packed_axis,
                    carrier_start,
                    carrier_length,
                    carrier_offset,
                    Some((-1, 2)),
                    true,
                )];
                for _ in 0..3 {
                    cycles.push(axis_translation(
                        packed_axis,
                        random() as isize % 8 - 3,
                        1,
                        random() as isize % 7 - 3,
                        Some((-1, 2)),
                        false,
                    ));
                }
                let references = cycles.iter().collect::<Vec<_>>();
                assert_eq!(
                    tagged_carrier_with_singleton_resets_closes(&references),
                    Some(bounded_expansion_closes(&cycles, packed_axis)),
                    "case={case}, axis={packed_axis}, cycles={cycles:?}"
                );
            }
        }
    }

    #[test]
    fn tagged_carrier_accelerator_rejects_inferred_or_incompatible_shapes() {
        let carrier = axis_translation(true, 0, 8, 1, Some((0, 2)), true);
        let mut cycles = [
            carrier.clone(),
            axis_translation(true, 0, 1, 1, Some((0, 2)), false),
        ];

        cycles[0].carrier = false;
        assert_eq!(
            tagged_carrier_with_singleton_resets_closes(&cycles.iter().collect::<Vec<_>>()),
            None
        );
        cycles[0] = carrier.clone();
        cycles[1].carrier = true;
        assert_eq!(
            tagged_carrier_with_singleton_resets_closes(&cycles.iter().collect::<Vec<_>>()),
            None
        );
        cycles[1] = axis_translation(true, 0, 1, 1, Some((1, 3)), false);
        assert_eq!(
            tagged_carrier_with_singleton_resets_closes(&cycles.iter().collect::<Vec<_>>()),
            None
        );
        cycles[1] = axis_translation(true, 0, 2, 1, Some((0, 2)), false);
        assert_eq!(
            tagged_carrier_with_singleton_resets_closes(&cycles.iter().collect::<Vec<_>>()),
            None
        );
        cycles[1] = axis_translation(true, 0, 1, 1, Some((0, 2)), false);
        cycles[1].condition = PathCondition::default()
            .with_choice(crate::comb_loop_detect::ssa::BranchId::new(1, 0, 2), 0);
        assert_eq!(
            tagged_carrier_with_singleton_resets_closes(&cycles.iter().collect::<Vec<_>>()),
            None
        );
    }
}
