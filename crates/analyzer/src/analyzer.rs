use crate::HashMap;
use crate::analyzer::resource_table::PathId;
use crate::analyzer_error::AnalyzerError;
use crate::attribute_table;
use crate::conv::{Context, Conv};
use crate::handlers::*;
use crate::ir::Ir;
use crate::msb_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::reference_table;
use crate::symbol::{Affiliation, Direction, DocComment, Symbol, SymbolId, SymbolKind, TypeKind};
use crate::symbol_path::SymbolPathNamespace;
use crate::symbol_table;
use crate::type_dag;
use crate::var_ref::{ExpressionTargetType, VarRef, VarRefAffiliation, VarRefPath, VarRefType};
use std::path::Path;
use veryl_metadata::{Build, EnvVar, Lint, Metadata};
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenSource};
use veryl_parser::veryl_walker::{Handler, VerylWalker};

pub struct AnalyzerPass1 {
    handlers: Pass1Handlers,
}

impl AnalyzerPass1 {
    pub fn new(build_opt: &Build, lint_opt: &Lint, env_var: &EnvVar) -> Self {
        AnalyzerPass1 {
            handlers: Pass1Handlers::new(build_opt, lint_opt, env_var),
        }
    }
}

impl VerylWalker for AnalyzerPass1 {
    fn get_handlers(&mut self) -> Option<Vec<(bool, &mut dyn Handler)>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct AnalyzerPass2 {
    handlers: Pass2Handlers,
}

impl AnalyzerPass2 {
    pub fn new(build_opt: &Build, lint_opt: &Lint, env_var: &EnvVar) -> Self {
        AnalyzerPass2 {
            handlers: Pass2Handlers::new(build_opt, lint_opt, env_var),
        }
    }
}

impl VerylWalker for AnalyzerPass2 {
    fn get_handlers(&mut self) -> Option<Vec<(bool, &mut dyn Handler)>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct AnalyzerPass3 {
    path: PathId,
}

impl AnalyzerPass3 {
    pub fn new(path: &Path) -> Self {
        let path = resource_table::get_path_id(path.to_path_buf()).unwrap();
        AnalyzerPass3 { path }
    }

    pub fn check_variables(&self, symbols: &[Symbol]) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        for symbol in symbols {
            if symbol.token.source == self.path
                && let SymbolKind::Variable(_) = symbol.kind
                && symbol.references.is_empty()
                && !symbol.allow_unused
            {
                let name = symbol.token.to_string();
                if !name.starts_with('_') {
                    ret.push(AnalyzerError::unused_variable(
                        &symbol.token.to_string(),
                        &symbol.token.into(),
                    ));
                }
            }
        }

        ret
    }

    pub fn check_assignment(&self, symbols: &[Symbol]) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        let assign_list = symbol_table::get_assign_list();
        let mut assignable_list = Vec::new();

        for symbol in symbols {
            if symbol.token.source == self.path {
                assignable_list.append(&mut traverse_assignable_symbol(
                    symbol.id,
                    &VarRefPath::new(vec![(&symbol.id).into()]),
                ));
            }
        }
        let mut assignable_list: Vec<_> = assignable_list
            .iter()
            .map(|x| (x, x.proto_path(), vec![]))
            .collect();
        for assign in &assign_list {
            let assign_proto_path = assign.path.proto_path();
            for assignable in &mut assignable_list {
                if assignable.0.included(&assign.path)
                    || assignable.1.included(&assign.path)
                    || assignable.0.included(&assign_proto_path)
                {
                    assignable.2.push((assign.position.clone(), assign.partial));
                }
            }
        }

        for (path, _, positions) in &assignable_list {
            if positions.is_empty() {
                let full_path = path.full_path();
                let symbol = symbol_table::get(*full_path.first().unwrap()).unwrap();
                if must_be_assigned(&symbol.kind) {
                    let path: Vec<_> = full_path
                        .iter()
                        .map(|x| symbol_table::get(*x).unwrap().token.to_string())
                        .collect();
                    ret.push(AnalyzerError::unassign_variable(
                        &path.join("."),
                        &symbol.token.into(),
                    ));
                }
            }
        }

        ret
    }

    pub fn check_unassigned(
        &self,
        var_refs: &HashMap<VarRefAffiliation, Vec<VarRef>>,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        let var_refs = var_refs.iter().filter(|(key, _)| {
            matches!(
                key,
                VarRefAffiliation::AlwaysComb { token } if token.source == self.path
            )
        });
        for (_, list) in var_refs {
            let assign_list: Vec<_> = list
                .iter()
                .enumerate()
                .filter(|(_, x)| x.is_assign())
                .collect();
            let assign_list: Vec<_> = assign_list
                .iter()
                .filter(|(from_index, from_ref)| {
                    !assign_list.iter().any(|(to_index, to_ref)| {
                        AnalyzerPass3::is_related_var_ref(from_ref, *from_index, to_ref, *to_index)
                    })
                })
                .collect();
            let target_list: Vec<_> = list
                .iter()
                .enumerate()
                .filter(|(_, x)| {
                    if let VarRefType::ExpressionTarget { r#type } = x.r#type {
                        matches!(
                            r#type,
                            ExpressionTargetType::Variable | ExpressionTargetType::OutputPort
                        )
                    } else {
                        false
                    }
                })
                .collect();
            for (ref_index, var_ref) in target_list {
                let before_assign = assign_list.iter().any(|(assing_index, assign)| {
                    AnalyzerPass3::is_related_var_ref(assign, *assing_index, var_ref, ref_index)
                });
                if before_assign {
                    let full_path = var_ref.path.full_path();
                    let symbol = symbol_table::get(*full_path.first().unwrap()).unwrap();
                    ret.push(AnalyzerError::unassign_variable(
                        &var_ref.path.to_string(),
                        &symbol.token.into(),
                    ));
                }
            }
        }

        ret
    }

    fn is_related_var_ref(
        from_var_ref: &VarRef,
        from_index: usize,
        to_var_ref: &VarRef,
        to_index: usize,
    ) -> bool {
        if to_index >= from_index {
            return false;
        }

        from_var_ref.path.may_fully_included(&to_var_ref.path)
            && (AnalyzerPass3::share_same_branch_path(from_var_ref, to_var_ref)
                || AnalyzerPass3::in_other_branch_group(from_var_ref, to_var_ref))
    }

    fn share_same_branch_path(from: &VarRef, to: &VarRef) -> bool {
        let len = if to.branch_group.len() < from.branch_group.len() {
            to.branch_group.len()
        } else {
            from.branch_group.len()
        };

        if len == 0 {
            return false;
        }

        for i in 0..len {
            if to.branch_group[i] != from.branch_group[i] {
                return false;
            }
            if to.branch_index[i] != from.branch_index[i] {
                return false;
            }
        }

        true
    }

    fn in_other_branch_group(from: &VarRef, to: &VarRef) -> bool {
        let len = if to.branch_group.len() < from.branch_group.len() {
            to.branch_group.len()
        } else {
            from.branch_group.len()
        };

        if len == 0 {
            return true;
        }

        for i in 0..len {
            if to.branch_group[i] != from.branch_group[i] {
                return true;
            }
        }

        false
    }
}

pub struct Analyzer {
    build_opt: Build,
    lint_opt: Lint,
    env_var: EnvVar,
}

pub struct AnalyzerPass3Info {
    symbols: Vec<Symbol>,
    var_refs: HashMap<VarRefAffiliation, Vec<VarRef>>,
}

fn insert_namespace_symbol(name: &str, public: bool) -> StrId {
    let token = Token::new(name, 0, 0, 0, 0, TokenSource::External);
    let symbol = Symbol::new(
        &token,
        SymbolKind::Namespace,
        &Namespace::new(),
        public,
        DocComment::default(),
    );
    symbol_table::insert(&token, symbol);
    token.text
}

impl Analyzer {
    pub fn new(metadata: &Metadata) -> Self {
        insert_namespace_symbol(&metadata.project.name, true);
        for locks in metadata.lockfile.lock_table.values() {
            for lock in locks {
                let prj = insert_namespace_symbol(&lock.name, lock.visible);
                for lock_dep in &lock.dependencies {
                    let from = resource_table::insert_str(&lock_dep.name);
                    let to = metadata
                        .lockfile
                        .lock_table
                        .get(&lock_dep.source.to_url())
                        .unwrap();

                    let to = to.iter().find(|x| x.source == lock_dep.source).unwrap();
                    let to = insert_namespace_symbol(&to.name, to.visible);
                    symbol_table::add_project_local(prj, from, to);
                }
            }
        }
        Analyzer {
            build_opt: metadata.build.clone(),
            lint_opt: metadata.lint.clone(),
            env_var: metadata.env_var.clone(),
        }
    }

    pub fn analyze_pass1<T: AsRef<Path>>(
        &self,
        project_name: &str,
        _path: T,
        input: &Veryl,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let mut pass1 = AnalyzerPass1::new(&self.build_opt, &self.lint_opt, &self.env_var);
        pass1.veryl(input);
        ret.append(&mut pass1.handlers.get_errors());

        ret
    }

    pub fn analyze_post_pass1() -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        symbol_table::apply_import();
        symbol_table::resolve_user_defined();
        ret.append(&mut symbol_table::apply_bind());
        ret.append(&mut reference_table::apply());
        ret.append(&mut type_dag::apply());

        ret
    }

    fn create_ir(input: &Veryl, build_opt: &Build) -> (Ir, Vec<AnalyzerError>) {
        let mut context = Context::default();
        context.instance_history.depth_limit = build_opt.instance_depth_limit;
        context.instance_history.total_limit = build_opt.instance_total_limit;
        let ir: Ir = Conv::conv(&mut context, input);
        ir.eval_assign(&mut context);
        let errors = context.drain_errors();
        (ir, errors)
    }

    pub fn analyze_pass2<T: AsRef<Path>>(
        &self,
        project_name: &str,
        _path: T,
        input: &Veryl,
        ir: Option<&mut Ir>,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let mut pass2 = AnalyzerPass2::new(&self.build_opt, &self.lint_opt, &self.env_var);
        pass2.veryl(input);
        ret.append(&mut pass2.handlers.get_errors());

        // some pass2 errors are generated in create_ir
        // The actual implementation is under crate/analyzer/src/ir/conv
        let mut ir_result = Self::create_ir(input, &self.build_opt);
        if let Some(x) = ir {
            x.append(&mut ir_result.0);
            ret.append(&mut ir_result.1);
        } else {
            // If IR is not used for successor stages, UnsupportedByIr should be ignored
            for error in ir_result.1 {
                if !matches!(error, AnalyzerError::UnsupportedByIr { .. }) {
                    ret.push(error);
                }
            }
        }

        ret
    }

    pub fn analyze_post_pass2() -> AnalyzerPass3Info {
        let symbols = symbol_table::get_all();
        let var_refs = symbol_table::get_var_ref_list();
        AnalyzerPass3Info { symbols, var_refs }
    }

    pub fn analyze_pass3<T: AsRef<Path>>(
        &self,
        project_name: &str,
        path: T,
        _input: &Veryl,
        info: &AnalyzerPass3Info,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let pass3 = AnalyzerPass3::new(path.as_ref());
        let enables = &self.env_var.analyzer_pass3_enables;
        if enables[0] {
            ret.append(&mut pass3.check_variables(&info.symbols));
        }
        if enables[1] {
            ret.append(&mut pass3.check_assignment(&info.symbols));
        }
        if enables[2] {
            ret.append(&mut pass3.check_unassigned(&info.var_refs));
        }

        ret
    }

    pub fn clear(&self) {
        attribute_table::clear();
        msb_table::clear();
        namespace_table::clear();
        symbol_table::clear();
        type_dag::clear();
    }
}

fn is_assignable(direction: &Direction) -> bool {
    matches!(
        direction,
        Direction::Inout | Direction::Output | Direction::Modport
    )
}

fn must_be_assigned(kind: &SymbolKind) -> bool {
    match kind {
        SymbolKind::Port(x) => x.direction == Direction::Output,
        SymbolKind::ModportVariableMember(x) => x.direction == Direction::Output,
        SymbolKind::Variable(_) => true,
        SymbolKind::StructMember(_) => true,
        _ => false,
    }
}

fn traverse_type_symbol(id: SymbolId, path: &VarRefPath) -> Vec<VarRefPath> {
    if let Some(symbol) = symbol_table::get(id) {
        match &symbol.kind {
            SymbolKind::Variable(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Some(id) = x.symbol {
                        return traverse_type_symbol(id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::StructMember(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Some(id) = x.symbol {
                        return traverse_type_symbol(id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::UnionMember(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Some(id) = x.symbol {
                        return traverse_type_symbol(id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::TypeDef(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Some(id) = x.symbol {
                        return traverse_type_symbol(id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::Parameter(x) if x.r#type.kind == TypeKind::Type => {
                let r#type: Result<crate::symbol::Type, ()> = x.value.as_ref().unwrap().try_into();
                if let Ok(r#type) = r#type {
                    if let TypeKind::UserDefined(ref x) = r#type.kind {
                        if let Some(id) = x.symbol {
                            return traverse_type_symbol(id, path);
                        }
                    } else {
                        return vec![path.clone()];
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::Struct(x) => {
                let mut ret = Vec::new();
                for member in &x.members {
                    let mut path = path.clone();
                    path.push(member.into());
                    ret.append(&mut traverse_type_symbol(*member, &path));
                }
                return ret;
            }
            // TODO union support
            //SymbolKind::Union(x) => {
            //    let mut ret = Vec::new();
            //    for member in &x.members {
            //        let mut path = path.clone();
            //        path.push(*member);
            //        ret.append(&mut traverse_type_symbol(*member, &path));
            //    }
            //    return ret;
            //}
            SymbolKind::Modport(x) => {
                let mut ret = Vec::new();
                for member in &x.members {
                    let mut path = path.clone();
                    path.push(member.into());
                    ret.append(&mut traverse_type_symbol(*member, &path));
                }
                return ret;
            }
            SymbolKind::ModportVariableMember(x) if is_assignable(&x.direction) => {
                // Use outer namespace of modport to trace variable
                let mut namespace = symbol.namespace.clone();
                let _ = namespace.pop();
                let symbol_path = SymbolPathNamespace((&symbol.token).into(), namespace);

                if let Ok(symbol) = symbol_table::resolve(symbol_path) {
                    return traverse_type_symbol(symbol.found.id, path);
                }
            }
            SymbolKind::Enum(_) => {
                return vec![path.clone()];
            }
            _ => (),
        }
    }

    vec![]
}

fn traverse_assignable_symbol(id: SymbolId, path: &VarRefPath) -> Vec<VarRefPath> {
    // check cyclic dependency
    if path.full_path().iter().filter(|x| **x == id).count() > 1 {
        return vec![];
    }

    if let Some(symbol) = symbol_table::get(id) {
        match &symbol.kind {
            SymbolKind::Port(x) if is_assignable(&x.direction) && !x.is_proto => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Some(id) = x.symbol {
                        return traverse_type_symbol(id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::Variable(x)
                if x.affiliation == Affiliation::Module
                    || x.affiliation == Affiliation::Function
                    || x.affiliation == Affiliation::StatementBlock =>
            {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Some(id) = x.symbol {
                        return traverse_type_symbol(id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            _ => (),
        }
    }

    vec![]
}
