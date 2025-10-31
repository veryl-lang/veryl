use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::attribute::Attribute as Attr;
use crate::attribute::{AllowItem, CondTypeItem};
use crate::attribute_table;
use crate::connect_operation_table;
use crate::evaluator::Evaluator;
use crate::namespace::Namespace;
use crate::symbol::{Direction, PortProperty, Symbol, SymbolId, SymbolKind};
use crate::symbol_path::GenericSymbolPath;
use crate::symbol_table;
use crate::var_ref::{
    AssignDeclarationType, AssignPosition, AssignPositionType, AssignStatementBranchItemType,
    AssignStatementBranchType, ExpressionTargetType, VarRef, VarRefAffiliation, VarRefPath,
    VarRefType,
};
use veryl_parser::ParolError;
use veryl_parser::resource_table::StrId;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

struct FunctionCallContext {
    pub token: Token,
    pub port_directions: Vec<Direction>,
}

#[derive(Default)]
pub struct CheckVarRef {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    affiliation: Vec<VarRefAffiliation>,
    assign_position: AssignPosition,
    in_expression: Vec<bool>,
    in_if_expression: Vec<()>,
    function_call: Vec<Option<FunctionCallContext>>,
    branch_group_index: usize,
    branch_group: Vec<usize>,
    branch_index: Vec<usize>,
}

impl CheckVarRef {
    pub fn new() -> Self {
        Self::default()
    }

    fn add_assign(&mut self, path: &VarRefPath) {
        let r#type = VarRefType::AssignTarget {
            position: self.assign_position.clone(),
        };
        let assign = VarRef {
            r#type,
            affiliation: *self.affiliation.last().unwrap(),
            path: path.clone(),
            branch_group: self.branch_group.clone(),
            branch_index: self.branch_index.clone(),
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
            branch_group: self.branch_group.clone(),
            branch_index: self.branch_index.clone(),
        };
        symbol_table::add_var_ref(&expression);
    }

    fn push_function_call(&mut self, identifier: &ExpressionIdentifier) {
        if let Ok(func) = symbol_table::resolve(identifier) {
            let ports = match func.found.kind {
                SymbolKind::Function(x) => x.ports,
                SymbolKind::SystemFunction(x) => x.ports,
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

    fn push_branch(&mut self) {
        self.branch_group.push(self.branch_group_index);
        self.branch_group_index += 1;
        self.branch_index.push(0);
    }

    fn pop_branch(&mut self) {
        self.branch_group.pop();
        self.branch_index.pop();
    }

    fn get_branch_index(&self) -> usize {
        *self.branch_index.last().unwrap_or(&0)
    }

    fn inc_branch_index(&mut self) {
        *self.branch_index.last_mut().unwrap() += 1;
    }

    fn check_inst(&mut self, header_token: &Token, arg: &ComponentInstantiation) {
        if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref())
            && let SymbolKind::Instance(ref x) = symbol.found.kind
        {
            let (sv_instance, port_unknown, ports) =
                Self::resolve_inst_target(&x.type_name, &symbol.found.namespace);
            self.assign_position.push(AssignPositionType::Declaration {
                token: *header_token,
                define_context: (*header_token).into(),
                r#type: AssignDeclarationType::Inst,
            });

            let mut evaluator = Evaluator::new(&[]);

            for (token, target) in &x.port_connects {
                // Gather port information
                let dir_output = if let Some(port) = ports.get(&token.text) {
                    matches!(port.direction, Direction::Inout | Direction::Output)
                } else {
                    false
                };
                let (is_clock, is_reset) = if let Some(port) = ports.get(&token.text) {
                    (port.r#type.kind.is_clock(), port.r#type.kind.is_reset())
                } else {
                    (false, false)
                };

                let exp = evaluator.expression(&target.expression);

                // Check assignment of clock/reset type
                if is_clock && !(exp.is_fixed() || exp.is_clock()) {
                    self.errors.push(AnalyzerError::mismatch_type(
                        &token.text.to_string(),
                        "clock type",
                        "non-clock type",
                        &token.into(),
                    ));
                }

                if is_reset && !(exp.is_fixed() || exp.is_reset()) {
                    self.errors.push(AnalyzerError::mismatch_type(
                        &token.text.to_string(),
                        "reset type",
                        "non-reset type",
                        &token.into(),
                    ));
                }

                // Check implicit reset to SV instance
                if sv_instance && exp.is_reset() && !exp.is_explicit_reset() {
                    self.errors
                        .push(AnalyzerError::sv_with_implicit_reset(&token.into()));
                }

                // Check output to non-assignable variable
                if dir_output && !target.expression.is_assignable() {
                    self.errors
                        .push(AnalyzerError::unassignable_output(&token.into()));
                }

                // Check assignment from output port
                for target in &target.identifiers {
                    if let Ok(path) = VarRefPath::try_from((target, &symbol.found.namespace))
                        && (dir_output | port_unknown)
                    {
                        self.assign_position.push(AssignPositionType::Connect {
                            token: *token,
                            define_context: (*token).into(),
                            maybe: port_unknown,
                        });
                        self.add_assign(&path);
                    }
                }
            }

            self.assign_position.pop();
        }
    }

    fn resolve_inst_target(
        path: &GenericSymbolPath,
        namespace: &Namespace,
    ) -> (bool, bool, HashMap<StrId, PortProperty>) {
        let mut ports = HashMap::default();

        let Ok(symbol) = symbol_table::resolve((&path.mangled_path(), namespace)) else {
            return (false, false, ports);
        };

        match &symbol.found.kind {
            SymbolKind::Module(x) => {
                for port in &x.ports {
                    ports.insert(port.name(), port.property());
                }
            }
            SymbolKind::ProtoModule(x) => {
                for port in &x.ports {
                    ports.insert(port.name(), port.property());
                }
            }
            SymbolKind::AliasModule(x) | SymbolKind::ProtoAliasModule(x) => {
                return Self::resolve_inst_target(&x.target, &symbol.found.namespace);
            }
            SymbolKind::GenericInstance(x) => {
                let base = symbol_table::get(x.base).unwrap();
                if let SymbolKind::Module(ref x) = base.kind {
                    for port in &x.ports {
                        ports.insert(port.name(), port.property());
                    }
                }
            }
            SymbolKind::GenericParameter(x) => {
                if let Some(proto) = x.bound.resolve_proto_bound(&symbol.found.namespace)
                    && let Some(SymbolKind::ProtoModule(x)) = proto.get_symbol().map(|x| x.kind)
                {
                    for port in &x.ports {
                        ports.insert(port.name(), port.property());
                    }
                }
            }
            SymbolKind::SystemVerilog => return (true, true, ports),
            _ => {}
        }

        (false, false, ports)
    }
}

impl Handler for CheckVarRef {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

fn is_assignable_symbol(symbol: &Symbol) -> bool {
    match &symbol.kind {
        SymbolKind::Variable(_) => true,
        SymbolKind::Port(x) if x.direction == Direction::Output => true,
        SymbolKind::Port(x) if x.direction == Direction::Inout => true,
        SymbolKind::ModportVariableMember(x) if x.direction == Direction::Output => true,
        SymbolKind::ModportVariableMember(x) if x.direction == Direction::Inout => true,
        _ => false,
    }
}

fn can_assign(full_path: &[SymbolId]) -> bool {
    if full_path.is_empty() {
        return false;
    }

    for path in full_path {
        if let Some(symbol) = symbol_table::get(*path)
            && is_assignable_symbol(&symbol)
        {
            return true;
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
    if let Some(Factor::IdentifierFactor(x)) = arg.unwrap_factor()
        && x.identifier_factor.identifier_factor_opt.is_none()
        && let Ok(symbol) =
            symbol_table::resolve(x.identifier_factor.expression_identifier.as_ref())
        && is_assignable_symbol(&symbol.found)
    {
        let path =
            VarRefPath::try_from(x.identifier_factor.expression_identifier.as_ref()).unwrap();
        return Some(path);
    }

    None
}

impl VerylGrammarTrait for CheckVarRef {
    fn r#else(&mut self, arg: &Else) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && self.in_if_expression.is_empty()
        {
            let position = if let AssignPositionType::StatementBranchItem { .. } =
                self.assign_position.0.last().unwrap()
            {
                AssignPositionType::StatementBranchItem {
                    token: arg.else_token.token,
                    define_context: arg.else_token.token.into(),
                    index: self.get_branch_index(),
                    r#type: AssignStatementBranchItemType::Else,
                }
            } else {
                AssignPositionType::DeclarationBranchItem {
                    token: arg.else_token.token,
                    define_context: arg.else_token.token.into(),
                    index: self.get_branch_index(),
                }
            };
            *self.assign_position.0.last_mut().unwrap() = position;
            self.inc_branch_index();
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
                    if !matches!(direction, Direction::Output | Direction::Inout) {
                        self.in_expression.push(true);
                    } else if let Some(path) =
                        map_assignable_factor(&arg.argument_expression.expression)
                    {
                        self.assign_position.push(AssignPositionType::Statement {
                            token: function_call.token,
                            define_context: function_call.token.into(),
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
        if let HandlerPoint::Before = self.point
            && *self.in_expression.last().unwrap_or(&false)
            && let Ok(path) = VarRefPath::try_from(arg)
        {
            let full_path = path.full_path();
            let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();
            let r#type = match symbol.kind {
                SymbolKind::Variable(_) => ExpressionTargetType::Variable,
                SymbolKind::Parameter(_) => ExpressionTargetType::Parameter,
                SymbolKind::Port(x) => match x.direction {
                    Direction::Input => ExpressionTargetType::InputPort,
                    Direction::Output => ExpressionTargetType::OutputPort,
                    Direction::Inout => ExpressionTargetType::InoutPort,
                    _ => return Ok(()),
                },
                _ => return Ok(()),
            };
            self.add_expression(&path, r#type);
        }
        Ok(())
    }

    fn let_statement(&mut self, arg: &LetStatement) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point
            && let Ok(path) = VarRefPath::try_from(arg.identifier.as_ref())
        {
            self.assign_position.push(AssignPositionType::Statement {
                token: arg.equ.equ_token.token,
                define_context: arg.equ.equ_token.token.into(),
                resettable: false,
            });
            self.add_assign(&path);
        }
        Ok(())
    }

    fn identifier_statement(&mut self, arg: &IdentifierStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => match &*arg.identifier_statement_group {
                IdentifierStatementGroup::Assignment(x) => {
                    if let AssignmentGroup::DiamondOperator(_) = *x.assignment.assignment_group {
                        let token = arg.expression_identifier.identifier().token;
                        if let Some(operation) = connect_operation_table::get(&token) {
                            for (path, r#type) in operation.get_expression_paths() {
                                self.add_expression(&path, r#type);
                            }
                        }
                    }
                }
                IdentifierStatementGroup::FunctionCall(_) => {
                    self.push_function_call(arg.expression_identifier.as_ref());
                }
            },
            HandlerPoint::After => {
                match &*arg.identifier_statement_group {
                    IdentifierStatementGroup::Assignment(x) => {
                        let assignment = x.assignment.assignment_group.as_ref();
                        if let AssignmentGroup::DiamondOperator(x) = assignment {
                            let token = arg.expression_identifier.identifier().token;
                            if let Some(operation) = connect_operation_table::get(&token) {
                                for path in operation.get_assign_paths() {
                                    self.assign_position.push(AssignPositionType::Statement {
                                        token,
                                        define_context: x
                                            .diamond_operator
                                            .diamond_operator_token
                                            .token
                                            .into(),
                                        resettable: true,
                                    });
                                    self.add_assign(&path);
                                }
                            }
                        } else {
                            let token = match assignment {
                                AssignmentGroup::Equ(x) => x.equ.equ_token.token,
                                AssignmentGroup::AssignmentOperator(x) => {
                                    x.assignment_operator.assignment_operator_token.token
                                }
                                _ => unreachable!(),
                            };
                            if let Ok(path) =
                                VarRefPath::try_from(arg.expression_identifier.as_ref())
                            {
                                let full_path = path.full_path();
                                let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();

                                if can_assign(full_path) {
                                    self.assign_position.push(AssignPositionType::Statement {
                                        token,
                                        define_context: token.into(),
                                        resettable: true,
                                    });
                                    self.add_assign(&path);
                                } else {
                                    let token = arg.expression_identifier.identifier().token;
                                    self.errors.push(AnalyzerError::invalid_assignment(
                                        &token.to_string(),
                                        &symbol.kind.to_kind_name(),
                                        &arg.expression_identifier.as_ref().into(),
                                    ));
                                }

                                // Check to confirm not assigning to constant
                                if let SymbolKind::Variable(v) = symbol.kind.clone()
                                    && v.r#type.is_const
                                {
                                    let token = arg.expression_identifier.identifier().token;
                                    self.errors.push(AnalyzerError::invalid_assignment_to_const(
                                        &token.to_string(),
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
                self.push_branch();
                let branches = 1 + arg.if_statement_list.len() + arg.if_statement_opt.iter().len();
                let has_explicit_default = arg.if_statement_opt.is_some();
                let has_cond_type = has_cond_type(&arg.r#if.if_token.token);
                let has_default = has_explicit_default | has_cond_type;
                self.assign_position
                    .push(AssignPositionType::StatementBranch {
                        token: arg.r#if.if_token.token,
                        define_context: arg.r#if.if_token.token.into(),
                        branches,
                        has_default,
                        allow_missing_reset_statement: false,
                        r#type: AssignStatementBranchType::If,
                    });
                self.assign_position
                    .push(AssignPositionType::StatementBranchItem {
                        token: arg.r#if.if_token.token,
                        define_context: arg.r#if.if_token.token.into(),
                        index: self.get_branch_index(),
                        r#type: AssignStatementBranchItemType::If,
                    });
                self.inc_branch_index();
            }
            HandlerPoint::After => {
                self.pop_branch();
                self.assign_position.pop();
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn if_reset_statement(&mut self, arg: &IfResetStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.push_branch();
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
                        define_context: arg.if_reset.if_reset_token.token.into(),
                        branches,
                        has_default,
                        allow_missing_reset_statement,
                        r#type: AssignStatementBranchType::IfReset,
                    });
                self.assign_position
                    .push(AssignPositionType::StatementBranchItem {
                        token: arg.if_reset.if_reset_token.token,
                        define_context: arg.if_reset.if_reset_token.token.into(),
                        index: self.get_branch_index(),
                        r#type: AssignStatementBranchItemType::IfReset,
                    });
                self.inc_branch_index();
            }
            HandlerPoint::After => {
                self.pop_branch();
                self.assign_position.pop();
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn for_statement(&mut self, arg: &ForStatement) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && let Ok(path) = VarRefPath::try_from(arg.identifier.as_ref())
        {
            self.assign_position.push(AssignPositionType::Statement {
                token: arg.r#for.for_token.token,
                define_context: arg.r#for.for_token.token.into(),
                resettable: false,
            });
            self.add_assign(&path);
        }
        Ok(())
    }

    fn case_statement(&mut self, arg: &CaseStatement) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.push_branch();
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
                        define_context: arg.case.case_token.token.into(),
                        branches,
                        has_default,
                        allow_missing_reset_statement: false,
                        r#type: AssignStatementBranchType::Case,
                    });
            }
            HandlerPoint::After => {
                self.pop_branch();
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
                        define_context: arg.colon.colon_token.token.into(),
                        index: self.get_branch_index(),
                        r#type: AssignStatementBranchItemType::Case,
                    });
                self.inc_branch_index();
            }
            HandlerPoint::After => {
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn let_declaration(&mut self, arg: &LetDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point
            && let Ok(path) = VarRefPath::try_from(arg.identifier.as_ref())
        {
            self.assign_position.push(AssignPositionType::Declaration {
                token: arg.r#let.let_token.token,
                define_context: arg.r#let.let_token.token.into(),
                r#type: AssignDeclarationType::Let,
            });
            self.add_assign(&path);
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
                    define_context: arg.always_ff.always_ff_token.token.into(),
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
                    define_context: arg.always_comb.always_comb_token.token.into(),
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
            let idents: Vec<_> = arg.assign_destination.as_ref().into();
            for ident in &idents {
                if let Ok(path) = VarRefPath::try_from(ident) {
                    let full_path = path.full_path();
                    if can_assign(full_path) {
                        self.assign_position.push(AssignPositionType::Declaration {
                            token: arg.assign.assign_token.token,
                            define_context: arg.assign.assign_token.token.into(),
                            r#type: AssignDeclarationType::Assign,
                        });
                        self.add_assign(&path);
                    } else {
                        let token = &ident.identifier.identifier_token.token;
                        let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();
                        self.errors.push(AnalyzerError::invalid_assignment(
                            &token.to_string(),
                            &symbol.kind.to_kind_name(),
                            &ident.into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn connect_declaration(&mut self, arg: &ConnectDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            let token = arg
                .hierarchical_identifier
                .identifier
                .identifier_token
                .token;
            let operation = connect_operation_table::get(&token);
            if operation.is_none() {
                return Ok(());
            }

            for path in operation.unwrap().get_assign_paths() {
                self.assign_position.push(AssignPositionType::Declaration {
                    token: arg.connect.connect_token.token,
                    define_context: arg.connect.connect_token.token.into(),
                    r#type: AssignDeclarationType::Assign,
                });
                self.add_assign(&path);
            }
        }
        Ok(())
    }

    fn inst_declaration(&mut self, arg: &InstDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point {
            // Mangled module is also resolved during resolving the inst symbol.
            // To get correct result, this statement should be executed after resolving generic parameters.
            self.check_inst(&arg.inst.inst_token.token, &arg.component_instantiation);
        }
        Ok(())
    }

    fn bind_declaration(&mut self, arg: &BindDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::After = self.point
            && let Ok(_) = symbol_table::resolve(arg.scoped_identifier.as_ref())
        {
            self.check_inst(&arg.bind.bind_token.token, &arg.component_instantiation);
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
                    define_context: arg.function.function_token.token.into(),
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
                self.push_branch();
                let branches = 1
                    + arg.generate_if_declaration_list.len()
                    + arg.generate_if_declaration_opt.iter().len();
                self.assign_position
                    .push(AssignPositionType::DeclarationBranch {
                        token: arg.r#if.if_token.token,
                        define_context: arg.r#if.if_token.token.into(),
                        branches,
                    });
                self.assign_position
                    .push(AssignPositionType::DeclarationBranchItem {
                        token: arg.r#if.if_token.token,
                        define_context: arg.r#if.if_token.token.into(),
                        index: self.get_branch_index(),
                    });
                self.inc_branch_index();
            }
            HandlerPoint::After => {
                self.pop_branch();
                self.assign_position.pop();
            }
        }
        Ok(())
    }

    fn generate_for_declaration(&mut self, arg: &GenerateForDeclaration) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point
            && let Ok(path) = VarRefPath::try_from(arg.identifier.as_ref())
        {
            self.assign_position.push(AssignPositionType::Statement {
                token: arg.r#for.for_token.token,
                define_context: arg.r#for.for_token.token.into(),
                resettable: false,
            });
            self.add_assign(&path);
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

    fn package_declaration(&mut self, arg: &PackageDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.affiliation.push(VarRefAffiliation::Package {
                    token: arg.package.package_token.token,
                });
            }
            HandlerPoint::After => {
                self.affiliation.pop();
            }
        }
        Ok(())
    }

    fn proto_module_declaration(&mut self, arg: &ProtoModuleDeclaration) -> Result<(), ParolError> {
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

    fn proto_interface_declaration(
        &mut self,
        arg: &ProtoInterfaceDeclaration,
    ) -> Result<(), ParolError> {
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

    fn proto_package_declaration(
        &mut self,
        arg: &ProtoPackageDeclaration,
    ) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => {
                self.affiliation.push(VarRefAffiliation::Package {
                    token: arg.package.package_token.token,
                });
            }
            HandlerPoint::After => {
                self.affiliation.pop();
            }
        }
        Ok(())
    }
}
