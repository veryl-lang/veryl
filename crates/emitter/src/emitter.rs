use crate::aligner::{Aligner, Location};
use veryl_analyzer::symbol::SymbolKind;
use veryl_analyzer::symbol_table;
use veryl_metadata::{ClockType, Metadata, ResetType};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, VerylToken};
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::Stringifier;

pub enum AttributeType {
    Ifdef,
    Sv,
}

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
    adjust_line: bool,
    in_always_ff: bool,
    in_function: bool,
    in_generate: bool,
    in_direction_modport: bool,
    reset_signal: Option<String>,
    default_block: Option<String>,
    enum_name: Option<String>,
    file_scope_import: Vec<String>,
    attribute: Vec<AttributeType>,
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
            adjust_line: false,
            in_always_ff: false,
            in_function: false,
            in_generate: false,
            in_direction_modport: false,
            reset_signal: None,
            default_block: None,
            enum_name: None,
            file_scope_import: Vec::new(),
            attribute: Vec::new(),
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
        self.adjust_line = true;
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
        self.adjust_line = true;
    }

    fn newline(&mut self) {
        self.unindent();
        if !self.consumed_next_newline {
            self.str("\n");
        } else {
            self.consumed_next_newline = false;
        }
        self.indent();
        self.adjust_line = true;
    }

    fn space(&mut self, repeat: usize) {
        self.str(&" ".repeat(repeat));
    }

    fn push_token(&mut self, x: &Token) {
        if self.adjust_line && x.line > self.line + 1 {
            self.newline();
        }
        self.adjust_line = false;
        let text = resource_table::get_str_value(x.text).unwrap();
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
        for x in &input.type_list {
            if let TypeModifier::Tri(x) = &*x.type_modifier {
                self.tri(&x.tri);
                self.space(1);
            }
        }
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
                for x in &input.type_list {
                    if let TypeModifier::Signed(x) = &*x.type_modifier {
                        self.space(1);
                        self.signed(&x.signed);
                    }
                }
                if width {
                    self.space(1);
                    for x in &input.type_list0 {
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
            for x in &input.type_list0 {
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

    fn attribute_end(&mut self) {
        match self.attribute.pop() {
            Some(AttributeType::Ifdef) => {
                self.newline();
                self.str("`endif");
            }
            _ => (),
        }
    }
}

impl VerylWalker for Emitter {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.token(arg);
    }

    /// Semantic action for non-terminal 'Comma'
    fn comma(&mut self, arg: &Comma) {
        if self.string.ends_with("`endif") {
            self.string.truncate(self.string.len() - "`endif".len());
            self.veryl_token(&arg.comma_token);
            self.str("`endif");
        } else {
            self.veryl_token(&arg.comma_token);
        }
    }

    /// Semantic action for non-terminal 'ScopedIdentifier'
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) {
        self.identifier(&arg.identifier);
        for x in &arg.scoped_identifier_list {
            if self.in_direction_modport {
                self.str(".");
            } else {
                self.colon_colon(&x.colon_colon);
            }
            self.identifier(&x.identifier);
        }
    }

    /// Semantic action for non-terminal 'ExpressionIdentifier'
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) {
        if let Some(ref x) = arg.expression_identifier_opt {
            self.dollar(&x.dollar);
        }

        self.identifier(&arg.identifier);
        let symbol = symbol_table::resolve(arg);
        let is_enum_member = if let Ok(ref symbol) = symbol {
            if let Some(ref symbol) = symbol.found {
                matches!(symbol.kind, SymbolKind::EnumMember(_))
            } else {
                false
            }
        } else {
            false
        };

        match &*arg.expression_identifier_group {
            ExpressionIdentifierGroup::ColonColonIdentifierExpressionIdentifierGroupList(x) => {
                if is_enum_member {
                    self.str("_");
                } else {
                    self.colon_colon(&x.colon_colon);
                }
                self.identifier(&x.identifier);
                for x in &x.expression_identifier_group_list {
                    self.colon_colon(&x.colon_colon);
                    self.identifier(&x.identifier);
                }
            }
            ExpressionIdentifierGroup::ExpressionIdentifierGroupList0ExpressionIdentifierGroupList1(x) => {
                for x in &x.expression_identifier_group_list0 {
                    self.range(&x.range);
                }
                for x in &x.expression_identifier_group_list1 {
                    self.dot(&x.dot);
                    self.identifier(&x.identifier);
                    for x in &x.expression_identifier_group_list1_list {
                        self.range(&x.range);
                    }
                }
            }
        }
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
            match &*x.expression09_list_group {
                Expression09ListGroup::Operator10(x) => self.operator10(&x.operator10),
                Expression09ListGroup::Star(x) => self.star(&x.star),
            }
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

    /// Semantic action for non-terminal 'Expression11'
    fn expression11(&mut self, arg: &Expression11) {
        for x in &arg.expression11_list {
            self.scoped_identifier(&x.scoped_identifier);
            self.str("'(");
        }
        self.expression12(&arg.expression12);
        for _ in &arg.expression11_list {
            self.str(")");
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

    /// Semantic action for non-terminal 'ConcatenationList'
    fn concatenation_list(&mut self, arg: &ConcatenationList) {
        self.concatenation_item(&arg.concatenation_item);
        for x in &arg.concatenation_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.concatenation_item(&x.concatenation_item);
        }
        if let Some(ref x) = arg.concatenation_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'ConcatenationItem'
    fn concatenation_item(&mut self, arg: &ConcatenationItem) {
        if let Some(ref x) = arg.concatenation_item_opt {
            self.str("{");
            self.expression(&x.expression);
            self.str("{");
            self.expression(&arg.expression);
            self.str("}");
            self.str("}");
        } else {
            self.expression(&arg.expression);
        }
    }

    /// Semantic action for non-terminal 'IfExpression'
    fn if_expression(&mut self, arg: &IfExpression) {
        self.token(&arg.r#if.if_token.replace("(("));
        self.expression(&arg.expression);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(") ? ("));
        self.newline_push();
        self.expression(&arg.expression0);
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace(")"));
        self.space(1);
        for x in &arg.if_expression_list {
            self.token(&x.r#else.else_token.replace(":"));
            self.space(1);
            self.token(&x.r#if.if_token.replace("("));
            self.expression(&x.expression);
            self.token_will_push(&x.l_brace.l_brace_token.replace(") ? ("));
            self.newline_push();
            self.expression(&x.expression0);
            self.newline_pop();
            self.token(&x.r_brace.r_brace_token.replace(")"));
            self.space(1);
        }
        self.token(&arg.r#else.else_token.replace(":"));
        self.space(1);
        self.token_will_push(&arg.l_brace0.l_brace_token.replace("("));
        self.newline_push();
        self.expression(&arg.expression1);
        self.newline_pop();
        self.token(&arg.r_brace0.r_brace_token.replace("))"));
    }

    /// Semantic action for non-terminal 'CaseExpression'
    fn case_expression(&mut self, arg: &CaseExpression) {
        self.token(&arg.case.case_token.replace("(("));
        self.expression(&arg.expression);
        self.space(1);
        self.str("==");
        self.space(1);
        self.expression(&arg.expression0);
        self.str(") ? (");
        self.newline_push();
        self.expression(&arg.expression1);
        self.newline_pop();
        self.str(")");
        self.space(1);
        for x in &arg.case_expression_list {
            self.str(": (");
            self.expression(&arg.expression);
            self.space(1);
            self.str("==");
            self.space(1);
            self.expression(&x.expression);
            self.str(") ? (");
            self.newline_push();
            self.expression(&x.expression0);
            self.newline_pop();
            self.token(&x.comma.comma_token.replace(")"));
            self.space(1);
        }
        self.str(": (");
        self.newline_push();
        self.expression(&arg.expression2);
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("))"));
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
            match &*arg.assignment_statement_group {
                AssignmentStatementGroup::Equ(x) => self.equ(&x.equ),
                AssignmentStatementGroup::AssignmentOperator(x) => {
                    let token = format!(
                        "{}",
                        x.assignment_operator.assignment_operator_token.token.text
                    );
                    // remove trailing `=` from assignment operator
                    let token = &token[0..token.len() - 1];
                    self.str("=");
                    self.space(1);
                    self.hierarchical_identifier(&arg.hierarchical_identifier);
                    self.space(1);
                    self.str(token);
                }
            }
            self.space(1);
            if let AssignmentStatementGroup::AssignmentOperator(_) =
                &*arg.assignment_statement_group
            {
                self.str("(");
                self.expression(&arg.expression);
                self.str(")");
            } else {
                self.expression(&arg.expression);
            }
        } else {
            match &*arg.assignment_statement_group {
                AssignmentStatementGroup::Equ(x) => self.equ(&x.equ),
                AssignmentStatementGroup::AssignmentOperator(x) => {
                    self.assignment_operator(&x.assignment_operator)
                }
            }
            self.space(1);
            self.expression(&arg.expression);
        }
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

    /// Semantic action for non-terminal 'CaseStatement'
    fn case_statement(&mut self, arg: &CaseStatement) {
        self.case(&arg.case);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(")"));
        self.newline_push();
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.case_item(&x.case_item);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("endcase"));
    }

    /// Semantic action for non-terminal 'CaseItem'
    fn case_item(&mut self, arg: &CaseItem) {
        match &*arg.case_item_group {
            CaseItemGroup::Expression(x) => self.expression(&x.expression),
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.case_item_group0 {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::LBraceCaseItemGroup0ListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
                self.newline_push();
                for (i, x) in x.case_item_group0_list.iter().enumerate() {
                    if i != 0 {
                        self.newline();
                    }
                    self.statement(&x.statement);
                }
                self.newline_pop();
                self.token(&x.r_brace.r_brace_token.replace("end"));
            }
        }
    }

    /// Semantic action for non-terminal 'Attribute'
    fn attribute(&mut self, arg: &Attribute) {
        self.adjust_line = false;
        let identifier = arg.identifier.identifier_token.text();
        match identifier.as_str() {
            "ifdef" | "ifndef" => {
                if let Some(ref x) = arg.attribute_opt {
                    let comma = if self.string.trim_end().ends_with(',') {
                        self.unindent();
                        self.string.truncate(self.string.len() - ",\n".len());
                        self.newline();
                        true
                    } else {
                        false
                    };

                    self.str("`");
                    self.identifier(&arg.identifier);
                    self.space(1);
                    if let AttributeItem::Identifier(x) = &*x.attribute_list.attribute_item {
                        self.identifier(&x.identifier);
                    }
                    self.newline();
                    self.attribute.push(AttributeType::Ifdef);

                    if comma {
                        self.str(",");
                        self.newline();
                    }
                }
            }
            "sv" => {
                if let Some(ref x) = arg.attribute_opt {
                    self.str("(*");
                    self.space(1);
                    if let AttributeItem::Strin(x) = &*x.attribute_list.attribute_item {
                        let text = x.strin.string_token.text();
                        let text = &text[1..text.len() - 1];
                        let text = text.replace("\\\"", "\"");
                        self.str(&text);
                    }
                    self.space(1);
                    self.str("*)");
                    self.newline();
                }
            }
            _ => (),
        }
        self.adjust_line = false;
    }

    /// Semantic action for non-terminal 'VarDeclaration'
    fn var_declaration(&mut self, arg: &VarDeclaration) {
        self.type_left(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.type_right(&arg.r#type);
        if let Some(ref x) = arg.var_declaration_opt {
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
        self.modport_group(&arg.modport_group);
        for x in &arg.modport_list_list {
            self.comma(&x.comma);
            self.newline();
            self.modport_group(&x.modport_group);
        }
        if let Some(ref x) = arg.modport_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'ModportGroup'
    fn modport_group(&mut self, arg: &ModportGroup) {
        if let Some(ref x) = arg.modport_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.modport_group_group {
            ModportGroupGroup::LBraceModportListRBrace(x) => {
                self.modport_list(&x.modport_list);
            }
            ModportGroupGroup::ModportItem(x) => self.modport_item(&x.modport_item),
        }
        if arg.modport_group_opt.is_some() {
            self.attribute_end();
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
        self.enum_name = Some(arg.identifier.identifier_token.text());
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
        self.enum_group(&arg.enum_group);
        for x in &arg.enum_list_list {
            self.comma(&x.comma);
            self.newline();
            self.enum_group(&x.enum_group);
        }
        if let Some(ref x) = arg.enum_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'EnumGroup'
    fn enum_group(&mut self, arg: &EnumGroup) {
        if let Some(ref x) = arg.enum_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.enum_group_group {
            EnumGroupGroup::LBraceEnumListRBrace(x) => {
                self.enum_list(&x.enum_list);
            }
            EnumGroupGroup::EnumItem(x) => self.enum_item(&x.enum_item),
        }
        if arg.enum_group_opt.is_some() {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'EnumItem'
    fn enum_item(&mut self, arg: &EnumItem) {
        self.str(&self.enum_name.clone().unwrap());
        self.str("_");
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
        self.struct_group(&arg.struct_group);
        for x in &arg.struct_list_list {
            self.token(&x.comma.comma_token.replace(";"));
            self.newline();
            self.struct_group(&x.struct_group);
        }
        if let Some(ref x) = arg.struct_list_opt {
            self.token(&x.comma.comma_token.replace(";"));
        } else {
            self.str(";");
        }
    }

    /// Semantic action for non-terminal 'StructGroup'
    fn struct_group(&mut self, arg: &StructGroup) {
        if let Some(ref x) = arg.struct_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.struct_group_group {
            StructGroupGroup::LBraceStructListRBrace(x) => {
                self.struct_list(&x.struct_list);
            }
            StructGroupGroup::StructItem(x) => self.struct_item(&x.struct_item),
        }
        if arg.struct_group_opt.is_some() {
            self.attribute_end();
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
        self.identifier(&arg.identifier0);
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
            self.token_will_push(&x.l_paren.l_paren_token.replace("("));
            self.newline_push();
            if let Some(ref x) = x.inst_declaration_opt2 {
                self.inst_port_list(&x.inst_port_list);
            }
            self.newline_pop();
            self.token(&x.r_paren.r_paren_token.replace(")"));
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
        self.inst_parameter_group(&arg.inst_parameter_group);
        for x in &arg.inst_parameter_list_list {
            self.comma(&x.comma);
            if self.single_line {
                self.space(1);
            } else {
                self.newline();
            }
            self.inst_parameter_group(&x.inst_parameter_group);
        }
        if let Some(ref x) = arg.inst_parameter_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'InstParameterGroup'
    fn inst_parameter_group(&mut self, arg: &InstParameterGroup) {
        if let Some(ref x) = arg.inst_parameter_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.inst_parameter_group_group {
            InstParameterGroupGroup::LBraceInstParameterListRBrace(x) => {
                self.inst_parameter_list(&x.inst_parameter_list);
            }
            InstParameterGroupGroup::InstParameterItem(x) => {
                self.inst_parameter_item(&x.inst_parameter_item)
            }
        }
        if arg.inst_parameter_group_opt.is_some() {
            self.attribute_end();
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
        self.inst_port_group(&arg.inst_port_group);
        for x in &arg.inst_port_list_list {
            self.comma(&x.comma);
            self.newline();
            self.inst_port_group(&x.inst_port_group);
        }
        if let Some(ref x) = arg.inst_port_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'InstPortGroup'
    fn inst_port_group(&mut self, arg: &InstPortGroup) {
        if let Some(ref x) = arg.inst_port_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.inst_port_group_group {
            InstPortGroupGroup::LBraceInstPortListRBrace(x) => {
                self.inst_port_list(&x.inst_port_list);
            }
            InstPortGroupGroup::InstPortItem(x) => self.inst_port_item(&x.inst_port_item),
        }
        if arg.inst_port_group_opt.is_some() {
            self.attribute_end();
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
        self.with_parameter_group(&arg.with_parameter_group);
        for x in &arg.with_parameter_list_list {
            self.comma(&x.comma);
            self.newline();
            self.with_parameter_group(&x.with_parameter_group);
        }
        if let Some(ref x) = arg.with_parameter_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'WithParameterGroup'
    fn with_parameter_group(&mut self, arg: &WithParameterGroup) {
        if let Some(ref x) = arg.with_parameter_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.with_parameter_group_group {
            WithParameterGroupGroup::LBraceWithParameterListRBrace(x) => {
                self.with_parameter_list(&x.with_parameter_list);
            }
            WithParameterGroupGroup::WithParameterItem(x) => {
                self.with_parameter_item(&x.with_parameter_item)
            }
        }
        if arg.with_parameter_group_opt.is_some() {
            self.attribute_end();
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
        self.port_declaration_group(&arg.port_declaration_group);
        for x in &arg.port_declaration_list_list {
            self.comma(&x.comma);
            self.newline();
            self.port_declaration_group(&x.port_declaration_group);
        }
        if let Some(ref x) = arg.port_declaration_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationGroup'
    fn port_declaration_group(&mut self, arg: &PortDeclarationGroup) {
        if let Some(ref x) = arg.port_declaration_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.port_declaration_group_group {
            PortDeclarationGroupGroup::LBracePortDeclarationListRBrace(x) => {
                self.port_declaration_list(&x.port_declaration_list);
            }
            PortDeclarationGroupGroup::PortDeclarationItem(x) => {
                self.port_declaration_item(&x.port_declaration_item)
            }
        }
        if arg.port_declaration_group_opt.is_some() {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        match &*arg.port_declaration_item_group {
            PortDeclarationItemGroup::DirectionType(x) => {
                self.direction(&x.direction);
                if let Direction::Modport(_) = *x.direction {
                    self.in_direction_modport = true;
                } else {
                    self.space(1);
                }
                self.r#type_left(&x.r#type);
                self.space(1);
                self.identifier(&arg.identifier);
                self.r#type_right(&x.r#type);
                self.in_direction_modport = false;
            }
            PortDeclarationItemGroup::Interface(x) => {
                self.interface(&x.interface);
                self.space(1);
                self.identifier(&arg.identifier);
            }
        }
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        match arg {
            Direction::Input(x) => self.input(&x.input),
            Direction::Output(x) => self.output(&x.output),
            Direction::Inout(x) => self.inout(&x.inout),
            Direction::Ref(x) => self.r#ref(&x.r#ref),
            Direction::Modport(_) => (),
        };
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

    /// Semantic action for non-terminal 'ImportDeclaration'
    fn import_declaration(&mut self, arg: &ImportDeclaration) {
        self.import(&arg.import);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon_colon(&arg.colon_colon);
        match &*arg.import_declaration_group {
            ImportDeclarationGroup::Identifier(x) => self.identifier(&x.identifier),
            ImportDeclarationGroup::Star(x) => self.star(&x.star),
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ExportDeclaration'
    fn export_declaration(&mut self, arg: &ExportDeclaration) {
        self.export(&arg.export);
        self.space(1);
        match &*arg.export_declaration_group {
            ExportDeclarationGroup::Identifier(x) => self.identifier(&x.identifier),
            ExportDeclarationGroup::Star(x) => self.star(&x.star),
        }
        self.colon_colon(&arg.colon_colon);
        match &*arg.export_declaration_group0 {
            ExportDeclarationGroup0::Identifier(x) => self.identifier(&x.identifier),
            ExportDeclarationGroup0::Star(x) => self.star(&x.star),
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        self.module(&arg.module);
        self.space(1);
        self.identifier(&arg.identifier);
        let file_scope_import = self.file_scope_import.clone();
        if !file_scope_import.is_empty() {
            self.newline_push();
        }
        for (i, x) in file_scope_import.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.str(x);
        }
        if !file_scope_import.is_empty() {
            self.newline_pop();
        }
        if let Some(ref x) = arg.module_declaration_opt {
            self.space(1);
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.module_declaration_opt0 {
            self.space(1);
            self.port_declaration(&x.port_declaration);
        }
        self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
        self.newline_push();
        for (i, x) in arg.module_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_group(&x.module_group);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("endmodule"));
    }

    /// Semantic action for non-terminal 'ModuleIfDeclaration'
    fn module_if_declaration(&mut self, arg: &ModuleIfDeclaration) {
        self.in_generate = true;
        self.r#if(&arg.r#if);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.str(")");
        self.space(1);
        self.module_named_block(&arg.module_named_block);
        for x in &arg.module_if_declaration_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.str("(");
            self.expression(&x.expression);
            self.str(")");
            self.space(1);
            self.module_optional_named_block(&x.module_optional_named_block);
        }
        if let Some(ref x) = arg.module_if_declaration_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.module_optional_named_block(&x.module_optional_named_block);
        }
        self.in_generate = false;
    }

    /// Semantic action for non-terminal 'ModuleForDeclaration'
    fn module_for_declaration(&mut self, arg: &ModuleForDeclaration) {
        self.in_generate = true;
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
        self.module_named_block(&arg.module_named_block);
        self.in_generate = false;
    }

    /// Semantic action for non-terminal 'ModuleNamedBlock'
    fn module_named_block(&mut self, arg: &ModuleNamedBlock) {
        if !self.in_generate {
            self.str("if");
            self.space(1);
            self.str("(1)");
            self.space(1);
        }
        self.str("begin");
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.default_block = Some(arg.identifier.identifier_token.text());
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.module_named_block_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_group(&x.module_group);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'ModuleOptionalNamedBlock'
    fn module_optional_named_block(&mut self, arg: &ModuleOptionalNamedBlock) {
        self.str("begin");
        if let Some(ref x) = arg.module_optional_named_block_opt {
            self.space(1);
            self.colon(&x.colon);
            self.identifier(&x.identifier);
        } else {
            self.space(1);
            self.str(":");
            let name = self.default_block.clone().unwrap();
            self.str(&name);
        }
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.module_optional_named_block_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_group(&x.module_group);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'ModuleGroup'
    fn module_group(&mut self, arg: &ModuleGroup) {
        if let Some(ref x) = arg.module_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.module_group_group {
            ModuleGroupGroup::LBraceModuleGroupGroupListRBrace(x) => {
                for (i, x) in x.module_group_group_list.iter().enumerate() {
                    if i != 0 {
                        self.newline();
                    }
                    self.module_group(&x.module_group);
                }
            }
            ModuleGroupGroup::ModuleItem(x) => self.module_item(&x.module_item),
        }
        if arg.module_group_opt.is_some() {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        self.interface(&arg.interface);
        self.space(1);
        self.identifier(&arg.identifier);
        let file_scope_import = self.file_scope_import.clone();
        if !file_scope_import.is_empty() {
            self.newline_push();
        }
        for (i, x) in file_scope_import.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.str(x);
        }
        if !file_scope_import.is_empty() {
            self.newline_pop();
        }
        if let Some(ref x) = arg.interface_declaration_opt {
            self.space(1);
            self.with_parameter(&x.with_parameter);
        }
        self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
        self.newline_push();
        for (i, x) in arg.interface_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_group(&x.interface_group);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("endinterface"));
    }

    /// Semantic action for non-terminal 'InterfaceIfDeclaration'
    fn interface_if_declaration(&mut self, arg: &InterfaceIfDeclaration) {
        self.in_generate = true;
        self.r#if(&arg.r#if);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.str(")");
        self.space(1);
        self.interface_named_block(&arg.interface_named_block);
        for x in &arg.interface_if_declaration_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.str("(");
            self.expression(&x.expression);
            self.str(")");
            self.space(1);
            self.interface_optional_named_block(&x.interface_optional_named_block);
        }
        if let Some(ref x) = arg.interface_if_declaration_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.interface_optional_named_block(&x.interface_optional_named_block);
        }
        self.in_generate = false;
    }

    /// Semantic action for non-terminal 'InterfaceForDeclaration'
    fn interface_for_declaration(&mut self, arg: &InterfaceForDeclaration) {
        self.in_generate = true;
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
        self.interface_named_block(&arg.interface_named_block);
        self.in_generate = false;
    }

    /// Semantic action for non-terminal 'InterfaceNamedBlock'
    fn interface_named_block(&mut self, arg: &InterfaceNamedBlock) {
        if !self.in_generate {
            self.str("if");
            self.space(1);
            self.str("(1)");
            self.space(1);
        }
        self.str("begin");
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.default_block = Some(arg.identifier.identifier_token.text());
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.interface_named_block_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_group(&x.interface_group);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'InterfaceOptionalNamedBlock'
    fn interface_optional_named_block(&mut self, arg: &InterfaceOptionalNamedBlock) {
        self.str("begin");
        if let Some(ref x) = arg.interface_optional_named_block_opt {
            self.space(1);
            self.colon(&x.colon);
            self.identifier(&x.identifier);
        } else {
            self.space(1);
            self.str(":");
            let name = self.default_block.clone().unwrap();
            self.str(&name);
        }
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        self.newline_push();
        for (i, x) in arg.interface_optional_named_block_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.interface_group(&x.interface_group);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'InterfaceGroup'
    fn interface_group(&mut self, arg: &InterfaceGroup) {
        if let Some(ref x) = arg.interface_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.interface_group_group {
            InterfaceGroupGroup::LBraceInterfaceGroupGroupListRBrace(x) => {
                for (i, x) in x.interface_group_group_list.iter().enumerate() {
                    if i != 0 {
                        self.newline();
                    }
                    self.interface_group(&x.interface_group);
                }
            }
            InterfaceGroupGroup::InterfaceItem(x) => self.interface_item(&x.interface_item),
        }
        if arg.interface_group_opt.is_some() {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'PackageDeclaration'
    fn package_declaration(&mut self, arg: &PackageDeclaration) {
        self.package(&arg.package);
        self.space(1);
        self.identifier(&arg.identifier);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
        self.newline_push();
        let file_scope_import = self.file_scope_import.clone();
        for x in &file_scope_import {
            self.str(x);
            self.newline();
        }
        for (i, x) in arg.package_declaration_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.package_group(&x.package_group);
        }
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("endpackage"));
    }

    /// Semantic action for non-terminal 'PackageGroup'
    fn package_group(&mut self, arg: &PackageGroup) {
        if let Some(ref x) = arg.package_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.package_group_group {
            PackageGroupGroup::LBracePackageGroupGroupListRBrace(x) => {
                for (i, x) in x.package_group_group_list.iter().enumerate() {
                    if i != 0 {
                        self.newline();
                    }
                    self.package_group(&x.package_group);
                }
            }
            PackageGroupGroup::PackageItem(x) => self.package_item(&x.package_item),
        }
        if arg.package_group_opt.is_some() {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'DescriptionGroup'
    fn description_group(&mut self, arg: &DescriptionGroup) {
        if let Some(ref x) = arg.description_group_opt {
            self.attribute(&x.attribute);
        }
        match &*arg.description_group_group {
            DescriptionGroupGroup::LBraceDescriptionGroupGroupListRBrace(x) => {
                for (i, x) in x.description_group_group_list.iter().enumerate() {
                    if i != 0 {
                        self.newline();
                    }
                    self.description_group(&x.description_group);
                }
            }
            DescriptionGroupGroup::DescriptionItem(x) => self.description_item(&x.description_item),
        }
        if arg.description_group_opt.is_some() {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'DescriptionItem'
    fn description_item(&mut self, arg: &DescriptionItem) {
        match arg {
            DescriptionItem::ModuleDeclaration(x) => self.module_declaration(&x.module_declaration),
            DescriptionItem::InterfaceDeclaration(x) => {
                self.interface_declaration(&x.interface_declaration)
            }
            DescriptionItem::PackageDeclaration(x) => {
                self.package_declaration(&x.package_declaration)
            }
            // file scope import is not emitted at SystemVerilog
            DescriptionItem::ImportDeclaration(_) => (),
        };
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        self.in_start_token = true;
        self.start(&arg.start);
        self.in_start_token = false;
        if !arg.start.start_token.comments.is_empty() {
            self.newline();
        }
        for x in &arg.veryl_list {
            let items: Vec<DescriptionItem> = x.description_group.as_ref().into();
            for item in items {
                if let DescriptionItem::ImportDeclaration(x) = item {
                    let mut emitter = Emitter::default();
                    emitter.import_declaration(&x.import_declaration);
                    self.file_scope_import.push(emitter.as_str().to_string());
                }
            }
        }
        for (i, x) in arg.veryl_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.description_group(&x.description_group);
        }
        self.newline();
    }
}
