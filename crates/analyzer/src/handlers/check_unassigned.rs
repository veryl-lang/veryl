use crate::analyzer_error::AnalyzerError;
use crate::assign::{Assign, AssignPath};
use crate::symbol::{Direction, SymbolKind};
use crate::symbol_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckUnassigned<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    in_always_comb: bool,
    in_assignment_lhs: Vec<()>,
    candicate_assignments: Vec<Assign>,
    referable_assignments: Vec<Assign>,
}

impl<'a> CheckUnassigned<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            in_always_comb: false,
            in_assignment_lhs: Vec::new(),
            candicate_assignments: Vec::new(),
            referable_assignments: Vec::new(),
        }
    }

    fn can_refer(&mut self, path: &AssignPath) -> bool {
        //  assignment is executed outside the current always_comb block
        if self.candicate_assignments.is_empty()
            || !self
                .candicate_assignments
                .iter()
                .any(|x| x.path.included(path))
        {
            return true;
        }

        self.referable_assignments
            .iter()
            .any(|x| x.path.included(path))
    }

    fn update_referable_assignments(&mut self, path: &AssignPath) {
        let assignment = self
            .candicate_assignments
            .iter()
            .find(|x| x.path.included(path));
        self.referable_assignments.push(assignment.unwrap().clone());
    }
}

impl<'a> Handler for CheckUnassigned<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl<'a> VerylGrammarTrait for CheckUnassigned<'a> {
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.candicate_assignments.clear();
                self.referable_assignments.clear();

                let assignments = symbol_table::get_assign_list();
                for x in assignments {
                    if x.position
                        .0
                        .iter()
                        .any(|x| *x.token() == arg.always_comb.always_comb_token.token)
                    {
                        self.candicate_assignments.push(x.clone());
                    }
                }
                self.in_always_comb = true;
            }
            HandlerPoint::After => self.in_always_comb = false,
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            if self.in_always_comb {
                if let IdentifierStatementGroup::Assignment(_) = &*arg.identifier_statement_group {
                    if let Ok(symbol) = symbol_table::resolve(arg.expression_identifier.as_ref()) {
                        let path = AssignPath(symbol.full_path);
                        self.update_referable_assignments(&path);
                    }
                }
            }
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            if self.in_always_comb {
                if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    let path = AssignPath(symbol.full_path);
                    self.update_referable_assignments(&path);
                }
            }
        }
        Ok(())
    }

    fn for_statement(&mut self, arg: &ForStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_always_comb {
                if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    let path = AssignPath(symbol.full_path);
                    self.update_referable_assignments(&path);
                }
            }
        }
        Ok(())
    }

    fn assignment(&mut self, _arg: &Assignment) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if self.in_always_comb {
                    self.in_assignment_lhs.push(());
                }
            }
            HandlerPoint::After => {
                if self.in_always_comb {
                    self.in_assignment_lhs.pop();
                }
            }
        }
        Ok(())
    }

    fn function_call(&mut self, _arg: &FunctionCall) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if self.in_always_comb {
                    self.in_assignment_lhs.push(());
                }
            }
            HandlerPoint::After => {
                if self.in_always_comb {
                    self.in_assignment_lhs.pop();
                }
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if !self.in_assignment_lhs.is_empty() {
                if let Ok(symbol) = symbol_table::resolve(arg) {
                    let path = AssignPath(symbol.full_path);
                    let is_referable_variable = match symbol.found.kind {
                        SymbolKind::Variable(_) => self.can_refer(&path),
                        SymbolKind::Port(x) => {
                            x.direction != Direction::Output || self.can_refer(&path)
                        }
                        _ => true,
                    };

                    if !is_referable_variable {
                        self.errors.push(AnalyzerError::unassign_variable(
                            &path.to_string(),
                            self.text,
                            &symbol.found.token.into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
