use crate::HashMap;
use crate::conv::Context;
use crate::ir::{AssignDestination, MemberSelectDomain, Type, VarId, VarIndex, VarSelect};

#[cfg(test)]
thread_local! {
    static PACKED_QUERY_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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

/// Per `IdxKey`, atomic packed-bit intervals.
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

    pub(super) fn overlapping(&self, key: IdxKey, span: PackedSpan) -> std::ops::Range<usize> {
        // Atomic ranges are sorted and disjoint. Locate both boundaries in
        // logarithmic time so repeated point accesses cannot scan every bit
        // partition. Preserve the original indices used by NodeKey.
        let ranges = self.ranges_of(key);
        let first = ranges.partition_point(|range| {
            #[cfg(test)]
            PACKED_QUERY_PROBES.set(PACKED_QUERY_PROBES.get() + 1);
            range.end() <= span.start
        });
        let count = ranges[first..].partition_point(|range| {
            #[cfg(test)]
            PACKED_QUERY_PROBES.set(PACKED_QUERY_PROBES.get() + 1);
            range.start < span.end()
        });
        first..first + count
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
        spans[first..]
            .iter()
            .take_while(|split| split.start < access_end)
            .filter(|split| split.overlaps(access))
            .flat_map(|split| {
                self.overlapping((id, *split), span)
                    .map(|range| (id, *split, range))
            })
            .collect()
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
    fn packed_point_queries_take_logarithmic_work() {
        const COUNT: usize = 16_384;
        let id = VarId::from_raw(0);
        let array = ArraySpan {
            start: 0,
            length: 1,
        };
        let ranges = (0..COUNT)
            .map(|index| PackedSpan {
                start: index * 2,
                length: 1,
            })
            .collect();
        let partition = BitPartition::new(HashMap::from_iter([((id, array), ranges)]));
        PACKED_QUERY_PROBES.set(0);
        for index in 0..COUNT {
            assert_eq!(
                partition.overlapping_access(
                    id,
                    array,
                    PackedSpan {
                        start: index * 2,
                        length: 1
                    }
                ),
                vec![(id, array, index)]
            );
        }
        assert!(PACKED_QUERY_PROBES.get() <= COUNT * 32);
    }

    #[test]
    fn packed_queries_preserve_gaps_boundaries_and_original_indices() {
        let id = VarId::from_raw(0);
        let array = ArraySpan {
            start: 1,
            length: 2,
        };
        let ranges = vec![
            PackedSpan {
                start: 2,
                length: 2,
            },
            PackedSpan {
                start: 4,
                length: 1,
            },
            PackedSpan {
                start: 8,
                length: 2,
            },
        ];
        let partition = BitPartition::new(HashMap::from_iter([((id, array), ranges.clone())]));
        for start in 0..12 {
            for length in 1..12 {
                let query = PackedSpan { start, length };
                let expected = ranges
                    .iter()
                    .enumerate()
                    .filter(|(_, range)| range.overlaps(query))
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                assert_eq!(
                    partition
                        .overlapping((id, array), query)
                        .collect::<Vec<_>>(),
                    expected
                );
                assert_eq!(
                    partition.overlapping_access(
                        id,
                        ArraySpan {
                            start: 0,
                            length: 4
                        },
                        query
                    ),
                    expected
                        .into_iter()
                        .map(|index| (id, array, index))
                        .collect::<Vec<_>>()
                );
            }
        }
        assert!(
            partition
                .overlapping_access(
                    id,
                    ArraySpan {
                        start: 0,
                        length: 1
                    },
                    ranges[0]
                )
                .is_empty()
        );
        assert!(
            partition
                .overlapping(
                    (
                        id,
                        ArraySpan {
                            start: 0,
                            length: 1
                        }
                    ),
                    ranges[0]
                )
                .is_empty()
        );
    }
}
