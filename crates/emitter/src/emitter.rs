use crate::aligner::{Aligner, Location};
use veryl_metadata::{ClockType, Metadata, ResetType};
use veryl_parser::global_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, VerylToken};
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::Stringifier;

pub struct Emitter {
    pub indent_width: usize,
    pub clock_type: ClockType,
    pub reset_type: ResetType,
    string: String,
    indent: usize,
    line: usize,
    aligner: Aligner,
    last_newline: usize,
    in_start_token: bool,
    consumed_next_newline: bool,
    single_line: bool,
    in_always_ff: bool,
    in_function: bool,
    reset_signal: Option<String>,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            indent_width: 4,
            clock_type: ClockType::PosEdge,
            reset_type: ResetType::AsyncLow,
            string: String::new(),
            indent: 0,
            line: 1,
            aligner: Aligner::new(),
            last_newline: 0,
            in_start_token: false,
            consumed_next_newline: false,
            single_line: false,
            in_always_ff: false,
            in_function: false,
            reset_signal: None,
        }
    }
}

impl Emitter {
    pub fn new(metadata: &Metadata) -> Self {
        Self {
            indent_width: metadata.format.indent_width,
            clock_type: metadata.build.clock_type,
            reset_type: metadata.build.reset_type,
            ..Default::default()
        }
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

    fn push_token(&mut self, x: &Token) {
        let text = global_table::get_str_value(x.text).unwrap();
        let text = if text.ends_with('\n') {
            self.consumed_next_newline = true;
            text.trim_end()
        } else {
            &text
        };
        self.last_newline = text.matches('\n').count();
        self.str(text);
        self.line = x.line;
    }

    fn process_token(&mut self, x: &VerylToken, will_push: bool, duplicated: Option<usize>) {
        self.push_token(&x.token);

        let mut loc: Location = x.token.into();
        loc.duplicated = duplicated;
        if let Some(width) = self.aligner.additions.get(&loc) {
            self.space(*width);
        }

        if duplicated.is_some() {
            return;
        }

        // temporary indent to adjust indent of comments with the next push
        if will_push {
            self.indent += 1;
        }
        // detect line comment newline which will consume the next newline
        self.consumed_next_newline = false;
        for x in &x.comments {
            // insert space between comments in the same line
            if x.line == self.line && !self.in_start_token {
                self.space(1);
            }
            for _ in 0..x.line - (self.line + self.last_newline) {
                self.unindent();
                self.str("\n");
                self.indent();
            }
            self.push_token(x);
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
        self.process_token(x, false, None)
    }

    fn token_will_push(&mut self, x: &VerylToken) {
        self.process_token(x, true, None)
    }

    fn duplicated_token(&mut self, x: &VerylToken, i: usize) {
        self.process_token(x, false, Some(i))
    }

    fn type_left(&mut self, input: &Type) {
        match &*input.type_group {
            TypeGroup::BuiltinType(x) => {
                let (width, token) = match &*x.builtin_type {
                    BuiltinType::Logic(x) => (true, x.logic.logic_token.clone()),
                    BuiltinType::Bit(x) => (true, x.bit.bit_token.clone()),
                    BuiltinType::U32(x) => (false, x.u32.u32_token.replace("int unsigned")),
                    BuiltinType::U64(x) => (false, x.u64.u64_token.replace("longint unsigned")),
                    BuiltinType::I32(x) => (false, x.i32.i32_token.replace("int signed")),
                    BuiltinType::I64(x) => (false, x.i64.i64_token.replace("longint signed")),
                    BuiltinType::F32(x) => (false, x.f32.f32_token.replace("shortreal")),
                    BuiltinType::F64(x) => (false, x.f64.f64_token.replace("real")),
                };
                self.token(&token);
                if width {
                    self.space(1);
                    for x in &input.type_list {
                        self.width(&x.width);
                    }
                }
            }
            TypeGroup::ScopedIdentifier(x) => self.scoped_identifier(&x.scoped_identifier),
        }
    }

    fn type_right(&mut self, input: &Type) {
        let width = match &*input.type_group {
            TypeGroup::BuiltinType(x) => match &*x.builtin_type {
                BuiltinType::Logic(_) => false,
                BuiltinType::Bit(_) => false,
                BuiltinType::U32(_) => true,
                BuiltinType::U64(_) => true,
                BuiltinType::I32(_) => true,
                BuiltinType::I64(_) => true,
                BuiltinType::F32(_) => true,
                BuiltinType::F64(_) => true,
            },
            TypeGroup::ScopedIdentifier(_) => true,
        };
        if width {
            self.space(1);
            for x in &input.type_list {
                self.width(&x.width);
            }
        }
    }

    fn always_ff_reset_exist_in_sensitivity_list(&mut self, arg: &AlwaysFfReset) -> bool {
        if let Some(ref x) = arg.always_ff_reset_opt {
            match &*x.always_ff_reset_opt_group {
                AlwaysFfResetOptGroup::AsyncLow(_) => true,
                AlwaysFfResetOptGroup::AsyncHigh(_) => true,
                AlwaysFfResetOptGroup::SyncLow(_) => false,
                AlwaysFfResetOptGroup::SyncHigh(_) => false,
            }
        } else {
            match self.reset_type {
                ResetType::AsyncLow => true,
                ResetType::AsyncHigh => true,
                ResetType::SyncLow => false,
                ResetType::SyncHigh => false,
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
        self.expression01(&arg.expression01);
        for x in &arg.expression_list {
            self.space(1);
            self.operator01(&x.operator01);
            self.space(1);
            self.expression01(&x.expression01);
        }
    }

    /// Semantic action for non-terminal 'Expression01'
    fn expression01(&mut self, arg: &Expression01) {
        self.expression02(&arg.expression02);
        for x in &arg.expression01_list {
            self.space(1);
            self.operator02(&x.operator02);
            self.space(1);
            self.expression02(&x.expression02);
        }
    }

    /// Semantic action for non-terminal 'Expression02'
    fn expression02(&mut self, arg: &Expression02) {
        self.expression03(&arg.expression03);
        for x in &arg.expression02_list {
            self.space(1);
            self.operator03(&x.operator03);
            self.space(1);
            self.expression03(&x.expression03);
        }
    }

    /// Semantic action for non-terminal 'Expression03'
    fn expression03(&mut self, arg: &Expression03) {
        self.expression04(&arg.expression04);
        for x in &arg.expression03_list {
            self.space(1);
            self.operator04(&x.operator04);
            self.space(1);
            self.expression04(&x.expression04);
        }
    }

    /// Semantic action for non-terminal 'Expression04'
    fn expression04(&mut self, arg: &Expression04) {
        self.expression05(&arg.expression05);
        for x in &arg.expression04_list {
            self.space(1);
            self.operator05(&x.operator05);
            self.space(1);
            self.expression05(&x.expression05);
        }
    }

    /// Semantic action for non-terminal 'Expression05'
    fn expression05(&mut self, arg: &Expression05) {
        self.expression06(&arg.expression06);
        for x in &arg.expression05_list {
            self.space(1);
            self.operator06(&x.operator06);
            self.space(1);
            self.expression06(&x.expression06);
        }
    }

    /// Semantic action for non-terminal 'Expression06'
    fn expression06(&mut self, arg: &Expression06) {
        self.expression07(&arg.expression07);
        for x in &arg.expression06_list {
            self.space(1);
            self.operator07(&x.operator07);
            self.space(1);
            self.expression07(&x.expression07);
        }
    }

    /// Semantic action for non-terminal 'Expression07'
    fn expression07(&mut self, arg: &Expression07) {
        self.expression08(&arg.expression08);
        for x in &arg.expression07_list {
            self.space(1);
            self.operator08(&x.operator08);
            self.space(1);
            self.expression08(&x.expression08);
        }
    }

    /// Semantic action for non-terminal 'Expression08'
    fn expression08(&mut self, arg: &Expression08) {
        self.expression09(&arg.expression09);
        for x in &arg.expression08_list {
            self.space(1);
            self.operator09(&x.operator09);
            self.space(1);
            self.expression09(&x.expression09);
        }
    }

    /// Semantic action for non-terminal 'Expression09'
    fn expression09(&mut self, arg: &Expression09) {
        self.expression10(&arg.expression10);
        for x in &arg.expression09_list {
            self.space(1);
            self.operator10(&x.operator10);
            self.space(1);
            self.expression10(&x.expression10);
        }
    }

    /// Semantic action for non-terminal 'Expression10'
    fn expression10(&mut self, arg: &Expression10) {
        self.expression11(&arg.expression11);
        for x in &arg.expression10_list {
            self.space(1);
            self.operator11(&x.operator11);
            self.space(1);
            self.expression11(&x.expression11);
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

    /// Semantic action for non-terminal 'Range'
    fn range(&mut self, arg: &Range) {
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        if let Some(ref x) = arg.range_opt {
            match &*x.range_operator {
                RangeOperator::Step(_) => {
                    self.str("*(");
                    self.expression(&x.expression);
                    self.str(")+:(");
                    self.expression(&x.expression);
                    self.str(")");
                }
                _ => {
                    self.range_operator(&x.range_operator);
                    self.expression(&x.expression);
                }
            }
        }
        self.r_bracket(&arg.r_bracket);
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
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        self.space(1);
        if self.in_always_ff {
            self.str("<");
        }
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
        self.str("(");
        self.expression(&arg.expression);
        self.str(")");
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        self.newline_push();
        for (i, x) in arg.if_statement_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
        for x in &arg.if_statement_list0 {
            self.space(1);
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
            for (i, x) in x.if_statement_list0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.statement(&x.statement);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
            self.newline_push();
            for (i, x) in x.if_statement_opt_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.statement(&x.statement);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
    }

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        self.token(&arg.if_reset.if_reset_token.replace("if"));
        self.space(1);
        self.str("(");
        let reset_signal = self.reset_signal.clone().unwrap();
        self.str(&reset_signal);
        self.str(")");
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        self.newline_push();
        for (i, x) in arg.if_reset_statement_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
        for x in &arg.if_reset_statement_list0 {
            self.space(1);
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
            for (i, x) in x.if_reset_statement_list0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.statement(&x.statement);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
            self.newline_push();
            for (i, x) in x.if_reset_statement_opt_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.statement(&x.statement);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
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
        self.str("(");
        self.type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.type_right(&arg.r#type);
        self.space(1);
        self.str("=");
        self.space(1);
        self.expression(&arg.expression);
        self.str(";");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("<");
        self.space(1);
        self.expression(&arg.expression0);
        self.str(";");
        self.space(1);
        if let Some(ref x) = arg.for_statement_opt {
            self.identifier(&arg.identifier);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
        } else {
            self.identifier(&arg.identifier);
            self.str("++");
        }
        self.str(")");
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        self.newline_push();
        for (i, x) in arg.for_statement_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.statement(&x.statement);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'LetDeclaration'
    fn let_declaration(&mut self, arg: &LetDeclaration) {
        self.type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.type_right(&arg.r#type);
        if let Some(ref x) = arg.let_declaration_opt {
            self.str(";");
            self.newline();
            if !self.in_function {
                self.str("assign");
                self.space(1);
            }
            self.str(&arg.identifier.identifier_token.text());
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.expression(&x.expression);
        }
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
        self.in_always_ff = true;
        self.always_ff(&arg.always_ff);
        self.space(1);
        self.str("@");
        self.space(1);
        self.l_paren(&arg.l_paren);
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.always_ff_declaration_opt {
            if self.always_ff_reset_exist_in_sensitivity_list(&x.always_ff_reset) {
                self.comma(&x.comma);
                self.space(1);
            }
            self.always_ff_reset(&x.always_ff_reset);
        }
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
        self.in_always_ff = false;
    }

    /// Semantic action for non-terminal 'AlwaysFfClock'
    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) {
        if let Some(ref x) = arg.always_ff_clock_opt {
            match &*x.always_ff_clock_opt_group {
                AlwaysFfClockOptGroup::Posedge(x) => self.posedge(&x.posedge),
                AlwaysFfClockOptGroup::Negedge(x) => self.negedge(&x.negedge),
            }
        } else {
            match self.clock_type {
                ClockType::PosEdge => self.str("posedge"),
                ClockType::NegEdge => self.str("negedge"),
            }
        }
        self.space(1);
        self.hierarchical_identifier(&arg.hierarchical_identifier);
    }

    /// Semantic action for non-terminal 'AlwaysFfReset'
    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) {
        let prefix = if let Some(ref x) = arg.always_ff_reset_opt {
            match &*x.always_ff_reset_opt_group {
                AlwaysFfResetOptGroup::AsyncLow(x) => {
                    self.token(&x.async_low.async_low_token.replace("negedge"));
                    "!"
                }
                AlwaysFfResetOptGroup::AsyncHigh(x) => {
                    self.token(&x.async_high.async_high_token.replace("posedge"));
                    ""
                }
                AlwaysFfResetOptGroup::SyncLow(x) => {
                    self.token(&x.sync_low.sync_low_token.replace(""));
                    "!"
                }
                AlwaysFfResetOptGroup::SyncHigh(x) => {
                    self.token(&x.sync_high.sync_high_token.replace(""));
                    ""
                }
            }
        } else {
            match self.reset_type {
                ResetType::AsyncLow => {
                    self.str("negedge");
                    "!"
                }
                ResetType::AsyncHigh => {
                    self.str("posedge");
                    ""
                }
                ResetType::SyncLow => "!",
                ResetType::SyncHigh => "",
            }
        };
        if self.always_ff_reset_exist_in_sensitivity_list(arg) {
            self.space(1);
            self.hierarchical_identifier(&arg.hierarchical_identifier);
        }

        let mut stringifier = Stringifier::new();
        stringifier.hierarchical_identifier(&arg.hierarchical_identifier);
        self.reset_signal = Some(format!("{}{}", prefix, stringifier.as_str()));
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

    /// Semantic action for non-terminal 'EnumDeclaration'
    fn enum_declaration(&mut self, arg: &EnumDeclaration) {
        self.str("typedef");
        self.space(1);
        self.r#enum(&arg.r#enum);
        self.space(1);
        self.type_left(&arg.r#type);
        self.type_right(&arg.r#type);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.enum_list(&arg.enum_list);
        self.newline_pop();
        self.str("}");
        self.space(1);
        self.identifier(&arg.identifier);
        self.str(";");
        self.token(&arg.r_brace.r_brace_token.replace(""));
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
            self.token(&x.comma.comma_token.replace(""));
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
        self.str("typedef");
        self.space(1);
        self.r#struct(&arg.r#struct);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.struct_list(&arg.struct_list);
        self.newline_pop();
        self.str("}");
        self.space(1);
        self.identifier(&arg.identifier);
        self.str(";");
        self.token(&arg.r_brace.r_brace_token.replace(""));
    }

    /// Semantic action for non-terminal 'StructList'
    fn struct_list(&mut self, arg: &StructList) {
        self.struct_item(&arg.struct_item);
        for x in &arg.struct_list_list {
            self.token(&x.comma.comma_token.replace(";"));
            self.newline();
            self.struct_item(&x.struct_item);
        }
        if let Some(ref x) = arg.struct_list_opt {
            self.token(&x.comma.comma_token.replace(";"));
        } else {
            self.str(";");
        }
    }

    /// Semantic action for non-terminal 'StructItem'
    fn struct_item(&mut self, arg: &StructItem) {
        self.type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.type_right(&arg.r#type);
    }

    /// Semantic action for non-terminal 'InstDeclaration'
    fn inst_declaration(&mut self, arg: &InstDeclaration) {
        if arg.inst_declaration_opt1.is_none() {
            self.single_line = true;
        }
        self.token(&arg.inst.inst_token.replace(""));
        self.scoped_identifier(&arg.scoped_identifier);
        self.space(1);
        if let Some(ref x) = arg.inst_declaration_opt0 {
            self.inst_parameter(&x.inst_parameter);
            self.space(1);
        }
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.inst_declaration_opt {
            self.space(1);
            self.width(&x.width);
        }
        self.space(1);
        if let Some(ref x) = arg.inst_declaration_opt1 {
            self.token_will_push(&x.l_brace.l_brace_token.replace("("));
            self.newline_push();
            if let Some(ref x) = x.inst_declaration_opt2 {
                self.inst_port_list(&x.inst_port_list);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace(")"));
        } else {
            self.str("()");
        }
        self.semicolon(&arg.semicolon);
        self.single_line = false;
    }

    /// Semantic action for non-terminal 'InstParameter'
    fn inst_parameter(&mut self, arg: &InstParameter) {
        self.hash(&arg.hash);
        if self.single_line {
            self.l_paren(&arg.l_paren);
        } else {
            self.token_will_push(&arg.l_paren.l_paren_token);
            self.newline_push();
        }
        if let Some(ref x) = arg.inst_parameter_opt {
            self.inst_parameter_list(&x.inst_parameter_list);
        }
        if !self.single_line {
            self.newline_pop();
        }
        self.r_paren(&arg.r_paren);
    }

    /// Semantic action for non-terminal 'InstParameterList'
    fn inst_parameter_list(&mut self, arg: &InstParameterList) {
        self.inst_parameter_item(&arg.inst_parameter_item);
        for x in &arg.inst_parameter_list_list {
            self.comma(&x.comma);
            if self.single_line {
                self.space(1);
            } else {
                self.newline();
            }
            self.inst_parameter_item(&x.inst_parameter_item);
        }
        if let Some(ref x) = arg.inst_parameter_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'InstParameterItem'
    fn inst_parameter_item(&mut self, arg: &InstParameterItem) {
        self.str(".");
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("(");
        if let Some(ref x) = arg.inst_parameter_item_opt {
            self.token(&x.colon.colon_token.replace(""));
            self.expression(&x.expression);
        } else {
            self.duplicated_token(&arg.identifier.identifier_token, 0);
        }
        self.str(")");
    }

    /// Semantic action for non-terminal 'InstPortList'
    fn inst_port_list(&mut self, arg: &InstPortList) {
        self.inst_port_item(&arg.inst_port_item);
        for x in &arg.inst_port_list_list {
            self.comma(&x.comma);
            self.newline();
            self.inst_port_item(&x.inst_port_item);
        }
        if let Some(ref x) = arg.inst_port_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'InstPortItem'
    fn inst_port_item(&mut self, arg: &InstPortItem) {
        self.str(".");
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("(");
        if let Some(ref x) = arg.inst_port_item_opt {
            self.token(&x.colon.colon_token.replace(""));
            self.expression(&x.expression);
        } else {
            self.duplicated_token(&arg.identifier.identifier_token, 0);
        }
        self.str(")");
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
            WithParameterItemGroup::Parameter(x) => self.parameter(&x.parameter),
            WithParameterItemGroup::Localparam(x) => self.localparam(&x.localparam),
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
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        self.direction(&arg.direction);
        self.space(1);
        self.r#type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.r#type_right(&arg.r#type);
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        self.in_function = true;
        if let Some(ref x) = arg.function_declaration_opt {
            self.str("module");
            self.space(1);
            self.identifier(&arg.identifier);
            self.space(1);
            self.with_parameter(&x.with_parameter);
            self.str(";");
            self.newline_push();
        }
        self.function(&arg.function);
        self.space(1);
        self.str("automatic");
        self.space(1);
        self.type_left(&arg.r#type);
        self.type_right(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
            self.space(1);
        }
        self.token(&arg.minus_g_t.minus_g_t_token.replace(""));
        self.str(";");
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.function_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.function_item(&x.function_item);
        }
        self.newline_pop();
        if arg.function_declaration_opt.is_some() {
            self.str("endfunction");
            self.newline_pop();
            self.token(&arg.r_brace.r_brace_token.replace("endmodule"));
        } else {
            self.token(&arg.r_brace.r_brace_token.replace("endfunction"));
        }
        self.in_function = false;
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

    /// Semantic action for non-terminal 'ModuleIfDeclaration'
    fn module_if_declaration(&mut self, arg: &ModuleIfDeclaration) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.str(")");
        self.space(1);
        self.str("begin");
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.module_if_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_item(&x.module_item);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
        for x in &arg.module_if_declaration_list0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.str("(");
            self.expression(&x.expression);
            self.str(")");
            self.space(1);
            self.str("begin");
            if let Some(ref x) = x.module_if_declaration_opt {
                self.space(1);
                self.colon(&x.colon);
                self.identifier(&x.identifier);
            } else {
                self.space(1);
                self.str(":");
                self.str(&arg.identifier.identifier_token.text());
            }
            self.token_will_push(&x.l_brace.l_brace_token.replace(""));
            self.newline_push();
            for (i, x) in x.module_if_declaration_list0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.module_item(&x.module_item);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
        if let Some(ref x) = arg.module_if_declaration_opt0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.str("begin");
            if let Some(ref x) = x.module_if_declaration_opt1 {
                self.space(1);
                self.colon(&x.colon);
                self.identifier(&x.identifier);
            } else {
                self.space(1);
                self.str(":");
                self.str(&arg.identifier.identifier_token.text());
            }
            self.token_will_push(&x.l_brace.l_brace_token.replace(""));
            self.newline_push();
            for (i, x) in x.module_if_declaration_opt0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.module_item(&x.module_item);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
    }

    /// Semantic action for non-terminal 'ModuleForDeclaration'
    fn module_for_declaration(&mut self, arg: &ModuleForDeclaration) {
        self.r#for(&arg.r#for);
        self.space(1);
        self.str("(");
        self.str("genvar");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("=");
        self.space(1);
        self.expression(&arg.expression);
        self.str(";");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("<");
        self.space(1);
        self.expression(&arg.expression0);
        self.str(";");
        self.space(1);
        if let Some(ref x) = arg.module_for_declaration_opt {
            self.identifier(&arg.identifier);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
        } else {
            self.identifier(&arg.identifier);
            self.str("++");
        }
        self.str(")");
        self.space(1);
        self.str("begin");
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier0);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.module_for_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_item(&x.module_item);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
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

    /// Semantic action for non-terminal 'InterfaceIfDeclaration'
    fn interface_if_declaration(&mut self, arg: &InterfaceIfDeclaration) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.str(")");
        self.space(1);
        self.str("begin");
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.interface_if_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_item(&x.interface_item);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
        for x in &arg.interface_if_declaration_list0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.str("(");
            self.expression(&x.expression);
            self.str(")");
            self.space(1);
            self.str("begin");
            if let Some(ref x) = x.interface_if_declaration_opt {
                self.space(1);
                self.colon(&x.colon);
                self.identifier(&x.identifier);
            } else {
                self.space(1);
                self.str(":");
                self.str(&arg.identifier.identifier_token.text());
            }
            self.token_will_push(&x.l_brace.l_brace_token.replace(""));
            self.newline_push();
            for (i, x) in x.interface_if_declaration_list0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.interface_item(&x.interface_item);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
        if let Some(ref x) = arg.interface_if_declaration_opt0 {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.str("begin");
            if let Some(ref x) = x.interface_if_declaration_opt1 {
                self.space(1);
                self.colon(&x.colon);
                self.identifier(&x.identifier);
            } else {
                self.space(1);
                self.str(":");
                self.str(&arg.identifier.identifier_token.text());
            }
            self.token_will_push(&x.l_brace.l_brace_token.replace(""));
            self.newline_push();
            for (i, x) in x.interface_if_declaration_opt0_list.iter().enumerate() {
                if i != 0 {
                    self.newline();
                }
                self.interface_item(&x.interface_item);
            }
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
    }

    /// Semantic action for non-terminal 'InterfaceForDeclaration'
    fn interface_for_declaration(&mut self, arg: &InterfaceForDeclaration) {
        self.r#for(&arg.r#for);
        self.space(1);
        self.str("(");
        self.str("genvar");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("=");
        self.space(1);
        self.expression(&arg.expression);
        self.str(";");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("<");
        self.space(1);
        self.expression(&arg.expression0);
        self.str(";");
        self.space(1);
        if let Some(ref x) = arg.interface_for_declaration_opt {
            self.identifier(&arg.identifier);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
        } else {
            self.identifier(&arg.identifier);
            self.str("++");
        }
        self.str(")");
        self.space(1);
        self.str("begin");
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier0);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.interface_for_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_item(&x.interface_item);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
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
