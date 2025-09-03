use crate::HashMap;
use crate::analyzer::resource_table::PathId;
use crate::analyzer_error::AnalyzerError;
use crate::attribute_table;
use crate::handlers::check_expression::CheckExpression;
use crate::handlers::*;
use crate::instance_history;
use crate::msb_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::reference_table;
use crate::symbol::{
    Direction, DocComment, Symbol, SymbolId, SymbolKind, TypeKind, VariableAffiliation,
};
use crate::symbol_path::SymbolPathNamespace;
use crate::symbol_table;
use crate::type_dag;
use crate::var_ref::{
    AssignPosition, AssignPositionTree, AssignPositionType, ExpressionTargetType, VarRef,
    VarRefAffiliation, VarRefPath, VarRefType,
};
use itertools::Itertools;
use std::path::Path;
use veryl_metadata::{Build, EnvVar, Lint, Metadata};
use veryl_parser::resource_table;
use veryl_parser::token_range::TokenRange;
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

pub struct AnalyzerPass2Expression {
    check_expression: CheckExpression,
}

impl AnalyzerPass2Expression {
    pub fn new(inst_context: Vec<TokenRange>) -> Self {
        AnalyzerPass2Expression {
            check_expression: CheckExpression::new(inst_context),
        }
    }

    pub fn get_errors(&mut self) -> Vec<AnalyzerError> {
        self.check_expression.errors.drain(0..).collect()
    }
}

impl VerylWalker for AnalyzerPass2Expression {
    fn get_handlers(&mut self) -> Option<Vec<(bool, &mut dyn Handler)>> {
        Some(vec![(true, &mut self.check_expression as &mut dyn Handler)])
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
        let mut assignable_list: Vec<_> = assignable_list.iter().map(|x| (x, vec![])).collect();
        for assign in &assign_list {
            for assignable in &mut assignable_list {
                if assignable.0.included(&assign.path) {
                    assignable.1.push((assign.position.clone(), assign.partial));
                }
            }
        }

        for (path, positions) in &assignable_list {
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

            let full_path = path.full_path();
            let symbol = symbol_table::get(*full_path.first().unwrap()).unwrap();

            if positions.len() > 1 {
                for comb in positions.iter().combinations(2) {
                    ret.append(&mut check_multiple_assignment(&symbol, comb[0], comb[1]));
                }
            }

            let non_state_variable = match &symbol.kind {
                SymbolKind::Port(_) => true,
                SymbolKind::Variable(x) => x.affiliation != VariableAffiliation::StatementBlock,
                _ => {
                    unreachable!()
                }
            };
            if non_state_variable {
                ret.append(&mut check_assign_position_tree(&symbol, positions));
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
                    *assing_index > ref_index
                        && (AnalyzerPass3::share_same_branch_path(var_ref, assign)
                            || AnalyzerPass3::in_other_branch_group(var_ref, assign))
                        && assign.path.may_fully_included(&var_ref.path)
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

    fn share_same_branch_path(var_ref: &VarRef, assign: &VarRef) -> bool {
        let len = if var_ref.branch_group.len() < assign.branch_group.len() {
            var_ref.branch_group.len()
        } else {
            assign.branch_group.len()
        };

        if len == 0 {
            return false;
        }

        for i in 0..len {
            if var_ref.branch_group[i] != assign.branch_group[i] {
                return false;
            }
            if var_ref.branch_index[i] != assign.branch_index[i] {
                return false;
            }
        }

        true
    }

    fn in_other_branch_group(var_ref: &VarRef, assign: &VarRef) -> bool {
        let len = if var_ref.branch_group.len() < assign.branch_group.len() {
            var_ref.branch_group.len()
        } else {
            assign.branch_group.len()
        };

        if len == 0 {
            return true;
        }

        for i in 0..len {
            if var_ref.branch_group[i] != assign.branch_group[i] {
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

fn new_namespace(name: &str, public: bool) -> (Token, Symbol) {
    let token = Token::new(name, 0, 0, 0, 0, TokenSource::External);
    let symbol = Symbol::new(
        &token,
        SymbolKind::Namespace,
        &Namespace::new(),
        public,
        DocComment::default(),
    );
    (token, symbol)
}

impl Analyzer {
    pub fn new(metadata: &Metadata) -> Self {
        for locks in metadata.lockfile.lock_table.values() {
            for lock in locks {
                let prj = resource_table::insert_str(&lock.name);
                let (token, symbol) = new_namespace(&lock.name, lock.visible);
                symbol_table::insert(&token, symbol);
                for lock_dep in &lock.dependencies {
                    let from = resource_table::insert_str(&lock_dep.name);
                    let to = metadata
                        .lockfile
                        .lock_table
                        .get(&lock_dep.source.to_url())
                        .unwrap();
                    let to = to.iter().find(|x| x.source == lock_dep.source).unwrap();

                    let (token, symbol) = new_namespace(&to.name, to.visible);
                    symbol_table::insert(&token, symbol);

                    let to = resource_table::insert_str(&to.name);
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

    pub fn analyze_pass2<T: AsRef<Path>>(
        &self,
        project_name: &str,
        _path: T,
        input: &Veryl,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        instance_history::clear();
        instance_history::set_depth_limit(self.build_opt.instance_depth_limit);
        instance_history::set_total_limit(self.build_opt.instance_total_limit);
        let mut pass2 = AnalyzerPass2::new(&self.build_opt, &self.lint_opt, &self.env_var);
        pass2.veryl(input);
        ret.append(&mut pass2.handlers.get_errors());

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
                if x.affiliation == VariableAffiliation::Module
                    || x.affiliation == VariableAffiliation::Function
                    || x.affiliation == VariableAffiliation::StatementBlock =>
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

fn check_multiple_assignment(
    symbol: &Symbol,
    x: &(AssignPosition, bool),
    y: &(AssignPosition, bool),
) -> Vec<AnalyzerError> {
    let x_pos = &x.0;
    let y_pos = &y.0;
    let x_partial = &x.1;
    let y_partial = &y.1;
    let mut ret = Vec::new();
    let len = x_pos.0.len().min(y_pos.0.len());

    let x_maybe = x_pos.0.last().unwrap().is_maybe();
    let y_maybe = y_pos.0.last().unwrap().is_maybe();
    if x_maybe || y_maybe {
        return vec![];
    }

    let x_define = x_pos.0.last().unwrap().define_context();
    let y_define = y_pos.0.last().unwrap().define_context();

    // If x and y is in exclusive define context, they are not conflict.
    if x_define.exclusive(y_define) {
        return vec![];
    }

    // Earyl return to avoid calling AnalyzerError constructor
    for i in 0..len {
        let x_type = &x_pos.0[i];
        let y_type = &y_pos.0[i];
        if x_type != y_type {
            match x_type {
                AssignPositionType::DeclarationBranch { .. }
                | AssignPositionType::Declaration { .. } => (),
                _ => return vec![],
            }
        }
    }

    for i in 0..len {
        let x_type = &x_pos.0[i];
        let y_type = &y_pos.0[i];
        if x_type != y_type {
            match x_type {
                AssignPositionType::DeclarationBranch { .. }
                | AssignPositionType::Declaration { .. } => {
                    if !x_partial | !y_partial {
                        ret.push(AnalyzerError::multiple_assignment(
                            &symbol.token.to_string(),
                            &symbol.token.into(),
                            &x_pos.0.last().unwrap().token().into(),
                            &y_pos.0.last().unwrap().token().into(),
                        ));
                    }
                }
                _ => (),
            }
        }
    }

    ret
}

fn check_assign_position_tree(
    symbol: &Symbol,
    positions: &[(AssignPosition, bool)],
) -> Vec<AnalyzerError> {
    let mut ret = Vec::new();

    let mut tree = AssignPositionTree::default();
    for x in positions {
        let pos = &x.0;

        tree.add(pos.clone());
    }

    if let Some(token) = tree.check_always_comb_uncovered() {
        ret.push(AnalyzerError::uncovered_branch(
            &symbol.token.to_string(),
            &symbol.token.into(),
            &token.into(),
        ));
    }

    if let Some(token) = tree.check_always_ff_missing_reset() {
        ret.push(AnalyzerError::missing_reset_statement(
            &symbol.token.to_string(),
            &symbol.token.into(),
            &token.into(),
        ));
    }

    ret
}
