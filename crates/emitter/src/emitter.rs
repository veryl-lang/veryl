use crate::expaneded_modport::{ExpandModportConnectionsTable, ExpandedModportPortTable};
use std::fs;
use std::path::Path;
use veryl_aligner::{Aligner, Location, Measure, align_kind};
use veryl_analyzer::attribute::Attribute as Attr;
use veryl_analyzer::attribute::{AlignItem, AllowItem, CondTypeItem, EnumEncodingItem, FormatItem};
use veryl_analyzer::attribute_table;
use veryl_analyzer::connect_operation_table;
use veryl_analyzer::evaluator::{EvaluatedTypeResetKind, Evaluator};
use veryl_analyzer::literal::{Literal, TypeLiteral};
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::Direction as SymDirection;
use veryl_analyzer::symbol::TypeModifierKind as SymTypeModifierKind;
use veryl_analyzer::symbol::{
    GenericMap, GenericTables, Port, Symbol, SymbolId, SymbolKind, TypeKind, VariableAffiliation,
};
use veryl_analyzer::symbol_path::{GenericSymbolPath, SymbolPath};
use veryl_analyzer::symbol_table::{self, ResolveError, ResolveResult};
use veryl_analyzer::{msb_table, namespace_table};
use veryl_metadata::{Build, BuiltinType, ClockType, Format, Metadata, ResetType, SourceMapTarget};
use veryl_parser::Stringifier;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenExt;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenSource, VerylToken, is_anonymous_token};
use veryl_parser::veryl_walker::VerylWalker;
use veryl_sourcemap::SourceMap;

#[cfg(target_os = "windows")]
const NEWLINE: &str = "\r\n";
#[cfg(not(target_os = "windows"))]
const NEWLINE: &str = "\n";

pub enum AttributeType {
    Ifdef,
    Sv,
    Test,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Emit,
    Align,
}

pub struct Emitter {
    mode: Mode,
    project_name: Option<StrId>,
    build_opt: Build,
    format_opt: Format,
    string: String,
    indent: usize,
    src_line: u32,
    dst_line: u32,
    dst_column: u32,
    aligner: Aligner,
    measure: Measure,
    force_duplicated: bool,
    in_start_token: bool,
    consumed_next_newline: bool,
    single_line: Vec<()>,
    multi_line: Vec<()>,
    adjust_line: bool,
    keep_tail_newline: bool,
    in_always_ff: bool,
    in_direction_modport: bool,
    in_direction_with_var: bool,
    in_import: bool,
    in_scalar_type: bool,
    in_expression: Vec<()>,
    in_attribute: bool,
    in_named_argument: Vec<bool>,
    in_generate_block: Vec<()>,
    signed: bool,
    default_clock: Option<SymbolId>,
    default_reset: Option<SymbolId>,
    reset_signal: Option<VerylToken>,
    reset_active_low: bool,
    default_block: Option<VerylToken>,
    enum_width: usize,
    enum_type: Option<ScalarType>,
    emit_enum_implicit_valiant: bool,
    file_scope_import: Vec<ImportDeclaration>,
    attribute: Vec<AttributeType>,
    assignment_lefthand_side: Option<ExpressionIdentifier>,
    generic_map: Vec<Vec<GenericMap>>,
    source_map: Option<SourceMap>,
    resolved_identifier: Vec<String>,
    last_token: Option<VerylToken>,
    duplicated_index: usize,
    modport_connections_tables: Vec<ExpandModportConnectionsTable>,
    modport_ports_table: Option<ExpandedModportPortTable>,
    inst_module_namespace: Option<Namespace>,
    skip_comment: bool,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            mode: Mode::Emit,
            project_name: None,
            build_opt: Build::default(),
            format_opt: Format::default(),
            string: String::new(),
            force_duplicated: false,
            indent: 0,
            src_line: 1,
            dst_line: 1,
            dst_column: 1,
            aligner: Aligner::new(),
            measure: Measure::default(),
            in_start_token: false,
            consumed_next_newline: false,
            single_line: Vec::new(),
            multi_line: Vec::new(),
            adjust_line: false,
            keep_tail_newline: false,
            in_always_ff: false,
            in_direction_modport: false,
            in_direction_with_var: false,
            in_import: false,
            in_scalar_type: false,
            in_expression: Vec::new(),
            in_attribute: false,
            in_named_argument: Vec::new(),
            in_generate_block: Vec::new(),
            signed: false,
            default_clock: None,
            default_reset: None,
            reset_signal: None,
            reset_active_low: false,
            default_block: None,
            enum_width: 0,
            enum_type: None,
            emit_enum_implicit_valiant: false,
            file_scope_import: Vec::new(),
            attribute: Vec::new(),
            assignment_lefthand_side: None,
            generic_map: Vec::new(),
            source_map: None,
            resolved_identifier: Vec::new(),
            last_token: None,
            duplicated_index: 0,
            modport_connections_tables: Vec::new(),
            modport_ports_table: None,
            inst_module_namespace: None,
            skip_comment: false,
        }
    }
}

fn is_ifdef_attribute(arg: &Attribute) -> bool {
    matches!(
        arg.identifier.identifier_token.token.to_string().as_str(),
        "ifdef" | "ifndef"
    )
}

impl Emitter {
    pub fn new(metadata: &Metadata, src_path: &Path, dst_path: &Path, map_path: &Path) -> Self {
        let source_map = SourceMap::new(src_path, dst_path, map_path);

        Self {
            project_name: Some(metadata.project.name.as_str().into()),
            build_opt: metadata.build.clone(),
            format_opt: metadata.format.clone(),
            aligner: Aligner::new(),
            source_map: Some(source_map),
            ..Default::default()
        }
    }

    pub fn emit(&mut self, project_name: &str, input: &Veryl) {
        namespace_table::set_default(&[project_name.into()]);
        if self.format_opt.vertical_align {
            self.mode = Mode::Align;
            self.duplicated_index = 0;
            self.veryl(input);
            self.aligner.finish_group();
            self.aligner.gather_additions();
        }
        self.mode = Mode::Emit;
        self.duplicated_index = 0;
        self.veryl(input);
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    pub fn source_map(&mut self) -> &mut SourceMap {
        self.source_map.as_mut().unwrap()
    }

    fn str(&mut self, x: &str) {
        match self.mode {
            Mode::Emit => {
                self.string.push_str(x);

                let new_lines = x.matches('\n').count() as u32;
                self.dst_line += new_lines;
                if new_lines == 0 {
                    self.dst_column += x.len() as u32;
                } else {
                    self.dst_column = (x.len() - x.rfind('\n').unwrap_or(0)) as u32;
                }
            }
            Mode::Align => {
                self.aligner.space(x.len());
                self.measure.add(x.len() as u32);
            }
        }
    }

    fn truncate(&mut self, x: usize) {
        if self.mode == Mode::Align {
            return;
        }

        let removed = self.string.split_off(x);

        let removed_lines = removed.matches('\n').count() as u32;
        if removed_lines == 0 {
            self.dst_column -= removed.len() as u32;
        } else {
            self.dst_line -= removed_lines;
            self.dst_column = (self.string.len() - self.string.rfind('\n').unwrap_or(0)) as u32;
        }
    }

    fn unindent(&mut self) {
        if self.mode == Mode::Align {
            return;
        }

        let indent_width = self.indent * self.format_opt.indent_width;
        if self.string.ends_with(&" ".repeat(indent_width)) {
            self.truncate(self.string.len() - indent_width);
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

    fn newline_list_post(&mut self, is_empty: bool) {
        if !is_empty {
            self.newline_pop();
        } else {
            self.newline();
        }
    }

    fn space(&mut self, repeat: usize) {
        self.str(&" ".repeat(repeat));
    }

    fn consume_adjust_line(&mut self, x: &Token) {
        if self.adjust_line && x.line > self.src_line + 1 {
            self.newline();
        }
        self.adjust_line = false;
    }

    fn clear_adjust_line(&mut self) {
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

        if x.line != 0
            && x.column != 0
            && let Some(ref mut map) = self.source_map
        {
            map.add(self.dst_line, self.dst_column, x.line, x.column, text);
        }

        let newlines_in_text = text.matches('\n').count() as u32;
        self.str(text);
        self.src_line = x.line + newlines_in_text;
    }

    fn process_token(&mut self, x: &VerylToken, will_push: bool, duplicated: Option<usize>) {
        match self.mode {
            Mode::Emit => {
                self.push_token(&x.token);

                let mut loc: Location = x.token.into();
                loc.duplicated = duplicated;
                if let Some(width) = self.aligner.additions.get(&loc) {
                    self.space(*width as usize);
                }

                // skip to emit comments
                if duplicated.is_some() || self.build_opt.strip_comments || self.skip_comment {
                    return;
                }

                self.process_comment(x, will_push);
            }
            Mode::Align => {
                self.aligner.token(x);
                self.measure.add(x.token.length);
            }
        }

        self.last_token = Some(x.clone());
    }

    fn process_comment(&mut self, x: &VerylToken, will_push: bool) {
        // temporary indent to adjust indent of comments with the next push
        if will_push {
            self.indent += 1;
        }
        // detect line comment newline which will consume the next newline
        self.consumed_next_newline = false;
        for x in &x.comments {
            // insert space between comments in the same line
            if x.line == self.src_line && !self.in_start_token {
                self.space(1);
            }
            for _ in 0..x.line - self.src_line {
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
        if self.force_duplicated {
            self.duplicated_token(x)
        } else {
            self.process_token(x, false, None)
        }
    }

    fn token_will_push(&mut self, x: &VerylToken) {
        let will_push = !self.single_line();
        self.process_token(x, will_push, None)
    }

    fn duplicated_token(&mut self, x: &VerylToken) {
        match self.mode {
            Mode::Align => {
                self.aligner.duplicated_token(x, self.duplicated_index);
            }
            Mode::Emit => {
                self.process_token(x, false, Some(self.duplicated_index));
            }
        }
        self.duplicated_index += 1;
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

    fn align_reset(&mut self) {
        if self.mode == Mode::Align {
            self.aligner.finish_item();
            self.aligner.finish_group();
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

    fn emit_scalar_type(&mut self, arg: &ScalarType, enable_align: bool) {
        self.in_scalar_type = true;

        // disable align
        if self.mode == Mode::Align && !enable_align {
            self.in_scalar_type = false;
            return;
        }

        self.align_start(align_kind::TYPE);
        if self.mode == Mode::Align {
            // dummy space for implicit type
            self.space(1);
        }
        if self.in_direction_with_var {
            self.str("var");
            self.space(1);
        }
        for x in &arg.scalar_type_list {
            self.type_modifier(&x.type_modifier);
        }
        match &*arg.scalar_type_group {
            ScalarTypeGroup::UserDefinedTypeScalarTypeOpt(x) => {
                self.user_defined_type(&x.user_defined_type);
                if self.signed {
                    self.space(1);
                    self.str("signed");
                    self.signed = false;
                }
                self.align_finish(align_kind::TYPE);
                self.align_start(align_kind::WIDTH);
                if let Some(ref x) = x.scalar_type_opt {
                    self.space(1);
                    self.width(&x.width);
                } else {
                    let loc = self.align_last_location(align_kind::TYPE);
                    self.align_dummy_location(align_kind::WIDTH, loc);
                }
            }
            ScalarTypeGroup::FactorType(x) => self.factor_type(&x.factor_type),
        }
        self.in_scalar_type = false;
        self.align_finish(align_kind::WIDTH);
    }

    fn emit_identifier(&mut self, arg: &Identifier, symbol: Option<&Symbol>) {
        let align = !self.align_any()
            && !self.in_attribute
            && attribute_table::is_align(&arg.first(), AlignItem::Identifier);
        if align {
            self.align_start(align_kind::IDENTIFIER);
        }

        let text = emitting_identifier_token(&arg.identifier_token, symbol);
        self.veryl_token(&text);
        self.push_resolved_identifier(&text.to_string());

        if align {
            self.align_finish(align_kind::IDENTIFIER);
        }
    }

    fn case_inside_statement(&mut self, arg: &CaseStatement) {
        let (prefix, force_last_item_default) = self.cond_type_prefix(&arg.case.case_token.token);
        self.token(&arg.case.case_token.append(&prefix, &None));
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(") inside"));
        let len = arg.case_statement_list.len();
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            let force_default = force_last_item_default & (i == (len - 1));
            self.newline_list(i);
            self.case_inside_item(&x.case_item, force_default);
        }
        self.newline_list_post(arg.case_statement_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("endcase"));
    }

    fn case_inside_item(&mut self, arg: &CaseItem, force_default: bool) {
        self.align_start(align_kind::EXPRESSION);
        match &*arg.case_item_group {
            CaseItemGroup::CaseCondition(x) => {
                if force_default {
                    self.str("default");
                } else {
                    self.range_item(&x.case_condition.range_item);
                    for x in &x.case_condition.case_condition_list {
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
            }
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&arg.colon);
        self.space(1);
        match arg.case_item_group0.as_ref() {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::StatementBlock(x) => {
                self.statement_block(&x.statement_block);
            }
        }
    }

    fn case_expaneded_statement(&mut self, arg: &CaseStatement) {
        let (prefix, force_last_item_default) = self.cond_type_prefix(&arg.case.case_token.token);
        self.token(&arg.case.case_token.append(&prefix, &None));
        self.space(1);
        self.str("(");
        self.str("1'b1");
        self.token_will_push(&arg.l_brace.l_brace_token.replace(")"));
        let len = arg.case_statement_list.len();
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            let force_default = force_last_item_default & (i == (len - 1));
            self.newline_list(i);
            self.case_expanded_item(&arg.expression, &x.case_item, force_default);
        }
        self.newline_list_post(arg.case_statement_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("endcase"));
    }

    fn case_expanded_item(&mut self, lhs: &Expression, item: &CaseItem, force_default: bool) {
        self.align_start(align_kind::EXPRESSION);
        match &*item.case_item_group {
            CaseItemGroup::CaseCondition(x) => {
                if force_default {
                    self.str("default");
                } else {
                    self.inside_element_operation(lhs, &x.case_condition.range_item);
                    for x in &x.case_condition.case_condition_list {
                        self.comma(&x.comma);
                        if x.comma.line() != x.range_item.line() {
                            self.newline();
                            self.align_finish(align_kind::EXPRESSION);
                            self.align_start(align_kind::EXPRESSION);
                        } else {
                            self.space(1);
                        }
                        self.inside_element_operation(lhs, &x.range_item);
                    }
                }
            }
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&item.colon);
        self.space(1);
        match item.case_item_group0.as_ref() {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::StatementBlock(x) => {
                self.statement_block(&x.statement_block);
            }
        }
    }

    fn case_expression_condition(&mut self, lhs: &Expression, rhs: &RangeItem) {
        if rhs.range.range_opt.is_some() && !self.build_opt.expand_inside_operation {
            self.str("(");
            self.expression(lhs);
            self.str(") inside {");
            self.range(&rhs.range);
            self.str("}");
        } else {
            self.inside_element_operation(lhs, rhs);
        }
    }

    fn inside_normal_expression(&mut self, arg: &InsideExpression) {
        self.str("((");
        self.expression(&arg.expression);
        self.str(")");
        self.token(&arg.inside.inside_token.replace(" inside "));
        self.l_brace(&arg.l_brace);
        self.range_list(&arg.range_list);
        self.r_brace(&arg.r_brace);
        self.str(")");
    }

    fn inside_expanded_expression(&mut self, arg: &InsideExpression) {
        self.str("(");
        self.inside_element_operation(&arg.expression, &arg.range_list.range_item);
        for x in &arg.range_list.range_list_list {
            self.str(" || ");
            self.inside_element_operation(&arg.expression, &x.range_item);
        }
        self.str(")");
    }

    fn outside_normal_expression(&mut self, arg: &OutsideExpression) {
        self.str("!((");
        self.expression(&arg.expression);
        self.str(")");
        self.token(&arg.outside.outside_token.replace(" inside "));
        self.l_brace(&arg.l_brace);
        self.range_list(&arg.range_list);
        self.r_brace(&arg.r_brace);
        self.str(")");
    }

    fn outside_expanded_expression(&mut self, arg: &OutsideExpression) {
        self.str("!(");
        self.inside_element_operation(&arg.expression, &arg.range_list.range_item);
        for x in &arg.range_list.range_list_list {
            self.str(" || ");
            self.inside_element_operation(&arg.expression, &x.range_item);
        }
        self.str(")");
    }

    fn inside_element_operation(&mut self, lhs: &Expression, rhs: &RangeItem) {
        if let Some(ref x) = rhs.range.range_opt {
            let (op_l, op_r) = match &*x.range_operator {
                RangeOperator::DotDot(_) => (">=", "<"),
                RangeOperator::DotDotEqu(_) => (">=", "<="),
            };
            self.str("((");
            self.expression(lhs);
            self.str(") ");
            self.str(op_l);
            self.str(" (");
            self.expression(&rhs.range.expression);
            self.str(")) && ((");
            self.expression(lhs);
            self.str(") ");
            self.str(op_r);
            self.str(" (");
            self.expression(&x.expression);
            self.str("))");
        } else {
            self.str("(");
            self.expression(lhs);
            self.str(") ==? (");
            self.expression(&rhs.range.expression);
            self.str(")");
        }
    }

    fn always_ff_explicit_event_list(
        &mut self,
        arg: &AlwaysFfEventList,
        decl: &AlwaysFfDeclaration,
    ) {
        self.l_paren(&arg.l_paren);
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.always_ff_event_list_opt {
            if self.always_ff_reset_exist_in_sensitivity_list(&x.always_ff_reset) {
                self.comma(&x.comma);
                self.space(1);
            }
            self.always_ff_reset(&x.always_ff_reset);
        } else if self.always_ff_if_reset_exists(decl) {
            self.always_ff_implicit_reset_event();
        }
        self.r_paren(&arg.r_paren);
    }

    fn always_ff_implicit_event_list(&mut self, arg: &AlwaysFfDeclaration) {
        self.str("(");
        self.always_ff_implicit_clock_event();
        if self.always_ff_if_reset_exists(arg) {
            self.always_ff_implicit_reset_event();
        }
        self.str(")")
    }

    fn always_ff_implicit_clock_event(&mut self) {
        let symbol = symbol_table::get(self.default_clock.unwrap()).unwrap();
        let (clock_kind, prefix, suffix) = match symbol.kind {
            SymbolKind::Port(x) => (x.r#type.kind, x.prefix.clone(), x.suffix.clone()),
            SymbolKind::Variable(x) => (x.r#type.kind, x.prefix.clone(), x.suffix.clone()),
            _ => unreachable!(),
        };
        let clock_type = match clock_kind {
            TypeKind::ClockPosedge => ClockType::PosEdge,
            TypeKind::ClockNegedge => ClockType::NegEdge,
            TypeKind::Clock => self.build_opt.clock_type,
            _ => unreachable!(),
        };

        match clock_type {
            ClockType::PosEdge => self.str("posedge"),
            ClockType::NegEdge => self.str("negedge"),
        }
        self.space(1);

        if prefix.is_some() || suffix.is_some() {
            let token = VerylToken::new(symbol.token).append(&prefix, &suffix);
            self.str(&token.token.to_string());
        } else {
            self.str(&symbol.token.to_string());
        }
    }

    fn always_ff_if_reset_exists(&mut self, arg: &AlwaysFfDeclaration) -> bool {
        if let Some(x) = arg.statement_block.statement_block_list.first() {
            let x: Vec<_> = x.statement_block_group.as_ref().into();
            if let Some(StatementBlockItem::Statement(x)) = x.first() {
                matches!(*x.statement, Statement::IfResetStatement(_))
            } else {
                false
            }
        } else {
            false
        }
    }

    fn always_ff_implicit_reset_event(&mut self) {
        let symbol = symbol_table::get(self.default_reset.unwrap()).unwrap();
        let reset_type = get_variable_type_kind(&symbol)
            .map(|x| match x {
                TypeKind::ResetAsyncHigh => ResetType::AsyncHigh,
                TypeKind::ResetAsyncLow => ResetType::AsyncLow,
                TypeKind::ResetSyncHigh => ResetType::SyncHigh,
                TypeKind::ResetSyncLow => ResetType::SyncLow,
                TypeKind::Reset => self.build_opt.reset_type,
                _ => unreachable!(),
            })
            .unwrap();

        let token = emitting_identifier_token(&VerylToken::new(symbol.token), Some(&symbol));

        match reset_type {
            ResetType::AsyncHigh => {
                self.str(",");
                self.space(1);
                self.str("posedge");
                self.space(1);
                self.duplicated_token(&token);
            }
            ResetType::AsyncLow => {
                self.str(",");
                self.space(1);
                self.str("negedge");
                self.space(1);
                self.duplicated_token(&token);
            }
            _ => {}
        };

        self.reset_signal = Some(token);
        self.reset_active_low = matches!(reset_type, ResetType::AsyncLow | ResetType::SyncLow);
    }

    fn always_ff_reset_exist_in_sensitivity_list(&mut self, arg: &AlwaysFfReset) -> bool {
        if let Ok(found) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
            let reset_kind = match found.found.kind {
                SymbolKind::Port(x) => x.r#type.kind,
                SymbolKind::Variable(x) => x.r#type.kind,
                SymbolKind::ModportVariableMember(x) => {
                    let symbol = symbol_table::get(x.variable).unwrap();
                    if let SymbolKind::Variable(x) = symbol.kind {
                        x.r#type.kind
                    } else {
                        unreachable!();
                    }
                }
                _ => unreachable!(),
            };

            match reset_kind {
                TypeKind::ResetAsyncHigh | TypeKind::ResetAsyncLow => true,
                TypeKind::ResetSyncHigh | TypeKind::ResetSyncLow => false,
                _ => match self.build_opt.reset_type {
                    ResetType::AsyncLow => true,
                    ResetType::AsyncHigh => true,
                    ResetType::SyncLow => false,
                    ResetType::SyncHigh => false,
                },
            }
        } else {
            unreachable!()
        }
    }

    fn attribute_end(&mut self) {
        match self.attribute.pop() {
            Some(AttributeType::Ifdef) => {
                self.newline();
                self.str("`endif");
            }
            Some(AttributeType::Test) => {
                self.newline();
                self.str("`endif");
            }
            _ => (),
        }
    }

    fn is_implicit_scalar_type(&mut self, x: &ScalarType) -> bool {
        let mut stringifier = Stringifier::new();
        stringifier.scalar_type(x);
        let r#type = match stringifier.as_str() {
            "u32" => Some(BuiltinType::U32),
            "u64" => Some(BuiltinType::U64),
            "i32" => Some(BuiltinType::I32),
            "i64" => Some(BuiltinType::I64),
            "f32" => Some(BuiltinType::F32),
            "f64" => Some(BuiltinType::F64),
            "string" => Some(BuiltinType::String),
            _ => None,
        };
        if let Some(x) = r#type {
            self.build_opt.implicit_parameter_types.contains(&x)
        } else {
            false
        }
    }

    fn is_implicit_type(&mut self) -> bool {
        self.build_opt
            .implicit_parameter_types
            .contains(&BuiltinType::Type)
    }

    fn emit_import_declaration(&mut self, arg: &ImportDeclaration, moved: bool) {
        if moved {
            self.clear_adjust_line();
        }
        let src_line = self.src_line;

        self.in_import = true;
        self.import(&arg.import);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        if let Some(ref x) = arg.import_declaration_opt {
            self.colon_colon(&x.colon_colon);
            self.star(&x.star);
        }
        if moved {
            self.skip_comment = true;
        }
        self.semicolon(&arg.semicolon);
        if moved {
            self.skip_comment = false;
        }
        self.in_import = false;

        if moved {
            self.src_line = src_line;
        }
    }

    fn emit_generate_named_block(&mut self, arg: &GenerateNamedBlock, prefix: &str) {
        self.in_generate_block.push(());

        self.default_block = Some(emitting_identifier_token(
            &arg.identifier.identifier_token,
            None,
        ));
        self.token_will_push(&arg.l_brace.l_brace_token.replace(&format!("{prefix}begin")));
        self.space(1);
        self.colon(&arg.colon);
        self.identifier(&arg.identifier);
        for (i, x) in arg.generate_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.generate_group(&x.generate_group);
        }
        self.newline_list_post(arg.generate_named_block_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));

        self.in_generate_block.pop();
    }

    fn emit_statement_block(&mut self, arg: &StatementBlock, begin_kw: &str, end_kw: &str) {
        self.token_will_push(&arg.l_brace.l_brace_token.replace(begin_kw));

        let statement_block_list: Vec<_> = arg
            .statement_block_list
            .iter()
            .map(|x| Into::<Vec<_>>::into(x.statement_block_group.as_ref()))
            .collect();

        let mut base = 0;
        let mut n_newlines = 0;
        for x in &statement_block_list {
            for x in x {
                (base, n_newlines) = self.emit_declaration_in_statement_block(x, base, n_newlines);
            }
        }

        let mut n_newlines = 0;
        for (i, x) in statement_block_list.iter().enumerate() {
            for x in x {
                let ifdef_attributes: Vec<_> = arg.statement_block_list[i]
                    .statement_block_group
                    .statement_block_group_list
                    .iter()
                    .filter(|x| is_ifdef_attribute(&x.attribute))
                    .collect();

                for (j, x) in ifdef_attributes.iter().enumerate() {
                    if i == 0 && j == 0 {
                        self.newline_list(base + n_newlines);
                        n_newlines += 1;
                    }
                    self.attribute(&x.attribute);
                }

                if matches!(
                    x,
                    StatementBlockItem::LetStatement(_) | StatementBlockItem::Statement(_)
                ) {
                    if i != 0 || ifdef_attributes.is_empty() {
                        self.newline_list(base + n_newlines);
                        n_newlines += 1;
                    }

                    match &x {
                        StatementBlockItem::LetStatement(x) => self.let_statement(&x.let_statement),
                        StatementBlockItem::Statement(x) => self.statement(&x.statement),
                        _ => unreachable!(),
                    }
                }

                for _ in ifdef_attributes {
                    self.attribute_end();
                }
            }
        }
        self.newline_list_post(arg.statement_block_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace(end_kw));
    }

    fn emit_declaration_in_statement_block(
        &mut self,
        arg: &StatementBlockItem,
        base: usize,
        n_newlines: usize,
    ) -> (usize, usize) {
        if matches!(arg, StatementBlockItem::Statement(_)) {
            return (base, n_newlines);
        }

        self.newline_list(n_newlines);
        self.clear_adjust_line();
        match arg {
            StatementBlockItem::VarDeclaration(x) => {
                self.var_declaration(&x.var_declaration);
            }
            StatementBlockItem::LetStatement(x) => {
                let x = &x.let_statement;
                self.scalar_type(&x.array_type.scalar_type);
                self.space(1);
                self.identifier(&x.identifier);
                if let Some(ref x) = x.array_type.array_type_opt {
                    self.space(1);
                    self.array(&x.array);
                }
                self.str(";");
            }
            StatementBlockItem::ConstDeclaration(x) => {
                self.const_declaration(&x.const_declaration);
            }
            _ => {}
        }

        (base + 1, n_newlines + 1)
    }

    fn cond_type_prefix(&self, token: &Token) -> (Option<String>, bool) {
        fn prefix(token: &Token) -> Option<String> {
            let mut attrs = attribute_table::get(token);
            attrs.reverse();
            for attr in attrs {
                match attr {
                    Attr::CondType(CondTypeItem::None) => {
                        return None;
                    }
                    Attr::CondType(x) => {
                        return Some(format!("{x} "));
                    }
                    _ => (),
                }
            }
            None
        }

        let prefix = prefix(token);
        if self.build_opt.emit_cond_type {
            (prefix, false)
        } else {
            (None, prefix.is_some())
        }
    }

    fn get_inst_modport_array_size(&self, path: &GenericSymbolPath) -> Vec<Expression> {
        let (Ok(symbol), _) = self.resolve_generic_path(path, None) else {
            return vec![];
        };

        match &symbol.found.kind {
            SymbolKind::Port(x) => {
                if matches!(x.direction, SymDirection::Modport) {
                    return x.r#type.array.clone();
                }
            }
            SymbolKind::Instance(x) => {
                return x.array.clone();
            }
            _ => {}
        }

        vec![]
    }

    fn emit_flattened_select(&mut self, select: &[Box<Select>], array_size: &[Expression]) {
        let last_select = select.last().unwrap();

        for (i, x) in select.iter().enumerate() {
            if i == 0 {
                self.l_bracket(&x.l_bracket);
            } else {
                self.token(&x.l_bracket.l_bracket_token.replace("+"));
            }

            self.str("(");
            self.expression(&x.expression);
            self.str(")");

            if (i + 1) < array_size.len() {
                self.force_duplicated = true;
                for x in array_size.iter().skip(i + 1) {
                    self.str("*(");
                    self.expression(x);
                    self.str(")");
                }
                self.force_duplicated = false;
            } else if let Some(ref y) = x.select_opt
                && matches!(&*y.select_operator, SelectOperator::Step(_))
            {
                self.str("*(");
                self.expression(&y.expression);
                self.str(")");
            }
        }

        if select.len() == array_size.len() && last_select.select_opt.is_none() {
            self.r_bracket(&last_select.r_bracket);
            return;
        }

        self.force_duplicated = true;
        for (i, x) in select.iter().enumerate() {
            if i == 0 {
                self.token(&x.l_bracket.l_bracket_token.replace(":"));
            } else {
                self.token(&x.l_bracket.l_bracket_token.replace("+"));
            }

            if (i + 1) < select.len() || x.select_opt.is_none() {
                if (i + 1) < select.len() {
                    self.str("(");
                    self.expression(&x.expression);
                    self.str(")");
                } else {
                    self.str("((");
                    self.expression(&x.expression);
                    self.str(")+1)");
                }

                for x in array_size.iter().skip(i + 1) {
                    self.str("*(");
                    self.expression(x);
                    self.str(")");
                }

                if (i + 1) == select.len() {
                    self.str("-1");
                }
            } else if let Some(ref y) = x.select_opt {
                match &*y.select_operator {
                    SelectOperator::Colon(_) => {
                        self.str("(");
                        self.expression(&y.expression);
                        self.str(")");
                    }
                    SelectOperator::PlusColon(_) => {
                        self.str("(");
                        self.expression(&x.expression);
                        self.str(")+(");
                        self.expression(&y.expression);
                        self.str(")-1");
                    }
                    SelectOperator::MinusColon(_) => {
                        self.str("(");
                        self.expression(&x.expression);
                        self.str(")-(");
                        self.expression(&y.expression);
                        self.str(")+1");
                    }
                    SelectOperator::Step(_) => {
                        self.str("((");
                        self.expression(&x.expression);
                        self.str(")+1)*(");
                        self.expression(&y.expression);
                        self.str(")-1");
                    }
                }
            }
        }
        self.force_duplicated = false;
        self.r_bracket(&last_select.r_bracket);
    }

    fn emit_array(&mut self, array: &Array, flatten: bool) {
        self.l_bracket(&array.l_bracket);
        if flatten && !array.array_list.is_empty() {
            self.str("0:");
            self.str("(");
            self.expression(&array.expression);
            self.str(")");
            for x in &array.array_list {
                self.token(&x.comma.comma_token.replace("*("));
                self.expression(&x.expression);
                self.str(")");
            }
            self.str("-1");
        } else {
            self.str("0:");
            self.expression(&array.expression);
            self.str("-1");
            for x in &array.array_list {
                self.token(&x.comma.comma_token.replace("]["));
                self.str("0:");
                self.expression(&x.expression);
                self.str("-1");
            }
        }
        self.r_bracket(&array.r_bracket);
    }

    fn emit_expanded_modport_connections(&mut self) {
        let src_line = self.src_line;
        self.force_duplicated = true;
        self.aligner.disable_auto_finish();
        self.align_reset();

        for entry in self.modport_ports_table.as_mut().unwrap().drain() {
            self.clear_adjust_line();

            // emit interface instance
            self.single_line_start();
            self.duplicated_token(&entry.interface_name);
            self.space(1);
            self.duplicated_token(&entry.identifier);
            if !entry.array_size.is_empty() {
                self.space(1);
                for size in entry.array_size {
                    self.str(&format!("[0:{size}-1]"));
                }
            }
            self.space(1);
            self.str("();");
            self.single_line_finish();
            self.newline();

            // emit connections
            for ports in entry.ports {
                self.str("always_comb begin");
                self.newline_push();
                for (i, port) in ports.ports.iter().enumerate() {
                    if i > 0 {
                        self.newline();
                    }
                    self.clear_adjust_line();

                    let (lhs, rhs) = if matches!(port.direction, SymDirection::Input) {
                        (&port.interface_target, &port.identifier)
                    } else {
                        (&port.identifier, &port.interface_target)
                    };

                    self.align_start(align_kind::IDENTIFIER);
                    self.duplicated_token(lhs);
                    self.align_finish(align_kind::IDENTIFIER);

                    self.space(1);
                    self.str("=");
                    self.space(1);

                    self.align_start(align_kind::EXPRESSION);
                    self.duplicated_token(rhs);
                    self.align_finish(align_kind::EXPRESSION);
                    self.str(";")
                }
                self.newline_pop();
                self.str("end");
                self.newline();
                self.align_reset();
            }
        }

        self.aligner.enable_auto_finish();
        self.src_line = src_line;
        self.force_duplicated = false;
    }

    fn emit_inst(
        &mut self,
        header_token: &VerylToken,
        arg: &ComponentInstantiation,
        semicolon: &Semicolon,
    ) {
        let allow_missing_port =
            attribute_table::contains(&header_token.token, Attr::Allow(AllowItem::MissingPort));

        let (defined_ports, generic_map, symbol) =
            if let (Ok(symbol), _) = self.resolve_scoped_idnetifier(&arg.scoped_identifier) {
                match symbol.found.kind {
                    SymbolKind::Module(ref x) if !allow_missing_port => {
                        (x.ports.clone(), vec![], symbol.found)
                    }
                    SymbolKind::GenericInstance(ref x) => {
                        let base = symbol_table::get(x.base).unwrap();
                        match base.kind {
                            SymbolKind::Module(ref x) if !allow_missing_port => {
                                (x.ports.clone(), symbol.found.generic_maps(), base)
                            }
                            _ => (vec![], vec![], base),
                        }
                    }
                    _ => (vec![], vec![], symbol.found),
                }
            } else {
                unreachable!()
            };
        let connected_ports: Vec<_> = if let Some(ref x) = arg.component_instantiation_opt2 {
            if let Some(ref x) = x.inst_port.inst_port_opt {
                x.inst_port_list.as_ref().into()
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let modport_connections_table = ExpandModportConnectionsTable::create_from_inst_ports(
            &defined_ports,
            &connected_ports,
            &generic_map,
            &symbol.namespace,
        );
        self.modport_connections_tables
            .push(modport_connections_table);
        self.inst_module_namespace = Some(symbol.inner_namespace());

        let compact = attribute_table::is_format(&arg.identifier.first(), FormatItem::Compact);
        let single_line =
            arg.component_instantiation_opt2.is_none() && defined_ports.is_empty() || compact;
        if single_line {
            self.single_line_start();
        }
        if self.single_line() {
            self.align_start(align_kind::TYPE);
        }
        self.scoped_identifier(&arg.scoped_identifier);
        self.space(1);
        if self.single_line() {
            self.align_finish(align_kind::TYPE);
        }

        if let Some(ref x) = arg.component_instantiation_opt1 {
            // skip align at single line
            if self.mode == Mode::Emit || !self.single_line() {
                self.inst_parameter(&x.inst_parameter);
            }
            self.space(1);
        }
        if self.single_line() {
            self.align_start(align_kind::IDENTIFIER);
        }
        self.identifier(&arg.identifier);
        if self.single_line() {
            self.align_finish(align_kind::IDENTIFIER);
        }
        if let Some(ref x) = arg.component_instantiation_opt0 {
            self.space(1);
            self.emit_array(&x.array, self.build_opt.flatten_array_interface);
        }
        self.space(1);

        if let Some(ref x) = arg.component_instantiation_opt2 {
            self.token_will_push(&x.inst_port.l_paren.l_paren_token.replace("("));
            self.newline_push();
            if let Some(ref x) = x.inst_port.inst_port_opt {
                self.inst_port_list(&x.inst_port_list);
            }
            self.emit_inst_unconnected_port(&defined_ports, &connected_ports, &generic_map);
            self.newline_pop();
            self.token(&x.inst_port.r_paren.r_paren_token.replace(")"));
        } else if !defined_ports.is_empty() {
            self.str("(");
            self.newline_push();
            self.emit_inst_unconnected_port(&defined_ports, &connected_ports, &generic_map);
            self.newline_pop();
            self.str(")");
        } else {
            self.str("()");
        }
        self.semicolon(semicolon);
        if single_line {
            self.single_line_finish();
        }

        self.modport_connections_tables.pop();
        self.inst_module_namespace = None;
    }

    fn emit_inst_param_port_item_assigned_by_name(&mut self, identifier: &Identifier) {
        self.align_start(align_kind::EXPRESSION);
        match self.resolve_generic_path(&identifier.into(), None) {
            (Ok(symbol), _) => {
                let context: SymbolContext = self.into();
                let text = symbol_string(
                    &identifier.identifier_token,
                    &symbol.found,
                    &symbol.found.namespace,
                    &symbol.full_path,
                    &symbol.generic_tables,
                    &context,
                    1,
                );
                self.duplicated_token(&identifier.identifier_token.replace(&text));
            }
            (Err(_), path) => {
                // literal provied by generic param
                let text = path.base_path(0).0[0].to_string();
                self.duplicated_token(&identifier.identifier_token.replace(&text));
            }
        }
        self.align_finish(align_kind::EXPRESSION);
    }

    fn emit_port_identifier(&mut self, identifier: &Identifier) {
        let symbol = symbol_table::resolve((identifier, self.inst_module_namespace.as_ref()))
            .map(|x| x.found)
            .ok();
        self.emit_identifier(identifier, symbol.as_ref());
    }

    fn emit_inst_unconnected_port(
        &mut self,
        defined_ports: &[Port],
        connected_ports: &Vec<&InstPortItem>,
        generic_map: &[GenericMap],
    ) {
        if defined_ports.is_empty() || defined_ports.len() == connected_ports.len() {
            return;
        }

        self.generic_map.push(generic_map.to_owned());

        let unconnected_ports = defined_ports.iter().filter(|x| {
            !connected_ports
                .iter()
                .any(|y| x.name() == y.identifier.identifier_token.token.text)
        });

        // Disable aligner auto finish based on line number
        // because line number of default values are not reliable
        self.aligner.disable_auto_finish();

        let src_line = self.src_line;
        for (i, port) in unconnected_ports.enumerate() {
            if i >= 1 || !connected_ports.is_empty() {
                self.str(",");
                self.newline();
            }

            let property = port.property();
            self.str(".");
            self.clear_adjust_line();
            self.align_start(align_kind::IDENTIFIER);
            self.token(&port.token);
            self.align_finish(align_kind::IDENTIFIER);
            self.space(1);
            self.str("(");
            self.align_start(align_kind::EXPRESSION);
            if let Some(x) = &property.default_value {
                self.expression(x);
            }

            // Create a dummy token from the last token in expression to add align information
            let token = self.last_token.as_ref().unwrap().replace("");
            self.duplicated_token(&token);

            self.align_finish(align_kind::EXPRESSION);
            self.str(")");
        }

        self.aligner.enable_auto_finish();

        self.src_line = src_line;
        self.generic_map.pop();
    }

    fn emit_connect_statement(&mut self, arg: &IdentifierStatement) -> bool {
        let (identifier, operator, expression) = {
            let IdentifierStatementGroup::Assignment(x) = &*arg.identifier_statement_group else {
                return false;
            };
            let AssignmentGroup::DiamondOperator(y) = &*x.assignment.assignment_group else {
                return false;
            };
            (
                &arg.expression_identifier,
                &y.diamond_operator,
                &x.assignment.expression,
            )
        };

        let token = identifier.identifier();
        let operation = connect_operation_table::get(&token.token).unwrap();

        let mut lhs_identifier = identifier.clone();
        if operation.is_lhs_instance() {
            // remove modport path
            lhs_identifier.expression_identifier_list0.pop();
        }
        let lhs_generic_map = {
            let symbol = symbol_table::resolve(lhs_identifier.as_ref()).unwrap();
            self.get_interface_generic_map(&symbol.found)
        };

        let assign_operator = if self.in_always_ff { "<=" } else { "=" };

        self.str("begin");
        self.newline_push();
        if let Some((ports, _)) = operation.get_ports_with_expression() {
            for (i, (port, _)) in ports.iter().enumerate() {
                if i > 0 {
                    self.newline();
                    self.force_duplicated = true;
                }

                self.align_start(align_kind::IDENTIFIER);
                self.emit_connect_expression_operand(&lhs_identifier, port);
                self.align_finish(align_kind::IDENTIFIER);

                self.space(1);
                self.token(&operator.diamond_operator_token.replace(assign_operator));
                self.space(1);

                let cast_emitted =
                    self.emit_cast_for_connect_operand(token, port, &lhs_generic_map, None, None);
                self.expression(expression);

                if cast_emitted {
                    self.str(")");
                }
                self.semicolon(&arg.semicolon);
            }

            self.force_duplicated = false;
        } else {
            let mut rhs_identifier = expression.unwrap_identifier().unwrap().clone();
            if operation.is_rhs_instance() {
                // remove modport path
                rhs_identifier.expression_identifier_list0.pop();
            }
            let rhs_generic_map = {
                let symbol = symbol_table::resolve(&rhs_identifier).unwrap();
                self.get_interface_generic_map(&symbol.found)
            };

            for (i, (lhs_symbol, lhs_direction, rhs_symbol, _)) in
                operation.get_connection_pairs().iter().enumerate()
            {
                if i > 0 {
                    self.newline();
                    self.force_duplicated = true;
                }

                self.align_start(align_kind::IDENTIFIER);
                let (target, target_map, driver, driver_map) =
                    if matches!(lhs_direction, SymDirection::Output) {
                        self.emit_connect_expression_operand(&lhs_identifier, lhs_symbol);
                        (&lhs_symbol, &lhs_generic_map, &rhs_symbol, &rhs_generic_map)
                    } else {
                        self.emit_connect_expression_operand(&rhs_identifier, rhs_symbol);
                        (&rhs_symbol, &rhs_generic_map, &lhs_symbol, &lhs_generic_map)
                    };
                self.align_finish(align_kind::IDENTIFIER);

                self.space(1);
                self.token(&operator.diamond_operator_token.replace(assign_operator));
                self.space(1);

                let cast_emitted = self.emit_cast_for_connect_operand(
                    token,
                    target,
                    target_map,
                    Some(driver),
                    Some(driver_map),
                );
                if matches!(lhs_direction, SymDirection::Input) {
                    self.emit_connect_expression_operand(&lhs_identifier, lhs_symbol);
                } else {
                    self.emit_connect_expression_operand(&rhs_identifier, rhs_symbol);
                }

                if cast_emitted {
                    self.str(")");
                }
                self.semicolon(&arg.semicolon);
            }

            self.force_duplicated = false;
        }
        self.newline_pop();
        self.str("end");

        true
    }

    fn get_interface_generic_map(&self, symbol: &Symbol) -> Vec<GenericMap> {
        match &symbol.kind {
            SymbolKind::Port(x) => {
                if let Some(x) = x.r#type.get_user_defined()
                    && let (Ok(symbol), _) =
                        self.resolve_generic_path(&x.path, Some(&symbol.namespace))
                {
                    // symbol for interface is parent symbol because
                    // resolved symbol is for modport
                    let parent = symbol.found.get_parent().unwrap();
                    return parent.generic_maps();
                }
            }
            SymbolKind::Instance(x) => {
                if let (Ok(symbol), _) =
                    self.resolve_generic_path(&x.type_name, Some(&symbol.namespace))
                {
                    return symbol.found.generic_maps();
                }
            }
            _ => {}
        }

        vec![]
    }

    fn emit_connect_expression_operand(
        &mut self,
        identifier: &ExpressionIdentifier,
        member: &Symbol,
    ) {
        let port_identifier = identifier.scoped_identifier.identifier();

        let mut expanded_modport = None;
        if let Some(table) = self.modport_ports_table.as_ref() {
            expanded_modport = table.get_modport_member(&port_identifier.token, &member.token, &[]);
        }

        if let Some(expanded_modport) = expanded_modport {
            let text = expanded_modport.identifier.to_string();
            self.veryl_token(&port_identifier.replace(&text));
        } else {
            self.expression_identifier(identifier);
            self.emit_connect_operand_member_identifier(member);
        };
    }

    fn emit_connect_hierarchical_operand(
        &mut self,
        identifier: &HierarchicalIdentifier,
        member: &Symbol,
    ) {
        let port_identifier = &identifier.identifier.identifier_token;

        let mut expanded_modport = None;
        if let Some(table) = self.modport_ports_table.as_ref() {
            expanded_modport = table.get_modport_member(&port_identifier.token, &member.token, &[]);
        }

        if let Some(expanded_modport) = expanded_modport {
            let text = expanded_modport.identifier.to_string();
            self.veryl_token(&port_identifier.replace(&text));
        } else {
            self.hierarchical_identifier(identifier);
            self.emit_connect_operand_member_identifier(member);
        };
    }

    fn emit_connect_operand_member_identifier(&mut self, symbol: &Symbol) {
        let last_token = self.last_token.as_ref().unwrap();
        let member_token = last_token.replace(&format!(".{}", symbol.token));
        self.duplicated_token(&member_token);
    }

    fn emit_cast_for_connect_operand(
        &mut self,
        token: &VerylToken,
        target: &Symbol,
        target_map: &Vec<GenericMap>,
        driver: Option<&Symbol>,
        driver_map: Option<&Vec<GenericMap>>,
    ) -> bool {
        fn get_type_symbol(
            symbol: &Symbol,
            map: &Vec<GenericMap>,
        ) -> Option<(Symbol, Vec<SymbolId>, GenericTables)> {
            let r#type = symbol.kind.get_type();
            if r#type.is_none() || !r#type.unwrap().width.is_empty() {
                return None;
            }

            let user_defined = r#type.unwrap().get_user_defined()?;
            let (type_symbol, _) =
                resolve_generic_path(&user_defined.path, &symbol.namespace, Some(map));
            type_symbol
                .ok()
                .map(|x| (x.found, x.full_path, x.generic_tables))
        }

        let Some((target_type, target_path, target_tables)) = get_type_symbol(target, target_map)
        else {
            return false;
        };

        if let (Some(driver), Some(driver_map)) = (driver, driver_map)
            && let Some((driver_type, _, _)) = get_type_symbol(driver, driver_map)
            && target_type.id == driver_type.id
        {
            return false;
        }

        let context = SymbolContext {
            project_name: self.project_name,
            build_opt: self.build_opt.clone(),
            in_import: false,
            in_direction_modport: false,
            generic_map: target_map.clone(),
        };
        let text = symbol_string(
            token,
            &target_type,
            &target_type.namespace,
            &target_path,
            &target_tables,
            &context,
            1,
        );
        self.str(&text);
        self.str("'(");

        true
    }

    fn emit_function_call(
        &mut self,
        identifier: &ExpressionIdentifier,
        function_call: &FunctionCall,
    ) {
        let (defined_ports, generic_map, namespace) =
            if let (Ok(symbol), _) = self.resolve_generic_path(&identifier.into(), None) {
                match symbol.found.kind {
                    SymbolKind::Function(ref x) => {
                        (x.ports.clone(), Vec::new(), symbol.found.namespace)
                    }
                    SymbolKind::ModportFunctionMember(x) => {
                        let symbol = symbol_table::get(x.function).unwrap();
                        let SymbolKind::Function(x) = symbol.kind else {
                            unreachable!();
                        };
                        (x.ports.clone(), Vec::new(), symbol.namespace)
                    }
                    SymbolKind::GenericInstance(ref x) => {
                        let base = symbol_table::get(x.base).unwrap();
                        match base.kind {
                            SymbolKind::Function(ref x) => {
                                (x.ports.clone(), symbol.found.generic_maps(), base.namespace)
                            }
                            _ => (Vec::new(), Vec::new(), base.namespace),
                        }
                    }
                    _ => (Vec::new(), Vec::new(), symbol.found.namespace),
                }
            } else {
                unreachable!()
            };

        let in_named_argument = if let Some(ref x) = function_call.function_call_opt {
            let list: Vec<_> = x.argument_list.as_ref().into();
            list.iter().any(|x| x.argument_item_opt.is_some())
        } else {
            false
        };

        self.in_named_argument.push(in_named_argument);
        if in_named_argument {
            self.token_will_push(&function_call.l_paren.l_paren_token);
            self.newline_push();
            self.align_reset();
        } else {
            self.l_paren(&function_call.l_paren);
        }
        let n_args = if let Some(ref x) = function_call.function_call_opt {
            let modport_connections_table =
                ExpandModportConnectionsTable::create_from_argument_list(
                    &defined_ports,
                    &x.argument_list,
                    &generic_map,
                    &namespace,
                );
            self.modport_connections_tables
                .push(modport_connections_table);
            self.argument_list(&x.argument_list);
            1 + x.argument_list.argument_list_list.len()
        } else {
            0
        };

        self.generic_map.push(generic_map);

        let unconnected_ports = defined_ports.iter().skip(n_args);
        for (i, port) in unconnected_ports.enumerate() {
            if i >= 1 || n_args >= 1 {
                self.str(", ");
            }

            let property = port.property();
            self.expression(&property.default_value.unwrap());
        }

        self.generic_map.pop();
        if in_named_argument {
            self.newline_pop();
        }
        self.r_paren(&function_call.r_paren);
        self.in_named_argument.pop();
        self.modport_connections_tables.pop();
    }

    fn emit_argument_item(&mut self, arg: &ArgumentItem, port_index: usize) {
        let modport_entry =
            if let Some(identifier) = arg.argument_expression.expression.unwrap_identifier() {
                if let Some(table) = self.modport_connections_tables.last_mut() {
                    if arg.argument_item_opt.is_some() {
                        table.remove(identifier.identifier())
                    } else {
                        table.pop_front(port_index)
                    }
                } else {
                    None
                }
            } else {
                None
            };

        if let Some(x) = modport_entry {
            let src_line = self.src_line;
            self.aligner.disable_auto_finish();
            self.clear_adjust_line();

            for (i, connection) in x
                .connections
                .iter()
                .flat_map(|x| x.connections.iter())
                .enumerate()
            {
                if i > 0 {
                    self.str(",");
                    if *self.in_named_argument.last().unwrap() {
                        self.newline();
                    } else {
                        self.space(1);
                    }
                }

                if arg.argument_item_opt.is_some() {
                    self.str(".");
                    self.align_start(align_kind::IDENTIFIER);
                    self.duplicated_token(&connection.port_target);
                    self.align_finish(align_kind::IDENTIFIER);
                    self.space(1);

                    self.str("(");
                    self.align_start(align_kind::EXPRESSION);
                    self.duplicated_token(&connection.interface_target);
                    self.align_finish(align_kind::EXPRESSION);
                    self.str(")");
                } else {
                    self.duplicated_token(&connection.interface_target);
                }

                self.clear_adjust_line();
            }

            self.aligner.enable_auto_finish();
            self.src_line = src_line;
        } else if let Some(ref x) = arg.argument_item_opt {
            self.str(".");
            self.align_start(align_kind::IDENTIFIER);
            // Directly emittion because named argument can't be resolved and emitted by symbol_string
            let token = VerylToken::new(arg.argument_expression.expression.first());
            self.token(&token);
            self.align_finish(align_kind::IDENTIFIER);
            self.space(1);
            self.str("(");
            self.align_start(align_kind::EXPRESSION);
            self.expression(&x.expression);
            self.align_finish(align_kind::EXPRESSION);
            self.str(")");
        } else {
            self.argument_expression(&arg.argument_expression);
        }
    }

    fn emit_modport_default_member(&mut self, arg: &ModportDeclaration) {
        if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
            if let SymbolKind::Modport(x) = symbol.found.kind {
                for (i, x) in x.members.iter().enumerate() {
                    let symbol = symbol_table::get(*x).unwrap();
                    if let TokenSource::Generated(_) = symbol.token.source
                        && let SymbolKind::ModportVariableMember(x) = symbol.kind
                    {
                        if i != 0 || arg.modport_declaration_opt.is_some() {
                            self.str(",");
                            self.newline();
                        }
                        let token = arg
                            .modport_declaration_opt0
                            .clone()
                            .unwrap()
                            .dot_dot
                            .dot_dot_token;
                        self.align_start(align_kind::DIRECTION);
                        self.duplicated_token(&token.replace(&x.direction.to_string()));
                        self.align_finish(align_kind::DIRECTION);
                        self.space(1);
                        self.align_start(align_kind::IDENTIFIER);
                        self.duplicated_token(&token.replace(&symbol.token.text.to_string()));
                        self.align_finish(align_kind::IDENTIFIER);
                    }
                }
            } else {
                unreachable!();
            }
        }
    }

    fn resolve_scoped_idnetifier(
        &self,
        arg: &ScopedIdentifier,
    ) -> (Result<ResolveResult, ResolveError>, GenericSymbolPath) {
        let path: GenericSymbolPath = arg.into();
        self.resolve_generic_path(&path, None)
    }

    fn resolve_generic_path(
        &self,
        path: &GenericSymbolPath,
        namespace: Option<&Namespace>,
    ) -> (Result<ResolveResult, ResolveError>, GenericSymbolPath) {
        let generic_map = self.generic_map.last();
        if let Some(namespace) = namespace {
            resolve_generic_path(path, namespace, generic_map)
        } else {
            let namespace = namespace_table::get(path.paths[0].base.id).unwrap();
            resolve_generic_path(path, &namespace, generic_map)
        }
    }

    fn push_resolved_identifier(&mut self, x: &str) {
        if let Some(identifier) = self.resolved_identifier.last_mut() {
            identifier.push_str(x);
        }
    }

    fn get_generic_maps(&self, symbol: &Symbol) -> Vec<GenericMap> {
        let parent_id = if let Some(maps) = self.generic_map.last() {
            maps.last().and_then(|x| x.id)
        } else {
            None
        };

        // The given symbol is a top level symbol or
        // the parent symbol is a non generic object.
        if parent_id.is_none() {
            return symbol.generic_maps();
        }

        let parent_id = parent_id.unwrap();
        symbol
            .generic_maps()
            .into_iter()
            .filter(|x| {
                if let Some(id) = x.id {
                    let symbol = symbol_table::get(id).unwrap();
                    let parent = symbol.get_parent().unwrap();
                    if matches!(parent.kind, SymbolKind::GenericInstance(_)) {
                        parent_id == parent.id
                    } else {
                        // If the symbol is a generic instance and used in the definition scope
                        // it belongs to the base object of the parent even if the parent is a generic object.
                        true
                    }
                } else {
                    true
                }
            })
            .collect()
    }

    fn push_generic_map(&mut self, map: GenericMap) {
        if let Some(maps) = self.generic_map.last_mut() {
            maps.push(map);
        } else {
            self.generic_map.push(vec![map]);
        }
    }

    fn get_generic_map(&self) -> Vec<GenericMap> {
        if let Some(map) = self.generic_map.last() {
            map.clone()
        } else {
            vec![]
        }
    }

    fn pop_generic_map(&mut self) {
        if let Some(maps) = self.generic_map.last_mut() {
            maps.pop();
        }
    }

    fn emit_generic_instance_name_comment(&mut self, generic_map: &GenericMap) {
        if generic_map.generic() && self.build_opt.hashed_mangled_name {
            let name = generic_map.name(false, false);
            self.str(&format!("// {name}"));
            self.newline();
        }
    }

    fn emit_generic_instance_name(
        &mut self,
        token: &VerylToken,
        generic_map: &GenericMap,
        omit_project_prefix: bool,
    ) {
        let name = generic_map.name(true, self.build_opt.hashed_mangled_name);
        let name = if self.build_opt.omit_project_prefix || omit_project_prefix {
            let project_name = format!("{}_", self.project_name.unwrap());
            if let Some(x) = name.strip_prefix(&project_name) {
                x
            } else {
                &name
            }
        } else {
            &name
        };
        let name = name.replace("$std_", "__std_");

        self.token(&token.replace(&name));
    }
}

fn calc_emitted_width(number: &str, base: u32) -> Option<usize> {
    let width = strnum_bitwidth::bitwidth(number, base)?;

    let width_by_digits = if number.starts_with("0") {
        // replace the 1st char with '1'
        let number: String = number
            .chars()
            .enumerate()
            .map(|(i, s)| if i == 0 { '1' } else { s })
            .collect();
        strnum_bitwidth::bitwidth(&number, base)?
    } else {
        width
    };

    let width = if width_by_digits > width {
        width_by_digits
    } else {
        width
    };

    if width >= 1 { Some(width) } else { Some(0) }
}

impl VerylWalker for Emitter {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.token(arg);
    }

    /// Semantic action for non-terminal 'Based'
    fn based(&mut self, arg: &Based) {
        let token = &arg.based_token;
        let text = token.to_string();
        let (width, tail) = text.split_once('\'').unwrap();

        if width.is_empty() {
            let base = &tail[0..1];
            let base_num = match base {
                "b" => 2,
                "o" => 8,
                "d" => 10,
                "h" => 16,
                _ => unreachable!(),
            };
            let number = &tail[1..];

            let text = if let Some(actual_width) = calc_emitted_width(number, base_num) {
                format!("{actual_width}'{base}{number}")
            } else {
                // If width can't be calculated, emit it as is (e.g. `'h0`)
                format!("'{base}{number}")
            };
            self.veryl_token(&arg.based_token.replace(&text));
        } else {
            self.veryl_token(&arg.based_token);
        }
    }

    /// Semantic action for non-terminal 'AllBit'
    fn all_bit(&mut self, arg: &AllBit) {
        let text = &arg.all_bit_token.to_string();
        let (width, tail) = text.split_once('\'').unwrap();

        if width.is_empty() {
            self.veryl_token(&arg.all_bit_token);
        } else {
            let width: usize = width.parse().unwrap();
            let text = format!("{width}'b{}", tail.repeat(width));
            self.veryl_token(&arg.all_bit_token.replace(&text));
        }
    }

    /// Semantic action for non-terminal 'Comma'
    fn comma(&mut self, arg: &Comma) {
        if self.string.ends_with("`endif") {
            self.truncate(self.string.len() - "`endif".len());

            let trailing_endif = format!(
                "`endif{}{}",
                NEWLINE,
                " ".repeat(self.indent * self.format_opt.indent_width)
            );
            let mut additional_endif = 0;
            while self.string.ends_with(&trailing_endif) {
                self.truncate(self.string.len() - trailing_endif.len());
                additional_endif += 1;
            }

            self.truncate(self.string.trim_end().len());
            self.veryl_token(&arg.comma_token);
            self.newline();
            self.str("`endif");
            for _ in 0..additional_endif {
                self.newline();
                self.str("`endif");
            }
        } else {
            self.veryl_token(&arg.comma_token);
        }
    }

    /// Semantic action for non-terminal 'Bool'
    fn bool(&mut self, arg: &Bool) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.bool_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'Clock'
    fn clock(&mut self, arg: &Clock) {
        self.veryl_token(&arg.clock_token.replace("logic"));
    }

    /// Semantic action for non-terminal 'ClockPosedge'
    fn clock_posedge(&mut self, arg: &ClockPosedge) {
        self.veryl_token(&arg.clock_posedge_token.replace("logic"));
    }

    /// Semantic action for non-terminal 'ClockNegedge'
    fn clock_negedge(&mut self, arg: &ClockNegedge) {
        self.veryl_token(&arg.clock_negedge_token.replace("logic"));
    }

    /// Semantic action for non-terminal 'Const'
    fn r#const(&mut self, arg: &Const) {
        self.veryl_token(&arg.const_token.replace("localparam"));
    }

    /// Semantic action for non-terminal 'Reset'
    fn reset(&mut self, arg: &Reset) {
        self.veryl_token(&arg.reset_token.replace("logic"));
    }

    /// Semantic action for non-terminal 'ResetAsyncHigh'
    fn reset_async_high(&mut self, arg: &ResetAsyncHigh) {
        self.veryl_token(&arg.reset_async_high_token.replace("logic"));
    }

    /// Semantic action for non-terminal 'ResetAsyncLow'
    fn reset_async_low(&mut self, arg: &ResetAsyncLow) {
        self.veryl_token(&arg.reset_async_low_token.replace("logic"));
    }

    /// Semantic action for non-terminal 'ResetSyncHigh'
    fn reset_sync_high(&mut self, arg: &ResetSyncHigh) {
        self.veryl_token(&arg.reset_sync_high_token.replace("logic"));
    }

    /// Semantic action for non-terminal 'ResetSyncLow'
    fn reset_sync_low(&mut self, arg: &ResetSyncLow) {
        self.veryl_token(&arg.reset_sync_low_token.replace("logic"));
    }

    /// Semantic action for non-terminal 'F32'
    fn f32(&mut self, arg: &F32) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.f32_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'F64'
    fn f64(&mut self, arg: &F64) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.f64_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'False'
    fn r#false(&mut self, arg: &False) {
        self.veryl_token(&arg.false_token.replace("1'b0"));
    }

    /// Semantic action for non-terminal 'I8'
    fn i8(&mut self, arg: &I8) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.i8_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'I16'
    fn i16(&mut self, arg: &I16) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.i16_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'I32'
    fn i32(&mut self, arg: &I32) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.i32_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'I64'
    fn i64(&mut self, arg: &I64) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.i64_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'Lsb'
    fn lsb(&mut self, arg: &Lsb) {
        self.token(&arg.lsb_token.replace("0"));
    }

    /// Semantic action for non-terminal 'Msb'
    fn msb(&mut self, arg: &Msb) {
        let identifier = self.resolved_identifier.last().unwrap();
        let demension_number = msb_table::get(arg.msb_token.token.id).unwrap();

        let text = if demension_number == 0 {
            format!("($bits({identifier}) - 1)")
        } else {
            format!("($size({identifier}, {demension_number}) - 1)")
        };
        self.token(&arg.msb_token.replace(&text));
    }

    /// Semantic action for non-terminal 'Param'
    fn param(&mut self, arg: &Param) {
        self.veryl_token(&arg.param_token.replace("parameter"));
    }

    /// Semantic action for non-terminal 'Switch'
    fn switch(&mut self, arg: &Switch) {
        self.veryl_token(&arg.switch_token.replace("case"));
    }

    /// Semantic action for non-terminal 'True'
    fn r#true(&mut self, arg: &True) {
        self.veryl_token(&arg.true_token.replace("1'b1"));
    }

    /// Semantic action for non-terminal 'U8'
    fn u8(&mut self, arg: &U8) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.u8_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'U16'
    fn u16(&mut self, arg: &U16) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.u16_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'U32'
    fn u32(&mut self, arg: &U32) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.u32_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'U64'
    fn u64(&mut self, arg: &U64) {
        let literal: TypeLiteral = arg.into();
        self.veryl_token(&arg.u64_token.replace(&literal.to_sv_string()));
    }

    /// Semantic action for non-terminal 'Identifier'
    fn identifier(&mut self, arg: &Identifier) {
        let symbol = symbol_table::resolve(arg).map(|x| x.found).ok();
        self.emit_identifier(arg, symbol.as_ref());
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

    /// Semantic action for non-terminal 'HierarchicalIdentifier'
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) {
        let list_len = &arg.hierarchical_identifier_list0.len();
        let array_size = if self.build_opt.flatten_array_interface
            && !arg.hierarchical_identifier_list.is_empty()
        {
            self.get_inst_modport_array_size(&arg.identifier.as_ref().into())
        } else {
            vec![]
        };

        if *list_len == 0 {
            let symbol = symbol_table::resolve(arg).map(|x| x.found).ok();
            self.emit_identifier(&arg.identifier, symbol.as_ref());
        } else {
            self.identifier(&arg.identifier);
        }

        if array_size.len() > 1 {
            let select: Vec<_> = arg
                .hierarchical_identifier_list
                .iter()
                .map(|x| x.select.clone())
                .collect();
            self.emit_flattened_select(&select, &array_size);
        } else {
            for x in &arg.hierarchical_identifier_list {
                self.select(&x.select);
            }
        }

        for (i, x) in arg.hierarchical_identifier_list0.iter().enumerate() {
            self.dot(&x.dot);
            if (i + 1) == *list_len {
                let symbol = symbol_table::resolve(arg).map(|x| x.found).ok();
                self.emit_identifier(&x.identifier, symbol.as_ref());
            } else {
                self.identifier(&x.identifier);
            }
            for x in &x.hierarchical_identifier_list0_list {
                self.select(&x.select);
            }
        }
    }

    /// Semantic action for non-terminal 'Operator08'
    fn operator08(&mut self, arg: &Operator08) {
        match arg.operator08_token.to_string().as_str() {
            "<:" => self.str("<"),
            ">:" => self.str(">"),
            _ => self.veryl_token(&arg.operator08_token),
        }
    }

    /// Semantic action for non-terminal 'ScopedIdentifier'
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) {
        if is_anonymous_token(&arg.identifier().token) {
            self.veryl_token(&arg.identifier().replace(""));
        } else {
            match self.resolve_scoped_idnetifier(arg) {
                (Ok(symbol), _) => {
                    let context: SymbolContext = self.into();
                    let text = symbol_string(
                        arg.identifier(),
                        &symbol.found,
                        &symbol.found.namespace,
                        &symbol.full_path,
                        &symbol.generic_tables,
                        &context,
                        arg.get_scope_depth(),
                    );
                    self.identifier(&Identifier {
                        identifier_token: arg.identifier().replace(&text),
                    });
                }
                (Err(_), path) if !path.is_resolvable() => {
                    // emit literal by generics
                    let text = if let Some(x) = path.to_literal() {
                        match x {
                            Literal::Value(_) => path.base_path(0).0[0].to_string(),
                            Literal::Type(x) => x.to_sv_string(),
                            Literal::Boolean(x) => if x { "1'b1" } else { "1'b0" }.to_string(),
                        }
                    } else {
                        path.base_path(0).0[0].to_string()
                    };
                    self.identifier(&Identifier {
                        identifier_token: arg.identifier().replace(&text),
                    });
                }
                _ => {}
            }
        }
    }

    /// Semantic action for non-terminal 'ExpressionIdentifier'
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) {
        let mut expanded_modport = None;
        if let Some(table) = self.modport_ports_table.as_ref()
            && let Some(member_identifier) = arg.expression_identifier_list0.first()
        {
            let port_identifier = arg.scoped_identifier.identifier();
            expanded_modport = table
                .get_modport_member(
                    &port_identifier.token,
                    &member_identifier.identifier.identifier_token.token,
                    &[],
                )
                .map(|x| (port_identifier, x));
        }

        let array_size = if self.build_opt.flatten_array_interface
            && !arg.expression_identifier_list.is_empty()
            && expanded_modport.is_none()
        {
            self.get_inst_modport_array_size(&arg.scoped_identifier.as_ref().into())
        } else {
            vec![]
        };

        self.resolved_identifier.push("".to_string());
        if let Some((token, modport_member)) = expanded_modport.as_ref() {
            let text = modport_member.identifier.to_string();
            self.veryl_token(&token.replace(&text));
            self.push_resolved_identifier(&text);
        } else if array_size.len() > 1 {
            let select: Vec<_> = arg
                .expression_identifier_list
                .iter()
                .map(|x| x.select.clone())
                .collect();
            self.scoped_identifier(&arg.scoped_identifier);
            self.emit_flattened_select(&select, &array_size);
            self.push_resolved_identifier("[0]");
        } else {
            self.scoped_identifier(&arg.scoped_identifier);
            for x in &arg.expression_identifier_list {
                self.select(&x.select);
            }
            for _x in &arg.expression_identifier_list {
                self.push_resolved_identifier("[0]");
            }
        }

        for (i, x) in arg.expression_identifier_list0.iter().enumerate() {
            if i > 0 || expanded_modport.is_none() {
                self.dot(&x.dot);
                self.push_resolved_identifier(".");
                if (i + 1) < arg.expression_identifier_list0.len() {
                    self.emit_identifier(&x.identifier, None);
                } else {
                    let symbol = symbol_table::resolve(arg).map(|x| x.found).ok();
                    self.emit_identifier(&x.identifier, symbol.as_ref());
                }
            }

            for x in &x.expression_identifier_list0_list {
                self.select(&x.select);
            }
            for _x in &x.expression_identifier_list0_list {
                self.push_resolved_identifier("[0]");
            }
        }
        self.resolved_identifier.pop();
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
                if i == 0 {
                    self.token(&x.r#if.if_token.replace("(("));
                } else {
                    self.token(&x.r#if.if_token.replace("("));
                }
                self.expression(&x.expression);

                self.token_will_push(&x.question.question_token.replace(") ? ("));
                self.newline_push();
                self.expression(&x.expression0);
                self.newline_pop();

                if (i + 1) < arg.if_expression_list.len() {
                    self.token(&x.colon.colon_token.replace(") : "));
                } else {
                    self.token_will_push(&x.colon.colon_token.replace(") : ("));
                }
            }

            self.newline_push();
            self.expression01(&arg.expression01);
            self.newline_pop();
            self.str("))");

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
        if let Some(x) = &arg.expression12_opt {
            match x.casting_type.as_ref() {
                CastingType::U8(_) => self.str("unsigned'(byte'("),
                CastingType::U16(_) => self.str("unsigned'(shortint'("),
                CastingType::U32(_) => self.str("unsigned'(int'("),
                CastingType::U64(_) => self.str("unsigned'(longint'("),
                CastingType::I8(_) => self.str("signed'(byte'("),
                CastingType::I16(_) => self.str("signed'(shortint'("),
                CastingType::I32(_) => self.str("signed'(int'("),
                CastingType::I64(_) => self.str("signed'(longint'("),
                CastingType::F32(x) => {
                    self.f32(&x.f32);
                    self.str("'(");
                }
                CastingType::F64(x) => {
                    self.f64(&x.f64);
                    self.str("'(");
                }
                CastingType::Bool(_) => self.str("(("),
                CastingType::UserDefinedType(x) => {
                    self.user_defined_type(&x.user_defined_type);
                    self.str("'(");
                }
                CastingType::Based(x) => {
                    self.based(&x.based);
                    self.str("'(");
                }
                CastingType::BaseLess(x) => {
                    self.base_less(&x.base_less);
                    self.str("'(");
                }
                // casting to clock type doesn't change polarity
                CastingType::Clock(_)
                | CastingType::ClockPosedge(_)
                | CastingType::ClockNegedge(_) => (),
                CastingType::Reset(_)
                | CastingType::ResetAsyncHigh(_)
                | CastingType::ResetAsyncLow(_)
                | CastingType::ResetSyncHigh(_)
                | CastingType::ResetSyncLow(_) => {
                    let mut eval = Evaluator::new(&[]);
                    let src = eval.expression13(&arg.expression13);
                    let dst = x.casting_type.as_ref();
                    let reset_type = self.build_opt.reset_type;

                    let src_kind = &src.get_reset_kind();

                    let src_is_high =
                        matches!(
                            (src_kind, reset_type),
                            (Some(EvaluatedTypeResetKind::Implicit), ResetType::AsyncHigh)
                        ) | matches!(
                            (src_kind, reset_type),
                            (Some(EvaluatedTypeResetKind::Implicit), ResetType::SyncHigh)
                        ) | matches!(src_kind, Some(EvaluatedTypeResetKind::AsyncHigh))
                            | matches!(src_kind, Some(EvaluatedTypeResetKind::SyncHigh));

                    let src_is_low = matches!(
                        (src_kind, reset_type),
                        (Some(EvaluatedTypeResetKind::Implicit), ResetType::AsyncLow)
                    ) | matches!(
                        (src_kind, reset_type),
                        (Some(EvaluatedTypeResetKind::Implicit), ResetType::SyncLow)
                    ) | matches!(src_kind, Some(EvaluatedTypeResetKind::AsyncLow))
                        | matches!(src_kind, Some(EvaluatedTypeResetKind::SyncLow));

                    let dst_is_high = matches!(
                        (dst, reset_type),
                        (CastingType::Reset(_), ResetType::AsyncHigh)
                    ) | matches!(
                        (dst, reset_type),
                        (CastingType::Reset(_), ResetType::SyncHigh)
                    ) | matches!(dst, CastingType::ResetAsyncHigh(_))
                        | matches!(dst, CastingType::ResetSyncHigh(_));

                    let dst_is_low = matches!(
                        (dst, reset_type),
                        (CastingType::Reset(_), ResetType::AsyncLow)
                    ) | matches!(
                        (dst, reset_type),
                        (CastingType::Reset(_), ResetType::SyncLow)
                    ) | matches!(dst, CastingType::ResetAsyncLow(_))
                        | matches!(dst, CastingType::ResetSyncLow(_));

                    if (src_is_high && dst_is_low) || (src_is_low && dst_is_high) {
                        self.str("~")
                    }
                }
            }
        }
        self.expression13(&arg.expression13);
        if let Some(x) = &arg.expression12_opt {
            match x.casting_type.as_ref() {
                CastingType::U8(_)
                | CastingType::U16(_)
                | CastingType::U32(_)
                | CastingType::U64(_)
                | CastingType::I8(_)
                | CastingType::I16(_)
                | CastingType::I32(_)
                | CastingType::I64(_) => self.str("))"),
                CastingType::F32(_)
                | CastingType::F64(_)
                | CastingType::UserDefinedType(_)
                | CastingType::Based(_)
                | CastingType::BaseLess(_) => self.str(")"),
                CastingType::Bool(_) => self.str(") != 1'b0)"),
                _ => (),
            }
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

    /// Semantic action for non-terminal 'IdentifierFactor'
    fn identifier_factor(&mut self, arg: &IdentifierFactor) {
        if let Some(ref x) = arg.identifier_factor_opt {
            match x.identifier_factor_opt_group.as_ref() {
                IdentifierFactorOptGroup::FunctionCall(x) => {
                    self.expression_identifier(&arg.expression_identifier);
                    self.emit_function_call(&arg.expression_identifier, &x.function_call);
                }
                IdentifierFactorOptGroup::StructConstructor(x) => {
                    self.expression_identifier(&arg.expression_identifier);
                    self.struct_constructor(&x.struct_constructor);
                }
            }
        } else {
            self.expression_identifier(&arg.expression_identifier);
        }
    }

    /// Semantic action for non-terminal 'ArgumentList'
    fn argument_list(&mut self, arg: &ArgumentList) {
        self.emit_argument_item(&arg.argument_item, 0);
        for (i, x) in arg.argument_list_list.iter().enumerate() {
            self.comma(&x.comma);
            if *self.in_named_argument.last().unwrap() {
                self.newline();
            } else {
                self.space(1);
            }
            self.emit_argument_item(&x.argument_item, i + 1);
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
            self.str(",");
            if self.multi_line() {
                self.newline();
            } else {
                self.space(1);
            }
            self.defaul(&x.defaul);
            self.str(":");
            self.space(1);
            self.expression(&x.expression);
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
        if self.multi_line() {
            self.newline_pop();
            self.align_reset();
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
        if self.multi_line() {
            self.newline_pop();
            self.align_reset();
        }
    }

    /// Semantic action for non-terminal 'ArrayLiteralItem'
    fn array_literal_item(&mut self, arg: &ArrayLiteralItem) {
        match &*arg.array_literal_item_group {
            ArrayLiteralItemGroup::ExpressionArrayLiteralItemOpt(x) => {
                if let Some(ref y) = x.array_literal_item_opt {
                    self.expression(&y.expression);
                    self.str("{");
                    self.expression(&x.expression);
                    self.str("}");
                } else {
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
        self.token(&arg.case.case_token.replace("(("));
        self.case_expression_condition(&arg.expression, &arg.case_condition.range_item);
        self.str(") ? (");
        self.newline_push();
        self.expression(&arg.expression0);
        self.newline_pop();
        for x in &arg.case_condition.case_condition_list {
            self.token(&x.comma.comma_token.replace(")"));
            self.space(1);
            self.str(": (");
            self.case_expression_condition(&arg.expression, &x.range_item);
            self.str(") ? (");
            self.newline_push();
            self.expression(&arg.expression0);
            self.newline_pop();
        }
        self.str(")");
        self.space(1);
        for x in &arg.case_expression_list {
            self.str(": (");
            self.case_expression_condition(&arg.expression, &x.case_condition.range_item);
            self.str(") ? (");
            self.newline_push();
            self.expression(&x.expression);
            self.newline_pop();
            for y in &x.case_condition.case_condition_list {
                self.token(&y.comma.comma_token.replace(")"));
                self.space(1);
                self.str(": (");
                self.case_expression_condition(&arg.expression, &y.range_item);
                self.str(") ? (");
                self.newline_push();
                self.expression(&x.expression);
                self.newline_pop();
            }
            self.token(&x.comma.comma_token.replace(")"));
            self.space(1);
        }
        self.str(": (");
        self.newline_push();
        self.expression(&arg.expression1);
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("))"));
    }

    /// Semantic action for non-terminal 'SwitchExpression'
    fn switch_expression(&mut self, arg: &SwitchExpression) {
        self.token(&arg.switch.switch_token.replace("((("));
        self.expression(&arg.switch_condition.expression);
        self.str(")");
        self.space(1);
        self.str("== 1'b1) ? (");
        self.newline_push();
        self.expression(&arg.expression);
        self.newline_pop();
        for x in &arg.switch_condition.switch_condition_list {
            self.token(&x.comma.comma_token.replace(")"));
            self.space(1);
            self.str(": ((");
            self.expression(&x.expression);
            self.str(")");
            self.space(1);
            self.str("== 1'b1) ? (");
            self.newline_push();
            self.expression(&arg.expression);
            self.newline_pop();
        }
        self.str(")");
        self.space(1);
        for x in &arg.switch_expression_list {
            self.str(": ((");
            self.expression(&x.switch_condition.expression);
            self.str(")");
            self.space(1);
            self.str("== 1'b1) ? (");
            self.newline_push();
            self.expression(&x.expression);
            self.newline_pop();
            for y in &x.switch_condition.switch_condition_list {
                self.token(&y.comma.comma_token.replace(")"));
                self.space(1);
                self.str(": ((");
                self.expression(&y.expression);
                self.str(")");
                self.space(1);
                self.str("== 1'b1) ? (");
                self.newline_push();
                self.expression(&x.expression);
                self.newline_pop();
            }
            self.token(&x.comma.comma_token.replace(")"));
            self.space(1);
        }
        self.str(": (");
        self.newline_push();
        self.expression(&arg.expression0);
        self.newline_pop();
        self.token(&arg.r_brace.r_brace_token.replace("))"));
    }

    /// Semantic action for non-terminal 'InsideExpression'
    fn inside_expression(&mut self, arg: &InsideExpression) {
        if self.build_opt.expand_inside_operation {
            self.inside_expanded_expression(arg);
        } else {
            self.inside_normal_expression(arg);
        }
    }

    /// Semantic action for non-terminal 'OutsideExpression'
    fn outside_expression(&mut self, arg: &OutsideExpression) {
        if self.build_opt.expand_inside_operation {
            self.outside_expanded_expression(arg);
        } else {
            self.outside_normal_expression(arg);
        }
    }

    /// Semantic action for non-terminal 'RangeList'
    fn range_list(&mut self, arg: &RangeList) {
        self.range_item(&arg.range_item);
        for x in &arg.range_list_list {
            self.comma(&x.comma);
            self.space(1);
            self.range_item(&x.range_item);
        }
    }

    /// Semantic action for non-terminal 'Select'
    fn select(&mut self, arg: &Select) {
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        if let Some(ref x) = arg.select_opt {
            match &*x.select_operator {
                SelectOperator::Step(_) => {
                    self.str("*(");
                    self.expression(&x.expression);
                    self.str(")+:(");
                    self.expression(&x.expression);
                    self.str(")");
                }
                _ => {
                    self.select_operator(&x.select_operator);
                    self.expression(&x.expression);
                }
            }
        }
        self.r_bracket(&arg.r_bracket);
    }

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        self.token(&arg.l_angle.l_angle_token.replace("["));
        self.expression(&arg.expression);
        self.str("-1:0");
        for x in &arg.width_list {
            self.token(&x.comma.comma_token.replace("]["));
            self.expression(&x.expression);
            self.str("-1:0");
        }
        self.token(&arg.r_angle.r_angle_token.replace("]"));
    }

    /// Semantic action for non-terminal 'Array'
    fn array(&mut self, arg: &Array) {
        let flatten = self.in_direction_modport && self.build_opt.flatten_array_interface;
        self.emit_array(arg, flatten);
    }

    /// Semantic action for non-terminal 'Range'
    fn range(&mut self, arg: &Range) {
        if let Some(ref x) = arg.range_opt {
            self.str("[");
            self.expression(&arg.expression);
            self.str(":");
            match &*x.range_operator {
                RangeOperator::DotDot(_) => {
                    self.str("(");
                    self.expression(&x.expression);
                    self.str(")-1");
                }
                RangeOperator::DotDotEqu(_) => {
                    self.expression(&x.expression);
                }
            }
            self.str("]");
        } else {
            self.expression(&arg.expression);
        }
    }

    /// Semantic action for non-terminal 'TypeModifier'
    fn type_modifier(&mut self, arg: &TypeModifier) {
        match arg {
            TypeModifier::Tri(x) => {
                self.tri(&x.tri);
                self.space(1);
            }
            TypeModifier::Signed(_) => self.signed = true,
            TypeModifier::Defaul(x) => {
                self.token(&x.defaul.default_token.replace(""));
            }
        }
    }

    /// Semantic action for non-terminal 'FactorType'
    fn factor_type(&mut self, arg: &FactorType) {
        match arg.factor_type_group.as_ref() {
            FactorTypeGroup::VariableTypeFactorTypeOpt(x) => {
                self.variable_type(&x.variable_type);
                if self.signed {
                    self.space(1);
                    self.str("signed");
                    self.signed = false;
                }
                if self.in_scalar_type {
                    self.align_finish(align_kind::TYPE);
                    self.align_start(align_kind::WIDTH);
                }
                if let Some(ref x) = x.factor_type_opt {
                    self.space(1);
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
        // disable align in Expression
        let enable_align = self.in_expression.is_empty();
        self.emit_scalar_type(arg, enable_align);
    }

    /// Semantic action for non-terminal 'StatementBlock'
    fn statement_block(&mut self, arg: &StatementBlock) {
        self.emit_statement_block(arg, "begin", "end");
    }

    /// Semantic action for non-terminal 'LetStatement'
    fn let_statement(&mut self, arg: &LetStatement) {
        // Variable declaration is moved to emit_declaration_in_statement_block
        //self.scalar_type(&arg.array_type.scalar_type);
        //self.space(1);
        //self.identifier(&arg.identifier);
        //if let Some(ref x) = arg.array_type.array_type_opt {
        //    self.space(1);
        //    self.array(&x.array);
        //}
        //self.str(";");
        //self.newline();
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'IdentifierStatement'
    fn identifier_statement(&mut self, arg: &IdentifierStatement) {
        let connect_statement_emitted = self.emit_connect_statement(arg);
        if !connect_statement_emitted {
            self.align_start(align_kind::IDENTIFIER);
            self.expression_identifier(&arg.expression_identifier);
            self.assignment_lefthand_side = Some(*arg.expression_identifier.clone());
            self.align_finish(align_kind::IDENTIFIER);
            match &*arg.identifier_statement_group {
                IdentifierStatementGroup::FunctionCall(x) => {
                    self.emit_function_call(&arg.expression_identifier, &x.function_call);
                }
                IdentifierStatementGroup::Assignment(x) => {
                    self.assignment(&x.assignment);
                }
            }
            self.semicolon(&arg.semicolon);
        }
    }

    /// Semantic action for non-terminal 'Assignment'
    fn assignment(&mut self, arg: &Assignment) {
        let is_nba = if !self.in_always_ff {
            false
        } else if let Some(lhs) = &self.assignment_lefthand_side {
            if let Ok(lhs_symbol) = symbol_table::resolve(lhs.scoped_identifier.as_ref()) {
                match lhs_symbol.found.kind {
                    SymbolKind::Variable(x) => !matches!(
                        x.affiliation,
                        VariableAffiliation::StatementBlock | VariableAffiliation::Function
                    ),
                    _ => true,
                }
            } else {
                true
            }
        } else {
            true
        };

        self.space(1);
        if is_nba {
            self.align_start(align_kind::ASSIGNMENT);
            self.str("<");
            match &*arg.assignment_group {
                AssignmentGroup::Equ(x) => {
                    self.equ(&x.equ);
                    self.align_finish(align_kind::ASSIGNMENT);
                }
                AssignmentGroup::AssignmentOperator(x) => {
                    let token = format!(
                        "{}",
                        x.assignment_operator.assignment_operator_token.token.text
                    );
                    // remove trailing `=` from assignment operator
                    let token = &token[0..token.len() - 1];
                    self.token(&x.assignment_operator.assignment_operator_token.replace("="));
                    self.align_finish(align_kind::ASSIGNMENT);
                    self.space(1);
                    let identifier = self.assignment_lefthand_side.take().unwrap();
                    self.expression_identifier(&identifier);
                    self.space(1);
                    self.str(token);
                }
                _ => unreachable!(),
            }
            self.space(1);
            if let AssignmentGroup::AssignmentOperator(_) = &*arg.assignment_group {
                self.str("(");
                self.expression(&arg.expression);
                self.str(")");
            } else {
                self.expression(&arg.expression);
            }
        } else {
            self.align_start(align_kind::ASSIGNMENT);
            match &*arg.assignment_group {
                AssignmentGroup::Equ(x) => self.equ(&x.equ),
                AssignmentGroup::AssignmentOperator(x) => {
                    self.assignment_operator(&x.assignment_operator)
                }
                _ => unreachable!(),
            }
            self.align_finish(align_kind::ASSIGNMENT);
            self.space(1);
            self.expression(&arg.expression);
        }
    }

    /// Semantic action for non-terminal 'IfStatement'
    fn if_statement(&mut self, arg: &IfStatement) {
        let (prefix, force_last_item_default) = self.cond_type_prefix(&arg.r#if.if_token.token);
        self.token(&arg.r#if.if_token.append(&prefix, &None));
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.str(")");
        self.space(1);
        self.statement_block(&arg.statement_block);
        let len = arg.if_statement_list.len();
        for (i, x) in arg.if_statement_list.iter().enumerate() {
            let force_default =
                force_last_item_default & (i == (len - 1)) & arg.if_statement_opt.is_none();
            if force_default {
                self.space(1);
                self.str("else");
                self.space(1);
            } else {
                self.space(1);
                self.r#else(&x.r#else);
                self.space(1);
                self.r#if(&x.r#if);
                self.space(1);
                self.str("(");
                self.expression(&x.expression);
                self.str(")");
                self.space(1);
            }
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
        let (prefix, force_last_item_default) =
            self.cond_type_prefix(&arg.if_reset.if_reset_token.token);
        self.token(
            &arg.if_reset
                .if_reset_token
                .replace("if")
                .append(&prefix, &None),
        );
        self.space(1);
        self.str("(");
        if self.reset_active_low {
            self.str("!");
        }
        self.duplicated_token(&self.reset_signal.clone().unwrap());
        self.str(")");
        self.space(1);
        self.statement_block(&arg.statement_block);
        let len = arg.if_reset_statement_list.len();
        for (i, x) in arg.if_reset_statement_list.iter().enumerate() {
            let force_default =
                force_last_item_default & (i == (len - 1)) & arg.if_reset_statement_opt.is_none();
            if force_default {
                self.space(1);
                self.str("else");
                self.space(1);
            } else {
                self.space(1);
                self.r#else(&x.r#else);
                self.space(1);
                self.r#if(&x.r#if);
                self.space(1);
                self.str("(");
                self.expression(&x.expression);
                self.str(")");
                self.space(1);
            }
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

    /// Semantic action for non-terminal 'BreakStatement'
    fn break_statement(&mut self, arg: &BreakStatement) {
        self.r#break(&arg.r#break);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ForStatement'
    fn for_statement(&mut self, arg: &ForStatement) {
        let ascending_order = arg.for_statement_opt.is_none();
        let include_end = if let Some(x) = &arg.range.range_opt {
            matches!(*x.range_operator, RangeOperator::DotDotEqu(_))
        } else {
            true
        };
        let (beg, end) = if let Some(x) = &arg.range.range_opt {
            if ascending_order {
                (&arg.range.expression, &x.expression)
            } else {
                (&x.expression, &arg.range.expression)
            }
        } else {
            (&arg.range.expression, &arg.range.expression)
        };

        self.r#for(&arg.r#for);
        self.space(1);
        self.str("(");
        self.emit_scalar_type(&arg.scalar_type, false);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("=");
        self.space(1);
        self.expression(beg);
        if !ascending_order && !include_end {
            self.str(" - 1");
        }
        self.str(";");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        match (ascending_order, include_end) {
            (true, true) => self.str("<="),
            (true, false) => self.str("<"),
            _ => self.str(">="),
        }
        self.space(1);
        self.expression(end);
        self.str(";");
        self.space(1);
        if let Some(ref x) = arg.for_statement_opt0 {
            self.identifier(&arg.identifier);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
        } else {
            self.identifier(&arg.identifier);
            if ascending_order {
                self.str("++");
            } else {
                self.str("--");
            }
        }
        self.str(")");
        self.space(1);
        self.statement_block(&arg.statement_block);
    }

    /// Semantic action for non-terminal 'CaseStatement'
    fn case_statement(&mut self, arg: &CaseStatement) {
        if self.build_opt.expand_inside_operation {
            self.case_expaneded_statement(arg);
        } else {
            self.case_inside_statement(arg);
        }
    }

    /// Semantic action for non-terminal 'SwitchStatement'
    fn switch_statement(&mut self, arg: &SwitchStatement) {
        self.switch(&arg.switch);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("(1'b1)"));
        for (i, x) in arg.switch_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.switch_item(&x.switch_item);
        }
        self.newline_list_post(arg.switch_statement_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("endcase"));
    }

    /// Semantic action for non-terminal 'SwitchItem'
    fn switch_item(&mut self, arg: &SwitchItem) {
        self.align_start(align_kind::EXPRESSION);
        match &*arg.switch_item_group {
            SwitchItemGroup::SwitchCondition(x) => {
                self.expression(&x.switch_condition.expression);
                for x in &x.switch_condition.switch_condition_list {
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
            SwitchItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.align_finish(align_kind::EXPRESSION);
        self.colon(&arg.colon);
        self.space(1);
        match &*arg.switch_item_group0 {
            SwitchItemGroup0::Statement(x) => self.statement(&x.statement),
            SwitchItemGroup0::StatementBlock(x) => self.statement_block(&x.statement_block),
        }
    }

    /// Semantic action for non-terminal 'Attribute'
    fn attribute(&mut self, arg: &Attribute) {
        self.in_attribute = true;
        let identifier = arg.identifier.identifier_token.to_string();
        match identifier.as_str() {
            "ifdef" | "ifndef" | "elsif" | "else" => {
                let elsif_else = matches!(identifier.as_str(), "elsif" | "else");
                let remove_endif = elsif_else && self.string.trim_end().ends_with("`endif");
                if remove_endif {
                    self.unindent();
                    self.truncate(self.string.len() - format!("`endif{NEWLINE}").len());
                }

                self.consume_adjust_line(&arg.identifier.identifier_token.token);
                self.str("`");
                self.identifier(&arg.identifier);

                if let Some(ref x) = arg.attribute_opt {
                    self.space(1);
                    if let AttributeItem::Identifier(x) = &*x.attribute_list.attribute_item {
                        self.identifier(&x.identifier);
                    }
                }

                self.newline();
                self.attribute.push(AttributeType::Ifdef);

                self.clear_adjust_line();
            }
            "sv" => {
                if let Some(ref x) = arg.attribute_opt {
                    self.str("(*");
                    self.space(1);
                    if let AttributeItem::StringLiteral(x) = &*x.attribute_list.attribute_item {
                        let text = x.string_literal.string_literal_token.to_string();
                        let text = &text[1..text.len() - 1];
                        let text = text.replace("\\\"", "\"");
                        self.token(&x.string_literal.string_literal_token.replace(&text));
                    }
                    self.space(1);
                    self.str("*)");
                    self.newline();
                }
            }
            "test" => {
                if let Some(ref x) = arg.attribute_opt
                    && let AttributeItem::Identifier(x) = &*x.attribute_list.attribute_item
                {
                    let test_name = x.identifier.identifier_token.to_string();
                    let text = format!(
                        "`ifdef __veryl_test_{}_{}__",
                        self.project_name.unwrap(),
                        test_name
                    );
                    self.token(&arg.hash_l_bracket.hash_l_bracket_token.replace(&text));
                    self.newline();
                    let mut wavedump = format!(
                        r##"    `ifdef __veryl_wavedump_{}_{}__
        module __veryl_wavedump;
            initial begin
                $dumpfile("{}.vcd");
                $dumpvars();
            end
        endmodule
    `endif
"##,
                        self.project_name.unwrap(),
                        test_name,
                        test_name
                    );

                    if cfg!(windows) {
                        wavedump = wavedump.replace("\n", NEWLINE);
                    }

                    self.str(&wavedump);
                    self.attribute.push(AttributeType::Test);
                }
            }
            _ => (),
        }
        self.in_attribute = false;
    }

    /// Semantic action for non-terminal 'LetDeclaration'
    fn let_declaration(&mut self, arg: &LetDeclaration) {
        let is_tri = arg
            .array_type
            .scalar_type
            .scalar_type_list
            .iter()
            .any(|x| matches!(x.type_modifier.as_ref(), TypeModifier::Tri(_)));

        self.scalar_type(&arg.array_type.scalar_type);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.align_start(align_kind::ARRAY);
        if let Some(ref x) = arg.array_type.array_type_opt {
            self.space(1);
            self.array(&x.array);
        } else {
            let loc = self.align_last_location(align_kind::IDENTIFIER);
            self.align_dummy_location(align_kind::ARRAY, loc);
        }
        self.align_finish(align_kind::ARRAY);
        self.str(";");
        self.space(1);
        if is_tri {
            self.str("assign");
        } else {
            self.str("always_comb");
        }
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'VarDeclaration'
    fn var_declaration(&mut self, arg: &VarDeclaration) {
        self.scalar_type(&arg.array_type.scalar_type);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.align_start(align_kind::ARRAY);
        if let Some(ref x) = arg.array_type.array_type_opt {
            self.space(1);
            self.array(&x.array);
        } else {
            let loc = self.align_last_location(align_kind::IDENTIFIER);
            self.align_dummy_location(align_kind::ARRAY, loc);
        }
        self.align_finish(align_kind::ARRAY);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ConstDeclaration'
    fn const_declaration(&mut self, arg: &ConstDeclaration) {
        self.r#const(&arg.r#const);
        self.space(1);
        match &*arg.const_declaration_group {
            ConstDeclarationGroup::ArrayType(x) => {
                if !self.is_implicit_scalar_type(&x.array_type.scalar_type) {
                    self.scalar_type(&x.array_type.scalar_type);
                    self.space(1);
                } else {
                    self.align_start(align_kind::TYPE);
                    self.align_dummy_location(
                        align_kind::TYPE,
                        Some(arg.r#const.const_token.token.into()),
                    );
                    self.align_finish(align_kind::TYPE);
                }
                self.align_start(align_kind::IDENTIFIER);
                self.identifier(&arg.identifier);
                self.align_finish(align_kind::IDENTIFIER);
                self.align_start(align_kind::ARRAY);
                if let Some(ref x) = x.array_type.array_type_opt {
                    self.space(1);
                    self.array(&x.array);
                } else {
                    let loc = self.align_last_location(align_kind::IDENTIFIER);
                    self.align_dummy_location(align_kind::ARRAY, loc);
                }
                self.align_finish(align_kind::ARRAY);
            }
            ConstDeclarationGroup::Type(x) => {
                self.align_start(align_kind::TYPE);
                if !self.is_implicit_type() {
                    self.r#type(&x.r#type);
                    self.space(1);
                } else {
                    self.align_dummy_location(
                        align_kind::TYPE,
                        Some(arg.r#const.const_token.token.into()),
                    );
                }
                self.align_finish(align_kind::TYPE);
                self.align_start(align_kind::IDENTIFIER);
                self.identifier(&arg.identifier);
                self.align_finish(align_kind::IDENTIFIER);
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
        self.token(&arg.r#type.type_token.replace("typedef"));
        self.space(1);
        self.scalar_type(&arg.array_type.scalar_type);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        if let Some(ato) = &arg.array_type.array_type_opt {
            self.space(1);
            self.align_start(align_kind::ARRAY);
            self.array(&ato.array);
            self.align_finish(align_kind::ARRAY);
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
        self.in_always_ff = true;
        self.always_ff(&arg.always_ff);
        self.space(1);
        self.str("@");
        self.space(1);
        if let Some(ref x) = arg.always_ff_declaration_opt {
            self.always_ff_explicit_event_list(&x.always_ff_event_list, arg);
        } else {
            self.always_ff_implicit_event_list(arg);
        }
        self.space(1);
        self.statement_block(&arg.statement_block);
        self.in_always_ff = false;
    }

    /// Semantic action for non-terminal 'AlwaysFfClock'
    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) {
        if let Ok(symbol) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
            let clock_type = get_variable_type_kind(&symbol.found)
                .map(|x| match x {
                    TypeKind::ClockPosedge => ClockType::PosEdge,
                    TypeKind::ClockNegedge => ClockType::NegEdge,
                    TypeKind::Clock => self.build_opt.clock_type,
                    _ => unreachable!(),
                })
                .unwrap();

            match clock_type {
                ClockType::PosEdge => self.str("posedge"),
                ClockType::NegEdge => self.str("negedge"),
            }
            self.space(1);
            self.hierarchical_identifier(&arg.hierarchical_identifier);
        } else {
            unreachable!()
        }
    }

    /// Semantic action for non-terminal 'AlwaysFfReset'
    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) {
        if let Ok(found) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
            let reset_type = get_variable_type_kind(&found.found)
                .map(|x| match x {
                    TypeKind::ResetAsyncHigh => ResetType::AsyncHigh,
                    TypeKind::ResetAsyncLow => ResetType::AsyncLow,
                    TypeKind::ResetSyncHigh => ResetType::SyncHigh,
                    TypeKind::ResetSyncLow => ResetType::SyncLow,
                    TypeKind::Reset => self.build_opt.reset_type,
                    _ => unreachable!(),
                })
                .unwrap();

            match reset_type {
                ResetType::AsyncHigh => {
                    self.str("posedge");
                    self.space(1);
                    self.hierarchical_identifier(&arg.hierarchical_identifier);
                }
                ResetType::AsyncLow => {
                    self.str("negedge");
                    self.space(1);
                    self.hierarchical_identifier(&arg.hierarchical_identifier);
                }
                _ => {}
            };

            let mut stringifier = Stringifier::new();
            stringifier.hierarchical_identifier(&arg.hierarchical_identifier);
            let token = arg
                .hierarchical_identifier
                .identifier
                .identifier_token
                .replace(stringifier.as_str());

            self.reset_signal = Some(emitting_identifier_token(&token, Some(&found.found)));
            self.reset_active_low = matches!(reset_type, ResetType::AsyncLow | ResetType::SyncLow);
        } else {
            unreachable!()
        }
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        self.always_comb(&arg.always_comb);
        self.space(1);
        self.statement_block(&arg.statement_block);
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        let idents: Vec<_> = arg.assign_destination.as_ref().into();
        let mut emit_assign = false;
        for ident in idents {
            if let Ok(symbol) = symbol_table::resolve(ident) {
                match &symbol.found.kind {
                    SymbolKind::Variable(x) => {
                        if x.r#type.has_modifier(&SymTypeModifierKind::Tri) {
                            emit_assign = true;
                        }
                    }
                    SymbolKind::Port(x) => {
                        if x.r#type.has_modifier(&SymTypeModifierKind::Tri) {
                            emit_assign = true;
                        }
                    }
                    _ => (),
                }
            } else {
                // External symbols may be tri-state
                emit_assign = true;
            }
        }
        if emit_assign {
            self.assign(&arg.assign);
        } else {
            self.token(&arg.assign.assign_token.replace("always_comb"));
        }
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
        let token = &arg.hierarchical_identifier.identifier.identifier_token;
        let operation = connect_operation_table::get(&token.token).unwrap();

        let mut lhs_identifier = arg.hierarchical_identifier.clone();
        if operation.is_lhs_instance() {
            // remove modport path
            lhs_identifier.hierarchical_identifier_list0.pop();
        }
        let lhs_generic_map = {
            let symbol = symbol_table::resolve(lhs_identifier.as_ref()).unwrap();
            self.get_interface_generic_map(&symbol.found)
        };

        if let Some((ports, _)) = operation.get_ports_with_expression() {
            let output_ports: Vec<_> = ports
                .iter()
                .filter(|(_, direction)| matches!(direction, SymDirection::Output))
                .collect();
            let inout_ports: Vec<_> = ports
                .iter()
                .filter(|(_, direction)| matches!(direction, SymDirection::Inout))
                .collect();

            for (i, (port, _)) in output_ports.iter().enumerate() {
                if i == 0 {
                    self.token_will_push(&arg.connect.connect_token.replace("always_comb begin"));
                    self.newline_push();
                } else {
                    self.newline();
                    self.force_duplicated = true;
                }

                self.align_start(align_kind::IDENTIFIER);
                self.emit_connect_hierarchical_operand(&lhs_identifier, port);
                self.align_finish(align_kind::IDENTIFIER);

                self.space(1);
                self.token(&arg.diamond_operator.diamond_operator_token.replace("="));
                self.space(1);

                let cast_emitted =
                    self.emit_cast_for_connect_operand(token, port, &lhs_generic_map, None, None);
                self.expression(&arg.expression);

                if cast_emitted {
                    self.str(")");
                }
                self.semicolon(&arg.semicolon);

                if (i + 1) == output_ports.len() {
                    self.newline_pop();
                    self.str("end");
                }
            }
            self.align_reset();

            for (i, (port, _)) in inout_ports.iter().enumerate() {
                if i > 0 || !output_ports.is_empty() {
                    self.newline();
                    self.force_duplicated = true;
                }

                self.token(&arg.connect.connect_token.replace("assign"));
                self.space(1);

                self.align_start(align_kind::IDENTIFIER);
                self.emit_connect_hierarchical_operand(&lhs_identifier, port);
                self.align_finish(align_kind::IDENTIFIER);

                self.space(1);
                self.token(&arg.diamond_operator.diamond_operator_token.replace("="));
                self.space(1);

                let cast_emitted =
                    self.emit_cast_for_connect_operand(token, port, &lhs_generic_map, None, None);
                self.expression(&arg.expression);

                if cast_emitted {
                    self.str(")");
                }
                self.semicolon(&arg.semicolon);
            }

            self.align_reset();
            self.force_duplicated = false;
        } else {
            let connect_pairs = operation.get_connection_pairs();
            let input_output_pairs: Vec<_> = connect_pairs
                .iter()
                .filter(|x| matches!(x.1, SymDirection::Input | SymDirection::Output))
                .collect();
            let inout_piars: Vec<_> = connect_pairs
                .iter()
                .filter(|x| matches!(x.1, SymDirection::Inout))
                .collect();

            let mut rhs_identifier = arg.expression.unwrap_identifier().unwrap().clone();
            if operation.is_rhs_instance() {
                // remove modport path
                rhs_identifier.expression_identifier_list0.pop();
            }
            let rhs_generic_map = {
                let symbol = symbol_table::resolve(&rhs_identifier).unwrap();
                self.get_interface_generic_map(&symbol.found)
            };

            for (i, (lhs_symbol, lhs_direction, rhs_symbol, _)) in
                input_output_pairs.iter().enumerate()
            {
                if i == 0 {
                    self.token_will_push(&arg.connect.connect_token.replace("always_comb begin"));
                    self.newline_push();
                } else {
                    self.newline();
                    self.force_duplicated = true;
                }

                self.align_start(align_kind::IDENTIFIER);
                let (target, target_map, driver, driver_map) =
                    if matches!(lhs_direction, SymDirection::Output) {
                        self.emit_connect_hierarchical_operand(&lhs_identifier, lhs_symbol);
                        (&lhs_symbol, &lhs_generic_map, &rhs_symbol, &rhs_generic_map)
                    } else {
                        self.emit_connect_expression_operand(&rhs_identifier, rhs_symbol);
                        (&rhs_symbol, &rhs_generic_map, &lhs_symbol, &lhs_generic_map)
                    };
                self.align_finish(align_kind::IDENTIFIER);

                self.space(1);
                self.token(&arg.diamond_operator.diamond_operator_token.replace("="));
                self.space(1);

                let cast_emitted = self.emit_cast_for_connect_operand(
                    token,
                    target,
                    target_map,
                    Some(driver),
                    Some(driver_map),
                );
                if matches!(lhs_direction, SymDirection::Input) {
                    self.emit_connect_hierarchical_operand(&lhs_identifier, lhs_symbol);
                } else {
                    self.emit_connect_expression_operand(&rhs_identifier, rhs_symbol);
                }

                if cast_emitted {
                    self.str(")");
                }
                self.semicolon(&arg.semicolon);

                if (i + 1) == input_output_pairs.len() {
                    self.newline_pop();
                    self.str("end");
                }
            }
            self.align_reset();

            for (i, (lhs_symbol, _, rhs_symbol, _)) in inout_piars.iter().enumerate() {
                if i > 0 || !input_output_pairs.is_empty() {
                    self.newline();
                    self.force_duplicated = true;
                }

                self.token(&arg.connect.connect_token.replace("tran ("));
                self.emit_connect_hierarchical_operand(&lhs_identifier, lhs_symbol);
                self.str(",");
                self.space(1);
                self.emit_connect_expression_operand(&rhs_identifier, rhs_symbol);
                self.str(")");
                self.semicolon(&arg.semicolon);
            }

            self.force_duplicated = false;
        }
    }

    /// Semantic action for non-terminal 'ModportDeclaration'
    fn modport_declaration(&mut self, arg: &ModportDeclaration) {
        self.modport(&arg.modport);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("("));
        self.newline_push();
        if let Some(ref x) = arg.modport_declaration_opt {
            self.modport_list(&x.modport_list);
        }
        if arg.modport_declaration_opt0.is_some() {
            self.emit_modport_default_member(arg);
        }
        self.newline_pop();
        self.clear_adjust_line();
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
        for x in &arg.modport_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.modport_group_group {
            ModportGroupGroup::LBraceModportListRBrace(x) => {
                self.modport_list(&x.modport_list);
            }
            ModportGroupGroup::ModportItem(x) => self.modport_item(&x.modport_item),
        }
        for _ in &arg.modport_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'ModportItem'
    fn modport_item(&mut self, arg: &ModportItem) {
        self.direction(&arg.direction);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
    }

    /// Semantic action for non-terminal 'EnumDeclaration'
    fn enum_declaration(&mut self, arg: &EnumDeclaration) {
        let enum_symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        if let SymbolKind::Enum(r#enum) = enum_symbol.found.kind {
            self.enum_width = r#enum.width;
            self.emit_enum_implicit_valiant = matches!(
                r#enum.encoding,
                EnumEncodingItem::OneHot | EnumEncodingItem::Gray
            );
        }

        self.token(
            &arg.r#enum
                .enum_token
                .append(&Some(String::from("typedef ")), &None),
        );
        self.space(1);
        if let Some(ref x) = arg.enum_declaration_opt {
            self.enum_type = Some(x.scalar_type.as_ref().clone());
            self.emit_scalar_type(&x.scalar_type, false);
        } else {
            self.str(&format!("logic [{}-1:0]", self.enum_width));
        }
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

        self.enum_type = None;
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
        for x in &arg.enum_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.enum_group_group {
            EnumGroupGroup::LBraceEnumListRBrace(x) => {
                self.enum_list(&x.enum_list);
            }
            EnumGroupGroup::EnumItem(x) => self.enum_item(&x.enum_item),
        }
        for _ in &arg.enum_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'EnumItem'
    fn enum_item(&mut self, arg: &EnumItem) {
        let member_symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let (prefix, value) = if let SymbolKind::EnumMember(member) = member_symbol.found.kind {
            (member.prefix, member.value)
        } else {
            unreachable!();
        };

        let token = identifier_token_with_prefix_suffix(
            &arg.identifier.identifier_token,
            &Some(format!("{prefix}_")),
            &None,
        );
        self.veryl_token(&token);
        if let Some(ref x) = arg.enum_item_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            if let Some(enum_type) = self.enum_type.as_ref().cloned() {
                self.str("$bits(");
                self.force_duplicated = true;
                self.emit_scalar_type(&enum_type, false);
                self.force_duplicated = false;
                self.str(")");
            } else {
                self.str(&self.enum_width.to_string());
            }
            self.str("'(");
            self.expression(&x.expression);
            self.str(")");
        } else if self.emit_enum_implicit_valiant {
            self.str(&format!(
                " = {}'d{}",
                self.enum_width,
                value.value().unwrap_or(0),
            ));
        }
    }

    /// Semantic action for non-terminal 'StructUnionDeclaration'
    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = self.get_generic_maps(&symbol.found);

        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.push_generic_map(map.clone());

            self.emit_generic_instance_name_comment(map);
            match &*arg.struct_union {
                StructUnion::Struct(x) => {
                    let prefix = Some(String::from("typedef "));
                    let suffix = Some(String::from(" packed"));
                    self.token(&x.r#struct.struct_token.append(&prefix, &suffix));
                }
                StructUnion::Union(x) => {
                    let prefix = Some(String::from("typedef "));
                    let suffix = Some(String::from(" packed"));
                    self.token(&x.union.union_token.append(&prefix, &suffix));
                }
            }
            self.space(1);
            self.token_will_push(&arg.l_brace.l_brace_token);
            self.newline_push();
            self.struct_union_list(&arg.struct_union_list);
            self.newline_pop();
            self.str("}");
            self.space(1);
            if map.generic() {
                self.emit_generic_instance_name(&arg.identifier.identifier_token, map, true);
            } else {
                self.identifier(&arg.identifier);
            }
            self.str(";");
            self.token(&arg.r_brace.r_brace_token.replace(""));

            self.pop_generic_map();
        }
    }

    /// Semantic action for non-terminal 'StructUnionList'
    fn struct_union_list(&mut self, arg: &StructUnionList) {
        self.struct_union_group(&arg.struct_union_group);
        for x in &arg.struct_union_list_list {
            self.token(&x.comma.comma_token.replace(";"));
            self.newline();
            self.struct_union_group(&x.struct_union_group);
        }
        if let Some(ref x) = arg.struct_union_list_opt {
            self.token(&x.comma.comma_token.replace(";"));
        } else {
            self.str(";");
        }
    }

    /// Semantic action for non-terminal 'StructUnionGroup'
    fn struct_union_group(&mut self, arg: &StructUnionGroup) {
        for x in &arg.struct_union_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.struct_union_group_group {
            StructUnionGroupGroup::LBraceStructUnionListRBrace(x) => {
                self.struct_union_list(&x.struct_union_list);
            }
            StructUnionGroupGroup::StructUnionItem(x) => {
                self.struct_union_item(&x.struct_union_item)
            }
        }
        for _ in &arg.struct_union_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'StructUnionItem'
    fn struct_union_item(&mut self, arg: &StructUnionItem) {
        self.scalar_type(&arg.scalar_type);
        self.space(1);
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
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
        self.token(&arg.inst.inst_token.replace(""));
        self.emit_inst(
            &arg.inst.inst_token,
            &arg.component_instantiation,
            &arg.semicolon,
        );
    }

    /// Semantic action for non-terminal 'BindDeclaration'
    fn bind_declaration(&mut self, arg: &BindDeclaration) {
        self.bind(&arg.bind);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        self.space(1);
        self.emit_inst(
            &arg.bind.bind_token,
            &arg.component_instantiation,
            &arg.semicolon,
        );
    }

    /// Semantic action for non-terminal 'InstParameter'
    fn inst_parameter(&mut self, arg: &InstParameter) {
        self.hash(&arg.hash);
        self.token_will_push(&arg.l_paren.l_paren_token);
        self.newline_push();
        if let Some(ref x) = arg.inst_parameter_opt {
            self.inst_parameter_list(&x.inst_parameter_list);
        }
        self.newline_pop();
        self.r_paren(&arg.r_paren);
    }

    /// Semantic action for non-terminal 'InstParameterList'
    fn inst_parameter_list(&mut self, arg: &InstParameterList) {
        self.inst_parameter_group(&arg.inst_parameter_group);
        for x in &arg.inst_parameter_list_list {
            self.comma(&x.comma);
            self.newline();
            self.inst_parameter_group(&x.inst_parameter_group);
        }
        if let Some(ref x) = arg.inst_parameter_list_opt {
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'InstParameterGroup'
    fn inst_parameter_group(&mut self, arg: &InstParameterGroup) {
        for x in &arg.inst_parameter_group_list {
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
        for _ in &arg.inst_parameter_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'InstParameterItem'
    fn inst_parameter_item(&mut self, arg: &InstParameterItem) {
        self.str(".");
        self.align_start(align_kind::IDENTIFIER);
        self.identifier(&arg.identifier);
        self.align_finish(align_kind::IDENTIFIER);
        self.space(1);
        self.str("(");
        if let Some(ref x) = arg.inst_parameter_item_opt {
            self.token(&x.colon.colon_token.replace(""));
            self.align_start(align_kind::EXPRESSION);
            self.expression(&x.expression);
            self.align_finish(align_kind::EXPRESSION);
        } else {
            self.emit_inst_param_port_item_assigned_by_name(&arg.identifier);
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
        for x in &arg.inst_port_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.inst_port_group_group {
            InstPortGroupGroup::LBraceInstPortListRBrace(x) => {
                self.inst_port_list(&x.inst_port_list);
            }
            InstPortGroupGroup::InstPortItem(x) => self.inst_port_item(&x.inst_port_item),
        }
        for _ in &arg.inst_port_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'InstPortItem'
    fn inst_port_item(&mut self, arg: &InstPortItem) {
        let modport_entry = if let Some(table) = self.modport_connections_tables.last_mut() {
            table.remove(&arg.identifier.identifier_token)
        } else {
            None
        };

        if let Some(x) = modport_entry {
            let src_line = self.src_line;
            self.aligner.disable_auto_finish();
            self.clear_adjust_line();

            for (i, connection) in x
                .connections
                .iter()
                .flat_map(|x| x.connections.iter())
                .enumerate()
            {
                if i > 0 {
                    self.str(",");
                    self.newline();
                }

                self.str(".");
                self.align_start(align_kind::IDENTIFIER);
                self.duplicated_token(&connection.port_target);
                self.align_finish(align_kind::IDENTIFIER);
                self.space(1);

                self.str("(");
                self.align_start(align_kind::EXPRESSION);
                self.duplicated_token(&connection.interface_target);
                self.align_finish(align_kind::EXPRESSION);
                self.str(")");

                self.clear_adjust_line();
            }

            self.aligner.enable_auto_finish();
            self.src_line = src_line;
        } else {
            self.str(".");
            self.align_start(align_kind::IDENTIFIER);
            self.emit_port_identifier(&arg.identifier);
            self.align_finish(align_kind::IDENTIFIER);
            self.space(1);
            self.str("(");
            if let Some(ref x) = arg.inst_port_item_opt {
                self.token(&x.colon.colon_token.replace(""));
                self.align_start(align_kind::EXPRESSION);
                self.expression(&x.expression);
                self.align_finish(align_kind::EXPRESSION);
            } else {
                self.emit_inst_param_port_item_assigned_by_name(&arg.identifier);
            }
            self.str(")");
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
            self.token(&x.comma.comma_token.replace(""));
        }
    }

    /// Semantic action for non-terminal 'WithParameterGroup'
    fn with_parameter_group(&mut self, arg: &WithParameterGroup) {
        for x in &arg.with_parameter_group_list {
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
        for _ in &arg.with_parameter_group_list {
            self.attribute_end();
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
        match &*arg.with_parameter_item_group0 {
            WithParameterItemGroup0::ArrayType(x) => {
                if !self.is_implicit_scalar_type(&x.array_type.scalar_type) {
                    self.scalar_type(&x.array_type.scalar_type);
                    self.space(1);
                } else {
                    self.align_start(align_kind::TYPE);
                    let loc = self.align_last_location(align_kind::PARAMETER);
                    self.align_dummy_location(align_kind::TYPE, loc);
                    self.align_finish(align_kind::TYPE);
                }
                self.align_start(align_kind::IDENTIFIER);
                self.identifier(&arg.identifier);
                self.align_finish(align_kind::IDENTIFIER);
                self.align_start(align_kind::ARRAY);
                if let Some(ref x) = x.array_type.array_type_opt {
                    self.space(1);
                    self.array(&x.array);
                } else {
                    let loc = self.align_last_location(align_kind::IDENTIFIER);
                    self.align_dummy_location(align_kind::ARRAY, loc);
                }
                self.align_finish(align_kind::ARRAY);
            }
            WithParameterItemGroup0::Type(x) => {
                self.align_start(align_kind::TYPE);
                if !self.is_implicit_type() {
                    self.r#type(&x.r#type);
                    self.space(1);
                } else {
                    let loc = self.align_last_location(align_kind::PARAMETER);
                    self.align_dummy_location(align_kind::TYPE, loc);
                }
                self.align_finish(align_kind::TYPE);
                self.align_start(align_kind::IDENTIFIER);
                self.identifier(&arg.identifier);
                self.align_finish(align_kind::IDENTIFIER);
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
        self.align_reset();
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
        for x in &arg.port_declaration_group_list {
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
        for _ in &arg.port_declaration_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        match &*arg.port_declaration_item_group {
            PortDeclarationItemGroup::PortTypeConcrete(x) => {
                let modport_entry = if let Some(table) = &self.modport_ports_table {
                    table.get(&arg.identifier.identifier_token.token)
                } else {
                    None
                };

                if let Some(entry) = modport_entry {
                    self.generic_map.push(entry.generic_maps.to_owned());

                    let src_line = self.src_line;
                    self.aligner.disable_auto_finish();
                    self.clear_adjust_line();
                    self.in_direction_with_var = true;

                    for (i, port) in entry.ports.iter().flat_map(|x| x.ports.iter()).enumerate() {
                        if i > 0 {
                            self.str(",");
                            self.newline();
                        }
                        let array_type = port.r#type.array_type.as_ref().unwrap();

                        self.align_start(align_kind::DIRECTION);
                        self.duplicated_token(&port.direction_token);
                        self.align_finish(align_kind::DIRECTION);
                        self.space(1);

                        self.scalar_type(&array_type.scalar_type);
                        self.space(1);

                        self.align_start(align_kind::IDENTIFIER);
                        self.duplicated_token(&port.identifier);
                        self.align_finish(align_kind::IDENTIFIER);

                        if let Some(ref x) = array_type.array_type_opt {
                            self.space(1);
                            self.array(&x.array);
                        } else {
                            let loc = self.align_last_location(align_kind::IDENTIFIER);
                            self.align_dummy_location(align_kind::ARRAY, loc);
                        }
                        self.align_finish(align_kind::ARRAY);

                        self.clear_adjust_line();
                    }

                    self.generic_map.pop();
                    self.aligner.enable_auto_finish();
                    self.src_line = src_line;
                    self.in_direction_with_var = false;
                } else {
                    let x = x.port_type_concrete.as_ref();
                    self.direction(&x.direction);
                    match x.direction.as_ref() {
                        Direction::Modport(_) => {
                            self.in_direction_modport = true;
                        }
                        Direction::Input(_) | Direction::Output(_) => {
                            self.in_direction_with_var = true;
                            self.space(1);
                        }
                        _ => {
                            self.space(1);
                        }
                    }
                    self.scalar_type(&x.array_type.scalar_type);
                    self.space(1);
                    self.align_start(align_kind::IDENTIFIER);
                    self.identifier(&arg.identifier);
                    self.align_finish(align_kind::IDENTIFIER);
                    self.align_start(align_kind::ARRAY);
                    if let Some(ref x) = x.array_type.array_type_opt {
                        self.space(1);
                        self.array(&x.array);
                    } else {
                        let loc = self.align_last_location(align_kind::IDENTIFIER);
                        self.align_dummy_location(align_kind::ARRAY, loc);
                    }
                    self.align_finish(align_kind::ARRAY);
                    self.in_direction_modport = false;
                    self.in_direction_with_var = false;
                }
            }
            PortDeclarationItemGroup::PortTypeAbstract(x) => {
                let x = x.port_type_abstract.as_ref();
                self.interface(&x.interface);
                if let Some(ref x) = x.port_type_abstract_opt0 {
                    self.str(".");
                    self.identifier(&x.identifier);
                }
                self.space(1);
                self.align_start(align_kind::IDENTIFIER);
                self.identifier(&arg.identifier);
                self.align_finish(align_kind::IDENTIFIER);
                self.align_start(align_kind::ARRAY);
                if let Some(ref x) = x.port_type_abstract_opt1 {
                    self.space(1);
                    self.array(&x.array);
                } else {
                    let loc = self.align_last_location(align_kind::IDENTIFIER);
                    self.align_dummy_location(align_kind::ARRAY, loc);
                }
                self.align_finish(align_kind::ARRAY);
            }
        }
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        if !matches!(arg, Direction::Modport(_)) {
            self.align_start(align_kind::DIRECTION);
        }
        match arg {
            Direction::Input(x) => self.input(&x.input),
            Direction::Output(x) => self.output(&x.output),
            Direction::Inout(x) => self.inout(&x.inout),
            Direction::Modport(_) => (),
            Direction::Import(x) => self.import(&x.import),
        };
        if !matches!(arg, Direction::Modport(_)) {
            self.align_finish(align_kind::DIRECTION);
        }
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = self.get_generic_maps(&symbol.found);

        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.push_generic_map(map.clone());

            if let SymbolKind::Function(ref x) = symbol.found.kind {
                let modport_ports_table = ExpandedModportPortTable::create(
                    &x.ports,
                    &self.get_generic_map(),
                    &arg.identifier.identifier_token,
                    &symbol.found.namespace,
                    true,
                    &self.into(),
                );
                if !modport_ports_table.is_empty() {
                    self.modport_ports_table = Some(modport_ports_table);
                }
            }

            self.emit_generic_instance_name_comment(map);
            self.function(&arg.function);
            self.space(1);
            self.str("automatic");
            self.space(1);
            if let Some(ref x) = arg.function_declaration_opt1 {
                self.emit_scalar_type(&x.scalar_type, false);
            } else {
                self.str("void");
            }
            self.space(1);
            if map.generic() {
                self.emit_generic_instance_name(&arg.identifier.identifier_token, map, true);
            } else {
                self.identifier(&arg.identifier);
            }
            if let Some(ref x) = arg.function_declaration_opt0 {
                self.port_declaration(&x.port_declaration);
                self.space(1);
            }
            if let Some(ref x) = arg.function_declaration_opt1 {
                self.token(&x.minus_g_t.minus_g_t_token.replace(""));
            }
            self.str(";");
            self.emit_statement_block(&arg.statement_block, "", "endfunction");

            self.pop_generic_map();
            self.align_reset();
        }

        self.modport_ports_table = None;
    }

    /// Semantic action for non-terminal 'ImportDeclaration'
    fn import_declaration(&mut self, arg: &ImportDeclaration) {
        if !self.in_generate_block.is_empty() {
            self.emit_import_declaration(arg, false);
        } else {
            // emit comments after import declaration which is moved
            self.clear_adjust_line();
            self.src_line = arg.semicolon.semicolon_token.token.line;
            self.process_comment(&arg.semicolon.semicolon_token, false);
        }
    }

    /// Semantic action for non-terminal 'UnsafeBlock'
    fn unsafe_block(&mut self, arg: &UnsafeBlock) {
        for (i, x) in arg.unsafe_block_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.generate_group(&x.generate_group);
        }
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let ports = if let SymbolKind::Module(ref x) = symbol.found.kind {
            self.default_clock = x.default_clock;
            self.default_reset = x.default_reset;
            x.ports.clone()
        } else {
            unreachable!()
        };
        let empty_header =
            arg.module_declaration_opt1.is_none() && arg.module_declaration_opt2.is_none();

        let maps = self.get_generic_maps(&symbol.found);
        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.push_generic_map(map.clone());

            let modport_ports_table = ExpandedModportPortTable::create(
                &ports,
                &self.get_generic_map(),
                &arg.identifier.identifier_token,
                &symbol.found.namespace,
                false,
                &self.into(),
            );
            if !modport_ports_table.is_empty() {
                self.modport_ports_table = Some(modport_ports_table);
            }

            self.emit_generic_instance_name_comment(map);
            self.module(&arg.module);
            self.space(1);
            if map.generic() {
                self.emit_generic_instance_name(&arg.identifier.identifier_token, map, false);
            } else {
                let context: SymbolContext = self.into();
                let text = format!(
                    "{}{}",
                    namespace_string(&symbol.found.namespace, &symbol.generic_tables, &context),
                    arg.identifier.identifier_token
                );
                self.veryl_token(&arg.identifier.identifier_token.replace(&text));
            }

            let mut import_declarations = self.file_scope_import.clone();
            import_declarations.append(&mut arg.collect_import_declarations());
            if !import_declarations.is_empty() && !empty_header {
                self.newline_push();
                for (i, x) in import_declarations.iter().enumerate() {
                    if i != 0 {
                        self.newline();
                    }
                    self.emit_import_declaration(x, true);
                }
                self.newline_pop();
            }

            if let Some(ref x) = arg.module_declaration_opt1 {
                if import_declarations.is_empty() {
                    self.space(1);
                }
                self.with_parameter(&x.with_parameter);
            }
            if let Some(ref x) = arg.module_declaration_opt2 {
                if import_declarations.is_empty() || arg.module_declaration_opt1.is_some() {
                    self.space(1);
                }
                self.port_declaration(&x.port_declaration);
            }
            self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
            for (i, x) in arg.module_declaration_list.iter().enumerate() {
                self.newline_list(i);
                if i == 0 && !import_declarations.is_empty() && empty_header {
                    for x in &import_declarations {
                        self.emit_import_declaration(x, true);
                        self.newline();
                    }
                }
                if i == 0 && self.modport_ports_table.is_some() {
                    self.emit_expanded_modport_connections();
                    self.modport_ports_table = None;
                }
                self.module_group(&x.module_group);
            }
            self.newline_list_post(arg.module_declaration_list.is_empty());
            self.token(&arg.r_brace.r_brace_token.replace("endmodule"));

            self.pop_generic_map();
            self.align_reset();
        }

        self.default_clock = None;
        self.default_reset = None;
    }

    /// Semantic action for non-terminal 'ModuleGroup'
    fn module_group(&mut self, arg: &ModuleGroup) {
        for x in &arg.module_group_list {
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
        for _ in &arg.module_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'InterfaceDeclaration'
    fn interface_declaration(&mut self, arg: &InterfaceDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let empty_header = arg.interface_declaration_opt1.is_none();

        let maps = self.get_generic_maps(&symbol.found);
        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.push_generic_map(map.clone());

            self.emit_generic_instance_name_comment(map);
            self.interface(&arg.interface);
            self.space(1);
            if map.generic() {
                self.emit_generic_instance_name(&arg.identifier.identifier_token, map, false);
            } else {
                let context: SymbolContext = self.into();
                let text = format!(
                    "{}{}",
                    namespace_string(&symbol.found.namespace, &symbol.generic_tables, &context),
                    arg.identifier.identifier_token
                );
                self.veryl_token(&arg.identifier.identifier_token.replace(&text));
            }

            let mut import_declarations = self.file_scope_import.clone();
            import_declarations.append(&mut arg.collect_import_declarations());
            if !import_declarations.is_empty() && !empty_header {
                self.newline_push();
                for (i, x) in import_declarations.iter().enumerate() {
                    if i != 0 {
                        self.newline();
                    }
                    self.emit_import_declaration(x, true);
                }
                self.newline_pop();
            }

            if let Some(ref x) = arg.interface_declaration_opt1 {
                if import_declarations.is_empty() {
                    self.space(1);
                }
                self.with_parameter(&x.with_parameter);
            }
            self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
            for (i, x) in arg.interface_declaration_list.iter().enumerate() {
                self.newline_list(i);
                if i == 0 && !import_declarations.is_empty() && empty_header {
                    for x in &import_declarations {
                        self.emit_import_declaration(x, true);
                        self.newline();
                    }
                }
                self.interface_group(&x.interface_group);
            }
            self.newline_list_post(arg.interface_declaration_list.is_empty());
            self.token(&arg.r_brace.r_brace_token.replace("endinterface"));

            self.pop_generic_map();
            self.align_reset();
        }
    }

    /// Semantic action for non-terminal 'InterfaceGroup'
    fn interface_group(&mut self, arg: &InterfaceGroup) {
        for x in &arg.interface_group_list {
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
        for _ in &arg.interface_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'GenerateIfDeclaration'
    fn generate_if_declaration(&mut self, arg: &GenerateIfDeclaration) {
        self.r#if(&arg.r#if);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.str(")");
        self.space(1);
        self.generate_named_block(&arg.generate_named_block);
        for x in &arg.generate_if_declaration_list {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.r#if(&x.r#if);
            self.space(1);
            self.str("(");
            self.expression(&x.expression);
            self.str(")");
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
        let ascending_order = arg.generate_for_declaration_opt.is_none();
        let include_end = if let Some(x) = &arg.range.range_opt {
            matches!(*x.range_operator, RangeOperator::DotDotEqu(_))
        } else {
            true
        };
        let (beg, end) = if let Some(x) = &arg.range.range_opt {
            if ascending_order {
                (&arg.range.expression, &x.expression)
            } else {
                (&x.expression, &arg.range.expression)
            }
        } else {
            (&arg.range.expression, &arg.range.expression)
        };

        self.r#for(&arg.r#for);
        self.space(1);
        self.str("(");
        self.str("genvar");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("=");
        self.space(1);
        self.expression(beg);
        if !ascending_order && !include_end {
            self.str(" - 1");
        }
        self.str(";");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        match (ascending_order, include_end) {
            (true, true) => self.str("<="),
            (true, false) => self.str("<"),
            _ => self.str(">="),
        }
        self.space(1);
        self.expression(end);
        self.str(";");
        self.space(1);
        if let Some(ref x) = arg.generate_for_declaration_opt0 {
            self.identifier(&arg.identifier);
            self.space(1);
            self.assignment_operator(&x.assignment_operator);
            self.space(1);
            self.expression(&x.expression);
        } else {
            self.identifier(&arg.identifier);
            if ascending_order {
                self.str("++");
            } else {
                self.str("--");
            }
        }
        self.str(")");
        self.space(1);
        self.generate_named_block(&arg.generate_named_block);
    }

    /// Semantic action for non-terminal 'GenerateBlockDeclaration'
    fn generate_block_declaration(&mut self, arg: &GenerateBlockDeclaration) {
        self.emit_generate_named_block(&arg.generate_named_block, "if (1) ");
    }

    /// Semantic action for non-terminal 'GenerateNamedBlock'
    fn generate_named_block(&mut self, arg: &GenerateNamedBlock) {
        self.emit_generate_named_block(arg, "");
    }

    /// Semantic action for non-terminal 'GenerateOptionalNamedBlock'
    fn generate_optional_named_block(&mut self, arg: &GenerateOptionalNamedBlock) {
        self.in_generate_block.push(());

        self.str("begin");
        if let Some(ref x) = arg.generate_optional_named_block_opt {
            self.space(1);
            self.colon(&x.colon);
            self.identifier(&x.identifier);
        } else {
            self.space(1);
            self.str(":");
            self.veryl_token(&self.default_block.clone().unwrap());
        }
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        for (i, x) in arg.generate_optional_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.generate_group(&x.generate_group);
        }
        self.newline_list_post(arg.generate_optional_named_block_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));

        self.in_generate_block.pop();
    }

    /// Semantic action for non-terminal 'GenerateGroup'
    fn generate_group(&mut self, arg: &GenerateGroup) {
        for x in &arg.generate_group_list {
            self.attribute(&x.attribute);
        }
        match &*arg.generate_group_group {
            GenerateGroupGroup::LBraceGenerateGroupGroupListRBrace(x) => {
                for (i, x) in x.generate_group_group_list.iter().enumerate() {
                    if i != 0 {
                        self.newline();
                    }
                    self.generate_group(&x.generate_group);
                }
            }
            GenerateGroupGroup::GenerateItem(x) => self.generate_item(&x.generate_item),
        }
        for _ in &arg.generate_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'PackageDeclaration'
    fn package_declaration(&mut self, arg: &PackageDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = self.get_generic_maps(&symbol.found);

        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.push_generic_map(map.clone());

            self.emit_generic_instance_name_comment(map);
            self.package(&arg.package);
            self.space(1);
            if map.generic() {
                self.emit_generic_instance_name(&arg.identifier.identifier_token, map, false);
            } else {
                let context: SymbolContext = self.into();
                let text = format!(
                    "{}{}",
                    namespace_string(&symbol.found.namespace, &symbol.generic_tables, &context),
                    arg.identifier.identifier_token
                );
                self.veryl_token(&arg.identifier.identifier_token.replace(&text));
            }
            self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
            for (i, x) in arg.package_declaration_list.iter().enumerate() {
                self.newline_list(i);
                if i == 0 {
                    let mut import_declarations = self.file_scope_import.clone();
                    import_declarations.append(&mut arg.collect_import_declarations());
                    for x in import_declarations {
                        self.emit_import_declaration(&x, true);
                        self.newline();
                    }
                }
                self.package_group(&x.package_group);
            }
            self.newline_list_post(arg.package_declaration_list.is_empty());
            self.token(&arg.r_brace.r_brace_token.replace("endpackage"));

            self.pop_generic_map();
            self.align_reset();
        }
    }

    /// Semantic action for non-terminal 'PackageGroup'
    fn package_group(&mut self, arg: &PackageGroup) {
        for x in &arg.package_group_list {
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
        for _ in &arg.package_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'AliasDeclaration'
    fn alias_declaration(&mut self, _arg: &AliasDeclaration) {
        // nothing to emit
    }

    /// Semantic action for non-terminal 'ProtoAliasDeclaration'
    fn proto_alias_declaration(&mut self, _arg: &ProtoAliasDeclaration) {
        // nothing to emit
    }

    /// Semantic action for non-terminal 'EmbedDeclaration'
    fn embed_declaration(&mut self, arg: &EmbedDeclaration) {
        if arg.identifier.identifier_token.to_string() == "inline" {
            let is_sv = arg.identifier0.identifier_token.to_string() == "sv";

            if is_sv {
                self.veryl_token(
                    &arg.embed_content
                        .triple_l_brace
                        .triple_l_brace_token
                        .replace("`ifndef SYNTHESIS"),
                );
            }

            self.keep_tail_newline = true;
            for x in &arg.embed_content.embed_content_list {
                self.embed_item(&x.embed_item);
            }
            self.keep_tail_newline = false;

            if is_sv {
                self.veryl_token(
                    &arg.embed_content
                        .triple_r_brace
                        .triple_r_brace_token
                        .replace("`endif"),
                );
            }
        }
    }

    /// Semantic action for non-terminal 'EmbedScopedIdentifier'
    fn embed_scoped_identifier(&mut self, arg: &EmbedScopedIdentifier) {
        self.scoped_identifier(&arg.scoped_identifier);
    }

    /// Semantic action for non-terminal 'IncludeDeclaration'
    fn include_declaration(&mut self, arg: &IncludeDeclaration) {
        if arg.identifier.identifier_token.to_string() == "inline" {
            let path = arg.string_literal.string_literal_token.to_string();
            let path = path.strip_prefix('"').unwrap();
            let path = path.strip_suffix('"').unwrap();
            if let TokenSource::File { path: x, .. } = arg.identifier.identifier_token.token.source
            {
                let base = resource_table::get_path_value(x).unwrap();
                let base = base.parent().unwrap();
                let path = base.join(path);
                // File existence is checked at analyzer
                let text = fs::read_to_string(path).unwrap();
                self.str(&text);
            }
        }
    }

    /// Semantic action for non-terminal 'DescriptionGroup'
    fn description_group(&mut self, arg: &DescriptionGroup) {
        for x in &arg.description_group_list {
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
        for _ in &arg.description_group_list {
            self.attribute_end();
        }
    }

    /// Semantic action for non-terminal 'DescriptionItem'
    fn description_item(&mut self, arg: &DescriptionItem) {
        match arg {
            DescriptionItem::DescriptionItemOptPublicDescriptionItem(x) => {
                self.public_description_item(&x.public_description_item);
            }
            // file scope import is not emitted at SystemVerilog
            DescriptionItem::ImportDeclaration(_) => (),
            DescriptionItem::BindDeclaration(x) => self.bind_declaration(&x.bind_declaration),
            DescriptionItem::EmbedDeclaration(x) => self.embed_declaration(&x.embed_declaration),
            DescriptionItem::IncludeDeclaration(x) => {
                self.include_declaration(&x.include_declaration)
            }
        };
    }

    /// Semantic action for non-terminal 'PublicDescriptionItem'
    fn public_description_item(&mut self, arg: &PublicDescriptionItem) {
        match arg {
            PublicDescriptionItem::ModuleDeclaration(x) => {
                self.module_declaration(&x.module_declaration)
            }
            PublicDescriptionItem::InterfaceDeclaration(x) => {
                self.interface_declaration(&x.interface_declaration)
            }
            PublicDescriptionItem::PackageDeclaration(x) => {
                self.package_declaration(&x.package_declaration)
            }
            // alias and proto are not emitted at SystemVerilog
            PublicDescriptionItem::AliasDeclaration(_)
            | PublicDescriptionItem::ProtoDeclaration(_) => (),
        };
    }

    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) {
        match self.mode {
            Mode::Emit => {
                self.in_start_token = true;
                self.start(&arg.start);
                self.in_start_token = false;
                if !arg.start.start_token.comments.is_empty() {
                    self.newline();
                }
                for x in &arg.veryl_list {
                    let items: Vec<_> = x.description_group.as_ref().into();
                    for item in items {
                        if let DescriptionItem::ImportDeclaration(x) = item {
                            self.file_scope_import
                                .push(x.import_declaration.as_ref().clone());
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

                // build map and insert link to map
                if self.build_opt.sourcemap_target != SourceMapTarget::None {
                    self.source_map.as_mut().unwrap().build();
                    self.str(&self.source_map.as_ref().unwrap().get_link());
                    self.newline();
                }
            }
            Mode::Align => {
                self.start(&arg.start);
                for x in &arg.veryl_list {
                    self.description_group(&x.description_group);
                }
            }
        }
    }
}

pub struct SymbolContext {
    pub project_name: Option<StrId>,
    pub build_opt: Build,
    pub in_import: bool,
    pub in_direction_modport: bool,
    pub generic_map: Vec<GenericMap>,
}

impl From<&mut Emitter> for SymbolContext {
    fn from(value: &mut Emitter) -> Self {
        let generic_map = if let Some(maps) = value.generic_map.last() {
            maps.clone()
        } else {
            Vec::new()
        };
        SymbolContext {
            project_name: value.project_name,
            build_opt: value.build_opt.clone(),
            in_import: value.in_import,
            in_direction_modport: value.in_direction_modport,
            generic_map,
        }
    }
}

fn namespace_string(
    namespace: &Namespace,
    generic_tables: &GenericTables,
    context: &SymbolContext,
) -> String {
    let mut ret = String::from("");
    let mut resolve_namespace = Namespace::new();
    let mut in_sv_namespace = false;
    for (i, path) in namespace.paths.iter().enumerate() {
        if i == 0 {
            // top level namespace is always `_`
            let text = format!("{path}_");

            // "$sv" namespace should be removed
            if text == "$sv_" {
                in_sv_namespace = true;
            } else {
                let emit_prj_prefix = if context.build_opt.omit_project_prefix {
                    context.project_name != Some(*path)
                } else {
                    true
                };

                if emit_prj_prefix {
                    ret.push_str(&text);
                }
            }
        } else {
            let symbol_path = SymbolPath::new(&[*path]);
            if let Ok(ref symbol) = symbol_table::resolve((&symbol_path, &resolve_namespace)) {
                let text = if let SymbolKind::GenericInstance(_) = symbol.found.kind {
                    generic_instance_namespace_string(&symbol.found, context)
                } else if let Some(symbol) = get_generic_instance(&symbol.found, generic_tables) {
                    generic_instance_namespace_string(&symbol, context)
                } else {
                    let separator = namespace_separator(
                        &symbol.found,
                        context.in_direction_modport,
                        in_sv_namespace,
                    );
                    format!("{path}{separator}")
                };
                ret.push_str(&text);
            } else {
                return format!("{namespace}");
            }
        }

        resolve_namespace.push(*path);
    }

    ret.replace("$std_", "__std_")
}

fn generic_instance_namespace_string(symbol: &Symbol, context: &SymbolContext) -> String {
    let SymbolKind::GenericInstance(ref inst) = symbol.kind else {
        unreachable!()
    };

    let base = symbol_table::get(inst.base).unwrap();
    let separator = namespace_separator(&base, context.in_direction_modport, false);
    if context.build_opt.hashed_mangled_name {
        let name = symbol
            .generic_maps()
            .first()
            .map(|x| x.name(false, true))
            .unwrap();
        format!("{name}{separator}")
    } else {
        format!("{}{}", symbol.token, separator)
    }
}

fn namespace_separator(
    symbol: &Symbol,
    in_direction_modport: bool,
    in_sv_namespace: bool,
) -> String {
    let separator = match symbol.kind {
        SymbolKind::Package(_) => "::",
        SymbolKind::Interface(_) => ".",
        SymbolKind::SystemVerilog if in_direction_modport => ".",
        _ if in_sv_namespace => "::",
        _ => "_",
    };
    separator.to_string()
}

fn get_generic_instance(symbol: &Symbol, generic_tables: &GenericTables) -> Option<Symbol> {
    let table = generic_tables.get(&symbol.inner_namespace())?;
    let params = symbol.generic_parameters();
    if params.is_empty() {
        return None;
    }

    let mut path: GenericSymbolPath = (&symbol.token).into();
    for (param, default_value) in &params {
        let arg = if let Some(x) = table.get(param) {
            x
        } else {
            default_value.default_value.as_ref().unwrap()
        };
        path.paths[0].arguments.push(arg.clone());
    }

    symbol_table::resolve((&path.mangled_path(), &symbol.namespace))
        .ok()
        .map(|x| x.found)
}

pub fn symbol_string(
    token: &VerylToken,
    symbol: &Symbol,
    symbol_namespace: &Namespace,
    full_path: &[SymbolId],
    generic_tables: &GenericTables,
    context: &SymbolContext,
    scope_depth: usize,
) -> String {
    let mut ret = String::new();
    let namespace = namespace_table::get(token.token.id).unwrap();

    let token_text = symbol.token.to_string();
    let token_text = if let Some(text) = token_text.strip_prefix("r#") {
        text.to_string()
    } else {
        token_text
    };

    match &symbol.kind {
        SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package(_) => {
            ret.push_str(&namespace_string(
                &symbol.namespace,
                generic_tables,
                context,
            ));
            ret.push_str(&token_text);
        }
        SymbolKind::Parameter(_)
        | SymbolKind::Function(_)
        | SymbolKind::Struct(_)
        | SymbolKind::Union(_)
        | SymbolKind::TypeDef(_)
        | SymbolKind::Enum(_) => {
            let visible = namespace.included(symbol_namespace)
                || symbol.imported.iter().any(|x| x.namespace == namespace);
            if (scope_depth == 1) & visible & !context.in_import {
                ret.push_str(&token_text);
            } else {
                ret.push_str(&namespace_string(symbol_namespace, generic_tables, context));
                ret.push_str(&token_text);
            }
        }
        SymbolKind::EnumMember(x) => {
            let mut enum_namespace = symbol_namespace.clone();
            enum_namespace.pop();

            // if enum definition is scoped or it is not visible, explicit namespace is required
            if scope_depth >= 3 || !namespace.included(&enum_namespace) {
                ret.push_str(&namespace_string(&enum_namespace, generic_tables, context));
            }
            ret.push_str(&x.prefix);
            ret.push('_');
            ret.push_str(&token_text);
        }
        SymbolKind::Modport(_) => {
            ret.push_str(&namespace_string(
                &symbol.namespace,
                generic_tables,
                context,
            ));
            ret.push_str(&token_text);
        }
        SymbolKind::SystemVerilog => {
            ret.push_str(&namespace_string(
                &symbol.namespace,
                generic_tables,
                context,
            ));
            ret.push_str(&token_text);
        }
        SymbolKind::GenericInstance(x) => {
            let base = symbol_table::get(x.base).unwrap();
            let visible = namespace.included(&base.namespace)
                || base.imported.iter().any(|x| x.namespace == namespace);
            let top_level = matches!(
                base.kind,
                SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package(_)
            );
            let add_namespace = (scope_depth >= 2) | !visible | top_level;
            if add_namespace {
                ret.push_str(&namespace_string(symbol_namespace, generic_tables, context));
            }
            if context.build_opt.hashed_mangled_name {
                let name = symbol
                    .generic_maps()
                    .first()
                    .map(|x| x.name(!add_namespace, true))
                    .unwrap();
                ret.push_str(&name);
            } else {
                ret.push_str(&token_text);
            }
        }
        SymbolKind::GenericParameter(_)
        | SymbolKind::ProtoModule(_)
        | SymbolKind::ProtoInterface(_)
        | SymbolKind::ProtoPackage(_) => (),
        SymbolKind::Port(x) => {
            if let Some(ref x) = x.prefix {
                ret.push_str(x);
            }
            ret.push_str(&token_text);
            if let Some(ref x) = x.suffix {
                ret.push_str(x);
            }
        }
        SymbolKind::Variable(x) => {
            if let Some(ref x) = x.prefix {
                ret.push_str(x);
            }
            ret.push_str(&token_text);
            if let Some(ref x) = x.suffix {
                ret.push_str(x);
            }
        }
        SymbolKind::StructMember(_) | SymbolKind::UnionMember(_) => {
            // for this case, struct member/union member is given as generic argument
            let symbols: Vec<_> = full_path
                .iter()
                .enumerate()
                .filter(|(i, _)| (i + 1) < full_path.len())
                .map(|(_, id)| symbol_table::get(*id).unwrap())
                .collect();
            for (i, symbol) in symbols.iter().enumerate() {
                if !(matches!(symbol.kind, SymbolKind::Namespace) || symbol.is_package(false)) {
                    let symbol_namespace = if i == 0 {
                        &symbol.namespace
                    } else {
                        &symbols[i - 1].inner_namespace()
                    };
                    let text = symbol_string(
                        token,
                        symbol,
                        symbol_namespace,
                        &[],
                        &GenericTables::default(),
                        context,
                        1,
                    );
                    ret.push_str(&text);
                    ret.push('.');
                }
            }
            ret.push_str(&token_text);
        }
        SymbolKind::Instance(_)
        | SymbolKind::Block
        | SymbolKind::ModportVariableMember(_)
        | SymbolKind::ModportFunctionMember(_)
        | SymbolKind::Genvar
        | SymbolKind::Namespace
        | SymbolKind::SystemFunction(_) => ret.push_str(&token_text),
        SymbolKind::AliasModule(_)
        | SymbolKind::ProtoAliasModule(_)
        | SymbolKind::AliasInterface(_)
        | SymbolKind::ProtoAliasInterface(_)
        | SymbolKind::AliasPackage(_)
        | SymbolKind::ProtoAliasPackage(_)
        | SymbolKind::ClockDomain
        | SymbolKind::EnumMemberMangled
        | SymbolKind::ProtoConst(_)
        | SymbolKind::ProtoTypeDef(_)
        | SymbolKind::ProtoFunction(_)
        | SymbolKind::Test(_)
        | SymbolKind::Embed => {
            unreachable!()
        }
    }

    ret.replace("$std_", "__std_")
}

fn get_variable_type_kind(symbol: &Symbol) -> Option<TypeKind> {
    match &symbol.kind {
        SymbolKind::Port(x) => Some(x.r#type.kind.clone()),
        SymbolKind::Variable(x) => Some(x.r#type.kind.clone()),
        SymbolKind::ModportVariableMember(x) => {
            let symbol = symbol_table::get(x.variable).unwrap();
            get_variable_type_kind(&symbol)
        }
        _ => None,
    }
}

fn get_variable_prefix_suffix(symbol: &Symbol) -> (Option<String>, Option<String>) {
    match &symbol.kind {
        SymbolKind::Port(x) => (x.prefix.clone(), x.suffix.clone()),
        SymbolKind::Variable(x) => (x.prefix.clone(), x.suffix.clone()),
        SymbolKind::ModportVariableMember(x) => {
            let symbol = symbol_table::get(x.variable).unwrap();
            get_variable_prefix_suffix(&symbol)
        }
        _ => (None, None),
    }
}

fn identifier_token_with_prefix_suffix(
    token: &VerylToken,
    prefix: &Option<String>,
    suffix: &Option<String>,
) -> VerylToken {
    if prefix.is_some() || suffix.is_some() {
        let token = token.strip_prefix("r#");
        token.append(prefix, suffix)
    } else {
        token.strip_prefix("r#")
    }
}

fn emitting_identifier_token(token: &VerylToken, symbol: Option<&Symbol>) -> VerylToken {
    let (prefix, suffix) = if let Some(symbol) = symbol {
        get_variable_prefix_suffix(symbol)
    } else {
        (None, None)
    };
    identifier_token_with_prefix_suffix(token, &prefix, &suffix)
}

pub fn resolve_generic_path(
    path: &GenericSymbolPath,
    namespace: &Namespace,
    generic_maps: Option<&Vec<GenericMap>>,
) -> (Result<ResolveResult, ResolveError>, GenericSymbolPath) {
    let mut path = path.clone();

    path.resolve_imported(namespace, generic_maps);
    if let Some(maps) = generic_maps {
        path.apply_map(maps);
    }
    path.unalias();

    let path_symbols: Vec<_> = (0..path.len())
        .filter_map(|i| {
            symbol_table::resolve((&path.slice(i).generic_path(), namespace))
                .map(|x| (i, x.found))
                .ok()
        })
        .collect();

    for (i, symbol) in &path_symbols {
        if symbol.kind.is_generic() {
            let params = symbol.generic_parameters();
            let n_args = path.paths[*i].arguments.len();

            // Apply default value
            for param in params.iter().skip(n_args) {
                let mut arg = param.1.default_value.as_ref().unwrap().clone();
                arg.unalias();
                path.paths[*i].arguments.push(arg);
            }

            for arg in path.paths[*i].arguments.iter_mut() {
                if let Some(maps) = generic_maps {
                    arg.apply_map(maps);
                }
                arg.unalias();
                arg.append_namespace_path(namespace, &symbol.namespace);
            }
        }
    }

    let result = symbol_table::resolve((&path.mangled_path(), namespace));
    if let Ok(symbol) = &result
        && let Some(target) = symbol.found.alias_target(false)
    {
        if let Some(parent) = symbol.found.get_parent()
            && matches!(parent.kind, SymbolKind::GenericInstance(_))
        {
            // Alias target may be a generic parameter if it is defined in a generic package.
            // Need to apply parent's generic map to resolve a generic parameter.
            let map = parent.generic_maps();
            return resolve_generic_path(&target, &symbol.found.namespace, Some(&map));
        }
        return resolve_generic_path(&target, &symbol.found.namespace, generic_maps);
    }

    (result, path)
}
