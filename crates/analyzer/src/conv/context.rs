use crate::analyzer_error::{AnalyzerError, ExceedLimitKind};
use crate::conv::instance::{InstanceHistory, InstanceHistoryError};
use crate::ir::{
    Component, Comptime, Declaration, Expression, FfClock, FfReset, FuncArg, FuncPath, FuncProto,
    Function, Interface, IrResult, ShapeRef, Signature, Type, VarId, VarIndex, VarKind, VarPath,
    VarSelect, Variable, VariableInfo,
};
use crate::namespace_table;
use crate::symbol::{Affiliation, ClockDomain, Direction, GenericMap, SymbolId};
use crate::symbol_path::GenericSymbolPath;
use crate::{HashMap, HashSet};
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone)]
pub struct Config {
    pub use_ir: bool,
    pub instance_depth_limit: usize,
    pub instance_total_limit: usize,
    pub evaluate_size_limit: usize,
    pub evaluate_array_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            use_ir: false,
            instance_depth_limit: 1024,
            instance_total_limit: 1024 * 1024,
            evaluate_size_limit: 1024 * 1024,
            evaluate_array_limit: 128,
        }
    }
}

#[derive(Default)]
pub struct Context {
    pub config: Config,
    pub var_id: VarId,
    pub var_paths: HashMap<VarPath, (VarId, Comptime)>,
    pub func_paths: HashMap<FuncPath, FuncProto>,
    pub variables: HashMap<VarId, Variable>,
    pub functions: HashMap<VarId, Function>,
    pub port_types: HashMap<VarPath, (Type, ClockDomain)>,
    pub modports: HashMap<StrId, Vec<(StrId, Direction)>>,
    pub declarations: Vec<Declaration>,
    pub default_clock: Option<(VarPath, SymbolId)>,
    pub default_reset: Option<(VarPath, SymbolId)>,
    pub instance_history: InstanceHistory,
    pub select_paths: Vec<(VarPath, GenericSymbolPath)>,
    pub select_dims: Vec<usize>,
    pub ignore_var_func: bool,
    pub in_if_reset: bool,
    pub current_clock: Option<Comptime>,
    hierarchy: Vec<StrId>,
    hierarchical_variables: Vec<Vec<VarPath>>,
    hierarchical_functions: Vec<Vec<FuncPath>>,
    shadowed_variables: HashMap<VarPath, Vec<(VarId, Comptime)>>,
    affiliation: Vec<Affiliation>,
    overrides: Vec<HashMap<VarPath, (Comptime, Expression)>>,
    generic_maps: Vec<Vec<GenericMap>>,
    errors: Vec<AnalyzerError>,
}

impl Context {
    pub fn inherit(&mut self, tgt: &mut Context) {
        std::mem::swap(&mut self.overrides, &mut tgt.overrides);
        std::mem::swap(&mut self.generic_maps, &mut tgt.generic_maps);
        std::mem::swap(&mut self.instance_history, &mut tgt.instance_history);
        std::mem::swap(&mut self.errors, &mut tgt.errors);
        self.config = tgt.config.clone();
    }

    pub fn get_override(&self, x: &VarPath) -> Option<&(Comptime, Expression)> {
        let overrides = self.overrides.last()?;
        overrides.get(x)
    }

    pub fn get_variable_info(&self, id: VarId) -> Option<VariableInfo> {
        self.variables.get(&id).map(VariableInfo::new)
    }

    pub fn resolve_path(&self, mut path: GenericSymbolPath) -> GenericSymbolPath {
        let Some(namespace) = namespace_table::get(path.paths[0].base.id) else {
            return path;
        };
        path.resolve_imported(&namespace, None);
        for map in self.generic_maps.iter().rev() {
            path.apply_map(map);
        }
        path
    }

    pub fn insert_var_path(&mut self, path: VarPath, value: Comptime) -> VarId {
        let id = self.var_id;
        self.insert_var_path_with_id(path, id, value);
        self.var_id.inc();
        id
    }

    pub fn insert_var_path_with_id(&mut self, path: VarPath, id: VarId, value: Comptime) {
        if let Some(x) = self.hierarchical_variables.last_mut() {
            x.push(path.clone());
        }

        let shadowed = self.var_paths.insert(path.clone(), (id, value));

        // store variable shadowed by inner-scope variable
        if let Some(x) = shadowed {
            self.shadowed_variables
                .entry(path)
                .and_modify(|v| v.push(x.clone()))
                .or_insert(vec![x]);
        }
    }

    pub fn insert_func_path(
        &mut self,
        name: StrId,
        path: FuncPath,
        ret: Option<Comptime>,
        arity: usize,
        args: Vec<FuncArg>,
        token: TokenRange,
    ) -> VarId {
        let id = self.var_id;

        if let Some(x) = self.hierarchical_functions.last_mut() {
            x.push(path.clone());
        }

        let proto = FuncProto {
            name,
            id,
            ret,
            arity,
            args,
            token,
        };
        self.func_paths.insert(path, proto);
        self.var_id.inc();
        id
    }

    pub fn insert_func_args(&mut self, path: &FuncPath, args: Vec<FuncArg>) {
        if let Some(x) = self.func_paths.get_mut(path) {
            x.args = args;
        }
    }

    pub fn insert_variable(&mut self, id: VarId, mut variable: Variable) {
        if self.ignore_var_func {
            return;
        }

        let hier = &self.hierarchy;
        if !hier.is_empty() {
            variable.path.add_prelude(hier);
        }
        self.variables.insert(id, variable);
    }

    pub fn insert_port_type(&mut self, path: VarPath, r#type: Type, clock_domain: ClockDomain) {
        self.port_types.insert(path, (r#type, clock_domain));
    }

    pub fn insert_function(&mut self, id: VarId, mut function: Function) {
        if self.ignore_var_func {
            return;
        }

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

        if !replaced && !self.errors.contains(&error) {
            self.errors.push(error);
        }
    }

    pub fn insert_ir_error<T>(&mut self, error: &IrResult<T>) {
        if let Err(error) = error
            && self.config.use_ir
        {
            self.insert_error(AnalyzerError::unsupported_by_ir(&error.code, &error.token));
        }
    }

    pub fn insert_modport(&mut self, name: StrId, members: Vec<(StrId, Direction)>) {
        self.modports.insert(name, members);
    }

    pub fn extract_function(&mut self, context: &mut Context, base: &VarPath, array: &ShapeRef) {
        for (id, mut variable) in context.variables.drain() {
            variable.path.add_prelude(&base.0);
            variable.prepend_array(array);
            self.variables.insert(id, variable);
        }

        for (mut path, proto) in context.func_paths.drain() {
            path.add_prelude(&base.0);
            self.func_paths.insert(path, proto);
        }

        for (id, mut function) in context.functions.drain() {
            function.path.add_prelude(&base.0);

            if !array.is_empty() {
                let total_array = array.total();
                let func_body = function.functions.remove(0);
                if let Some(total_array) = total_array {
                    for i in 0..total_array {
                        let var_index = VarIndex::from_index(i, array);
                        let mut func_body = func_body.clone();
                        func_body.set_index(&var_index);
                        function.functions.push(func_body);
                    }
                }
            }

            self.functions.insert(id, function);
        }
    }

    pub fn extract_var_paths(&mut self, context: &Context, base: &VarPath, array: &ShapeRef) {
        for (path, (id, comptime)) in &context.var_paths {
            if path.starts_with(&base.0) {
                let mut path = path.clone();
                path.remove_prelude(&base.0);
                if !path.0.is_empty() {
                    let mut comptime = comptime.clone();
                    for _ in 0..array.dims() {
                        comptime.r#type.array.remove(0);
                    }
                    self.var_paths.insert(path, (*id, comptime));
                }
            }
        }
    }

    pub fn extract_interface_member(
        &mut self,
        base: StrId,
        array: &ShapeRef,
        component: Interface,
        modport: Option<StrId>,
        clock_domain: ClockDomain,
        token: TokenRange,
    ) {
        let mut inserted = HashSet::default();

        let modport_members = if let Some(x) = &modport {
            component.get_modport(x)
        } else {
            HashMap::default()
        };

        let mut id_map = HashMap::default();
        for mut variable in component.variables.into_values() {
            if modport.is_some() {
                if let Some(x) = modport_members.get(&variable.path.first()) {
                    variable.kind = match x {
                        Direction::Input => VarKind::Input,
                        Direction::Output => VarKind::Output,
                        Direction::Inout => VarKind::Inout,
                        _ => variable.kind,
                    };
                } else {
                    // Skip non-modport member
                    continue;
                }
            }

            variable.prepend_array(array);

            // override token, affiliation to interface instance
            variable.token = token;
            variable.affiliation = self.get_affiliation();

            inserted.insert(variable.path.clone());
            let comptime = Comptime::from_type(variable.r#type.clone(), clock_domain, token);
            variable.path.add_prelude(&[base]);
            let id = self.insert_var_path(variable.path.clone(), comptime);

            // id mapping for struct/union members
            id_map.insert(variable.id, id);

            variable.id = id;
            self.insert_variable(id, variable);
        }

        // import non-variable VarPath
        for (mut path, (id, mut comptime)) in component.var_paths {
            if !inserted.contains(&path) {
                path.add_prelude(&[base]);
                comptime.r#type.prepend_array(array);

                if let Some(id) = id_map.get(&id) {
                    self.insert_var_path_with_id(path, *id, comptime);
                } else {
                    self.insert_var_path(path, comptime);
                }
            }
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

    pub fn set_default_clock(&mut self, path: VarPath, id: SymbolId) {
        self.default_clock.replace((path, id));
    }

    pub fn set_default_reset(&mut self, path: VarPath, id: SymbolId) {
        self.default_reset.replace((path, id));
    }

    pub fn get_default_clock(&self) -> Option<(FfClock, SymbolId)> {
        if let Some(x) = &self.default_clock {
            if let Some((id, comptime)) = &self.var_paths.get(&x.0) {
                let ret = FfClock {
                    id: *id,
                    index: VarIndex::default(),
                    select: VarSelect::default(),
                    comptime: comptime.clone(),
                };
                Some((ret, x.1))
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn get_default_reset(&self) -> Option<(FfReset, SymbolId)> {
        if let Some(x) = &self.default_reset {
            if let Some((id, comptime)) = &self.var_paths.get(&x.0) {
                let ret = FfReset {
                    id: *id,
                    index: VarIndex::default(),
                    select: VarSelect::default(),
                    comptime: comptime.clone(),
                };
                Some((ret, x.1))
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn check_size(&mut self, x: usize, token: TokenRange) -> Option<usize> {
        if x > self.config.evaluate_size_limit {
            if self.config.use_ir {
                self.insert_error(AnalyzerError::exceed_limit(
                    ExceedLimitKind::EvaluateSize,
                    x,
                    &token,
                ));
            }
            None
        } else {
            Some(x)
        }
    }

    pub fn block<F, T>(&mut self, f: F) -> IrResult<T>
    where
        F: FnOnce(&mut Context) -> IrResult<T>,
    {
        f(self)
    }

    pub fn push_instance_history(&mut self, x: Signature) -> Result<bool, InstanceHistoryError> {
        self.instance_history.push(x, &self.config)
    }

    pub fn pop_instance_history(&mut self) {
        self.instance_history.pop();
    }

    pub fn get_current_signature(&self) -> Option<&Signature> {
        self.instance_history.get_current_signature()
    }

    pub fn get_instance_history(&self, sig: &Signature) -> Option<Component> {
        self.instance_history.get(sig)
    }

    pub fn set_instance_history(&mut self, sig: &Signature, component: Component) {
        self.instance_history.set(sig, component);
    }

    pub fn push_hierarchy(&mut self, x: StrId) {
        self.hierarchy.push(x);
        self.hierarchical_variables.push(vec![]);
        self.hierarchical_functions.push(vec![]);
    }

    pub fn pop_hierarchy(&mut self) {
        self.hierarchy.pop();
        let drops = self.hierarchical_variables.pop();
        if let Some(drops) = drops {
            for x in drops {
                if let Some(y) = self.shadowed_variables.get_mut(&x)
                    && let Some(y) = y.pop()
                {
                    self.var_paths.insert(x, y);
                } else {
                    self.var_paths.remove(&x);
                }
            }
        }
        let drops = self.hierarchical_functions.pop();
        if let Some(drops) = drops {
            for x in drops {
                self.func_paths.remove(&x);
            }
        }
    }

    pub fn push_affiliation(&mut self, x: Affiliation) {
        self.affiliation.push(x);
    }

    pub fn pop_affiliation(&mut self) {
        self.affiliation.pop();
    }

    pub fn push_override(&mut self, x: HashMap<VarPath, (Comptime, Expression)>) {
        self.overrides.push(x);
    }

    pub fn pop_override(&mut self) {
        self.overrides.pop();
    }

    pub fn push_generic_map(&mut self, x: Vec<GenericMap>) {
        self.generic_maps.push(x);
    }

    pub fn pop_generic_map(&mut self) {
        self.generic_maps.pop();
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

    pub fn find_path(&self, path: &VarPath) -> Option<(VarId, Comptime)> {
        self.var_paths.get(path).cloned()
    }

    pub fn get_variable(&self, id: &VarId) -> Option<Variable> {
        self.variables.get(id).cloned()
    }

    pub fn remove_path(&mut self, path: &VarPath) {
        self.var_paths.remove(path);
    }

    pub fn drain_var_paths(&mut self) -> HashMap<VarPath, (VarId, Comptime)> {
        self.var_paths.drain().collect()
    }

    pub fn drain_func_paths(&mut self) -> HashMap<FuncPath, FuncProto> {
        self.func_paths.drain().collect()
    }

    pub fn drain_variables(&mut self) -> HashMap<VarId, Variable> {
        self.variables.drain().collect()
    }

    pub fn drain_port_types(&mut self) -> HashMap<VarPath, (Type, ClockDomain)> {
        self.port_types.drain().collect()
    }

    pub fn drain_functions(&mut self) -> HashMap<VarId, Function> {
        self.functions.drain().collect()
    }

    pub fn drain_modports(&mut self) -> HashMap<StrId, Vec<(StrId, Direction)>> {
        self.modports.drain().collect()
    }

    pub fn drain_declarations(&mut self) -> Vec<Declaration> {
        self.declarations.drain(..).collect()
    }

    pub fn drain_errors(&mut self) -> Vec<AnalyzerError> {
        self.errors.drain(..).collect()
    }
}
