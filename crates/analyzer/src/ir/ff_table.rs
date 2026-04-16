use crate::HashMap;
use crate::ir::VarId;

/// Identifies the LHS of an assignment: (VarId, Option<array_element_index>).
/// None index means dynamic (unknown at analysis time).
type AssignTarget = (VarId, Option<usize>);

#[derive(Clone, Debug)]
pub struct FfTableEntry {
    pub assigned: Option<usize>,
    /// (decl_index, assign_target, from_ff) pairs where this variable is referenced.
    /// None assign_target for condition expressions (if/case).
    /// from_ff: true if the reference is in an always_ff block (NBA-sensitive),
    /// false if in always_comb / continuous assign (re-evaluated after NBA).
    pub refered: Vec<(usize, Option<AssignTarget>, bool)>,
    pub is_ff: bool,
    pub assigned_comb: Option<usize>,
}

impl FfTableEntry {
    fn update_is_ff(&mut self, self_key: (VarId, usize)) {
        if let Some(assigned_decl) = self.assigned {
            // FF classification rules (strict NBA semantics):
            // - A variable may be treated as comb (ff_opt) only if no always_ff
            //   block reads it (cross-block NBA races would be violated).
            // - always_comb / continuous assigns re-evaluate after NBA in SV,
            //   so they correctly see new FF values; ff_opt is safe for them.
            // - Within the same always_ff (assigned_decl), self-reference is
            //   safe; but cross-variable assignments must see old values.
            self.is_ff = self.refered.iter().any(|(decl, assign_target, from_ff)| {
                if !from_ff {
                    return false;
                }
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
        from_ff: bool,
    ) {
        self.table
            .entry((id, index))
            .and_modify(|x| x.refered.push((decl, assign_target, from_ff)))
            .or_insert(FfTableEntry {
                assigned: None,
                refered: vec![(decl, assign_target, from_ff)],
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
