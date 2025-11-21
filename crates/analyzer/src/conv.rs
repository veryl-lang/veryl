mod checker;
pub mod declaration;
pub mod expression;
pub mod instance;
pub mod ir;
pub mod statement;
pub mod utils;
pub mod var;

use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::conv::instance::InstanceHistory;
use crate::ir::{Declaration, TypedValue, Value, VarId, VarPath, Variable};
use crate::symbol::Affiliation;
use veryl_parser::resource_table::StrId;

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, value: T) -> Self;
}

#[derive(Default)]
pub struct Context {
    pub var_id: VarId,
    pub var_paths: HashMap<VarPath, (VarId, TypedValue)>,
    pub variables: HashMap<VarId, Variable>,
    pub declarations: Vec<Declaration>,
    pub hierarchy: Vec<StrId>,
    pub hierarchical_variables: Vec<Vec<VarPath>>,
    pub default_clock: Option<VarPath>,
    pub default_reset: Option<VarPath>,
    pub affiliation: Vec<Affiliation>,
    pub overrides: HashMap<VarPath, Value>,
    pub instance_history: InstanceHistory,
    pub errors: Vec<AnalyzerError>,
}

impl Context {
    pub fn inherit(&mut self, tgt: &mut Context) {
        tgt.overrides = self.overrides.clone();
        tgt.instance_history = self.instance_history.clone();
        tgt.errors = self.errors.drain(..).collect();
    }

    pub fn insert_variable(
        &mut self,
        path: VarPath,
        value: TypedValue,
        mut variable: Variable,
    ) -> VarId {
        let id = self.var_id;
        let hier = &self.hierarchy;

        variable.id = id;
        if !hier.is_empty() {
            variable.path.add_prelude(hier);
        }
        if let Some(x) = self.hierarchical_variables.last_mut() {
            x.push(path.clone());
        }

        self.var_paths.insert(path, (id, value));
        self.variables.insert(id, variable);
        self.var_id.inc();
        id
    }

    pub fn insert_declaration(&mut self, decl: Declaration) {
        if !decl.is_null() {
            self.declarations.push(decl);
        }
    }

    pub fn insert_error(&mut self, error: AnalyzerError) {
        self.errors.push(error);
    }

    pub fn set_default_clock(&mut self, path: VarPath) {
        self.default_clock.replace(path);
    }

    pub fn set_default_reset(&mut self, path: VarPath) {
        self.default_reset.replace(path);
    }

    pub fn get_default_clock(&self) -> Option<VarId> {
        if let Some(x) = &self.default_clock {
            if let Some((x, _)) = &self.var_paths.get(x) {
                Some(*x)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn get_default_reset(&self) -> Option<VarId> {
        if let Some(x) = &self.default_reset {
            if let Some((x, _)) = &self.var_paths.get(x) {
                Some(*x)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn push_hier(&mut self, x: StrId) {
        self.hierarchy.push(x);
        self.hierarchical_variables.push(vec![]);
    }

    pub fn pop_hier(&mut self) {
        self.hierarchy.pop();
        let drops = self.hierarchical_variables.pop();
        if let Some(drops) = drops {
            for x in drops {
                self.var_paths.remove(&x);
            }
        }
    }

    pub fn is_affiliated(&self, value: Affiliation) -> bool {
        if let Some(x) = self.affiliation.last() {
            *x == value
        } else {
            false
        }
    }

    pub fn find_path(&self, path: &VarPath) -> Option<(VarId, TypedValue)> {
        self.var_paths.get(path).cloned()
    }

    pub fn get_variable(&self, id: &VarId) -> Option<Variable> {
        self.variables.get(id).cloned()
    }

    pub fn remove_path(&mut self, path: &VarPath) {
        self.var_paths.remove(path);
    }

    pub fn drain_variables(&mut self) -> HashMap<VarId, Variable> {
        self.variables.drain().collect()
    }

    pub fn drain_declarations(&mut self) -> Vec<Declaration> {
        self.declarations.drain(..).collect()
    }

    pub fn drain_errors(&mut self) -> Vec<AnalyzerError> {
        self.errors.drain(..).collect()
    }
}
