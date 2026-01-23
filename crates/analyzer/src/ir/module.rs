use crate::HashMap;
use crate::attribute::{AllowItem, Attribute};
use crate::attribute_table;
use crate::conv::Context;
use crate::ir::assign_table::AssignTable;
use crate::ir::{Declaration, Function, Type, VarId, VarIndex, VarPath, Variable};
use crate::symbol::ClockDomain;
use indent::indent_all_by;
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone)]
pub struct Module {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub port_types: HashMap<VarPath, (Type, ClockDomain)>,
    pub variables: HashMap<VarId, Variable>,
    pub functions: HashMap<VarId, Function>,
    pub declarations: Vec<Declaration>,
    pub suppress_unassigned: bool,
}

impl Module {
    pub fn eval_assign(&self, context: &mut Context) {
        if self.suppress_unassigned {
            return;
        }

        context.variables = self.variables.clone();
        context.functions = self.functions.clone();

        let mut assign_table = AssignTable::new(context);

        for x in &self.declarations {
            let mut new_table = AssignTable::new(context);
            x.eval_assign(context, &mut new_table);
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
        }

        for variable in variables.values() {
            // skip to check systemverilog type or large array
            let check_skip = variable.r#type.is_systemverilog()
                || variable.r#type.total_array().unwrap_or(0) > context.config.evaluate_array_limit;

            if variable.is_assignable() && !check_skip {
                for index in &variable.unassigned() {
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
