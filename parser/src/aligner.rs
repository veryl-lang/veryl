use crate::veryl_grammar_trait::*;
use crate::veryl_token::VerylToken;
use crate::veryl_walker::VerylWalker;
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Location {
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

impl From<&parol_runtime::lexer::Location> for Location {
    fn from(x: &parol_runtime::lexer::Location) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
        }
    }
}

impl From<parol_runtime::lexer::Location> for Location {
    fn from(x: parol_runtime::lexer::Location) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
        }
    }
}

pub struct Align {
    index: usize,
    max_width: usize,
    width: usize,
    line: usize,
    rest: Vec<(Location, usize)>,
    widths: HashMap<Location, usize>,
    last_token: Option<VerylToken>,
}

impl Default for Align {
    fn default() -> Self {
        Self {
            index: 0,
            max_width: 0,
            width: 0,
            line: 0,
            rest: Vec::new(),
            widths: HashMap::new(),
            last_token: None,
        }
    }
}

impl Align {
    fn reset(&mut self) {
        for (loc, width) in &self.rest {
            self.widths.insert(*loc, self.max_width - width);
        }
        self.rest.clear();
        self.max_width = 0;
    }

    fn update(&mut self, x: &VerylToken) {
        let loc: Location = (&x.token.token.location).into();
        if loc.line - self.line > 1 {
            self.reset();
        }
        self.max_width = usize::max(self.max_width, self.width);
        self.line = loc.line;
        self.rest.push((loc, self.width));

        self.width = 0;
        self.index += 1;
    }

    fn update_with_last(&mut self) {
        let last_token = self.last_token.take();
        if let Some(last_token) = last_token {
            self.update(&last_token);
        }
    }

    fn reset_width(&mut self) {
        self.width = 0;
    }

    fn width(&mut self, x: &VerylToken) {
        self.width += x.token.token.location.length;
        self.last_token = Some(x.clone());
    }

    fn space(&mut self, x: usize) {
        self.width += x;
    }
}

pub struct Aligner {
    pub widths: HashMap<Location, usize>,
    align_identifier: Align,
    align_type: Align,
    align_expression: Align,
    align_width: Align,
}

impl Default for Aligner {
    fn default() -> Self {
        Self {
            align_identifier: Default::default(),
            align_type: Default::default(),
            align_expression: Default::default(),
            align_width: Default::default(),
            widths: HashMap::new(),
        }
    }
}

impl Aligner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn align(&mut self, input: &Veryl) {
        self.veryl(input);
        self.reset();
        for (x, y) in &self.align_identifier.widths {
            self.widths.insert(*x, *y);
        }
        for (x, y) in &self.align_type.widths {
            self.widths.insert(*x, *y);
        }
        for (x, y) in &self.align_expression.widths {
            self.widths.insert(*x, *y);
        }
        for (x, y) in &self.align_width.widths {
            self.widths.insert(*x, *y);
        }
    }

    fn reset(&mut self) {
        self.align_identifier.reset();
        self.align_type.reset();
        self.align_expression.reset();
        self.align_width.reset();
    }

    fn insert(&mut self, token: &VerylToken, width: usize) {
        let loc: Location = (&token.token.token.location).into();
        self.widths.insert(loc, width);
    }
}

impl VerylWalker for Aligner {
    // ----------------------------------------------------------------------------
    // Terminals
    // ----------------------------------------------------------------------------

    fn identifier(&mut self, input: &Identifier) {
        self.align_identifier.width(&input.identifier_token);
        self.align_identifier.update(&input.identifier_token);
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
        match &*input.integral_number {
            IntegralNumber::IntegralNumber0(x) => {
                self.align_expression
                    .width(&x.based_binary.based_binary_token);
                self.align_width.width(&x.based_binary.based_binary_token);
            }
            IntegralNumber::IntegralNumber1(x) => {
                self.align_expression
                    .width(&x.based_octal.based_octal_token);
                self.align_width.width(&x.based_octal.based_octal_token);
            }
            IntegralNumber::IntegralNumber2(x) => {
                self.align_expression
                    .width(&x.based_decimal.based_decimal_token);
                self.align_width.width(&x.based_decimal.based_decimal_token);
            }
            IntegralNumber::IntegralNumber3(x) => {
                self.align_expression.width(&x.based_hex.based_hex_token);
                self.align_width.width(&x.based_hex.based_hex_token);
            }
            IntegralNumber::IntegralNumber4(x) => {
                self.align_expression.width(&x.base_less.base_less_token);
                self.align_width.width(&x.base_less.base_less_token);
            }
        }
    }

    // ----------------------------------------------------------------------------
    // Expression
    // ----------------------------------------------------------------------------

    fn expression(&mut self, input: &Expression) {
        self.expression0(&input.expression0);
    }

    fn expression0(&mut self, input: &Expression0) {
        self.expression1(&input.expression1);
        for x in &input.expression0_list {
            self.align_expression.space(1);
            self.align_width.space(1);
            match &*x.operator_precedence1 {
                OperatorPrecedence1::OperatorPrecedence10(x) => {
                    self.align_expression.width(&x.plus.plus_token);
                    self.align_width.width(&x.plus.plus_token)
                }
                OperatorPrecedence1::OperatorPrecedence11(x) => {
                    self.align_expression.width(&x.minus.minus_token);
                    self.align_width.width(&x.minus.minus_token)
                }
            };
            self.align_expression.space(1);
            self.align_width.space(1);
            self.expression1(&x.expression1);
        }
    }

    fn expression1(&mut self, input: &Expression1) {
        self.expression2(&input.expression2);
        for x in &input.expression1_list {
            self.align_expression.space(1);
            self.align_width.space(1);
            match &*x.operator_precedence2 {
                OperatorPrecedence2::OperatorPrecedence20(x) => {
                    self.align_expression.width(&x.star.star_token);
                    self.align_width.width(&x.star.star_token)
                }
                OperatorPrecedence2::OperatorPrecedence21(x) => {
                    self.align_expression.width(&x.slash.slash_token);
                    self.align_width.width(&x.slash.slash_token)
                }
            };
            self.align_expression.space(1);
            self.align_width.space(1);
            self.expression2(&x.expression2);
        }
    }

    fn expression2(&mut self, input: &Expression2) {
        self.factor(&input.factor);
    }

    fn factor(&mut self, input: &Factor) {
        match input {
            Factor::Factor0(x) => self.number(&x.number),
            Factor::Factor1(x) => {
                self.align_expression.width(&x.identifier.identifier_token);
                self.align_width.width(&x.identifier.identifier_token);
                for x in &x.factor_list {
                    self.range(&x.range);
                }
            }
            Factor::Factor2(x) => {
                self.align_expression.width(&x.l_paren.l_paren_token);
                self.align_width.width(&x.l_paren.l_paren_token);
                self.expression(&x.expression);
                self.align_expression.width(&x.r_paren.r_paren_token);
                self.align_width.width(&x.r_paren.r_paren_token);
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
        self.align_expression
            .width(&input.l_bracket.l_bracket_token);
        self.align_width.width(&input.l_bracket.l_bracket_token);
        self.expression(&input.expression);
        if let Some(ref x) = input.range_opt {
            self.align_expression.width(&x.colon.colon_token);
            self.align_width.width(&x.colon.colon_token);
            self.expression(&x.expression);
        }
        self.align_expression
            .width(&input.r_bracket.r_bracket_token);
        self.align_width.width(&input.r_bracket.r_bracket_token);
    }

    fn width(&mut self, input: &Width) {
        self.align_expression
            .width(&input.l_bracket.l_bracket_token);
        self.align_width.width(&input.l_bracket.l_bracket_token);
        self.expression(&input.expression);
        self.align_expression
            .width(&input.r_bracket.r_bracket_token);
        self.align_width.width(&input.r_bracket.r_bracket_token);
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
        self.align_type.width(&token);
        self.align_type.update(&token);

        self.align_width.reset_width();
        for x in &input.type_list {
            self.width(&x.width);
        }
        if !input.type_list.is_empty() {
            self.align_width.update_with_last();
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
        self.align_expression.reset_width();
        self.expression(&input.expression);
        self.align_expression.update_with_last();
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
