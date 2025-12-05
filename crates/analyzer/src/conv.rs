mod checker;
pub mod declaration;
pub mod expression;
pub mod instance;
pub mod ir;
pub mod statement;
pub mod system_function;
pub mod utils;
pub mod var;

use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::conv::instance::InstanceHistory;
use crate::ir::{Declaration, Function, TypedValue, ValueVariant, VarId, VarPath, Variable};
use crate::symbol::Affiliation;
use crate::symbol_path::GenericSymbolPath;
use veryl_parser::resource_table::StrId;

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, value: T) -> Self;
}

#[derive(Default)]
pub struct Context {
    pub var_id: VarId,
    pub var_paths: HashMap<VarPath, (VarId, TypedValue)>,
    pub func_paths: HashMap<VarPath, (VarId, Option<TypedValue>)>,
    pub variables: HashMap<VarId, Variable>,
    pub functions: HashMap<VarId, Function>,
    pub declarations: Vec<Declaration>,
    pub hierarchy: Vec<StrId>,
    pub hierarchical_variables: Vec<Vec<VarPath>>,
    pub default_clock: Option<VarPath>,
    pub default_reset: Option<VarPath>,
    pub affiliation: Vec<Affiliation>,
    pub overrides: Vec<HashMap<VarPath, ValueVariant>>,
    pub generic_arguments: Vec<HashMap<StrId, GenericSymbolPath>>,
    pub instance_history: InstanceHistory,
    pub select_paths: Vec<VarPath>,
    pub select_dims: Vec<usize>,
    pub errors: Vec<AnalyzerError>,
}

impl Context {
    pub fn inherit(&mut self, tgt: &mut Context) {
        std::mem::swap(&mut self.overrides, &mut tgt.overrides);
        std::mem::swap(&mut self.generic_arguments, &mut tgt.generic_arguments);
        std::mem::swap(&mut self.instance_history, &mut tgt.instance_history);
        std::mem::swap(&mut self.errors, &mut tgt.errors);
    }

    pub fn get_override(&self, x: &VarPath) -> Option<&ValueVariant> {
        let overrides = self.overrides.last()?;
        overrides.get(x)
    }

    pub fn get_generic_argument(&self, x: &StrId) -> Option<&GenericSymbolPath> {
        let generic_arguments = self.generic_arguments.last()?;
        generic_arguments.get(x)
    }

    pub fn insert_var_path(&mut self, path: VarPath, value: TypedValue) -> VarId {
        let id = self.var_id;

        if let Some(x) = self.hierarchical_variables.last_mut() {
            x.push(path.clone());
        }

        self.var_paths.insert(path, (id, value));
        self.var_id.inc();
        id
    }

    pub fn insert_func_path(&mut self, path: VarPath, value: Option<TypedValue>) -> VarId {
        let id = self.var_id;

        if let Some(x) = self.hierarchical_variables.last_mut() {
            x.push(path.clone());
        }

        self.func_paths.insert(path, (id, value));
        self.var_id.inc();
        id
    }

    pub fn insert_variable(&mut self, id: VarId, mut variable: Variable) {
        let hier = &self.hierarchy;
        if !hier.is_empty() {
            variable.path.add_prelude(hier);
        }
        self.variables.insert(id, variable);
    }

    pub fn insert_function(&mut self, id: VarId, mut function: Function) {
        let hier = &self.hierarchy;
        if !hier.is_empty() {
            function.path.add_prelude(hier);
        }
        self.functions.insert(id, function);
    }

    pub fn insert_declaration(&mut self, decl: Declaration) {
        if !decl.is_null() {
            self.declarations.push(decl);
        }
    }

    pub fn insert_error(&mut self, mut error: AnalyzerError) {
        let mut replaced = false;

        // merge MultipleAssignment which have same identifier
        if let AnalyzerError::MultipleAssignment {
            identifier: ref new_ident,
            error_locations: ref mut new_locations,
            ..
        } = error
        {
            for e in &mut self.errors {
                if let AnalyzerError::MultipleAssignment {
                    identifier: org_ident,
                    error_locations: org_locations,
                    ..
                } = e
                    && new_ident == org_ident
                {
                    org_locations.append(new_locations);
                    org_locations.sort();
                    org_locations.dedup();
                    replaced = true;
                    break;
                }
            }
        }

        if !replaced {
            self.errors.push(error);
        }
    }

    pub fn inc_select_dim(&mut self) {
        if let Some(x) = self.select_dims.last_mut() {
            *x += 1;
        }
    }

    pub fn get_select_dim(&self) -> Option<usize> {
        self.select_dims.last().copied()
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
                self.func_paths.remove(&x);
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

    pub fn get_affiliation(&self) -> Affiliation {
        self.affiliation.last().copied().unwrap()
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

    pub fn drain_var_paths(&mut self) -> HashMap<VarPath, (VarId, TypedValue)> {
        self.var_paths.drain().collect()
    }

    pub fn drain_func_paths(&mut self) -> HashMap<VarPath, (VarId, Option<TypedValue>)> {
        self.func_paths.drain().collect()
    }

    pub fn drain_variables(&mut self) -> HashMap<VarId, Variable> {
        self.variables.drain().collect()
    }

    pub fn drain_functions(&mut self) -> HashMap<VarId, Function> {
        self.functions.drain().collect()
    }

    pub fn drain_declarations(&mut self) -> Vec<Declaration> {
        self.declarations.drain(..).collect()
    }

    pub fn drain_errors(&mut self) -> Vec<AnalyzerError> {
        self.errors.drain(..).collect()
    }
}
