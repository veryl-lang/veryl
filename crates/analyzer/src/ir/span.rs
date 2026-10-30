//! Interval regions of a variable: unpacked-array elements and packed bits.
//! Storage and operations are independent of array sizes and bit widths.

use crate::BigUint;
use crate::conv::Context;
use crate::ir::{AssignDestination, MemberSelectDomain, Type, VarId, VarIndex, VarSelect};

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ArraySpan {
    pub start: usize,
    pub length: usize,
}

impl ArraySpan {
    pub fn end(self) -> Option<usize> {
        self.start.checked_add(self.length)
    }

    pub fn overlaps(self, other: Self) -> bool {
        let Some(left_end) = self.end() else {
            return false;
        };
        let Some(right_end) = other.end() else {
            return false;
        };
        self.start < right_end && other.start < left_end
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end()?.min(other.end()?);
        let length = end.checked_sub(start)?;
        (length != 0).then_some(Self { start, length })
    }

    pub fn translated(self, from: usize, to: usize) -> Option<Self> {
        let start = self.start.checked_sub(from)?.checked_add(to)?;
        (self.length != 0 && start.checked_add(self.length).is_some()).then_some(Self {
            start,
            length: self.length,
        })
    }

    /// The union of two touching or overlapping spans.
    pub fn union(self, other: Self) -> Option<Self> {
        let start = self.start.min(other.start);
        let end = self.end()?.max(other.end()?);
        (self.start <= other.end()? && other.start <= self.end()?).then_some(Self {
            start,
            length: end - start,
        })
    }
}

/// Half-open packed-bit interval. Unlike a dense bit mask, its storage and
/// operations are independent of the declared bit width.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct PackedSpan {
    pub start: usize,
    pub length: usize,
}

impl PackedSpan {
    pub fn new(start: usize, length: usize) -> Option<Self> {
        (length != 0 && start.checked_add(length).is_some()).then_some(Self { start, length })
    }

    pub fn whole(width: usize) -> Option<Self> {
        Self::new(0, width)
    }

    pub fn from_select(high: usize, low: usize) -> Option<Self> {
        Self::new(low, high.checked_sub(low)?.checked_add(1)?)
    }

    pub fn end(self) -> usize {
        self.start + self.length
    }

    /// The bits of the span as a dense mask.
    pub fn mask(self) -> BigUint {
        ((BigUint::from(1u32) << self.length) - 1u32) << self.start
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end() && other.start < self.end()
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end().min(other.end());
        Self::new(start, end.checked_sub(start)?)
    }

    pub fn translated(self, from: usize, to: usize) -> Option<Self> {
        let start = self.start.checked_sub(from)?.checked_add(to)?;
        Self::new(start, self.length)
    }

    /// The union of two touching or overlapping spans.
    pub fn union(self, other: Self) -> Option<Self> {
        let start = self.start.min(other.start);
        let end = self.end().max(other.end());
        (self.start <= other.end() && other.start <= self.end()).then_some(Self {
            start,
            length: end - start,
        })
    }
}

/// Mirrors the packed-region logic of `AssignDestination::eval_assign`.
pub fn dst_writes(dst: &AssignDestination, ctx: &mut Context) -> Vec<(ArraySpan, PackedSpan)> {
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

pub fn var_reads(
    id: VarId,
    index: &VarIndex,
    select: &VarSelect,
    member_select_domain: Option<MemberSelectDomain>,
    ctx: &mut Context,
) -> Vec<(ArraySpan, PackedSpan)> {
    // Only the type is needed, not the variable's values.
    let Some(r#type) = ctx
        .variables
        .get(&id)
        .map(|variable| variable.r#type.clone())
    else {
        return Vec::new();
    };
    let Some((high, low)) = select.conservative_packed_range(ctx, &r#type, member_select_domain)
    else {
        return Vec::new();
    };
    let Some(packed) = PackedSpan::from_select(high, low) else {
        return Vec::new();
    };
    array_access_span(index, &r#type, ctx)
        .map(|span| vec![(span, packed)])
        .unwrap_or_default()
}

fn array_access_span(index: &VarIndex, r#type: &Type, ctx: &mut Context) -> Option<ArraySpan> {
    let (start, inclusive_end) = index.read_range(ctx, &r#type.array)?;
    Some(ArraySpan {
        start,
        length: inclusive_end.checked_sub(start)?.checked_add(1)?,
    })
}
