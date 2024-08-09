use crate::aligner::{Aligner, Location};
use std::fs;
use std::path::Path;
use veryl_analyzer::attribute::Attribute as Attr;
use veryl_analyzer::attribute_table;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::TypeModifier as SymTypeModifier;
use veryl_analyzer::symbol::{GenericMap, Symbol, SymbolId, SymbolKind, TypeKind};
use veryl_analyzer::symbol_path::{GenericSymbolPath, SymbolPath};
use veryl_analyzer::symbol_table;
use veryl_analyzer::{msb_table, namespace_table};
use veryl_metadata::{Build, BuiltinType, ClockType, Format, Metadata, ResetType, SourceMapTarget};
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenSource, VerylToken};
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::Stringifier;
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

pub struct Emitter {
    project_name: Option<StrId>,
    build_opt: Build,
    format_opt: Format,
    string: String,
    indent: usize,
    src_line: u32,
    dst_line: u32,
    dst_column: u32,
    aligner: Aligner,
    in_start_token: bool,
    consumed_next_newline: bool,
    single_line: bool,
    adjust_line: bool,
    case_item_indent: Option<usize>,
    in_always_ff: bool,
    in_generate: bool,
    in_direction_modport: bool,
    in_import: bool,
    signed: bool,
    default_clock: Option<SymbolId>,
    default_reset: Option<SymbolId>,
    reset_signal: Option<String>,
    default_block: Option<String>,
    enum_member_prefix: Option<String>,
    file_scope_import: Vec<String>,
    attribute: Vec<AttributeType>,
    assignment_lefthand_side: Option<ExpressionIdentifier>,
    generic_map: Vec<GenericMap>,
    source_map: Option<SourceMap>,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            project_name: None,
            build_opt: Build::default(),
            format_opt: Format::default(),
            string: String::new(),
            indent: 0,
            src_line: 1,
            dst_line: 1,
            dst_column: 1,
            aligner: Aligner::new(),
            in_start_token: false,
            consumed_next_newline: false,
            single_line: false,
            adjust_line: false,
            case_item_indent: None,
            in_always_ff: false,
            in_generate: false,
            in_direction_modport: false,
            in_import: false,
            signed: false,
            default_clock: None,
            default_reset: None,
            reset_signal: None,
            default_block: None,
            enum_member_prefix: None,
            file_scope_import: Vec::new(),
            attribute: Vec::new(),
            assignment_lefthand_side: None,
            generic_map: Vec::new(),
            source_map: None,
        }
    }
}

impl Emitter {
    pub fn new(metadata: &Metadata, src_path: &Path, dst_path: &Path, map_path: &Path) -> Self {
        let mut aligner = Aligner::new();
        aligner.set_metadata(metadata);

        let source_map = SourceMap::new(src_path, dst_path, map_path);

        Self {
            project_name: Some(metadata.project.name.as_str().into()),
            build_opt: metadata.build.clone(),
            format_opt: metadata.format.clone(),
            aligner,
            source_map: Some(source_map),
            ..Default::default()
        }
    }

    pub fn emit(&mut self, project_name: &str, input: &Veryl) {
        namespace_table::set_default(&[project_name.into()]);
        self.aligner.align(input);
        self.veryl(input);
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    pub fn source_map(&mut self) -> &mut SourceMap {
        self.source_map.as_mut().unwrap()
    }

    fn str(&mut self, x: &str) {
        self.string.push_str(x);

        let new_lines = x.matches('\n').count() as u32;
        self.dst_line += new_lines;
        if new_lines == 0 {
            self.dst_column += x.len() as u32;
        } else {
            self.dst_column = (x.len() - x.rfind('\n').unwrap_or(0)) as u32;
        }
    }

    fn truncate(&mut self, x: usize) {
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
        if self
            .string
            .ends_with(&" ".repeat(self.indent * self.format_opt.indent_width))
        {
            self.truncate(self.string.len() - self.indent * self.format_opt.indent_width);
        }
    }

    fn indent(&mut self) {
        self.str(&" ".repeat(
            self.indent * self.format_opt.indent_width + self.case_item_indent.unwrap_or(0),
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
        let text = if text.ends_with('\n') {
            self.consumed_next_newline = true;
            text.trim_end()
        } else {
            &text
        };

        if x.line != 0 && x.column != 0 {
            if let Some(ref mut map) = self.source_map {
                map.add(self.dst_line, self.dst_column, x.line, x.column, text);
            }
        }

        let newlines_in_text = text.matches('\n').count() as u32;
        self.str(text);
        self.src_line = x.line + newlines_in_text;
    }

    fn process_token(&mut self, x: &VerylToken, will_push: bool, duplicated: Option<usize>) {
        self.push_token(&x.token);

        let mut loc: Location = x.token.into();
        loc.duplicated = duplicated;
        if let Some(width) = self.aligner.additions.get(&loc) {
            self.space(*width as usize);
        }

        // skip to emit comments
        if duplicated.is_some() || self.build_opt.strip_comments {
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
        self.process_token(x, false, None)
    }

    fn token_will_push(&mut self, x: &VerylToken) {
        self.process_token(x, true, None)
    }

    fn duplicated_token(&mut self, x: &VerylToken, i: usize) {
        self.process_token(x, false, Some(i))
    }

    fn case_inside_statement(&mut self, arg: &CaseStatement) {
        self.case(&arg.case);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(") inside"));
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.case_inside_item(&x.case_item);
        }
        self.newline_list_post(arg.case_statement_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("endcase"));
    }

    fn case_inside_item(&mut self, arg: &CaseItem) {
        let start = self.dst_column;
        match &*arg.case_item_group {
            CaseItemGroup::CaseCondition(x) => {
                self.range_item(&x.case_condition.range_item);
                for x in &x.case_condition.case_condition_list {
                    self.comma(&x.comma);
                    self.space(1);
                    self.range_item(&x.range_item);
                }
            }
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.colon(&arg.colon);
        self.space(1);
        self.case_item_indent = Some((self.dst_column - start) as usize);
        self.case_item_statement(&arg.case_item_group0);
        self.case_item_indent = None;
    }

    fn case_expaneded_statement(&mut self, arg: &CaseStatement) {
        self.case(&arg.case);
        self.space(1);
        self.str("(");
        self.str("1'b1");
        self.token_will_push(&arg.l_brace.l_brace_token.replace(")"));
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.case_expanded_item(&arg.expression, &x.case_item);
        }
        self.newline_list_post(arg.case_statement_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("endcase"));
    }

    fn case_expanded_item(&mut self, lhs: &Expression, item: &CaseItem) {
        let start: u32 = self.dst_column;
        match &*item.case_item_group {
            CaseItemGroup::CaseCondition(x) => {
                self.inside_element_opration(lhs, &x.case_condition.range_item);
                for x in &x.case_condition.case_condition_list {
                    self.comma(&x.comma);
                    self.space(1);
                    self.inside_element_opration(lhs, &x.range_item);
                }
            }
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.colon(&item.colon);
        self.space(1);
        self.case_item_indent = Some((self.dst_column - start) as usize);
        self.case_item_statement(&item.case_item_group0);
        self.case_item_indent = None;
    }

    fn case_item_statement(&mut self, arg: &CaseItemGroup0) {
        match arg {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::LBraceCaseItemGroup0ListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
                let mut base = 0;
                for (i, x) in x
                    .case_item_group0_list
                    .iter()
                    .filter(|x| is_let_statement(&x.statement))
                    .enumerate()
                {
                    self.newline_list(i);
                    base += self.statement_variable_declatation_only(&x.statement);
                }
                for (i, x) in x.case_item_group0_list.iter().enumerate() {
                    self.newline_list(base + i);
                    self.statement(&x.statement);
                }
                self.newline_list_post(x.case_item_group0_list.is_empty());
                self.token(&x.r_brace.r_brace_token.replace("end"));
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
            self.inside_element_opration(lhs, rhs);
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
        self.inside_element_opration(&arg.expression, &arg.range_list.range_item);
        for x in &arg.range_list.range_list_list {
            self.str(" || ");
            self.inside_element_opration(&arg.expression, &x.range_item);
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
        self.inside_element_opration(&arg.expression, &arg.range_list.range_item);
        for x in &arg.range_list.range_list_list {
            self.str(" || ");
            self.inside_element_opration(&arg.expression, &x.range_item);
        }
        self.str(")");
    }

    fn inside_element_opration(&mut self, lhs: &Expression, rhs: &RangeItem) {
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
            SymbolKind::Port(x) => (
                x.r#type.clone().unwrap().kind,
                x.prefix.clone(),
                x.suffix.clone(),
            ),
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
        if let Some(x) = arg.always_ff_declaration_list.first() {
            matches!(&*x.statement, Statement::IfResetStatement(_))
        } else {
            false
        }
    }

    fn always_ff_implicit_reset_event(&mut self) {
        let symbol = symbol_table::get(self.default_reset.unwrap()).unwrap();
        let (reset_kind, prefix, suffix) = match symbol.kind {
            SymbolKind::Port(x) => (
                x.r#type.clone().unwrap().kind,
                x.prefix.clone(),
                x.suffix.clone(),
            ),
            SymbolKind::Variable(x) => (x.r#type.kind, x.prefix.clone(), x.suffix.clone()),
            _ => unreachable!(),
        };
        let reset_type = match reset_kind {
            TypeKind::ResetAsyncHigh => ResetType::AsyncHigh,
            TypeKind::ResetAsyncLow => ResetType::AsyncLow,
            TypeKind::ResetSyncHigh => ResetType::SyncHigh,
            TypeKind::ResetSyncLow => ResetType::SyncLow,
            TypeKind::Reset => self.build_opt.reset_type,
            _ => unreachable!(),
        };

        let token = if prefix.is_some() || suffix.is_some() {
            VerylToken::new(symbol.token).append(&prefix, &suffix).token
        } else {
            symbol.token
        };

        let prefix_op = match reset_type {
            ResetType::AsyncHigh => {
                self.str(",");
                self.space(1);
                self.str("posedge");
                self.space(1);
                self.str(&token.to_string());
                ""
            }
            ResetType::AsyncLow => {
                self.str(",");
                self.space(1);
                self.str("negedge");
                self.space(1);
                self.str(&token.to_string());
                "!"
            }
            ResetType::SyncHigh => "",
            ResetType::SyncLow => "!",
        };

        self.reset_signal = Some(format!("{}{}", prefix_op, token));
    }

    fn always_ff_reset_exist_in_sensitivity_list(&mut self, arg: &AlwaysFfReset) -> bool {
        if let Ok(found) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
            let reset_kind = match found.found.kind {
                SymbolKind::Port(x) => x.r#type.clone().unwrap().kind,
                SymbolKind::Variable(x) => x.r#type.kind,
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

    fn statement_variable_declatation_only(&mut self, arg: &Statement) -> usize {
        self.clear_adjust_line();
        match arg {
            Statement::LetStatement(x) => {
                let x = &x.let_statement;
                self.scalar_type(&x.array_type.scalar_type);
                self.space(1);
                self.identifier(&x.identifier);
                if let Some(ref x) = x.array_type.array_type_opt {
                    self.space(1);
                    self.array(&x.array);
                }
                self.str(";");
                1
            }
            _ => 0,
        }
    }

    fn function_item_variable_declatation_only(&mut self, arg: &FunctionItem) -> usize {
        self.clear_adjust_line();
        match arg {
            FunctionItem::VarDeclaration(x) => {
                self.var_declaration(&x.var_declaration);
                1
            }
            FunctionItem::Statement(x) => self.statement_variable_declatation_only(&x.statement),
        }
    }

    fn function_item_statement_only(&mut self, arg: &FunctionItem) {
        match arg {
            FunctionItem::VarDeclaration(_) => (),
            FunctionItem::Statement(x) => self.statement(&x.statement),
        };
    }
}

fn is_let_statement(arg: &Statement) -> bool {
    matches!(arg, Statement::LetStatement(_))
}

fn is_func_let_statement(arg: &FunctionItem) -> bool {
    match arg {
        FunctionItem::VarDeclaration(_) => true,
        FunctionItem::Statement(x) => is_let_statement(&x.statement),
    }
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
        let base = &tail[0..1];
        let number = &tail[1..];

        if width.is_empty() {
            let base_num = match base {
                "b" => 2,
                "o" => 8,
                "d" => 10,
                "h" => 16,
                _ => unreachable!(),
            };

            if let Some(actual_width) = strnum_bitwidth::bitwidth(number, base_num) {
                let text = format!("{actual_width}'{base}{number}");
                self.veryl_token(&arg.based_token.replace(&text));
            } else {
                unreachable!()
            }
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

            self.veryl_token(&arg.comma_token);
            self.str("`endif");
            for _ in 0..additional_endif {
                self.newline();
                self.str("`endif");
            }
        } else {
            self.veryl_token(&arg.comma_token);
        }
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
        self.veryl_token(&arg.f32_token.replace("shortreal"));
    }

    /// Semantic action for non-terminal 'F64'
    fn f64(&mut self, arg: &F64) {
        self.veryl_token(&arg.f64_token.replace("real"));
    }

    /// Semantic action for non-terminal 'I32'
    fn i32(&mut self, arg: &I32) {
        self.veryl_token(&arg.i32_token.replace("int signed"));
    }

    /// Semantic action for non-terminal 'I64'
    fn i64(&mut self, arg: &I64) {
        self.veryl_token(&arg.i64_token.replace("longint signed"));
    }

    /// Semantic action for non-terminal 'Local'
    fn local(&mut self, arg: &Local) {
        self.veryl_token(&arg.local_token.replace("localparam"));
    }

    /// Semantic action for non-terminal 'Lsb'
    fn lsb(&mut self, arg: &Lsb) {
        self.token(&arg.lsb_token.replace("0"));
    }

    /// Semantic action for non-terminal 'Msb'
    fn msb(&mut self, arg: &Msb) {
        let expression = msb_table::get(arg.msb_token.token.id).unwrap();
        self.str("((");
        self.expression(&expression);
        self.str(") - 1)");
    }

    /// Semantic action for non-terminal 'Param'
    fn param(&mut self, arg: &Param) {
        self.veryl_token(&arg.param_token.replace("parameter"));
    }

    /// Semantic action for non-terminal 'Switch'
    fn switch(&mut self, arg: &Switch) {
        self.veryl_token(&arg.switch_token.replace("case"));
    }

    /// Semantic action for non-terminal 'U32'
    fn u32(&mut self, arg: &U32) {
        self.veryl_token(&arg.u32_token.replace("int unsigned"));
    }

    /// Semantic action for non-terminal 'U64'
    fn u64(&mut self, arg: &U64) {
        self.veryl_token(&arg.u64_token.replace("longint unsigned"));
    }

    /// Semantic action for non-terminal 'Identifier'
    fn identifier(&mut self, arg: &Identifier) {
        let (prefix, suffix) = if let Ok(found) = symbol_table::resolve(arg) {
            match &found.found.kind {
                SymbolKind::Port(x) => (x.prefix.clone(), x.suffix.clone()),
                SymbolKind::Variable(x) => (x.prefix.clone(), x.suffix.clone()),
                _ => (None, None),
            }
        } else {
            (None, None)
        };
        self.veryl_token(&identifier_with_prefix_suffix(arg, &prefix, &suffix));
    }

    /// Semantic action for non-terminal 'HierarchicalIdentifier'
    fn hierarchical_identifier(&mut self, arg: &HierarchicalIdentifier) {
        let list_len = &arg.hierarchical_identifier_list0.len();
        let (prefix, suffix) = if let Ok(found) = symbol_table::resolve(arg) {
            match &found.found.kind {
                SymbolKind::Port(x) => (x.prefix.clone(), x.suffix.clone()),
                SymbolKind::Variable(x) => (x.prefix.clone(), x.suffix.clone()),
                _ => (None, None),
            }
        } else {
            unreachable!()
        };

        if *list_len == 0 {
            self.veryl_token(&identifier_with_prefix_suffix(
                &arg.identifier,
                &prefix,
                &suffix,
            ));
        } else {
            self.identifier(&arg.identifier);
        }

        for x in &arg.hierarchical_identifier_list {
            self.select(&x.select);
        }

        for (i, x) in arg.hierarchical_identifier_list0.iter().enumerate() {
            self.dot(&x.dot);
            if (i + 1) == *list_len {
                self.veryl_token(&identifier_with_prefix_suffix(
                    &x.identifier,
                    &prefix,
                    &suffix,
                ));
            } else {
                self.identifier(&x.identifier);
            }
            for x in &x.hierarchical_identifier_list0_list {
                self.select(&x.select);
            }
        }
    }

    /// Semantic action for non-terminal 'Operator07'
    fn operator07(&mut self, arg: &Operator07) {
        match arg.operator07_token.to_string().as_str() {
            "<:" => self.str("<"),
            ">:" => self.str(">"),
            _ => self.veryl_token(&arg.operator07_token),
        }
    }

    /// Semantic action for non-terminal 'ScopedIdentifier'
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) {
        let namespace = namespace_table::get(arg.identifier().token.id).unwrap();
        let mut path: GenericSymbolPath = arg.into();

        for i in 0..path.len() {
            let base_path = path.base_path(i);
            if let Ok(symbol) = symbol_table::resolve((&base_path, &namespace)) {
                let params = symbol.found.generic_parameters();
                let n_args = path.paths[i].arguments.len();

                for param in params.iter().skip(n_args) {
                    path.paths[i]
                        .arguments
                        .push(param.1.as_ref().unwrap().clone());
                }
            }
        }

        path.apply_map(&self.generic_map);
        if let Ok(symbol) = symbol_table::resolve((&path.mangled_path(), &namespace)) {
            let context: SymbolContext = self.into();
            let text = symbol_string(arg.identifier(), &symbol.found, &context);
            self.veryl_token(&arg.identifier().replace(&text));
        } else if !path.is_resolvable() {
            // emit literal by generics
            let text = path.base_path(0).0[0].to_string();
            self.veryl_token(&arg.identifier().replace(&text));
        }
    }

    /// Semantic action for non-terminal 'ExpressionIdentifier'
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) {
        self.scoped_identifier(&arg.scoped_identifier);
        for x in &arg.expression_identifier_list {
            self.select(&x.select);
        }
        for x in &arg.expression_identifier_list0 {
            self.dot(&x.dot);
            self.identifier(&x.identifier);
            for x in &x.expression_identifier_list0_list {
                self.select(&x.select);
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
        self.l_bracket(&arg.l_bracket);
        self.str("0:");
        self.expression(&arg.expression);
        self.str("-1");
        for x in &arg.array_list {
            self.token(&x.comma.comma_token.replace("]["));
            self.str("0:");
            self.expression(&x.expression);
            self.str("-1");
        }
        self.r_bracket(&arg.r_bracket);
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

    /// Semantic action for non-terminal 'VariableType'
    fn variable_type(&mut self, arg: &VariableType) {
        match &*arg.variable_type_group {
            VariableTypeGroup::Clock(x) => self.clock(&x.clock),
            VariableTypeGroup::ClockPosedge(x) => self.clock_posedge(&x.clock_posedge),
            VariableTypeGroup::ClockNegedge(x) => self.clock_negedge(&x.clock_negedge),
            VariableTypeGroup::Reset(x) => self.reset(&x.reset),
            VariableTypeGroup::ResetAsyncHigh(x) => self.reset_async_high(&x.reset_async_high),
            VariableTypeGroup::ResetAsyncLow(x) => self.reset_async_low(&x.reset_async_low),
            VariableTypeGroup::ResetSyncHigh(x) => self.reset_sync_high(&x.reset_sync_high),
            VariableTypeGroup::ResetSyncLow(x) => self.reset_sync_low(&x.reset_sync_low),
            VariableTypeGroup::Logic(x) => self.logic(&x.logic),
            VariableTypeGroup::Bit(x) => self.bit(&x.bit),
            VariableTypeGroup::ScopedIdentifier(x) => self.scoped_identifier(&x.scoped_identifier),
        };
        if self.signed {
            self.space(1);
            self.str("signed");
        }
        if let Some(ref x) = arg.variable_type_opt {
            self.space(1);
            self.width(&x.width);
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
        }
    }

    /// Semantic action for non-terminal 'ScalarType'
    fn scalar_type(&mut self, arg: &ScalarType) {
        for x in &arg.scalar_type_list {
            self.type_modifier(&x.type_modifier);
        }
        match &*arg.scalar_type_group {
            ScalarTypeGroup::VariableType(x) => self.variable_type(&x.variable_type),
            ScalarTypeGroup::FixedType(x) => self.fixed_type(&x.fixed_type),
        }
        self.signed = false;
    }

    /// Semantic action for non-terminal 'LetStatement'
    fn let_statement(&mut self, arg: &LetStatement) {
        // Variable declaration is moved to statement_variable_declatation_only
        //self.scalar_type(&arg.array_type.scalar_type);
        //self.space(1);
        //self.identifier(&arg.identifier);
        //if let Some(ref x) = arg.array_type.array_type_opt {
        //    self.space(1);
        //    self.array(&x.array);
        //}
        //self.str(";");
        //self.newline();
        self.str(&arg.identifier.identifier_token.to_string());
        self.space(1);
        self.equ(&arg.equ);
        self.space(1);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'IdentifierStatement'
    fn identifier_statement(&mut self, arg: &IdentifierStatement) {
        self.expression_identifier(&arg.expression_identifier);
        self.assignment_lefthand_side = Some(*arg.expression_identifier.clone());
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
        if self.in_always_ff {
            self.str("<");
            match &*arg.assignment_group {
                AssignmentGroup::Equ(x) => self.equ(&x.equ),
                AssignmentGroup::AssignmentOperator(x) => {
                    let token = format!(
                        "{}",
                        x.assignment_operator.assignment_operator_token.token.text
                    );
                    // remove trailing `=` from assignment operator
                    let token = &token[0..token.len() - 1];
                    self.str("=");
                    self.space(1);
                    let identifier = self.assignment_lefthand_side.take().unwrap();
                    self.expression_identifier(&identifier);
                    self.space(1);
                    self.str(token);
                }
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
            match &*arg.assignment_group {
                AssignmentGroup::Equ(x) => self.equ(&x.equ),
                AssignmentGroup::AssignmentOperator(x) => {
                    self.assignment_operator(&x.assignment_operator)
                }
            }
            self.space(1);
            self.expression(&arg.expression);
        }
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
        let mut base = 0;
        for (i, x) in arg
            .if_statement_list
            .iter()
            .filter(|x| is_let_statement(&x.statement))
            .enumerate()
        {
            self.newline_list(i);
            base += self.statement_variable_declatation_only(&x.statement);
        }
        for (i, x) in arg.if_statement_list.iter().enumerate() {
            self.newline_list(base + i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.if_statement_list.is_empty());
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
            let mut base = 0;
            for (i, x) in x
                .if_statement_list0_list
                .iter()
                .filter(|x| is_let_statement(&x.statement))
                .enumerate()
            {
                self.newline_list(i);
                base += self.statement_variable_declatation_only(&x.statement);
            }
            for (i, x) in x.if_statement_list0_list.iter().enumerate() {
                self.newline_list(base + i);
                self.statement(&x.statement);
            }
            self.newline_list_post(x.if_statement_list0_list.is_empty());
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
        if let Some(ref x) = arg.if_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
            let mut base = 0;
            for (i, x) in x
                .if_statement_opt_list
                .iter()
                .filter(|x| is_let_statement(&x.statement))
                .enumerate()
            {
                self.newline_list(i);
                base += self.statement_variable_declatation_only(&x.statement);
            }
            for (i, x) in x.if_statement_opt_list.iter().enumerate() {
                self.newline_list(base + i);
                self.statement(&x.statement);
            }
            self.newline_list_post(x.if_statement_opt_list.is_empty());
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
        let mut base = 0;
        for (i, x) in arg
            .if_reset_statement_list
            .iter()
            .filter(|x| is_let_statement(&x.statement))
            .enumerate()
        {
            self.newline_list(i);
            base += self.statement_variable_declatation_only(&x.statement);
        }
        for (i, x) in arg.if_reset_statement_list.iter().enumerate() {
            self.newline_list(base + i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.if_reset_statement_list.is_empty());
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
            let mut base = 0;
            for (i, x) in x
                .if_reset_statement_list0_list
                .iter()
                .filter(|x| is_let_statement(&x.statement))
                .enumerate()
            {
                self.newline_list(i);
                base += self.statement_variable_declatation_only(&x.statement);
            }
            for (i, x) in x.if_reset_statement_list0_list.iter().enumerate() {
                self.newline_list(base + i);
                self.statement(&x.statement);
            }
            self.newline_list_post(x.if_reset_statement_list0_list.is_empty());
            self.token(&x.r_brace.r_brace_token.replace("end"));
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.space(1);
            self.r#else(&x.r#else);
            self.space(1);
            self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
            let mut base = 0;
            for (i, x) in x
                .if_reset_statement_opt_list
                .iter()
                .filter(|x| is_let_statement(&x.statement))
                .enumerate()
            {
                self.newline_list(i);
                base += self.statement_variable_declatation_only(&x.statement);
            }
            for (i, x) in x.if_reset_statement_opt_list.iter().enumerate() {
                self.newline_list(base + i);
                self.statement(&x.statement);
            }
            self.newline_list_post(x.if_reset_statement_opt_list.is_empty());
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

    /// Semantic action for non-terminal 'BreakStatement'
    fn break_statement(&mut self, arg: &BreakStatement) {
        self.r#break(&arg.r#break);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ForStatement'
    fn for_statement(&mut self, arg: &ForStatement) {
        self.r#for(&arg.r#for);
        self.space(1);
        self.str("(");
        self.scalar_type(&arg.scalar_type);
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("=");
        self.space(1);
        self.expression(&arg.range.expression);
        self.str(";");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        if let Some(ref x) = arg.range.range_opt {
            match &*x.range_operator {
                RangeOperator::DotDot(_) => self.str("<"),
                RangeOperator::DotDotEqu(_) => self.str("<="),
            }
        } else {
            self.str("<=");
        }
        self.space(1);
        if let Some(ref x) = arg.range.range_opt {
            self.expression(&x.expression);
        } else {
            self.expression(&arg.range.expression);
        }
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
        let mut base = 0;
        for (i, x) in arg
            .for_statement_list
            .iter()
            .filter(|x| is_let_statement(&x.statement))
            .enumerate()
        {
            self.newline_list(i);
            base += self.statement_variable_declatation_only(&x.statement);
        }
        for (i, x) in arg.for_statement_list.iter().enumerate() {
            self.newline_list(base + i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.for_statement_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));
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
        let start = self.dst_column;
        match &*arg.switch_item_group {
            SwitchItemGroup::SwitchCondition(x) => {
                self.expression(&x.switch_condition.expression);
                for x in &x.switch_condition.switch_condition_list {
                    self.comma(&x.comma);
                    self.space(1);
                    self.expression(&x.expression);
                }
            }
            SwitchItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.colon(&arg.colon);
        self.space(1);
        self.case_item_indent = Some((self.dst_column - start) as usize);
        match &*arg.switch_item_group0 {
            SwitchItemGroup0::Statement(x) => self.statement(&x.statement),
            SwitchItemGroup0::LBraceSwitchItemGroup0ListRBrace(x) => {
                self.token_will_push(&x.l_brace.l_brace_token.replace("begin"));
                let mut base = 0;
                for (i, x) in x
                    .switch_item_group0_list
                    .iter()
                    .filter(|x| is_let_statement(&x.statement))
                    .enumerate()
                {
                    self.newline_list(i);
                    base += self.statement_variable_declatation_only(&x.statement);
                }
                for (i, x) in x.switch_item_group0_list.iter().enumerate() {
                    self.newline_list(base + i);
                    self.statement(&x.statement);
                }
                self.newline_list_post(x.switch_item_group0_list.is_empty());
                self.token(&x.r_brace.r_brace_token.replace("end"));
            }
        }
        self.case_item_indent = None;
    }

    /// Semantic action for non-terminal 'Attribute'
    fn attribute(&mut self, arg: &Attribute) {
        let identifier = arg.identifier.identifier_token.to_string();
        match identifier.as_str() {
            "ifdef" | "ifndef" => {
                if let Some(ref x) = arg.attribute_opt {
                    let comma = if self.string.trim_end().ends_with(',') {
                        self.unindent();
                        self.truncate(self.string.len() - format!(",{}", NEWLINE).len());
                        self.newline();
                        true
                    } else {
                        false
                    };

                    self.consume_adjust_line(&arg.identifier.identifier_token.token);
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

                    self.clear_adjust_line();
                }
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
                if let Some(ref x) = arg.attribute_opt {
                    if let AttributeItem::Identifier(x) = &*x.attribute_list.attribute_item {
                        let test_name = x.identifier.identifier_token.to_string();
                        let text = format!(
                            "`ifdef __veryl_test_{}_{}__",
                            self.project_name.unwrap(),
                            test_name
                        );
                        self.token(&arg.hash.hash_token.replace(&text));
                        self.newline();
                        self.attribute.push(AttributeType::Test);
                    }
                }
            }
            _ => (),
        }
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
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.array_type.array_type_opt {
            self.space(1);
            self.array(&x.array);
        }
        self.str(";");
        self.newline();
        if is_tri {
            self.str("assign");
        } else {
            self.str("always_comb");
        }
        self.space(1);
        self.str(&arg.identifier.identifier_token.to_string());
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
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.array_type.array_type_opt {
            self.space(1);
            self.array(&x.array);
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'LocalDeclaration'
    fn local_declaration(&mut self, arg: &LocalDeclaration) {
        self.local(&arg.local);
        self.space(1);
        match &*arg.local_declaration_group {
            LocalDeclarationGroup::ArrayTypeEquExpression(x) => {
                if !self.is_implicit_scalar_type(&x.array_type.scalar_type) {
                    self.scalar_type(&x.array_type.scalar_type);
                    self.space(1);
                }
                self.identifier(&arg.identifier);
                if let Some(ref x) = x.array_type.array_type_opt {
                    self.space(1);
                    self.array(&x.array);
                }
                self.space(1);
                self.equ(&x.equ);
                self.space(1);
                self.expression(&x.expression);
            }
            LocalDeclarationGroup::TypeEquTypeExpression(x) => {
                if !self.is_implicit_type() {
                    self.r#type(&x.r#type);
                    self.space(1);
                }
                self.identifier(&arg.identifier);
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
        self.token(&arg.r#type.type_token.replace("typedef"));
        self.space(1);
        self.scalar_type(&arg.array_type.scalar_type);
        self.space(1);
        self.identifier(&arg.identifier);

        if let Some(ato) = &arg.array_type.array_type_opt {
            self.space(1);
            self.array(&ato.array);
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
            self.alwayf_ff_event_list(&x.alwayf_ff_event_list);
        } else {
            self.always_ff_implicit_event_list(arg);
        }
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        let mut base = 0;
        for (i, x) in arg
            .always_ff_declaration_list
            .iter()
            .filter(|x| is_let_statement(&x.statement))
            .enumerate()
        {
            self.newline_list(i);
            base += self.statement_variable_declatation_only(&x.statement);
        }
        for (i, x) in arg.always_ff_declaration_list.iter().enumerate() {
            self.newline_list(base + i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.always_ff_declaration_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));
        self.in_always_ff = false;
    }

    /// Semantic action for non-terminal 'AlwayfFfEventList'
    fn alwayf_ff_event_list(&mut self, arg: &AlwayfFfEventList) {
        self.l_paren(&arg.l_paren);
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.alwayf_ff_event_list_opt {
            if self.always_ff_reset_exist_in_sensitivity_list(&x.always_ff_reset) {
                self.comma(&x.comma);
                self.space(1);
            }
            self.always_ff_reset(&x.always_ff_reset);
        }
        self.r_paren(&arg.r_paren);
    }

    /// Semantic action for non-terminal 'AlwaysFfClock'
    fn always_ff_clock(&mut self, arg: &AlwaysFfClock) {
        if let Ok(found) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
            let clock = match found.found.kind {
                SymbolKind::Port(x) => x.r#type.clone().unwrap().kind,
                SymbolKind::Variable(x) => x.r#type.kind,
                _ => unreachable!(),
            };
            let clock_type = match clock {
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
            self.hierarchical_identifier(&arg.hierarchical_identifier);
        } else {
            unreachable!()
        }
    }

    /// Semantic action for non-terminal 'AlwaysFfReset'
    fn always_ff_reset(&mut self, arg: &AlwaysFfReset) {
        if let Ok(found) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
            let (reset_kind, prefix, suffix) = match found.found.kind {
                SymbolKind::Port(x) => (
                    x.r#type.clone().unwrap().kind,
                    x.prefix.clone(),
                    x.suffix.clone(),
                ),
                SymbolKind::Variable(x) => (x.r#type.kind, x.prefix.clone(), x.suffix.clone()),
                _ => unreachable!(),
            };
            let reset_type = match reset_kind {
                TypeKind::ResetAsyncHigh => ResetType::AsyncHigh,
                TypeKind::ResetAsyncLow => ResetType::AsyncLow,
                TypeKind::ResetSyncHigh => ResetType::SyncHigh,
                TypeKind::ResetSyncLow => ResetType::SyncLow,
                TypeKind::Reset => self.build_opt.reset_type,
                _ => unreachable!(),
            };
            let prefix_op = match reset_type {
                ResetType::AsyncHigh => {
                    self.str("posedge");
                    self.space(1);
                    self.hierarchical_identifier(&arg.hierarchical_identifier);
                    ""
                }
                ResetType::AsyncLow => {
                    self.str("negedge");
                    self.space(1);
                    self.hierarchical_identifier(&arg.hierarchical_identifier);
                    "!"
                }
                ResetType::SyncHigh => "",
                ResetType::SyncLow => "!",
            };

            let mut stringifier = Stringifier::new();
            stringifier.hierarchical_identifier_with_prefix_suffix(
                &arg.hierarchical_identifier,
                &prefix,
                &suffix,
            );
            self.reset_signal = Some(format!("{}{}", prefix_op, stringifier.as_str()));
        } else {
            unreachable!()
        }
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        self.always_comb(&arg.always_comb);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        let mut base = 0;
        for (i, x) in arg
            .always_comb_declaration_list
            .iter()
            .filter(|x| is_let_statement(&x.statement))
            .enumerate()
        {
            self.newline_list(i);
            base += self.statement_variable_declatation_only(&x.statement);
        }
        for (i, x) in arg.always_comb_declaration_list.iter().enumerate() {
            self.newline_list(base + i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.always_comb_declaration_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        let emit_assign =
            if let Ok(symbol) = symbol_table::resolve(arg.hierarchical_identifier.as_ref()) {
                match &symbol.found.kind {
                    SymbolKind::Variable(x) => x.r#type.modifier.contains(&SymTypeModifier::Tri),
                    SymbolKind::Port(x) => {
                        if let Some(ref x) = x.r#type {
                            x.modifier.contains(&SymTypeModifier::Tri)
                        } else {
                            false
                        }
                    }
                    _ => false,
                }
            } else {
                // External symbols may be tri-state
                true
            };
        if emit_assign {
            self.assign(&arg.assign);
        } else {
            self.token(&arg.assign.assign_token.replace("always_comb"));
        }
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
        self.identifier(&arg.identifier);
    }

    /// Semantic action for non-terminal 'EnumDeclaration'
    fn enum_declaration(&mut self, arg: &EnumDeclaration) {
        self.enum_member_prefix = Some(arg.identifier.identifier_token.to_string());
        for attr in &attribute_table::get(&arg.r#enum.enum_token.token) {
            if let Attr::EnumMemberPrefix(x) = attr {
                self.enum_member_prefix = Some(x.to_string());
            }
        }
        self.token(
            &arg.r#enum
                .enum_token
                .append(&Some(String::from("typedef ")), &None),
        );
        self.space(1);
        self.scalar_type(&arg.scalar_type);
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
        self.enum_member_prefix = None;
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
        let prefix = format!("{}_", self.enum_member_prefix.clone().unwrap());
        self.token(&arg.identifier.identifier_token.append(&Some(prefix), &None));
        if let Some(ref x) = arg.enum_item_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'StructUnionDeclaration'
    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = symbol.found.generic_maps();

        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.generic_map.push(map.clone());

            match &*arg.struct_union {
                StructUnion::Struct(ref x) => {
                    let prefix = Some(String::from("typedef "));
                    let suffix = Some(String::from(" packed"));
                    self.token(&x.r#struct.struct_token.append(&prefix, &suffix));
                }
                StructUnion::Union(ref x) => {
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
                self.str(&map.name.clone());
            } else {
                self.identifier(&arg.identifier);
            }
            self.str(";");
            self.token(&arg.r_brace.r_brace_token.replace(""));

            self.generic_map.pop();
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
        self.identifier(&arg.identifier);
    }

    /// Semantic action for non-terminal 'InitialDeclaration'
    fn initial_declaration(&mut self, arg: &InitialDeclaration) {
        self.initial(&arg.initial);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        let mut base = 0;
        for (i, x) in arg
            .initial_declaration_list
            .iter()
            .filter(|x| is_let_statement(&x.statement))
            .enumerate()
        {
            self.newline_list(i);
            base += self.statement_variable_declatation_only(&x.statement);
        }
        for (i, x) in arg.initial_declaration_list.iter().enumerate() {
            self.newline_list(base + i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.initial_declaration_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'FinalDeclaration'
    fn final_declaration(&mut self, arg: &FinalDeclaration) {
        self.r#final(&arg.r#final);
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token.replace("begin"));
        let mut base = 0;
        for (i, x) in arg
            .final_declaration_list
            .iter()
            .filter(|x| is_let_statement(&x.statement))
            .enumerate()
        {
            self.newline_list(i);
            base += self.statement_variable_declatation_only(&x.statement);
        }
        for (i, x) in arg.final_declaration_list.iter().enumerate() {
            self.newline_list(base + i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.final_declaration_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));
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
            self.array(&x.array);
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
        self.str(".");
        self.identifier(&arg.identifier);
        self.space(1);
        self.str("(");
        if let Some(ref x) = arg.inst_port_item_opt {
            self.token(&x.colon.colon_token.replace(""));
            let mut stringifier = Stringifier::new();
            stringifier.expression(&x.expression);
            if stringifier.as_str() != "_" {
                self.expression(&x.expression);
            }
        } else {
            let (prefix, suffix) = if let Ok(found) = symbol_table::resolve(arg.identifier.as_ref())
            {
                match &found.found.kind {
                    SymbolKind::Port(x) => (x.prefix.clone(), x.suffix.clone()),
                    SymbolKind::Variable(x) => (x.prefix.clone(), x.suffix.clone()),
                    _ => (None, None),
                }
            } else {
                unreachable!()
            };
            let token = identifier_with_prefix_suffix(&arg.identifier, &prefix, &suffix);
            self.duplicated_token(&token, 0);
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
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::Param(x) => self.param(&x.param),
            WithParameterItemGroup::Local(x) => self.local(&x.local),
        };
        self.space(1);
        match &*arg.with_parameter_item_group0 {
            WithParameterItemGroup0::ArrayTypeEquExpression(x) => {
                if !self.is_implicit_scalar_type(&x.array_type.scalar_type) {
                    self.scalar_type(&x.array_type.scalar_type);
                    self.space(1);
                }
                self.identifier(&arg.identifier);
                if let Some(ref x) = x.array_type.array_type_opt {
                    self.space(1);
                    self.array(&x.array);
                }
                self.space(1);
                self.equ(&x.equ);
                self.space(1);
                self.expression(&x.expression);
            }
            WithParameterItemGroup0::TypeEquTypeExpression(x) => {
                if !self.is_implicit_type() {
                    self.r#type(&x.r#type);
                    self.space(1);
                }
                self.identifier(&arg.identifier);
                self.space(1);
                self.equ(&x.equ);
                self.space(1);
                self.type_expression(&x.type_expression);
            }
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
                let x = x.port_type_concrete.as_ref();
                self.direction(&x.direction);
                if let Direction::Modport(_) = *x.direction {
                    self.in_direction_modport = true;
                } else {
                    self.space(1);
                }
                self.scalar_type(&x.array_type.scalar_type);
                self.space(1);
                self.identifier(&arg.identifier);
                if let Some(ref x) = x.array_type.array_type_opt {
                    self.space(1);
                    self.array(&x.array);
                }
                self.in_direction_modport = false;
            }
            PortDeclarationItemGroup::PortTypeAbstract(x) => {
                let x = x.port_type_abstract.as_ref();
                self.interface(&x.interface);
                self.space(1);
                self.identifier(&arg.identifier);
                if let Some(ref x) = x.port_type_abstract_opt0 {
                    self.space(1);
                    self.array(&x.array);
                }
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
            Direction::Import(x) => self.import(&x.import),
        };
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = symbol.found.generic_maps();

        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.generic_map.push(map.clone());

            self.function(&arg.function);
            self.space(1);
            self.str("automatic");
            self.space(1);
            if let Some(ref x) = &arg.function_declaration_opt1 {
                self.scalar_type(&x.scalar_type);
            } else {
                self.str("void");
            }
            self.space(1);
            if map.generic() {
                self.str(&map.name.clone());
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
            self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
            let mut base = 0;
            for (i, x) in arg
                .function_declaration_list
                .iter()
                .filter(|x| is_func_let_statement(&x.function_item))
                .enumerate()
            {
                self.newline_list(i);
                base += self.function_item_variable_declatation_only(&x.function_item);
            }
            for (i, x) in arg.function_declaration_list.iter().enumerate() {
                self.newline_list(base + i);
                self.function_item_statement_only(&x.function_item);
            }
            self.newline_list_post(arg.function_declaration_list.is_empty());
            self.token(&arg.r_brace.r_brace_token.replace("endfunction"));

            self.generic_map.pop();
        }
    }

    /// Semantic action for non-terminal 'ImportDeclaration'
    fn import_declaration(&mut self, arg: &ImportDeclaration) {
        self.in_import = true;
        self.import(&arg.import);
        self.space(1);
        self.scoped_identifier(&arg.scoped_identifier);
        if let Some(ref x) = arg.import_declaration_opt {
            self.colon_colon(&x.colon_colon);
            self.star(&x.star);
        }
        self.semicolon(&arg.semicolon);
        self.in_import = false;
    }

    /// Semantic action for non-terminal 'ExportDeclaration'
    fn export_declaration(&mut self, arg: &ExportDeclaration) {
        self.export(&arg.export);
        self.space(1);
        match &*arg.export_declaration_group {
            ExportDeclarationGroup::Star(x) => self.token(&x.star.star_token.replace("*::*")),
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

    /// Semantic action for non-terminal 'UnsafeBlock'
    fn unsafe_block(&mut self, arg: &UnsafeBlock) {
        for (i, x) in arg.unsafe_block_list.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.module_group(&x.module_group);
        }
    }

    /// Semantic action for non-terminal 'ModuleDeclaration'
    fn module_declaration(&mut self, arg: &ModuleDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        if let SymbolKind::Module(ref x) = symbol.found.kind {
            self.default_clock = x.default_clock;
            self.default_reset = x.default_reset;
        }

        let maps = symbol.found.generic_maps();
        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.generic_map.push(map.clone());

            self.module(&arg.module);
            self.space(1);
            if map.generic() {
                self.str(&map.name.clone());
            } else {
                if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    let context: SymbolContext = self.into();
                    self.str(&namespace_string(&symbol.found.namespace, &context));
                }
                self.identifier(&arg.identifier);
            }
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
            if let Some(ref x) = arg.module_declaration_opt1 {
                self.space(1);
                self.with_parameter(&x.with_parameter);
            }
            if let Some(ref x) = arg.module_declaration_opt2 {
                self.space(1);
                self.port_declaration(&x.port_declaration);
            }
            self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
            for (i, x) in arg.module_declaration_list.iter().enumerate() {
                self.newline_list(i);
                self.module_group(&x.module_group);
            }
            self.newline_list_post(arg.module_declaration_list.is_empty());
            self.token(&arg.r_brace.r_brace_token.replace("endmodule"));

            self.generic_map.pop();
        }

        self.default_clock = None;
        self.default_reset = None;
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
        self.expression(&arg.range.expression);
        self.str(";");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        if let Some(ref x) = arg.range.range_opt {
            match &*x.range_operator {
                RangeOperator::DotDot(_) => self.str("<"),
                RangeOperator::DotDotEqu(_) => self.str("<="),
            }
        } else {
            self.str("<=");
        }
        self.space(1);
        if let Some(ref x) = arg.range.range_opt {
            self.expression(&x.expression);
        } else {
            self.expression(&arg.range.expression);
        }
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
        self.default_block = Some(arg.identifier.identifier_token.to_string());
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        for (i, x) in arg.module_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.module_group(&x.module_group);
        }
        self.newline_list_post(arg.module_named_block_list.is_empty());
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
        for (i, x) in arg.module_optional_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.module_group(&x.module_group);
        }
        self.newline_list_post(arg.module_optional_named_block_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));
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
        let maps = symbol.found.generic_maps();

        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.generic_map.push(map.clone());

            self.interface(&arg.interface);
            self.space(1);
            if map.generic() {
                self.str(&map.name.clone());
            } else {
                if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    let context: SymbolContext = self.into();
                    self.str(&namespace_string(&symbol.found.namespace, &context));
                }
                self.identifier(&arg.identifier);
            }
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
            if let Some(ref x) = arg.interface_declaration_opt1 {
                self.space(1);
                self.with_parameter(&x.with_parameter);
            }
            self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
            for (i, x) in arg.interface_declaration_list.iter().enumerate() {
                self.newline_list(i);
                self.interface_group(&x.interface_group);
            }
            self.newline_list_post(arg.interface_declaration_list.is_empty());
            self.token(&arg.r_brace.r_brace_token.replace("endinterface"));

            self.generic_map.pop();
        }
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
        self.expression(&arg.range.expression);
        self.str(";");
        self.space(1);
        self.identifier(&arg.identifier);
        self.space(1);
        if let Some(ref x) = arg.range.range_opt {
            match &*x.range_operator {
                RangeOperator::DotDot(_) => self.str("<"),
                RangeOperator::DotDotEqu(_) => self.str("<="),
            }
        } else {
            self.str("<=");
        }
        self.space(1);
        if let Some(ref x) = arg.range.range_opt {
            self.expression(&x.expression);
        } else {
            self.expression(&arg.range.expression);
        }
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
        self.default_block = Some(arg.identifier.identifier_token.to_string());
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        for (i, x) in arg.interface_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.interface_group(&x.interface_group);
        }
        self.newline_list_post(arg.interface_named_block_list.is_empty());
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
        for (i, x) in arg.interface_optional_named_block_list.iter().enumerate() {
            self.newline_list(i);
            self.interface_group(&x.interface_group);
        }
        self.newline_list_post(arg.interface_optional_named_block_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));
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

    /// Semantic action for non-terminal 'PackageDeclaration'
    fn package_declaration(&mut self, arg: &PackageDeclaration) {
        let symbol = symbol_table::resolve(arg.identifier.as_ref()).unwrap();
        let maps = symbol.found.generic_maps();

        for (i, map) in maps.iter().enumerate() {
            if i != 0 {
                self.newline();
            }
            self.generic_map.push(map.clone());

            self.package(&arg.package);
            self.space(1);
            if map.generic() {
                self.str(&map.name.clone());
            } else {
                if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
                    let context: SymbolContext = self.into();
                    self.str(&namespace_string(&symbol.found.namespace, &context));
                }
                self.identifier(&arg.identifier);
            }
            self.token_will_push(&arg.l_brace.l_brace_token.replace(";"));
            for (i, x) in arg.package_declaration_list.iter().enumerate() {
                self.newline_list(i);
                if i == 0 {
                    let file_scope_import = self.file_scope_import.clone();
                    for x in &file_scope_import {
                        self.str(x);
                        self.newline();
                    }
                }
                self.package_group(&x.package_group);
            }
            self.newline_list_post(arg.package_declaration_list.is_empty());
            self.token(&arg.r_brace.r_brace_token.replace("endpackage"));

            self.generic_map.pop();
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

    /// Semantic action for non-terminal 'EmbedDeclaration'
    fn embed_declaration(&mut self, arg: &EmbedDeclaration) {
        if arg.identifier.identifier_token.to_string() == "inline" {
            let text = arg.embed_content.embed_content_token.to_string();
            let text = text.strip_prefix("{{{").unwrap();
            let text = text.strip_suffix("}}}").unwrap();
            self.veryl_token(&arg.embed_content.embed_content_token.replace(text));
        }
    }

    /// Semantic action for non-terminal 'IncludeDeclaration'
    fn include_declaration(&mut self, arg: &IncludeDeclaration) {
        if arg.identifier.identifier_token.to_string() == "inline" {
            let path = arg.string_literal.string_literal_token.to_string();
            let path = path.strip_prefix('"').unwrap();
            let path = path.strip_suffix('"').unwrap();
            if let TokenSource::File(x) = arg.identifier.identifier_token.token.source {
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
            DescriptionItem::ModuleDeclaration(x) => self.module_declaration(&x.module_declaration),
            DescriptionItem::InterfaceDeclaration(x) => {
                self.interface_declaration(&x.interface_declaration)
            }
            DescriptionItem::PackageDeclaration(x) => {
                self.package_declaration(&x.package_declaration)
            }
            // file scope import is not emitted at SystemVerilog
            DescriptionItem::ImportDeclaration(_) => (),
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
        for x in &arg.veryl_list {
            let items: Vec<DescriptionItem> = x.description_group.as_ref().into();
            for item in items {
                if let DescriptionItem::ImportDeclaration(x) = item {
                    let mut emitter = Emitter {
                        project_name: self.project_name,
                        build_opt: self.build_opt.clone(),
                        format_opt: self.format_opt.clone(),
                        ..Default::default()
                    };
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

        // build map and insert link to map
        if self.build_opt.sourcemap_target != SourceMapTarget::None {
            self.source_map.as_mut().unwrap().build();
            self.str(&self.source_map.as_ref().unwrap().get_link());
            self.newline();
        }
    }
}

pub struct SymbolContext {
    pub project_name: Option<StrId>,
    pub build_opt: Build,
    pub in_import: bool,
    pub generic_map: Vec<GenericMap>,
}

impl From<&mut Emitter> for SymbolContext {
    fn from(value: &mut Emitter) -> Self {
        SymbolContext {
            project_name: value.project_name,
            build_opt: value.build_opt.clone(),
            in_import: value.in_import,
            generic_map: value.generic_map.clone(),
        }
    }
}

fn namespace_string(namespace: &Namespace, context: &SymbolContext) -> String {
    let mut ret = String::from("");
    let mut resolve_namespace = Namespace::new();
    let mut in_sv_namespace = false;
    for (i, path) in namespace.paths.iter().enumerate() {
        if i == 0 {
            // top level namespace is always `_`
            let text = format!("{}_", path);

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
                let separator = match symbol.found.kind {
                    SymbolKind::Package(_) => "::",
                    SymbolKind::GenericInstance(_) => "::",
                    SymbolKind::Interface(_) => ".",
                    _ if in_sv_namespace => "::",
                    _ => "_",
                };
                ret.push_str(&format!("{}{}", path, separator));
            } else {
                return format!("{}", namespace);
            }
        }

        resolve_namespace.push(*path);
    }
    ret
}

pub fn symbol_string(token: &VerylToken, symbol: &Symbol, context: &SymbolContext) -> String {
    let mut ret = String::new();
    let namespace = namespace_table::get(token.token.id).unwrap();
    match &symbol.kind {
        SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package(_) => {
            ret.push_str(&namespace_string(&symbol.namespace, context));
            ret.push_str(&symbol.token.to_string());
        }
        SymbolKind::Parameter(_)
        | SymbolKind::Function(_)
        | SymbolKind::Struct(_)
        | SymbolKind::Union(_)
        | SymbolKind::TypeDef(_)
        | SymbolKind::Enum(_) => {
            let visible = namespace.included(&symbol.namespace)
                || symbol.imported.iter().any(|x| *x == namespace);
            if visible & !context.in_import {
                ret.push_str(&symbol.token.to_string());
            } else {
                ret.push_str(&namespace_string(&symbol.namespace, context));
                ret.push_str(&symbol.token.to_string());
            }
        }
        SymbolKind::EnumMember(x) => {
            let mut enum_namespace = symbol.namespace.clone();
            enum_namespace.pop();

            // if enum definition is not visible, explicit namespace is required
            if !namespace.included(&enum_namespace) {
                ret.push_str(&namespace_string(&enum_namespace, context));
            }
            ret.push_str(&x.prefix);
            ret.push('_');
            ret.push_str(&symbol.token.to_string());
        }
        SymbolKind::Modport(_) => {
            ret.push_str(&namespace_string(&symbol.namespace, context));
            ret.push_str(&symbol.token.to_string());
        }
        SymbolKind::SystemVerilog => {
            ret.push_str(&namespace_string(&symbol.namespace, context));
            ret.push_str(&symbol.token.to_string());
        }
        SymbolKind::GenericInstance(x) => {
            let base = symbol_table::get(x.base).unwrap();
            let visible = namespace.included(&base.namespace)
                || base.imported.iter().any(|x| *x == namespace);
            let top_level = matches!(
                base.kind,
                SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package(_)
            );
            if !visible | top_level {
                ret.push_str(&namespace_string(&base.namespace, context));
            }
            ret.push_str(&symbol.token.to_string());
        }
        SymbolKind::GenericParameter(_) => (),
        SymbolKind::Port(x) => {
            if let Some(ref x) = x.prefix {
                ret.push_str(x);
            }
            ret.push_str(&symbol.token.to_string());
            if let Some(ref x) = x.suffix {
                ret.push_str(x);
            }
        }
        SymbolKind::Variable(x) => {
            if let Some(ref x) = x.prefix {
                ret.push_str(x);
            }
            ret.push_str(&symbol.token.to_string());
            if let Some(ref x) = x.suffix {
                ret.push_str(x);
            }
        }
        SymbolKind::Instance(_)
        | SymbolKind::Block
        | SymbolKind::StructMember(_)
        | SymbolKind::UnionMember(_)
        | SymbolKind::ModportVariableMember(_)
        | SymbolKind::ModportFunctionMember(_)
        | SymbolKind::Genvar
        | SymbolKind::Namespace
        | SymbolKind::SystemFunction => ret.push_str(&symbol.token.to_string()),
        SymbolKind::ClockDomain => unreachable!(),
    }
    ret
}

pub fn identifier_with_prefix_suffix(
    identifier: &Identifier,
    prefix: &Option<String>,
    suffix: &Option<String>,
) -> VerylToken {
    if prefix.is_some() || suffix.is_some() {
        let token = &identifier.identifier_token.strip_prefix("r#");
        token.append(prefix, suffix)
    } else {
        identifier.identifier_token.strip_prefix("r#")
    }
}
