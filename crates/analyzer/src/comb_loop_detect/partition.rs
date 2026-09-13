//! Sparse partition construction with bounded Cartesian expansion.

#[cfg(test)]
use super::region::BitPartition;
use super::region::{ArraySpan, IdxKey, PackedSpan};
use crate::ir::VarId;
use crate::{HashMap, HashSet};

// A linear number of overlapping array and packed accesses can create a
// quadratic number of atoms. Bound repeated endpoint processing across the
// whole module. A linear allowance for accesses already present in the IR
// keeps large, disjoint assignments analyzable without permitting quadratic
// amplification of those accesses.
const PARTITION_WORK: usize = 1_000_000;
const WORK_PER_ACCESS: usize = 32;

#[cfg(test)]
thread_local! {
    static PARTITION_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static PARTITION_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn with_partition_work_limit<T>(limit: usize, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) {
            PARTITION_LIMIT.set(self.0);
        }
    }
    let _reset = Reset(PARTITION_LIMIT.replace(Some(limit)));
    f()
}

struct PartitionBudget {
    remaining: usize,
}

impl PartitionBudget {
    fn new(accesses: usize) -> Self {
        let remaining = PARTITION_WORK.saturating_add(accesses.saturating_mul(WORK_PER_ACCESS));
        #[cfg(test)]
        let remaining = PARTITION_LIMIT.get().unwrap_or(remaining);
        Self { remaining }
    }

    fn reserve(
        &mut self,
        spans: usize,
        capacity: usize,
        endpoints: Option<&HashSet<usize>>,
    ) -> Option<()> {
        let events = spans
            .checked_mul(2)?
            .checked_add(endpoints.map_or(0, HashSet::len))?;
        // Include the sort's logarithmic factor as well as event scanning and
        // output storage. Arithmetic overflow is also an exhausted budget.
        let cost = events.checked_mul(events.checked_ilog2().unwrap_or(0) as usize + 2)?;
        let cost = cost
            .checked_add(capacity)?
            .checked_add(endpoints.map_or(0, HashSet::capacity))?;
        self.remaining = self.remaining.checked_sub(cost)?;
        #[cfg(test)]
        PARTITION_VISITS.set(PARTITION_VISITS.get() + cost);
        Some(())
    }
}

/// Split only at observed access endpoints. Runtime and storage depend on the
/// number of accesses, never on the highest referenced bit position.
fn atomic_ranges(spans: &[PackedSpan], endpoints: Option<&HashSet<usize>>) -> Vec<PackedSpan> {
    let mut events = Vec::with_capacity(spans.len() * 2 + endpoints.map_or(0, HashSet::len));
    for span in spans {
        events.push((span.start, 1isize));
        events.push((span.end(), -1isize));
    }
    if let Some(endpoints) = endpoints {
        events.extend(endpoints.iter().map(|endpoint| (*endpoint, 0)));
    }
    events.sort_unstable_by_key(|event| event.0);

    let mut atoms = Vec::new();
    let mut active = 0isize;
    let mut index = 0;
    while index < events.len() {
        let position = events[index].0;
        while index < events.len() && events[index].0 == position {
            active += events[index].1;
            index += 1;
        }
        if active > 0
            && let Some(next) = events.get(index).map(|event| event.0)
            && let Some(atom) = PackedSpan::new(position, next - position)
        {
            atoms.push(atom);
        }
    }
    atoms
}

pub(super) fn split_array_spans(
    accesses_by_index: HashMap<IdxKey, Vec<PackedSpan>>,
    endpoints: &HashMap<VarId, HashSet<usize>>,
) -> Option<HashMap<IdxKey, Vec<PackedSpan>>> {
    let mut accesses: HashMap<VarId, Vec<(ArraySpan, PackedSpan)>> = HashMap::default();
    let mut access_count = 0usize;
    for ((id, span), packed_spans) in accesses_by_index {
        access_count = access_count.saturating_add(packed_spans.len());
        for packed in packed_spans {
            accesses.entry(id).or_default().push((span, packed));
        }
    }

    let mut budget = PartitionBudget::new(access_count);
    let mut ranges = HashMap::default();
    for (id, accesses) in accesses {
        let mut events = Vec::with_capacity(accesses.len() * 2);
        for (span, packed) in accesses {
            if span.length == 0 {
                continue;
            }
            let Some(end) = span.end() else {
                continue;
            };
            events.push((span.start, true, packed));
            events.push((end, false, packed));
        }
        events.sort_unstable_by_key(|(position, starts, packed)| {
            (*position, *starts, packed.start, packed.length)
        });

        let mut active: HashMap<PackedSpan, usize> = HashMap::default();
        let mut previous = events.first().map(|event| event.0);
        let mut cursor = 0;
        while cursor < events.len() {
            let position = events[cursor].0;
            if let Some(previous) = previous
                && previous < position
                && !active.is_empty()
            {
                let split = ArraySpan {
                    start: previous,
                    length: position - previous,
                };
                // Charge before scanning the hash table, allocating endpoint
                // events, sorting them, or retaining the resulting atoms.
                // HashMap iteration costs capacity, even after most overlapping
                // accesses have ended and only a few keys remain active.
                budget.reserve(active.len(), active.capacity(), endpoints.get(&id))?;
                let split_spans = active.keys().copied().collect::<Vec<_>>();
                let parts = atomic_ranges(&split_spans, endpoints.get(&id));
                if !parts.is_empty() {
                    ranges.insert((id, split), parts);
                }
            }
            while cursor < events.len() && events[cursor].0 == position {
                let (_, starts, packed) = events[cursor];
                if starts {
                    *active.entry(packed).or_default() += 1;
                } else if let std::collections::hash_map::Entry::Occupied(mut entry) =
                    active.entry(packed)
                {
                    *entry.get_mut() -= 1;
                    if *entry.get() == 0 {
                        entry.remove();
                    }
                }
                cursor += 1;
            }
            previous = Some(position);
        }
    }
    Some(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_array_and_packed_spans_have_bounded_expansion() {
        const LIMIT: usize = 16_384;
        for count in [8, 128, 1024] {
            let id = VarId::from_raw(0);
            let accesses = (0..count)
                .map(|start| {
                    (
                        (
                            id,
                            ArraySpan {
                                start,
                                length: count,
                            },
                        ),
                        vec![PackedSpan {
                            start,
                            length: count,
                        }],
                    )
                })
                .collect();
            with_partition_work_limit(LIMIT, || {
                PARTITION_VISITS.set(0);
                let ranges = split_array_spans(accesses, &HashMap::default());
                assert_eq!(ranges.is_some(), count == 8);
                assert!(PARTITION_VISITS.get() <= LIMIT);
                if let Some(ranges) = ranges {
                    assert_eq!(
                        ranges.values().map(Vec::len).sum::<usize>(),
                        count * count + (count - 1) * (count - 1)
                    );
                }
            });
        }
    }

    #[test]
    fn default_budget_stops_cartesian_growth_but_keeps_linear_partitions() {
        let id = VarId::from_raw(0);
        for count in [512, 2048] {
            let accesses = (0..count)
                .map(|start| {
                    (
                        (
                            id,
                            ArraySpan {
                                start,
                                length: count,
                            },
                        ),
                        vec![PackedSpan {
                            start,
                            length: count,
                        }],
                    )
                })
                .collect();
            PARTITION_VISITS.set(0);
            assert!(split_array_spans(accesses, &HashMap::default()).is_none());
            assert!(PARTITION_VISITS.get() <= PARTITION_WORK + count * WORK_PER_ACCESS);
        }

        // Already-expanded array assignments have one disjoint access per
        // element. Their work and output stay linear even beyond the fixed
        // allowance, so they must not lose proven feedback at distant indices.
        let count = 131_072;
        let accesses = (0..count)
            .map(|start| {
                (
                    (id, ArraySpan { start, length: 1 }),
                    vec![PackedSpan {
                        start: 0,
                        length: 1,
                    }],
                )
            })
            .collect();
        let ranges = split_array_spans(accesses, &HashMap::default()).unwrap();
        assert_eq!(ranges.len(), count);
    }

    #[test]
    fn partition_budget_is_shared_by_variables() {
        let packed = PackedSpan {
            start: 0,
            length: 1,
        };
        let accesses = (0..1024)
            .map(|id| {
                (
                    (
                        VarId::from_raw(id),
                        ArraySpan {
                            start: 0,
                            length: 1,
                        },
                    ),
                    vec![packed],
                )
            })
            .collect();
        with_partition_work_limit(128, || {
            PARTITION_VISITS.set(0);
            assert!(split_array_spans(accesses, &HashMap::default()).is_none());
            assert!(PARTITION_VISITS.get() <= 128);
        });
    }

    #[test]
    fn partition_budget_counts_retained_hash_capacity_and_extra_endpoints() {
        with_partition_work_limit(64, || {
            assert!(PartitionBudget::new(0).reserve(1, 1024, None).is_none());
            let endpoints = (0..1024).collect();
            assert!(
                PartitionBudget::new(0)
                    .reserve(1, 1, Some(&endpoints))
                    .is_none()
            );
            assert!(
                PartitionBudget::new(0)
                    .reserve(usize::MAX, 0, None)
                    .is_none()
            );
        });
    }

    #[test]
    fn packed_partition_storage_depends_on_endpoints_not_declared_width() {
        let distant = 1_000_000_000;
        let spans = [
            PackedSpan {
                start: 0,
                length: 1,
            },
            PackedSpan {
                start: distant,
                length: 1,
            },
        ];

        assert_eq!(atomic_ranges(&spans, None), spans);
    }
    #[test]
    fn array_partition_sweep_keeps_an_access_active_until_its_own_end() {
        let id = VarId::from_raw(0);
        let packed = PackedSpan {
            start: 0,
            length: 1,
        };
        let mut accesses = HashMap::default();
        accesses.insert(
            (
                id,
                ArraySpan {
                    start: 0,
                    length: 2,
                },
            ),
            vec![packed],
        );
        accesses.insert(
            (
                id,
                ArraySpan {
                    start: 1,
                    length: 2,
                },
            ),
            vec![packed],
        );

        let ranges = split_array_spans(accesses, &HashMap::default()).unwrap();
        for start in 0..3 {
            assert_eq!(
                ranges
                    .get(&(id, ArraySpan { start, length: 1 }))
                    .map(Vec::as_slice),
                Some([packed].as_slice())
            );
        }
    }
    #[test]
    fn disjoint_array_point_queries_do_not_scan_every_partition() {
        const COUNT: usize = 16_384;

        let id = VarId::from_raw(0);
        let packed = PackedSpan {
            start: 0,
            length: 32,
        };
        let mut accesses = HashMap::default();
        for start in 0..COUNT {
            accesses.insert((id, ArraySpan { start, length: 1 }), vec![packed]);
        }

        let ranges = split_array_spans(accesses, &HashMap::default()).unwrap();
        let partition = BitPartition::new(ranges);
        assert_eq!(partition.array_spans(id).len(), COUNT);
        for start in 0..COUNT {
            assert_eq!(
                partition.overlapping_access(id, ArraySpan { start, length: 1 }, packed),
                vec![(id, ArraySpan { start, length: 1 }, 0)]
            );
        }
    }
    #[test]
    fn partition_rejects_positions_that_do_not_fit_the_relation_type() {
        let id = VarId::from_raw(0);
        let mut ranges = HashMap::default();
        ranges.insert(
            (
                id,
                ArraySpan {
                    start: isize::MAX as usize + 1,
                    length: 1,
                },
            ),
            vec![PackedSpan {
                start: 0,
                length: 1,
            }],
        );

        assert_eq!(BitPartition::new(ranges).position_overflow(), Some(id));
    }
}
