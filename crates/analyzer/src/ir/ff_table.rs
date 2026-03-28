use crate::HashMap;
use crate::ir::VarId;

/// Identifies the LHS of an assignment: (VarId, Option<array_element_index>).
/// None index means dynamic (unknown at analysis time).
type AssignTarget = (VarId, Option<usize>);

#[derive(Clone, Debug)]
pub struct FfTableEntry {
    pub assigned: Option<usize>,
    /// (decl_index, assign_target) pairs where this variable is referenced.
    /// None assign_target for condition expressions (if/case).
    pub refered: Vec<(usize, Option<AssignTarget>)>,
    pub is_ff: bool,
    pub assigned_comb: Option<usize>,
}

impl FfTableEntry {
    fn update_is_ff(&mut self, self_key: (VarId, usize)) {
        if let Some(assigned_decl) = self.assigned {
            // FF if referenced in a different decl (traditional check) OR
            // referenced by a different variable's assignment within the same
            // decl (NBA: cross-variable reads must see old values).
            // Self-referencing within the same decl (e.g. `a = a + 1` alone)
            // can remain comb because no other variable observes the old value.
            //
            // assign_target=None means the reference is in a condition expression
            // (if/case) which indirectly affects all assignments in the block,
            // so it must be treated as a cross-reference.
            self.is_ff = self.refered.iter().any(|(decl, assign_target)| {
                if *decl != assigned_decl {
                    return true;
                }
                match assign_target {
                    Some((target_id, target_idx)) => {
                        if *target_id != self_key.0 {
                            return true;
                        }
                        // Same VarId: compare array index.
                        // None index (dynamic) is conservative → FF.
                        match target_idx {
                            Some(idx) => *idx != self_key.1,
                            None => true,
                        }
                    }
                    None => true,
                }
            });
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FfTable {
    pub table: HashMap<(VarId, usize), FfTableEntry>,
}

impl FfTable {
    pub fn update_is_ff(&mut self) {
        let keys: Vec<_> = self.table.keys().cloned().collect();
        for key in keys {
            self.table.get_mut(&key).unwrap().update_is_ff(key);
        }
    }

    /// Force all always_ff-assigned variables to FF, disabling the
    /// assign_target refinement. Used by --disable-ff-opt for debugging.
    pub fn force_all_ff(&mut self) {
        for entry in self.table.values_mut() {
            if entry.assigned.is_some() {
                entry.is_ff = true;
            }
        }
    }

    pub fn is_ff(&self, id: VarId, index: usize) -> bool {
        if let Some(x) = self.table.get(&(id, index)) {
            x.is_ff
        } else {
            false
        }
    }

    pub fn insert_refered(
        &mut self,
        id: VarId,
        index: usize,
        decl: usize,
        assign_target: Option<AssignTarget>,
    ) {
        self.table
            .entry((id, index))
            .and_modify(|x| x.refered.push((decl, assign_target)))
            .or_insert(FfTableEntry {
                assigned: None,
                refered: vec![(decl, assign_target)],
                is_ff: false,
                assigned_comb: None,
            });
    }

    pub fn insert_assigned(&mut self, id: VarId, index: usize, decl: usize) {
        self.table
            .entry((id, index))
            .and_modify(|x| x.assigned = Some(decl))
            .or_insert(FfTableEntry {
                assigned: Some(decl),
                refered: vec![],
                is_ff: false,
                assigned_comb: None,
            });
    }

    pub fn insert_assigned_comb(&mut self, id: VarId, index: usize, decl: usize) {
        self.table
            .entry((id, index))
            .and_modify(|x| x.assigned_comb = Some(decl))
            .or_insert(FfTableEntry {
                assigned: None,
                refered: vec![],
                is_ff: false,
                assigned_comb: Some(decl),
            });
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
    }
}
