use crate::analyzer::resource_table::PathId;
use crate::analyzer_error::AnalyzerError;
use crate::assign::{AssignPath, AssignPosition, AssignPositionTree, AssignPositionType};
use crate::attribute_table;
use crate::handlers::*;
use crate::msb_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{
    Direction, DocComment, ParameterValue, Symbol, SymbolId, SymbolKind, TypeKind,
    VariableAffiniation,
};
use crate::symbol_path::SymbolPath;
use crate::symbol_table;
use crate::type_dag;
use itertools::Itertools;
use std::path::Path;
use veryl_metadata::{Lint, Metadata};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenSource};
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

pub struct AnalyzerPass3<'a> {
    path: PathId,
    text: &'a str,
    symbols: Vec<Symbol>,
}

impl<'a> AnalyzerPass3<'a> {
    pub fn new(path: &'a Path, text: &'a str) -> Self {
        let symbols = symbol_table::get_all();
        let path = resource_table::get_path_id(path.to_path_buf()).unwrap();
        AnalyzerPass3 {
            path,
            text,
            symbols,
        }
    }

    pub fn check_variables(&self) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        for symbol in &self.symbols {
            if symbol.token.source == self.path {
                if let SymbolKind::Variable(ref x) = symbol.kind {
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
                    if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                        if let Ok(x) =
                            symbol_table::resolve((&SymbolPath::new(x), &symbol.namespace))
                        {
                            match x.found.kind {
                                SymbolKind::Enum(_)
                                | SymbolKind::Union(_)
                                | SymbolKind::Struct(_)
                                | SymbolKind::TypeDef(_)
                                | SymbolKind::SystemVerilog => (),
                                SymbolKind::Parameter(x) if x.r#type.kind == TypeKind::Type => (),
                                _ => {
                                    ret.push(AnalyzerError::mismatch_type(
                                        &x.found.token.to_string(),
                                        "enum or union or struct",
                                        &x.found.kind.to_kind_name(),
                                        self.text,
                                        &symbol.token.into(),
                                    ));
                                }
                            }
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
                if must_be_assigned(&symbol.kind) {
                    let path: Vec<_> = path
                        .0
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

            let symbol = symbol_table::get(*path.0.first().unwrap()).unwrap();

            if positions.len() > 1 {
                for comb in positions.iter().combinations(2) {
                    ret.append(&mut check_multiple_assignment(
                        &symbol, self.text, comb[0], comb[1],
                    ));
                }
            }

            ret.append(&mut check_assign_position_tree(
                &symbol, self.text, positions,
            ));
        }

        ret
    }
}

pub struct Analyzer {
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
        let pass3 = AnalyzerPass3::new(path.as_ref(), text);
        ret.append(&mut pass3.check_variables());
        ret.append(&mut pass3.check_assignment());

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
        SymbolKind::ModportMember(x) => x.direction == Direction::Output,
        SymbolKind::Variable(_) => true,
        SymbolKind::StructMember(_) => true,
        _ => false,
    }
}

fn traverse_type_symbol(id: SymbolId, path: &AssignPath) -> Vec<AssignPath> {
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
                if let ParameterValue::TypeExpression(TypeExpression::ScalarType(ref x)) = x.value {
                    let r#type: crate::symbol::Type = (&*x.scalar_type).into();
                    if let TypeKind::UserDefined(ref x) = r#type.kind {
                        if let Ok(symbol) = symbol_table::resolve((x, &symbol.namespace)) {
                            return traverse_type_symbol(symbol.found.id, path);
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
                            return traverse_type_symbol(symbol.found.id, path);
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
