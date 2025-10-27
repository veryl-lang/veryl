use crate::HashSet;
use crate::conv::Context;
use crate::ir::bigint::gen_mask;
use crate::ir::{VarId, VarPath, Variable};
use crate::symbol::Affiliation;
use crate::{AnalyzerError, HashMap};
use num_bigint::BigUint;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug, Default)]
pub struct AssignTable {
    pub table: HashMap<(VarId, Vec<usize>), AssignTableEntry>,
}

#[derive(Clone, Debug)]
pub struct AssignTableEntry {
    pub mask: BigUint,
    pub path: VarPath,
    pub width: usize,
    pub affiliation: Affiliation,
    pub tokens: Vec<TokenRange>,
}

impl AssignTableEntry {
    pub fn is_always(&self) -> bool {
        matches!(
            self.affiliation,
            Affiliation::AlwaysFf | Affiliation::AlwaysComb
        )
    }
}

impl AssignTable {
    pub fn insert(
        &mut self,
        variable: &Variable,
        index: Vec<usize>,
        mask: BigUint,
        token: TokenRange,
    ) -> (bool, Vec<TokenRange>) {
        let mut ret = true;
        let mut tokens = vec![];

        self.table
            .entry((variable.id, index))
            .and_modify(|x| {
                if &x.mask & &mask != 0u32.into() {
                    ret = false;
                }
                x.mask |= &mask;
                x.tokens.push(token);
                tokens = x.tokens.clone();
            })
            .or_insert(AssignTableEntry {
                mask,
                path: variable.path.clone(),
                width: variable.total_width(),
                affiliation: variable.affiliation,
                tokens: vec![token],
            });

        (ret, tokens)
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

                if x.mask.clone() & val.mask.clone() != 0u32.into() && check_conflict {
                    context.insert_error(AnalyzerError::multiple_assignment(
                        &x.path.to_string(),
                        &x.tokens[0],
                        &x.tokens,
                    ));
                }

                x.mask |= val.mask;
            } else {
                self.table.insert(key, val);
            }
        }
    }

    pub fn merge_by_and(&mut self, _context: &mut Context, value: &mut AssignTable) {
        for (key, mut val) in value.table.drain() {
            if let Some(x) = self.table.get_mut(&key) {
                x.mask &= val.mask;
                x.tokens.append(&mut val.tokens);
            } else {
                self.table.insert(key, val);
            }
        }
    }

    pub fn check_uncoverd(&self, context: &mut Context, value: &AssignTable) {
        let mut keys = HashSet::default();
        for key in self.table.keys() {
            keys.insert(key);
        }
        for key in value.table.keys() {
            keys.insert(key);
        }

        for key in &keys {
            let src_val = self.table.get(key);
            let tgt_val = value.table.get(key);

            if let (Some(src_val), Some(tgt_val)) = (src_val, tgt_val) {
                if src_val.is_always() | tgt_val.is_always() {
                    continue;
                }

                let mut tokens = src_val.tokens.clone();
                tokens.append(&mut tgt_val.tokens.clone());

                if src_val.mask.clone() & tgt_val.mask.clone() != gen_mask(src_val.width) {
                    context.insert_error(AnalyzerError::uncovered_branch(
                        &src_val.path.to_string(),
                        &tokens[0],
                        &tokens,
                    ));
                }
            } else if let Some(src_val) = src_val {
                if src_val.is_always() {
                    continue;
                }

                context.insert_error(AnalyzerError::uncovered_branch(
                    &src_val.path.to_string(),
                    &src_val.tokens[0],
                    &src_val.tokens,
                ));
            } else if let Some(tgt_val) = tgt_val {
                if tgt_val.is_always() {
                    continue;
                }

                context.insert_error(AnalyzerError::uncovered_branch(
                    &tgt_val.path.to_string(),
                    &tgt_val.tokens[0],
                    &tgt_val.tokens,
                ));
            }
        }
    }

    pub fn check_missing_reset(&self, context: &mut Context, false_side: &AssignTable) {
        for (key, tgt_val) in &false_side.table {
            if let Some(src_val) = self.table.get(key) {
                if src_val.mask.clone() & tgt_val.mask.clone() != gen_mask(src_val.width) {
                    context.insert_error(AnalyzerError::missing_reset_statement(
                        &src_val.path.to_string(),
                        &src_val.tokens[0],
                        &src_val.tokens,
                    ));
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
    use crate::ir::bigint::gen_mask_range;
    use crate::ir::{Type, TypeKind, VarKind};

    #[test]
    fn insert() {
        let mut table = AssignTable::default();
        let r#type = Type::new(TypeKind::Unknown, vec![], false);
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
        let ret = table.insert(&variable, vec![], mask, TokenRange::default());
        assert_eq!(ret.0, true);

        let mask = gen_mask_range(20, 11);
        let ret = table.insert(&variable, vec![], mask, TokenRange::default());
        assert_eq!(ret.0, true);

        let mask = gen_mask_range(14, 8);
        let ret = table.insert(&variable, vec![], mask, TokenRange::default());
        assert_eq!(ret.0, false);
    }
}
