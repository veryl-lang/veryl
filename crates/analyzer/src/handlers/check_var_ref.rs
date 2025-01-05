use crate::analyzer_error::AnalyzerError;
use crate::attribute::Attribute as Attr;
use crate::attribute::{AllowItem, CondTypeItem};
use crate::attribute_table;
use crate::symbol::{Direction, Symbol, SymbolId, SymbolKind, TypeKind};
use crate::symbol_table;
use crate::var_ref::{
    AssignDeclarationType, AssignPosition, AssignPositionType, AssignStatementBranchItemType,
    AssignStatementBranchType, ExpressionTargetType, VarRef, VarRefAffiliation, VarRefPath,
    VarRefType,
};
use std::collections::HashMap;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

struct FunctionCallContext {
    pub token: Token,
    pub port_directions: Vec<Direction>,
}

pub struct CheckVarRef<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    affiliation: Vec<VarRefAffiliation>,
    assign_position: AssignPosition,
    in_expression: Vec<bool>,
    in_if_expression: Vec<()>,
    function_call: Vec<Option<FunctionCallContext>>,
    branch_index: usize,
}

impl<'a> CheckVarRef<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            errors: Vec::new(),
            text,
            point: HandlerPoint::Before,
            affiliation: Vec::new(),
            assign_position: AssignPosition::default(),
            in_expression: Vec::new(),
            in_if_expression: Vec::new(),
            function_call: Vec::new(),
            branch_index: 0,
        }
    }

    fn add_assign(&mut self, path: &VarRefPath) {
        let r#type = VarRefType::AssignTarget {
            position: self.assign_position.clone(),
        };
        let assign = VarRef {
            r#type,
            affiliation: *self.affiliation.last().unwrap(),
            path: path.clone(),
        };
        symbol_table::add_var_ref(&assign);
        self.assign_position.pop();
    }

    fn add_expression(&mut self, path: &VarRefPath, r#type: ExpressionTargetType) {
        let r#type = VarRefType::ExpressionTarget { r#type };
        let expression = VarRef {
            r#type,
            affiliation: *self.affiliation.last().unwrap(),
            path: path.clone(),
        };
        symbol_table::add_var_ref(&expression);
    }

    fn push_function_call(&mut self, identifier: &ExpressionIdentifier) {
        if let Ok(func) = symbol_table::resolve(identifier) {
            let ports = match func.found.kind {
                SymbolKind::Function(x) => x.ports,
                SymbolKind::ModportFunctionMember(x) => symbol_table::get(x.function)
                    .map(|x| {
                        if let SymbolKind::Function(x) = x.kind {
                            x.ports
                        } else {
                            unreachable!()
                        }
                    })
                    .unwrap(),
                _ => Vec::new(),
            };
            let port_directions: Vec<_> = ports
                .iter()
                .rev()
                .map(|port| {
                    if let SymbolKind::Port(port) = symbol_table::get(port.symbol).unwrap().kind {
                        port.direction
                    } else {
                        unreachable!()
                    }
                })
                .collect();
            let context = FunctionCallContext {
                token: func.found.token,
                port_directions,
            };
            self.function_call.push(Some(context));
        } else {
            self.function_call.push(None);
        }
    }
}

impl Handler for CheckVarRef<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn is_assignable_symbol(symbol: &Symbol) -> bool {
    match &symbol.kind {
        SymbolKind::Variable(_) => true,
        SymbolKind::Port(x) if x.direction == Direction::Output => true,
        SymbolKind::Port(x) if x.direction == Direction::Ref => true,
        SymbolKind::Port(x) if x.direction == Direction::Inout => true,
        SymbolKind::ModportVariableMember(x) if x.direction == Direction::Output => true,
        SymbolKind::ModportVariableMember(x) if x.direction == Direction::Ref => true,
        SymbolKind::ModportVariableMember(x) if x.direction == Direction::Inout => true,
        _ => false,
    }
}

fn can_assign(full_path: &[SymbolId]) -> bool {
    if full_path.is_empty() {
        return false;
    }

    for path in full_path {
        if let Some(symbol) = symbol_table::get(*path) {
            if is_assignable_symbol(&symbol) {
                return true;
            }
        }
    }

    false
}

fn has_cond_type(token: &Token) -> bool {
    let mut attrs = attribute_table::get(token);
    attrs.reverse();
    for attr in attrs {
        match attr {
            Attr::CondType(CondTypeItem::None) => return false,
            Attr::CondType(_) => return true,
            _ => (),
        }
    }
    false
}

fn map_assignable_factor(arg: &Expression) -> Option<VarRefPath> {
    if !arg.expression_list.is_empty() {
        return None;
    }

    let exp = &*arg.expression01;
    if !exp.expression01_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression02;
    if !exp.expression02_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression03;
    if !exp.expression03_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression04;
    if !exp.expression04_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression05;
    if !exp.expression05_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression06;
    if !exp.expression06_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression07;
    if !exp.expression07_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression08;
    if !exp.expression08_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression09;
    if !exp.expression09_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression10;
    if !exp.expression10_list.is_empty() {
        return None;
    }

    let exp = &*exp.expression11;
    if exp.expression11_opt.is_some() {
        return None;
    }

    let exp = &*exp.expression12;
    if !exp.expression12_list.is_empty() {
        return None;
    }

    if let Factor::IdentifierFactor(factor) = &*exp.factor {
        if factor.identifier_factor.identifier_factor_opt.is_none() {
            if let Ok(symbol) =
                symbol_table::resolve(factor.identifier_factor.expression_identifier.as_ref())
            {
                if is_assignable_symbol(&symbol.found) {
                    let path = VarRefPath::try_from(
                        factor.identifier_factor.expression_identifier.as_ref(),
                    )
                    .unwrap();
                    return Some(path);
                }
            }
        }
    }

    None
}

impl VerylGrammarTrait for CheckVarRef<'_> {
    fn r#else(&mut self, arg: &Else) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if self.in_if_expression.is_empty() {
                let position = if let AssignPositionType::StatementBranchItem { .. } =
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
                *self.assign_position.0.last_mut().unwrap() = position;
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

    fn assignment(&mut self, _arg: &Assignment) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.in_expression.push(true);
            }
            HandlerPoint::After => {
                self.in_expression.pop();
            }
        }
        Ok(())
    }

    fn identifier_factor(&mut self, arg: &IdentifierFactor) -> Result<(), ParolError> {
        if arg.identifier_factor_opt.is_some() {
            match self.point {
                HandlerPoint::Before => self.push_function_call(&arg.expression_identifier),
                HandlerPoint::After => {
                    self.function_call.pop();
                }
            }
        }
        Ok(())
    }

    fn argument_item(&mut self, arg: &ArgumentItem) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if let Some(function_call) = self.function_call.last_mut().unwrap() {
                    let direction = function_call
                        .port_directions
                        .pop()
                        .unwrap_or(Direction::Input);
                    if !matches!(
                        direction,
                        Direction::Output | Direction::Inout | Direction::Ref
                    ) {
                        self.in_expression.push(true);
                    } else if let Some(path) = map_assignable_factor(&arg.expression) {
                        self.assign_position.push(AssignPositionType::Statement {
                            token: function_call.token,
                            resettable: false,
                        });
                        self.add_assign(&path);
                        self.in_expression.push(false);
                    } else {
                        self.in_expression.push(true);
                    }
                } else {
                    // unassignable expression is connected with an output param.
                    // direction mismatch error should be raised.
                    self.in_expression.push(true);
                }
            }
            HandlerPoint::After => {
                self.in_expression.pop();
            }
        }
        Ok(())
    }

    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if *self.in_expression.last().unwrap_or(&false) {
                if let Ok(path) = VarRefPath::try_from(arg) {
                    let full_path = path.full_path();
                    let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();
                    let r#type = match symbol.kind {
                        SymbolKind::Variable(_) => ExpressionTargetType::Variable,
                        SymbolKind::Parameter(_) => ExpressionTargetType::Parameter,
                        SymbolKind::Port(x) => match x.direction {
                            Direction::Input => ExpressionTargetType::InputPort,
                            Direction::Output => ExpressionTargetType::OutputPort,
                            Direction::Inout => ExpressionTargetType::InoutPort,
                            Direction::Ref => ExpressionTargetType::RefPort,
                            _ => return Ok(()),
                        },
                        _ => return Ok(()),
                    };
                    self.add_expression(&path, r#type);
                }
            }
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            if let Ok(path) = VarRefPath::try_from(arg.identifier.as_ref()) {
                self.assign_position.push(AssignPositionType::Statement {
                    token: arg.equ.equ_token.token,
                    resettable: false,
                });
                self.add_assign(&path);
            }
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                if matches!(
                    &*arg.identifier_statement_group,
                    IdentifierStatementGroup::FunctionCall(_)
                ) {
                    self.push_function_call(arg.expression_identifier.as_ref());
                }
            }
            HandlerPoint::After => {
                match &*arg.identifier_statement_group {
                    IdentifierStatementGroup::Assignment(x) => {
                        let token = match x.assignment.assignment_group.as_ref() {
                            AssignmentGroup::Equ(x) => x.equ.equ_token.token,
                            AssignmentGroup::AssignmentOperator(x) => {
                                x.assignment_operator.assignment_operator_token.token
                            }
                        };
                        if let Ok(path) = VarRefPath::try_from(arg.expression_identifier.as_ref()) {
                            let full_path = path.full_path();
                            let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();

                            if can_assign(full_path) {
                                self.assign_position.push(AssignPositionType::Statement {
                                    token,
                                    resettable: true,
                                });
                                self.add_assign(&path);
                            } else {
                                let token = arg.expression_identifier.identifier().token;
                                self.errors.push(AnalyzerError::invalid_assignment(
                                    &token.to_string(),
                                    self.text,
                                    &symbol.kind.to_kind_name(),
                                    &arg.expression_identifier.as_ref().into(),
                                ));
                            }

                            // Check to confirm not assigning to constant
                            if let SymbolKind::Variable(v) = symbol.kind.clone() {
                                if v.r#type.is_const {
                                    let token = arg.expression_identifier.identifier().token;
                                    self.errors.push(AnalyzerError::invalid_assignment_to_const(
                                        &token.to_string(),
                                        self.text,
                                        &symbol.kind.to_kind_name(),
                                        &arg.expression_identifier.as_ref().into(),
                                    ));
                                }
                            }
                        }
                    }
                    IdentifierStatementGroup::FunctionCall(_) => {
                        self.function_call.pop();
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
                let branches = 1 + arg.if_statement_list.len() + arg.if_statement_opt.iter().len();
                let has_explicit_default = arg.if_statement_opt.is_some();
                let has_cond_type = has_cond_type(&arg.r#if.if_token.token);
                let has_default = has_explicit_default | has_cond_type;
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
                let branches =
                    1 + arg.if_reset_statement_list.len() + arg.if_reset_statement_opt.iter().len();
                let has_explicit_default = arg.if_reset_statement_opt.is_some();
                let has_cond_type = has_cond_type(&arg.if_reset.if_reset_token.token);
                let has_default = has_explicit_default | has_cond_type;
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
            if let Ok(path) = VarRefPath::try_from(arg.identifier.as_ref()) {
                self.assign_position.push(AssignPositionType::Statement {
                    token: arg.r#for.for_token.token,
                    resettable: false,
                });
                self.add_assign(&path);
            }
        }
        Ok(())
    }

    fn case_statement(&mut self, arg: &CaseStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.branch_index = 0;
                let branches = arg.case_statement_list.len();
                let has_explicit_default = arg.case_statement_list.iter().any(|x| {
                    matches!(
                        x.case_item.case_item_group.as_ref(),
                        CaseItemGroup::Defaul(_)
                    )
                });
                let has_cond_type = has_cond_type(&arg.case.case_token.token);
                let has_default = has_explicit_default | has_cond_type;
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
        if let HandlerPoint::After = self.point {
            if let Ok(path) = VarRefPath::try_from(arg.identifier.as_ref()) {
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.r#let.let_token.token,
                    r#type: AssignDeclarationType::Let,
                });
                self.add_assign(&path);
            }
        }
        Ok(())
    }

    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.affiliation.push(VarRefAffiliation::AlwaysFF {
                    token: arg.always_ff.always_ff_token.token,
                });
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.always_ff.always_ff_token.token,
                    r#type: AssignDeclarationType::AlwaysFF,
                });
            }
            HandlerPoint::After => {
                self.affiliation.pop();
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.affiliation.push(VarRefAffiliation::AlwaysComb {
                    token: arg.always_comb.always_comb_token.token,
                });
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.always_comb.always_comb_token.token,
                    r#type: AssignDeclarationType::AlwaysComb,
                });
            }
            HandlerPoint::After => {
                self.affiliation.pop();
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn assign_declaration(&mut self, arg: &AssignDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            if let Ok(path) = VarRefPath::try_from(arg.hierarchical_identifier.as_ref()) {
                let full_path = path.full_path();
                if can_assign(full_path) {
                    self.assign_position.push(AssignPositionType::Declaration {
                        token: arg.assign.assign_token.token,
                        r#type: AssignDeclarationType::Assign,
                    });
                    self.add_assign(&path);
                } else {
                    let token = &arg
                        .hierarchical_identifier
                        .identifier
                        .identifier_token
                        .token;
                    let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();
                    self.errors.push(AnalyzerError::invalid_assignment(
                        &token.to_string(),
                        self.text,
                        &symbol.kind.to_kind_name(),
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
                    let mut ports = HashMap::new();
                    let mut port_unknown = false;
                    let mut sv_instance = false;

                    if let Ok(x) = symbol_table::resolve((
                        &x.type_name.mangled_path(),
                        &symbol.found.namespace,
                    )) {
                        match x.found.kind {
                            SymbolKind::Module(ref x) => {
                                for port in &x.ports {
                                    ports.insert(port.name(), port.property());
                                }
                            }
                            SymbolKind::SystemVerilog => {
                                port_unknown = true;
                                sv_instance = true;
                            }
                            // TODO this should be removed after implementing bounded generic
                            // parameter
                            SymbolKind::GenericParameter(_) => port_unknown = true,
                            _ => (),
                        }
                    }

                    self.assign_position.push(AssignPositionType::Declaration {
                        token: arg.inst.inst_token.token,
                        r#type: AssignDeclarationType::Inst,
                    });

                    for (token, targets) in &x.connects {
                        for target in targets {
                            if let Ok(path) =
                                VarRefPath::try_from((target, &symbol.found.namespace))
                            {
                                let full_path = path.full_path();
                                let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();

                                // Check assignment from output port
                                let dir_output = if let Some(port) = ports.get(&token.text) {
                                    matches!(
                                        port.direction,
                                        Direction::Ref | Direction::Inout | Direction::Output
                                    )
                                } else {
                                    false
                                };

                                if dir_output | port_unknown {
                                    self.assign_position.push(AssignPositionType::Connect {
                                        token: *token,
                                        maybe: port_unknown,
                                    });
                                    self.add_assign(&path);
                                }

                                // Check assignment of clock/reset type
                                let (is_clock, is_reset) =
                                    if let Some(port) = ports.get(&token.text) {
                                        if let Some(x) = &port.r#type {
                                            (x.kind.is_clock(), x.kind.is_reset())
                                        } else {
                                            (false, false)
                                        }
                                    } else {
                                        (false, false)
                                    };

                                if is_clock && !symbol.kind.is_clock() {
                                    self.errors.push(AnalyzerError::mismatch_type(
                                        &token.text.to_string(),
                                        "clock type",
                                        "non-clock type",
                                        self.text,
                                        &token.into(),
                                    ));
                                }

                                if is_reset && !symbol.kind.is_reset() {
                                    self.errors.push(AnalyzerError::mismatch_type(
                                        &token.text.to_string(),
                                        "reset type",
                                        "non-reset type",
                                        self.text,
                                        &token.into(),
                                    ));
                                }

                                // Check implicit reset to SV instance
                                let is_implicit_reset = match &symbol.kind {
                                    SymbolKind::Port(x) => {
                                        if let Some(x) = &x.r#type {
                                            x.kind == TypeKind::Reset
                                        } else {
                                            false
                                        }
                                    }
                                    SymbolKind::Variable(x) => x.r#type.kind == TypeKind::Reset,
                                    _ => false,
                                };

                                if sv_instance && is_implicit_reset {
                                    self.errors.push(AnalyzerError::sv_with_implicit_reset(
                                        self.text,
                                        &token.into(),
                                    ));
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
                self.affiliation.push(VarRefAffiliation::Function {
                    token: arg.function.function_token.token,
                });
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.function.function_token.token,
                    r#type: AssignDeclarationType::Function,
                });
            }
            HandlerPoint::After => {
                self.affiliation.pop();
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn generate_if_declaration(&mut self, arg: &GenerateIfDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.branch_index = 0;
                let branches = 1
                    + arg.generate_if_declaration_list.len()
                    + arg.generate_if_declaration_opt.iter().len();
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

    fn generate_for_declaration(&mut self, arg: &GenerateForDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let Ok(path) = VarRefPath::try_from(arg.identifier.as_ref()) {
                self.assign_position.push(AssignPositionType::Statement {
                    token: arg.r#for.for_token.token,
                    resettable: false,
                });
                self.add_assign(&path);
            }
        }
        Ok(())
    }

    fn module_declaration(&mut self, arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.affiliation.push(VarRefAffiliation::Module {
                    token: arg.module.module_token.token,
                });
            }
            HandlerPoint::After => {
                self.affiliation.pop();
            }
        }
        Ok(())
    }

    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.affiliation.push(VarRefAffiliation::Interface {
                    token: arg.interface.interface_token.token,
                });
            }
            HandlerPoint::After => {
                self.affiliation.pop();
            }
        }
        Ok(())
    }
}
