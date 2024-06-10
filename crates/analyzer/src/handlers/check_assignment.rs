use crate::analyzer_error::AnalyzerError;
use crate::assign::{
    AssignDeclarationType, AssignPosition, AssignPositionType, AssignStatementBranchItemType,
    AssignStatementBranchType,
};
use crate::attribute::AllowItem;
use crate::attribute::Attribute as Attr;
use crate::attribute_table;
use crate::symbol::{Direction, SymbolId, SymbolKind};
use crate::symbol_table;
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
    branch_index: usize,
}

impl<'a> CheckAssignment<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            assign_position: AssignPosition::default(),
            in_if_expression: Vec::new(),
            branch_index: 0,
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

    for path in full_path {
        if let Some(symbol) = symbol_table::get(*path) {
            let can_assign = match symbol.kind {
                SymbolKind::Variable(_) => true,
                SymbolKind::Port(x) if x.direction == Direction::Output => true,
                SymbolKind::Port(x) if x.direction == Direction::Ref => true,
                SymbolKind::Port(x) if x.direction == Direction::Inout => true,
                SymbolKind::ModportVariableMember(x) if x.direction == Direction::Output => true,
                SymbolKind::ModportVariableMember(x) if x.direction == Direction::Ref => true,
                SymbolKind::ModportVariableMember(x) if x.direction == Direction::Inout => true,
                _ => false,
            };
            if can_assign {
                return true;
            }
        }
    }

    false
}

impl<'a> VerylGrammarTrait for CheckAssignment<'a> {
    fn r#else(&mut self, arg: &Else) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_expression.is_empty() {
                let new_position = if let AssignPositionType::StatementBranchItem { .. } =
                    self.assign_position.0.last().unwrap()
                {
                    AssignPositionType::StatementBranchItem {
                        token: arg.else_token.token,
                        index: self.branch_index,
                        r#type: AssignStatementBranchItemType::Else,
                    }
                } else {
                    AssignPositionType::DeclarationBranchItem {
                        token: arg.else_token.token,
                        index: self.branch_index,
                    }
                };
                *self.assign_position.0.last_mut().unwrap() = new_position;
                self.branch_index += 1;
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
                self.assign_position.push(AssignPositionType::Statement {
                    token: arg.equ.equ_token.token,
                    resettable: false,
                });
                symbol_table::add_assign(x.full_path, &self.assign_position, false);
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let IdentifierStatementGroup::Assignment(x) = &*arg.identifier_statement_group {
                let token = match x.assignment.assignment_group.as_ref() {
                    AssignmentGroup::Equ(x) => x.equ.equ_token.token,
                    AssignmentGroup::AssignmentOperator(x) => {
                        x.assignment_operator.assignment_operator_token.token
                    }
                };
                if let Ok(x) = symbol_table::resolve(arg.expression_identifier.as_ref()) {
                    let full_path = x.full_path;
                    if can_assign(&full_path) {
                        let mut partial = !arg
                            .expression_identifier
                            .expression_identifier_list
                            .is_empty();
                        partial |= arg
                            .expression_identifier
                            .expression_identifier_list0
                            .iter()
                            .any(|x| !x.expression_identifier_list0_list.is_empty());

                        self.assign_position.push(AssignPositionType::Statement {
                            token,
                            resettable: true,
                        });
                        symbol_table::add_assign(full_path, &self.assign_position, partial);
                        self.assign_position.pop();
                    } else {
                        let token = arg.expression_identifier.identifier().token;
                        self.errors.push(AnalyzerError::invalid_assignment(
                            &token.to_string(),
                            self.text,
                            &x.found.kind.to_kind_name(),
                            &arg.expression_identifier.as_ref().into(),
                        ));
                    }

                    // Check to confirm not assigning to constant
                    if let SymbolKind::Variable(v) = x.found.kind.clone() {
                        if v.r#type.is_const {
                            let token = arg.expression_identifier.identifier().token;
                            self.errors.push(AnalyzerError::invalid_assignment_to_const(
                                &token.to_string(),
                                self.text,
                                &x.found.kind.to_kind_name(),
                                &arg.expression_identifier.as_ref().into(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn if_statement(&mut self, arg: &IfStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.branch_index = 0;
                let branches = 1 + arg.if_statement_list0.len() + arg.if_statement_opt.iter().len();
                let has_default = arg.if_statement_opt.is_some();
                self.assign_position
                    .push(AssignPositionType::StatementBranch {
                        token: arg.r#if.if_token.token,
                        branches,
                        has_default,
                        allow_missing_reset_statement: false,
                        r#type: AssignStatementBranchType::If,
                    });
                self.assign_position
                    .push(AssignPositionType::StatementBranchItem {
                        token: arg.r#if.if_token.token,
                        index: self.branch_index,
                        r#type: AssignStatementBranchItemType::If,
                    });
                self.branch_index += 1;
            }
            HandlerPoint::After => {
                self.assign_position.pop();
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn if_reset_statement(&mut self, arg: &IfResetStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.branch_index = 0;
                let branches = 1
                    + arg.if_reset_statement_list0.len()
                    + arg.if_reset_statement_opt.iter().len();
                let has_default = arg.if_reset_statement_opt.is_some();
                let allow_missing_reset_statement = attribute_table::contains(
                    &arg.if_reset.if_reset_token.token,
                    Attr::Allow(AllowItem::MissingResetStatement),
                );
                self.assign_position
                    .push(AssignPositionType::StatementBranch {
                        token: arg.if_reset.if_reset_token.token,
                        branches,
                        has_default,
                        allow_missing_reset_statement,
                        r#type: AssignStatementBranchType::IfReset,
                    });
                self.assign_position
                    .push(AssignPositionType::StatementBranchItem {
                        token: arg.if_reset.if_reset_token.token,
                        index: self.branch_index,
                        r#type: AssignStatementBranchItemType::IfReset,
                    });
                self.branch_index += 1;
            }
            HandlerPoint::After => {
                self.assign_position.pop();
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn for_statement(&mut self, arg: &ForStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(x) = symbol_table::resolve(arg.identifier.as_ref()) {
                self.assign_position.push(AssignPositionType::Statement {
                    token: arg.r#for.for_token.token,
                    resettable: false,
                });
                symbol_table::add_assign(x.full_path, &self.assign_position, false);
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn case_statement(&mut self, arg: &CaseStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.branch_index = 0;
                let branches = arg.case_statement_list.len();
                let has_default = arg.case_statement_list.iter().any(|x| {
                    matches!(
                        x.case_item.case_item_group.as_ref(),
                        CaseItemGroup::Defaul(_)
                    )
                });
                self.assign_position
                    .push(AssignPositionType::StatementBranch {
                        token: arg.case.case_token.token,
                        branches,
                        has_default,
                        allow_missing_reset_statement: false,
                        r#type: AssignStatementBranchType::Case,
                    });
            }
            HandlerPoint::After => {
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn case_item(&mut self, arg: &CaseItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position
                    .push(AssignPositionType::StatementBranchItem {
                        token: arg.colon.colon_token.token,
                        index: self.branch_index,
                        r#type: AssignStatementBranchItemType::Case,
                    });
                self.branch_index += 1;
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
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.r#let.let_token.token,
                    r#type: AssignDeclarationType::Let,
                });
                symbol_table::add_assign(x.full_path, &self.assign_position, false);
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.always_ff.always_ff_token.token,
                    r#type: AssignDeclarationType::AlwaysFf,
                });
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
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.always_comb.always_comb_token.token,
                    r#type: AssignDeclarationType::AlwaysComb,
                });
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
                if can_assign(&full_path) {
                    // selected partially
                    let partial = !arg
                        .hierarchical_identifier
                        .hierarchical_identifier_list
                        .is_empty()
                        | arg
                            .hierarchical_identifier
                            .hierarchical_identifier_list0
                            .iter()
                            .any(|x| !x.hierarchical_identifier_list0_list.is_empty());

                    self.assign_position.push(AssignPositionType::Declaration {
                        token: arg.assign.assign_token.token,
                        r#type: AssignDeclarationType::Assign,
                    });
                    symbol_table::add_assign(full_path, &self.assign_position, partial);
                    self.assign_position.pop();
                } else {
                    let token = &arg
                        .hierarchical_identifier
                        .identifier
                        .identifier_token
                        .token;
                    self.errors.push(AnalyzerError::invalid_assignment(
                        &token.to_string(),
                        self.text,
                        &x.found.kind.to_kind_name(),
                        &arg.hierarchical_identifier.as_ref().into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                if let SymbolKind::Instance(ref x) = symbol.found.kind {
                    // get port direction
                    let mut dirs = HashMap::new();
                    let mut dir_unknown = false;
                    if let Ok(x) = symbol_table::resolve((&x.type_name, &symbol.found.namespace)) {
                        match x.found.kind {
                            SymbolKind::Module(ref x) => {
                                for port in &x.ports {
                                    dirs.insert(port.name, port.property.direction);
                                }
                            }
                            SymbolKind::SystemVerilog => dir_unknown = true,
                            _ => (),
                        }
                    }

                    self.assign_position.push(AssignPositionType::Declaration {
                        token: arg.inst.inst_token.token,
                        r#type: AssignDeclarationType::Inst,
                    });

                    for (token, targets) in &x.connects {
                        for target in targets {
                            if !target.is_empty() {
                                let dir_output = if let Some(dir) = dirs.get(&token.text) {
                                    matches!(
                                        dir,
                                        Direction::Ref | Direction::Inout | Direction::Output
                                    )
                                } else {
                                    false
                                };

                                if dir_output | dir_unknown {
                                    if let Ok(x) = symbol_table::resolve((
                                        &target.path(),
                                        &symbol.found.namespace,
                                    )) {
                                        self.assign_position.push(AssignPositionType::Connect {
                                            token: *token,
                                            maybe: dir_unknown,
                                        });
                                        let partial = target.is_partial();
                                        symbol_table::add_assign(
                                            x.full_path,
                                            &self.assign_position,
                                            partial,
                                        );
                                        self.assign_position.pop();
                                    }
                                }
                            }
                        }
                    }

                    self.assign_position.pop();
                }
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.function.function_token.token,
                    r#type: AssignDeclarationType::Function,
                });
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
                self.branch_index = 0;
                let branches = 1
                    + arg.module_if_declaration_list.len()
                    + arg.module_if_declaration_opt.iter().len();
                self.assign_position
                    .push(AssignPositionType::DeclarationBranch {
                        token: arg.r#if.if_token.token,
                        branches,
                    });
                self.assign_position
                    .push(AssignPositionType::DeclarationBranchItem {
                        token: arg.r#if.if_token.token,
                        index: self.branch_index,
                    });
                self.branch_index += 1;
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
                self.assign_position.push(AssignPositionType::Statement {
                    token: arg.r#for.for_token.token,
                    resettable: false,
                });
                symbol_table::add_assign(x.full_path, &self.assign_position, false);
                self.assign_position.pop();
            }
        }
        Ok(())
    }
}
