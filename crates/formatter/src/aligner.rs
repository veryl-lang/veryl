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
}

impl From<&ParolLocation> for Location {
    fn from(x: &ParolLocation) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
        }
    }
}

impl From<ParolLocation> for Location {
    fn from(x: ParolLocation) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
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
    last_token: Option<VerylToken>,
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
        let last_token = self.last_token.take();
        if let Some(last_token) = last_token {
            let loc: Location = (&last_token.token.token.location).into();
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
        self.width += x.token.token.location.length;
        self.last_token = Some(x.clone());
    }

    fn dummy_token(&mut self, x: &VerylToken) {
        self.width += 0; // 0 length token
        self.last_token = Some(x.clone());
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
}

#[derive(Default)]
pub struct Aligner {
    pub additions: HashMap<Location, usize>,
    aligns: [Align; 4],
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
        let loc: Location = (&token.token.token.location).into();
        self.additions
            .entry(loc)
            .and_modify(|val| *val += width)
            .or_insert(width);
    }
}

impl VerylWalker for Aligner {
    // ----------------------------------------------------------------------------
    // Terminals
    // ----------------------------------------------------------------------------

    fn identifier(&mut self, input: &Identifier) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.aligns[align_kind::IDENTIFIER].token(&input.identifier_token);
        self.aligns[align_kind::IDENTIFIER].finish_item();
    }

    // ----------------------------------------------------------------------------
    // SourceCode
    // ----------------------------------------------------------------------------

    fn veryl(&mut self, input: &Veryl) {
        for x in &input.veryl_list {
            self.description(&x.description);
        }
    }

    // ----------------------------------------------------------------------------
    // Description
    // ----------------------------------------------------------------------------

    fn description(&mut self, input: &Description) {
        match input {
            Description::Description0(x) => self.module_declaration(&x.module_declaration),
            Description::Description1(x) => self.interface_declaration(&x.interface_declaration),
        }
    }

    // ----------------------------------------------------------------------------
    // Number
    // ----------------------------------------------------------------------------

    fn number(&mut self, input: &Number) {
        let token = match input {
            Number::Number0(x) => match &*x.integral_number {
                IntegralNumber::IntegralNumber0(x) => &x.based.based_token,
                IntegralNumber::IntegralNumber1(x) => &x.base_less.base_less_token,
                IntegralNumber::IntegralNumber2(x) => &x.all_bit.all_bit_token,
            },
            Number::Number1(x) => match &*x.real_number {
                RealNumber::RealNumber0(x) => &x.fixed_point.fixed_point_token,
                RealNumber::RealNumber1(x) => &x.exponent.exponent_token,
            },
        };
        self.aligns[align_kind::EXPRESSION].token(token);
        self.aligns[align_kind::WIDTH].token(token);
    }

    // ----------------------------------------------------------------------------
    // Expression
    // ----------------------------------------------------------------------------

    fn expression(&mut self, input: &Expression) {
        self.expression1(&input.expression1);
        for x in &input.expression_list {
            self.aligns[align_kind::EXPRESSION].space(1);
            self.aligns[align_kind::WIDTH].space(1);
            let token = match &*x.expression_list_group {
                ExpressionListGroup::ExpressionListGroup0(x) => {
                    &x.binary_operator.binary_operator_token
                }
                ExpressionListGroup::ExpressionListGroup1(x) => {
                    &x.common_operator.common_operator_token
                }
            };
            self.aligns[align_kind::EXPRESSION].token(token);
            self.aligns[align_kind::WIDTH].token(token);
            self.aligns[align_kind::EXPRESSION].space(1);
            self.aligns[align_kind::WIDTH].space(1);
            self.expression1(&x.expression1);
        }
    }

    fn expression1(&mut self, input: &Expression1) {
        if let Some(ref x) = input.expression1_opt {
            let token = match &*x.expression1_opt_group {
                Expression1OptGroup::Expression1OptGroup0(x) => {
                    &x.unary_operator.unary_operator_token
                }
                Expression1OptGroup::Expression1OptGroup1(x) => {
                    &x.common_operator.common_operator_token
                }
            };
            self.aligns[align_kind::EXPRESSION].token(token);
            self.aligns[align_kind::WIDTH].token(token);
        }
        self.factor(&input.factor);
    }

    fn factor(&mut self, input: &Factor) {
        match input {
            Factor::Factor0(x) => self.number(&x.number),
            Factor::Factor1(x) => {
                self.aligns[align_kind::EXPRESSION].token(&x.identifier.identifier_token);
                self.aligns[align_kind::WIDTH].token(&x.identifier.identifier_token);
                for x in &x.factor_list {
                    self.range(&x.range);
                }
            }
            Factor::Factor2(x) => {
                self.aligns[align_kind::EXPRESSION].token(&x.l_paren.l_paren_token);
                self.aligns[align_kind::WIDTH].token(&x.l_paren.l_paren_token);
                self.expression(&x.expression);
                self.aligns[align_kind::EXPRESSION].token(&x.r_paren.r_paren_token);
                self.aligns[align_kind::WIDTH].token(&x.r_paren.r_paren_token);
            }
        }
    }

    // ----------------------------------------------------------------------------
    // Statement
    // ----------------------------------------------------------------------------

    fn statement(&mut self, input: &Statement) {
        match input {
            Statement::Statement0(x) => self.assignment_statement(&x.assignment_statement),
            Statement::Statement1(x) => self.if_statement(&x.if_statement),
        }
    }

    fn assignment_statement(&mut self, input: &AssignmentStatement) {
        self.identifier(&input.identifier);
    }

    fn if_statement(&mut self, _input: &IfStatement) {}

    // ----------------------------------------------------------------------------
    // Range / Width
    // ----------------------------------------------------------------------------

    fn range(&mut self, input: &Range) {
        self.aligns[align_kind::EXPRESSION].token(&input.l_bracket.l_bracket_token);
        self.aligns[align_kind::WIDTH].token(&input.l_bracket.l_bracket_token);
        self.expression(&input.expression);
        if let Some(ref x) = input.range_opt {
            self.aligns[align_kind::EXPRESSION].token(&x.colon.colon_token);
            self.aligns[align_kind::WIDTH].token(&x.colon.colon_token);
            self.expression(&x.expression);
        }
        self.aligns[align_kind::EXPRESSION].token(&input.r_bracket.r_bracket_token);
        self.aligns[align_kind::WIDTH].token(&input.r_bracket.r_bracket_token);
    }

    fn width(&mut self, input: &Width) {
        self.aligns[align_kind::EXPRESSION].token(&input.l_bracket.l_bracket_token);
        self.aligns[align_kind::WIDTH].token(&input.l_bracket.l_bracket_token);
        self.expression(&input.expression);
        self.aligns[align_kind::EXPRESSION].token(&input.r_bracket.r_bracket_token);
        self.aligns[align_kind::WIDTH].token(&input.r_bracket.r_bracket_token);
    }

    // ----------------------------------------------------------------------------
    // Type
    // ----------------------------------------------------------------------------

    fn r#type(&mut self, input: &Type) {
        let token = match &*input.type_group {
            TypeGroup::TypeGroup0(x) => match &*x.builtin_type {
                BuiltinType::BuiltinType0(x) => &x.logic.logic_token,
                BuiltinType::BuiltinType1(x) => &x.bit.bit_token,
                BuiltinType::BuiltinType2(x) => &x.u32.u32_token,
                BuiltinType::BuiltinType3(x) => &x.u64.u64_token,
                BuiltinType::BuiltinType4(x) => &x.i32.i32_token,
                BuiltinType::BuiltinType5(x) => &x.i64.i64_token,
                BuiltinType::BuiltinType6(x) => &x.f32.f32_token,
                BuiltinType::BuiltinType7(x) => &x.f64.f64_token,
            },
            TypeGroup::TypeGroup1(x) => &x.identifier.identifier_token,
        };
        self.aligns[align_kind::TYPE].start_item();
        self.aligns[align_kind::TYPE].token(token);
        self.aligns[align_kind::TYPE].finish_item();

        if input.type_list.is_empty() {
            self.aligns[align_kind::WIDTH].start_item();
            self.aligns[align_kind::WIDTH].dummy_token(token);
            self.aligns[align_kind::WIDTH].finish_item();
        } else {
            self.aligns[align_kind::WIDTH].start_item();
            for x in &input.type_list {
                self.width(&x.width);
            }
            self.aligns[align_kind::WIDTH].finish_item();
        }
    }

    // ----------------------------------------------------------------------------
    // WithParameter
    // ----------------------------------------------------------------------------

    fn with_parameter(&mut self, input: &WithParameter) {
        if let Some(ref x) = input.with_parameter_opt {
            self.with_parameter_list(&x.with_parameter_list);
        }
    }

    fn with_parameter_list(&mut self, input: &WithParameterList) {
        self.with_parameter_item(&input.with_parameter_item);
        for x in &input.with_parameter_list_list {
            self.with_parameter_item(&x.with_parameter_item);
        }
    }

    fn with_parameter_item(&mut self, input: &WithParameterItem) {
        match &*input.with_parameter_item_group {
            WithParameterItemGroup::WithParameterItemGroup0(x) => {
                self.insert(&x.parameter.parameter_token, 1);
            }
            WithParameterItemGroup::WithParameterItemGroup1(_) => (),
        }
        self.identifier(&input.identifier);
        self.r#type(&input.r#type);
        self.aligns[align_kind::EXPRESSION].start_item();
        self.expression(&input.expression);
        self.aligns[align_kind::EXPRESSION].finish_item();
    }

    // ----------------------------------------------------------------------------
    // Module
    // ----------------------------------------------------------------------------

    fn module_declaration(&mut self, input: &ModuleDeclaration) {
        if let Some(ref x) = input.module_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = input.module_declaration_opt0 {
            self.module_port(&x.module_port);
        }
        for x in &input.module_declaration_list {
            self.module_item(&x.module_item);
        }
    }

    fn module_port(&mut self, input: &ModulePort) {
        if let Some(ref x) = input.module_port_opt {
            self.module_port_list(&x.module_port_list);
        }
    }

    fn module_port_list(&mut self, input: &ModulePortList) {
        self.module_port_item(&input.module_port_item);
        for x in &input.module_port_list_list {
            self.module_port_item(&x.module_port_item);
        }
    }

    fn module_port_item(&mut self, input: &ModulePortItem) {
        self.identifier(&input.identifier);
        self.direction(&input.direction);
        self.r#type(&input.r#type);
    }

    fn module_item(&mut self, input: &ModuleItem) {
        match input {
            ModuleItem::ModuleItem0(x) => self.variable_declaration(&x.variable_declaration),
            ModuleItem::ModuleItem1(x) => self.parameter_declaration(&x.parameter_declaration),
            ModuleItem::ModuleItem2(x) => self.localparam_declaration(&x.localparam_declaration),
            ModuleItem::ModuleItem3(x) => self.always_ff_declaration(&x.always_ff_declaration),
            ModuleItem::ModuleItem4(x) => self.always_comb_declaration(&x.always_comb_declaration),
            ModuleItem::ModuleItem5(x) => self.assign_declaration(&x.assign_declaration),
        }
    }

    fn direction(&mut self, input: &Direction) {
        match input {
            Direction::Direction0(x) => {
                self.insert(&x.input.input_token, 1);
            }
            Direction::Direction1(_) => (),
            Direction::Direction2(x) => {
                self.insert(&x.inout.inout_token, 1);
            }
        }
    }

    // ----------------------------------------------------------------------------
    // Interface
    // ----------------------------------------------------------------------------

    fn interface_declaration(&mut self, input: &InterfaceDeclaration) {
        if let Some(ref x) = input.interface_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        for x in &input.interface_declaration_list {
            self.interface_item(&x.interface_item);
        }
    }

    fn interface_item(&mut self, input: &InterfaceItem) {
        match input {
            InterfaceItem::InterfaceItem0(x) => self.variable_declaration(&x.variable_declaration),
            InterfaceItem::InterfaceItem1(x) => {
                self.parameter_declaration(&x.parameter_declaration)
            }
            InterfaceItem::InterfaceItem2(x) => {
                self.localparam_declaration(&x.localparam_declaration)
            }
            InterfaceItem::InterfaceItem3(x) => self.modport_declaration(&x.modport_declaration),
        }
    }

    // ----------------------------------------------------------------------------
    // Declaration
    // ----------------------------------------------------------------------------

    fn variable_declaration(&mut self, input: &VariableDeclaration) {
        self.identifier(&input.identifier);
        self.r#type(&input.r#type);
    }

    fn parameter_declaration(&mut self, input: &ParameterDeclaration) {
        self.insert(&input.parameter.parameter_token, 1);
        self.identifier(&input.identifier);
        self.r#type(&input.r#type);
    }

    fn localparam_declaration(&mut self, input: &LocalparamDeclaration) {
        self.identifier(&input.identifier);
        self.r#type(&input.r#type);
    }

    fn always_ff_declaration(&mut self, _input: &AlwaysFfDeclaration) {}

    fn always_ff_conditions(&mut self, _input: &AlwaysFfConditions) {}

    fn always_ff_condition(&mut self, _input: &AlwaysFfCondition) {}

    fn always_comb_declaration(&mut self, input: &AlwaysCombDeclaration) {
        for x in &input.always_comb_declaration_list {
            self.statement(&x.statement);
        }
    }

    fn assign_declaration(&mut self, input: &AssignDeclaration) {
        self.identifier(&input.identifier);
        if let Some(ref x) = input.assign_declaration_opt {
            self.r#type(&x.r#type);
        }
        self.expression(&input.expression);
    }

    fn modport_declaration(&mut self, input: &ModportDeclaration) {
        self.identifier(&input.identifier);
        self.modport_list(&input.modport_list);
    }

    fn modport_list(&mut self, input: &ModportList) {
        self.modport_item(&input.modport_item);
        for x in &input.modport_list_list {
            self.modport_item(&x.modport_item);
        }
    }

    fn modport_item(&mut self, input: &ModportItem) {
        self.identifier(&input.identifier);
        self.direction(&input.direction);
    }
}
