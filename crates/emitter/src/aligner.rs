use std::collections::HashMap;
use veryl_metadata::{Build, BuiltinType, Metadata};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, VerylToken};
use veryl_parser::veryl_walker::VerylWalker;
use veryl_parser::Stringifier;

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Location {
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub duplicated: Option<usize>,
}

impl From<&Token> for Location {
    fn from(x: &Token) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
            duplicated: None,
        }
    }
}

impl From<Token> for Location {
    fn from(x: Token) -> Self {
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
    enable: bool,
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
        self.enable = false;
        if let Some(loc) = self.last_location {
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
        self.enable = true;
        self.width = 0;
    }

    fn token(&mut self, x: &VerylToken) {
        if self.enable {
            self.width += x.token.length;
            let loc: Location = x.token.into();
            self.last_location = Some(loc);
        }
    }

    fn dummy_location(&mut self, x: Location) {
        if self.enable {
            self.width += 0; // 0 length token
            self.last_location = Some(x);
        }
    }

    fn duplicated_token(&mut self, x: &VerylToken, i: usize) {
        if self.enable {
            self.width += x.token.length;
            let mut loc: Location = x.token.into();
            loc.duplicated = Some(i);
            self.last_location = Some(loc);
        }
    }

    fn space(&mut self, x: usize) {
        if self.enable {
            self.width += x;
        }
    }
}

mod align_kind {
    pub const IDENTIFIER: usize = 0;
    pub const TYPE: usize = 1;
    pub const EXPRESSION: usize = 2;
    pub const WIDTH: usize = 3;
    pub const ARRAY: usize = 4;
    pub const ASSIGNMENT: usize = 5;
    pub const PARAMETER: usize = 6;
    pub const DIRECTION: usize = 7;
}

#[derive(Default)]
pub struct Aligner {
    pub additions: HashMap<Location, usize>,
    aligns: [Align; 8],
    in_type_expression: bool,
    build_opt: Build,
}

impl Aligner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn set_metadata(&mut self, metadata: &Metadata) {
        self.build_opt = metadata.build.clone();
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

    fn space(&mut self, repeat: usize) {
        for i in 0..self.aligns.len() {
            self.aligns[i].space(repeat);
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
        self.veryl_token(&arg.i32_token.replace("int signed"));
    }

    /// Semantic action for non-terminal 'I64'
    fn i64(&mut self, arg: &I64) {
        self.veryl_token(&arg.i64_token.replace("longint signed"));
    }

    /// Semantic action for non-terminal 'U32'
    fn u32(&mut self, arg: &U32) {
        self.veryl_token(&arg.u32_token.replace("int unsigned"));
    }

    /// Semantic action for non-terminal 'U64'
    fn u64(&mut self, arg: &U64) {
        self.veryl_token(&arg.u64_token.replace("longint unsigned"));
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

    /// Semantic action for non-terminal 'TypeExpression'
    fn type_expression(&mut self, arg: &TypeExpression) {
        self.in_type_expression = true;
        match arg {
            TypeExpression::ScalarType(x) => self.scalar_type(&x.scalar_type),
            TypeExpression::TypeLParenExpressionRParen(x) => {
                self.r#type(&x.r#type);
                self.l_paren(&x.l_paren);
                self.expression(&x.expression);
                self.r_paren(&x.r_paren);
            }
        }
        self.in_type_expression = false;
    }

    /// Semantic action for non-terminal 'Width'
    fn width(&mut self, arg: &Width) {
        self.l_angle(&arg.l_angle);
        self.expression(&arg.expression);
        self.space("-1:0".len());
        for x in &arg.width_list {
            self.space("][".len());
            self.expression(&x.expression);
            self.space("-1:0".len());
        }
        self.r_angle(&arg.r_angle);
    }

    /// Semantic action for non-terminal 'Array'
    fn array(&mut self, arg: &Array) {
        self.l_bracket(&arg.l_bracket);
        self.expression(&arg.expression);
        self.space("-1:0".len());
        for x in &arg.array_list {
            self.space("][".len());
            self.expression(&x.expression);
            self.space("-1:0".len());
        }
        self.r_bracket(&arg.r_bracket);
    }

    /// Semantic action for non-terminal 'VariableType'
    fn variable_type(&mut self, arg: &VariableType) {
        match &*arg.variable_type_group {
            VariableTypeGroup::Logic(x) => self.logic(&x.logic),
            VariableTypeGroup::Bit(x) => self.bit(&x.bit),
            VariableTypeGroup::ScopedIdentifier(x) => self.scoped_identifier(&x.scoped_identifier),
        };
        if !self.in_type_expression {
            self.aligns[align_kind::TYPE].finish_item();
            self.aligns[align_kind::WIDTH].start_item();
        }
        if let Some(ref x) = arg.variable_type_opt {
            self.space(1);
            self.width(&x.width);
        } else if !self.in_type_expression {
            let loc = self.aligns[align_kind::TYPE].last_location;
            self.aligns[align_kind::WIDTH].dummy_location(loc.unwrap());
        }
    }

    /// Semantic action for non-terminal 'ScalarType'
    fn scalar_type(&mut self, arg: &ScalarType) {
        if !self.in_type_expression {
            self.aligns[align_kind::TYPE].start_item();
            // dummy space for implicit type
            self.space(1);
        }
        for x in &arg.scalar_type_list {
            self.type_modifier(&x.type_modifier);
            self.space(1);
        }
        match &*arg.scalar_type_group {
            ScalarTypeGroup::VariableType(x) => self.variable_type(&x.variable_type),
            ScalarTypeGroup::FixedType(x) => {
                self.fixed_type(&x.fixed_type);
                if !self.in_type_expression {
                    self.aligns[align_kind::TYPE].finish_item();
                    self.aligns[align_kind::WIDTH].start_item();
                    let loc = self.aligns[align_kind::TYPE].last_location;
                    self.aligns[align_kind::WIDTH].dummy_location(loc.unwrap());
                }
            }
        }
        if !self.in_type_expression {
            self.aligns[align_kind::WIDTH].finish_item();
        }
    }

    /// Semantic action for non-terminal 'ArrayType'
    fn array_type(&mut self, arg: &ArrayType) {
        self.scalar_type(&arg.scalar_type);
        self.aligns[align_kind::ARRAY].start_item();
        if let Some(ref x) = arg.array_type_opt {
            self.space(1);
            self.array(&x.array);
        } else {
            let loc = self.aligns[align_kind::IDENTIFIER].last_location;
            self.aligns[align_kind::ARRAY].dummy_location(loc.unwrap());
        }
        self.aligns[align_kind::ARRAY].finish_item();
    }

    /// Semantic action for non-terminal 'IdentifierStatement'
    fn identifier_statement(&mut self, arg: &IdentifierStatement) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.expression_identifier(&arg.expression_identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
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
        self.aligns[align_kind::ASSIGNMENT].start_item();
        match &*arg.assignment_group {
            AssignmentGroup::Equ(x) => self.equ(&x.equ),
            AssignmentGroup::AssignmentOperator(x) => {
                self.assignment_operator(&x.assignment_operator)
            }
        }
        self.aligns[align_kind::ASSIGNMENT].finish_item();
        self.expression(&arg.expression);
    }

    /// Semantic action for non-terminal 'CaseItem'
    fn case_item(&mut self, arg: &CaseItem) {
        self.aligns[align_kind::EXPRESSION].start_item();
        match &*arg.case_item_group {
            CaseItemGroup::Expression(x) => self.expression(&x.expression),
            CaseItemGroup::Defaul(x) => self.defaul(&x.defaul),
        }
        self.aligns[align_kind::EXPRESSION].finish_item();
        self.colon(&arg.colon);
        match &*arg.case_item_group0 {
            CaseItemGroup0::Statement(x) => self.statement(&x.statement),
            CaseItemGroup0::LBraceCaseItemGroup0ListRBrace(x) => {
                self.l_brace(&x.l_brace);
                for x in &x.case_item_group0_list {
                    self.statement(&x.statement);
                }
                self.r_brace(&x.r_brace);
            }
        }
    }

    /// Semantic action for non-terminal 'VarDeclaration'
    fn var_declaration(&mut self, arg: &VarDeclaration) {
        self.var(&arg.var);
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        self.array_type(&arg.array_type);
        if let Some(ref x) = arg.var_declaration_opt {
            self.equ(&x.equ);
            self.expression(&x.expression);
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'LocalparamDeclaration'
    fn localparam_declaration(&mut self, arg: &LocalparamDeclaration) {
        self.localparam(&arg.localparam);
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        match &*arg.localparam_declaration_group {
            LocalparamDeclarationGroup::ArrayTypeEquExpression(x) => {
                if !self.is_implicit_scalar_type(&x.array_type.scalar_type) {
                    self.array_type(&x.array_type);
                } else {
                    self.aligns[align_kind::TYPE].start_item();
                    self.aligns[align_kind::TYPE]
                        .dummy_location(arg.localparam.localparam_token.token.into());
                    self.aligns[align_kind::TYPE].finish_item();
                }
                self.equ(&x.equ);
                self.expression(&x.expression);
            }
            LocalparamDeclarationGroup::TypeEquTypeExpression(x) => {
                self.aligns[align_kind::TYPE].start_item();
                if !self.is_implicit_type() {
                    self.r#type(&x.r#type);
                    self.space(1);
                } else {
                    self.aligns[align_kind::TYPE]
                        .dummy_location(arg.localparam.localparam_token.token.into());
                }
                self.aligns[align_kind::TYPE].finish_item();
                self.equ(&x.equ);
                self.type_expression(&x.type_expression);
            }
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'TypeDefDeclaration'
    fn type_def_declaration(&mut self, arg: &TypeDefDeclaration) {
        self.r#type(&arg.r#type);
        self.aligns[align_kind::TYPE].start_item();
        self.scalar_type(&arg.array_type.scalar_type);
        self.aligns[align_kind::TYPE].finish_item();
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        if let Some(ato) = &arg.array_type.array_type_opt {
            self.aligns[align_kind::ARRAY].start_item();
            self.array(&ato.array);
            self.aligns[align_kind::ARRAY].finish_item();
        }
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
        self.scalar_type(&arg.scalar_type);
    }

    /// Semantic action for non-terminal 'InstDeclaration'
    fn inst_declaration(&mut self, arg: &InstDeclaration) {
        let single_line = arg.inst_declaration_opt1.is_none();
        self.inst(&arg.inst);
        if single_line {
            self.aligns[align_kind::IDENTIFIER].start_item();
        }
        self.identifier(&arg.identifier);
        if single_line {
            self.aligns[align_kind::IDENTIFIER].finish_item();
        }
        self.colon(&arg.colon);
        self.scoped_identifier(&arg.scoped_identifier);
        // skip align at single line
        if single_line {
            return;
        }
        if let Some(ref x) = arg.inst_declaration_opt {
            self.array(&x.array);
        }
        if let Some(ref x) = arg.inst_declaration_opt0 {
            self.inst_parameter(&x.inst_parameter);
        }
        if let Some(ref x) = arg.inst_declaration_opt1 {
            self.l_paren(&x.l_paren);
            if let Some(ref x) = x.inst_declaration_opt2 {
                self.inst_port_list(&x.inst_port_list);
            }
            self.r_paren(&x.r_paren);
        }
        self.semicolon(&arg.semicolon);
    }

    /// Semantic action for non-terminal 'InstParameterItem'
    fn inst_parameter_item(&mut self, arg: &InstParameterItem) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        if let Some(ref x) = arg.inst_parameter_item_opt {
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

    /// Semantic action for non-terminal 'InstPortItem'
    fn inst_port_item(&mut self, arg: &InstPortItem) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        if let Some(ref x) = arg.inst_port_item_opt {
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
        self.aligns[align_kind::PARAMETER].start_item();
        match &*arg.with_parameter_item_group {
            WithParameterItemGroup::Parameter(x) => self.parameter(&x.parameter),
            WithParameterItemGroup::Localparam(x) => self.localparam(&x.localparam),
        };
        self.aligns[align_kind::PARAMETER].finish_item();
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        match &*arg.with_parameter_item_group0 {
            WithParameterItemGroup0::ArrayTypeEquExpression(x) => {
                if !self.is_implicit_scalar_type(&x.array_type.scalar_type) {
                    self.array_type(&x.array_type);
                } else {
                    self.aligns[align_kind::TYPE].start_item();
                    let loc = self.aligns[align_kind::PARAMETER].last_location;
                    self.aligns[align_kind::TYPE].dummy_location(loc.unwrap());
                    self.aligns[align_kind::TYPE].finish_item();
                }
                self.equ(&x.equ);
                self.aligns[align_kind::EXPRESSION].start_item();
                self.expression(&x.expression);
                self.aligns[align_kind::EXPRESSION].finish_item();
            }
            WithParameterItemGroup0::TypeEquTypeExpression(x) => {
                self.aligns[align_kind::TYPE].start_item();
                if !self.is_implicit_type() {
                    self.r#type(&x.r#type);
                    self.space(1);
                } else {
                    let loc = self.aligns[align_kind::PARAMETER].last_location;
                    self.aligns[align_kind::TYPE].dummy_location(loc.unwrap());
                }
                self.aligns[align_kind::TYPE].finish_item();
                self.equ(&x.equ);
                self.aligns[align_kind::EXPRESSION].start_item();
                self.type_expression(&x.type_expression);
                self.aligns[align_kind::EXPRESSION].finish_item();
            }
        }
    }

    /// Semantic action for non-terminal 'PortDeclarationItem'
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) {
        self.aligns[align_kind::IDENTIFIER].start_item();
        self.identifier(&arg.identifier);
        self.aligns[align_kind::IDENTIFIER].finish_item();
        self.colon(&arg.colon);
        match &*arg.port_declaration_item_group {
            PortDeclarationItemGroup::DirectionArrayType(x) => {
                self.direction(&x.direction);
                self.array_type(&x.array_type);
            }
            PortDeclarationItemGroup::InterfacePortDeclarationItemOpt(x) => {
                self.interface(&x.interface);
                if let Some(ref x) = x.port_declaration_item_opt {
                    self.array(&x.array);
                }
            }
        }
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        if !matches!(arg, Direction::Modport(_)) {
            self.aligns[align_kind::DIRECTION].start_item();
        }
        match arg {
            Direction::Input(x) => self.input(&x.input),
            Direction::Output(x) => self.output(&x.output),
            Direction::Inout(x) => self.inout(&x.inout),
            Direction::Ref(x) => self.r#ref(&x.r#ref),
            Direction::Modport(_) => (),
        };
        if !matches!(arg, Direction::Modport(_)) {
            self.aligns[align_kind::DIRECTION].finish_item();
        }
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
        //self.scalar_type(&arg.scalar_type);
        self.l_brace(&arg.l_brace);
        for x in &arg.function_declaration_list {
            self.function_item(&x.function_item);
        }
        self.r_brace(&arg.r_brace);
    }
}
