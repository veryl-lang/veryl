use crate::HashMap;
use crate::conv::Context;
use crate::ir::assign_table::AssignTable;
use crate::ir::{Declaration, Function, Type, VarId, VarPath, Variable};
use indent::indent_all_by;
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone)]
pub struct Module {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub port_types: HashMap<VarPath, Type>,
    pub variables: HashMap<VarId, Variable>,
    pub functions: HashMap<VarId, Function>,
    pub declarations: Vec<Declaration>,
}

impl Module {
    pub fn eval_assign(&self, context: &mut Context) {
        context.variables = self.variables.clone();
        context.functions = self.functions.clone();

        let mut assign_table = AssignTable::default();

        for x in &self.declarations {
            let mut new_table = AssignTable::default();
            x.eval_assign(context, &mut new_table);
            assign_table.merge_by_or(context, &mut new_table, true);
        }

        for x in self.functions.values() {
            let mut new_table = AssignTable::default();
            x.eval_assign(context, &mut new_table);
            assign_table.merge_by_or(context, &mut new_table, true);
        }

        let mut variables = self.variables.clone();

        for (key, entry) in &assign_table.table {
            if let Some(variable) = variables.get_mut(&key.0) {
                variable.set_assigned(&key.1, entry.mask.clone());
            }
        }

        for variable in variables.values() {
            if variable.is_assignable() {
                for _index in &variable.unassigned() {
                    if !assign_table.maybe_assigned(&variable.path) {
                        //context.insert_error(crate::AnalyzerError::unassign_variable(
                        //    &variable.path.to_string(),
                        //    &variable.token,
                        //));
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
            // TODO type should be printed from TypedValue not Variable
            // because the type of Variable is the type of the right-hand side
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
