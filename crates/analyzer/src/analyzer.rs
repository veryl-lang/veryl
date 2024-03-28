use crate::analyzer_error::AnalyzerError;
use crate::handlers::*;
use crate::namespace_table;
use crate::symbol::{
    Direction, ParameterValue, Symbol, SymbolId, SymbolKind, TypeKind, VariableAffiniation,
};
use crate::symbol_table::{
    self, AssignPath, AssignPosition, AssignPositionTree, AssignPositionType, ResolveSymbol,
};
use itertools::Itertools;
use std::path::Path;
use veryl_metadata::{Lint, Metadata};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, VerylWalker};

pub struct AnalyzerPass1<'a> {
    handlers: Pass1Handlers<'a>,
}

impl<'a> AnalyzerPass1<'a> {
    pub fn new(text: &'a str, lint_opt: &'a Lint) -> Self {
        AnalyzerPass1 {
            handlers: Pass1Handlers::new(text, lint_opt),
        }
    }
}

impl<'a> VerylWalker for AnalyzerPass1<'a> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct AnalyzerPass2<'a> {
    handlers: Pass2Handlers<'a>,
}

impl<'a> AnalyzerPass2<'a> {
    pub fn new(text: &'a str, lint_opt: &'a Lint) -> Self {
        AnalyzerPass2 {
            handlers: Pass2Handlers::new(text, lint_opt),
        }
    }
}

impl<'a> VerylWalker for AnalyzerPass2<'a> {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct Analyzer {
    lint_opt: Lint,
}

impl Analyzer {
    pub fn new(metadata: &Metadata) -> Self {
        for locks in metadata.lockfile.lock_table.values() {
            for lock in locks {
                let prj = resource_table::insert_str(&lock.name);
                for lock_dep in &lock.dependencies {
                    let from = resource_table::insert_str(&lock_dep.name);
                    let to = metadata.lockfile.lock_table.get(&lock_dep.url).unwrap();
                    let to = to.iter().find(|x| x.version == lock_dep.version).unwrap();
                    let to = resource_table::insert_str(&to.name);
                    symbol_table::add_project_local(prj, from, to);
                }
            }
        }
        Analyzer {
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
        let mut pass1 = AnalyzerPass1::new(text, &self.lint_opt);
        pass1.veryl(input);
        ret.append(&mut pass1.handlers.get_errors());

        ret
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
        let mut pass2 = AnalyzerPass2::new(text, &self.lint_opt);
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
        ret.append(&mut Analyzer::check_symbol_table(path.as_ref(), text));

        ret
    }

    fn check_symbol_table(path: &Path, text: &str) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();
        let symbols = symbol_table::get_all();

        // check unused variables
        let path = resource_table::get_path_id(path.to_path_buf()).unwrap();
        for symbol in &symbols {
            if symbol.token.source == path {
                if let SymbolKind::Variable(_) = symbol.kind {
                    if symbol.references.is_empty() && !symbol.allow_unused {
                        let name = symbol.token.to_string();
                        if name.starts_with('_') {
                            continue;
                        }

                        ret.push(AnalyzerError::unused_variable(
                            &symbol.token.to_string(),
                            text,
                            &symbol.token,
                        ));
                    }
                }
            }
        }

        // check assignment
        let assign_list = symbol_table::get_assign_list();
        let mut assignable_list = Vec::new();
        for symbol in &symbols {
            if symbol.token.source == path {
                assignable_list.append(&mut traverse_assignable_symbol(
                    symbol.id,
                    &AssignPath::new(symbol.id),
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
                let symbol = symbol_table::get(*path.0.first().unwrap()).unwrap();
                ret.push(AnalyzerError::unassign_variable(
                    &symbol.token.to_string(),
                    text,
                    &symbol.token,
                ));
            }

            let symbol = symbol_table::get(*path.0.first().unwrap()).unwrap();

            if positions.len() > 1 {
                for comb in positions.iter().combinations(2) {
                    ret.append(&mut check_multiple_assignment(
                        &symbol, text, comb[0], comb[1],
                    ));
                }
            }

            ret.append(&mut check_uncovered_branch(&symbol, text, positions));
        }

        ret
    }
}

fn is_assignable(direction: &Direction) -> bool {
    matches!(
        direction,
        Direction::Ref | Direction::Inout | Direction::Output | Direction::Modport
    )
}

fn traverse_type_symbol(id: SymbolId, path: &AssignPath) -> Vec<AssignPath> {
    if let Some(symbol) = symbol_table::get(id) {
        match &symbol.kind {
            SymbolKind::Variable(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        if let ResolveSymbol::Symbol(symbol) = symbol.found {
                            return traverse_type_symbol(symbol.id, path);
                        }
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::StructMember(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        if let ResolveSymbol::Symbol(symbol) = symbol.found {
                            return traverse_type_symbol(symbol.id, path);
                        }
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::UnionMember(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        if let ResolveSymbol::Symbol(symbol) = symbol.found {
                            return traverse_type_symbol(symbol.id, path);
                        }
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::TypeDef(x) => {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        if let ResolveSymbol::Symbol(symbol) = symbol.found {
                            return traverse_type_symbol(symbol.id, path);
                        }
                    }
                } else {
                    return vec![path.clone()];
                }
            }
            SymbolKind::Parameter(x) if x.r#type.kind == TypeKind::Type => {
                if let ParameterValue::TypeExpression(TypeExpression::ScalarType(ref x)) = x.value {
                    let r#type: crate::symbol::Type = (&*x.scalar_type).into();
                    if let TypeKind::UserDefined(ref x) = r#type.kind {
                        if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                            if let ResolveSymbol::Symbol(symbol) = symbol.found {
                                return traverse_type_symbol(symbol.id, path);
                            }
                        }
                    } else {
                        return vec![path.clone()];
                    }
                }
            }
            SymbolKind::Struct(x) => {
                let mut ret = Vec::new();
                for member in &x.members {
                    let mut path = path.clone();
                    path.push(*member);
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
                    path.push(*member);
                    ret.append(&mut traverse_type_symbol(*member, &path));
                }
                return ret;
            }
            SymbolKind::ModportMember(x) if is_assignable(&x.direction) => {
                if let Ok(symbol) = symbol_table::resolve(&symbol.token) {
                    if let ResolveSymbol::Symbol(symbol) = symbol.found {
                        return traverse_type_symbol(symbol.id, path);
                    }
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

fn traverse_assignable_symbol(id: SymbolId, path: &AssignPath) -> Vec<AssignPath> {
    // check cyclic dependency
    if path.0.iter().filter(|x| **x == id).count() > 1 {
        return vec![];
    }

    if let Some(symbol) = symbol_table::get(id) {
        match &symbol.kind {
            SymbolKind::Port(x) if is_assignable(&x.direction) => {
                if let Some(ref x) = x.r#type {
                    if let TypeKind::UserDefined(ref x) = x.kind {
                        if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                            if let ResolveSymbol::Symbol(symbol) = symbol.found {
                                return traverse_type_symbol(symbol.id, path);
                            }
                        }
                    } else {
                        return vec![path.clone()];
                    }
                }
            }
            SymbolKind::Variable(x)
                if x.affiniation == VariableAffiniation::Module
                    || x.affiniation == VariableAffiniation::Function =>
            {
                if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                    if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                        if let ResolveSymbol::Symbol(symbol) = symbol.found {
                            return traverse_type_symbol(symbol.id, path);
                        }
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
                            &symbol.token,
                            x_pos.0.last().unwrap().token(),
                            y_pos.0.last().unwrap().token(),
                        ));
                    }
                }
                _ => return vec![],
            }
        }
    }

    ret
}

fn check_uncovered_branch(
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
            &symbol.token,
            &token,
        ));
    }

    ret
}
