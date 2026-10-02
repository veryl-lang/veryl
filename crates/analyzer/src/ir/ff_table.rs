use crate::BigUint;
use crate::HashMap;
use crate::conv::Context;
use crate::ir::VarId;
use crate::ir::declaration::Declaration;
use crate::ir::write_count::{UnsafeSelfReads, unsafe_self_reads};

/// LHS of an assignment: `(VarId, array element index, bit-mask)`.
/// `None` array index = dynamic. Empty bit-mask = unavailable (consumers
/// fall back to per-decl aggregates).
pub type AssignTarget = (VarId, Option<usize>, BigUint);

/// One entry of [`FfTableEntry::refered`].
pub type Refered = (usize, Option<AssignTarget>, BigUint, bool);

/// A reference to a contiguous interval, including whole-array dynamic reads.
pub type RangeRefered = (std::ops::Range<usize>, Refered);

#[derive(Clone, Debug)]
pub struct FfTableEntry {
    pub assigned: Option<usize>,
    /// More than one always_ff declaration writes this key. `assigned` keeps
    /// only the last of them, so the exemption for a block reading what it
    /// wrote itself would clear a read of what another block wrote: the bits
    /// differ but the key, a whole packed word, is the same.
    pub multi_assigned: bool,
    /// `(decl_index, assign_target, src_read_mask, from_ff)` per reference.
    /// `assign_target` is `None` for condition expressions and similar
    /// non-assign contexts. Empty `src_read_mask` = unavailable (fall back
    /// to per-decl aggregate). `from_ff` distinguishes always_ff (NBA-
    /// sensitive) from always_comb / continuous assign.
    /// References through an unevaluated index are kept apart, in
    /// [`FfTable::range_refered`]; [`FfTable::refered`] yields both.
    pub refered: Vec<Refered>,
    pub is_ff: bool,
    pub assigned_comb: Option<usize>,
}

/// Writes through an index that does not evaluate, which reach every element
/// below `len`. An element with an entry of its own has them folded into it;
/// this describes every other element.
#[derive(Clone, Debug, Default)]
pub struct WholeAssigned {
    pub len: usize,
    pub assigned: Option<usize>,
    pub multi_assigned: bool,
    pub assigned_comb: Option<usize>,
    force_ff: bool,
}

fn record_assign(assigned: &mut Option<usize>, multi_assigned: &mut bool, decl: usize) {
    if assigned.is_some_and(|d| d != decl) {
        *multi_assigned = true;
    }
    *assigned = Some(decl);
}

/// `index` is `None` for an element no reference names by index.
fn classify<'a>(
    assigned_decl: usize,
    multi_assigned: bool,
    id: VarId,
    index: Option<usize>,
    readable: bool,
    mut refs: impl Iterator<Item = &'a Refered>,
) -> bool {
    // FF classification rules (strict NBA semantics):
    // - A variable may be treated as comb (ff_opt) only if no always_ff
    //   block reads it (cross-block NBA races would be violated).
    // - always_comb / continuous assigns re-evaluate after NBA in SV,
    //   so they correctly see new FF values; ff_opt is safe for them.
    // - Within the same always_ff (assigned_decl), a self-reference
    //   is safe while it still reads what the block started with
    //   (see `write_count`); every other read must see old values.
    refs.any(|(decl, assign_target, _src_mask, from_ff)| {
        if !from_ff {
            return false;
        }
        if *decl != assigned_decl {
            return true;
        }
        // With a second writing block the exemption cannot hold:
        // this read may be of a bit that block wrote, and `refered`
        // records only the array index, not which bits.
        if multi_assigned {
            return true;
        }
        match assign_target {
            Some((target_id, target_idx, _)) => {
                if *target_id != id {
                    return true;
                }
                // A dynamic index is conservative → FF.
                match target_idx {
                    Some(idx) if Some(*idx) == index => !readable,
                    Some(_) => true,
                    None => true,
                }
            }
            None => true,
        }
    })
}

impl FfTableEntry {
    fn update_is_ff(
        &mut self,
        self_key: (VarId, usize),
        unsafe_reads: &UnsafeSelfReads,
        whole: &[RangeRefered],
    ) {
        if let Some(assigned_decl) = self.assigned {
            let readable = !unsafe_reads.contains(assigned_decl, self_key.0, self_key.1);
            self.is_ff = classify(
                assigned_decl,
                self.multi_assigned,
                self_key.0,
                Some(self_key.1),
                readable,
                self.refered.iter().chain(range_refs(whole, self_key.1)),
            );
        }
    }
}

fn range_refs(whole: &[RangeRefered], index: usize) -> impl Iterator<Item = &Refered> {
    whole
        .iter()
        .filter(move |(range, _)| range.contains(&index))
        .map(|(_, r)| r)
}

#[derive(Clone, Debug, Default)]
pub struct FfTable {
    pub table: HashMap<(VarId, usize), FfTableEntry>,
    /// Stored once per variable rather than in every element's entry: a big
    /// array read at several runtime indices would otherwise cost readers x
    /// elements entries.
    pub range_refered: HashMap<VarId, Vec<RangeRefered>>,
    /// The write side of `range_refered`.
    pub whole_assigned: HashMap<VarId, WholeAssigned>,
    /// The indices in `table` per variable, which a later write through an
    /// unevaluated index has to reach.
    indices: HashMap<VarId, Vec<usize>>,
    /// Record every access per element: the layout the compact form is
    /// checked against.
    per_element: bool,
}

impl FfTable {
    pub fn per_element() -> Self {
        Self {
            per_element: true,
            ..Default::default()
        }
    }

    /// `decls` must be the declarations this table was gathered from: what
    /// they write before a self-reference reads decides whether it needs the
    /// register.
    pub fn update_is_ff(&mut self, decls: &[Declaration], context: &mut Context) {
        let unsafe_reads = unsafe_self_reads(decls, context, self.per_element);
        for (key, entry) in self.table.iter_mut() {
            let whole = self
                .range_refered
                .get(&key.0)
                .map(Vec::as_slice)
                .unwrap_or_default();
            entry.update_is_ff(*key, &unsafe_reads, whole);
        }
    }

    /// Every reference to element `index` of `id`, including the ones through
    /// an unevaluated index.
    pub fn refered(&self, id: VarId, index: usize) -> impl Iterator<Item = &Refered> {
        let own = self
            .table
            .get(&(id, index))
            .map(|x| x.refered.as_slice())
            .unwrap_or_default();
        let whole = self
            .range_refered
            .get(&id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        own.iter().chain(range_refs(whole, index))
    }

    /// `(assigned, multi_assigned, assigned_comb)` of element `index` of `id`.
    pub fn writers(&self, id: VarId, index: usize) -> (Option<usize>, bool, Option<usize>) {
        if let Some(x) = self.table.get(&(id, index)) {
            (x.assigned, x.multi_assigned, x.assigned_comb)
        } else if let Some(w) = self.whole(id, index) {
            (w.assigned, w.multi_assigned, w.assigned_comb)
        } else {
            (None, false, None)
        }
    }

    pub fn has_ff_writer(&self, id: VarId) -> bool {
        self.whole_assigned
            .get(&id)
            .is_some_and(|w| w.assigned.is_some())
            || self.indices.get(&id).is_some_and(|indices| {
                indices
                    .iter()
                    .any(|i| self.table[&(id, *i)].assigned.is_some())
            })
    }

    fn whole(&self, id: VarId, index: usize) -> Option<&WholeAssigned> {
        self.whole_assigned.get(&id).filter(|w| index < w.len)
    }

    /// Force all always_ff-assigned variables to FF, disabling the
    /// assign_target refinement. Used by --disable-ff-opt for debugging.
    pub fn force_all_ff(&mut self) {
        for entry in self.table.values_mut() {
            if entry.assigned.is_some() {
                entry.is_ff = true;
            }
        }
        for w in self.whole_assigned.values_mut() {
            if w.assigned.is_some() {
                w.force_ff = true;
            }
        }
    }

    pub fn is_ff(&self, id: VarId, index: usize) -> bool {
        if let Some(x) = self.table.get(&(id, index)) {
            x.is_ff
        } else if let Some(w) = self.whole(id, index) {
            w.force_ff
                || w.assigned.is_some_and(|decl| {
                    classify(
                        decl,
                        w.multi_assigned,
                        id,
                        None,
                        true,
                        self.range_refered
                            .get(&id)
                            .into_iter()
                            .flatten()
                            .filter(|(range, _)| range.contains(&index))
                            .map(|(_, r)| r),
                    )
                })
        } else {
            false
        }
    }

    /// The entry of element `index`, created with what the writes through an
    /// unevaluated index have recorded for it so far.
    fn entry(&mut self, id: VarId, index: usize) -> &mut FfTableEntry {
        let Self {
            table,
            whole_assigned,
            indices,
            ..
        } = self;
        table.entry((id, index)).or_insert_with(|| {
            indices.entry(id).or_default().push(index);
            let w = whole_assigned.get(&id).filter(|w| index < w.len);
            FfTableEntry {
                assigned: w.and_then(|w| w.assigned),
                multi_assigned: w.is_some_and(|w| w.multi_assigned),
                refered: vec![],
                is_ff: false,
                assigned_comb: w.and_then(|w| w.assigned_comb),
            }
        })
    }

    pub fn insert_refered(
        &mut self,
        id: VarId,
        index: usize,
        decl: usize,
        assign_target: Option<AssignTarget>,
        src_read_mask: BigUint,
        from_ff: bool,
    ) {
        self.entry(id, index)
            .refered
            .push((decl, assign_target, src_read_mask, from_ff));
    }

    /// A reference to every element below `total_array` of `id`.
    pub fn insert_refered_whole(
        &mut self,
        id: VarId,
        total_array: usize,
        decl: usize,
        assign_target: Option<AssignTarget>,
        src_read_mask: BigUint,
        from_ff: bool,
    ) {
        self.insert_refered_range(
            id,
            0..total_array,
            decl,
            assign_target,
            src_read_mask,
            from_ff,
        );
    }

    pub fn insert_refered_range(
        &mut self,
        id: VarId,
        range: std::ops::Range<usize>,
        decl: usize,
        assign_target: Option<AssignTarget>,
        src_read_mask: BigUint,
        from_ff: bool,
    ) {
        if self.per_element {
            for i in range {
                self.insert_refered(
                    id,
                    i,
                    decl,
                    assign_target.clone(),
                    src_read_mask.clone(),
                    from_ff,
                );
            }
            return;
        }
        if range.is_empty() {
            return;
        }
        self.range_refered
            .entry(id)
            .or_default()
            .push((range, (decl, assign_target, src_read_mask, from_ff)));
    }

    pub fn insert_assigned(&mut self, id: VarId, index: usize, decl: usize) {
        let entry = self.entry(id, index);
        record_assign(&mut entry.assigned, &mut entry.multi_assigned, decl);
    }

    pub fn insert_assigned_comb(&mut self, id: VarId, index: usize, decl: usize) {
        self.entry(id, index).assigned_comb = Some(decl);
    }

    /// A write to every element below `total_array` of `id`.
    pub fn insert_assigned_whole(&mut self, id: VarId, total_array: usize, decl: usize) {
        if !self.whole_fits(id, total_array) {
            for i in 0..total_array {
                self.insert_assigned(id, i, decl);
            }
            return;
        }
        let w = self.whole_entry(id, total_array);
        record_assign(&mut w.assigned, &mut w.multi_assigned, decl);
        self.for_each_own(id, total_array, |x| {
            record_assign(&mut x.assigned, &mut x.multi_assigned, decl)
        });
    }

    /// The always_comb counterpart of [`Self::insert_assigned_whole`].
    pub fn insert_assigned_comb_whole(&mut self, id: VarId, total_array: usize, decl: usize) {
        if !self.whole_fits(id, total_array) {
            for i in 0..total_array {
                self.insert_assigned_comb(id, i, decl);
            }
            return;
        }
        self.whole_entry(id, total_array).assigned_comb = Some(decl);
        self.for_each_own(id, total_array, |x| x.assigned_comb = Some(decl));
    }

    fn whole_fits(&self, id: VarId, total_array: usize) -> bool {
        // Every whole access to a variable spans its whole array.
        debug_assert!(
            self.whole_assigned
                .get(&id)
                .is_none_or(|w| w.len == total_array)
        );
        total_array != 0 && !self.per_element
    }

    fn whole_entry(&mut self, id: VarId, len: usize) -> &mut WholeAssigned {
        self.whole_assigned
            .entry(id)
            .or_insert_with(|| WholeAssigned {
                len,
                ..Default::default()
            })
    }

    fn for_each_own(&mut self, id: VarId, len: usize, mut f: impl FnMut(&mut FfTableEntry)) {
        for i in self.indices.get(&id).into_iter().flatten() {
            if *i < len {
                f(self.table.get_mut(&(id, *i)).unwrap());
            }
        }
    }

    #[cfg(debug_assertions)]
    pub fn validate(&self) {
        for ((id, index), entry) in &self.table {
            if let (Some(ff_decl), Some(comb_decl)) = (entry.assigned, entry.assigned_comb) {
                log::warn!(
                    "FfTable: variable {:?}[{}] assigned in both always_ff (decl {}) and always_comb (decl {})",
                    id,
                    index,
                    ff_decl,
                    comb_decl
                );
            }
        }
        for (id, w) in &self.whole_assigned {
            if let (Some(ff_decl), Some(comb_decl)) = (w.assigned, w.assigned_comb) {
                log::warn!(
                    "FfTable: variable {:?}[..{}] assigned in both always_ff (decl {}) and always_comb (decl {})",
                    id,
                    w.len,
                    ff_decl,
                    comb_decl
                );
            }
        }
    }
}
