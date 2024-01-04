use crate::aligner::{Aligner, Location};
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::SymbolKind;
use veryl_analyzer::symbol_table::{self, SymbolPath};
use veryl_analyzer::{msb_table, namespace_table};
use veryl_metadata::{Build, BuiltinType, ClockType, Format, Metadata, ResetType};
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
    build_opt: Build,
    format_opt: Format,
    string: String,
    indent: usize,
    line: u32,
    aligner: Aligner,
    last_newline: u32,
    in_start_token: bool,
    consumed_next_newline: bool,
    single_line: bool,
    adjust_line: bool,
    in_always_ff: bool,
    in_function: bool,
    in_generate: bool,
    in_direction_modport: bool,
    signed: bool,
    reset_signal: Option<String>,
    default_block: Option<String>,
    enum_name: Option<String>,
    file_scope_import: Vec<String>,
    attribute: Vec<AttributeType>,
    assignment_lefthand_side: Option<ExpressionIdentifier>,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            build_opt: Build::default(),
            format_opt: Format::default(),
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
            signed: false,
            reset_signal: None,
            default_block: None,
            enum_name: None,
            file_scope_import: Vec::new(),
            attribute: Vec::new(),
            assignment_lefthand_side: None,
        }
    }
}

impl Emitter {
    pub fn new(metadata: &Metadata) -> Self {
        let mut aligner = Aligner::new();
        aligner.set_metadata(metadata);
        Self {
            build_opt: metadata.build.clone(),
            format_opt: metadata.format.clone(),
            aligner,
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
        self.str(&" ".repeat(self.indent * self.format_opt.indent_width));
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
        self.last_newline = text.matches('\n').count() as u32;
        self.str(text);
        self.line = x.line;
    }

    fn process_token(&mut self, x: &VerylToken, will_push: bool, duplicated: Option<usize>) {
        self.push_token(&x.token);

        let mut loc: Location = x.token.into();
        loc.duplicated = duplicated;
        if let Some(width) = self.aligner.additions.get(&loc) {
            self.space(*width as usize);
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

    fn always_ff_reset_exist_in_sensitivity_list(&mut self, arg: &AlwaysFfReset) -> bool {
        if let Some(ref x) = arg.always_ff_reset_opt {
            match &*x.always_ff_reset_opt_group {
                AlwaysFfResetOptGroup::AsyncLow(_) => true,
                AlwaysFfResetOptGroup::AsyncHigh(_) => true,
                AlwaysFfResetOptGroup::SyncLow(_) => false,
                AlwaysFfResetOptGroup::SyncHigh(_) => false,
            }
        } else {
            match self.build_opt.reset_type {
                ResetType::AsyncLow => true,
                ResetType::AsyncHigh => true,
                ResetType::SyncLow => false,
                ResetType::SyncHigh => false,
            }
        }
    }

    fn attribute_end(&mut self) {
        if let Some(AttributeType::Ifdef) = self.attribute.pop() {
            self.newline();
            self.str("`endif");
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

    fn namespace(&mut self, namespace: &Namespace) {
        let mut ret = String::from("");
        let mut resolve_namespace = Namespace::new();
        for (i, path) in namespace.paths.iter().enumerate() {
            if i > 0 {
                let symbol_path = SymbolPath::new(&[*path]);
                if let Ok(ref symbol) = symbol_table::get(&symbol_path, &resolve_namespace) {
                    if let Some(ref symbol) = symbol.found {
                        let separator = match symbol.kind {
                            SymbolKind::Package => "::",
                            SymbolKind::Interface(_) => ".",
                            _ => "_",
                        };
                        ret.push_str(&format!("{}{}", path, separator));
                    } else {
                        return self.str(&format!("{}", namespace));
                    }
                } else {
                    return self.str(&format!("{}", namespace));
                }
            } else {
                // top level namespace is always `_`
                ret.push_str(&format!("{}_", path));
            }

            resolve_namespace.push(*path);
        }
        self.str(&ret);
    }

    fn path_identifier(&mut self, arg: &[Identifier]) {
        if let Ok(ref symbol) = symbol_table::resolve(arg) {
            if let Some(ref symbol) = symbol.found {
                match symbol.kind {
                    SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package => {
                        self.namespace(&symbol.namespace);
                        self.str(&format!("{}", symbol.token.text));
                    }
                    SymbolKind::Parameter(_)
                    | SymbolKind::Function(_)
                    | SymbolKind::Struct
                    | SymbolKind::Union
                    | SymbolKind::TypeDef(_)
                    | SymbolKind::Enum(_) => {
                        if arg.len() > 1 {
                            self.namespace(&symbol.namespace);
                            self.str(&format!("{}", symbol.token.text));
                        } else {
                            self.identifier(&arg[0]);
                        }
                    }
                    SymbolKind::EnumMember(_) => {
                        if arg.len() > 2 {
                            self.namespace(&symbol.namespace);
                            self.str(&format!("{}", symbol.token.text));
                        } else {
                            self.identifier(&arg[0]);
                            self.str("_");
                            self.identifier(&arg[1]);
                        }
                    }
                    SymbolKind::Modport(_) => {
                        self.namespace(&symbol.namespace);
                        self.str(&format!("{}", symbol.token.text));
                    }
                    SymbolKind::Port(_)
                    | SymbolKind::Variable(_)
                    | SymbolKind::Instance(_)
                    | SymbolKind::Block
                    | SymbolKind::StructMember(_)
                    | SymbolKind::UnionMember(_)
                    | SymbolKind::ModportMember
                    | SymbolKind::Genvar => unreachable!(),
                }
                return;
            }
        }

        // case at unresolved
        for (i, x) in arg.iter().enumerate() {
            if i != 0 {
                if self.in_direction_modport {
                    self.str(".");
                } else {
                    self.str("::");
                }
            }
            self.identifier(x);
        }
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
        let text = token.text();
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
        let text = &arg.all_bit_token.text();
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
            self.string.truncate(self.string.len() - "`endif".len());

            let trailing_endif = format!(
                "`endif\n{}",
                " ".repeat(self.indent * self.format_opt.indent_width)
            );
            let mut additional_endif = 0;
            while self.string.ends_with(&trailing_endif) {
                self.string
                    .truncate(self.string.len() - trailing_endif.len());
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

    /// Semantic action for non-terminal 'U32'
    fn u32(&mut self, arg: &U32) {
        self.veryl_token(&arg.u32_token.replace("int unsigned"));
    }

    /// Semantic action for non-terminal 'U64'
    fn u64(&mut self, arg: &U64) {
        self.veryl_token(&arg.u64_token.replace("longint unsigned"));
    }

    /// Semantic action for non-terminal 'Operator07'
    fn operator07(&mut self, arg: &Operator07) {
        match arg.operator07_token.text().as_str() {
            "<:" => self.str("<"),
            ">:" => self.str(">"),
            _ => self.veryl_token(&arg.operator07_token),
        }
    }

    /// Semantic action for non-terminal 'ScopedIdentifier'
    fn scoped_identifier(&mut self, arg: &ScopedIdentifier) {
        let mut path = vec![arg.identifier.as_ref().clone()];
        for x in &arg.scoped_identifier_list {
            path.push(x.identifier.as_ref().clone());
        }
        self.path_identifier(&path);
    }

    /// Semantic action for non-terminal 'ExpressionIdentifier'
    fn expression_identifier(&mut self, arg: &ExpressionIdentifier) {
        if let Some(ref x) = arg.expression_identifier_opt {
            self.dollar(&x.dollar);
        }

        match &*arg.expression_identifier_group {
            ExpressionIdentifierGroup::ColonColonIdentifierExpressionIdentifierGroupListExpressionIdentifierGroupList0(x) => {
                let mut path = vec![arg.identifier.as_ref().clone(), x.identifier.as_ref().clone()];
                for x in &x.expression_identifier_group_list {
                    path.push(x.identifier.as_ref().clone());
                }
                self.path_identifier(&path);
                for x in &x.expression_identifier_group_list0 {
                    self.select(&x.select);
                }
            }
            ExpressionIdentifierGroup::ExpressionIdentifierGroupList1ExpressionIdentifierGroupList2(x) => {
                self.identifier(&arg.identifier);
                for x in &x.expression_identifier_group_list1 {
                    self.select(&x.select);
                }
                for x in &x.expression_identifier_group_list2 {
                    self.dot(&x.dot);
                    self.identifier(&x.identifier);
                    for x in &x.expression_identifier_group_list2_list {
                        self.select(&x.select);
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

    /// Semantic action for non-terminal 'InsideExpression'
    fn inside_expression(&mut self, arg: &InsideExpression) {
        self.str("(");
        self.expression(&arg.expression);
        self.space(1);
        self.inside(&arg.inside);
        self.space(1);
        self.l_brace(&arg.l_brace);
        self.range_list(&arg.range_list);
        self.r_brace(&arg.r_brace);
        self.str(")");
    }

    /// Semantic action for non-terminal 'OutsideExpression'
    fn outside_expression(&mut self, arg: &OutsideExpression) {
        self.str("!(");
        self.expression(&arg.expression);
        self.space(1);
        self.token(&arg.outside.outside_token.replace("inside"));
        self.space(1);
        self.l_brace(&arg.l_brace);
        self.range_list(&arg.range_list);
        self.r_brace(&arg.r_brace);
        self.str(")");
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
        for (i, x) in arg.if_statement_list.iter().enumerate() {
            self.newline_list(i);
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
            for (i, x) in x.if_statement_list0_list.iter().enumerate() {
                self.newline_list(i);
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
            for (i, x) in x.if_statement_opt_list.iter().enumerate() {
                self.newline_list(i);
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
        for (i, x) in arg.if_reset_statement_list.iter().enumerate() {
            self.newline_list(i);
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
            for (i, x) in x.if_reset_statement_list0_list.iter().enumerate() {
                self.newline_list(i);
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
            for (i, x) in x.if_reset_statement_opt_list.iter().enumerate() {
                self.newline_list(i);
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
        for (i, x) in arg.for_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.for_statement_list.is_empty());
        self.token(&arg.r_brace.r_brace_token.replace("end"));
    }

    /// Semantic action for non-terminal 'CaseStatement'
    fn case_statement(&mut self, arg: &CaseStatement) {
        self.case(&arg.case);
        self.space(1);
        self.str("(");
        self.expression(&arg.expression);
        self.token_will_push(&arg.l_brace.l_brace_token.replace(")"));
        for (i, x) in arg.case_statement_list.iter().enumerate() {
            self.newline_list(i);
            self.case_item(&x.case_item);
        }
        self.newline_list_post(arg.case_statement_list.is_empty());
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
                for (i, x) in x.case_item_group0_list.iter().enumerate() {
                    self.newline_list(i);
                    self.statement(&x.statement);
                }
                self.newline_list_post(x.case_item_group0_list.is_empty());
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

                    self.adjust_line = false;
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
                    if let AttributeItem::StringLiteral(x) = &*x.attribute_list.attribute_item {
                        let text = x.string_literal.string_literal_token.text();
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
        self.scalar_type(&arg.array_type.scalar_type);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.array_type.array_type_opt {
            self.space(1);
            self.array(&x.array);
        }
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
        match &*arg.localparam_declaration_group {
            LocalparamDeclarationGroup::ArrayTypeEquExpression(x) => {
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
            LocalparamDeclarationGroup::TypeEquTypeExpression(x) => {
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

    /// Semantic action for non-terminal 'LocalparamDeclaration'
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
        for (i, x) in arg.always_ff_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.always_ff_declaration_list.is_empty());
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
            match self.build_opt.clock_type {
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
            match self.build_opt.reset_type {
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
        for (i, x) in arg.always_comb_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.statement(&x.statement);
        }
        self.newline_list_post(arg.always_comb_declaration_list.is_empty());
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
        self.enum_name = Some(arg.identifier.identifier_token.text());
        self.str("typedef");
        self.space(1);
        self.r#enum(&arg.r#enum);
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
        let prefix = format!("{}_", self.enum_name.clone().unwrap());
        self.token(&arg.identifier.identifier_token.append(&prefix, ""));
        if let Some(ref x) = arg.enum_item_opt {
            self.space(1);
            self.equ(&x.equ);
            self.space(1);
            self.expression(&x.expression);
        }
    }

    /// Semantic action for non-terminal 'StructUnionDeclaration'
    fn struct_union_declaration(&mut self, arg: &StructUnionDeclaration) {
        match &*arg.struct_union {
            StructUnion::Struct(ref x) => {
                self.token(&x.r#struct.struct_token.append("typedef ", " packed"));
            }
            StructUnion::Union(ref x) => {
                self.token(&x.union.union_token.append("typedef ", " packed"));
            }
        }
        self.space(1);
        self.token_will_push(&arg.l_brace.l_brace_token);
        self.newline_push();
        self.struct_union_list(&arg.struct_union_list);
        self.newline_pop();
        self.str("}");
        self.space(1);
        self.identifier(&arg.identifier);
        self.str(";");
        self.token(&arg.r_brace.r_brace_token.replace(""));
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
        for (i, x) in arg.initial_declaration_list.iter().enumerate() {
            self.newline_list(i);
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
        for (i, x) in arg.final_declaration_list.iter().enumerate() {
            self.newline_list(i);
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
            WithParameterItemGroup::Parameter(x) => self.parameter(&x.parameter),
            WithParameterItemGroup::Localparam(x) => self.localparam(&x.localparam),
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
            PortDeclarationItemGroup::DirectionArrayType(x) => {
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
            PortDeclarationItemGroup::InterfacePortDeclarationItemOpt(x) => {
                self.interface(&x.interface);
                self.space(1);
                self.identifier(&arg.identifier);
                if let Some(ref x) = x.port_declaration_item_opt {
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
        self.scalar_type(&arg.scalar_type);
        self.space(1);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
            self.space(1);
        }
        self.token(&arg.minus_g_t.minus_g_t_token.replace(""));
        self.str(";");
        self.token_will_push(&arg.l_brace.l_brace_token.replace(""));
        for (i, x) in arg.function_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.function_item(&x.function_item);
        }
        self.newline_list_post(arg.function_declaration_list.is_empty());
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
        if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
            if let Some(symbol) = symbol.found {
                self.namespace(&symbol.namespace);
            }
        }
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
        for (i, x) in arg.module_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.module_group(&x.module_group);
        }
        self.newline_list_post(arg.module_declaration_list.is_empty());
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
        self.default_block = Some(arg.identifier.identifier_token.text());
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
        self.interface(&arg.interface);
        self.space(1);
        if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
            if let Some(symbol) = symbol.found {
                self.namespace(&symbol.namespace);
            }
        }
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
        for (i, x) in arg.interface_declaration_list.iter().enumerate() {
            self.newline_list(i);
            self.interface_group(&x.interface_group);
        }
        self.newline_list_post(arg.interface_declaration_list.is_empty());
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
        self.default_block = Some(arg.identifier.identifier_token.text());
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
        self.package(&arg.package);
        self.space(1);
        if let Ok(symbol) = symbol_table::resolve(arg.identifier.as_ref()) {
            if let Some(symbol) = symbol.found {
                self.namespace(&symbol.namespace);
            }
        }
        self.identifier(&arg.identifier);
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
