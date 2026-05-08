use crate::BigUint;
use crate::HashMap;
use crate::attribute::{AllowItem, Attribute};
use crate::attribute_table;
use crate::conv::Context;
use crate::ir::assign_table::{AssignTable, ReferencedEntry};
use crate::ir::{
    Declaration, FfTable, Function, Type, VarId, VarIndex, VarKind, VarPath, Variable,
};
use crate::symbol::ClockDomain;
use crate::value::ValueBigUint;
use indent::indent_all_by;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

/// Per-variable list of assignment tokens, used by the combinational-loop
/// detector to point error reports at the assign statements involved in a
/// cycle (rather than at the variable declaration).
pub type AssignTokens = HashMap<VarId, Vec<TokenRange>>;

#[derive(Clone)]
pub struct Module {
    pub name: StrId,
    pub token: TokenRange,
    pub ports: HashMap<VarPath, VarId>,
    pub port_types: HashMap<VarPath, (Type, ClockDomain)>,
    pub variables: HashMap<VarId, Variable>,
    pub functions: HashMap<VarId, Function>,
    pub declarations: Vec<Declaration>,
    pub suppress_unassigned: bool,
    /// Per-declaration `AssignTable.refernced` snapshot, captured at the end
    /// of each `Declaration::eval_assign` for use by the combinational-loop
    /// detector. Keyed by declaration index in `declarations`.
    pub per_decl_refs: HashMap<usize, HashMap<VarId, ReferencedEntry>>,
    /// Per-variable assignment-site tokens, captured from the merged
    /// `AssignTable` during `eval_assign`. Used by the combinational-loop
    /// detector to highlight assign statements in error reports.
    pub assign_tokens: AssignTokens,
    /// `FfTable` derived from this module's declarations (and updated with
    /// `update_is_ff`). Computed during `eval_assign` so consumers
    /// (combinational-loop detector, simulator, future analyses) can read
    /// it as a public IR property without re-walking `gather_ff`. Consumers
    /// that need to mutate (e.g. simulator's `force_all_ff` or post-hoist
    /// rebuild) should clone first.
    pub ff_table: FfTable,
}

impl Module {
    pub fn eval_assign(&mut self, context: &mut Context) {
        // FfTable is a derived property of the IR -- safe to (re)build
        // even when `suppress_unassigned` short-circuits the rest of
        // eval_assign. Consumers expect `ff_table` populated whenever
        // the module IR is fully available.
        self.rebuild_ff_table(context);

        if self.suppress_unassigned {
            return;
        }

        context.variables = self.variables.clone();
        context.functions = self.functions.clone();

        let mut assign_table = AssignTable::new(context);

        for (i, x) in self.declarations.iter().enumerate() {
            let mut new_table = AssignTable::new(context);
            x.eval_assign(context, &mut new_table);
            // Snapshot per-decl reference masks before `new_table` is dropped.
            // `Declaration::eval_assign` no longer clears `refernced`, so the
            // accumulated reads/writes are still in place here.
            let snapshot = std::mem::take(&mut new_table.refernced);
            self.per_decl_refs.insert(i, snapshot);
            assign_table.merge_by_or(context, &mut new_table, true);
        }

        for x in self.functions.values() {
            let mut new_table = AssignTable::new(context);
            x.eval_assign(context, &mut new_table);
            assign_table.merge_by_or(context, &mut new_table, false);
        }

        let mut variables = self.variables.clone();

        for (key, entry) in &assign_table.table {
            if let Some(variable) = variables.get_mut(key)
                && let Some(array) = entry.array.total()
            {
                for i in 0..array {
                    if let Some(x) = entry.mask.get(i) {
                        variable.set_assigned(i, x.clone());
                    }
                }
            }
            // Capture assign tokens for combinational loop reporting.
            self.assign_tokens.insert(*key, entry.tokens.clone());
        }

        for variable in variables.values() {
            // inout ports are driven externally; unknown-size arrays can't be iterated.
            let check_skip = variable.r#type.is_systemverilog()
                || variable.r#type.total_array().unwrap_or(0) > context.config.evaluate_array_limit
                || matches!(variable.kind, VarKind::Inout)
                || variable.r#type.array.total().is_none();

            if variable.is_assignable() && !check_skip {
                let zero: BigUint = 0u32.into();
                let full_mask = variable
                    .total_width()
                    .map(ValueBigUint::gen_mask)
                    .unwrap_or_else(|| zero.clone());
                let accumulated_reads = assign_table.accumulated_reads.get(&variable.id);

                for index in &variable.unassigned() {
                    let assigned_mask = variable
                        .assigned
                        .get(*index)
                        .cloned()
                        .unwrap_or_else(|| zero.clone());
                    // BigUint has no bit-complement: full_mask ^ (full_mask & assigned).
                    let unassigned_bits = &full_mask ^ &(&full_mask & &assigned_mask);
                    let read_mask = accumulated_reads
                        .and_then(|r| r.get(*index))
                        .cloned()
                        .unwrap_or_else(|| zero.clone());
                    // Accept dead bits of a partially driven register; still
                    // warn if nothing was assigned at all.
                    let any_assigned = assigned_mask != zero;
                    let any_read_unassigned = (&read_mask & &unassigned_bits) != zero;
                    if any_assigned && !any_read_unassigned {
                        continue;
                    }

                    if !attribute_table::contains(
                        &variable.token.beg,
                        Attribute::Allow(AllowItem::UnassignVariable),
                    ) {
                        let index = VarIndex::from_index(*index, &variable.r#type.array);
                        context.insert_error(crate::AnalyzerError::unassign_variable(
                            &format!("{}{index}", variable.path),
                            &variable.token,
                        ));
                    }
                }
            }
        }
    }

    pub fn gather_ff(&self, context: &mut Context, table: &mut FfTable) {
        for (i, x) in self.declarations.iter().enumerate() {
            x.gather_ff(context, table, i);
        }
    }

    /// (Re)compute `self.ff_table` from the current `declarations`.
    /// Runs `gather_ff` over every declaration and applies
    /// `update_is_ff` so the resulting table is ready for consumers.
    pub fn rebuild_ff_table(&mut self, context: &mut Context) {
        let saved_vars = std::mem::take(&mut context.variables);
        let saved_funcs = std::mem::take(&mut context.functions);
        context.variables = self.variables.clone();
        context.functions = self.functions.clone();

        let mut table = FfTable::default();
        self.gather_ff(context, &mut table);
        table.update_is_ff();
        self.ff_table = table;

        context.variables = saved_vars;
        context.functions = saved_funcs;
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("module {} {{\n", self.name);

        let mut variables: Vec<_> = self.variables.iter().collect();
        variables.sort_by(|a, b| a.0.cmp(b.0));

        let mut functions: Vec<_> = self.functions.iter().collect();
        functions.sort_by(|a, b| a.0.cmp(b.0));

        for (_, x) in variables {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }

        for (_, x) in functions {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('\n');

        for x in &self.declarations {
            let text = format!("{}\n", x);
            ret.push_str(&indent_all_by(2, text));
        }

        ret.push('}');
        ret.fmt(f)
    }
}
