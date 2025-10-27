use crate::HashSet;
use crate::conv::Context;
use crate::ir::utils::{calc_index, total_array};
use crate::ir::{VarId, VarPath, Variable};
use crate::symbol::Affiliation;
use crate::{AnalyzerError, HashMap};
use num_bigint::BigUint;
use std::borrow::Cow;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AssignContext {
    Ff,
    Comb,
    Function,
    SystemVerilog,
}

impl AssignContext {
    pub fn is_ff(&self) -> bool {
        self == &AssignContext::Ff
    }

    pub fn is_comb(&self) -> bool {
        self == &AssignContext::Comb
    }

    pub fn is_function(&self) -> bool {
        self == &AssignContext::Function
    }

    pub fn is_system_verilog(&self) -> bool {
        self == &AssignContext::SystemVerilog
    }
}

#[derive(Clone, Debug)]
pub struct AssignTableEntry {
    pub mask: Vec<BigUint>,
    pub path: VarPath,
    pub width: usize,
    pub array: Vec<usize>,
    pub affiliation: Affiliation,
    pub maybe: bool,
    pub tokens: Vec<TokenRange>,
}

impl AssignTableEntry {
    pub fn new(
        variable: &Variable,
        index: &[usize],
        mask: BigUint,
        maybe: bool,
        token: TokenRange,
    ) -> Self {
        let array = &variable.r#type.array;
        let mut masks = vec![];

        if let Some(index) = calc_index(index, array) {
            for i in 0..total_array(array) {
                if index == i {
                    masks.push(mask.clone());
                } else {
                    masks.push(0u32.into());
                }
            }
        }

        Self {
            mask: masks,
            path: variable.path.clone(),
            width: variable.total_width(),
            array: array.to_vec(),
            affiliation: variable.affiliation,
            maybe,
            tokens: vec![token],
        }
    }

    pub fn add(
        &mut self,
        index: &[usize],
        mask: &BigUint,
        maybe: bool,
        token: TokenRange,
    ) -> Option<Vec<TokenRange>> {
        let i = calc_index(index, &self.array).unwrap();

        let fail = &self.mask[i] & mask != 0u32.into();
        self.mask[i] |= mask;
        self.maybe |= maybe;
        self.tokens.push(token);

        if fail & !self.maybe {
            Some(self.tokens.clone())
        } else {
            None
        }
    }

    pub fn is_always(&self) -> bool {
        matches!(
            self.affiliation,
            Affiliation::AlwaysFf | Affiliation::AlwaysComb
        )
    }

    pub fn merge_by_or(&mut self, value: &AssignTableEntry) {
        for (i, val) in self.mask.iter_mut().enumerate() {
            *val |= &value.mask[i];
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReferencedEntry {
    pub mask_ref: Vec<BigUint>,
    pub mask_assign: Vec<BigUint>,
    pub array: Vec<usize>,
}

impl ReferencedEntry {
    pub fn new_ref(index: &[usize], array: &[usize], mask: &BigUint) -> Self {
        let mut mask_ref = vec![];
        let mut mask_assign = vec![];

        if let Some(index) = calc_index(index, array) {
            for i in 0..total_array(array) {
                if index == i {
                    mask_ref.push(mask.clone());
                } else {
                    mask_ref.push(0u32.into());
                }
                mask_assign.push(0u32.into());
            }
        }

        Self {
            mask_ref,
            mask_assign,
            array: array.to_vec(),
        }
    }

    pub fn new_assign(index: &[usize], array: &[usize], mask: &BigUint) -> Self {
        let mut mask_ref = vec![];
        let mut mask_assign = vec![];

        if let Some(index) = calc_index(index, array) {
            for i in 0..total_array(array) {
                mask_ref.push(0u32.into());
                if index == i {
                    mask_assign.push(mask.clone());
                } else {
                    mask_assign.push(0u32.into());
                }
            }
        }

        Self {
            mask_ref,
            mask_assign,
            array: array.to_vec(),
        }
    }

    pub fn add_ref(&mut self, index: &[usize], mask: &BigUint) {
        if let Some(index) = calc_index(index, &self.array)
            && let Some(x) = self.mask_ref.get_mut(index)
        {
            *x |= mask;
        }
    }

    pub fn add_assign(&mut self, index: &[usize], mask: &BigUint) {
        if let Some(index) = calc_index(index, &self.array)
            && let Some(x) = self.mask_assign.get_mut(index)
        {
            *x |= mask;
        }
    }

    pub fn merge_by_or(&mut self, x: &ReferencedEntry) {
        for i in 0..total_array(&self.array) {
            self.mask_ref[i] |= &x.mask_ref[i];
            self.mask_assign[i] |= &x.mask_assign[i];
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AssignTable {
    pub table: HashMap<VarId, AssignTableEntry>,
    pub refernced: HashMap<VarId, ReferencedEntry>,
}

impl AssignTable {
    pub fn insert_assign(
        &mut self,
        variable: &Variable,
        index: Vec<usize>,
        mask: BigUint,
        maybe: bool,
        token: TokenRange,
    ) -> (bool, Vec<TokenRange>) {
        let mut ret = true;
        let mut tokens = vec![];

        let array = &variable.r#type.array;

        // add assign bit
        self.refernced
            .entry(variable.id)
            .and_modify(|x| x.add_assign(&index, &mask))
            .or_insert(ReferencedEntry::new_assign(&index, array, &mask));

        self.table
            .entry(variable.id)
            .and_modify(|x| {
                if let Some(x) = x.add(&index, &mask, maybe, token) {
                    ret = false;
                    tokens = x;
                }
            })
            .or_insert(AssignTableEntry::new(variable, &index, mask, maybe, token));

        (ret, tokens)
    }

    pub fn insert_reference(&mut self, variable: &Variable, index: Vec<usize>, mask: BigUint) {
        let array = &variable.r#type.array;

        self.refernced
            .entry(variable.id)
            .and_modify(|x| x.add_ref(&index, &mask))
            .or_insert(ReferencedEntry::new_ref(&index, array, &mask));
    }

    pub fn merge_by_or(
        &mut self,
        context: &mut Context,
        value: &mut AssignTable,
        check_conflict: bool,
    ) {
        for (key, mut val) in value.table.drain() {
            if let Some(x) = self.table.get_mut(&key) {
                x.tokens.append(&mut val.tokens);

                for i in 0..total_array(&val.array) {
                    if (!x.maybe && !val.maybe)
                        && (&x.mask[i] & &val.mask[i] != 0u32.into())
                        && check_conflict
                    {
                        context.insert_error(AnalyzerError::multiple_assignment(
                            &x.path.to_string(),
                            &x.tokens[0],
                            &x.tokens,
                        ));
                    }

                    x.mask[i] |= &val.mask[i];
                }
            } else {
                self.table.insert(key, val);
            }
        }
    }

    pub fn check_refered(&self, id: &VarId, index: &[usize], mask: &BigUint) -> bool {
        if let Some(x) = self.refernced.get(id) {
            if let Some(i) = calc_index(index, &x.array) {
                ((&x.mask_ref[i] & mask) != 0u32.into())
                    && ((&x.mask_ref[i] & mask & &x.mask_assign[i]) == 0u32.into())
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn check_uncoverd(
        &self,
        context: &mut Context,
        tgt_table: &AssignTable,
        base_tables: &[&AssignTable],
    ) {
        let mut keys = HashSet::default();
        for key in self.table.keys() {
            keys.insert(key);
        }
        for key in tgt_table.table.keys() {
            keys.insert(key);
        }

        for key in &keys {
            let src_val = self.table.get(key);
            let tgt_val = tgt_table.table.get(key);

            let mut base_val: Option<AssignTableEntry> = None;
            for x in base_tables {
                if let Some(x) = x.table.get(key) {
                    if let Some(y) = &mut base_val {
                        y.merge_by_or(x);
                    } else {
                        base_val = Some(x.clone());
                    }
                }
            }

            let src_tgt = if let (Some(src_val), Some(tgt_val)) = (src_val, tgt_val) {
                Some((Cow::Borrowed(src_val), Cow::Borrowed(tgt_val)))
            } else if let Some(src_val) = src_val {
                let mut tgt_val = src_val.clone();
                for x in &mut tgt_val.mask {
                    *x = 0u32.into();
                }
                tgt_val.tokens.clear();
                Some((Cow::Borrowed(src_val), Cow::Owned(tgt_val)))
            } else if let Some(tgt_val) = tgt_val {
                let mut src_val = tgt_val.clone();
                for x in &mut src_val.mask {
                    *x = 0u32.into();
                }
                src_val.tokens.clear();
                Some((Cow::Owned(src_val), Cow::Borrowed(tgt_val)))
            } else {
                None
            };

            if let Some((src_val, tgt_val)) = src_tgt {
                if src_val.is_always() | tgt_val.is_always() {
                    continue;
                }

                let mut tokens = src_val.tokens.clone();
                tokens.append(&mut tgt_val.tokens.clone());

                for i in 0..total_array(&src_val.array) {
                    let (src, tgt) = if let Some(base_val) = &base_val {
                        (
                            &(&src_val.mask[i] | &base_val.mask[i]),
                            &(&tgt_val.mask[i] | &base_val.mask[i]),
                        )
                    } else {
                        (&src_val.mask[i], &tgt_val.mask[i])
                    };
                    if src ^ tgt != 0u32.into() {
                        context.insert_error(AnalyzerError::uncovered_branch(
                            &src_val.path.to_string(),
                            &tokens[0],
                            &tokens,
                        ));
                    }
                }
            }
        }
    }

    pub fn check_missing_reset(&self, context: &mut Context, false_side: &AssignTable) {
        for (key, tgt_val) in &false_side.table {
            if let Some(src_val) = self.table.get(key) {
                for i in 0..total_array(&src_val.array) {
                    if &src_val.mask[i] ^ &tgt_val.mask[i] != 0u32.into() {
                        context.insert_error(AnalyzerError::missing_reset_statement(
                            &src_val.path.to_string(),
                            &src_val.tokens[0],
                            &src_val.tokens,
                        ));
                    }
                }
            } else {
                context.insert_error(AnalyzerError::missing_reset_statement(
                    &tgt_val.path.to_string(),
                    &tgt_val.tokens[0],
                    &tgt_val.tokens,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Type, TypeKind, VarKind};
    use crate::value::gen_mask_range;

    #[test]
    fn insert() {
        let mut table = AssignTable::default();
        let r#type = Type::new(TypeKind::Unknown, vec![], vec![], false);
        let variable = Variable::new(
            VarId::default(),
            VarPath::default(),
            VarKind::Variable,
            r#type,
            vec![],
            Affiliation::Module,
            &TokenRange::default(),
        );

        let mask = gen_mask_range(10, 1);
        let ret = table.insert_assign(&variable, vec![], mask, false, TokenRange::default());
        assert_eq!(ret.0, true);

        let mask = gen_mask_range(20, 11);
        let ret = table.insert_assign(&variable, vec![], mask, false, TokenRange::default());
        assert_eq!(ret.0, true);

        let mask = gen_mask_range(14, 8);
        let ret = table.insert_assign(&variable, vec![], mask, false, TokenRange::default());
        assert_eq!(ret.0, false);
    }
}
