use crate::HashMap;
use crate::ir::VarId;
pub(super) use crate::ir::span::{ArraySpan, PackedSpan, dst_writes, var_reads};

pub(super) fn signed_difference(destination: usize, source: usize) -> Option<isize> {
    isize::try_from(destination)
        .ok()?
        .checked_sub(isize::try_from(source).ok()?)
}

pub(super) fn translate_position(position: usize, offset: isize) -> Option<usize> {
    if offset >= 0 {
        position.checked_add(offset.unsigned_abs())
    } else {
        position.checked_sub(offset.unsigned_abs())
    }
}

/// One split unpacked-array interval. Bit precision lives in packed spans.
pub(super) type IdxKey = (VarId, ArraySpan);

/// `(VarId, array_idx, range_idx)`. `range_idx` indexes the variable's
/// `BitPartition`, so bit-disjoint reads/writes form disjoint nodes.
pub(super) type NodeKey = (VarId, ArraySpan, usize);

/// Per `IdxKey`, sorted, non-overlapping atomic packed-bit intervals.
#[derive(Default)]
pub(super) struct BitPartition {
    ranges: HashMap<IdxKey, Vec<PackedSpan>>,
    array_spans: HashMap<VarId, Vec<ArraySpan>>,
}

impl BitPartition {
    pub(super) fn new(ranges: HashMap<IdxKey, Vec<PackedSpan>>) -> Self {
        debug_assert!(
            ranges
                .values()
                .all(|spans| { spans.windows(2).all(|pair| pair[0].end() <= pair[1].start) })
        );
        let mut array_spans: HashMap<VarId, Vec<ArraySpan>> = HashMap::default();
        for &(id, span) in ranges.keys() {
            array_spans.entry(id).or_default().push(span);
        }
        for spans in array_spans.values_mut() {
            spans.sort_unstable();
            spans.dedup();
            debug_assert!(
                spans
                    .windows(2)
                    .all(|pair| pair[0].end().is_some_and(|end| end <= pair[1].start))
            );
        }
        Self {
            ranges,
            array_spans,
        }
    }

    pub(super) fn array_spans(&self, id: VarId) -> &[ArraySpan] {
        self.array_spans.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(super) fn position_overflow(&self) -> Option<VarId> {
        let limit = isize::MAX as usize;
        self.ranges.iter().find_map(|(&(id, array), packed)| {
            let array_overflows = array.start > limit || array.end().is_none_or(|end| end > limit);
            let packed_overflows = packed
                .iter()
                .any(|span| span.start > limit || span.end() > limit);
            (array_overflows || packed_overflows).then_some(id)
        })
    }

    /// Empty slice means the variable's bits are untouched.
    pub(super) fn ranges_of(&self, key: IdxKey) -> &[PackedSpan] {
        self.ranges.get(&key).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub(super) fn overlapping(
        &self,
        key: IdxKey,
        span: PackedSpan,
    ) -> impl Iterator<Item = usize> + '_ {
        let ranges = self.ranges_of(key);
        let first = ranges.partition_point(|range| range.end() <= span.start);
        ranges[first..]
            .iter()
            .enumerate()
            .take_while(move |(_, range)| range.start < span.end())
            .map(move |(i, _)| first + i)
    }

    pub(super) fn overlapping_access(
        &self,
        id: VarId,
        access: ArraySpan,
        span: PackedSpan,
    ) -> Vec<NodeKey> {
        let Some(access_end) = access.end() else {
            return Vec::new();
        };
        let spans = self.array_spans(id);
        let first = spans.partition_point(|span| span.end().is_some_and(|end| end <= access.start));
        let mut keys = spans[first..]
            .iter()
            .take_while(|split| split.start < access_end)
            .filter(|split| split.overlaps(access))
            .flat_map(|split| {
                self.overlapping((id, *split), span)
                    .map(move |range| (id, *split, range))
            })
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_queries_preserve_interval_indices() {
        let id = VarId::from_raw(0);
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        for offset in [0, 1_000_000_000] {
            let ranges = [(2, 3), (5, 1), (8, 2)]
                .map(|(start, length)| PackedSpan::new(offset + start, length).unwrap());
            let partition =
                BitPartition::new([((id, array), ranges.to_vec())].into_iter().collect());
            let queries: &[(usize, usize, &[usize])] = &[
                (0, 2, &[]),
                (0, 3, &[0]),
                (2, 3, &[0]),
                (3, 3, &[0, 1]),
                (5, 1, &[1]),
                (6, 2, &[]),
                (7, 2, &[2]),
                (9, 4, &[2]),
                (10, 1, &[]),
                (0, 13, &[0, 1, 2]),
            ];
            for &(start, length, expected) in queries {
                let span = PackedSpan::new(offset + start, length).unwrap();
                assert_eq!(
                    partition.overlapping((id, array), span).collect::<Vec<_>>(),
                    expected,
                    "offset={offset}, span={span:?}"
                );
                assert_eq!(
                    partition.overlapping_access(id, array, span),
                    expected.iter().map(|&i| (id, array, i)).collect::<Vec<_>>()
                );
                let missing = VarId::from_raw(1);
                assert_eq!(partition.overlapping((missing, array), span).next(), None);
                assert!(
                    partition
                        .overlapping_access(missing, array, span)
                        .is_empty()
                );
            }
        }
    }

    #[test]
    fn packed_queries_use_each_array_partitions_indices() {
        let id = VarId::from_raw(0);
        let first = ArraySpan {
            start: 0,
            length: 2,
        };
        let second = ArraySpan {
            start: 3,
            length: 2,
        };
        let partition = BitPartition::new(
            [
                (
                    (id, first),
                    vec![
                        PackedSpan::new(0, 2).unwrap(),
                        PackedSpan::new(4, 2).unwrap(),
                    ],
                ),
                (
                    (id, second),
                    vec![
                        PackedSpan::new(1, 2).unwrap(),
                        PackedSpan::new(3, 1).unwrap(),
                        PackedSpan::new(7, 2).unwrap(),
                    ],
                ),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            partition.overlapping_access(
                id,
                ArraySpan {
                    start: 1,
                    length: 3
                },
                PackedSpan::new(2, 6).unwrap()
            ),
            vec![
                (id, first, 1),
                (id, second, 0),
                (id, second, 1),
                (id, second, 2)
            ]
        );
    }
}
