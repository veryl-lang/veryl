use crate::aligner::{Aligner, Location};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::ParolToken;

pub struct Formatter {
    pub indent_width: usize,
    string: String,
    indent: usize,
    line: usize,
    aligner: Aligner,
    last_newline: usize,
    start_token: bool,
}

impl Default for Formatter {
    fn default() -> Self {
        Self {
            string: String::new(),
            indent_width: 4,
            indent: 0,
            line: 1,
            aligner: Aligner::new(),
            last_newline: 0,
            start_token: false,
        }
    }
}

impl Formatter {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn format(&mut self, input: &Veryl) {
        self.aligner.align(input);
        self.veryl(input);
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    fn str(&mut self, x: &str) {
        self.string.push_str(x);
    }

    fn newline_push(&mut self) {
        if self.string.ends_with(' ') {
            self.string
                .truncate(self.string.len() - self.indent * self.indent_width);
        }
        self.str("\n");
        self.indent += 1;
        self.str(&" ".repeat(self.indent * self.indent_width));
    }

    fn newline_pop(&mut self) {
        if self.string.ends_with(' ') {
            self.string
                .truncate(self.string.len() - self.indent * self.indent_width);
        }
        self.str("\n");
        self.indent -= 1;
        self.str(&" ".repeat(self.indent * self.indent_width));
    }

    fn newline(&mut self) {
        if self.string.ends_with(' ') {
            self.string
                .truncate(self.string.len() - self.indent * self.indent_width);
        }
        self.str("\n");
        self.str(&" ".repeat(self.indent * self.indent_width));
    }

    fn space(&mut self, repeat: usize) {
        self.str(&" ".repeat(repeat));
    }

    fn parol_token(&mut self, x: &ParolToken, adjust_line: bool) {
        if adjust_line && x.location.line - self.line > 1 {
            self.newline();
        }
        let text = x.text();
        let text = if text.ends_with('\n') {
            text.trim_end()
        } else {
            text
        };
        self.last_newline = text.matches('\n').count();
        self.str(text);
        self.line = x.location.line;
    }

    fn process_token(&mut self, x: &VerylToken, will_push: bool) {
        self.parol_token(&x.token.token, true);

        let loc: Location = (&x.token.token.location).into();
        if let Some(width) = self.aligner.additions.get(&loc) {
            self.space(*width);
        }

        // temporary indent to adjust indent of comments with the next push
        if will_push {
            self.indent += 1;
        }
        for x in &x.comments {
            if x.token.location.line == self.line && !self.start_token {
                self.space(1);
            }
            for _ in 0..x.token.location.line - (self.line + self.last_newline) {
                self.newline();
            }
            self.parol_token(&x.token, false);
        }
        if will_push {
            self.indent -= 1;
        }
    }

    fn token(&mut self, x: &VerylToken) {
        self.process_token(x, false)
    }

    fn token_will_push(&mut self, x: &VerylToken) {
        self.process_token(x, true)
    }
}

impl VerylWalker for Formatter {
    // ----------------------------------------------------------------------------
    // Terminals
    // ----------------------------------------------------------------------------

    fn identifier(&mut self, input: &Identifier) {
        self.token(&input.identifier_token);
    }

    // ----------------------------------------------------------------------------
    // SourceCode
    // ----------------------------------------------------------------------------

    fn veryl(&mut self, input: &Veryl) {
        self.start_token = true;
        self.token(&input.start.start_token);
        self.start_token = false;
        if !input.start.start_token.comments.is_empty() {
            self.newline();
        }
        for (i, x) in input.veryl_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.description(&x.description);
        }
        self.newline();
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
                IntegralNumber::IntegralNumber0(x) => &x.based_binary.based_binary_token,
                IntegralNumber::IntegralNumber1(x) => &x.based_octal.based_octal_token,
                IntegralNumber::IntegralNumber2(x) => &x.based_decimal.based_decimal_token,
                IntegralNumber::IntegralNumber3(x) => &x.based_hex.based_hex_token,
                IntegralNumber::IntegralNumber4(x) => &x.base_less.base_less_token,
                IntegralNumber::IntegralNumber5(x) => &x.all_bit.all_bit_token,
            },
            Number::Number1(x) => match &*x.real_number {
                RealNumber::RealNumber0(x) => &x.fixed_point.fixed_point_token,
                RealNumber::RealNumber1(x) => &x.exponent.exponent_token,
            },
        };
        self.token(token);
    }

    // ----------------------------------------------------------------------------
    // Expression
    // ----------------------------------------------------------------------------

    fn expression(&mut self, input: &Expression) {
        self.expression1(&input.expression1);
        for x in &input.expression_list {
            self.space(1);
            let token = match &*x.binary_operator {
                BinaryOperator::BinaryOperator0(x) => &x.plus.plus_token,
                BinaryOperator::BinaryOperator1(x) => &x.minus.minus_token,
                BinaryOperator::BinaryOperator2(x) => &x.star.star_token,
                BinaryOperator::BinaryOperator3(x) => &x.slash.slash_token,
                BinaryOperator::BinaryOperator4(x) => &x.percent.percent_token,
                BinaryOperator::BinaryOperator5(x) => &x.amp.amp_token,
                BinaryOperator::BinaryOperator6(x) => &x.or.or_token,
                BinaryOperator::BinaryOperator7(x) => &x.circumflex.circumflex_token,
                BinaryOperator::BinaryOperator8(x) => &x.star_star.star_star_token,
                BinaryOperator::BinaryOperator9(x) => &x.amp_amp.amp_amp_token,
                BinaryOperator::BinaryOperator10(x) => &x.or_or.or_or_token,
                BinaryOperator::BinaryOperator11(x) => &x.tilde_circumflex.tilde_circumflex_token,
                BinaryOperator::BinaryOperator12(x) => &x.circumflex_tilde.circumflex_tilde_token,
                BinaryOperator::BinaryOperator13(x) => &x.l_t_l_t.l_t_l_t_token,
                BinaryOperator::BinaryOperator14(x) => &x.g_t_g_t.g_t_g_t_token,
                BinaryOperator::BinaryOperator15(x) => &x.l_t_l_t_l_t.l_t_l_t_l_t_token,
                BinaryOperator::BinaryOperator16(x) => &x.g_t_g_t_g_t.g_t_g_t_g_t_token,
                BinaryOperator::BinaryOperator17(x) => &x.l_t.l_t_token,
                BinaryOperator::BinaryOperator18(x) => &x.g_t.g_t_token,
                BinaryOperator::BinaryOperator19(x) => &x.l_t_equ.l_t_equ_token,
                BinaryOperator::BinaryOperator20(x) => &x.g_t_equ.g_t_equ_token,
                BinaryOperator::BinaryOperator21(x) => &x.equ_equ.equ_equ_token,
                BinaryOperator::BinaryOperator22(x) => &x.bang_equ.bang_equ_token,
                BinaryOperator::BinaryOperator23(x) => &x.equ_equ_equ.equ_equ_equ_token,
                BinaryOperator::BinaryOperator24(x) => &x.bang_equ_equ.bang_equ_equ_token,
                BinaryOperator::BinaryOperator25(x) => &x.equ_equ_quest.equ_equ_quest_token,
                BinaryOperator::BinaryOperator26(x) => &x.bang_equ_quest.bang_equ_quest_token,
            };
            self.token(token);
            self.space(1);
            self.expression1(&x.expression1);
        }
    }

    fn expression1(&mut self, input: &Expression1) {
        if let Some(ref x) = input.expression1_opt {
            let token = match &*x.unary_operator {
                UnaryOperator::UnaryOperator0(x) => &x.plus.plus_token,
                UnaryOperator::UnaryOperator1(x) => &x.minus.minus_token,
                UnaryOperator::UnaryOperator2(x) => &x.bang.bang_token,
                UnaryOperator::UnaryOperator3(x) => &x.tilde.tilde_token,
                UnaryOperator::UnaryOperator4(x) => &x.amp.amp_token,
                UnaryOperator::UnaryOperator5(x) => &x.or.or_token,
                UnaryOperator::UnaryOperator6(x) => &x.circumflex.circumflex_token,
                UnaryOperator::UnaryOperator7(x) => &x.tilde_amp.tilde_amp_token,
                UnaryOperator::UnaryOperator8(x) => &x.tilde_or.tilde_or_token,
                UnaryOperator::UnaryOperator9(x) => &x.tilde_circumflex.tilde_circumflex_token,
                UnaryOperator::UnaryOperator10(x) => &x.circumflex_tilde.circumflex_tilde_token,
            };
            self.token(token);
        }
        self.factor(&input.factor);
    }

    fn factor(&mut self, input: &Factor) {
        match input {
            Factor::Factor0(x) => self.number(&x.number),
            Factor::Factor1(x) => {
                self.identifier(&x.identifier);
                for x in &x.factor_list {
                    self.range(&x.range);
                }
            }
            Factor::Factor2(x) => {
                self.token(&x.l_paren.l_paren_token);
                self.expression(&x.expression);
                self.token(&x.r_paren.r_paren_token);
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
        self.space(1);
        self.token(&input.equ.equ_token);
        self.space(1);
        self.expression(&input.expression);
        self.token(&input.semicolon.semicolon_token);
    }

    fn if_statement(&mut self, input: &IfStatement) {
        self.token(&input.r#if.if_token);
        self.space(1);
        self.expression(&input.expression);
        self.space(1);
        self.token_will_push(&input.l_brace.l_brace_token);
        self.newline_push();
        self.statement(&input.statement);
        self.newline_pop();
        self.token(&input.r_brace.r_brace_token);
        if !input.if_statement_list.is_empty() {
            self.space(1);
        }
        for x in &input.if_statement_list {
            self.token(&x.r#else.else_token);
            self.space(1);
            self.token(&x.r#if.if_token);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            self.statement(&x.statement);
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token);
        }
        if let Some(ref x) = input.if_statement_opt {
            self.space(1);
            self.token(&x.r#else.else_token);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            self.statement(&x.statement);
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token);
        }
    }

    // ----------------------------------------------------------------------------
    // Range / Width
    // ----------------------------------------------------------------------------

    fn range(&mut self, input: &Range) {
        self.token(&input.l_bracket.l_bracket_token);
        self.expression(&input.expression);
        if let Some(ref x) = input.range_opt {
            self.token(&x.colon.colon_token);
            self.expression(&x.expression);
        }
        self.token(&input.r_bracket.r_bracket_token);
    }

    fn width(&mut self, input: &Width) {
        self.token(&input.l_bracket.l_bracket_token);
        self.expression(&input.expression);
        self.token(&input.r_bracket.r_bracket_token);
    }

    // ----------------------------------------------------------------------------
    // Type
    // ----------------------------------------------------------------------------

    fn r#type(&mut self, input: &Type) {
        match &*input.type_group {
            TypeGroup::TypeGroup0(x) => match &*x.builtin_type {
                BuiltinType::BuiltinType0(x) => self.token(&x.logic.logic_token),
                BuiltinType::BuiltinType1(x) => self.token(&x.bit.bit_token),
                BuiltinType::BuiltinType2(x) => self.token(&x.u32.u32_token),
                BuiltinType::BuiltinType3(x) => self.token(&x.u64.u64_token),
                BuiltinType::BuiltinType4(x) => self.token(&x.i32.i32_token),
                BuiltinType::BuiltinType5(x) => self.token(&x.i64.i64_token),
                BuiltinType::BuiltinType6(x) => self.token(&x.f32.f32_token),
                BuiltinType::BuiltinType7(x) => self.token(&x.f64.f64_token),
            },
            // This identifier should be treat as type not identifier
            TypeGroup::TypeGroup1(x) => self.token(&x.identifier.identifier_token),
        };
        self.space(1);
        for x in &input.type_list {
            self.width(&x.width);
        }
    }

    // ----------------------------------------------------------------------------
    // WithParameter
    // ----------------------------------------------------------------------------

    fn with_parameter(&mut self, input: &WithParameter) {
        if let Some(ref x) = input.with_parameter_opt {
            self.token(&input.hash.hash_token);
            self.token_will_push(&input.l_paren.l_paren_token);
            self.newline_push();
            self.with_parameter_list(&x.with_parameter_list);
            self.newline_pop();
            self.token(&input.r_paren.r_paren_token);
        } else {
            self.token(&input.hash.hash_token);
            self.token(&input.l_paren.l_paren_token);
            self.token(&input.r_paren.r_paren_token);
        }
    }

    fn with_parameter_list(&mut self, input: &WithParameterList) {
        self.with_parameter_item(&input.with_parameter_item);
        for x in &input.with_parameter_list_list {
            self.token(&x.comma.comma_token);
            self.newline();
            self.with_parameter_item(&x.with_parameter_item);
        }
        if let Some(ref x) = input.with_parameter_list_opt {
            self.token(&x.comma.comma_token);
        } else {
            self.str(",");
        }
    }

    fn with_parameter_item(&mut self, input: &WithParameterItem) {
        match &*input.with_parameter_item_group {
            WithParameterItemGroup::WithParameterItemGroup0(x) => {
                self.token(&x.parameter.parameter_token);
            }
            WithParameterItemGroup::WithParameterItemGroup1(x) => {
                self.token(&x.localparam.localparam_token);
            }
        }
        self.space(1);
        self.identifier(&input.identifier);
        self.token(&input.colon.colon_token);
        self.space(1);
        self.r#type(&input.r#type);
        self.space(1);
        self.token(&input.equ.equ_token);
        self.space(1);
        self.expression(&input.expression);
    }

    // ----------------------------------------------------------------------------
    // Module
    // ----------------------------------------------------------------------------

    fn module_declaration(&mut self, input: &ModuleDeclaration) {
        self.token(&input.module.module_token);
        self.space(1);
        self.identifier(&input.identifier);
        self.space(1);
        if let Some(ref x) = input.module_declaration_opt {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        if let Some(ref x) = input.module_declaration_opt0 {
            self.module_port(&x.module_port);
            self.space(1);
        }
        self.token_will_push(&input.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in input.module_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_item(&x.module_item);
        }
        self.newline_pop();
        self.token(&input.r_brace.r_brace_token);
    }

    fn module_port(&mut self, input: &ModulePort) {
        if let Some(ref x) = input.module_port_opt {
            self.token_will_push(&input.l_paren.l_paren_token);
            self.newline_push();
            self.module_port_list(&x.module_port_list);
            self.newline_pop();
            self.token(&input.r_paren.r_paren_token);
        } else {
            self.token(&input.l_paren.l_paren_token);
            self.token(&input.r_paren.r_paren_token);
        }
    }

    fn module_port_list(&mut self, input: &ModulePortList) {
        self.module_port_item(&input.module_port_item);
        for x in &input.module_port_list_list {
            self.token(&x.comma.comma_token);
            self.newline();
            self.module_port_item(&x.module_port_item);
        }
        if let Some(ref x) = input.module_port_list_opt {
            self.token(&x.comma.comma_token);
        } else {
            self.str(",");
        }
    }

    fn module_port_item(&mut self, input: &ModulePortItem) {
        self.identifier(&input.identifier);
        self.token(&input.colon.colon_token);
        self.space(1);
        self.direction(&input.direction);
        self.space(1);
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
                self.token(&x.input.input_token);
            }
            Direction::Direction1(x) => {
                self.token(&x.output.output_token);
            }
            Direction::Direction2(x) => {
                self.token(&x.inout.inout_token);
            }
        }
    }

    // ----------------------------------------------------------------------------
    // Interface
    // ----------------------------------------------------------------------------

    fn interface_declaration(&mut self, input: &InterfaceDeclaration) {
        self.token(&input.interface.interface_token);
        self.space(1);
        self.identifier(&input.identifier);
        self.space(1);
        if let Some(ref x) = input.interface_declaration_opt {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        self.token_will_push(&input.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in input.interface_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_item(&x.interface_item);
        }
        self.newline_pop();
        self.token(&input.r_brace.r_brace_token);
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
        self.token(&input.colon.colon_token);
        self.space(1);
        self.r#type(&input.r#type);
        self.token(&input.semicolon.semicolon_token);
    }

    fn parameter_declaration(&mut self, input: &ParameterDeclaration) {
        self.token(&input.parameter.parameter_token);
        self.space(1);
        self.identifier(&input.identifier);
        self.token(&input.colon.colon_token);
        self.space(1);
        self.r#type(&input.r#type);
        self.space(1);
        self.token(&input.equ.equ_token);
        self.space(1);
        self.expression(&input.expression);
        self.token(&input.semicolon.semicolon_token);
    }

    fn localparam_declaration(&mut self, input: &LocalparamDeclaration) {
        self.token(&input.localparam.localparam_token);
        self.space(1);
        self.identifier(&input.identifier);
        self.token(&input.colon.colon_token);
        self.space(1);
        self.r#type(&input.r#type);
        self.space(1);
        self.token(&input.equ.equ_token);
        self.space(1);
        self.expression(&input.expression);
        self.token(&input.semicolon.semicolon_token);
    }

    fn always_ff_declaration(&mut self, input: &AlwaysFfDeclaration) {
        self.token(&input.always_ff.always_ff_token);
        self.space(1);
        self.token(&input.l_paren.l_paren_token);
        self.always_ff_conditions(&input.always_ff_conditions);
        self.token(&input.r_paren.r_paren_token);
        self.space(1);
        self.token_will_push(&input.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in input.always_ff_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&input.r_brace.r_brace_token);
    }

    fn always_ff_conditions(&mut self, input: &AlwaysFfConditions) {
        self.always_ff_condition(&input.always_ff_condition);
        for x in &input.always_ff_conditions_list {
            self.token(&x.comma.comma_token);
            self.space(1);
            self.always_ff_condition(&x.always_ff_condition);
        }
    }

    fn always_ff_condition(&mut self, input: &AlwaysFfCondition) {
        match &*input.always_ff_condition_group {
            AlwaysFfConditionGroup::AlwaysFfConditionGroup0(x) => {
                self.token(&x.posedge.posedge_token)
            }
            AlwaysFfConditionGroup::AlwaysFfConditionGroup1(x) => {
                self.token(&x.negedge.negedge_token)
            }
        };
        self.space(1);
        self.identifier(&input.identifier);
    }

    fn always_comb_declaration(&mut self, input: &AlwaysCombDeclaration) {
        self.token(&input.always_comb.always_comb_token);
        self.space(1);
        self.token_will_push(&input.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in input.always_comb_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&input.r_brace.r_brace_token);
    }

    fn assign_declaration(&mut self, input: &AssignDeclaration) {
        self.token(&input.assign.assign_token);
        self.space(1);
        self.identifier(&input.identifier);
        if let Some(ref x) = input.assign_declaration_opt {
            self.token(&x.colon.colon_token);
            self.space(1);
            self.r#type(&x.r#type);
        }
        self.space(1);
        self.token(&input.equ.equ_token);
        self.space(1);
        self.expression(&input.expression);
        self.token(&input.semicolon.semicolon_token);
    }

    fn modport_declaration(&mut self, input: &ModportDeclaration) {
        self.token(&input.modport.modport_token);
        self.space(1);
        self.identifier(&input.identifier);
        self.space(1);
        self.token_will_push(&input.l_brace.l_brace_token);
        self.newline_push();
        self.modport_list(&input.modport_list);
        self.newline_pop();
        self.token(&input.r_brace.r_brace_token);
    }

    fn modport_list(&mut self, input: &ModportList) {
        self.modport_item(&input.modport_item);
        for x in &input.modport_list_list {
            self.token(&x.comma.comma_token);
            self.newline();
            self.modport_item(&x.modport_item);
        }
        if let Some(ref x) = input.modport_list_opt {
            self.token(&x.comma.comma_token);
        } else {
            self.str(",");
        }
    }

    fn modport_item(&mut self, input: &ModportItem) {
        self.identifier(&input.identifier);
        self.token(&input.colon.colon_token);
        self.space(1);
        self.direction(&input.direction);
    }
}
