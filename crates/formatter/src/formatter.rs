use crate::aligner::{Aligner, Location};
use veryl_metadata::{Format, Metadata};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, VerylToken};
use veryl_parser::veryl_walker::VerylWalker;

#[cfg(target_os = "windows")]
const NEWLINE: &str = "\r\n";
#[cfg(not(target_os = "windows"))]
const NEWLINE: &str = "\n";

pub struct Formatter {
    format_opt: Format,
    string: String,
    indent: usize,
    line: u32,
    aligner: Aligner,
    in_start_token: bool,
    consumed_next_newline: bool,
    single_line: bool,
    adjust_line: bool,
    case_item_indent: Vec<usize>,
}

impl Default for Formatter {
    fn default() -> Self {
        Self {
            format_opt: Format::default(),
            string: String::new(),
            indent: 0,
            line: 1,
            aligner: Aligner::new(),
            in_start_token: false,
            consumed_next_newline: false,
            single_line: false,
            adjust_line: false,
            case_item_indent: Vec::new(),
        }
    }
}

impl Formatter {
    pub fn new(metadata: &Metadata) -> Self {
        Self {
            format_opt: metadata.format.clone(),
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

    fn column(&self) -> usize {
        self.string.len() - self.string.rfind('\n').unwrap_or(0)
    }

    fn str(&mut self, x: &str) {
        self.string.push_str(x);
    }

    fn unindent(&mut self) {
        if self
            .string
            .ends_with(&" ".repeat(self.indent * self.format_opt.indent_width))
        {
            self.string
                .truncate(self.string.len() - self.indent * self.format_opt.indent_width);
        }
    }

    fn indent(&mut self) {
        self.str(&" ".repeat(
            self.indent * self.format_opt.indent_width + self.case_item_indent.last().unwrap_or(&0),
        ));
    }

    fn newline_push(&mut self) {
        self.unindent();
        if !self.consumed_next_newline {
            self.str(NEWLINE);
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
            self.str(NEWLINE);
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
            self.str(NEWLINE);
        } else {
            self.consumed_next_newline = false;
        }
        self.indent();
        self.adjust_line = true;
    }

    fn newline_list(&mut self, i: usize) {
        if i == 0 {
            self.newline_push();
        } else {
            self.newline();
        }
    }

    fn newline_list_post(&mut self, is_empty: bool) {
        if !is_empty {
            self.newline_pop();
        }
    }

    fn space(&mut self, repeat: usize) {
        self.str(&" ".repeat(repeat));
    }

    fn consume_adjust_line(&mut self, x: &Token) {
        if self.adjust_line && x.line > self.line + 1 {
            self.newline();
        }
        self.adjust_line = false;
    }

    fn push_token(&mut self, x: &Token) {
        self.consume_adjust_line(x);
        let text = resource_table::get_str_value(x.text).unwrap();
        let text = if text.ends_with('\n') {
            self.consumed_next_newline = true;
            text.trim_end()
        } else {
            &text
        };
        let newlines_in_text = text.matches('\n').count() as u32;
        self.str(text);
        self.line = x.line + newlines_in_text;
    }

    fn process_token(&mut self, x: &VerylToken, will_push: bool) {
        self.push_token(&x.token);

        let loc: Location = x.token.into();
        if let Some(width) = self.aligner.additions.get(&loc) {
            self.space(*width as usize);
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
            for _ in 0..x.line - self.line {
                self.unindent();
                self.str(NEWLINE);
                self.indent();
            }
            self.push_token(x);
        }
        if will_push {
            self.indent -= 1;
        }
        if self.consumed_next_newline {
            self.unindent();
            self.str(NEWLINE);
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
        self.expression12(&arg.expression12);
        for x in &arg.expression11_list {
            self.space(1);
            self.r#as(&x.r#as);
            self.space(1);
            self.scoped_identifier(&x.scoped_identifier);
        }
    }

    /// Semantic action for non-terminal 'ArgumentList'
    fn argument_list(&mut self, arg: &ArgumentList) {
        self.argument_item(&arg.argument_item);
        for x in &arg.argument_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.argument_item(&x.argument_item);
        }
        if let Some(ref x) = arg.argument_list_opt {
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
        self.expression(&arg.expression);
        if let Some(ref x) = arg.concatenation_item_opt {
            self.space(1);
            self.repeat(&x.repeat);
            self.space(1);
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'ArrayLiteralList'
    fn array_literal_list(&mut self, arg: &ArrayLiteralList) {
        self.array_literal_item(&arg.array_literal_item);
        for x in &arg.array_literal_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.array_literal_item(&x.array_literal_item);
        }
        if let Some(ref x) = arg.array_literal_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'ArrayLiteralItem'
    fn array_literal_item(&mut self, arg: &ArrayLiteralItem) {
        match &*arg.array_literal_item_group {
            ArrayLiteralItemGroup::ExpressionArrayLiteralItemOpt(x) => {
                self.expression(&x.expression);
                if let Some(ref x) = x.array_literal_item_opt {
                    self.space(1);
                    self.repeat(&x.repeat);
                    self.space(1);
                    self.expression(&x.expression);
                }
            }
            ArrayLiteralItemGroup::DefaulColonExpression(x) => {
                self.defaul(&x.defaul);
                self.colon(&x.colon);
                self.space(1);
                self.expression(&x.expression);
            }
        }
    }

    /// Semantic action for non-terminal 'IfExpression'
    fn if_expression(&mut self, arg: &IfExpression) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.expression(&arg.expression0);
        self.newline_pop();
        self.r_brace(&arg.r_brace);
        for x in &arg.if_expression_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            self.newline_push();
            self.expression(&x.expression0);
            self.newline_pop();
            self.r_brace(&x.r_brace);
        }
        self.space(1);
        self.r#else(&arg.r#else);
        self.space(1);
        self.token_will_push(&arg.l_brace0.l_brace_token);
        self.newline_push();
        self.expression(&arg.expression1);
        self.newline_pop();
        self.r_brace(&arg.r_brace0);
    }

    /// Semantic action for non-terminal 'CaseExpression'
    fn case_expression(&mut self, arg: &CaseExpression) {
        self.case(&arg.case);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.expression(&arg.expression0);
        for x in &arg.case_expression_list {
            self.comma(&x.comma);
            self.space(1);
            self.expression(&x.expression);
        }
        self.colon(&arg.colon);
        self.space(1);
        self.expression(&arg.expression1);
        self.comma(&arg.comma);
        self.newline();
        for x in &arg.case_expression_list0 {
            self.expression(&x.expression);
            for x in &x.case_expression_list0_list {
                self.comma(&x.comma);
                self.space(1);
                self.expression(&x.expression);
            }
            self.colon(&x.colon);
            self.space(1);
            self.expression(&x.expression0);
            self.comma(&x.comma);
            self.newline();
        }
        self.defaul(&arg.defaul);
        self.colon(&arg.colon0);
        self.space(1);
        self.expression(&arg.expression2);
        if let Some(ref x) = arg.case_expression_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'TypeExpression'
    fn type_expression(&mut self, arg: &TypeExpression) {
        match arg {
            TypeExpression::ScalarType(x) => self.scalar_type(&x.scalar_type),
            TypeExpression::TypeLParenExpressionRParen(x) => {
                self.r#type(&x.r#type);
                self.l_paren(&x.l_paren);
                self.expression(&x.expression);
                self.r_paren(&x.r_paren);
            }
        }
    }

    /// Semantic action for non-terminal 'InsideExpression'
    fn inside_expression(&mut self, arg: &InsideExpression) {
        self.inside(&arg.inside);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.l_brace(&arg.l_brace);
        self.range_list(&arg.range_list);
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'OutsideExpression'
    fn outside_expression(&mut self, arg: &OutsideExpression) {
        self.outside(&arg.outside);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.l_brace(&arg.l_brace);
        self.range_list(&arg.range_list);
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'RangeList'
    fn range_list(&mut self, arg: &RangeList) {
        self.range_item(&arg.range_item);
        for x in &arg.range_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.range_item(&x.range_item);
        }
        if let Some(ref x) = arg.range_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'SelectOperator'
    fn select_operator(&mut self, arg: &SelectOperator) {
        match arg {
            SelectOperator::Colon(x) => self.colon(&x.colon),
            SelectOperator::PlusColon(x) => self.plus_colon(&x.plus_colon),
            SelectOperator::MinusColon(x) => self.minus_colon(&x.minus_colon),
            SelectOperator::Step(x) => {
                self.space(1);
                self.step(&x.step);
                self.space(1);
            }
        }
    }

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        self.l_angle(&arg.l_angle);
        self.expression(&arg.expression);
        for x in &arg.width_list {
            self.comma(&x.comma);
            self.space(1);
            self.expression(&x.expression);
        }
        self.r_angle(&arg.r_angle);
    }

    /// Semantic action for non-terminal 'Array'
    fn array(&mut self, arg: &Array) {
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        for x in &arg.array_list {
            self.comma(&x.comma);
            self.space(1);
            self.expression(&x.expression);
        }
        self.r_bracket(&arg.r_bracket);
    }

    /// Semantic action for non-terminal 'ScalarType'
    fn scalar_type(&mut self, arg: &ScalarType) {
        for x in &arg.scalar_type_list {
            self.type_modifier(&x.type_modifier);
            self.space(1);
        }
        match &*arg.scalar_type_group {
            ScalarTypeGroup::VariableType(x) => self.variable_type(&x.variable_type),
            ScalarTypeGroup::FixedType(x) => self.fixed_type(&x.fixed_type),
        };
    }

    /// Semantic action for non-terminal 'ArrayType'
    fn array_type(&mut self, arg: &ArrayType) {
        self.scalar_type(&arg.scalar_type);
        if let Some(ref x) = arg.array_type_opt {
            self.space(1);
            self.array(&x.array);
        }
    }

    /// Semantic action for non-terminal 'LetStatement'
    fn let_statement(&mut self, arg: &LetStatement) {
        self.r#let(&arg.r#let);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.array_type(&arg.array_type);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'Assignment'
    fn assignment(&mut self, arg: &Assignment) {
        self.space(1);
        match &*arg.assignment_group {
            AssignmentGroup::Equ(x) => self.equ(&x.equ),
            AssignmentGroup::AssignmentOperator(x) => {
                self.assignment_operator(&x.assignment_operator)
            }
        }
        self.space(1);
        self.expression(&arg.expression);
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.if_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.if_statement_list.is_empty());
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
            for (i, x) in x.if_statement_list0_list.iter().enumerate() {
                self.newline_list(i);
                self.statement(&x.statement);
            }
            self.newline_list_post(x.if_statement_list0_list.is_empty());
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            for (i, x) in x.if_statement_opt_list.iter().enumerate() {
                self.newline_list(i);
                self.statement(&x.statement);
            }
            self.newline_list_post(x.if_statement_opt_list.is_empty());
            self.r_brace(&x.r_brace);
        }
    }

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        self.if_reset(&arg.if_reset);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.if_reset_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.if_reset_statement_list.is_empty());
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
            for (i, x) in x.if_reset_statement_list0_list.iter().enumerate() {
                self.newline_list(i);
                self.statement(&x.statement);
            }
            self.newline_list_post(x.if_reset_statement_list0_list.is_empty());
            self.r_brace(&x.r_brace);
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token);
            for (i, x) in x.if_reset_statement_opt_list.iter().enumerate() {
                self.newline_list(i);
                self.statement(&x.statement);
            }
            self.newline_list_post(x.if_reset_statement_opt_list.is_empty());
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
        self.scalar_type(&arg.scalar_type);
        self.space(1);
        self.r#in(&arg.r#in);
        self.space(1);
        self.range(&arg.range);
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
        for (i, x) in arg.for_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.for_statement_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'CaseStatement'
    fn case_statement(&mut self, arg: &CaseStatement) {
        self.case(&arg.case);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.case_item(&x.case_item);
        }
        self.newline_list_post(arg.case_statement_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'CaseItem'
    fn case_item(&mut self, arg: &CaseItem) {
        let start = self.column();
        match &*arg.case_item_group {
            CaseItemGroup::ExpressionCaseItemGroupList(x) => {
                self.expression(&x.expression);
                for x in &x.case_item_group_list {
                    self.comma(&x.comma);
                    self.space(1);
                    self.expression(&x.expression);
                }
            }
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.colon(&arg.colon);
        self.space(1);
        self.case_item_indent.push(self.column() - start);
        match &*arg.case_item_group0 {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::LBraceCaseItemGroup0ListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                for (i, x) in x.case_item_group0_list.iter().enumerate() {
                    self.newline_list(i);
                    self.statement(&x.statement);
                }
                self.newline_list_post(x.case_item_group0_list.is_empty());
                self.r_brace(&x.r_brace);
            }
        }
        self.case_item_indent.pop();
    }

    /// Semantic action for non-terminal 'AttributeList'
    fn attribute_list(&mut self, arg: &AttributeList) {
        self.attribute_item(&arg.attribute_item);
        for x in &arg.attribute_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.attribute_item(&x.attribute_item);
        }
        if let Some(ref x) = arg.attribute_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'LetDeclaration'
    fn let_declaration(&mut self, arg: &LetDeclaration) {
        self.r#let(&arg.r#let);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.array_type(&arg.array_type);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'VarDeclaration'
    fn var_declaration(&mut self, arg: &VarDeclaration) {
        self.var(&arg.var);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.array_type(&arg.array_type);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'LocalDeclaration'
    fn local_declaration(&mut self, arg: &LocalDeclaration) {
        self.local(&arg.local);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.local_declaration_group {
            LocalDeclarationGroup::ArrayTypeEquExpression(x) => {
                self.array_type(&x.array_type);
                self.space(1);
                self.equ(&x.equ);
                self.space(1);
                self.expression(&x.expression);
            }
            LocalDeclarationGroup::TypeEquTypeExpression(x) => {
                self.r#type(&x.r#type);
                self.space(1);
                self.equ(&x.equ);
                self.space(1);
                self.type_expression(&x.type_expression);
            }
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'TypeDefDeclaration'
    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) {
        self.r#type(&arg.r#type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.array_type(&arg.array_type);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
        self.always_ff(&arg.always_ff);
        self.space(1);
        if let Some(ref x) = arg.always_ff_declaration_opt {
            self.alwayf_ff_event_list(&x.alwayf_ff_event_list);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.always_ff_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.always_ff_declaration_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'AlwayfFfEventList'
    fn alwayf_ff_event_list(&mut self, arg: &AlwayfFfEventList) {
        self.l_paren(&arg.l_paren);
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.alwayf_ff_event_list_opt {
            self.comma(&x.comma);
            self.space(1);
            self.always_ff_reset(&x.always_ff_reset);
        }
        self.r_paren(&arg.r_paren);
        self.space(1);
    }

    /// Semantic action for non-terminal 'AlwaysFfClock'
    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) {
        self.hierarchical_identifier(&arg.hierarchical_identifier);
    }

    /// Semantic action for non-terminal 'AlwaysFfReset'
    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) {
        self.hierarchical_identifier(&arg.hierarchical_identifier);
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        self.always_comb(&arg.always_comb);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.always_comb_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.always_comb_declaration_list.is_empty());
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
        self.modport_group(&arg.modport_group);
        for x in &arg.modport_list_list {
            self.comma(&x.comma);
            self.newline();
            self.modport_group(&x.modport_group);
        }
        if let Some(ref x) = arg.modport_list_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'ModportGroup'
    fn modport_group(&mut self, arg: &ModportGroup) {
        for x in &arg.modport_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.modport_group_group {
            ModportGroupGroup::LBraceModportListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                self.newline_push();
                self.modport_list(&x.modport_list);
                self.newline_pop();
                self.r_brace(&x.r_brace);
            }
            ModportGroupGroup::ModportItem(x) => self.modport_item(&x.modport_item),
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
        self.scalar_type(&arg.scalar_type);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.enum_list(&arg.enum_list);
        self.newline_pop();
        self.r_brace(&arg.r_brace);
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
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'EnumGroup'
    fn enum_group(&mut self, arg: &EnumGroup) {
        for x in &arg.enum_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.enum_group_group {
            EnumGroupGroup::LBraceEnumListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                self.newline_push();
                self.enum_list(&x.enum_list);
                self.newline_pop();
                self.r_brace(&x.r_brace);
            }
            EnumGroupGroup::EnumItem(x) => self.enum_item(&x.enum_item),
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

    /// Semantic action for non-terminal 'StructUnionDeclaration'
    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) {
        self.struct_union(&arg.struct_union);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.struct_union_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.struct_union_list(&arg.struct_union_list);
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'StructUnionList'
    fn struct_union_list(&mut self, arg: &StructUnionList) {
        self.struct_union_group(&arg.struct_union_group);
        for x in &arg.struct_union_list_list {
            self.comma(&x.comma);
            self.newline();
            self.struct_union_group(&x.struct_union_group);
        }
        if let Some(ref x) = arg.struct_union_list_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'StructUnionGroup'
    fn struct_union_group(&mut self, arg: &StructUnionGroup) {
        for x in &arg.struct_union_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.struct_union_group_group {
            StructUnionGroupGroup::LBraceStructUnionListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                self.newline_push();
                self.struct_union_list(&x.struct_union_list);
                self.newline_pop();
                self.r_brace(&x.r_brace);
            }
            StructUnionGroupGroup::StructUnionItem(x) => {
                self.struct_union_item(&x.struct_union_item)
            }
        }
    }

    /// Semantic action for non-terminal 'StructUnionItem'
    fn struct_union_item(&mut self, arg: &StructUnionItem) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.scalar_type(&arg.scalar_type);
    }

    /// Semantic action for non-terminal 'InitialDeclaration'
    fn initial_declaration(&mut self, arg: &InitialDeclaration) {
        self.initial(&arg.initial);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.initial_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.initial_declaration_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'FinalDeclaration'
    fn final_declaration(&mut self, arg: &FinalDeclaration) {
        self.r#final(&arg.r#final);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.final_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.final_declaration_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'InstDeclaration'
    fn inst_declaration(&mut self, arg: &InstDeclaration) {
        if arg.inst_declaration_opt1.is_none() {
            self.single_line = true;
        }
        self.inst(&arg.inst);
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        if let Some(ref x) = arg.inst_declaration_opt {
            self.space(1);
            self.array(&x.array);
        }
        if let Some(ref x) = arg.inst_declaration_opt0 {
            self.space(1);
            self.inst_parameter(&x.inst_parameter);
        }
        if let Some(ref x) = arg.inst_declaration_opt1 {
            self.space(1);
            self.token_will_push(&x.l_paren.l_paren_token);
            self.newline_push();
            if let Some(ref x) = x.inst_declaration_opt2 {
                self.inst_port_list(&x.inst_port_list);
            }
            self.newline_pop();
            self.r_paren(&x.r_paren);
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
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'InstParameterGroup'
    fn inst_parameter_group(&mut self, arg: &InstParameterGroup) {
        for x in &arg.inst_parameter_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.inst_parameter_group_group {
            InstParameterGroupGroup::LBraceInstParameterListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                self.newline_push();
                self.inst_parameter_list(&x.inst_parameter_list);
                self.newline_pop();
                self.r_brace(&x.r_brace);
            }
            InstParameterGroupGroup::InstParameterItem(x) => {
                self.inst_parameter_item(&x.inst_parameter_item)
            }
        }
    }

    /// Semantic action for non-terminal 'InstParameterItem'
    fn inst_parameter_item(&mut self, arg: &InstParameterItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.inst_parameter_item_opt {
            self.colon(&x.colon);
            self.space(1);
            self.expression(&x.expression);
        }
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
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'InstPortGroup'
    fn inst_port_group(&mut self, arg: &InstPortGroup) {
        for x in &arg.inst_port_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.inst_port_group_group {
            InstPortGroupGroup::LBraceInstPortListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                self.newline_push();
                self.inst_port_list(&x.inst_port_list);
                self.newline_pop();
                self.r_brace(&x.r_brace);
            }
            InstPortGroupGroup::InstPortItem(x) => self.inst_port_item(&x.inst_port_item),
        }
    }

    /// Semantic action for non-terminal 'InstPortItem'
    fn inst_port_item(&mut self, arg: &InstPortItem) {
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.inst_port_item_opt {
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
        self.with_parameter_group(&arg.with_parameter_group);
        for x in &arg.with_parameter_list_list {
            self.comma(&x.comma);
            self.newline();
            self.with_parameter_group(&x.with_parameter_group);
        }
        if let Some(ref x) = arg.with_parameter_list_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'WithParameterGroup'
    fn with_parameter_group(&mut self, arg: &WithParameterGroup) {
        for x in &arg.with_parameter_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.with_parameter_group_group {
            WithParameterGroupGroup::LBraceWithParameterListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                self.newline_push();
                self.with_parameter_list(&x.with_parameter_list);
                self.newline_pop();
                self.r_brace(&x.r_brace);
            }
            WithParameterGroupGroup::WithParameterItem(x) => {
                self.with_parameter_item(&x.with_parameter_item)
            }
        }
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::Param(x) => self.param(&x.param),
            WithParameterItemGroup::Local(x) => self.local(&x.local),
        };
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.with_parameter_item_group0 {
            WithParameterItemGroup0::ArrayTypeEquExpression(x) => {
                self.array_type(&x.array_type);
                self.space(1);
                self.equ(&x.equ);
                self.space(1);
                self.expression(&x.expression);
            }
            WithParameterItemGroup0::TypeEquTypeExpression(x) => {
                self.r#type(&x.r#type);
                self.space(1);
                self.equ(&x.equ);
                self.space(1);
                self.type_expression(&x.type_expression);
            }
        }
    }

    /// Semantic action for non-terminal 'WithGenericParameterList'
    fn with_generic_parameter_list(&mut self, arg: &WithGenericParameterList) {
        self.with_generic_parameter_item(&arg.with_generic_parameter_item);
        for x in &arg.with_generic_parameter_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.with_generic_parameter_item(&x.with_generic_parameter_item);
        }
        if let Some(ref x) = arg.with_generic_parameter_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'WithGenericArgumentList'
    fn with_generic_argument_list(&mut self, arg: &WithGenericArgumentList) {
        self.with_generic_argument_item(&arg.with_generic_argument_item);
        for x in &arg.with_generic_argument_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.with_generic_argument_item(&x.with_generic_argument_item);
        }
        if let Some(ref x) = arg.with_generic_argument_list_opt {
            self.comma(&x.comma);
        }
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
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationGroup'
    fn port_declaration_group(&mut self, arg: &PortDeclarationGroup) {
        for x in &arg.port_declaration_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.port_declaration_group_group {
            PortDeclarationGroupGroup::LBracePortDeclarationListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                self.newline_push();
                self.port_declaration_list(&x.port_declaration_list);
                self.newline_pop();
                self.r_brace(&x.r_brace);
            }
            PortDeclarationGroupGroup::PortDeclarationItem(x) => {
                self.port_declaration_item(&x.port_declaration_item)
            }
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.port_declaration_item_group {
            PortDeclarationItemGroup::DirectionArrayType(x) => {
                self.direction(&x.direction);
                self.space(1);
                self.array_type(&x.array_type);
            }
            PortDeclarationItemGroup::InterfacePortDeclarationItemOpt(x) => {
                self.interface(&x.interface);
                if let Some(ref x) = x.port_declaration_item_opt {
                    self.space(1);
                    self.array(&x.array);
                }
            }
        }
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        self.function(&arg.function);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.function_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        self.space(1);
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
            self.space(1);
        }
        if let Some(ref x) = arg.function_declaration_opt1 {
            self.minus_g_t(&x.minus_g_t);
            self.space(1);
            self.scalar_type(&x.scalar_type);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.function_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.function_item(&x.function_item);
        }
        self.newline_list_post(arg.function_declaration_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ImportDeclaration'
    fn import_declaration(&mut self, arg: &ImportDeclaration) {
        self.import(&arg.import);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        if let Some(ref x) = arg.import_declaration_opt {
            self.colon_colon(&x.colon_colon);
            self.star(&x.star);
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ExportDeclaration'
    fn export_declaration(&mut self, arg: &ExportDeclaration) {
        self.export(&arg.export);
        self.space(1);
        match &*arg.export_declaration_group {
            ExportDeclarationGroup::Star(x) => self.star(&x.star),
            ExportDeclarationGroup::ScopedIdentifierExportDeclarationOpt(x) => {
                self.scoped_identifier(&x.scoped_identifier);
                if let Some(ref x) = x.export_declaration_opt {
                    self.colon_colon(&x.colon_colon);
                    self.star(&x.star);
                }
            }
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        if let Some(ref x) = arg.module_declaration_opt {
            self.r#pub(&x.r#pub);
            self.space(1);
        }
        self.module(&arg.module);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.module_declaration_opt0 {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        self.space(1);
        if let Some(ref x) = arg.module_declaration_opt1 {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        if let Some(ref x) = arg.module_declaration_opt2 {
            self.port_declaration(&x.port_declaration);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.module_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.module_group(&x.module_group);
        }
        self.newline_list_post(arg.module_declaration_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ModuleIfDeclaration'
    fn module_if_declaration(&mut self, arg: &ModuleIfDeclaration) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.module_named_block(&arg.module_named_block);
        for x in &arg.module_if_declaration_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.module_optional_named_block(&x.module_optional_named_block);
        }
        if let Some(ref x) = arg.module_if_declaration_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.module_optional_named_block(&x.module_optional_named_block);
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
        self.range(&arg.range);
        self.space(1);
        if let Some(ref x) = arg.module_for_declaration_opt {
            self.step(&x.step);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
        }
        self.module_named_block(&arg.module_named_block);
    }

    /// Semantic action for non-terminal 'ModuleNamedBlock'
    fn module_named_block(&mut self, arg: &ModuleNamedBlock) {
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.module_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.module_group(&x.module_group);
        }
        self.newline_list_post(arg.module_named_block_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ModuleOptionalNamedBlock'
    fn module_optional_named_block(&mut self, arg: &ModuleOptionalNamedBlock) {
        if let Some(ref x) = arg.module_optional_named_block_opt {
            self.colon(&x.colon);
            self.identifier(&x.identifier);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.module_optional_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.module_group(&x.module_group);
        }
        self.newline_list_post(arg.module_optional_named_block_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ModuleGroup'
    fn module_group(&mut self, arg: &ModuleGroup) {
        for x in &arg.module_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.module_group_group {
            ModuleGroupGroup::LBraceModuleGroupGroupListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                for (i, x) in x.module_group_group_list.iter().enumerate() {
                    self.newline_list(i);
                    self.module_group(&x.module_group);
                }
                self.newline_list_post(x.module_group_group_list.is_empty());
                self.r_brace(&x.r_brace);
            }
            ModuleGroupGroup::ModuleItem(x) => self.module_item(&x.module_item),
        }
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        if let Some(ref x) = arg.interface_declaration_opt {
            self.r#pub(&x.r#pub);
            self.space(1);
        }
        self.interface(&arg.interface);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.interface_declaration_opt0 {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        self.space(1);
        if let Some(ref x) = arg.interface_declaration_opt1 {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.interface_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.interface_group(&x.interface_group);
        }
        self.newline_list_post(arg.interface_declaration_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'InterfaceIfDeclaration'
    fn interface_if_declaration(&mut self, arg: &InterfaceIfDeclaration) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.interface_named_block(&arg.interface_named_block);
        for x in &arg.interface_if_declaration_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.interface_optional_named_block(&x.interface_optional_named_block);
        }
        if let Some(ref x) = arg.interface_if_declaration_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.interface_optional_named_block(&x.interface_optional_named_block);
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
        self.range(&arg.range);
        self.space(1);
        if let Some(ref x) = arg.interface_for_declaration_opt {
            self.step(&x.step);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
        }
        self.interface_named_block(&arg.interface_named_block);
    }

    /// Semantic action for non-terminal 'InterfaceNamedBlock'
    fn interface_named_block(&mut self, arg: &InterfaceNamedBlock) {
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        for (i, x) in arg.interface_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.interface_group(&x.interface_group);
        }
        self.newline_list_post(arg.interface_named_block_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'InterfaceOptionalNamedBlock'
    fn interface_optional_named_block(&mut self, arg: &InterfaceOptionalNamedBlock) {
        if let Some(ref x) = arg.interface_optional_named_block_opt {
            self.colon(&x.colon);
            self.identifier(&x.identifier);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.interface_optional_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.interface_group(&x.interface_group);
        }
        self.newline_list_post(arg.interface_optional_named_block_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'InterfaceGroup'
    fn interface_group(&mut self, arg: &InterfaceGroup) {
        for x in &arg.interface_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.interface_group_group {
            InterfaceGroupGroup::LBraceInterfaceGroupGroupListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                for (i, x) in x.interface_group_group_list.iter().enumerate() {
                    self.newline_list(i);
                    self.interface_group(&x.interface_group);
                }
                self.newline_list_post(x.interface_group_group_list.is_empty());
                self.r_brace(&x.r_brace);
            }
            InterfaceGroupGroup::InterfaceItem(x) => self.interface_item(&x.interface_item),
        }
    }

    /// Semantic action for non-terminal 'PackageDeclaration'
    fn package_declaration(&mut self, arg: &PackageDeclaration) {
        if let Some(ref x) = arg.package_declaration_opt {
            self.r#pub(&x.r#pub);
            self.space(1);
        }
        self.package(&arg.package);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.package_declaration_opt0 {
            self.with_generic_parameter(&x.with_generic_parameter);
        }
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.package_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.package_group(&x.package_group);
        }
        self.newline_list_post(arg.package_declaration_list.is_empty());
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'PackageGroup'
    fn package_group(&mut self, arg: &PackageGroup) {
        for x in &arg.package_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.package_group_group {
            PackageGroupGroup::LBracePackageGroupGroupListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                for (i, x) in x.package_group_group_list.iter().enumerate() {
                    self.newline_list(i);
                    self.package_group(&x.package_group);
                }
                self.newline_list_post(x.package_group_group_list.is_empty());
                self.r_brace(&x.r_brace);
            }
            PackageGroupGroup::PackageItem(x) => self.package_item(&x.package_item),
        }
    }

    /// Semantic action for non-terminal 'EmbedDeclaration'
    fn embed_declaration(&mut self, arg: &EmbedDeclaration) {
        self.embed(&arg.embed);
        self.space(1);
        self.l_paren(&arg.l_paren);
        self.identifier(&arg.identifier);
        self.r_paren(&arg.r_paren);
        self.space(1);
        self.identifier(&arg.identifier0);

        self.embed_content(&arg.embed_content);
    }

    /// Semantic action for non-terminal 'IncludeDeclaration'
    fn include_declaration(&mut self, arg: &IncludeDeclaration) {
        self.include(&arg.include);
        self.space(1);
        self.l_paren(&arg.l_paren);
        self.identifier(&arg.identifier);
        self.comma(&arg.comma);
        self.space(1);
        self.string_literal(&arg.string_literal);
        self.r_paren(&arg.r_paren);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'DescriptionGroup'
    fn description_group(&mut self, arg: &DescriptionGroup) {
        for x in &arg.description_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.description_group_group {
            DescriptionGroupGroup::LBraceDescriptionGroupGroupListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                for (i, x) in x.description_group_group_list.iter().enumerate() {
                    self.newline_list(i);
                    self.description_group(&x.description_group);
                }
                self.newline_list_post(x.description_group_group_list.is_empty());
                self.r_brace(&x.r_brace);
            }
            DescriptionGroupGroup::DescriptionItem(x) => self.description_item(&x.description_item),
        }
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
            self.description_group(&x.description_group);
        }
        self.newline();
    }
}
