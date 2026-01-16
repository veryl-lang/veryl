use crate::HashMap;
use crate::ir::{Comptime, FuncPath, FuncProto, Function, VarId, VarPath, Variable};
use crate::symbol::Direction;
use indent::indent_all_by;
use std::fmt;
use veryl_parser::resource_table::StrId;

#[derive(Clone)]
pub struct Interface {
    pub name: StrId,
    pub var_paths: HashMap<VarPath, (VarId, Comptime)>,
    pub func_paths: HashMap<FuncPath, FuncProto>,
    pub variables: HashMap<VarId, Variable>,
    pub functions: HashMap<VarId, Function>,
    pub modports: HashMap<StrId, Vec<(StrId, Direction)>>,
}

impl Interface {
    pub fn get_modport(&self, name: &StrId) -> HashMap<StrId, Direction> {
        let mut ret = HashMap::default();
        if let Some(x) = self.modports.get(name) {
            for x in x {
                ret.insert(x.0, x.1);
            }
        }
        ret
    }
}

impl fmt::Display for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = format!("interface {} {{\n", self.name);

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

        ret.push('}');
        ret.fmt(f)
    }
}
