use std::collections::HashMap;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::ParolLocation;

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Location {
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub duplicated: Option<usize>,
}

impl From<&ParolLocation> for Location {
    fn from(x: &ParolLocation) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
            duplicated: None,
        }
    }
}

impl From<ParolLocation> for Location {
    fn from(x: ParolLocation) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
            duplicated: None,
        }
    }
}

#[derive(Default)]
pub struct Align {
    index: usize,
    max_width: usize,
    width: usize,
    line: usize,
    rest: Vec<(Location, usize)>,
    additions: HashMap<Location, usize>,
    last_location: Option<Location>,
}

impl Align {
    fn finish_group(&mut self) {
        for (loc, width) in &self.rest {
            self.additions.insert(*loc, self.max_width - width);
        }
        self.rest.clear();
        self.max_width = 0;
    }

    fn finish_item(&mut self) {
        let last_location = self.last_location.take();
        if let Some(loc) = last_location {
            if loc.line - self.line > 1 {
                self.finish_group();
            }
            self.max_width = usize::max(self.max_width, self.width);
            self.line = loc.line;
            self.rest.push((loc, self.width));

            self.width = 0;
            self.index += 1;
        }
    }

    fn start_item(&mut self) {
        self.width = 0;
    }

    fn token(&mut self, x: &VerylToken) {
        self.width += x.location().length;
        let loc: Location = x.location().into();
        self.last_location = Some(loc);
    }

    fn dummy_token(&mut self, x: &VerylToken) {
        self.width += 0; // 0 length token
        let loc: Location = x.location().into();
        self.last_location = Some(loc);
    }

    fn duplicated_token(&mut self, x: &VerylToken, i: usize) {
        self.width += x.location().length;
        let mut loc: Location = x.location().into();
        loc.duplicated = Some(i);
        self.last_location = Some(loc);
    }

    fn space(&mut self, x: usize) {
        self.width += x;
    }
}

mod align_kind {
    pub const IDENTIFIER: usize = 0;
    pub const TYPE: usize = 1;
    pub const EXPRESSION: usize = 2;
    pub const WIDTH: usize = 3;
    pub const ASSIGNMENT: usize = 4;
}

#[derive(Default)]
pub struct Aligner {
    pub additions: HashMap<Location, usize>,
    aligns: [Align; 5],
}

impl Aligner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn align(&mut self, input: &Veryl) {
        self.veryl(input);
        self.finish_group();
        for align in &self.aligns {
            for (x, y) in &align.additions {
                self.additions
                    .entry(*x)
                    .and_modify(|val| *val += *y)
                    .or_insert(*y);
            }
        }
    }

    fn finish_group(&mut self) {
        for i in 0..self.aligns.len() {
            self.aligns[i].finish_group();
        }
    }

    fn insert(&mut self, token: &VerylToken, width: usize) {
        let loc: Location = token.location().into();
        self.additions
            .entry(loc)
            .and_modify(|val| *val += width)
            .or_insert(width);
    }
}

impl VerylWalker for Aligner {
    /// Semantic action for non-terminal 'Identifier'
    fn identifier(&mut self, arg: &Identifier) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.aligns[align_kind::IDENTIFIER].token(&arg.identifier_token);
        self.aligns[align_kind::IDENTIFIER].finish_item();
    }

    /// Semantic action for non-terminal 'Number'
    fn number(&mut self, arg: &Number) {
        let token = match arg {
            Number::IntegralNumber(x) => match &*x.integral_number {
                IntegralNumber::Based(x) => &x.based.based_token,
                IntegralNumber::BaseLess(x) => &x.base_less.base_less_token,
                IntegralNumber::AllBit(x) => &x.all_bit.all_bit_token,
            },
            Number::RealNumber(x) => match &*x.real_number {
                RealNumber::FixedPoint(x) => &x.fixed_point.fixed_point_token,
                RealNumber::Exponent(x) => &x.exponent.exponent_token,
            },
        };
        self.aligns[align_kind::EXPRESSION].token(token);
        self.aligns[align_kind::WIDTH].token(token);
    }

    /// Semantic action for non-terminal 'Expression'
    fn expression(&mut self, arg: &Expression) {
        self.expression1(&arg.expression1);
        for x in &arg.expression_list {
            self.aligns[align_kind::EXPRESSION].space(1);
            self.aligns[align_kind::WIDTH].space(1);
            let token = match &*x.expression_list_group {
                ExpressionListGroup::BinaryOperator(x) => &x.binary_operator.binary_operator_token,
                ExpressionListGroup::CommonOperator(x) => &x.common_operator.common_operator_token,
            };
            self.aligns[align_kind::EXPRESSION].token(token);
            self.aligns[align_kind::WIDTH].token(token);
            self.aligns[align_kind::EXPRESSION].space(1);
            self.aligns[align_kind::WIDTH].space(1);
            self.expression1(&x.expression1);
        }
    }

    /// Semantic action for non-terminal 'Expression1'
    fn expression1(&mut self, arg: &Expression1) {
        if let Some(ref x) = arg.expression1_opt {
            let token = match &*x.expression1_opt_group {
                Expression1OptGroup::UnaryOperator(x) => &x.unary_operator.unary_operator_token,
                Expression1OptGroup::CommonOperator(x) => &x.common_operator.common_operator_token,
            };
            self.aligns[align_kind::EXPRESSION].token(token);
            self.aligns[align_kind::WIDTH].token(token);
        }
        self.factor(&arg.factor);
    }

    /// Semantic action for non-terminal 'Factor'
    fn factor(&mut self, arg: &Factor) {
        match arg {
            Factor::Number(x) => self.number(&x.number),
            Factor::IdentifierFactorList(x) => {
                self.aligns[align_kind::EXPRESSION].token(&x.identifier.identifier_token);
                self.aligns[align_kind::WIDTH].token(&x.identifier.identifier_token);
                for x in &x.factor_list {
                    self.range(&x.range);
                }
            }
            Factor::LParenExpressionRParen(x) => {
                self.aligns[align_kind::EXPRESSION].token(&x.l_paren.l_paren_token);
                self.aligns[align_kind::WIDTH].token(&x.l_paren.l_paren_token);
                self.expression(&x.expression);
                self.aligns[align_kind::EXPRESSION].token(&x.r_paren.r_paren_token);
                self.aligns[align_kind::WIDTH].token(&x.r_paren.r_paren_token);
            }
        }
    }

    /// Semantic action for non-terminal 'Range'
    fn range(&mut self, arg: &Range) {
        self.aligns[align_kind::EXPRESSION].token(&arg.l_bracket.l_bracket_token);
        self.aligns[align_kind::WIDTH].token(&arg.l_bracket.l_bracket_token);
        self.expression(&arg.expression);
        if let Some(ref x) = arg.range_opt {
            self.aligns[align_kind::EXPRESSION].token(&x.colon.colon_token);
            self.aligns[align_kind::WIDTH].token(&x.colon.colon_token);
            self.expression(&x.expression);
        }
        self.aligns[align_kind::EXPRESSION].token(&arg.r_bracket.r_bracket_token);
        self.aligns[align_kind::WIDTH].token(&arg.r_bracket.r_bracket_token);
    }

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        self.aligns[align_kind::EXPRESSION].token(&arg.l_bracket.l_bracket_token);
        self.aligns[align_kind::WIDTH].token(&arg.l_bracket.l_bracket_token);
        self.expression(&arg.expression);
        self.aligns[align_kind::EXPRESSION].space("-1:0".len());
        self.aligns[align_kind::WIDTH].space("-1:0".len());
        self.aligns[align_kind::EXPRESSION].token(&arg.r_bracket.r_bracket_token);
        self.aligns[align_kind::WIDTH].token(&arg.r_bracket.r_bracket_token);
    }

    /// Semantic action for non-terminal 'Type'
    fn r#type(&mut self, arg: &Type) {
        let token = match &*arg.type_group {
            TypeGroup::BuiltinType(x) => match &*x.builtin_type {
                BuiltinType::Logic(x) => x.logic.logic_token.clone(),
                BuiltinType::Bit(x) => x.bit.bit_token.clone(),
                BuiltinType::U32(x) => x.u32.u32_token.replace("unsigned int"),
                BuiltinType::U64(x) => x.u64.u64_token.replace("unsigned longint"),
                BuiltinType::I32(x) => x.i32.i32_token.replace("signed int"),
                BuiltinType::I64(x) => x.i64.i64_token.replace("signed longint"),
                BuiltinType::F32(x) => x.f32.f32_token.replace("shortreal"),
                BuiltinType::F64(x) => x.f64.f64_token.replace("real"),
            },
            TypeGroup::Identifier(x) => x.identifier.identifier_token.clone(),
        };
        self.aligns[align_kind::TYPE].start_item();
        self.aligns[align_kind::TYPE].token(&token);
        self.aligns[align_kind::TYPE].finish_item();

        if arg.type_list.is_empty() {
            self.aligns[align_kind::WIDTH].start_item();
            self.aligns[align_kind::WIDTH].dummy_token(&token);
            self.aligns[align_kind::WIDTH].finish_item();
        } else {
            self.aligns[align_kind::WIDTH].start_item();
            for x in &arg.type_list {
                self.width(&x.width);
            }
            self.aligns[align_kind::WIDTH].finish_item();
        }
    }

    /// Semantic action for non-terminal 'AssignmentStatement'
    fn assignment_statement(&mut self, arg: &AssignmentStatement) {
        self.identifier(&arg.identifier);
        let token = match &*arg.assignment_statement_group {
            AssignmentStatementGroup::Equ(x) => &x.equ.equ_token,
            AssignmentStatementGroup::AssignmentOperator(x) => {
                &x.assignment_operator.assignment_operator_token
            }
        };
        self.aligns[align_kind::ASSIGNMENT].start_item();
        self.aligns[align_kind::ASSIGNMENT].token(token);
        self.aligns[align_kind::ASSIGNMENT].finish_item();
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        for x in &arg.if_statement_list {
            self.statement(&x.statement);
        }
        for x in &arg.if_statement_list0 {
            for x in &x.if_statement_list0_list {
                self.statement(&x.statement);
            }
        }
        if let Some(ref x) = arg.if_statement_opt {
            for x in &x.if_statement_opt_list {
                self.statement(&x.statement);
            }
        }
    }

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        for x in &arg.if_reset_statement_list {
            self.statement(&x.statement);
        }
        for x in &arg.if_reset_statement_list0 {
            for x in &x.if_reset_statement_list0_list {
                self.statement(&x.statement);
            }
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            for x in &x.if_reset_statement_opt_list {
                self.statement(&x.statement);
            }
        }
    }

    /// Semantic action for non-terminal 'ForStatement'
    fn for_statement(&mut self, arg: &ForStatement) {
        for x in &arg.for_statement_list {
            self.statement(&x.statement);
        }
    }

    /// Semantic action for non-terminal 'LetDeclaration'
    fn let_declaration(&mut self, arg: &LetDeclaration) {
        match &*arg.let_declaration_group {
            LetDeclarationGroup::VariableDeclaration(x) => {
                let x = &x.variable_declaration;
                self.identifier(&arg.identifier);
                self.r#type(&x.r#type);
            }
            LetDeclarationGroup::InstanceDeclaration(x) => {
                let x = &x.instance_declaration;
                if let Some(ref x) = x.instance_declaration_opt0 {
                    self.instance_parameter(&x.instance_parameter);
                }
                if let Some(ref x) = x.instance_declaration_opt1 {
                    if let Some(ref x) = x.instance_declaration_opt2 {
                        self.instance_port_list(&x.instance_port_list);
                    }
                }
            }
        }
    }

    /// Semantic action for non-terminal 'ParameterDeclaration'
    fn parameter_declaration(&mut self, arg: &ParameterDeclaration) {
        self.insert(&arg.parameter.parameter_token, 1);
        self.identifier(&arg.identifier);
        self.r#type(&arg.r#type);
    }

    /// Semantic action for non-terminal 'LocalparamDeclaration'
    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) {
        self.identifier(&arg.identifier);
        self.r#type(&arg.r#type);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, _arg: &AlwaysFfDeclaration) {}

    /// Semantic action for non-terminal 'ModportDeclaration'
    fn modport_declaration(&mut self, arg: &ModportDeclaration) {
        self.modport_list(&arg.modport_list);
    }

    /// Semantic action for non-terminal 'EnumDeclaration'
    fn enum_declaration(&mut self, arg: &EnumDeclaration) {
        self.enum_list(&arg.enum_list);
    }

    /// Semantic action for non-terminal 'StructDeclaration'
    fn struct_declaration(&mut self, arg: &StructDeclaration) {
        self.struct_list(&arg.struct_list);
    }

    /// Semantic action for non-terminal 'InstanceParameterItem'
    fn instance_parameter_item(&mut self, arg: &InstanceParameterItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_parameter_item_opt {
            self.aligns[align_kind::EXPRESSION].start_item();
            self.expression(&x.expression);
            self.aligns[align_kind::EXPRESSION].finish_item();
        } else {
            self.aligns[align_kind::EXPRESSION].start_item();
            self.aligns[align_kind::EXPRESSION]
                .duplicated_token(&arg.identifier.identifier_token, 0);
            self.aligns[align_kind::EXPRESSION].finish_item();
        }
    }

    /// Semantic action for non-terminal 'InstancePortItem'
    fn instance_port_item(&mut self, arg: &InstancePortItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_port_item_opt {
            self.aligns[align_kind::EXPRESSION].start_item();
            self.expression(&x.expression);
            self.aligns[align_kind::EXPRESSION].finish_item();
        } else {
            self.aligns[align_kind::EXPRESSION].start_item();
            self.aligns[align_kind::EXPRESSION]
                .duplicated_token(&arg.identifier.identifier_token, 0);
            self.aligns[align_kind::EXPRESSION].finish_item();
        }
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::Parameter(x) => {
                self.insert(&x.parameter.parameter_token, 1);
            }
            WithParameterItemGroup::Localparam(_) => (),
        }
        self.identifier(&arg.identifier);
        self.r#type(&arg.r#type);
        self.aligns[align_kind::EXPRESSION].start_item();
        self.expression(&arg.expression);
        self.aligns[align_kind::EXPRESSION].finish_item();
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        if let Some(ref x) = arg.function_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
        }
        for x in &arg.function_declaration_list {
            self.function_item(&x.function_item);
        }
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        if let Some(ref x) = arg.module_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.module_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
        }
        for x in &arg.module_declaration_list {
            self.module_item(&x.module_item);
        }
    }

    /// Semantic action for non-terminal 'ModuleIfDeclaration'
    fn module_if_declaration(&mut self, arg: &ModuleIfDeclaration) {
        for x in &arg.module_if_declaration_list {
            self.module_item(&x.module_item);
        }
        for x in &arg.module_if_declaration_list0 {
            for x in &x.module_if_declaration_list0_list {
                self.module_item(&x.module_item);
            }
        }
        if let Some(ref x) = arg.module_if_declaration_opt0 {
            for x in &x.module_if_declaration_opt0_list {
                self.module_item(&x.module_item);
            }
        }
    }

    /// Semantic action for non-terminal 'ModuleForDeclaration'
    fn module_for_declaration(&mut self, arg: &ModuleForDeclaration) {
        for x in &arg.module_for_declaration_list {
            self.module_item(&x.module_item);
        }
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        match arg {
            Direction::Input(x) => {
                self.insert(&x.input.input_token, 1);
            }
            Direction::Output(_) => (),
            Direction::Inout(x) => {
                self.insert(&x.inout.inout_token, 1);
            }
            Direction::Ref(x) => {
                self.insert(&x.r#ref.ref_token, 3);
            }
        }
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        if let Some(ref x) = arg.interface_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        for x in &arg.interface_declaration_list {
            self.interface_item(&x.interface_item);
        }
    }

    /// Semantic action for non-terminal 'InterfaceIfDeclaration'
    fn interface_if_declaration(&mut self, arg: &InterfaceIfDeclaration) {
        for x in &arg.interface_if_declaration_list {
            self.interface_item(&x.interface_item);
        }
        for x in &arg.interface_if_declaration_list0 {
            for x in &x.interface_if_declaration_list0_list {
                self.interface_item(&x.interface_item);
            }
        }
        if let Some(ref x) = arg.interface_if_declaration_opt0 {
            for x in &x.interface_if_declaration_opt0_list {
                self.interface_item(&x.interface_item);
            }
        }
    }

    /// Semantic action for non-terminal 'InterfaceForDeclaration'
    fn interface_for_declaration(&mut self, arg: &InterfaceForDeclaration) {
        for x in &arg.interface_for_declaration_list {
            self.interface_item(&x.interface_item);
        }
    }
}
