use veryl_aligner::{Aligner, Location, Measure, align_kind};
use veryl_analyzer::attribute::{AlignItem, FormatItem};
use veryl_analyzer::attribute_table;
use veryl_metadata::{Format, Metadata};
use veryl_parser::resource_table;
use veryl_parser::token_range::TokenExt;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, VerylToken};
use veryl_parser::veryl_walker::VerylWalker;

#[cfg(target_os = "windows")]
const NEWLINE: &str = "\r\n";
#[cfg(not(target_os = "windows"))]
const NEWLINE: &str = "\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Emit,
    Align,
}

pub struct Formatter {
    mode: Mode,
    format_opt: Format,
    string: String,
    indent: usize,
    line: u32,
    aligner: Aligner,
    measure: Measure,
    in_start_token: bool,
    consumed_next_newline: bool,
    single_line: Vec<()>,
    multi_line: Vec<()>,
    adjust_line: bool,
    keep_tail_newline: bool,
    in_scalar_type: bool,
    in_expression: Vec<()>,
    in_attribute: bool,
    in_named_argument: Vec<bool>,
}

impl Default for Formatter {
    fn default() -> Self {
        Self {
            mode: Mode::Emit,
            format_opt: Format::default(),
            string: String::new(),
            indent: 0,
            line: 1,
            aligner: Aligner::new(),
            measure: Measure::default(),
            in_start_token: false,
            consumed_next_newline: false,
            single_line: Vec::new(),
            multi_line: Vec::new(),
            adjust_line: false,
            keep_tail_newline: false,
            in_scalar_type: false,
            in_expression: Vec::new(),
            in_attribute: false,
            in_named_argument: Vec::new(),
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
        self.mode = Mode::Align;
        self.veryl(input);
        self.aligner.finish_group();
        self.aligner.gather_additions();
        self.mode = Mode::Emit;
        self.veryl(input);
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    fn str(&mut self, x: &str) {
        match self.mode {
            Mode::Emit => {
                self.string.push_str(x);
            }
            Mode::Align => {
                self.aligner.space(x.len());
                self.measure.add(x.len() as u32);
            }
        }
    }

    fn unindent(&mut self) {
        if self.mode == Mode::Align {
            return;
        }

        let indent_width = self.indent * self.format_opt.indent_width;
        if self.string.ends_with(&" ".repeat(indent_width)) {
            self.string.truncate(self.string.len() - indent_width);
        }
    }

    fn indent(&mut self) {
        if self.mode == Mode::Align {
            return;
        }

        let indent_width = self.indent * self.format_opt.indent_width;
        self.str(&" ".repeat(indent_width));
    }

    fn newline_push(&mut self) {
        if self.single_line() {
            self.space(1);
        } else {
            if self.mode == Mode::Align {
                return;
            }

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
    }

    fn newline_pop(&mut self) {
        if self.single_line() {
            self.space(1);
        } else {
            if self.mode == Mode::Align {
                return;
            }

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
    }

    fn newline_tail_pop(&mut self) {
        if !self.single_line() {
            self.newline_pop();
        }
    }

    fn newline(&mut self) {
        if self.single_line() {
            self.space(1);
        } else {
            if self.mode == Mode::Align {
                return;
            }

            self.unindent();
            if !self.consumed_next_newline {
                self.str(NEWLINE);
            } else {
                self.consumed_next_newline = false;
            }
            self.indent();
            self.adjust_line = true;
        }
    }

    fn newline_list(&mut self, i: usize) {
        if i == 0 {
            self.newline_push();
        } else {
            self.newline();
        }
    }

    fn newline_list_post(&mut self, is_empty: bool, start_token: &VerylToken) {
        if !is_empty {
            self.newline_pop();
        } else if let Some(last_commant) = start_token.comments.last() {
            if resource_table::get_str_value(last_commant.text)
                .map(|x| !x.ends_with("\n"))
                .unwrap()
            {
                self.newline();
            }
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
        let text = if !self.keep_tail_newline && text.ends_with('\n') {
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
        match self.mode {
            Mode::Emit => {
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
            Mode::Align => {
                self.aligner.token(x);
                self.measure.add(x.token.length);
            }
        }
    }

    fn token(&mut self, x: &VerylToken) {
        self.process_token(x, false)
    }

    fn token_will_push(&mut self, x: &VerylToken) {
        let will_push = !self.single_line();
        self.process_token(x, will_push)
    }

    fn align_start(&mut self, kind: usize) {
        if self.mode == Mode::Align {
            self.aligner.aligns[kind].start_item();
        }
    }

    fn align_any(&self) -> bool {
        self.aligner.any_enabled()
    }

    fn align_finish(&mut self, kind: usize) {
        if self.mode == Mode::Align {
            self.aligner.aligns[kind].finish_item();
        }
    }

    fn align_last_location(&mut self, kind: usize) -> Option<Location> {
        self.aligner.aligns[kind].last_location
    }

    fn align_dummy_location(&mut self, kind: usize, loc: Option<Location>) {
        if self.mode == Mode::Align {
            self.aligner.aligns[kind].dummy_location(loc.unwrap());
        }
    }

    fn align_dummy_token(&mut self, kind: usize, token: &VerylToken) {
        if self.mode == Mode::Align {
            self.aligner.aligns[kind].dummy_token(token);
        }
    }

    fn align_reset(&mut self) {
        if self.mode == Mode::Align {
            self.aligner.finish_item();
            self.aligner.finish_group();
        }
    }

    fn align_insert(&mut self, token: &VerylToken, width: usize) {
        if self.mode == Mode::Align {
            let loc: Location = token.token.into();
            self.aligner
                .additions
                .entry(loc)
                .and_modify(|val| *val += width as u32)
                .or_insert(width as u32);
        }
    }

    fn measure_start(&mut self) {
        if self.mode == Mode::Align {
            self.measure.start();
        }
    }

    fn measure_finish(&mut self, token: &Token) {
        if self.mode == Mode::Align {
            self.measure.finish(token.id);
        }
    }

    fn measure_get(&mut self, token: &Token) -> Option<u32> {
        self.measure.get(token.id)
    }

    fn single_line_start(&mut self) {
        self.single_line.push(());
    }

    fn single_line_finish(&mut self) {
        self.single_line.pop();
    }

    fn single_line(&self) -> bool {
        !self.single_line.is_empty()
    }

    fn multi_line_start(&mut self) {
        self.multi_line.push(());
    }

    fn multi_line_finish(&mut self) {
        self.multi_line.pop();
    }

    fn multi_line(&self) -> bool {
        !self.multi_line.is_empty()
    }
}

impl VerylWalker for Formatter {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.token(arg);
    }

    /// Semantic action for non-terminal 'Identifier'
    fn identifier(&mut self, arg: &Identifier) {
        let align = !self.align_any()
            && !self.in_attribute
            && attribute_table::is_align(&arg.first(), AlignItem::Identifier);
        if align {
            self.align_start(align_kind::IDENTIFIER);
        }
        self.veryl_token(&arg.identifier_token);
        if align {
            self.align_finish(align_kind::IDENTIFIER);
        }
    }

    /// Semantic action for non-terminal 'Number'
    fn number(&mut self, arg: &Number) {
        let align = !self.align_any() && attribute_table::is_align(&arg.first(), AlignItem::Number);
        if align {
            self.align_start(align_kind::NUMBER);
        }
        match arg {
            Number::IntegralNumber(x) => self.integral_number(&x.integral_number),
            Number::RealNumber(x) => self.real_number(&x.real_number),
        };
        if align {
            self.align_finish(align_kind::NUMBER);
        }
    }

    /// Semantic action for non-terminal 'Expression'
    fn expression(&mut self, arg: &Expression) {
        self.in_expression.push(());
        self.if_expression(&arg.if_expression);
        self.in_expression.pop();
    }

    /// Semantic action for non-terminal 'IfExpression'
    fn if_expression(&mut self, arg: &IfExpression) {
        if arg.if_expression_list.is_empty() {
            self.expression01(&arg.expression01);
        } else {
            self.measure_start();

            let compact = attribute_table::is_format(&arg.first(), FormatItem::Compact);
            let single_line = if self.mode == Mode::Emit {
                let width = self.measure_get(&arg.first()).unwrap();
                (width < self.format_opt.max_width as u32) || compact
            } else {
                // calc line width as single_line in Align mode
                true
            };
            if single_line {
                self.single_line_start();
            }

            for (i, x) in arg.if_expression_list.iter().enumerate() {
                self.r#if(&x.r#if);
                self.space(1);
                self.expression(&x.expression);

                self.space(1);
                self.token_will_push(&x.question.question_token);
                self.newline_push();
                self.expression(&x.expression0);
                self.newline_pop();

                if (i + 1) < arg.if_expression_list.len() {
                    self.colon(&x.colon);
                    self.space(1);
                } else {
                    self.token_will_push(&x.colon.colon_token);
                }
            }

            self.newline_push();
            self.expression01(&arg.expression01);
            self.newline_tail_pop();

            self.measure_finish(&arg.first());
            if single_line {
                self.single_line_finish();
            }
        }
    }

    /// Semantic action for non-terminal 'Expression01'
    // Add `#[inline(never)]` to `expression*` as a workaround for long time compilation
    // https://github.com/rust-lang/rust/issues/106211
    #[inline(never)]
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
    #[inline(never)]
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
    #[inline(never)]
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
    #[inline(never)]
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
    #[inline(never)]
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
    #[inline(never)]
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
    #[inline(never)]
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
    #[inline(never)]
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
    #[inline(never)]
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
    #[inline(never)]
    fn expression10(&mut self, arg: &Expression10) {
        self.expression11(&arg.expression11);
        for x in &arg.expression10_list {
            self.space(1);
            match &*x.expression10_list_group {
                Expression10ListGroup::Operator11(x) => self.operator11(&x.operator11),
                Expression10ListGroup::Star(x) => self.star(&x.star),
            }
            self.space(1);
            self.expression11(&x.expression11);
        }
    }

    /// Semantic action for non-terminal 'Expression11'
    #[inline(never)]
    fn expression11(&mut self, arg: &Expression11) {
        self.expression12(&arg.expression12);
        for x in &arg.expression11_list {
            self.space(1);
            self.operator12(&x.operator12);
            self.space(1);
            self.expression12(&x.expression12);
        }
    }

    /// Semantic action for non-terminal 'Expression12'
    #[inline(never)]
    fn expression12(&mut self, arg: &Expression12) {
        self.expression13(&arg.expression13);
        if let Some(x) = &arg.expression12_opt {
            self.space(1);
            self.r#as(&x.r#as);
            self.space(1);
            self.casting_type(&x.casting_type);
        }
    }

    /// Semantic action for non-terminal 'Factor'
    fn factor(&mut self, arg: &Factor) {
        match arg {
            Factor::Number(x) => self.number(&x.number),
            Factor::BooleanLiteral(x) => self.boolean_literal(&x.boolean_literal),
            Factor::IdentifierFactor(x) => self.identifier_factor(&x.identifier_factor),
            Factor::LParenExpressionRParen(x) => {
                self.l_paren(&x.l_paren);
                self.expression(&x.expression);
                self.r_paren(&x.r_paren);
            }
            Factor::LBraceConcatenationListRBrace(x) => {
                if x.l_brace.line() != x.r_brace.line() {
                    self.multi_line_start();
                }
                self.l_brace(&x.l_brace);
                self.concatenation_list(&x.concatenation_list);
                self.r_brace(&x.r_brace);
                if x.l_brace.line() != x.r_brace.line() {
                    self.multi_line_finish();
                }
            }
            Factor::QuoteLBraceArrayLiteralListRBrace(x) => {
                if x.quote_l_brace.line() != x.r_brace.line() {
                    self.multi_line_start();
                }
                self.quote_l_brace(&x.quote_l_brace);
                self.array_literal_list(&x.array_literal_list);
                self.r_brace(&x.r_brace);
                if x.quote_l_brace.line() != x.r_brace.line() {
                    self.multi_line_finish();
                }
            }
            Factor::CaseExpression(x) => {
                self.case_expression(&x.case_expression);
            }
            Factor::SwitchExpression(x) => {
                self.switch_expression(&x.switch_expression);
            }
            Factor::StringLiteral(x) => {
                self.string_literal(&x.string_literal);
            }
            Factor::FactorGroup(x) => match &*x.factor_group {
                FactorGroup::Msb(x) => self.msb(&x.msb),
                FactorGroup::Lsb(x) => self.lsb(&x.lsb),
            },
            Factor::InsideExpression(x) => {
                self.inside_expression(&x.inside_expression);
            }
            Factor::OutsideExpression(x) => {
                self.outside_expression(&x.outside_expression);
            }
            Factor::TypeExpression(x) => {
                self.type_expression(&x.type_expression);
            }
            Factor::FactorTypeFactor(x) => {
                self.factor_type_factor(&x.factor_type_factor);
            }
        }
    }

    /// Semantic action for non-terminal 'FactorTypeFactor'
    fn factor_type_factor(&mut self, arg: &FactorTypeFactor) {
        for x in &arg.factor_type_factor_list {
            self.type_modifier(&x.type_modifier);
            self.space(1);
        }
        self.factor_type(&arg.factor_type);
    }

    /// Semantic action for non-terminal 'FunctionCall'
    fn function_call(&mut self, arg: &FunctionCall) {
        let in_named_argument = if let Some(ref x) = arg.function_call_opt {
            let list: Vec<_> = x.argument_list.as_ref().into();
            list.iter().any(|x| x.argument_item_opt.is_some())
        } else {
            false
        };
        self.in_named_argument.push(in_named_argument);
        if in_named_argument {
            self.token_will_push(&arg.l_paren.l_paren_token);
            self.newline_push();
            self.align_reset();
        } else {
            self.l_paren(&arg.l_paren);
        }
        if let Some(ref x) = arg.function_call_opt {
            self.argument_list(&x.argument_list);
        }
        if in_named_argument {
            self.newline_pop();
        }
        self.r_paren(&arg.r_paren);
        self.in_named_argument.pop();
    }

    /// Semantic action for non-terminal 'ArgumentList'
    #[inline(never)]
    fn argument_list(&mut self, arg: &ArgumentList) {
        self.argument_item(&arg.argument_item);
        for x in &arg.argument_list_list {
            self.comma(&x.comma);
            if *self.in_named_argument.last().unwrap() {
                self.newline();
            } else {
                self.space(1);
            }
            self.argument_item(&x.argument_item);
        }
        if let Some(ref x) = arg.argument_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'ArgumentItem'
    fn argument_item(&mut self, arg: &ArgumentItem) {
        if let Some(ref x) = arg.argument_item_opt {
            self.align_start(align_kind::IDENTIFIER);
            self.argument_expression(&arg.argument_expression);
            self.align_finish(align_kind::IDENTIFIER);
            self.colon(&x.colon);
            self.space(1);
            self.align_start(align_kind::EXPRESSION);
            self.expression(&x.expression);
            self.align_finish(align_kind::EXPRESSION);
        } else {
            self.argument_expression(&arg.argument_expression);
        }
    }

    /// Semantic action for non-terminal 'StructConstructor'
    fn struct_constructor(&mut self, arg: &StructConstructor) {
        if arg.quote_l_brace.line() != arg.r_brace.line() {
            self.multi_line_start();
        }
        self.quote_l_brace(&arg.quote_l_brace);
        if self.multi_line() {
            self.newline_push();
        }
        self.struct_constructor_list(&arg.struct_constructor_list);
        if let Some(ref x) = arg.struct_constructor_opt {
            if self.multi_line() {
                self.newline();
            } else {
                self.space(1);
            }
            self.dot_dot(&x.dot_dot);
            self.defaul(&x.defaul);
            self.l_paren(&x.l_paren);
            self.expression(&x.expression);
            self.r_paren(&x.r_paren);
        }
        if self.multi_line() {
            self.newline_pop();
            self.align_reset();
        }
        self.r_brace(&arg.r_brace);
        if arg.quote_l_brace.line() != arg.r_brace.line() {
            self.multi_line_finish();
        }
    }

    /// Semantic action for non-terminal 'StructConstructorList'
    fn struct_constructor_list(&mut self, arg: &StructConstructorList) {
        self.struct_constructor_item(&arg.struct_constructor_item);
        for x in &arg.struct_constructor_list_list {
            self.comma(&x.comma);
            if x.comma.line() != x.struct_constructor_item.line() {
                self.newline();
            } else {
                self.space(1);
            }
            self.struct_constructor_item(&x.struct_constructor_item);
        }
        if let Some(ref x) = arg.struct_constructor_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'StructConstructorItem'
    fn struct_constructor_item(&mut self, arg: &StructConstructorItem) {
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        self.align_start(align_kind::EXPRESSION);
        self.expression(&arg.expression);
        self.align_finish(align_kind::EXPRESSION);
    }

    /// Semantic action for non-terminal 'ConcatenationList'
    fn concatenation_list(&mut self, arg: &ConcatenationList) {
        if self.multi_line() {
            self.newline_push();
        }
        self.concatenation_item(&arg.concatenation_item);
        for x in &arg.concatenation_list_list {
            self.comma(&x.comma);
            if x.comma.line() != x.concatenation_item.line() {
                self.newline();
            } else {
                self.space(1);
            }
            self.concatenation_item(&x.concatenation_item);
        }
        if let Some(ref x) = arg.concatenation_list_opt {
            self.comma(&x.comma);
        }
        if self.multi_line() {
            self.newline_pop();
            self.align_reset();
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
        if self.multi_line() {
            self.newline_push();
        }
        self.array_literal_item(&arg.array_literal_item);
        for x in &arg.array_literal_list_list {
            self.comma(&x.comma);
            if x.comma.line() != x.array_literal_item.line() {
                self.newline();
            } else {
                self.space(1);
            }
            self.array_literal_item(&x.array_literal_item);
        }
        if let Some(ref x) = arg.array_literal_list_opt {
            self.comma(&x.comma);
        }
        if self.multi_line() {
            self.newline_pop();
            self.align_reset();
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

    /// Semantic action for non-terminal 'CaseExpression'
    fn case_expression(&mut self, arg: &CaseExpression) {
        self.case(&arg.case);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.align_reset();
        self.align_start(align_kind::EXPRESSION);
        self.case_condition(&arg.case_condition);
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&arg.colon);
        self.space(1);
        self.expression(&arg.expression0);
        self.comma(&arg.comma);
        self.newline();
        for x in &arg.case_expression_list {
            self.align_start(align_kind::EXPRESSION);
            self.case_condition(&x.case_condition);
            self.align_finish(align_kind::EXPRESSION);
            self.colon(&x.colon);
            self.space(1);
            self.expression(&x.expression);
            self.comma(&x.comma);
            self.newline();
        }
        self.align_start(align_kind::EXPRESSION);
        self.defaul(&arg.defaul);
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&arg.colon0);
        self.space(1);
        self.expression(&arg.expression1);
        if let Some(ref x) = arg.case_expression_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'SwitchExpression'
    fn switch_expression(&mut self, arg: &SwitchExpression) {
        self.switch(&arg.switch);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.align_start(align_kind::EXPRESSION);
        self.switch_condition(&arg.switch_condition);
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&arg.colon);
        self.space(1);
        self.expression(&arg.expression);
        self.comma(&arg.comma);
        self.newline();
        for x in &arg.switch_expression_list {
            self.align_start(align_kind::EXPRESSION);
            self.switch_condition(&x.switch_condition);
            self.align_finish(align_kind::EXPRESSION);
            self.colon(&x.colon);
            self.space(1);
            self.expression(&x.expression);
            self.comma(&x.comma);
            self.newline();
        }
        self.align_start(align_kind::EXPRESSION);
        self.defaul(&arg.defaul);
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&arg.colon0);
        self.space(1);
        self.expression(&arg.expression0);
        if let Some(ref x) = arg.switch_expression_opt {
            self.comma(&x.comma);
        }
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'TypeExpression'
    fn type_expression(&mut self, arg: &TypeExpression) {
        self.r#type(&arg.r#type);
        self.l_paren(&arg.l_paren);
        self.expression(&arg.expression);
        self.r_paren(&arg.r_paren);
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

    /// Semantic action for non-terminal 'FactorType'
    fn factor_type(&mut self, arg: &FactorType) {
        match arg.factor_type_group.as_ref() {
            FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                self.variable_type(&x.variable_type);
                if self.in_scalar_type {
                    self.align_finish(align_kind::TYPE);
                    self.align_start(align_kind::WIDTH);
                }
                if let Some(ref x) = x.factor_type_opt {
                    self.width(&x.width);
                } else if self.in_scalar_type {
                    let loc = self.align_last_location(align_kind::TYPE);
                    self.align_dummy_location(align_kind::WIDTH, loc);
                }
            }
            FactorTypeGroup::FixedType(x) => {
                self.fixed_type(&x.fixed_type);
                if self.in_scalar_type {
                    self.align_finish(align_kind::TYPE);
                    self.align_start(align_kind::WIDTH);
                    let loc = self.align_last_location(align_kind::TYPE);
                    self.align_dummy_location(align_kind::WIDTH, loc);
                }
            }
        }
    }

    /// Semantic action for non-terminal 'ScalarType'
    fn scalar_type(&mut self, arg: &ScalarType) {
        self.in_scalar_type = true;

        // disable align in Expression
        if self.mode == Mode::Align && !self.in_expression.is_empty() {
            self.in_scalar_type = false;
            return;
        }

        self.align_start(align_kind::TYPE);
        for x in &arg.scalar_type_list {
            self.type_modifier(&x.type_modifier);
            self.space(1);
        }
        match &*arg.scalar_type_group {
            ScalarTypeGroup::UserDefinedTypeScalarTypeOpt(x) => {
                self.user_defined_type(&x.user_defined_type);
                self.align_finish(align_kind::TYPE);
                self.align_start(align_kind::WIDTH);
                if let Some(ref x) = x.scalar_type_opt {
                    self.width(&x.width);
                } else {
                    let loc = self.align_last_location(align_kind::TYPE);
                    self.align_dummy_location(align_kind::WIDTH, loc);
                }
            }
            ScalarTypeGroup::FactorType(x) => self.factor_type(&x.factor_type),
        }
        self.align_finish(align_kind::WIDTH);
        self.in_scalar_type = false;
    }

    /// Semantic action for non-terminal 'ArrayType'
    fn array_type(&mut self, arg: &ArrayType) {
        self.scalar_type(&arg.scalar_type);
        self.align_start(align_kind::ARRAY);
        if let Some(ref x) = arg.array_type_opt {
            self.space(1);
            self.array(&x.array);
        } else {
            let loc = self.align_last_location(align_kind::WIDTH);
            self.align_dummy_location(align_kind::ARRAY, loc);
        }
        self.align_finish(align_kind::ARRAY);
    }

    /// Semantic action for non-terminal 'LetStatement'
    fn let_statement(&mut self, arg: &LetStatement) {
        self.r#let(&arg.r#let);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        if let Some(ref x) = arg.let_statement_opt {
            self.align_start(align_kind::CLOCK_DOMAIN);
            self.clock_domain(&x.clock_domain);
            self.space(1);
            self.align_finish(align_kind::CLOCK_DOMAIN);
        } else {
            self.align_start(align_kind::CLOCK_DOMAIN);
            self.align_dummy_token(align_kind::CLOCK_DOMAIN, &arg.colon.colon_token);
            self.align_finish(align_kind::CLOCK_DOMAIN);
        }
        self.array_type(&arg.array_type);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'IdentifierStatement'
    fn identifier_statement(&mut self, arg: &IdentifierStatement) {
        self.align_start(align_kind::IDENTIFIER);
        self.expression_identifier(&arg.expression_identifier);
        self.align_finish(align_kind::IDENTIFIER);
        match &*arg.identifier_statement_group {
            IdentifierStatementGroup::FunctionCall(x) => {
                self.function_call(&x.function_call);
            }
            IdentifierStatementGroup::Assignment(x) => {
                self.assignment(&x.assignment);
            }
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'Assignment'
    fn assignment(&mut self, arg: &Assignment) {
        self.space(1);
        self.align_start(align_kind::ASSIGNMENT);
        match &*arg.assignment_group {
            AssignmentGroup::Equ(x) => self.equ(&x.equ),
            AssignmentGroup::AssignmentOperator(x) => {
                self.assignment_operator(&x.assignment_operator)
            }
            AssignmentGroup::DiamondOperator(x) => {
                self.diamond_operator(&x.diamond_operator);
            }
        }
        self.align_finish(align_kind::ASSIGNMENT);
        self.space(1);
        self.expression(&arg.expression);
    }

    /// Semantic action for non-terminal 'StatementBlock'
    fn statement_block(&mut self, arg: &StatementBlock) {
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.statement_block_list.iter().enumerate() {
            self.newline_list(i);
            self.statement_block_group(&x.statement_block_group);
        }
        self.newline_list_post(
            arg.statement_block_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'StatementBlockGroup'
    fn statement_block_group(&mut self, arg: &StatementBlockGroup) {
        for x in &arg.statement_block_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match arg.statement_block_group_group.as_ref() {
            StatementBlockGroupGroup::LBraceStatementBlockGroupGroupListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                for (i, x) in x.statement_block_group_group_list.iter().enumerate() {
                    self.newline_list(i);
                    self.statement_block_group(&x.statement_block_group);
                }
                self.newline_list_post(
                    x.statement_block_group_group_list.is_empty(),
                    &x.l_brace.l_brace_token,
                );
                self.r_brace(&x.r_brace);
            }
            StatementBlockGroupGroup::StatementBlockItem(x) => {
                self.statement_block_item(&x.statement_block_item);
            }
        }
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.statement_block(&arg.statement_block);
        for x in &arg.if_statement_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.statement_block(&x.statement_block);
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.statement_block(&x.statement_block);
        }
    }

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        self.if_reset(&arg.if_reset);
        self.space(1);
        self.statement_block(&arg.statement_block);
        for x in &arg.if_reset_statement_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.statement_block(&x.statement_block);
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.statement_block(&x.statement_block);
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
        self.statement_block(&arg.statement_block);
    }

    /// Semantic action for non-terminal 'CaseStatement'
    fn case_statement(&mut self, arg: &CaseStatement) {
        self.case(&arg.case);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.align_reset();
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.case_item(&x.case_item);
        }
        self.newline_list_post(
            arg.case_statement_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'CaseItem'
    fn case_item(&mut self, arg: &CaseItem) {
        self.align_start(align_kind::EXPRESSION);
        match &*arg.case_item_group {
            CaseItemGroup::CaseCondition(x) => self.case_condition(&x.case_condition),
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.case_item_group0 {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::StatementBlock(x) => {
                self.statement_block(&x.statement_block);
            }
        }
    }

    /// Semantic action for non-terminal 'CaseCondition'
    fn case_condition(&mut self, arg: &CaseCondition) {
        self.range_item(&arg.range_item);
        for x in &arg.case_condition_list {
            self.comma(&x.comma);
            if x.comma.line() != x.range_item.line() {
                self.newline();
                self.align_finish(align_kind::EXPRESSION);
                self.align_start(align_kind::EXPRESSION);
            } else {
                self.space(1);
            }
            self.range_item(&x.range_item);
        }
    }

    /// Semantic action for non-terminal 'SwitchStatement'
    fn switch_statement(&mut self, arg: &SwitchStatement) {
        self.switch(&arg.switch);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.switch_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.switch_item(&x.switch_item);
        }
        self.newline_list_post(
            arg.switch_statement_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'SwitchItem'
    fn switch_item(&mut self, arg: &SwitchItem) {
        self.align_start(align_kind::EXPRESSION);
        match &*arg.switch_item_group {
            SwitchItemGroup::SwitchCondition(x) => self.switch_condition(&x.switch_condition),
            SwitchItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.switch_item_group0 {
            SwitchItemGroup0::Statement(x) => self.statement(&x.statement),
            SwitchItemGroup0::StatementBlock(x) => {
                self.statement_block(&x.statement_block);
            }
        }
    }

    /// Semantic action for non-terminal 'SwitchCondition'
    fn switch_condition(&mut self, arg: &SwitchCondition) {
        self.expression(&arg.expression);
        for x in &arg.switch_condition_list {
            self.comma(&x.comma);
            if x.comma.line() != x.expression.line() {
                self.newline();
                self.align_finish(align_kind::EXPRESSION);
                self.align_start(align_kind::EXPRESSION);
            } else {
                self.space(1);
            }
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'Attribute'
    fn attribute(&mut self, arg: &Attribute) {
        self.in_attribute = true;
        self.hash(&arg.hash);
        self.l_bracket(&arg.l_bracket);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.attribute_opt {
            self.l_paren(&x.l_paren);
            self.attribute_list(&x.attribute_list);
            self.r_paren(&x.r_paren);
        }
        self.r_bracket(&arg.r_bracket);
        self.in_attribute = false;
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
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        if let Some(ref x) = arg.let_declaration_opt {
            self.align_start(align_kind::CLOCK_DOMAIN);
            self.clock_domain(&x.clock_domain);
            self.space(1);
            self.align_finish(align_kind::CLOCK_DOMAIN);
        } else {
            self.align_start(align_kind::CLOCK_DOMAIN);
            self.align_dummy_token(align_kind::CLOCK_DOMAIN, &arg.colon.colon_token);
            self.align_finish(align_kind::CLOCK_DOMAIN);
        }
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
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        if let Some(ref x) = arg.var_declaration_opt {
            self.align_start(align_kind::CLOCK_DOMAIN);
            self.clock_domain(&x.clock_domain);
            self.space(1);
            self.align_finish(align_kind::CLOCK_DOMAIN);
        } else {
            self.align_start(align_kind::CLOCK_DOMAIN);
            self.align_dummy_token(align_kind::CLOCK_DOMAIN, &arg.colon.colon_token);
            self.align_finish(align_kind::CLOCK_DOMAIN);
        }
        self.array_type(&arg.array_type);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ConstDeclaration'
    fn const_declaration(&mut self, arg: &ConstDeclaration) {
        self.r#const(&arg.r#const);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.const_declaration_group {
            ConstDeclarationGroup::ArrayType(x) => {
                self.array_type(&x.array_type);
            }
            ConstDeclarationGroup::Type(x) => {
                self.align_start(align_kind::TYPE);
                self.r#type(&x.r#type);
                self.align_finish(align_kind::TYPE);
            }
        }
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'TypeDefDeclaration'
    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) {
        self.r#type(&arg.r#type);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
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
            self.always_ff_event_list(&x.always_ff_event_list);
        }
        self.statement_block(&arg.statement_block);
    }

    /// Semantic action for non-terminal 'AlwaysFfEventList'
    fn always_ff_event_list(&mut self, arg: &AlwaysFfEventList) {
        self.l_paren(&arg.l_paren);
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.always_ff_event_list_opt {
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
        self.statement_block(&arg.statement_block);
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        self.assign(&arg.assign);
        self.space(1);
        self.assign_destination(&arg.assign_destination);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'AssignDestination'
    fn assign_destination(&mut self, arg: &AssignDestination) {
        match arg {
            AssignDestination::HierarchicalIdentifier(x) => {
                self.align_start(align_kind::IDENTIFIER);
                self.hierarchical_identifier(&x.hierarchical_identifier);
                self.align_finish(align_kind::IDENTIFIER);
            }
            AssignDestination::LBraceAssignConcatenationListRBrace(x) => {
                self.l_brace(&x.l_brace);
                self.assign_concatenation_list(&x.assign_concatenation_list);
                self.r_brace(&x.r_brace);
            }
        }
    }

    /// Semantic action for non-terminal 'AssignConcatenationList'
    fn assign_concatenation_list(&mut self, arg: &AssignConcatenationList) {
        self.assign_concatenation_item(&arg.assign_concatenation_item);
        for x in &arg.assign_concatenation_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.assign_concatenation_item(&x.assign_concatenation_item);
        }
        if let Some(ref x) = arg.assign_concatenation_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'ConnectDeclaration'
    fn connect_declaration(&mut self, arg: &ConnectDeclaration) {
        self.connect(&arg.connect);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.space(1);
        self.diamond_operator(&arg.diamond_operator);
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
        if let Some(ref x) = arg.modport_declaration_opt {
            self.modport_list(&x.modport_list);
        }
        if let Some(ref x) = arg.modport_declaration_opt0 {
            if arg.modport_declaration_opt.is_some() {
                self.newline();
            }
            self.dot_dot(&x.dot_dot);
            self.modport_default(&x.modport_default);
        }
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
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        self.direction(&arg.direction);
    }

    /// Semantic action for non-terminal 'EnumDeclaration'
    fn enum_declaration(&mut self, arg: &EnumDeclaration) {
        self.r#enum(&arg.r#enum);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.enum_declaration_opt {
            self.colon(&x.colon);
            self.space(1);
            self.scalar_type(&x.scalar_type);
        }
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
            self.align_reset();
        }
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.struct_union_list(&arg.struct_union_list);
        self.newline_pop();
        self.r_brace(&arg.r_brace);
        self.align_reset();
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
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        self.scalar_type(&arg.scalar_type);
    }

    /// Semantic action for non-terminal 'InitialDeclaration'
    fn initial_declaration(&mut self, arg: &InitialDeclaration) {
        self.initial(&arg.initial);
        self.space(1);
        self.statement_block(&arg.statement_block);
    }

    /// Semantic action for non-terminal 'FinalDeclaration'
    fn final_declaration(&mut self, arg: &FinalDeclaration) {
        self.r#final(&arg.r#final);
        self.space(1);
        self.statement_block(&arg.statement_block);
    }

    /// Semantic action for non-terminal 'InstDeclaration'
    fn inst_declaration(&mut self, arg: &InstDeclaration) {
        let single_line = is_single_line_inst_declaration(arg);

        self.inst(&arg.inst);
        self.space(1);
        if single_line {
            self.single_line_start();
        }
        if self.single_line() {
            self.align_start(align_kind::IDENTIFIER);
        }
        self.identifier(&arg.identifier);
        if self.single_line() {
            self.align_finish(align_kind::IDENTIFIER);
        }
        self.colon(&arg.colon);
        self.space(1);
        if let Some(ref x) = arg.inst_declaration_opt {
            if self.single_line() {
                self.align_start(align_kind::CLOCK_DOMAIN);
            }
            self.clock_domain(&x.clock_domain);
            self.space(1);
            if self.single_line() {
                self.align_finish(align_kind::CLOCK_DOMAIN);
            }
        } else if self.single_line() {
            self.align_start(align_kind::CLOCK_DOMAIN);
            self.align_dummy_token(align_kind::CLOCK_DOMAIN, &arg.colon.colon_token);
            self.align_finish(align_kind::CLOCK_DOMAIN);
        }
        self.scoped_identifier(&arg.scoped_identifier);
        // skip align at single line
        if self.mode == Mode::Align && self.single_line() {
            self.single_line_finish();
            return;
        }
        if let Some(ref x) = arg.inst_declaration_opt0 {
            self.space(1);
            self.array(&x.array);
        }
        if let Some(ref x) = arg.inst_declaration_opt1 {
            self.space(1);
            self.inst_parameter(&x.inst_parameter);
        }
        if let Some(ref x) = arg.inst_declaration_opt2 {
            if let Some(ref y) = x.inst_declaration_opt3 {
                self.space(1);
                self.token_will_push(&x.l_paren.l_paren_token);
                self.newline_push();
                self.inst_port_list(&y.inst_port_list);
                self.newline_pop();
                self.r_paren(&x.r_paren);
            } else {
                // Remove empty `()`
            }
        }
        self.semicolon(&arg.semicolon);
        if single_line {
            self.single_line_finish();
        }
    }

    /// Semantic action for non-terminal 'InstParameter'
    fn inst_parameter(&mut self, arg: &InstParameter) {
        if let Some(ref x) = arg.inst_parameter_opt {
            self.hash(&arg.hash);
            self.token_will_push(&arg.l_paren.l_paren_token);
            self.newline_push();
            self.inst_parameter_list(&x.inst_parameter_list);
            self.newline_pop();
            self.r_paren(&arg.r_paren);
        } else {
            // Remove empty '#()'
        }
    }

    /// Semantic action for non-terminal 'InstParameterList'
    fn inst_parameter_list(&mut self, arg: &InstParameterList) {
        self.inst_parameter_group(&arg.inst_parameter_group);
        for x in &arg.inst_parameter_list_list {
            self.comma(&x.comma);
            self.newline();
            self.inst_parameter_group(&x.inst_parameter_group);
        }
        // Omit trailing comma at single_line
        if !self.single_line() {
            if let Some(ref x) = arg.inst_parameter_list_opt {
                self.comma(&x.comma);
            } else {
                self.str(",");
            }
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
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        if let Some(ref x) = arg.inst_parameter_item_opt {
            self.colon(&x.colon);
            self.space(1);
            self.align_start(align_kind::EXPRESSION);
            self.expression(&x.expression);
            self.align_finish(align_kind::EXPRESSION);
        } else {
            self.align_insert(&arg.identifier.identifier_token, ": ".len());
            self.align_start(align_kind::EXPRESSION);
            self.align_dummy_token(align_kind::EXPRESSION, &arg.identifier.identifier_token);
            self.align_finish(align_kind::EXPRESSION);
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
        // Omit trailing comma at single_line
        if !self.single_line() {
            if let Some(ref x) = arg.inst_port_list_opt {
                self.comma(&x.comma);
            } else {
                self.str(",");
            }
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
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        if let Some(ref x) = arg.inst_port_item_opt {
            self.colon(&x.colon);
            self.space(1);
            self.align_start(align_kind::EXPRESSION);
            self.expression(&x.expression);
            self.align_finish(align_kind::EXPRESSION);
        } else {
            self.align_insert(&arg.identifier.identifier_token, ": ".len());
            self.align_start(align_kind::EXPRESSION);
            self.align_dummy_token(align_kind::EXPRESSION, &arg.identifier.identifier_token);
            self.align_finish(align_kind::EXPRESSION);
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

    /// Semantic action for non-terminal 'GenericBound'
    fn generic_bound(&mut self, arg: &GenericBound) {
        match arg {
            GenericBound::Type(x) => self.r#type(&x.r#type),
            GenericBound::InstScopedIdentifier(x) => {
                self.inst(&x.inst);
                self.space(1);
                self.scoped_identifier(&x.scoped_identifier);
            }
            GenericBound::GenericProtoBound(x) => match &*x.generic_proto_bound {
                GenericProtoBound::ScopedIdentifier(x) => {
                    self.scoped_identifier(&x.scoped_identifier)
                }
                GenericProtoBound::FixedType(x) => self.fixed_type(&x.fixed_type),
            },
        }
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        self.align_start(align_kind::PARAMETER);
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::Param(x) => self.param(&x.param),
            WithParameterItemGroup::Const(x) => self.r#const(&x.r#const),
        };
        self.align_finish(align_kind::PARAMETER);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.with_parameter_item_group0 {
            WithParameterItemGroup0::ArrayType(x) => {
                self.array_type(&x.array_type);
            }
            WithParameterItemGroup0::Type(x) => {
                self.align_start(align_kind::TYPE);
                self.r#type(&x.r#type);
                self.align_finish(align_kind::TYPE);
            }
        }
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.align_start(align_kind::EXPRESSION);
        self.expression(&arg.expression);
        self.align_finish(align_kind::EXPRESSION);
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

    /// Semantic action for non-terminal 'WithGenericParameterItem'
    fn with_generic_parameter_item(&mut self, arg: &WithGenericParameterItem) {
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.generic_bound(&arg.generic_bound);
        if let Some(ref x) = arg.with_generic_parameter_item_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.with_generic_argument_item(&x.with_generic_argument_item);
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
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.port_declaration_item_group {
            PortDeclarationItemGroup::PortTypeConcrete(x) => {
                let x = x.port_type_concrete.as_ref();
                self.direction(&x.direction);
                self.space(1);
                if let Some(ref x) = x.port_type_concrete_opt {
                    self.align_start(align_kind::CLOCK_DOMAIN);
                    self.clock_domain(&x.clock_domain);
                    self.space(1);
                    self.align_finish(align_kind::CLOCK_DOMAIN);
                } else {
                    self.align_start(align_kind::CLOCK_DOMAIN);
                    let token = match x.direction.as_ref() {
                        Direction::Input(x) => &x.input.input_token,
                        Direction::Output(x) => &x.output.output_token,
                        Direction::Inout(x) => &x.inout.inout_token,
                        Direction::Modport(x) => &x.modport.modport_token,
                        Direction::Import(x) => &x.import.import_token,
                    };
                    self.align_dummy_token(align_kind::CLOCK_DOMAIN, token);
                    self.align_finish(align_kind::CLOCK_DOMAIN);
                }
                self.array_type(&x.array_type);
                self.align_start(align_kind::EXPRESSION);
                if let Some(ref x) = x.port_type_concrete_opt0 {
                    self.space(1);
                    self.equ(&x.equ);
                    self.space(1);
                    self.expression(&x.port_default_value.expression);
                } else {
                    let loc = self.align_last_location(align_kind::ARRAY);
                    self.align_dummy_location(align_kind::EXPRESSION, loc);
                }
                self.align_finish(align_kind::EXPRESSION);
            }
            PortDeclarationItemGroup::PortTypeAbstract(x) => {
                let x = x.port_type_abstract.as_ref();
                if let Some(ref x) = x.port_type_abstract_opt {
                    self.align_start(align_kind::CLOCK_DOMAIN);
                    self.clock_domain(&x.clock_domain);
                    self.space(1);
                    self.align_finish(align_kind::CLOCK_DOMAIN);
                } else {
                    self.align_start(align_kind::CLOCK_DOMAIN);
                    self.align_dummy_token(align_kind::CLOCK_DOMAIN, &arg.colon.colon_token);
                    self.align_finish(align_kind::CLOCK_DOMAIN);
                }
                self.interface(&x.interface);
                if let Some(ref x) = x.port_type_abstract_opt0 {
                    self.colon_colon(&x.colon_colon);
                    self.identifier(&x.identifier);
                }
                if let Some(ref x) = x.port_type_abstract_opt1 {
                    self.space(1);
                    self.array(&x.array);
                }
            }
        }
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        self.align_start(align_kind::DIRECTION);
        match arg {
            Direction::Input(x) => self.input(&x.input),
            Direction::Output(x) => self.output(&x.output),
            Direction::Inout(x) => self.inout(&x.inout),
            Direction::Modport(x) => self.modport(&x.modport),
            Direction::Import(x) => self.import(&x.import),
        };
        self.align_finish(align_kind::DIRECTION);
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        self.function(&arg.function);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.function_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
            self.align_reset();
        }
        self.space(1);
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
            self.space(1);
            self.align_reset();
        }
        if let Some(ref x) = arg.function_declaration_opt1 {
            self.minus_g_t(&x.minus_g_t);
            self.space(1);
            self.scalar_type(&x.scalar_type);
            self.space(1);
            self.align_reset();
        }
        self.statement_block(&arg.statement_block);
        self.align_reset();
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

    /// Semantic action for non-terminal 'UnsafeBlock'
    fn unsafe_block(&mut self, arg: &UnsafeBlock) {
        self.r#unsafe(&arg.r#unsafe);
        self.space(1);
        self.l_paren(&arg.l_paren);
        self.identifier(&arg.identifier);
        self.r_paren(&arg.r_paren);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.unsafe_block_list.iter().enumerate() {
            self.newline_list(i);
            self.generate_group(&x.generate_group);
        }
        self.newline_list_post(arg.unsafe_block_list.is_empty(), &arg.l_brace.l_brace_token);
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        self.module(&arg.module);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.module_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
            self.align_reset();
        }
        self.space(1);
        if let Some(ref x) = arg.module_declaration_opt0 {
            self.r#for(&x.r#for);
            self.space(1);
            self.scoped_identifier(&x.scoped_identifier);
            self.space(1);
        }
        if let Some(ref x) = arg.module_declaration_opt1 {
            self.with_parameter(&x.with_parameter);
            self.space(1);
            self.align_reset();
        }
        if let Some(ref x) = arg.module_declaration_opt2 {
            self.port_declaration(&x.port_declaration);
            self.space(1);
            self.align_reset();
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.module_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.module_group(&x.module_group);
        }
        self.newline_list_post(
            arg.module_declaration_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
        self.align_reset();
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
                self.newline_list_post(
                    x.module_group_group_list.is_empty(),
                    &x.l_brace.l_brace_token,
                );
                self.r_brace(&x.r_brace);
            }
            ModuleGroupGroup::ModuleItem(x) => self.module_item(&x.module_item),
        }
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        self.interface(&arg.interface);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.interface_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
            self.align_reset();
        }
        self.space(1);
        if let Some(ref x) = arg.interface_declaration_opt0 {
            self.r#for(&x.r#for);
            self.space(1);
            self.scoped_identifier(&x.scoped_identifier);
            self.space(1);
        }
        if let Some(ref x) = arg.interface_declaration_opt1 {
            self.with_parameter(&x.with_parameter);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.interface_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.interface_group(&x.interface_group);
        }
        self.newline_list_post(
            arg.interface_declaration_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
        self.align_reset();
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
                self.newline_list_post(
                    x.interface_group_group_list.is_empty(),
                    &x.l_brace.l_brace_token,
                );
                self.r_brace(&x.r_brace);
            }
            InterfaceGroupGroup::InterfaceItem(x) => self.interface_item(&x.interface_item),
        }
    }

    /// Semantic action for non-terminal 'GenerateIfDeclaration'
    fn generate_if_declaration(&mut self, arg: &GenerateIfDeclaration) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.generate_named_block(&arg.generate_named_block);
        for x in &arg.generate_if_declaration_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.generate_optional_named_block(&x.generate_optional_named_block);
        }
        if let Some(ref x) = arg.generate_if_declaration_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.generate_optional_named_block(&x.generate_optional_named_block);
        }
    }

    /// Semantic action for non-terminal 'GenerateForDeclaration'
    fn generate_for_declaration(&mut self, arg: &GenerateForDeclaration) {
        self.r#for(&arg.r#for);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.r#in(&arg.r#in);
        self.space(1);
        self.range(&arg.range);
        self.space(1);
        if let Some(ref x) = arg.generate_for_declaration_opt {
            self.step(&x.step);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
        }
        self.generate_named_block(&arg.generate_named_block);
    }

    /// Semantic action for non-terminal 'GenerateNamedBlock'
    fn generate_named_block(&mut self, arg: &GenerateNamedBlock) {
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.generate_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.generate_group(&x.generate_group);
        }
        self.newline_list_post(
            arg.generate_named_block_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'GenerateOptionalNamedBlock'
    fn generate_optional_named_block(&mut self, arg: &GenerateOptionalNamedBlock) {
        if let Some(ref x) = arg.generate_optional_named_block_opt {
            self.colon(&x.colon);
            self.identifier(&x.identifier);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.generate_optional_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.generate_group(&x.generate_group);
        }
        self.newline_list_post(
            arg.generate_optional_named_block_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'GenerateGroup'
    fn generate_group(&mut self, arg: &GenerateGroup) {
        for x in &arg.generate_group_list {
            self.attribute(&x.attribute);
            self.newline();
        }
        match &*arg.generate_group_group {
            GenerateGroupGroup::LBraceGenerateGroupGroupListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token);
                for (i, x) in x.generate_group_group_list.iter().enumerate() {
                    self.newline_list(i);
                    self.generate_group(&x.generate_group);
                }
                self.newline_list_post(
                    x.generate_group_group_list.is_empty(),
                    &x.l_brace.l_brace_token,
                );
                self.r_brace(&x.r_brace);
            }
            GenerateGroupGroup::GenerateItem(x) => self.generate_item(&x.generate_item),
        }
    }

    /// Semantic action for non-terminal 'PackageDeclaration'
    fn package_declaration(&mut self, arg: &PackageDeclaration) {
        self.package(&arg.package);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.package_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
            self.align_reset();
        }
        self.space(1);
        if let Some(ref x) = arg.package_declaration_opt0 {
            self.r#for(&x.r#for);
            self.space(1);
            self.scoped_identifier(&x.scoped_identifier);
            self.space(1);
        }
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.package_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.package_group(&x.package_group);
        }
        self.newline_list_post(
            arg.package_declaration_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
        self.align_reset();
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
                self.newline_list_post(
                    x.package_group_group_list.is_empty(),
                    &x.l_brace.l_brace_token,
                );
                self.r_brace(&x.r_brace);
            }
            PackageGroupGroup::PackageItem(x) => self.package_item(&x.package_item),
        }
    }

    /// Semantic action for non-terminal 'AliasDeclaration'
    fn alias_declaration(&mut self, arg: &AliasDeclaration) {
        self.alias(&arg.alias);
        self.space(1);
        match &*arg.alias_declaration_group {
            AliasDeclarationGroup::Module(x) => self.module(&x.module),
            AliasDeclarationGroup::Interface(x) => self.interface(&x.interface),
            AliasDeclarationGroup::Package(x) => self.package(&x.package),
        }
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ProtoDeclaration'
    fn proto_declaration(&mut self, arg: &ProtoDeclaration) {
        self.proto(&arg.proto);
        self.space(1);
        match &*arg.proto_declaration_group {
            ProtoDeclarationGroup::ProtoModuleDeclaration(x) => {
                self.proto_module_declaration(&x.proto_module_declaration);
            }
            ProtoDeclarationGroup::ProtoInterfaceDeclaration(x) => {
                self.proto_interface_declaration(&x.proto_interface_declaration);
            }
            ProtoDeclarationGroup::ProtoPackageDeclaration(x) => {
                self.proto_package_declaration(&x.proto_package_declaration);
            }
        }
    }

    /// Semantic action for non-terminal 'ProtoModuleDeclaration'
    fn proto_module_declaration(&mut self, arg: &ProtoModuleDeclaration) {
        self.module(&arg.module);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.proto_module_declaration_opt {
            self.space(1);
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.proto_module_declaration_opt0 {
            self.space(1);
            self.port_declaration(&x.port_declaration);
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ProtoInterfaceDeclaration'
    fn proto_interface_declaration(&mut self, arg: &ProtoInterfaceDeclaration) {
        self.interface(&arg.interface);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.proto_interface_declaration_opt {
            self.space(1);
            self.with_parameter(&x.with_parameter);
        }
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.proto_interface_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.proto_interface_item(&x.proto_interface_item);
        }
        self.newline_list_post(
            arg.proto_interface_declaration_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ProtoPackageDeclaration'
    fn proto_package_declaration(&mut self, arg: &ProtoPackageDeclaration) {
        self.package(&arg.package);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        for (i, x) in arg.proto_package_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.proto_pacakge_item(&x.proto_pacakge_item);
        }
        self.newline_list_post(
            arg.proto_package_declaration_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'ProtoConstDeclaration'
    fn proto_const_declaration(&mut self, arg: &ProtoConstDeclaration) {
        self.r#const(&arg.r#const);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.proto_const_declaration_group {
            ProtoConstDeclarationGroup::ArrayType(x) => {
                self.array_type(&x.array_type);
            }
            ProtoConstDeclarationGroup::Type(x) => {
                self.r#type(&x.r#type);
            }
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ProtoTypeDefDeclaration'
    fn proto_type_def_declaration(&mut self, arg: &ProtoTypeDefDeclaration) {
        self.r#type(&arg.r#type);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ProtoFunctionDeclaration'
    fn proto_function_declaration(&mut self, arg: &ProtoFunctionDeclaration) {
        self.function(&arg.function);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.proto_function_declaration_opt {
            self.with_generic_parameter(&x.with_generic_parameter);
            self.align_reset();
        }
        if let Some(ref x) = arg.proto_function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
            self.space(1);
            self.align_reset();
        }
        if let Some(ref x) = arg.proto_function_declaration_opt1 {
            self.minus_g_t(&x.minus_g_t);
            self.space(1);
            self.scalar_type(&x.scalar_type);
            self.space(1);
            self.align_reset();
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ProtoAliasDeclaration'
    fn proto_alias_declaration(&mut self, arg: &ProtoAliasDeclaration) {
        self.alias(&arg.alias);
        self.space(1);
        match &*arg.proto_alias_declaration_group {
            ProtoAliasDeclarationGroup::Module(x) => self.module(&x.module),
            ProtoAliasDeclarationGroup::Interface(x) => self.interface(&x.interface),
            ProtoAliasDeclarationGroup::Package(x) => self.package(&x.package),
        }
        self.space(1);
        self.identifier(&arg.identifier);
        self.colon(&arg.colon);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        self.semicolon(&arg.semicolon);
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

    /// Semantic action for non-terminal 'EmbedContent'
    fn embed_content(&mut self, arg: &EmbedContent) {
        self.embed_triple_l_brace(&arg.embed_triple_l_brace);
        self.keep_tail_newline = true;
        for x in &arg.embed_content_list {
            self.embed_item(&x.embed_item);
        }
        self.keep_tail_newline = false;
        if !self
            .string
            .chars()
            .last()
            .map(|c| c.is_ascii_whitespace())
            .unwrap_or(false)
        {
            self.newline();
        }
        self.embed_triple_r_brace(&arg.embed_triple_r_brace);
    }

    /// Semantic action for non-terminal 'EmbedIdentifier'
    fn embed_identifier(&mut self, arg: &EmbedIdentifier) {
        self.escaped_l_paren(&arg.escaped_l_paren);
        self.embed_l_paren(&arg.embed_l_paren);
        self.embed_l_paren(&arg.embed_l_paren0);
        self.scoped_identifier(&arg.scoped_identifier);
        self.embed_r_paren(&arg.embed_r_paren);
        self.embed_r_paren(&arg.embed_r_paren0);
        self.embed_r_paren(&arg.embed_r_paren1);
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
                self.newline_list_post(
                    x.description_group_group_list.is_empty(),
                    &x.l_brace.l_brace_token,
                );
                self.r_brace(&x.r_brace);
            }
            DescriptionGroupGroup::DescriptionItem(x) => self.description_item(&x.description_item),
        }
    }

    /// Semantic action for non-terminal 'DescriptionItem'
    fn description_item(&mut self, arg: &DescriptionItem) {
        match arg {
            DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) => {
                if let Some(ref x) = x.description_item_opt {
                    self.r#pub(&x.r#pub);
                    self.space(1);
                }
                self.public_description_item(&x.public_description_item);
            }
            DescriptionItem::ImportDeclaration(x) => self.import_declaration(&x.import_declaration),
            DescriptionItem::EmbedDeclaration(x) => self.embed_declaration(&x.embed_declaration),
            DescriptionItem::IncludeDeclaration(x) => {
                self.include_declaration(&x.include_declaration)
            }
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
        for (i, x) in arg.veryl_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.description_group(&x.description_group);
        }
        self.newline();
    }
}

fn is_single_line_inst_declaration(arg: &InstDeclaration) -> bool {
    if attribute_table::is_format(&arg.identifier.first(), FormatItem::Compact) {
        return true;
    }

    if let Some(x) = &arg.inst_declaration_opt2 {
        if x.inst_declaration_opt3.is_some() {
            // Non empty port list
            return false;
        }
    }

    true
}
