use crate::analyzer_error::AnalyzerError;
use crate::symbol::{Direction, SymbolId, SymbolKind};
use crate::symbol_table::{self, AssignPosition, AssignPositionType, ResolveSymbol};
use std::collections::HashMap;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

pub struct CheckAssignment<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    assign_position: AssignPosition,
    in_if_expression: Vec<()>,
}

impl<'a> CheckAssignment<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            assign_position: AssignPosition::default(),
            in_if_expression: Vec::new(),
        }
    }
}

impl<'a> Handler for CheckAssignment<'a> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn can_assign(full_path: &[SymbolId]) -> bool {
    if full_path.is_empty() {
        return false;
    }

    let leaf_symbol = full_path.last().unwrap();
    if let Some(leaf_symbol) = symbol_table::get(*leaf_symbol) {
        match leaf_symbol.kind {
            SymbolKind::Variable(_) => true,
            SymbolKind::StructMember(_) | SymbolKind::UnionMember(_) => {
                let root_symbol = full_path.first().unwrap();
                if let Some(root_symbol) = symbol_table::get(*root_symbol) {
                    match root_symbol.kind {
                        SymbolKind::Variable(_) => true,
                        SymbolKind::Port(x) if x.direction == Direction::Output => true,
                        SymbolKind::Port(x) if x.direction == Direction::Ref => true,
                        SymbolKind::Port(x) if x.direction == Direction::Inout => true,
                        SymbolKind::ModportMember(x) if x.direction == Direction::Output => true,
                        SymbolKind::ModportMember(x) if x.direction == Direction::Ref => true,
                        SymbolKind::ModportMember(x) if x.direction == Direction::Inout => true,
                        _ => false,
                    }
                } else {
                    false
                }
            }
            SymbolKind::Port(x) if x.direction == Direction::Output => true,
            SymbolKind::Port(x) if x.direction == Direction::Ref => true,
            SymbolKind::Port(x) if x.direction == Direction::Inout => true,
            SymbolKind::ModportMember(x) if x.direction == Direction::Output => true,
            SymbolKind::ModportMember(x) if x.direction == Direction::Ref => true,
            SymbolKind::ModportMember(x) if x.direction == Direction::Inout => true,
            _ => false,
        }
    } else {
        false
    }
}

impl<'a> VerylGrammarTrait for CheckAssignment<'a> {
    fn r#else(&mut self, arg: &Else) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_expression.is_empty() {
                let new_position = if let AssignPositionType::StatementBlock(_) =
                    self.assign_position.0.last().unwrap()
                {
                    AssignPositionType::StatementBlock(arg.else_token.token)
                } else {
                    AssignPositionType::DeclarationBlock(arg.else_token.token)
                };
                *self.assign_position.0.last_mut().unwrap() = new_position;
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
                let mut position = self.assign_position.clone();
                position.push(AssignPositionType::Statement(arg.r#let.let_token.token));
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
                            if can_assign(&full_path) {
                                symbol_table::add_assign(full_path, self.assign_position.clone());
                            } else {
                                let token =
                                    &arg.expression_identifier.identifier.identifier_token.token;
                                self.errors.push(AnalyzerError::invalid_assignment(
                                    &x.kind.to_kind_name(),
                                    self.text,
                                    &token.to_string(),
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
                self.assign_position
                    .push(AssignPositionType::StatementBlock(arg.r#if.if_token.token));
            }
            HandlerPoint::After => {
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn if_reset_statement(&mut self, arg: &IfResetStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position
                    .push(AssignPositionType::StatementBlock(
                        arg.if_reset.if_reset_token.token,
                    ));
            }
            HandlerPoint::After => {
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn for_statement(&mut self, arg: &ForStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(x) = symbol_table::resolve(arg.identifier.as_ref()) {
                let mut position = self.assign_position.clone();
                position.push(AssignPositionType::Statement(arg.r#for.for_token.token));
                symbol_table::add_assign(x.full_path, position);
            }
        }
        Ok(())
    }

    fn case_item(&mut self, arg: &CaseItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position
                    .push(AssignPositionType::StatementBlock(
                        arg.colon.colon_token.token,
                    ));
            }
            HandlerPoint::After => {
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(x) = symbol_table::resolve(arg.identifier.as_ref()) {
                let mut position = self.assign_position.clone();
                position.push(AssignPositionType::Declaration(arg.r#let.let_token.token));
                symbol_table::add_assign(x.full_path, position);
            }
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position.push(AssignPositionType::Declaration(
                    arg.always_ff.always_ff_token.token,
                ));
            }
            HandlerPoint::After => {
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position.push(AssignPositionType::Declaration(
                    arg.always_comb.always_comb_token.token,
                ));
            }
            HandlerPoint::After => {
                self.assign_position.pop();
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
                        if can_assign(&full_path) {
                            let mut position = self.assign_position.clone();
                            position.push(AssignPositionType::Declaration(
                                arg.assign.assign_token.token,
                            ));
                            symbol_table::add_assign(full_path, position);
                        } else {
                            let token = &arg
                                .hierarchical_identifier
                                .identifier
                                .identifier_token
                                .token;
                            self.errors.push(AnalyzerError::invalid_assignment(
                                &x.kind.to_kind_name(),
                                self.text,
                                &token.to_string(),
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

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(x) = symbol_table::resolve(arg.identifier.as_ref()) {
                if let ResolveSymbol::Symbol(ref symbol) = x.found {
                    if let SymbolKind::Instance(ref x) = symbol.kind {
                        // get port direction
                        let mut dirs = HashMap::new();
                        let mut dir_unknown = false;
                        if let Ok(x) = symbol_table::resolve((&x.type_name, &symbol.namespace)) {
                            if let ResolveSymbol::Symbol(ref symbol) = x.found {
                                match symbol.kind {
                                    SymbolKind::Module(ref x) => {
                                        for port in &x.ports {
                                            dirs.insert(port.name, port.property.direction);
                                        }
                                    }
                                    SymbolKind::SystemVerilog => dir_unknown = true,
                                    _ => (),
                                }
                            }
                        }

                        for (name, target) in &x.connects {
                            if !target.is_empty() {
                                let dir_output = if let Some(dir) = dirs.get(name) {
                                    matches!(
                                        dir,
                                        Direction::Ref | Direction::Inout | Direction::Output
                                    )
                                } else {
                                    false
                                };

                                if dir_output | dir_unknown {
                                    if let Ok(x) =
                                        symbol_table::resolve((target, &symbol.namespace))
                                    {
                                        let mut position = self.assign_position.clone();
                                        position.push(AssignPositionType::Declaration(
                                            arg.inst.inst_token.token,
                                        ));
                                        symbol_table::add_assign(x.full_path, position);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position.push(AssignPositionType::Declaration(
                    arg.function.function_token.token,
                ));
            }
            HandlerPoint::After => {
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn module_if_declaration(&mut self, arg: &ModuleIfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position
                    .push(AssignPositionType::DeclarationBlock(
                        arg.r#if.if_token.token,
                    ));
            }
            HandlerPoint::After => {
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn module_for_declaration(&mut self, arg: &ModuleForDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(x) = symbol_table::resolve(arg.identifier.as_ref()) {
                let mut position = self.assign_position.clone();
                position.push(AssignPositionType::Statement(arg.r#for.for_token.token));
                symbol_table::add_assign(x.full_path, position);
            }
        }
        Ok(())
    }
}
