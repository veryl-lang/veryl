use crate::HashMap;
use crate::ir::VarId;

#[derive(Clone, Debug)]
pub struct FfTableEntry {
    assigned: Option<usize>,
    refered: Vec<usize>,
    is_ff: bool,
}

impl FfTableEntry {
    fn update_is_ff(&mut self) {
        if let Some(assigned) = self.assigned {
            // If assigned is only member of refered, it is not necessary to treat as FF.
            // For example, if `a` in the following code is not refered in other always_ff,
            // it can be treat as non-FF value.
            //
            // always_ff {
            //   a += 1;
            // }
            self.is_ff = self.refered.iter().any(|x| *x != assigned);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FfTable {
    table: HashMap<(VarId, usize), FfTableEntry>,
}

impl FfTable {
    pub fn update_is_ff(&mut self) {
        for x in self.table.values_mut() {
            x.update_is_ff()
        }
    }

    pub fn is_ff(&self, id: VarId, index: usize) -> bool {
        if let Some(x) = self.table.get(&(id, index)) {
            x.is_ff
        } else {
            false
        }
    }

    pub fn insert_refered(&mut self, id: VarId, index: usize, decl: usize) {
        self.table
            .entry((id, index))
            .and_modify(|x| x.refered.push(decl))
            .or_insert(FfTableEntry {
                assigned: None,
                refered: vec![decl],
                is_ff: false,
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
            });
    }
}
