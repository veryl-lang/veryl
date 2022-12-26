use std::collections::HashMap;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::VerylToken;
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::ParolLocation;

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Location {
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub duplicated: Option<usize>,
}

impl From<&ParolLocation> for Location {
    fn from(x: &ParolLocation) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
            duplicated: None,
        }
    }
}

impl From<ParolLocation> for Location {
    fn from(x: ParolLocation) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
            duplicated: None,
        }
    }
}

#[derive(Default)]
pub struct Align {
    index: usize,
    max_width: usize,
    width: usize,
    line: usize,
    rest: Vec<(Location, usize)>,
    additions: HashMap<Location, usize>,
    last_location: Option<Location>,
}

impl Align {
    fn finish_group(&mut self) {
        for (loc, width) in &self.rest {
            self.additions.insert(*loc, self.max_width - width);
        }
        self.rest.clear();
        self.max_width = 0;
    }

    fn finish_item(&mut self) {
        let last_location = self.last_location.take();
        if let Some(loc) = last_location {
            if loc.line - self.line > 1 {
                self.finish_group();
            }
            self.max_width = usize::max(self.max_width, self.width);
            self.line = loc.line;
            self.rest.push((loc, self.width));

            self.width = 0;
            self.index += 1;
        }
    }

    fn start_item(&mut self) {
        self.width = 0;
    }

    fn token(&mut self, x: &VerylToken) {
        self.width += x.location().length;
        let loc: Location = x.location().into();
        self.last_location = Some(loc);
    }

    fn dummy_location(&mut self, x: Location) {
        self.width += 0; // 0 length token
        self.last_location = Some(x);
    }

    fn duplicated_token(&mut self, x: &VerylToken, i: usize) {
        self.width += x.location().length;
        let mut loc: Location = x.location().into();
        loc.duplicated = Some(i);
        self.last_location = Some(loc);
    }

    fn space(&mut self, x: usize) {
        self.width += x;
    }
}

mod align_kind {
    pub const IDENTIFIER: usize = 0;
    pub const TYPE: usize = 1;
    pub const EXPRESSION: usize = 2;
    pub const WIDTH: usize = 3;
    pub const ASSIGNMENT: usize = 4;
}

#[derive(Default)]
pub struct Aligner {
    pub additions: HashMap<Location, usize>,
    aligns: [Align; 5],
}

impl Aligner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn align(&mut self, input: &Veryl) {
        self.veryl(input);
        self.finish_group();
        for align in &self.aligns {
            for (x, y) in &align.additions {
                self.additions
                    .entry(*x)
                    .and_modify(|val| *val += *y)
                    .or_insert(*y);
            }
        }
    }

    fn finish_group(&mut self) {
        for i in 0..self.aligns.len() {
            self.aligns[i].finish_group();
        }
    }

    fn insert(&mut self, token: &VerylToken, width: usize) {
        let loc: Location = token.location().into();
        self.additions
            .entry(loc)
            .and_modify(|val| *val += width)
            .or_insert(width);
    }

    fn space(&mut self, repeat: usize) {
        for i in 0..self.aligns.len() {
            self.aligns[i].space(repeat);
        }
    }
}

impl VerylWalker for Aligner {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        for i in 0..self.aligns.len() {
            self.aligns[i].token(arg);
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
        self.veryl_token(&arg.i32_token.replace("signed int"));
    }

    /// Semantic action for non-terminal 'I64'
    fn i64(&mut self, arg: &I64) {
        self.veryl_token(&arg.i64_token.replace("signed longint"));
    }

    /// Semantic action for non-terminal 'Inout'
    fn inout(&mut self, arg: &Inout) {
        self.insert(&arg.inout_token, 1);
    }

    /// Semantic action for non-terminal 'Input'
    fn input(&mut self, arg: &Input) {
        self.insert(&arg.input_token, 1);
    }

    /// Semantic action for non-terminal 'Parameter'
    fn parameter(&mut self, arg: &Parameter) {
        self.insert(&arg.parameter_token, 1);
    }

    /// Semantic action for non-terminal 'Ref'
    fn r#ref(&mut self, arg: &Ref) {
        self.insert(&arg.ref_token, 3);
    }

    /// Semantic action for non-terminal 'U32'
    fn u32(&mut self, arg: &U32) {
        self.veryl_token(&arg.u32_token.replace("unsigned int"));
    }

    /// Semantic action for non-terminal 'U64'
    fn u64(&mut self, arg: &U64) {
        self.veryl_token(&arg.u64_token.replace("unsigned longint"));
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

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        self.space("-1:0".len());
        self.r_bracket(&arg.r_bracket);
    }

    /// Semantic action for non-terminal 'Type'
    fn r#type(&mut self, arg: &Type) {
        self.aligns[align_kind::TYPE].start_item();
        match &*arg.type_group {
            TypeGroup::BuiltinType(x) => self.builtin_type(&x.builtin_type),
            TypeGroup::Identifier(x) => self.identifier(&x.identifier),
        };
        let loc = self.aligns[align_kind::TYPE].last_location.clone();
        self.aligns[align_kind::TYPE].finish_item();
        self.aligns[align_kind::WIDTH].start_item();
        if arg.type_list.is_empty() {
            let loc = loc.unwrap();
            self.aligns[align_kind::WIDTH].dummy_location(loc);
        } else {
            for x in &arg.type_list {
                self.width(&x.width);
            }
        }
        self.aligns[align_kind::WIDTH].finish_item();
    }

    /// Semantic action for non-terminal 'AssignmentStatement'
    fn assignment_statement(&mut self, arg: &AssignmentStatement) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.aligns[align_kind::ASSIGNMENT].start_item();
        match &*arg.assignment_statement_group {
            AssignmentStatementGroup::Equ(x) => self.equ(&x.equ),
            AssignmentStatementGroup::AssignmentOperator(x) => {
                self.assignment_operator(&x.assignment_operator)
            }
        }
        self.aligns[align_kind::ASSIGNMENT].finish_item();
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'LetDeclaration'
    fn let_declaration(&mut self, arg: &LetDeclaration) {
        self.r#let(&arg.r#let);
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
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

    /// Semantic action for non-terminal 'ParameterDeclaration'
    fn parameter_declaration(&mut self, arg: &ParameterDeclaration) {
        self.parameter(&arg.parameter);
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'LocalparamDeclaration'
    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) {
        self.localparam(&arg.localparam);
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'AssignDeclaration'
    fn assign_declaration(&mut self, arg: &AssignDeclaration) {
        self.assign(&arg.assign);
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.hierarchical_identifier(&arg.hierarchical_identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.equ(&arg.equ);
        self.expression(&arg.expression);
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'ModportItem'
    fn modport_item(&mut self, arg: &ModportItem) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        self.direction(&arg.direction);
    }

    /// Semantic action for non-terminal 'StructItem'
    fn struct_item(&mut self, arg: &StructItem) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
    }

    /// Semantic action for non-terminal 'InstanceParameterItem'
    fn instance_parameter_item(&mut self, arg: &InstanceParameterItem) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        if let Some(ref x) = arg.instance_parameter_item_opt {
            self.colon(&x.colon);
            self.space(1);
            self.aligns[align_kind::EXPRESSION].start_item();
            self.expression(&x.expression);
            self.aligns[align_kind::EXPRESSION].finish_item();
        } else {
            self.aligns[align_kind::EXPRESSION].start_item();
            self.aligns[align_kind::EXPRESSION]
                .duplicated_token(&arg.identifier.identifier_token, 0);
            self.aligns[align_kind::EXPRESSION].finish_item();
        }
    }

    /// Semantic action for non-terminal 'InstancePortItem'
    fn instance_port_item(&mut self, arg: &InstancePortItem) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        if let Some(ref x) = arg.instance_port_item_opt {
            self.colon(&x.colon);
            self.space(1);
            self.aligns[align_kind::EXPRESSION].start_item();
            self.expression(&x.expression);
            self.aligns[align_kind::EXPRESSION].finish_item();
        } else {
            self.aligns[align_kind::EXPRESSION].start_item();
            self.aligns[align_kind::EXPRESSION]
                .duplicated_token(&arg.identifier.identifier_token, 0);
            self.aligns[align_kind::EXPRESSION].finish_item();
        }
    }

    /// Semantic action for non-terminal 'WithParameterItem'
    fn with_parameter_item(&mut self, arg: &WithParameterItem) {
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::Parameter(x) => self.parameter(&x.parameter),
            WithParameterItemGroup::Localparam(x) => self.localparam(&x.localparam),
        };
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        self.r#type(&arg.r#type);
        self.equ(&arg.equ);
        self.aligns[align_kind::EXPRESSION].start_item();
        self.expression(&arg.expression);
        self.aligns[align_kind::EXPRESSION].finish_item();
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        self.direction(&arg.direction);
        self.r#type(&arg.r#type);
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        self.function(&arg.function);
        self.identifier(&arg.identifier);
        if let Some(ref x) = arg.function_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
        }
        self.minus_g_t(&arg.minus_g_t);
        // skip type align
        //self.r#type(&arg.r#type);
        self.l_brace(&arg.l_brace);
        for x in &arg.function_declaration_list {
            self.function_item(&x.function_item);
        }
        self.r_brace(&arg.r_brace);
    }
}
