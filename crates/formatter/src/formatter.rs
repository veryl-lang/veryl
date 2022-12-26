use crate::aligner::{Aligner, Location};
use veryl_metadata::Metadata;
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
    in_start_token: bool,
    consumed_next_newline: bool,
}

impl Default for Formatter {
    fn default() -> Self {
        Self {
            indent_width: 4,
            string: String::new(),
            indent: 0,
            line: 1,
            aligner: Aligner::new(),
            last_newline: 0,
            in_start_token: false,
            consumed_next_newline: false,
        }
    }
}

impl Formatter {
    pub fn new(metadata: &Metadata) -> Self {
        Self {
            indent_width: metadata.format.indent_width,
            ..Default::default()
        }
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

    fn unindent(&mut self) {
        if self
            .string
            .ends_with(&" ".repeat(self.indent * self.indent_width))
        {
            self.string
                .truncate(self.string.len() - self.indent * self.indent_width);
        }
    }

    fn indent(&mut self) {
        self.str(&" ".repeat(self.indent * self.indent_width));
    }

    fn newline_push(&mut self) {
        self.unindent();
        if !self.consumed_next_newline {
            self.str("\n");
        } else {
            self.consumed_next_newline = false;
        }
        self.indent += 1;
        self.indent();
    }

    fn newline_pop(&mut self) {
        self.unindent();
        if !self.consumed_next_newline {
            self.str("\n");
        } else {
            self.consumed_next_newline = false;
        }
        self.indent -= 1;
        self.indent();
    }

    fn newline(&mut self) {
        self.unindent();
        if !self.consumed_next_newline {
            self.str("\n");
        } else {
            self.consumed_next_newline = false;
        }
        self.indent();
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
            self.consumed_next_newline = true;
            text.trim_end()
        } else {
            text
        };
        self.last_newline = text.matches('\n').count();
        self.str(text);
        self.line = x.location.line;
    }

    fn process_token(&mut self, x: &VerylToken, will_push: bool) {
        self.parol_token(x.parol_token(), true);

        let loc: Location = x.location().into();
        if let Some(width) = self.aligner.additions.get(&loc) {
            self.space(*width);
        }

        // temporary indent to adjust indent of comments with the next push
        if will_push {
            self.indent += 1;
        }
        // detect line comment newline which will consume the next newline
        self.consumed_next_newline = false;
        for x in &x.comments {
            // insert space between comments in the same line
            if x.token.location.line == self.line && !self.in_start_token {
                self.space(1);
            }
            for _ in 0..x.token.location.line - (self.line + self.last_newline) {
                self.unindent();
                self.str("\n");
                self.indent();
            }
            self.parol_token(&x.token, false);
        }
        if will_push {
            self.indent -= 1;
        }
        if self.consumed_next_newline {
            self.unindent();
            self.str("\n");
            self.indent();
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
                ExpressionListGroup::BinaryOperator(x) => self.binary_operator(&x.binary_operator),
                ExpressionListGroup::CommonOperator(x) => self.common_operator(&x.common_operator),
            };
            self.space(1);
            self.expression1(&x.expression1);
        }
    }

    /// Semantic action for non-terminal 'FunctionCallArg'
    fn function_call_arg(&mut self, arg: &FunctionCallArg) {
        self.expression(&arg.expression);
        for x in &arg.function_call_arg_list {
            self.comma(&x.comma);
            self.space(1);
            self.expression(&x.expression);
        }
        if let Some(ref x) = arg.function_call_arg_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'Type'
    fn r#type(&mut self, arg: &Type) {
        match &*arg.type_group {
            TypeGroup::BuiltinType(x) => self.builtin_type(&x.builtin_type),
            TypeGroup::Identifier(x) => self.identifier(&x.identifier),
        };
        self.space(1);
        for x in &arg.type_list {
            self.width(&x.width);
        }
    }

    /// Semantic action for non-terminal 'AssignmentStatement'
    fn assignment_statement(&mut self, arg: &AssignmentStatement) {
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        self.space(1);
        match &*arg.assignment_statement_group {
            AssignmentStatementGroup::Equ(x) => self.equ(&x.equ),
            AssignmentStatementGroup::AssignmentOperator(x) => {
                self.assignment_operator(&x.assignment_operator)
            }
        }
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.if_statement_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
        for x in &arg.if_statement_list0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            for (i, x) in x.if_statement_list0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.statement(&x.statement);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            for (i, x) in x.if_statement_opt_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.statement(&x.statement);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
    }

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        self.if_reset(&arg.if_reset);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.if_reset_statement_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
        for x in &arg.if_reset_statement_list0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            for (i, x) in x.if_reset_statement_list0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.statement(&x.statement);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            for (i, x) in x.if_reset_statement_opt_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.statement(&x.statement);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
    }

    /// Semantic action for non-terminal 'ReturnStatement'
    fn return_statement(&mut self, arg: &ReturnStatement) {
        self.r#return(&arg.r#return);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ForStatement'
    fn for_statement(&mut self, arg: &ForStatement) {
        self.r#for(&arg.r#for);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.r#type(&arg.r#type);
        self.space(1);
        self.r#in(&arg.r#in);
        self.space(1);
        self.expression(&arg.expression);
        self.dot_dot(&arg.dot_dot);
        self.expression(&arg.expression0);
        self.space(1);
        if let Some(ref x) = arg.for_statement_opt {
            self.step(&x.step);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.for_statement_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'LetDeclaration'
    fn let_declaration(&mut self, arg: &LetDeclaration) {
        self.r#let(&arg.r#let);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.let_declaration_group {
            LetDeclarationGroup::VariableDeclaration(x) => {
                self.variable_declaration(&x.variable_declaration)
            }
            LetDeclarationGroup::InstanceDeclaration(x) => {
                self.instance_declaration(&x.instance_declaration)
            }
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'VariableDeclaration'
    fn variable_declaration(&mut self, arg: &VariableDeclaration) {
        self.r#type(&arg.r#type);
        if let Some(ref x) = arg.variable_declaration_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'ParameterDeclaration'
    fn parameter_declaration(&mut self, arg: &ParameterDeclaration) {
        self.parameter(&arg.parameter);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.r#type(&arg.r#type);
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
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.r#type(&arg.r#type);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
        self.always_ff(&arg.always_ff);
        self.space(1);
        self.l_paren(&arg.l_paren);
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.always_ff_declaration_opt {
            self.comma(&x.comma);
            self.space(1);
            self.always_ff_reset(&x.always_ff_reset);
        }
        self.r_paren(&arg.r_paren);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.always_ff_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'AlwaysFfClock'
    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) {
        if let Some(ref x) = arg.always_ff_clock_opt {
            match &*x.always_ff_clock_opt_group {
                AlwaysFfClockOptGroup::Posedge(x) => self.posedge(&x.posedge),
                AlwaysFfClockOptGroup::Negedge(x) => self.negedge(&x.negedge),
            }
            self.space(1);
        }
        self.hierarchical_identifier(&arg.hierarchical_identifier);
    }

    /// Semantic action for non-terminal 'AlwaysFfReset'
    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) {
        if let Some(ref x) = arg.always_ff_reset_opt {
            match &*x.always_ff_reset_opt_group {
                AlwaysFfResetOptGroup::AsyncLow(x) => self.async_low(&x.async_low),
                AlwaysFfResetOptGroup::AsyncHigh(x) => self.async_high(&x.async_high),
                AlwaysFfResetOptGroup::SyncLow(x) => self.sync_low(&x.sync_low),
                AlwaysFfResetOptGroup::SyncHigh(x) => self.sync_high(&x.sync_high),
            }
            self.space(1);
        }
        self.hierarchical_identifier(&arg.hierarchical_identifier);
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        self.always_comb(&arg.always_comb);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.always_comb_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        self.assign(&arg.assign);
        self.space(1);
        self.hierarchical_identifier(&arg.hierarchical_identifier);
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
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.modport_list(&arg.modport_list);
        self.newline_pop();
        self.r_brace(&arg.r_brace);
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
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'ModportItem'
    fn modport_item(&mut self, arg: &ModportItem) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.direction(&arg.direction);
    }

    /// Semantic action for non-terminal 'EnumDeclaration'
    fn enum_declaration(&mut self, arg: &EnumDeclaration) {
        self.r#enum(&arg.r#enum);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.r#type(&arg.r#type);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.enum_list(&arg.enum_list);
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'EnumList'
    fn enum_list(&mut self, arg: &EnumList) {
        self.enum_item(&arg.enum_item);
        for x in &arg.enum_list_list {
            self.comma(&x.comma);
            self.newline();
            self.enum_item(&x.enum_item);
        }
        if let Some(ref x) = arg.enum_list_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'EnumItem'
    fn enum_item(&mut self, arg: &EnumItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.enum_item_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'StructDeclaration'
    fn struct_declaration(&mut self, arg: &StructDeclaration) {
        self.r#struct(&arg.r#struct);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.struct_list(&arg.struct_list);
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'StructList'
    fn struct_list(&mut self, arg: &StructList) {
        self.struct_item(&arg.struct_item);
        for x in &arg.struct_list_list {
            self.comma(&x.comma);
            self.newline();
            self.struct_item(&x.struct_item);
        }
        if let Some(ref x) = arg.struct_list_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'StructItem'
    fn struct_item(&mut self, arg: &StructItem) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.r#type(&arg.r#type);
    }

    /// Semantic action for non-terminal 'InstanceDeclaration'
    fn instance_declaration(&mut self, arg: &InstanceDeclaration) {
        self.inst(&arg.inst);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_declaration_opt {
            self.space(1);
            self.width(&x.width);
        }
        if let Some(ref x) = arg.instance_declaration_opt0 {
            self.space(1);
            self.instance_parameter(&x.instance_parameter);
        }
        if let Some(ref x) = arg.instance_declaration_opt1 {
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            if let Some(ref x) = x.instance_declaration_opt2 {
                self.instance_port_list(&x.instance_port_list);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
    }

    /// Semantic action for non-terminal 'InstanceParameter'
    fn instance_parameter(&mut self, arg: &InstanceParameter) {
        self.hash(&arg.hash);
        self.token_will_push(&arg.l_paren.l_paren_token);
        self.newline_push();
        if let Some(ref x) = arg.instance_parameter_opt {
            self.instance_parameter_list(&x.instance_parameter_list);
        }
        self.newline_pop();
        self.r_paren(&arg.r_paren);
    }

    /// Semantic action for non-terminal 'InstanceParameterList'
    fn instance_parameter_list(&mut self, arg: &InstanceParameterList) {
        self.instance_parameter_item(&arg.instance_parameter_item);
        for x in &arg.instance_parameter_list_list {
            self.comma(&x.comma);
            self.newline();
            self.instance_parameter_item(&x.instance_parameter_item);
        }
        if let Some(ref x) = arg.instance_parameter_list_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'InstanceParameterItem'
    fn instance_parameter_item(&mut self, arg: &InstanceParameterItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_parameter_item_opt {
            self.colon(&x.colon);
            self.space(1);
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'InstancePortList'
    fn instance_port_list(&mut self, arg: &InstancePortList) {
        self.instance_port_item(&arg.instance_port_item);
        for x in &arg.instance_port_list_list {
            self.comma(&x.comma);
            self.newline();
            self.instance_port_item(&x.instance_port_item);
        }
        if let Some(ref x) = arg.instance_port_list_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'InstancePortItem'
    fn instance_port_item(&mut self, arg: &InstancePortItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.instance_port_item_opt {
            self.colon(&x.colon);
            self.space(1);
            self.expression(&x.expression);
        }
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
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::Parameter(x) => self.parameter(&x.parameter),
            WithParameterItemGroup::Localparam(x) => self.localparam(&x.localparam),
        };
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.r#type(&arg.r#type);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
    }

    /// Semantic action for non-terminal 'PortDeclaration'
    fn port_declaration(&mut self, arg: &PortDeclaration) {
        if let Some(ref x) = arg.port_declaration_opt {
            self.token_will_push(&arg.l_paren.l_paren_token);
            self.newline_push();
            self.port_declaration_list(&x.port_declaration_list);
            self.newline_pop();
            self.r_paren(&arg.r_paren);
        } else {
            self.l_paren(&arg.l_paren);
            self.r_paren(&arg.r_paren);
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationList'
    fn port_declaration_list(&mut self, arg: &PortDeclarationList) {
        self.port_declaration_item(&arg.port_declaration_item);
        for x in &arg.port_declaration_list_list {
            self.comma(&x.comma);
            self.newline();
            self.port_declaration_item(&x.port_declaration_item);
        }
        if let Some(ref x) = arg.port_declaration_list_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.direction(&arg.direction);
        self.space(1);
        self.r#type(&arg.r#type);
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        self.function(&arg.function);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        if let Some(ref x) = arg.function_declaration_opt {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
            self.space(1);
        }
        self.minus_g_t(&arg.minus_g_t);
        self.space(1);
        self.r#type(&arg.r#type);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.function_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.function_item(&x.function_item);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
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
            self.port_declaration(&x.port_declaration);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.module_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_item(&x.module_item);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ModuleIfDeclaration'
    fn module_if_declaration(&mut self, arg: &ModuleIfDeclaration) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.module_if_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_item(&x.module_item);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
        for x in &arg.module_if_declaration_list0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            if let Some(ref x) = x.module_if_declaration_opt {
                self.colon(&x.colon);
                self.identifier(&x.identifier);
                self.space(1);
            }
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            for (i, x) in x.module_if_declaration_list0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.module_item(&x.module_item);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.module_if_declaration_opt0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            if let Some(ref x) = x.module_if_declaration_opt1 {
                self.colon(&x.colon);
                self.identifier(&x.identifier);
                self.space(1);
            }
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            for (i, x) in x.module_if_declaration_opt0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.module_item(&x.module_item);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
    }

    /// Semantic action for non-terminal 'ModuleForDeclaration'
    fn module_for_declaration(&mut self, arg: &ModuleForDeclaration) {
        self.r#for(&arg.r#for);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.r#in(&arg.r#in);
        self.space(1);
        self.expression(&arg.expression);
        self.dot_dot(&arg.dot_dot);
        self.expression(&arg.expression0);
        self.space(1);
        if let Some(ref x) = arg.module_for_declaration_opt {
            self.step(&x.step);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
        }
        self.colon(&arg.colon);
        self.identifier(&arg.identifier0);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.module_for_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_item(&x.module_item);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
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
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.interface_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_item(&x.interface_item);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'InterfaceIfDeclaration'
    fn interface_if_declaration(&mut self, arg: &InterfaceIfDeclaration) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.interface_if_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_item(&x.interface_item);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
        for x in &arg.interface_if_declaration_list0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            if let Some(ref x) = x.interface_if_declaration_opt {
                self.colon(&x.colon);
                self.identifier(&x.identifier);
                self.space(1);
            }
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            for (i, x) in x.interface_if_declaration_list0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.interface_item(&x.interface_item);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.interface_if_declaration_opt0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            if let Some(ref x) = x.interface_if_declaration_opt1 {
                self.colon(&x.colon);
                self.identifier(&x.identifier);
                self.space(1);
            }
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            for (i, x) in x.interface_if_declaration_opt0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.interface_item(&x.interface_item);
            }
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
    }

    /// Semantic action for non-terminal 'InterfaceForDeclaration'
    fn interface_for_declaration(&mut self, arg: &InterfaceForDeclaration) {
        self.r#for(&arg.r#for);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.r#in(&arg.r#in);
        self.space(1);
        self.expression(&arg.expression);
        self.dot_dot(&arg.dot_dot);
        self.expression(&arg.expression0);
        self.space(1);
        if let Some(ref x) = arg.interface_for_declaration_opt {
            self.step(&x.step);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
        }
        self.colon(&arg.colon);
        self.identifier(&arg.identifier0);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.interface_for_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_item(&x.interface_item);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        self.in_start_token = true;
        self.start(&arg.start);
        self.in_start_token = false;
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
