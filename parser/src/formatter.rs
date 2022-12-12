use crate::veryl_grammar_trait::*;
use parol_runtime::lexer::Token;

pub struct Formatter {
    pub string: String,
    pub indent_width: usize,
    indent: usize,
    line: usize,
}

impl Default for Formatter {
    fn default() -> Self {
        Self {
            string: String::new(),
            indent_width: 4,
            indent: 0,
            line: 0,
        }
    }
}

impl Formatter {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn format(&mut self, input: &Veryl) {
        for (i, x) in input.veryl_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.description(&x.description);
        }
    }

    fn newline_push(&mut self) {
        self.string.push_str("\n");
        self.indent += 1;
        self.string
            .push_str(&" ".repeat(self.indent * self.indent_width));
    }

    fn newline_pop(&mut self) {
        self.string.push_str("\n");
        self.indent -= 1;
        self.string
            .push_str(&" ".repeat(self.indent * self.indent_width));
    }

    fn newline(&mut self) {
        self.string.push_str("\n");
        self.string
            .push_str(&" ".repeat(self.indent * self.indent_width));
    }

    fn space(&mut self, repeat: usize) {
        self.string.push_str(&" ".repeat(repeat));
    }

    fn str(&mut self, x: &str) {
        self.string.push_str(x);
    }

    fn token(&mut self, x: &Token) {
        if x.location.line - self.line > 1 {
            self.string
                .truncate(self.string.len() - self.indent * self.indent_width);
            self.newline();
        }
        self.string.push_str(x.text());
        self.line = x.location.line;
    }

    fn description(&mut self, input: &Description) {
        match input {
            Description::Description0(x) => self.module_declaration(&x.module_declaration),
            Description::Description1(x) => self.interface_declaration(&x.interface_declaration),
        }
    }

    fn module_declaration(&mut self, input: &ModuleDeclaration) {
        self.token(&input.module.module);
        self.space(1);
        self.token(&input.identifier.identifier);
        self.space(1);
        if let Some(ref x) = input.module_declaration_opt {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        if let Some(ref x) = input.module_declaration_opt0 {
            self.module_port(&x.module_port);
            self.space(1);
        }
        self.token(&input.l_brace.l_brace);
        self.newline_push();
        for (i, x) in input.module_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_item(&x.module_item);
        }
        self.newline_pop();
        self.token(&input.r_brace.r_brace);
    }

    fn with_parameter(&mut self, input: &WithParameter) {
        if let Some(ref x) = input.with_parameter_opt {
            self.token(&input.sharp.sharp);
            self.token(&input.l_paren.l_paren);
            self.newline_push();
            self.with_parameter_list(&x.with_parameter_list);
            self.newline_pop();
            self.token(&input.r_paren.r_paren);
        } else {
            self.token(&input.sharp.sharp);
            self.token(&input.l_paren.l_paren);
            self.token(&input.r_paren.r_paren);
        }
    }

    fn with_parameter_list(&mut self, input: &WithParameterList) {
        self.with_parameter_item(&input.with_parameter_item);
        for x in &input.with_parameter_list_list {
            self.newline();
            self.with_parameter_item(&x.with_parameter_item);
        }
    }

    fn with_parameter_item(&mut self, input: &WithParameterItem) {
        match &*input.with_parameter_item_group {
            WithParameterItemGroup::WithParameterItemGroup0(x) => {
                self.token(&x.parameter.parameter);
                self.space(2);
            }
            WithParameterItemGroup::WithParameterItemGroup1(x) => {
                self.token(&x.localparam.localparam);
                self.space(1);
            }
        }
        self.token(&input.identifier.identifier);
        self.token(&input.colon.colon);
        self.space(1);
        self.r#type(&input.r#type);
        self.space(1);
        self.token(&input.assignment.assignment);
        self.space(1);
        self.expression(&input.expression);
        self.str(",");
    }

    fn module_port(&mut self, input: &ModulePort) {
        if let Some(ref x) = input.module_port_opt {
            self.token(&input.l_paren.l_paren);
            self.newline_push();
            self.module_port_list(&x.module_port_list);
            self.newline_pop();
            self.token(&input.r_paren.r_paren);
        } else {
            self.token(&input.l_paren.l_paren);
            self.token(&input.r_paren.r_paren);
        }
    }

    fn module_port_list(&mut self, input: &ModulePortList) {
        self.module_port_item(&input.module_port_item);
        for x in &input.module_port_list_list {
            self.newline();
            self.module_port_item(&x.module_port_item);
        }
    }

    fn module_port_item(&mut self, input: &ModulePortItem) {
        self.token(&input.identifier.identifier);
        self.token(&input.colon.colon);
        self.space(1);
        self.direction(&input.direction);
        self.space(1);
        self.r#type(&input.r#type);
        self.str(",");
    }

    fn module_item(&mut self, input: &ModuleItem) {
        match input {
            ModuleItem::ModuleItem0(x) => self.variable_declaration(&x.variable_declaration),
            ModuleItem::ModuleItem1(x) => self.parameter_declaration(&x.parameter_declaration),
            ModuleItem::ModuleItem2(x) => self.localparam_declaration(&x.localparam_declaration),
            ModuleItem::ModuleItem3(x) => self.always_f_f_declaration(&x.always_f_f_declaration),
            ModuleItem::ModuleItem4(x) => self.always_comb_declaration(&x.always_comb_declaration),
        }
    }

    fn variable_declaration(&mut self, input: &VariableDeclaration) {
        self.token(&input.identifier.identifier);
        self.token(&input.colon.colon);
        self.space(1);
        self.r#type(&input.r#type);
        self.token(&input.semi_colon.semi_colon);
    }

    fn parameter_declaration(&mut self, input: &ParameterDeclaration) {
        self.token(&input.parameter.parameter);
        self.space(2);
        self.token(&input.identifier.identifier);
        self.token(&input.colon.colon);
        self.space(1);
        self.r#type(&input.r#type);
        self.space(1);
        self.token(&input.assignment.assignment);
        self.space(1);
        self.expression(&input.expression);
        self.token(&input.semi_colon.semi_colon);
    }

    fn localparam_declaration(&mut self, input: &LocalparamDeclaration) {
        self.token(&input.localparam.localparam);
        self.space(1);
        self.token(&input.identifier.identifier);
        self.token(&input.colon.colon);
        self.space(1);
        self.r#type(&input.r#type);
        self.space(1);
        self.token(&input.assignment.assignment);
        self.space(1);
        self.expression(&input.expression);
        self.token(&input.semi_colon.semi_colon);
    }

    fn always_f_f_declaration(&mut self, input: &AlwaysFFDeclaration) {
        self.token(&input.always_f_f.always_f_f);
        self.space(1);
        self.token(&input.l_paren.l_paren);
        self.always_f_f_conditions(&input.always_f_f_conditions);
        self.token(&input.r_paren.r_paren);
        self.space(1);
        self.token(&input.l_brace.l_brace);
        self.newline_push();
        for (i, x) in input.always_f_f_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&input.r_brace.r_brace);
    }

    fn always_f_f_conditions(&mut self, input: &AlwaysFFConditions) {
        self.always_f_f_condition(&input.always_f_f_condition);
        for x in &input.always_f_f_conditions_list {
            self.space(1);
            self.always_f_f_condition(&x.always_f_f_condition);
        }
    }

    fn always_f_f_condition(&mut self, input: &AlwaysFFCondition) {
        match &*input.always_f_f_condition_group {
            AlwaysFFConditionGroup::AlwaysFFConditionGroup0(x) => self.token(&x.posedge.posedge),
            AlwaysFFConditionGroup::AlwaysFFConditionGroup1(x) => self.token(&x.negedge.negedge),
        }
        self.space(1);
        self.token(&input.identifier.identifier);
        self.str(",");
    }

    fn always_comb_declaration(&mut self, input: &AlwaysCombDeclaration) {
        self.token(&input.always_comb.always_comb);
        self.space(1);
        self.token(&input.l_brace.l_brace);
        self.newline_push();
        for (i, x) in input.always_comb_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&input.r_brace.r_brace);
    }

    fn direction(&mut self, input: &Direction) {
        match input {
            Direction::Direction0(x) => {
                self.token(&x.input.input);
                self.space(1);
            }
            Direction::Direction1(x) => self.token(&x.output.output),
            Direction::Direction2(x) => {
                self.token(&x.inout.inout);
                self.space(1);
            }
        }
    }

    fn r#type(&mut self, input: &Type) {
        match &*input.type_group {
            TypeGroup::TypeGroup0(x) => match &*x.builtin_type {
                BuiltinType::BuiltinType0(x) => self.token(&x.logic.logic),
                BuiltinType::BuiltinType1(x) => self.token(&x.bit.bit),
                BuiltinType::BuiltinType2(x) => self.token(&x.u32.u32),
                BuiltinType::BuiltinType3(x) => self.token(&x.u64.u64),
                BuiltinType::BuiltinType4(x) => self.token(&x.i32.i32),
                BuiltinType::BuiltinType5(x) => self.token(&x.i64.i64),
                BuiltinType::BuiltinType6(x) => self.token(&x.f32.f32),
                BuiltinType::BuiltinType7(x) => self.token(&x.f64.f64),
            },
            TypeGroup::TypeGroup1(x) => self.token(&x.identifier.identifier),
        }
        if !input.type_list.is_empty() {
            self.space(1);
        }
        for x in &input.type_list {
            self.width(&x.width);
        }
    }

    fn statement(&mut self, input: &Statement) {
        match input {
            Statement::Statement0(x) => self.assignment_statement(&x.assignment_statement),
            Statement::Statement1(x) => self.if_statement(&x.if_statement),
        }
    }

    fn assignment_statement(&mut self, input: &AssignmentStatement) {
        self.token(&input.identifier.identifier);
        self.space(1);
        self.token(&input.assignment.assignment);
        self.space(1);
        self.expression(&input.expression);
        self.token(&input.semi_colon.semi_colon);
    }

    fn if_statement(&mut self, input: &IfStatement) {
        self.token(&input.r#if.r#if);
        self.space(1);
        self.expression(&input.expression);
        self.space(1);
        self.token(&input.l_brace.l_brace);
        self.newline_push();
        self.statement(&input.statement);
        self.newline_pop();
        self.token(&input.r_brace.r_brace);
        if !input.if_statement_list.is_empty() {
            self.space(1);
        }
        for x in &input.if_statement_list {
            self.token(&x.r#else.r#else);
            self.space(1);
            self.token(&x.r#if.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.token(&x.l_brace.l_brace);
            self.newline_push();
            self.statement(&x.statement);
            self.newline_pop();
            self.token(&x.r_brace.r_brace);
        }
        if let Some(ref x) = input.if_statement_opt {
            self.space(1);
            self.token(&x.r#else.r#else);
            self.space(1);
            self.token(&x.l_brace.l_brace);
            self.newline_push();
            self.statement(&x.statement);
            self.newline_pop();
            self.token(&x.r_brace.r_brace);
        }
    }

    fn expression(&mut self, input: &Expression) {
        self.expression0(&input.expression0);
    }

    fn expression0(&mut self, input: &Expression0) {
        self.expression1(&input.expression1);
        for x in &input.expression0_list {
            match &*x.operator_precedence1 {
                OperatorPrecedence1::OperatorPrecedence10(x) => self.token(&x.plus.plus),
                OperatorPrecedence1::OperatorPrecedence11(x) => self.token(&x.minus.minus),
            }
            self.expression1(&x.expression1);
        }
    }

    fn expression1(&mut self, input: &Expression1) {
        self.expression2(&input.expression2);
        for x in &input.expression1_list {
            match &*x.operator_precedence2 {
                OperatorPrecedence2::OperatorPrecedence20(x) => self.token(&x.mul.mul),
                OperatorPrecedence2::OperatorPrecedence21(x) => self.token(&x.div.div),
            }
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
                self.token(&x.identifier.identifier);
                if let Some(ref x) = x.factor_opt {
                    self.range(&x.range);
                }
            }
            Factor::Factor2(x) => {
                self.token(&x.l_paren.l_paren);
                self.expression(&x.expression);
                self.token(&x.r_paren.r_paren);
            }
        }
    }

    fn number(&mut self, input: &Number) {
        match &*input.integral_number {
            IntegralNumber::IntegralNumber0(x) => {
                self.token(&x.binary_number.base_less.base_less);
                self.token(&x.binary_number.based_binary.based_binary);
            }
            IntegralNumber::IntegralNumber1(x) => {
                self.token(&x.octal_number.base_less.base_less);
                self.token(&x.octal_number.based_octal.based_octal);
            }
            IntegralNumber::IntegralNumber2(x) => {
                self.token(&x.decimal_number.base_less.base_less);
                self.token(&x.decimal_number.based_decimal.based_decimal);
            }
            IntegralNumber::IntegralNumber3(x) => {
                self.token(&x.hex_number.base_less.base_less);
                self.token(&x.hex_number.based_hex.based_hex);
            }
            IntegralNumber::IntegralNumber4(x) => {
                self.token(&x.base_less_number.base_less.base_less);
            }
        }
    }

    fn width(&mut self, input: &Width) {
        self.token(&input.l_bracket.l_bracket);
        self.expression(&input.expression);
        self.token(&input.r_bracket.r_bracket);
    }

    fn range(&mut self, input: &Range) {
        self.token(&input.l_bracket.l_bracket);
        self.expression(&input.expression);
        if let Some(ref x) = input.range_opt {
            self.token(&x.colon.colon);
            self.expression(&x.expression);
        }
        self.token(&input.r_bracket.r_bracket);
    }

    fn interface_declaration(&mut self, input: &InterfaceDeclaration) {
        self.token(&input.interface.interface);
        self.space(1);
        self.token(&input.identifier.identifier);
        self.space(1);
        if let Some(ref x) = input.interface_declaration_opt {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        self.token(&input.l_brace.l_brace);
        self.newline_push();
        for (i, x) in input.interface_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_item(&x.interface_item);
        }
        self.newline_pop();
        self.token(&input.r_brace.r_brace);
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

    fn modport_declaration(&mut self, input: &ModportDeclaration) {
        self.token(&input.modport.modport);
        self.space(1);
        self.token(&input.identifier.identifier);
        self.space(1);
        self.token(&input.l_brace.l_brace);
        self.newline_push();
        self.modport_list(&input.modport_list);
        self.newline_pop();
        self.token(&input.r_brace.r_brace);
    }

    fn modport_list(&mut self, input: &ModportList) {
        self.modport_item(&input.modport_item);
        for x in &input.modport_list_list {
            self.newline();
            self.modport_item(&x.modport_item);
        }
    }

    fn modport_item(&mut self, input: &ModportItem) {
        self.token(&input.identifier.identifier);
        self.token(&input.colon.colon);
        self.space(1);
        self.direction(&input.direction);
        self.str(",");
    }
}
