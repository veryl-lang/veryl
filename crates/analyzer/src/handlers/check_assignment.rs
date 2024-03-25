use crate::analyzer_error::AnalyzerError;
use crate::symbol::Direction as SymDirection;
use crate::symbol::{Symbol, SymbolKind};
use crate::symbol_table::{self, ResolveSymbol};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckAssignment<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    current_position: Vec<VerylToken>,
    in_if_expression: Vec<()>,
}

impl<'a> CheckAssignment<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            current_position: Vec::new(),
            in_if_expression: Vec::new(),
        }
    }
}

impl<'a> Handler for CheckAssignment<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn can_assign(arg: &Symbol) -> bool {
    match &arg.kind {
        SymbolKind::Variable(_) => true,
        SymbolKind::StructMember(_) => true,
        SymbolKind::UnionMember(_) => true,
        SymbolKind::Port(x) if x.direction == SymDirection::Output => true,
        SymbolKind::Port(x) if x.direction == SymDirection::Ref => true,
        SymbolKind::Port(x) if x.direction == SymDirection::Inout => true,
        SymbolKind::ModportMember(x) if x.direction == SymDirection::Output => true,
        SymbolKind::ModportMember(x) if x.direction == SymDirection::Ref => true,
        SymbolKind::ModportMember(x) if x.direction == SymDirection::Inout => true,
        _ => false,
    }
}

impl<'a> VerylGrammarTrait for CheckAssignment<'a> {
    fn r#else(&mut self, arg: &Else) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_expression.is_empty() {
                *self.current_position.last_mut().unwrap() = arg.else_token.clone();
            }
        }
        Ok(())
    }

    fn if_expression(&mut self, _arg: &IfExpression) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_if_expression.push(());
            }
            HandlerPoint::After => {
                self.in_if_expression.pop();
            }
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(x) = symbol_table::resolve(arg.identifier.as_ref()) {
                let mut position = self.current_position.clone();
                position.push(arg.r#let.let_token.clone());
                symbol_table::add_assign(x.full_path, position);
            }
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let IdentifierStatementGroup::Assignment(_) = &*arg.identifier_statement_group {
                if let Ok(x) = symbol_table::resolve(arg.expression_identifier.as_ref()) {
                    let full_path = x.full_path;
                    match x.found {
                        ResolveSymbol::Symbol(x) => {
                            if can_assign(&x) {
                                symbol_table::add_assign(full_path, self.current_position.clone());
                            } else {
                                let token = &arg.expression_identifier.identifier.identifier_token;
                                self.errors.push(AnalyzerError::invalid_assignment(
                                    &x.kind.to_kind_name(),
                                    self.text,
                                    &token.text(),
                                    token,
                                ));
                            }
                        }
                        // External symbol can't be checkd
                        ResolveSymbol::External => (),
                    }
                }
            }
        }
        Ok(())
    }

    fn if_statement(&mut self, arg: &IfStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.current_position.push(arg.r#if.if_token.clone());
            }
            HandlerPoint::After => {
                self.current_position.pop();
            }
        }
        Ok(())
    }

    fn if_reset_statement(&mut self, arg: &IfResetStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.current_position
                    .push(arg.if_reset.if_reset_token.clone());
            }
            HandlerPoint::After => {
                self.current_position.pop();
            }
        }
        Ok(())
    }

    fn case_item(&mut self, arg: &CaseItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.current_position.push(arg.colon.colon_token.clone());
            }
            HandlerPoint::After => {
                self.current_position.pop();
            }
        }
        Ok(())
    }

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(x) = symbol_table::resolve(arg.identifier.as_ref()) {
                let mut position = self.current_position.clone();
                position.push(arg.r#let.let_token.clone());
                symbol_table::add_assign(x.full_path, position);
            }
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.current_position
                    .push(arg.always_ff.always_ff_token.clone());
            }
            HandlerPoint::After => {
                self.current_position.pop();
            }
        }
        Ok(())
    }

    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.current_position
                    .push(arg.always_comb.always_comb_token.clone());
            }
            HandlerPoint::After => {
                self.current_position.pop();
            }
        }
        Ok(())
    }

    fn assign_declaration(&mut self, arg: &AssignDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(x) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
                let full_path = x.full_path;
                match x.found {
                    ResolveSymbol::Symbol(x) => {
                        if can_assign(&x) {
                            let mut position = self.current_position.clone();
                            position.push(arg.assign.assign_token.clone());
                            symbol_table::add_assign(full_path, position);
                        } else {
                            let token = &arg.hierarchical_identifier.identifier.identifier_token;
                            self.errors.push(AnalyzerError::invalid_assignment(
                                &x.kind.to_kind_name(),
                                self.text,
                                &token.text(),
                                token,
                            ));
                        }
                    }
                    // External symbol can't be checkd
                    ResolveSymbol::External => (),
                }
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.current_position
                    .push(arg.function.function_token.clone());
            }
            HandlerPoint::After => {
                self.current_position.pop();
            }
        }
        Ok(())
    }

    fn module_if_declaration(&mut self, arg: &ModuleIfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.current_position.push(arg.r#if.if_token.clone());
            }
            HandlerPoint::After => {
                self.current_position.pop();
            }
        }
        Ok(())
    }
}
