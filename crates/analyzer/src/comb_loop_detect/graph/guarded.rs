//! Guarded composition of non-zero positional cycles.

use super::{FeasiblePosition, dependency_may_return_to_same_position, intersect_axis};
use crate::comb_loop_detect::model::BitDependency;
use crate::comb_loop_detect::ssa::PathCondition;
use crate::{HashMap, HashSet};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct GuardedCycle {
    /// Net translation after returning to the same graph node.
    pub(super) dependency: BitDependency,
    pub(super) condition: PathCondition,
    /// Positions at which every edge of this cycle is applicable.
    pub(super) feasible: Vec<FeasiblePosition>,
}

pub(super) fn guarded_cycle_displacements_cancel(cycles: &HashSet<GuardedCycle>) -> bool {
    // Displacement geometry is a cheap necessary test. It is deliberately
    // not sufficient: cycles whose guards cannot be connected must not be
    // reported merely because their vectors add to zero.
    let displacements = cycles
        .iter()
        .map(|cycle| (cycle.dependency, cycle.condition.clone()))
        .collect();
    if !compatible_cycle_displacements_cancel(&displacements) {
        return false;
    }

    let cycles = cycles.iter().collect::<Vec<_>>();
    // Large regular walks, such as +1 repeated 999_999 times followed by one
    // -999_999 wrap, are checked from their GCD-derived repetition counts and
    // interval endpoints. Runtime is independent of the declared width.
    if guarded_opposing_pair_closes(&cycles) || guarded_three_cycle_closes(&cycles) {
        return true;
    }
    // The remaining irregular cases retain the exact cumulative translation.
    // A state is reusable only when its branch condition and valid starting
    // positions cover the new state as well.
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
            let Some(next_condition) = condition.union_if_compatible(&cycle.condition) else {
                continue;
            };
            let next_dependency = dependency.compose(cycle.dependency);
            let Some(offset) = dependency.exact_offset() else {
                return true;
            };
            let next_feasible = compose_feasible_positions(&feasible, offset, &cycle.feasible);
            if next_feasible.is_empty() {
                continue;
            }
            if dependency_may_return_to_same_position(next_dependency) {
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
    false
}

fn guarded_opposing_pair_closes(cycles: &[&GuardedCycle]) -> bool {
    for left in 0..cycles.len() {
        for right in (left + 1)..cycles.len() {
            if cycles[left]
                .condition
                .union_if_compatible(&cycles[right].condition)
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
            debug_assert!(dependency_may_return_to_same_position(
                left_walk.0.compose(right_walk.0)
            ));
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

fn guarded_three_cycle_closes(cycles: &[&GuardedCycle]) -> bool {
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
                .union_if_compatible(&cycles[second].condition)
            else {
                continue;
            };
            for third in (second + 1)..cycles.len() {
                if condition
                    .union_if_compatible(&cycles[third].condition)
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
                    if dependency_may_return_to_same_position(combined.0) {
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
    cycle: &GuardedCycle,
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
            existing_condition.is_subset_of(&condition)
                && feasible_positions_contain(existing_feasible, &feasible)
        })
    {
        return;
    }
    states.retain(|(existing_condition, existing_feasible)| {
        !condition.is_subset_of(existing_condition)
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
            let Some(next_array) = translate_axis_to_initial(next.array, current_offset.0) else {
                continue;
            };
            let Some(next_packed) = translate_axis_to_initial(next.packed, current_offset.1) else {
                continue;
            };
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
) -> Option<Option<(isize, isize)>> {
    match range {
        None => Some(None),
        Some((start, end)) => Some(Some((start.checked_sub(offset)?, end.checked_sub(offset)?))),
    }
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
            let Some(condition) = exact[left].1.union_if_compatible(exact[right].1) else {
                continue;
            };
            if opposite_collinear(exact[left].0, exact[right].0) {
                return true;
            }
            for third in (right + 1)..exact.len() {
                if condition.union_if_compatible(exact[third].1).is_some()
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
