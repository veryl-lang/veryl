use crate::analyzer::resource_table::PathId;
use crate::analyzer_error::AnalyzerError;
use crate::attribute_table;
use crate::handlers::*;
use crate::msb_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{
    Direction, DocComment, Symbol, SymbolId, SymbolKind, TypeKind, VariableAffiliation,
};
use crate::symbol_table;
use crate::type_dag;
use crate::var_ref::{
    AssignPosition, AssignPositionTree, AssignPositionType, ExpressionTargetType, VarRef,
    VarRefAffiliation, VarRefPath, VarRefType,
};
use itertools::Itertools;
use std::collections::HashMap;
use std::path::Path;
use veryl_metadata::{Build, Lint, Metadata};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenSource};
use veryl_parser::veryl_walker::{Handler, VerylWalker};

pub struct AnalyzerPass1<'a> {
    handlers: Pass1Handlers<'a>,
}

impl<'a> AnalyzerPass1<'a> {
    pub fn new(text: &'a str, build_opt: &'a Build, lint_opt: &'a Lint) -> Self {
        AnalyzerPass1 {
            handlers: Pass1Handlers::new(text, build_opt, lint_opt),
        }
    }
}

impl VerylWalker for AnalyzerPass1<'_> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct AnalyzerPass2<'a> {
    handlers: Pass2Handlers<'a>,
}

impl<'a> AnalyzerPass2<'a> {
    pub fn new(text: &'a str, build_opt: &'a Build, lint_opt: &'a Lint) -> Self {
        AnalyzerPass2 {
            handlers: Pass2Handlers::new(text, build_opt, lint_opt),
        }
    }
}

impl VerylWalker for AnalyzerPass2<'_> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}
pub struct AnalyzerPass3<'a> {
    path: PathId,
    text: &'a str,
    symbols: Vec<Symbol>,
    var_refs: HashMap<VarRefAffiliation, Vec<VarRef>>,
}

impl<'a> AnalyzerPass3<'a> {
    pub fn new(path: &'a Path, text: &'a str) -> Self {
        let symbols = symbol_table::get_all();
        let var_refs = symbol_table::get_var_ref_list();
        let path = resource_table::get_path_id(path.to_path_buf()).unwrap();
        AnalyzerPass3 {
            path,
            text,
            symbols,
            var_refs,
        }
    }

    pub fn check_variables(&self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        for symbol in &self.symbols {
            if symbol.token.source == self.path {
                if let SymbolKind::Variable(_) = symbol.kind {
                    if symbol.references.is_empty() && !symbol.allow_unused {
                        let name = symbol.token.to_string();
                        if !name.starts_with('_') {
                            ret.push(AnalyzerError::unused_variable(
                                &symbol.token.to_string(),
                                self.text,
                                &symbol.token.into(),
                            ));
                        }
                    }
                }
            }
        }

        ret
    }

    pub fn check_assignment(&self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        let assign_list = symbol_table::get_assign_list();
        let mut assignable_list = Vec::new();

        for symbol in &self.symbols {
            if symbol.token.source == self.path {
                assignable_list.append(&mut traverse_assignable_symbol(
                    symbol.id,
                    &VarRefPath::new((&symbol.id).into()),
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
                        self.text,
                        &symbol.token.into(),
                    ));
                }
            }

            let full_path = path.full_path();
            let symbol = symbol_table::get(*full_path.first().unwrap()).unwrap();

            if positions.len() > 1 {
                for comb in positions.iter().combinations(2) {
                    ret.append(&mut check_multiple_assignment(
                        &symbol, self.text, comb[0], comb[1],
                    ));
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
                ret.append(&mut check_assign_position_tree(
                    &symbol, self.text, positions,
                ));
            }
        }

        ret
    }

    pub fn check_unassigned(&self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        let var_refs = self.var_refs.iter().filter(|(key, _)| {
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
                    *assing_index > ref_index && assign.path.may_fully_included(&var_ref.path)
                });
                if before_assign {
                    let full_path = var_ref.path.full_path();
                    let symbol = symbol_table::get(*full_path.first().unwrap()).unwrap();
                    ret.push(AnalyzerError::unassign_variable(
                        &var_ref.path.to_string(),
                        self.text,
                        &symbol.token.into(),
                    ));
                }
            }
        }

        ret
    }
}

pub struct Analyzer {
    build_opt: Build,
    lint_opt: Lint,
}

fn new_namespace(name: &str) -> (Token, Symbol) {
    let token = Token::new(name, 0, 0, 0, 0, TokenSource::External);
    let symbol = Symbol::new(
        &token,
        SymbolKind::Namespace,
        &Namespace::new(),
        false,
        DocComment::default(),
    );
    (token, symbol)
}

impl Analyzer {
    pub fn new(metadata: &Metadata) -> Self {
        for locks in metadata.lockfile.lock_table.values() {
            for lock in locks {
                let prj = resource_table::insert_str(&lock.name);
                let (token, symbol) = new_namespace(&lock.name);
                symbol_table::insert(&token, symbol);
                for lock_dep in &lock.dependencies {
                    let from = resource_table::insert_str(&lock_dep.name);
                    let to = metadata.lockfile.lock_table.get(&lock_dep.url).unwrap();
                    let to = to.iter().find(|x| x.version == lock_dep.version).unwrap();

                    let (token, symbol) = new_namespace(&to.name);
                    symbol_table::insert(&token, symbol);

                    let to = resource_table::insert_str(&to.name);
                    symbol_table::add_project_local(prj, from, to);
                }
            }
        }
        Analyzer {
            build_opt: metadata.build.clone(),
            lint_opt: metadata.lint.clone(),
        }
    }

    pub fn analyze_pass1<T: AsRef<Path>>(
        &self,
        project_name: &str,
        text: &str,
        _path: T,
        input: &Veryl,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let mut pass1 = AnalyzerPass1::new(text, &self.build_opt, &self.lint_opt);
        pass1.veryl(input);
        ret.append(&mut pass1.handlers.get_errors());

        ret
    }

    pub fn analyze_post_pass1() {
        symbol_table::apply_import();
    }

    pub fn analyze_pass2<T: AsRef<Path>>(
        &self,
        project_name: &str,
        text: &str,
        _path: T,
        input: &Veryl,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let mut pass2 = AnalyzerPass2::new(text, &self.build_opt, &self.lint_opt);
        pass2.veryl(input);
        ret.append(&mut pass2.handlers.get_errors());

        ret
    }

    pub fn analyze_pass3<T: AsRef<Path>>(
        &self,
        project_name: &str,
        text: &str,
        path: T,
        _input: &Veryl,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let pass3 = AnalyzerPass3::new(path.as_ref(), text);
        ret.append(&mut pass3.check_variables());
        ret.append(&mut pass3.check_assignment());
        ret.append(&mut pass3.check_unassigned());

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
        Direction::Ref | Direction::Inout | Direction::Output | Direction::Modport
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
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        return traverse_type_symbol(symbol.found.id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::StructMember(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        return traverse_type_symbol(symbol.found.id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::UnionMember(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        return traverse_type_symbol(symbol.found.id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::TypeDef(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        return traverse_type_symbol(symbol.found.id, path);
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::Parameter(x) if x.r#type.kind == TypeKind::Type => {
                let r#type: Result<crate::symbol::Type, ()> = (&x.value).try_into();
                if let Ok(r#type) = r#type {
                    if let TypeKind::UserDefined(ref x) = r#type.kind {
                        if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                            return traverse_type_symbol(symbol.found.id, path);
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
                if let Ok(symbol) = symbol_table::resolve(&symbol.token) {
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
                if let Some(ref x) = x.r#type {
                    if let TypeKind::UserDefined(ref x) = x.kind {
                        if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                            return traverse_type_symbol(symbol.found.id, path);
                        }
                    } else {
                        return vec![path.clone()];
                    }
                }
            }
            SymbolKind::Variable(x)
                if x.affiliation == VariableAffiliation::Module
                    || x.affiliation == VariableAffiliation::Function
                    || x.affiliation == VariableAffiliation::StatementBlock =>
            {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        return traverse_type_symbol(symbol.found.id, path);
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
    text: &str,
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
                            text,
                            &symbol.token.into(),
                            &x_pos.0.last().unwrap().token().into(),
                            &y_pos.0.last().unwrap().token().into(),
                        ));
                    }
                }
                _ => return vec![],
            }
        }
    }

    ret
}

fn check_assign_position_tree(
    symbol: &Symbol,
    text: &str,
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
            text,
            &symbol.token.into(),
            &token.into(),
        ));
    }

    if let Some(token) = tree.check_always_ff_missing_reset() {
        ret.push(AnalyzerError::missing_reset_statement(
            &symbol.token.to_string(),
            text,
            &symbol.token.into(),
            &token.into(),
        ));
    }

    ret
}
