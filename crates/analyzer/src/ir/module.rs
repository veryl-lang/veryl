use crate::HashMap;
use crate::ir::{Declaration, VarId, VarPath, Variable};
use indent::indent_all_by;
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone)]
pub struct Module {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub variables: HashMap<VarId, Variable>,
    pub declarations: Vec<Declaration>,
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("module {} {{\n", self.name);

        let mut variables: Vec<_> = self.variables.iter().collect();
        variables.sort_by(|a, b| a.0.cmp(b.0));

        for (_, x) in variables {
            // TODO type should be printed from TypedValue not Variable
            // because the type of Variable is the type of the right-hand side
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
