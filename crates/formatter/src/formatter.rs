use std::rc::Rc;
use veryl_aligner::{Aligner, Location, PadKind, align_kind};
use veryl_analyzer::attribute::{AlignItem, FormatItem};
use veryl_analyzer::attribute_table;
use veryl_metadata::{Format, Metadata};
use veryl_parser::resource_table;
use veryl_parser::token_collector::TokenCollector;
use veryl_parser::token_range::TokenExt;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, VerylToken};
use veryl_parser::veryl_walker::VerylWalker;
use veryl_pretty::doc::{self, CommentDoc, Doc};
use veryl_pretty::render::{RenderOpts, render};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Pass 1: feed tokens to the aligner. No output is produced.
    Align,
    /// Pass 2: build the Doc IR; rendered after walking completes.
    Emit,
}

pub struct Formatter {
    mode: Mode,
    format_opt: Format,
    newline: &'static str,

    string: String,
    /// Stack of currently-assembling Doc buffers (one per open indent block).
    doc_buffer: Vec<Vec<Doc>>,
    /// Last character emitted into the Doc tree; lets peek-at-tail code work
    /// before the renderer materializes output.
    last_emitted_char: Option<char>,

    indent: usize,
    line: u32,
    /// Set by `newline*()` helpers, consumed by `push_token`: insert a
    /// blank line when the next token skips a source line.
    adjust_line: bool,
    aligner: Aligner,

    keep_tail_newline: bool,

    in_scalar_type: bool,
    /// Depth counter for nested `expression` walkers.
    in_expression: Vec<()>,
    in_attribute: bool,
    /// Per-`function_call` stack of "uses named arguments" flags.
    in_named_argument: Vec<bool>,
}

impl Default for Formatter {
    fn default() -> Self {
        Self {
            mode: Mode::Emit,
            format_opt: Format::default(),
            newline: "\n",

            string: String::new(),
            doc_buffer: Vec::new(),
            last_emitted_char: None,

            indent: 0,
            line: 1,
            adjust_line: false,
            aligner: Aligner::new(),

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

    pub fn format(&mut self, input: &Veryl, raw_input: &str) {
        self.newline = self.format_opt.newline_style.newline_str(raw_input);
        if self.format_opt.vertical_align {
            self.mode = Mode::Align;
            self.veryl(input);
            self.aligner.finish_group();
            self.aligner.gather_additions();
        }
        self.mode = Mode::Emit;
        self.doc_buffer = vec![Vec::new()];
        self.veryl(input);
        let top = self.doc_buffer.pop().unwrap_or_default();
        let doc = doc::concat(top);
        let opts = RenderOpts {
            max_width: self.format_opt.max_width,
            indent_width: self.format_opt.indent_width,
            newline: self.newline,
            strip_trailing_whitespace: true,
        };
        self.string = render(&doc, &opts);
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    /// No-op in `Mode::Align`, so callers can emit unconditionally.
    fn emit_doc(&mut self, d: Doc) {
        if !matches!(self.mode, Mode::Emit) {
            return;
        }
        if matches!(d, Doc::Nil) {
            return;
        }
        self.doc_buffer
            .last_mut()
            .expect("doc_buffer must have at least one frame in Mode::Emit")
            .push(d);
    }

    fn push_indent_block(&mut self) {
        self.doc_buffer.push(Vec::new());
    }

    /// Close the innermost indent block: wrap its accumulated docs in
    /// `Doc::Indent(+1, _)` and push onto the parent frame.
    fn pop_indent_block(&mut self) {
        let inner = self.doc_buffer.pop().expect("indent block underflow");
        let nested = doc::indent_by(1, doc::concat(inner));
        if !matches!(nested, Doc::Nil) {
            self.doc_buffer
                .last_mut()
                .expect("indent block has no parent")
                .push(nested);
        }
    }

    fn str(&mut self, x: &str) {
        match self.mode {
            Mode::Emit => {
                if let Some(c) = x.chars().next_back() {
                    self.last_emitted_char = Some(c);
                }
                self.emit_doc(doc::text(x));
            }
            Mode::Align => {
                self.aligner.space(x.len());
            }
        }
    }

    fn newline_push(&mut self) {
        if matches!(self.mode, Mode::Emit) {
            self.push_indent_block();
            self.emit_doc(Doc::Hardline);
            self.last_emitted_char = Some('\n');
            self.indent += 1;
            self.adjust_line = true;
        }
    }

    fn newline_pop(&mut self) {
        if matches!(self.mode, Mode::Emit) {
            self.pop_indent_block();
            self.indent -= 1;
            self.emit_doc(Doc::Hardline);
            self.last_emitted_char = Some('\n');
            self.adjust_line = true;
        }
    }

    fn newline(&mut self) {
        if matches!(self.mode, Mode::Emit) {
            self.emit_doc(Doc::Hardline);
            self.last_emitted_char = Some('\n');
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
        } else if let Some(last_commant) = start_token.comments.last()
            && resource_table::get_str_value(last_commant.text)
                .map(|x| !x.ends_with("\n"))
                .unwrap()
        {
            self.newline();
        }
    }

    fn space(&mut self, repeat: usize) {
        self.str(&" ".repeat(repeat));
    }

    /// No-op outside `Mode::Emit` (where there's no doc tree to capture).
    fn buf_begin(&mut self) {
        if matches!(self.mode, Mode::Emit) {
            self.doc_buffer.push(Vec::new());
        }
    }

    /// Returns `Doc::Nil` outside `Mode::Emit`.
    fn buf_end_concat(&mut self) -> Doc {
        if matches!(self.mode, Mode::Emit) {
            let inner = self.doc_buffer.pop().expect("buf underflow");
            doc::concat(inner)
        } else {
            Doc::Nil
        }
    }

    fn group_begin(&mut self) {
        self.buf_begin();
    }

    fn group_end(&mut self) {
        if matches!(self.mode, Mode::Emit) {
            let inner = self.buf_end_concat();
            self.emit_doc(doc::group(inner));
        }
    }

    /// Wrap the next emissions in a `+1` indent block. Doc-tree-only: does
    /// not bump `self.indent`.
    fn group_nest_begin(&mut self) {
        self.buf_begin();
    }

    fn group_nest_end(&mut self) {
        if matches!(self.mode, Mode::Emit) {
            let inner = self.buf_end_concat();
            self.emit_doc(doc::indent_by(1, inner));
        }
    }

    fn soft_line(&mut self) {
        match self.mode {
            Mode::Emit => self.emit_doc(doc::line()),
            _ => self.space(1),
        }
    }

    fn soft_break(&mut self) {
        if matches!(self.mode, Mode::Emit) {
            self.emit_doc(doc::softline());
        }
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
                if let Some((width, kind)) = self.aligner.additions.get(&loc) {
                    match kind {
                        PadKind::Always => self.emit_doc(doc::pad(*width)),
                        PadKind::IfBreak => self.emit_doc(doc::if_break_pad(*width)),
                        PadKind::IfFlat => self.emit_doc(doc::if_flat_pad(*width)),
                    }
                }

                self.emit_trailing_comments(x, will_push);
            }
            Mode::Align => {
                self.aligner.token(x);
            }
        }
    }

    /// `wrap_indent` adds `+1` indent for the case where the next emitted
    /// thing will be a `{` immediately followed by a `newline_push`.
    fn emit_trailing_comments(&mut self, x: &VerylToken, wrap_indent: bool) {
        if !matches!(self.mode, Mode::Emit) {
            return;
        }
        if x.comments.is_empty() {
            return;
        }
        let mut cs: Vec<CommentDoc> = Vec::with_capacity(x.comments.len());
        let mut prev_line = self.line;
        for c in &x.comments {
            let raw = resource_table::get_str_value(c.text).unwrap();
            let is_line_comment = raw.ends_with('\n');
            let trimmed: String = if is_line_comment {
                raw.trim_end().to_string()
            } else {
                raw.clone()
            };
            let leading_newlines = c.line.saturating_sub(prev_line);
            cs.push(CommentDoc {
                text: Rc::<str>::from(trimmed.as_str()),
                leading_newlines,
                is_line_comment,
                src_line: 0,
                src_column: 0,
            });
            // Advance our notion of the source line. For line comments
            // the trailing `\n` is consumed; for block comments we add
            // any embedded newlines from the body.
            let raw_nls = raw.matches('\n').count() as u32;
            let trailing = if is_line_comment {
                raw_nls.saturating_sub(1)
            } else {
                raw_nls
            };
            prev_line = c.line + trailing;
        }
        self.line = prev_line;
        let mut node = doc::comments(cs);
        if wrap_indent {
            node = doc::indent_by(1, node);
        }
        self.emit_doc(node);
    }

    fn token(&mut self, x: &VerylToken) {
        self.process_token(x, false)
    }

    fn token_will_push(&mut self, x: &VerylToken) {
        self.process_token(x, true)
    }

    fn align_start(&mut self, kind: usize) {
        if self.mode == Mode::Align {
            self.aligner.aligns[kind].start_item();
        }
    }

    fn align_start_break_gated(&mut self, kind: usize) {
        if self.mode == Mode::Align {
            self.aligner.aligns[kind].start_item_break_gated();
        }
    }

    fn align_start_maybe_flat_gated(&mut self, kind: usize, flat_gated: bool) {
        if flat_gated {
            self.align_start_flat_gated(kind);
        } else {
            self.align_start(kind);
        }
    }

    /// A value wide enough to wrap would drag short siblings into a huge
    /// padding column, so wide items are isolated via `align_reset`.
    fn emit_inst_item(&mut self, identifier: &Identifier, body: Option<(&Colon, &Expression)>) {
        self.align_start_break_gated(align_kind::INST_ITEM_IDENTIFIER);
        self.identifier(identifier);
        self.align_finish(align_kind::INST_ITEM_IDENTIFIER);
        if let Some((colon, expression)) = body {
            self.colon(colon);
            self.space(1);
            let isolate = estimated_expression_width(expression) > self.wrap_isolation_threshold();
            if isolate {
                self.align_reset();
            }
            self.align_start_break_gated(align_kind::INST_ITEM_EXPRESSION);
            self.expression(expression);
            self.align_finish(align_kind::INST_ITEM_EXPRESSION);
            if isolate {
                self.align_reset();
            }
        } else {
            self.align_insert_break_gated(&identifier.identifier_token, ": ".len());
            self.align_start_break_gated(align_kind::INST_ITEM_EXPRESSION);
            self.align_dummy_token(
                align_kind::INST_ITEM_EXPRESSION,
                &identifier.identifier_token,
            );
            self.align_finish(align_kind::INST_ITEM_EXPRESSION);
        }
    }

    fn align_start_flat_gated(&mut self, kind: usize) {
        if self.mode == Mode::Align {
            self.aligner.aligns[kind].start_item_flat_gated();
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
        if self.mode == Mode::Align
            && let Some(loc) = loc
        {
            self.aligner.aligns[kind].dummy_location(loc);
        }
    }

    fn align_dummy_token(&mut self, kind: usize, token: &VerylToken) {
        if self.mode == Mode::Align {
            self.aligner.aligns[kind].dummy_token(token);
        }
    }

    /// Discard the alignment context across a boundary where alignment
    /// must not propagate (e.g. before an `{...}` body with its own
    /// intra-block alignment).
    fn align_reset(&mut self) {
        if self.mode == Mode::Align {
            self.aligner.finish_item();
            self.aligner.finish_group();
            self.aligner.clear_had_item_in_statement();
        }
    }

    /// Carry reference lines forward so a wrapped trailing token doesn't
    /// look like a blank-line gap to the next statement's alignment.
    fn align_note_statement_end(&mut self) {
        if self.mode == Mode::Align {
            self.aligner.note_statement_end();
        }
    }

    /// Condition width above which an item is considered likely to
    /// wrap at render time and should be isolated. Subtracts a slack
    /// (indent + `: ` + body) from `max_width`, but never below
    /// `max_width / 2` so narrow configurations still permit short
    /// single-key arms.
    fn wrap_isolation_threshold(&self) -> u32 {
        let max_width = self.format_opt.max_width as u32;
        max_width.saturating_sub(24).max(max_width / 2)
    }

    /// Shared driver for case/switch items: isolates wide arms so a
    /// long multi-key condition doesn't drag short siblings to a huge
    /// alignment column. The isolation decision is structural (width-
    /// estimate based), not source-position based, to keep pass 1 and
    /// pass 2 in sync.
    fn aligned_case_arm<F: FnOnce(&mut Self)>(&mut self, wide_estimate: u32, body: F) {
        let isolate = wide_estimate > self.wrap_isolation_threshold();
        if isolate {
            self.align_reset();
        }
        self.align_start(align_kind::EXPRESSION);
        body(self);
        self.align_finish(align_kind::EXPRESSION);
        if isolate {
            self.align_reset();
        }
    }

    fn align_insert_break_gated(&mut self, token: &VerylToken, width: usize) {
        self.align_insert_with_kind(token, width, PadKind::IfBreak);
    }

    fn align_insert_with_kind(&mut self, token: &VerylToken, width: usize, kind: PadKind) {
        if self.mode == Mode::Align {
            let loc: Location = token.token.into();
            self.aligner
                .additions
                .entry(loc)
                .and_modify(|(val, k)| {
                    *val += width as u32;
                    *k = k.merge(kind);
                })
                .or_insert((width as u32, kind));
        }
    }

    fn format_inst(&mut self, arg: &ComponentInstantiation, semicolon: &Semicolon) {
        let compact = attribute_table::is_format(&arg.identifier.first(), FormatItem::Compact);
        let has_params = arg.component_instantiation_opt1.is_some();
        let has_ports = matches!(
            &arg.component_instantiation_opt2,
            Some(x) if x.inst_port.inst_port_opt.is_some()
        );
        let wrap_in_group = has_params || has_ports;

        if compact {
            self.buf_begin();
        } else if wrap_in_group {
            self.group_begin();
        }

        // When the inst body lives in a layout-flexible Group, the
        // cross-inst alignment is only meaningful in the flat layout.
        let flat_gated = wrap_in_group && !compact;
        self.align_start_maybe_flat_gated(align_kind::INST_NAME_IDENTIFIER, flat_gated);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::INST_NAME_IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        self.align_start_maybe_flat_gated(align_kind::CLOCK_DOMAIN, flat_gated);
        if let Some(ref x) = arg.component_instantiation_opt {
            self.clock_domain(&x.clock_domain);
            self.space(1);
        } else {
            self.align_dummy_token(align_kind::CLOCK_DOMAIN, &arg.colon.colon_token);
        }
        self.align_finish(align_kind::CLOCK_DOMAIN);
        self.scoped_identifier(&arg.scoped_identifier);
        if let Some(ref x) = arg.component_instantiation_opt0 {
            self.space(1);
            self.array(&x.array);
        }
        if let Some(ref x) = arg.component_instantiation_opt1 {
            self.space(1);
            self.inst_parameter(&x.inst_parameter);
        }
        if let Some(ref x) = arg.component_instantiation_opt2
            && let Some(ref y) = x.inst_port.inst_port_opt
        {
            self.space(1);
            self.token_will_push(&x.inst_port.l_paren.l_paren_token);
            self.group_nest_begin();
            self.soft_line();
            self.inst_port_list(&y.inst_port_list);
            self.group_nest_end();
            self.soft_line();
            self.r_paren(&x.inst_port.r_paren);
        }

        // Close the Group before emitting `;` — Veryl attaches
        // trailing line comments to `;`, and a Comments node with a
        // line comment makes `fits_flat` return false.
        if compact {
            let inner = self.buf_end_concat();
            self.emit_doc(doc::force_flat(inner));
        } else if wrap_in_group {
            self.group_end();
        }
        self.semicolon(semicolon);
    }

    /// Emit `embed` body tokens as one raw text node, preserving the
    /// original whitespace.
    fn unformat_embed_items(&mut self, arg: &EmbedContent) {
        let mut token_collector = TokenCollector::new(true);
        for x in &arg.embed_content_list {
            token_collector.embed_item(&x.embed_item);
        }
        if token_collector.tokens.is_empty() {
            return;
        }

        let mut string = String::new();
        for (i, token) in token_collector.tokens.iter().enumerate() {
            let (delta_line, delta_column) = if i > 0 {
                let last_token = token_collector.tokens[i - 1];
                let last_line = last_token.end_line();
                if token.line == last_line {
                    let last_column = last_token.end_column();
                    let delta_column = token.column - last_column - 1;
                    (0, delta_column)
                } else {
                    let delta_line = token.line - last_line;
                    (delta_line, token.column - 1)
                }
            } else {
                (0, 0)
            };

            for _ in 0..delta_line {
                string.push_str(self.newline);
            }
            for _ in 0..delta_column {
                string.push(' ');
            }

            string.push_str(&token.to_string());
        }

        let mut token = *token_collector.tokens.first().unwrap();
        token.text = resource_table::insert_str(&string);
        token.length = string.len() as u32;

        self.push_token(&token);
    }

    fn unformat_description_item(&mut self, arg: &DescriptionItem) {
        let mut token_collector = TokenCollector::new(true);
        token_collector.description_item(arg);

        let mut string = String::new();
        for (i, token) in token_collector.tokens.iter().enumerate() {
            let (delta_line, delta_column) = if i > 0 {
                let last_token = token_collector.tokens[i - 1];
                let last_line = last_token.end_line();
                if token.line == last_line {
                    let last_column = last_token.end_column();
                    let delat_column = token.column - last_column - 1;
                    (0, delat_column)
                } else {
                    let delta_line = token.line - last_line;
                    (delta_line, token.column - 1)
                }
            } else {
                (0, token.column - 1)
            };

            for _ in 0..delta_line {
                string.push_str(self.newline);
            }
            for _ in 0..delta_column {
                string.push(' ');
            }

            string.push_str(&token.to_string());
        }

        let mut token = *token_collector.tokens.first().unwrap();
        token.text = resource_table::insert_str(&string);
        token.length = string.len() as u32;

        self.push_token(&token);
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
            return;
        }
        let compact = attribute_table::is_format(&arg.first(), FormatItem::Compact);
        // For `#[fmt(compact)]` wrap the same Doc IR in `ForceFlat`:
        // every Hardline collapses to a space, so the if-expression
        // renders on a single line regardless of width.
        if compact {
            self.buf_begin();
        }
        // Broken layout indents each then-branch one level past `?`,
        // with each `:` returning to the outer indent.
        self.group_begin();
        for (i, x) in arg.if_expression_list.iter().enumerate() {
            self.r#if(&x.r#if);
            self.space(1);
            self.expression(&x.expression);
            self.space(1);
            self.token_will_push(&x.question.question_token);
            self.group_nest_begin();
            self.soft_line();
            self.expression(&x.expression0);
            self.group_nest_end();
            if (i + 1) < arg.if_expression_list.len() {
                self.soft_line();
                self.colon(&x.colon);
                self.space(1);
            } else {
                self.soft_line();
                self.token_will_push(&x.colon.colon_token);
            }
        }
        self.group_nest_begin();
        self.soft_line();
        self.expression01(&arg.expression01);
        self.group_nest_end();
        // When the group breaks, drop the trailing semicolon (or
        // surrounding context) onto a new line at the outer indent. In
        // flat mode this emits nothing.
        self.soft_break();
        self.group_end();
        if compact {
            let inner = self.buf_end_concat();
            self.emit_doc(doc::force_flat(inner));
        }
    }

    /// Semantic action for non-terminal 'Expression01'
    // Add `#[inline(never)]` to `expression*` as a workaround for long time compilation
    // https://github.com/rust-lang/rust/issues/106211
    #[inline(never)]
    fn expression01(&mut self, arg: &Expression01) {
        if arg.expression01_list.is_empty() {
            self.expression02(&arg.expression02);
            return;
        }
        // Each "<op> <rhs>" segment lives in its own group with a leading
        // soft line, wrapped in an outer group + nest so continuation
        // lines indent one level past the surrounding statement.
        self.group_begin();
        self.group_nest_begin();
        self.expression02(&arg.expression02);
        for x in &arg.expression01_list {
            self.group_begin();
            self.soft_line();
            self.expression01_op(&x.expression01_op);
            self.space(1);
            self.expression02(&x.expression02);
            self.group_end();
        }
        self.group_nest_end();
        self.group_end();
    }

    /// Semantic action for non-terminal 'Expression01Op'
    #[inline(never)]
    fn expression01_op(&mut self, arg: &Expression01Op) {
        match arg {
            Expression01Op::Operator01(x) => self.operator01(&x.operator01),
            Expression01Op::Operator02(x) => self.operator02(&x.operator02),
            Expression01Op::Operator03(x) => self.operator03(&x.operator03),
            Expression01Op::Operator04(x) => self.operator04(&x.operator04),
            Expression01Op::Operator05(x) => self.operator05(&x.operator05),
            Expression01Op::Operator06(x) => self.operator06(&x.operator06),
            Expression01Op::Operator07(x) => self.operator07(&x.operator07),
            Expression01Op::Star(x) => self.star(&x.star),
            Expression01Op::Operator08(x) => self.operator08(&x.operator08),
        }
    }

    /// Semantic action for non-terminal 'Expression02'
    #[inline(never)]
    fn expression02(&mut self, arg: &Expression02) {
        for x in &arg.expression02_list {
            self.expression02_op(&x.expression02_op);
        }
        self.factor(&arg.factor);
        if let Some(x) = &arg.expression02_opt {
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
                self.group_begin();
                self.l_brace(&x.l_brace);
                self.group_nest_begin();
                self.soft_break();
                self.concatenation_list(&x.concatenation_list);
                self.group_nest_end();
                self.soft_break();
                self.r_brace(&x.r_brace);
                self.group_end();
            }
            Factor::QuoteLBraceArrayLiteralListRBrace(x) => {
                self.group_begin();
                self.quote_l_brace(&x.quote_l_brace);
                self.group_nest_begin();
                self.soft_break();
                self.array_literal_list(&x.array_literal_list);
                self.group_nest_end();
                self.soft_break();
                self.r_brace(&x.r_brace);
                self.group_end();
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
            // Named arguments always force multi-line layout. The Group
            // wrap still matters: embedded Hardlines mark the context as
            // broken so `IfBreakPad` activates value-column alignment.
            self.group_begin();
            self.token_will_push(&arg.l_paren.l_paren_token);
            self.newline_push();
            self.align_reset();
            if let Some(ref x) = arg.function_call_opt {
                self.argument_list(&x.argument_list);
            }
            self.newline_pop();
            self.r_paren(&arg.r_paren);
            self.group_end();
        } else {
            // Positional arguments wrap purely by `max_width` via the
            // surrounding Group.
            self.group_begin();
            self.l_paren(&arg.l_paren);
            if let Some(ref x) = arg.function_call_opt {
                self.group_nest_begin();
                self.soft_break();
                self.argument_list(&x.argument_list);
                self.group_nest_end();
                self.soft_break();
            }
            self.r_paren(&arg.r_paren);
            self.group_end();
        }
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
                self.soft_line();
            }
            self.argument_item(&x.argument_item);
        }
        // `if_break(",")` keeps the trailing comma flat-only (so a
        // collapsed call isn't `func(a, b,)`). The source comma's
        // trailing comments are emitted separately so a `// ...`
        // between the last item and the closing delimiter survives.
        self.emit_doc(doc::if_break(","));
        if let Some(ref x) = arg.argument_list_opt {
            self.emit_trailing_comments(&x.comma.comma_token, false);
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
        // Width-driven stacked-or-flat layout.
        self.group_begin();
        self.quote_l_brace(&arg.quote_l_brace);
        self.group_nest_begin();
        self.soft_break();
        self.struct_constructor_list(&arg.struct_constructor_list);
        if let Some(ref x) = arg.struct_constructor_opt {
            self.soft_line();
            self.dot_dot(&x.dot_dot);
            self.defaul(&x.defaul);
            self.l_paren(&x.l_paren);
            self.expression(&x.expression);
            self.r_paren(&x.r_paren);
        }
        self.group_nest_end();
        self.soft_break();
        self.r_brace(&arg.r_brace);
        self.group_end();
    }

    /// Semantic action for non-terminal 'StructConstructorList'
    fn struct_constructor_list(&mut self, arg: &StructConstructorList) {
        // Stacked-or-flat: the outer struct-constructor group decides
        // break/flat once for the whole list, so columns line up when
        // the constructor wraps and disappear cleanly when it fits.
        self.struct_constructor_item(&arg.struct_constructor_item);
        for x in &arg.struct_constructor_list_list {
            self.comma(&x.comma);
            self.soft_line();
            self.struct_constructor_item(&x.struct_constructor_item);
        }
        if let Some(ref x) = arg.struct_constructor_list_opt {
            self.comma(&x.comma);
        }
    }

    /// Semantic action for non-terminal 'StructConstructorItem'
    fn struct_constructor_item(&mut self, arg: &StructConstructorItem) {
        self.align_start_break_gated(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        self.align_start_break_gated(align_kind::EXPRESSION);
        self.expression(&arg.expression);
        self.align_finish(align_kind::EXPRESSION);
    }

    /// Semantic action for non-terminal 'ConcatenationList'
    fn concatenation_list(&mut self, arg: &ConcatenationList) {
        self.concatenation_item(&arg.concatenation_item);
        for x in &arg.concatenation_list_list {
            // Fill mode: each ",<sep>item" segment lives in its own
            // group so it can stay flat next to the previous item even
            // when the outer group is in break mode.
            self.group_begin();
            self.comma(&x.comma);
            self.soft_line();
            self.concatenation_item(&x.concatenation_item);
            self.group_end();
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
            self.group_begin();
            self.comma(&x.comma);
            self.soft_line();
            self.array_literal_item(&x.array_literal_item);
            self.group_end();
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

    /// Semantic action for non-terminal 'CaseExpression'
    fn case_expression(&mut self, arg: &CaseExpression) {
        self.case(&arg.case);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.align_reset();
        // Auto-finish would split the EXPRESSION group on source-line
        // gaps, breaking idempotency (pass 2 sees merged keys). Wide
        // arms are isolated explicitly via `aligned_case_arm` instead.
        if self.mode == Mode::Align {
            self.aligner.disable_auto_finish_for(align_kind::EXPRESSION);
        }
        let estimate = estimated_case_condition_width(&arg.case_condition);
        self.aligned_case_arm(estimate, |this| this.case_condition(&arg.case_condition));
        self.colon(&arg.colon);
        self.space(1);
        self.expression(&arg.expression0);
        self.comma(&arg.comma);
        self.newline();
        for x in &arg.case_expression_list {
            let estimate = estimated_case_condition_width(&x.case_condition);
            self.aligned_case_arm(estimate, |this| this.case_condition(&x.case_condition));
            self.colon(&x.colon);
            self.space(1);
            self.expression(&x.expression);
            self.comma(&x.comma);
            self.newline();
        }
        self.align_start(align_kind::EXPRESSION);
        self.defaul(&arg.defaul);
        self.align_finish(align_kind::EXPRESSION);
        if self.mode == Mode::Align {
            self.aligner.enable_auto_finish_for(align_kind::EXPRESSION);
        }
        self.colon(&arg.colon0);
        self.space(1);
        self.expression(&arg.expression1);
        if let Some(ref x) = arg.case_expression_opt {
            self.comma(&x.comma);
        } else {
            self.str(",");
        }
        // Prevent alignment leak into a sibling case/switch.
        self.align_reset();
        self.newline_pop();
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'SwitchExpression'
    fn switch_expression(&mut self, arg: &SwitchExpression) {
        self.switch(&arg.switch);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.align_reset();
        // See `case_expression`.
        if self.mode == Mode::Align {
            self.aligner.disable_auto_finish_for(align_kind::EXPRESSION);
        }
        let estimate = estimated_switch_condition_width(&arg.switch_condition);
        self.aligned_case_arm(estimate, |this| {
            this.switch_condition(&arg.switch_condition)
        });
        self.colon(&arg.colon);
        self.space(1);
        self.expression(&arg.expression);
        self.comma(&arg.comma);
        self.newline();
        for x in &arg.switch_expression_list {
            let estimate = estimated_switch_condition_width(&x.switch_condition);
            self.aligned_case_arm(estimate, |this| this.switch_condition(&x.switch_condition));
            self.colon(&x.colon);
            self.space(1);
            self.expression(&x.expression);
            self.comma(&x.comma);
            self.newline();
        }
        self.align_start(align_kind::EXPRESSION);
        self.defaul(&arg.defaul);
        self.align_finish(align_kind::EXPRESSION);
        if self.mode == Mode::Align {
            self.aligner.enable_auto_finish_for(align_kind::EXPRESSION);
        }
        self.colon(&arg.colon0);
        self.space(1);
        self.expression(&arg.expression0);
        if let Some(ref x) = arg.switch_expression_opt {
            self.comma(&x.comma);
        }
        // See `case_expression`.
        self.align_reset();
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
        if matches!(self.mode, Mode::Emit) {
            self.group_begin();
            self.l_brace(&arg.l_brace);
            self.group_nest_begin();
            self.soft_break();
            self.range_list(&arg.range_list);
            self.group_nest_end();
            self.soft_break();
            self.r_brace(&arg.r_brace);
            self.group_end();
        } else {
            self.l_brace(&arg.l_brace);
            self.range_list(&arg.range_list);
            self.r_brace(&arg.r_brace);
        }
    }

    /// Semantic action for non-terminal 'OutsideExpression'
    fn outside_expression(&mut self, arg: &OutsideExpression) {
        self.outside(&arg.outside);
        self.space(1);
        self.expression(&arg.expression);
        self.space(1);
        if matches!(self.mode, Mode::Emit) {
            self.group_begin();
            self.l_brace(&arg.l_brace);
            self.group_nest_begin();
            self.soft_break();
            self.range_list(&arg.range_list);
            self.group_nest_end();
            self.soft_break();
            self.r_brace(&arg.r_brace);
            self.group_end();
        } else {
            self.l_brace(&arg.l_brace);
            self.range_list(&arg.range_list);
            self.r_brace(&arg.r_brace);
        }
    }

    /// Semantic action for non-terminal 'RangeList'
    fn range_list(&mut self, arg: &RangeList) {
        let in_build = matches!(self.mode, Mode::Emit);
        self.range_item(&arg.range_item);
        for x in &arg.range_list_list {
            if in_build {
                // Fill mode: each ",<sep>item" segment is its own group so a
                // long range list can wrap based on max_width while keeping
                // user-grouped items together.
                self.group_begin();
                self.comma(&x.comma);
                self.soft_line();
                self.range_item(&x.range_item);
                self.group_end();
            } else {
                self.comma(&x.comma);
                self.space(1);
                self.range_item(&x.range_item);
            }
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
        self.align_start(align_kind::VAR_KEYWORD);
        self.r#let(&arg.r#let);
        self.align_finish(align_kind::VAR_KEYWORD);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        if let Some(ref x) = arg.let_statement_opt {
            self.colon(&x.colon);
            self.space(1);
            if let Some(ref y) = x.let_statement_opt0 {
                self.align_start(align_kind::CLOCK_DOMAIN);
                self.clock_domain(&y.clock_domain);
                self.space(1);
                self.align_finish(align_kind::CLOCK_DOMAIN);
            } else {
                self.align_start(align_kind::CLOCK_DOMAIN);
                self.align_dummy_token(align_kind::CLOCK_DOMAIN, &x.colon.colon_token);
                self.align_finish(align_kind::CLOCK_DOMAIN);
            }
            self.array_type(&x.array_type);
        }
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

    /// Semantic action for non-terminal 'ConcatenationAssignment'
    fn concatenation_assignment(&mut self, arg: &ConcatenationAssignment) {
        self.align_start(align_kind::IDENTIFIER);
        self.l_brace(&arg.l_brace);
        self.assign_concatenation_list(&arg.assign_concatenation_list);
        self.r_brace(&arg.r_brace);
        self.align_finish(align_kind::IDENTIFIER);
        self.space(1);
        self.align_start(align_kind::ASSIGNMENT);
        self.equ(&arg.equ);
        self.align_finish(align_kind::ASSIGNMENT);
        self.space(1);
        self.expression(&arg.expression);
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
            self.align_note_statement_end();
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
            StatementBlockGroupGroup::BlockLBraceStatementBlockGroupGroupListRBrace(x) => {
                self.block(&x.block);
                self.space(1);
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
        self.space(1);
        self.r#in(&arg.r#in);
        self.space(1);
        if let Some(ref x) = arg.for_statement_opt {
            self.rev(&x.rev);
            self.space(1);
        }
        self.range(&arg.range);
        self.space(1);
        if let Some(ref x) = arg.for_statement_opt0 {
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
        // See `case_expression`. Per-block separation (formerly from
        // source-line-gap auto-finish) is preserved by the explicit
        // `align_reset()` after each statement-block body in `case_item`.
        if self.mode == Mode::Align {
            self.aligner.disable_auto_finish_for(align_kind::EXPRESSION);
        }
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.case_item(&x.case_item);
        }
        if self.mode == Mode::Align {
            self.aligner.enable_auto_finish_for(align_kind::EXPRESSION);
        }
        self.newline_list_post(
            arg.case_statement_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'CaseItem'
    fn case_item(&mut self, arg: &CaseItem) {
        let estimate = match &*arg.case_item_group {
            CaseItemGroup::CaseCondition(x) => estimated_case_condition_width(&x.case_condition),
            CaseItemGroup::Defaul(_) => 0,
        };
        self.aligned_case_arm(estimate, |this| match &*arg.case_item_group {
            CaseItemGroup::CaseCondition(x) => this.case_condition(&x.case_condition),
            CaseItemGroup::Defaul(x) => this.defaul(&x.defaul),
        });
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.case_item_group0 {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::StatementBlock(x) => {
                self.statement_block(&x.statement_block);
                // Section break after a block body so short pre-block
                // arms don't pad to post-block arms' column.
                self.align_reset();
            }
        }
    }

    /// Semantic action for non-terminal 'CaseCondition'
    fn case_condition(&mut self, arg: &CaseCondition) {
        // Fill mode: each `, key` segment in its own group so a long
        // multi-key condition fills lines up to `max_width` rather
        // than dropping one key per line.
        self.range_item(&arg.range_item);
        for x in &arg.case_condition_list {
            self.group_begin();
            self.comma(&x.comma);
            self.soft_line();
            self.range_item(&x.range_item);
            self.group_end();
        }
    }

    /// Semantic action for non-terminal 'SwitchStatement'
    fn switch_statement(&mut self, arg: &SwitchStatement) {
        self.switch(&arg.switch);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.align_reset();
        // See `case_statement`.
        if self.mode == Mode::Align {
            self.aligner.disable_auto_finish_for(align_kind::EXPRESSION);
        }
        for (i, x) in arg.switch_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.switch_item(&x.switch_item);
        }
        if self.mode == Mode::Align {
            self.aligner.enable_auto_finish_for(align_kind::EXPRESSION);
        }
        self.newline_list_post(
            arg.switch_statement_list.is_empty(),
            &arg.l_brace.l_brace_token,
        );
        self.r_brace(&arg.r_brace);
    }

    /// Semantic action for non-terminal 'SwitchItem'
    fn switch_item(&mut self, arg: &SwitchItem) {
        // See `case_item`.
        let estimate = match &*arg.switch_item_group {
            SwitchItemGroup::SwitchCondition(x) => {
                estimated_switch_condition_width(&x.switch_condition)
            }
            SwitchItemGroup::Defaul(_) => 0,
        };
        self.aligned_case_arm(estimate, |this| match &*arg.switch_item_group {
            SwitchItemGroup::SwitchCondition(x) => this.switch_condition(&x.switch_condition),
            SwitchItemGroup::Defaul(x) => this.defaul(&x.defaul),
        });
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.switch_item_group0 {
            SwitchItemGroup0::Statement(x) => self.statement(&x.statement),
            SwitchItemGroup0::StatementBlock(x) => {
                self.statement_block(&x.statement_block);
                // See `case_item`.
                self.align_reset();
            }
        }
    }

    /// Semantic action for non-terminal 'SwitchCondition'
    fn switch_condition(&mut self, arg: &SwitchCondition) {
        // See `case_condition`.
        self.expression(&arg.expression);
        for x in &arg.switch_condition_list {
            self.group_begin();
            self.comma(&x.comma);
            self.soft_line();
            self.expression(&x.expression);
            self.group_end();
        }
    }

    /// Semantic action for non-terminal 'Attribute'
    fn attribute(&mut self, arg: &Attribute) {
        self.in_attribute = true;
        self.hash_l_bracket(&arg.hash_l_bracket);
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
        self.align_start(align_kind::VAR_KEYWORD);
        self.r#let(&arg.r#let);
        self.align_finish(align_kind::VAR_KEYWORD);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        if let Some(ref x) = arg.let_declaration_opt {
            self.colon(&x.colon);
            self.space(1);
            if let Some(ref y) = x.let_declaration_opt0 {
                self.align_start(align_kind::CLOCK_DOMAIN);
                self.clock_domain(&y.clock_domain);
                self.space(1);
                self.align_finish(align_kind::CLOCK_DOMAIN);
            } else {
                self.align_start(align_kind::CLOCK_DOMAIN);
                self.align_dummy_token(align_kind::CLOCK_DOMAIN, &x.colon.colon_token);
                self.align_finish(align_kind::CLOCK_DOMAIN);
            }
            self.array_type(&x.array_type);
        }
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'VarDeclaration'
    fn var_declaration(&mut self, arg: &VarDeclaration) {
        self.align_start(align_kind::VAR_KEYWORD);
        self.var(&arg.var);
        self.align_finish(align_kind::VAR_KEYWORD);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        if let Some(ref x) = arg.var_declaration_opt {
            self.colon(&x.colon);
            self.space(1);
            if let Some(ref y) = x.var_declaration_opt0 {
                self.align_start(align_kind::CLOCK_DOMAIN);
                self.clock_domain(&y.clock_domain);
                self.space(1);
                self.align_finish(align_kind::CLOCK_DOMAIN);
            } else {
                self.align_start(align_kind::CLOCK_DOMAIN);
                self.align_dummy_token(align_kind::CLOCK_DOMAIN, &x.colon.colon_token);
                self.align_finish(align_kind::CLOCK_DOMAIN);
            }
            self.array_type(&x.array_type);
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ConstDeclaration'
    fn const_declaration(&mut self, arg: &ConstDeclaration) {
        self.align_start(align_kind::VAR_KEYWORD);
        self.r#const(&arg.r#const);
        self.align_finish(align_kind::VAR_KEYWORD);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        if let Some(ref opt) = arg.const_declaration_opt {
            self.colon(&opt.colon);
            self.space(1);
            match &*opt.const_declaration_opt_group {
                ConstDeclarationOptGroup::ArrayType(x) => {
                    self.array_type(&x.array_type);
                }
                ConstDeclarationOptGroup::Type(x) => {
                    self.align_start(align_kind::TYPE);
                    self.r#type(&x.r#type);
                    self.align_finish(align_kind::TYPE);
                }
            }
        }
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'GenDeclaration'
    fn gen_declaration(&mut self, arg: &GenDeclaration) {
        self.align_start(align_kind::VAR_KEYWORD);
        self.r#gen(&arg.r#gen);
        self.align_finish(align_kind::VAR_KEYWORD);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        self.align_start(align_kind::TYPE);
        match &*arg.gen_declaration_group {
            GenDeclarationGroup::Type(x) => self.r#type(&x.r#type),
            GenDeclarationGroup::GenericProtoBound(x) => match &*x.generic_proto_bound {
                GenericProtoBound::ScopedIdentifier(x) => {
                    self.scoped_identifier(&x.scoped_identifier)
                }
                GenericProtoBound::FixedType(x) => self.fixed_type(&x.fixed_type),
            },
        }
        self.align_finish(align_kind::TYPE);
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
                self.align_start(align_kind::ASSIGN_DECL_IDENTIFIER);
                self.hierarchical_identifier(&x.hierarchical_identifier);
                self.align_finish(align_kind::ASSIGN_DECL_IDENTIFIER);
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
        self.align_start(align_kind::ASSIGN_DECL_IDENTIFIER);
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        self.align_finish(align_kind::ASSIGN_DECL_IDENTIFIER);
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
        self.align_note_statement_end();
        for x in &arg.modport_list_list {
            self.comma(&x.comma);
            self.newline();
            self.modport_group(&x.modport_group);
            self.align_note_statement_end();
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

    /// Semantic action for non-terminal 'ModportDefaultList'
    fn modport_default_list(&mut self, arg: &ModportDefaultList) {
        self.identifier(&arg.identifier);
        for x in &arg.modport_default_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.identifier(&x.identifier);
        }
        if let Some(ref x) = arg.modport_default_list_opt {
            self.comma(&x.comma);
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
        self.align_note_statement_end();
        for x in &arg.enum_list_list {
            self.comma(&x.comma);
            self.newline();
            self.enum_group(&x.enum_group);
            self.align_note_statement_end();
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
        self.align_note_statement_end();
        for x in &arg.struct_union_list_list {
            self.comma(&x.comma);
            self.newline();
            self.struct_union_group(&x.struct_union_group);
            self.align_note_statement_end();
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
        self.inst(&arg.inst);
        self.space(1);
        self.format_inst(&arg.component_instantiation, &arg.semicolon);
    }

    /// Semantic action for non-terminal 'BindDeclaration'
    fn bind_declaration(&mut self, arg: &BindDeclaration) {
        self.bind(&arg.bind);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        self.space(1);
        self.l_t_minus(&arg.l_t_minus);
        self.space(1);
        self.format_inst(&arg.component_instantiation, &arg.semicolon);
    }

    /// Semantic action for non-terminal 'InstParameter'
    fn inst_parameter(&mut self, arg: &InstParameter) {
        if let Some(ref x) = arg.inst_parameter_opt {
            self.hash(&arg.hash);
            self.token_will_push(&arg.l_paren.l_paren_token);
            self.group_nest_begin();
            self.soft_line();
            self.inst_parameter_list(&x.inst_parameter_list);
            self.group_nest_end();
            self.soft_line();
            self.r_paren(&arg.r_paren);
        }
    }

    /// Semantic action for non-terminal 'InstParameterList'
    fn inst_parameter_list(&mut self, arg: &InstParameterList) {
        // No `align_note_statement_end` between items: parameters
        // are not statement boundaries, and firing it would reset
        // the outer cross-inst alignment's `had_item_in_statement`.
        self.inst_parameter_group(&arg.inst_parameter_group);
        for x in &arg.inst_parameter_list_list {
            self.comma(&x.comma);
            self.soft_line();
            self.inst_parameter_group(&x.inst_parameter_group);
        }
        self.emit_doc(doc::if_break(","));
        if let Some(ref x) = arg.inst_parameter_list_opt {
            self.emit_trailing_comments(&x.comma.comma_token, false);
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
        let body = arg
            .inst_parameter_item_opt
            .as_ref()
            .map(|x| (&*x.colon, &*x.expression));
        self.emit_inst_item(&arg.identifier, body);
    }

    /// Semantic action for non-terminal 'InstPortList'
    fn inst_port_list(&mut self, arg: &InstPortList) {
        self.inst_port_group(&arg.inst_port_group);
        for x in &arg.inst_port_list_list {
            self.comma(&x.comma);
            self.soft_line();
            self.inst_port_group(&x.inst_port_group);
        }
        self.emit_doc(doc::if_break(","));
        if let Some(ref x) = arg.inst_port_list_opt {
            self.emit_trailing_comments(&x.comma.comma_token, false);
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
        let body = arg
            .inst_port_item_opt
            .as_ref()
            .map(|x| (&*x.colon, &*x.expression));
        self.emit_inst_item(&arg.identifier, body);
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
        self.align_note_statement_end();
        for x in &arg.with_parameter_list_list {
            self.comma(&x.comma);
            self.newline();
            self.with_parameter_group(&x.with_parameter_group);
            self.align_note_statement_end();
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

        if let Some(ref x) = arg.with_parameter_item_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.align_start(align_kind::EXPRESSION);
            self.expression(&x.expression);
            self.align_finish(align_kind::EXPRESSION);
        }
    }

    /// Semantic action for non-terminal 'WithGenericParameter'
    fn with_generic_parameter(&mut self, arg: &WithGenericParameter) {
        // Width-driven layout: the outer Group decides flat vs.
        // stacked based on `max_width`, with no dependency on the
        // source's line layout.
        self.group_begin();
        self.colon_colon_l_angle(&arg.colon_colon_l_angle);
        self.group_nest_begin();
        self.soft_break();
        self.with_generic_parameter_list(&arg.with_generic_parameter_list);
        self.group_nest_end();
        self.soft_break();
        self.r_angle(&arg.r_angle);
        self.group_end();
    }

    /// Semantic action for non-terminal 'WithGenericParameterList'
    fn with_generic_parameter_list(&mut self, arg: &WithGenericParameterList) {
        // Stacked-or-flat: no per-item sub-group. The outer
        // `with_generic_parameter` group's break decision is shared by
        // every item, so the list either fits on one line in full or
        // wraps with one item per line — never a mix. That keeps the
        // alignment columns visually meaningful.
        self.with_generic_parameter_item(&arg.with_generic_parameter_item);
        for x in &arg.with_generic_parameter_list_list {
            self.comma(&x.comma);
            self.soft_line();
            self.with_generic_parameter_item(&x.with_generic_parameter_item);
        }
        self.emit_doc(doc::if_break(","));
        if let Some(ref x) = arg.with_generic_parameter_list_opt {
            self.emit_trailing_comments(&x.comma.comma_token, false);
        }
    }

    /// Semantic action for non-terminal 'WithGenericParameterItem'
    fn with_generic_parameter_item(&mut self, arg: &WithGenericParameterItem) {
        // `GENERIC_*` kinds keep the column groups separate from a
        // surrounding declaration's IDENTIFIER/TYPE/EXPRESSION.
        self.align_start_break_gated(align_kind::GENERIC_IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::GENERIC_IDENTIFIER);
        self.colon(&arg.colon);
        self.space(1);
        self.align_start_break_gated(align_kind::GENERIC_TYPE);
        self.generic_bound(&arg.generic_bound);
        self.align_finish(align_kind::GENERIC_TYPE);
        if let Some(ref x) = arg.with_generic_parameter_item_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.align_start_break_gated(align_kind::GENERIC_EXPRESSION);
            self.with_generic_argument_item(&x.with_generic_argument_item);
            self.align_finish(align_kind::GENERIC_EXPRESSION);
        }
    }

    /// Semantic action for non-terminal 'WithGenericArgument'
    fn with_generic_argument(&mut self, arg: &WithGenericArgument) {
        // Width-driven layout: see `with_generic_parameter`.
        self.group_begin();
        self.colon_colon_l_angle(&arg.colon_colon_l_angle);
        if let Some(x) = &arg.with_generic_argument_opt {
            self.group_nest_begin();
            self.soft_break();
            self.with_generic_argument_list(&x.with_generic_argument_list);
            self.group_nest_end();
            self.soft_break();
        }
        self.r_angle(&arg.r_angle);
        self.group_end();
    }

    /// Semantic action for non-terminal 'WithGenericArgumentList'
    fn with_generic_argument_list(&mut self, arg: &WithGenericArgumentList) {
        // Stacked-or-flat (see `with_generic_parameter_list`).
        self.with_generic_argument_item(&arg.with_generic_argument_item);
        for x in &arg.with_generic_argument_list_list {
            self.comma(&x.comma);
            self.soft_line();
            self.with_generic_argument_item(&x.with_generic_argument_item);
        }
        self.emit_doc(doc::if_break(","));
        if let Some(ref x) = arg.with_generic_argument_list_opt {
            self.emit_trailing_comments(&x.comma.comma_token, false);
        }
    }

    /// Semantic action for non-terminal 'WithGenericArgumentItem'
    fn with_generic_argument_item(&mut self, arg: &WithGenericArgumentItem) {
        // See `with_generic_parameter_item`: use a dedicated align kind
        // so the generic argument list doesn't bleed into the outer
        // EXPRESSION column group.
        self.align_start_break_gated(align_kind::GENERIC_EXPRESSION);
        match arg {
            WithGenericArgumentItem::GenericArgIdentifier(x) => {
                self.generic_arg_identifier(&x.generic_arg_identifier);
            }
            WithGenericArgumentItem::FixedType(x) => {
                self.fixed_type(&x.fixed_type);
            }
            WithGenericArgumentItem::Number(x) => {
                self.number(&x.number);
            }
            WithGenericArgumentItem::BooleanLiteral(x) => {
                self.boolean_literal(&x.boolean_literal);
            }
        }
        self.align_finish(align_kind::GENERIC_EXPRESSION);
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
        self.align_note_statement_end();
        for x in &arg.port_declaration_list_list {
            self.comma(&x.comma);
            self.newline();
            self.port_declaration_group(&x.port_declaration_group);
            self.align_note_statement_end();
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
            self.align_note_statement_end();
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
                    self.align_note_statement_end();
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
            self.align_note_statement_end();
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
                    self.align_note_statement_end();
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
        if let Some(ref x) = arg.generate_for_declaration_opt {
            self.rev(&x.rev);
            self.space(1);
        }
        self.range(&arg.range);
        self.space(1);
        if let Some(ref x) = arg.generate_for_declaration_opt0 {
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
            self.align_note_statement_end();
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
            self.align_note_statement_end();
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
                    self.align_note_statement_end();
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
            self.align_note_statement_end();
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
                    self.align_note_statement_end();
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
        if let Some(x) = &arg.proto_type_def_declaration_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.array_type(&x.array_type);
        }
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
        self.triple_l_brace(&arg.triple_l_brace);
        self.keep_tail_newline = true;
        // Treat the embedded payload as opaque target-language source;
        // re-formatting it would mangle constructs like `\{ Module::<T> \}`
        // that re-parse as Veryl generics inside the macro body.
        self.unformat_embed_items(arg);
        self.keep_tail_newline = false;
        let last_ws = match self.mode {
            Mode::Emit => self
                .last_emitted_char
                .map(|c| c.is_ascii_whitespace())
                .unwrap_or(false),
            _ => self
                .string
                .chars()
                .last()
                .map(|c| c.is_ascii_whitespace())
                .unwrap_or(false),
        };
        if !last_ws {
            self.newline();
        }
        self.triple_r_brace(&arg.triple_r_brace);
    }

    /// Semantic action for non-terminal 'EmbedScopedIdentifier'
    fn embed_scoped_identifier(&mut self, arg: &EmbedScopedIdentifier) {
        self.escaped_l_brace(&arg.escaped_l_brace);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        self.space(1);
        self.escaped_r_brace(&arg.escaped_r_brace);
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
        if !skip_formatting(arg) {
            match arg {
                DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) => {
                    if let Some(ref x) = x.description_item_opt {
                        self.r#pub(&x.r#pub);
                        self.space(1);
                    }
                    self.public_description_item(&x.public_description_item);
                }
                DescriptionItem::ImportDeclaration(x) => {
                    self.import_declaration(&x.import_declaration)
                }
                DescriptionItem::BindDeclaration(x) => {
                    self.bind_declaration(&x.bind_declaration);
                }
                DescriptionItem::EmbedDeclaration(x) => {
                    self.embed_declaration(&x.embed_declaration)
                }
                DescriptionItem::IncludeDeclaration(x) => {
                    self.include_declaration(&x.include_declaration)
                }
            };
        } else {
            self.unformat_description_item(arg);
        }
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        self.start(&arg.start);
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

fn skip_formatting(arg: &DescriptionItem) -> bool {
    let Some(identifier) = arg.identifier_token() else {
        return false;
    };
    attribute_table::is_format(&identifier.token, FormatItem::Skip)
}

/// Rendered-width estimate (source-layout-independent) used by
/// `aligned_case_arm` to decide isolation before emitting to the
/// aligner.
fn estimated_case_condition_width(arg: &CaseCondition) -> u32 {
    let mut collector = TokenCollector::new(false);
    collector.case_condition(arg);
    estimated_token_width(&collector.tokens)
}

fn estimated_switch_condition_width(arg: &SwitchCondition) -> u32 {
    let mut collector = TokenCollector::new(false);
    collector.switch_condition(arg);
    estimated_token_width(&collector.tokens)
}

fn estimated_expression_width(arg: &Expression) -> u32 {
    let mut collector = TokenCollector::new(false);
    collector.expression(arg);
    estimated_token_width(&collector.tokens)
}

fn estimated_token_width(tokens: &[Token]) -> u32 {
    let mut total: u32 = tokens.iter().map(|t| t.length).sum();
    for pair in tokens.windows(2) {
        let (curr, next) = (&pair[0], &pair[1]);
        let gap = if curr.line == next.line {
            next.column.saturating_sub(curr.column + curr.length)
        } else {
            // Cross-line tokens collapse to a single space when joined.
            1
        };
        total += gap;
    }
    total
}
