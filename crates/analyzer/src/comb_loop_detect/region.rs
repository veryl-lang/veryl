use crate::HashMap;
use crate::conv::Context;
use crate::ir::{AssignDestination, MemberSelectDomain, Type, VarId, VarIndex, VarSelect};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct ArraySpan {
    pub(super) start: usize,
    pub(super) length: usize,
}

impl ArraySpan {
    pub(super) fn end(self) -> Option<usize> {
        self.start.checked_add(self.length)
    }

    pub(super) fn overlaps(self, other: Self) -> bool {
        let Some(left_end) = self.end() else {
            return false;
        };
        let Some(right_end) = other.end() else {
            return false;
        };
        self.start < right_end && other.start < left_end
    }

    pub(super) fn intersection(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end()?.min(other.end()?);
        let length = end.checked_sub(start)?;
        (length != 0).then_some(Self { start, length })
    }

    pub(super) fn translated(self, from: usize, to: usize) -> Option<Self> {
        let start = self.start.checked_sub(from)?.checked_add(to)?;
        (self.length != 0 && start.checked_add(self.length).is_some()).then_some(Self {
            start,
            length: self.length,
        })
    }
}

/// Half-open packed-bit interval. Unlike a dense bit mask, its storage and
/// operations are independent of the declared bit width.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct PackedSpan {
    pub(super) start: usize,
    pub(super) length: usize,
}

impl PackedSpan {
    pub(super) fn new(start: usize, length: usize) -> Option<Self> {
        (length != 0 && start.checked_add(length).is_some()).then_some(Self { start, length })
    }

    pub(super) fn whole(width: usize) -> Option<Self> {
        Self::new(0, width)
    }

    pub(super) fn from_select(high: usize, low: usize) -> Option<Self> {
        Self::new(low, high.checked_sub(low)?.checked_add(1)?)
    }

    pub(super) fn end(self) -> usize {
        self.start + self.length
    }

    pub(super) fn overlaps(self, other: Self) -> bool {
        self.start < other.end() && other.start < self.end()
    }

    pub(super) fn intersection(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end().min(other.end());
        Self::new(start, end.checked_sub(start)?)
    }

    pub(super) fn translated(self, from: usize, to: usize) -> Option<Self> {
        let start = self.start.checked_sub(from)?.checked_add(to)?;
        Self::new(start, self.length)
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

/// Mirrors the packed-region logic of `AssignDestination::eval_assign`.
pub(super) fn dst_writes(
    dst: &AssignDestination,
    ctx: &mut Context,
) -> Vec<(ArraySpan, PackedSpan)> {
    let Some(variable) = ctx.get_variable_info(dst.id) else {
        return Vec::new();
    };
    let Some((high, low)) = dst.select.conservative_packed_range(
        ctx,
        &variable.r#type,
        dst.comptime.member_select_domain,
    ) else {
        return Vec::new();
    };
    let Some(packed) = PackedSpan::from_select(high, low) else {
        return Vec::new();
    };

    array_access_span(&dst.index, &variable.r#type, ctx)
        .map(|span| vec![(span, packed)])
        .unwrap_or_default()
}

pub(super) fn var_reads(
    id: VarId,
    index: &VarIndex,
    select: &VarSelect,
    member_select_domain: Option<MemberSelectDomain>,
    ctx: &mut Context,
) -> Vec<(ArraySpan, PackedSpan)> {
    let Some(variable) = ctx.variables.get(&id).cloned() else {
        return Vec::new();
    };
    let Some((high, low)) =
        select.conservative_packed_range(ctx, &variable.r#type, member_select_domain)
    else {
        return Vec::new();
    };
    let Some(packed) = PackedSpan::from_select(high, low) else {
        return Vec::new();
    };
    array_access_span(index, &variable.r#type, ctx)
        .map(|span| vec![(span, packed)])
        .unwrap_or_default()
}

fn array_access_span(index: &VarIndex, r#type: &Type, ctx: &mut Context) -> Option<ArraySpan> {
    let prefix_len = index
        .0
        .iter()
        .take_while(|expression| expression.comptime().is_const)
        .count();
    let prefix = VarIndex(index.0[..prefix_len].to_vec());
    let values = prefix.eval_value(ctx)?;
    let (start, inclusive_end) = r#type.array.calc_range(&values)?;
    Some(ArraySpan {
        start,
        length: inclusive_end.checked_sub(start)?.checked_add(1)?,
    })
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
