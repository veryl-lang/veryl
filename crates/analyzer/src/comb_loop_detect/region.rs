use crate::HashMap;
use crate::conv::Context;
use crate::ir::{AssignDestination, Type, VarId, VarIndex, VarSelect};

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

    /// Empty slice means the variable's bits are untouched.
    pub(super) fn ranges_of(&self, key: IdxKey) -> &[PackedSpan] {
        self.ranges.get(&key).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub(super) fn overlapping(&self, key: IdxKey, span: PackedSpan) -> Vec<usize> {
        self.ranges_of(key)
            .iter()
            .enumerate()
            .filter(|(_, range)| range.overlaps(span))
            .map(|(i, _)| i)
            .collect()
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
                let ranges = self.ranges_of((id, *split));
                ranges
                    .iter()
                    .enumerate()
                    .filter(|(_, range)| range.overlaps(span))
                    .map(|(range, _)| (id, *split, range))
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
    let is_select_const = dst.select.is_const();

    let packed = if !is_select_const {
        let Some(packed) = conservative_select_span(&dst.select, &variable.r#type, ctx) else {
            return Vec::new();
        };
        packed
    } else {
        let Some((high, low)) = dst.select.eval_value(ctx, &variable.r#type, false) else {
            return Vec::new();
        };
        let Some(packed) = PackedSpan::from_select(high, low) else {
            return Vec::new();
        };
        packed
    };

    array_access_span(&dst.index, &variable.r#type, ctx)
        .map(|span| vec![(span, packed)])
        .unwrap_or_default()
}

pub(super) fn var_reads(
    id: VarId,
    index: &VarIndex,
    select: &VarSelect,
    ctx: &mut Context,
) -> Vec<(ArraySpan, PackedSpan)> {
    let Some(variable) = ctx.variables.get(&id).cloned() else {
        return Vec::new();
    };
    let packed = if select.is_const_with_range()
        && let Some((high, low)) = select.eval_value(ctx, &variable.r#type, false)
    {
        let Some(packed) = PackedSpan::from_select(high, low) else {
            return Vec::new();
        };
        packed
    } else {
        let Some(packed) = conservative_select_span(select, &variable.r#type, ctx) else {
            return Vec::new();
        };
        packed
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

fn conservative_select_span(
    select: &VarSelect,
    r#type: &Type,
    ctx: &mut Context,
) -> Option<PackedSpan> {
    let prefix = VarSelect(
        select
            .0
            .iter()
            .take_while(|expression| expression.comptime().is_const)
            .cloned()
            .collect(),
        None,
    );
    if let Some((high, low)) = prefix.eval_value(ctx, r#type, false) {
        PackedSpan::from_select(high, low)
    } else {
        r#type.total_width().and_then(PackedSpan::whole)
    }
}
