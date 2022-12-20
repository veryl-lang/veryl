use crate::aligner::{Aligner, Location};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::ParolToken;

pub struct Emitter {
    pub indent_width: usize,
    string: String,
    indent: usize,
    line: usize,
    aligner: Aligner,
    last_newline: usize,
    start_token: bool,
    always_ff: bool,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            string: String::new(),
            indent_width: 4,
            indent: 0,
            line: 1,
            aligner: Aligner::new(),
            last_newline: 0,
            start_token: false,
            always_ff: false,
        }
    }
}

impl Emitter {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn emit(&mut self, input: &Veryl) {
        self.aligner.align(input);
        self.veryl(input);
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    fn str(&mut self, x: &str) {
        self.string.push_str(x);
    }

    fn unindent(&mut self) {
        if self.string.ends_with(' ') {
            self.string
                .truncate(self.string.len() - self.indent * self.indent_width);
        }
    }

    fn newline_push(&mut self) {
        self.unindent();
        self.str("\n");
        self.indent += 1;
        self.str(&" ".repeat(self.indent * self.indent_width));
    }

    fn newline_pop(&mut self) {
        self.unindent();
        self.str("\n");
        self.indent -= 1;
        self.str(&" ".repeat(self.indent * self.indent_width));
    }

    fn newline(&mut self) {
        self.unindent();
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

    fn type_left(&mut self, input: &Type) {
        let (width, token) = match &*input.type_group {
            TypeGroup::TypeGroup0(x) => match &*x.builtin_type {
                BuiltinType::BuiltinType0(x) => (true, x.logic.logic_token.clone()),
                BuiltinType::BuiltinType1(x) => (true, x.bit.bit_token.clone()),
                BuiltinType::BuiltinType2(x) => (false, x.u32.u32_token.replace("int unsigned")),
                BuiltinType::BuiltinType3(x) => {
                    (false, x.u64.u64_token.replace("longint unsigned"))
                }
                BuiltinType::BuiltinType4(x) => (false, x.i32.i32_token.replace("int signed")),
                BuiltinType::BuiltinType5(x) => (false, x.i64.i64_token.replace("longint signed")),
                BuiltinType::BuiltinType6(x) => (false, x.f32.f32_token.replace("real")),
                BuiltinType::BuiltinType7(x) => (false, x.f64.f64_token.replace("longreal")),
            },
            TypeGroup::TypeGroup1(x) => (false, x.identifier.identifier_token.clone()),
        };
        self.token(&token);
        if width {
            self.space(1);
            for x in &input.type_list {
                self.width(&x.width);
            }
        }
    }

    fn type_right(&mut self, input: &Type) {
        let width = match &*input.type_group {
            TypeGroup::TypeGroup0(x) => match &*x.builtin_type {
                BuiltinType::BuiltinType0(_) => false,
                BuiltinType::BuiltinType1(_) => false,
                BuiltinType::BuiltinType2(_) => true,
                BuiltinType::BuiltinType3(_) => true,
                BuiltinType::BuiltinType4(_) => true,
                BuiltinType::BuiltinType5(_) => true,
                BuiltinType::BuiltinType6(_) => true,
                BuiltinType::BuiltinType7(_) => true,
            },
            TypeGroup::TypeGroup1(_) => true,
        };
        if width {
            self.space(1);
            for x in &input.type_list {
                self.width(&x.width);
            }
        }
    }
}

impl VerylWalker for Emitter {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.token(arg);
    }

    /// Semantic action for non-terminal 'Expression'
    fn expression(&mut self, arg: &Expression) {
        self.expression1(&arg.expression1);
        for x in &arg.expression_list {
            self.space(1);
            match &*x.expression_list_group {
                ExpressionListGroup::ExpressionListGroup0(x) => {
                    self.binary_operator(&x.binary_operator)
                }
                ExpressionListGroup::ExpressionListGroup1(x) => {
                    self.common_operator(&x.common_operator)
                }
            };
            self.space(1);
            self.expression1(&x.expression1);
        }
    }

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        self.str("-1:0");
        self.r_bracket(&arg.r_bracket);
    }

    /// Semantic action for non-terminal 'Type'
    fn r#type(&mut self, _arg: &Type) {}

    /// Semantic action for non-terminal 'AssignmentStatement'
    fn assignment_statement(&mut self, arg: &AssignmentStatement) {
        self.identifier(&arg.identifier);
        self.space(1);
        if self.always_ff {
            self.str("<");
        }
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.str(")");
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        self.newline_push();
        self.statement(&arg.statement);
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
        if !arg.if_statement_list.is_empty() {
            self.space(1);
        }
        for x in &arg.if_statement_list {
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.str("(");
            self.expression(&x.expression);
            self.str(")");
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
            self.newline_push();
            self.statement(&x.statement);
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
            self.newline_push();
            self.statement(&x.statement);
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
    }

    /// Semantic action for non-terminal 'VariableDeclaration'
    fn variable_declaration(&mut self, arg: &VariableDeclaration) {
        self.type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.type_right(&arg.r#type);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ParameterDeclaration'
    fn parameter_declaration(&mut self, arg: &ParameterDeclaration) {
        self.parameter(&arg.parameter);
        self.space(1);
        self.type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.type_right(&arg.r#type);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'LocalparamDeclaration'
    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) {
        self.localparam(&arg.localparam);
        self.space(1);
        self.type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.type_right(&arg.r#type);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
        self.always_ff = true;
        self.always_ff(&arg.always_ff);
        self.space(1);
        self.str("@");
        self.space(1);
        self.l_paren(&arg.l_paren);
        self.always_ff_conditions(&arg.always_ff_conditions);
        self.r_paren(&arg.r_paren);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        self.newline_push();
        for (i, x) in arg.always_ff_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
        self.always_ff = false;
    }

    /// Semantic action for non-terminal 'AlwaysFfConditions'
    fn always_ff_conditions(&mut self, arg: &AlwaysFfConditions) {
        self.always_ff_condition(&arg.always_ff_condition);
        for x in &arg.always_ff_conditions_list {
            self.comma(&x.comma);
            self.space(1);
            self.always_ff_condition(&x.always_ff_condition);
        }
        if let Some(ref x) = arg.always_ff_conditions_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'AlwaysFfCondition'
    fn always_ff_condition(&mut self, arg: &AlwaysFfCondition) {
        match &*arg.always_ff_condition_group {
            AlwaysFfConditionGroup::AlwaysFfConditionGroup0(x) => self.posedge(&x.posedge),
            AlwaysFfConditionGroup::AlwaysFfConditionGroup1(x) => self.negedge(&x.negedge),
        };
        self.space(1);
        self.identifier(&arg.identifier);
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        self.always_comb(&arg.always_comb);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        self.newline_push();
        for (i, x) in arg.always_comb_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        self.assign(&arg.assign);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.assign_declaration_opt {
            self.colon(&x.colon);
            self.space(1);
            self.r#type(&x.r#type);
        }
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ModportDeclaration'
    fn modport_declaration(&mut self, arg: &ModportDeclaration) {
        self.modport(&arg.modport);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("("));
        self.newline_push();
        self.modport_list(&arg.modport_list);
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace(")"));
        self.str(";");
    }

    /// Semantic action for non-terminal 'ModportList'
    fn modport_list(&mut self, arg: &ModportList) {
        self.modport_item(&arg.modport_item);
        for x in &arg.modport_list_list {
            self.comma(&x.comma);
            self.newline();
            self.modport_item(&x.modport_item);
        }
        if let Some(ref x) = arg.modport_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'ModportItem'
    fn modport_item(&mut self, arg: &ModportItem) {
        self.direction(&arg.direction);
        self.space(1);
        self.identifier(&arg.identifier);
    }

    /// Semantic action for non-terminal 'WithParameter'
    fn with_parameter(&mut self, arg: &WithParameter) {
        if let Some(ref x) = arg.with_parameter_opt {
            self.hash(&arg.hash);
            self.token_will_push(&arg.l_paren.l_paren_token);
            self.newline_push();
            self.with_parameter_list(&x.with_parameter_list);
            self.newline_pop();
            self.r_paren(&arg.r_paren);
        } else {
            self.hash(&arg.hash);
            self.l_paren(&arg.l_paren);
            self.r_paren(&arg.r_paren);
        }
    }

    /// Semantic action for non-terminal 'WithParameterList'
    fn with_parameter_list(&mut self, arg: &WithParameterList) {
        self.with_parameter_item(&arg.with_parameter_item);
        for x in &arg.with_parameter_list_list {
            self.comma(&x.comma);
            self.newline();
            self.with_parameter_item(&x.with_parameter_item);
        }
        if let Some(ref x) = arg.with_parameter_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::WithParameterItemGroup0(x) => self.parameter(&x.parameter),
            WithParameterItemGroup::WithParameterItemGroup1(x) => self.localparam(&x.localparam),
        };
        self.space(1);
        self.type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.type_right(&arg.r#type);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        self.module(&arg.module);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        if let Some(ref x) = arg.module_declaration_opt {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        if let Some(ref x) = arg.module_declaration_opt0 {
            self.module_port(&x.module_port);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
        self.newline_push();
        for (i, x) in arg.module_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_item(&x.module_item);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("endmodule"));
    }

    /// Semantic action for non-terminal 'ModulePort'
    fn module_port(&mut self, arg: &ModulePort) {
        if let Some(ref x) = arg.module_port_opt {
            self.token_will_push(&arg.l_paren.l_paren_token);
            self.newline_push();
            self.module_port_list(&x.module_port_list);
            self.newline_pop();
            self.r_paren(&arg.r_paren);
        } else {
            self.l_paren(&arg.l_paren);
            self.r_paren(&arg.r_paren);
        }
    }

    /// Semantic action for non-terminal 'ModulePortList'
    fn module_port_list(&mut self, arg: &ModulePortList) {
        self.module_port_item(&arg.module_port_item);
        for x in &arg.module_port_list_list {
            self.comma(&x.comma);
            self.newline();
            self.module_port_item(&x.module_port_item);
        }
        if let Some(ref x) = arg.module_port_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'ModulePortItem'
    fn module_port_item(&mut self, arg: &ModulePortItem) {
        self.direction(&arg.direction);
        self.space(1);
        self.r#type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.r#type_right(&arg.r#type);
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        self.interface(&arg.interface);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        if let Some(ref x) = arg.interface_declaration_opt {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
        self.newline_push();
        for (i, x) in arg.interface_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_item(&x.interface_item);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("endinterface"));
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        self.start_token = true;
        self.start(&arg.start);
        self.start_token = false;
        if !arg.start.start_token.comments.is_empty() {
            self.newline();
        }
        for (i, x) in arg.veryl_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.description(&x.description);
        }
        self.newline();
    }
}
