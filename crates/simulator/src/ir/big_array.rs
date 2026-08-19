//! Analysis-side folding of very large dynamically-indexed arrays.
//!
//! `gather_variable_offsets_expanded` names every element a dynamic access can
//! reach, so that a middle element is never missed by a base+last summary.
//! For a multi-million-element memory that expansion dominates elaboration:
//! every pass building an offset map pays `num_elements` pushes and hash
//! inserts per access, and the maps themselves grow to one entry per element.
//!
//! Above [`FOLD_SPAN_BYTES`] the array folds onto a single representative
//! offset — its base — for analysis purposes.  Both sides of the analysis go
//! through the same fold: a dynamic access contributes the base instead of
//! every element, and a static element access is canonicalised to the same
//! base.  Element-level distinctions then vanish from the offset maps, which
//! only ever MERGES accesses that were previously distinct — every alias the
//! expansion reported is still reported, so the passes stay conservative and a
//! folded offset simply stops being a fusion candidate.
//!
//! Below the cap nothing folds and the expansion is unchanged, so designs
//! without a multi-megabyte memory behave exactly as before.

use crate::ir::variable::VarOffset;

/// Byte span above which a dynamically-indexed array folds onto its base.
///
/// Sized so that per-element precision is only given up where it cannot pay
/// for itself: an array this large is a memory, whose elements are reached by
/// runtime indices that no offset-keyed pass can tell apart anyway.
pub const FOLD_SPAN_BYTES: usize = (1 << 20) * 8;

/// The set of arrays folded onto their base offset, as byte ranges.
///
/// Built per analysis pass from the statements that pass will walk (see
/// [`BigArrayFold::from_statements`]).  A fold missing an array is safe — it
/// just leaves the expansion in place — because the gather and the
/// canonicalisation consult the same instance.
#[derive(Clone, Debug, Default)]
pub struct BigArrayFold {
    /// `(is_ff, start, end)` half-open byte ranges, sorted and non-overlapping.
    spans: Vec<(bool, isize, isize)>,
}

impl BigArrayFold {
    /// Record one dynamic array access.  Spans at or below the cap are
    /// ignored, leaving those arrays expanded per element.
    pub fn record(&mut self, base: VarOffset, stride: isize, num_elements: usize) {
        if stride <= 0 || num_elements <= 1 {
            return;
        }
        let span = (stride as usize).saturating_mul(num_elements);
        if span <= FOLD_SPAN_BYTES {
            return;
        }
        let start = base.raw();
        self.spans
            .push((base.is_ff(), start, start.saturating_add(span as isize)));
    }

    /// Sort and merge the recorded ranges.  Must be called once recording is
    /// done; the lookups below assume sorted, non-overlapping spans.
    pub fn finish(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        self.spans.sort_unstable();
        let mut merged: Vec<(bool, isize, isize)> = Vec::with_capacity(self.spans.len());
        for &(is_ff, start, end) in &self.spans {
            match merged.last_mut() {
                // Merging overlaps is what keeps `canon` idempotent — an
                // array reached at different base elements records ranges
                // that would otherwise fold onto two bases.  Touching ranges
                // merge as well: sharing one span between two arrays only
                // makes `canon` coarser, never less conservative.
                Some(last) if last.0 == is_ff && start <= last.2 => {
                    last.2 = last.2.max(end);
                }
                _ => merged.push((is_ff, start, end)),
            }
        }
        self.spans = merged;
    }

    /// Build the fold for the arrays reached by `stmts`.
    pub fn from_statements<'a, I>(stmts: I) -> Self
    where
        I: IntoIterator<Item = &'a crate::ir::ProtoStatement>,
    {
        let mut fold = Self::default();
        for s in stmts {
            s.collect_big_arrays(&mut fold);
        }
        fold.finish();
        fold
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Index of the span containing `off`, if any.
    #[inline]
    fn find(&self, off: VarOffset) -> Option<usize> {
        if self.spans.is_empty() {
            return None;
        }
        let key = (off.is_ff(), off.raw());
        // Last span starting at or before `off`.
        let i = self
            .spans
            .partition_point(|&(is_ff, start, _)| (is_ff, start) <= key);
        let i = i.checked_sub(1)?;
        let (is_ff, _, end) = self.spans[i];
        (is_ff == key.0 && key.1 < end).then_some(i)
    }

    /// True when `off` is an element of a folded array.
    #[inline]
    pub fn covers(&self, off: VarOffset) -> bool {
        self.find(off).is_some()
    }

    /// `off` mapped onto its array's base when folded, unchanged otherwise.
    #[inline]
    pub fn canon(&self, off: VarOffset) -> VarOffset {
        match self.find(off) {
            Some(i) => VarOffset::new(off.is_ff(), self.spans[i].1),
            None => off,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIG: usize = FOLD_SPAN_BYTES / 8 + 1;

    #[test]
    fn small_arrays_do_not_fold() {
        let mut f = BigArrayFold::default();
        f.record(VarOffset::Comb(0), 8, 16);
        f.finish();
        assert!(f.is_empty());
        assert_eq!(f.canon(VarOffset::Comb(8)), VarOffset::Comb(8));
    }

    #[test]
    fn elements_canonicalise_to_the_base() {
        let mut f = BigArrayFold::default();
        f.record(VarOffset::Comb(64), 8, BIG);
        f.finish();
        assert!(f.covers(VarOffset::Comb(64)));
        assert!(f.covers(VarOffset::Comb(64 + 8 * (BIG as isize - 1))));
        assert_eq!(f.canon(VarOffset::Comb(64 + 4096)), VarOffset::Comb(64));
        // One past the end, and the other buffer, stay untouched.
        assert!(!f.covers(VarOffset::Comb(64 + 8 * BIG as isize)));
        assert!(!f.covers(VarOffset::Ff(64)));
        assert_eq!(f.canon(VarOffset::Ff(64 + 4096)), VarOffset::Ff(64 + 4096));
    }

    #[test]
    fn canon_is_idempotent_across_overlapping_records() {
        let mut f = BigArrayFold::default();
        f.record(VarOffset::Ff(0), 8, BIG);
        f.record(VarOffset::Ff(0), 8, BIG);
        f.record(VarOffset::Ff(8), 8, BIG);
        f.finish();
        assert_eq!(f.spans.len(), 1);
        let c = f.canon(VarOffset::Ff(4096));
        assert_eq!(c, VarOffset::Ff(0));
        assert_eq!(f.canon(c), c);
    }

    #[test]
    fn separate_arrays_keep_separate_bases() {
        let far = 8 * BIG as isize + 4096;
        let mut f = BigArrayFold::default();
        f.record(VarOffset::Comb(0), 8, BIG);
        f.record(VarOffset::Comb(far), 8, BIG);
        f.finish();
        assert_eq!(f.spans.len(), 2);
        assert_eq!(f.canon(VarOffset::Comb(4096)), VarOffset::Comb(0));
        assert_eq!(f.canon(VarOffset::Comb(far + 4096)), VarOffset::Comb(far));
        // The gap between them is nobody's element.
        assert!(!f.covers(VarOffset::Comb(far - 8)));
    }
}
