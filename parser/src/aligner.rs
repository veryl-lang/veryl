use crate::veryl_grammar_trait::*;
use crate::veryl_walker::VerylWalker;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Location {
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

impl From<&parol_runtime::lexer::Location> for Location {
    fn from(x: &parol_runtime::lexer::Location) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
        }
    }
}

pub struct Align {
    index: usize,
    max_width: usize,
    line: usize,
    rest: Vec<Location>,
    pub widths: HashMap<Location, usize>,
}

impl Default for Align {
    fn default() -> Self {
        Self {
            index: 0,
            max_width: 0,
            line: 0,
            rest: Vec::new(),
            widths: HashMap::new(),
        }
    }
}

impl Align {
    fn reset(&mut self) {
        for x in &self.rest {
            self.widths.insert(*x, self.max_width);
        }
        self.rest.clear();
        self.max_width = 0;
    }

    fn update(&mut self, x: Location) {
        if x.line - self.line > 1 {
            self.reset();
        }
        self.max_width = usize::max(self.max_width, x.length);
        self.index += 1;
        self.rest.push(x.into());
        self.line = x.line;
    }
}

pub struct Aligner {
    pub identifier: Align,
    pub r#type: Align,
}

impl Default for Aligner {
    fn default() -> Self {
        Self {
            identifier: Default::default(),
            r#type: Default::default(),
        }
    }
}

impl Aligner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn align(&mut self, input: &Veryl) {
        self.veryl(input);
        self.reset();
    }

    fn reset(&mut self) {
        self.identifier.reset();
        self.r#type.reset();
    }
}

impl VerylWalker for Aligner {
    // ----------------------------------------------------------------------------
    // SourceCode
    // ----------------------------------------------------------------------------

    fn veryl(&mut self, input: &Veryl) {
        for x in &input.veryl_list {
            self.description(&x.description);
        }
    }

    // ----------------------------------------------------------------------------
    // Description
    // ----------------------------------------------------------------------------

    fn description(&mut self, input: &Description) {
        match input {
            Description::Description0(x) => self.module_declaration(&x.module_declaration),
            Description::Description1(x) => self.interface_declaration(&x.interface_declaration),
        }
    }

    // ----------------------------------------------------------------------------
    // Number
    // ----------------------------------------------------------------------------

    fn number(&mut self, _input: &Number) {}

    // ----------------------------------------------------------------------------
    // Expression
    // ----------------------------------------------------------------------------

    fn expression(&mut self, input: &Expression) {
        self.expression0(&input.expression0);
    }

    fn expression0(&mut self, input: &Expression0) {
        self.expression1(&input.expression1);
        for x in &input.expression0_list {
            self.expression1(&x.expression1);
        }
    }

    fn expression1(&mut self, input: &Expression1) {
        self.expression2(&input.expression2);
        for x in &input.expression1_list {
            self.expression2(&x.expression2);
        }
    }

    fn expression2(&mut self, input: &Expression2) {
        self.factor(&input.factor);
    }

    fn factor(&mut self, input: &Factor) {
        match input {
            Factor::Factor0(x) => self.number(&x.number),
            Factor::Factor1(x) => {
                self.identifier(&x.identifier);
                if let Some(ref x) = x.factor_opt {
                    self.range(&x.range);
                }
            }
            Factor::Factor2(x) => {
                self.expression(&x.expression);
            }
        }
    }

    // ----------------------------------------------------------------------------
    // Statement
    // ----------------------------------------------------------------------------

    fn statement(&mut self, input: &Statement) {
        match input {
            Statement::Statement0(x) => self.assignment_statement(&x.assignment_statement),
            Statement::Statement1(x) => self.if_statement(&x.if_statement),
        }
    }

    fn assignment_statement(&mut self, input: &AssignmentStatement) {
        self.identifier(&input.identifier);
        self.expression(&input.expression);
    }

    fn if_statement(&mut self, input: &IfStatement) {
        self.expression(&input.expression);
        self.statement(&input.statement);
        for x in &input.if_statement_list {
            self.expression(&x.expression);
            self.statement(&x.statement);
        }
        if let Some(ref x) = input.if_statement_opt {
            self.statement(&x.statement);
        }
    }

    // ----------------------------------------------------------------------------
    // Range / Width
    // ----------------------------------------------------------------------------

    fn range(&mut self, input: &Range) {
        self.expression(&input.expression);
        if let Some(ref x) = input.range_opt {
            self.expression(&x.expression);
        }
    }

    fn width(&mut self, input: &Width) {
        self.expression(&input.expression);
    }

    // ----------------------------------------------------------------------------
    // Type
    // ----------------------------------------------------------------------------

    fn r#type(&mut self, input: &Type) {
        let location = match &*input.type_group {
            TypeGroup::TypeGroup0(x) => match &*x.builtin_type {
                BuiltinType::BuiltinType0(x) => &x.logic.logic.location,
                BuiltinType::BuiltinType1(x) => &x.bit.bit.location,
                BuiltinType::BuiltinType2(x) => &x.u32.u32.location,
                BuiltinType::BuiltinType3(x) => &x.u64.u64.location,
                BuiltinType::BuiltinType4(x) => &x.i32.i32.location,
                BuiltinType::BuiltinType5(x) => &x.i64.i64.location,
                BuiltinType::BuiltinType6(x) => &x.f32.f32.location,
                BuiltinType::BuiltinType7(x) => &x.f64.f64.location,
            },
            TypeGroup::TypeGroup1(x) => &x.identifier.identifier.location,
        };
        self.r#type.update(location.into());
    }

    fn identifier(&mut self, x: &Identifier) {
        self.identifier.update((&x.identifier.location).into());
    }

    // ----------------------------------------------------------------------------
    // WithParameter
    // ----------------------------------------------------------------------------

    fn with_parameter(&mut self, input: &WithParameter) {
        if let Some(ref x) = input.with_parameter_opt {
            self.with_parameter_list(&x.with_parameter_list);
        }
    }

    fn with_parameter_list(&mut self, input: &WithParameterList) {
        self.with_parameter_item(&input.with_parameter_item);
        for x in &input.with_parameter_list_list {
            self.with_parameter_item(&x.with_parameter_item);
        }
    }

    fn with_parameter_item(&mut self, input: &WithParameterItem) {
        self.identifier(&input.identifier);
        self.r#type(&input.r#type);
        self.expression(&input.expression);
    }

    // ----------------------------------------------------------------------------
    // Module
    // ----------------------------------------------------------------------------

    fn module_declaration(&mut self, input: &ModuleDeclaration) {
        if let Some(ref x) = input.module_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = input.module_declaration_opt0 {
            self.module_port(&x.module_port);
        }
        for x in &input.module_declaration_list {
            self.module_item(&x.module_item);
        }
    }

    fn module_port(&mut self, input: &ModulePort) {
        if let Some(ref x) = input.module_port_opt {
            self.module_port_list(&x.module_port_list);
        }
    }

    fn module_port_list(&mut self, input: &ModulePortList) {
        self.module_port_item(&input.module_port_item);
        for x in &input.module_port_list_list {
            self.module_port_item(&x.module_port_item);
        }
    }

    fn module_port_item(&mut self, input: &ModulePortItem) {
        self.identifier(&input.identifier);
        self.direction(&input.direction);
        self.r#type(&input.r#type);
    }

    fn module_item(&mut self, input: &ModuleItem) {
        match input {
            ModuleItem::ModuleItem0(x) => self.variable_declaration(&x.variable_declaration),
            ModuleItem::ModuleItem1(x) => self.parameter_declaration(&x.parameter_declaration),
            ModuleItem::ModuleItem2(x) => self.localparam_declaration(&x.localparam_declaration),
            ModuleItem::ModuleItem3(x) => self.always_f_f_declaration(&x.always_f_f_declaration),
            ModuleItem::ModuleItem4(x) => self.always_comb_declaration(&x.always_comb_declaration),
        }
    }

    fn direction(&mut self, _input: &Direction) {}

    // ----------------------------------------------------------------------------
    // Interface
    // ----------------------------------------------------------------------------

    fn interface_declaration(&mut self, input: &InterfaceDeclaration) {
        if let Some(ref x) = input.interface_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        for x in &input.interface_declaration_list {
            self.interface_item(&x.interface_item);
        }
    }

    fn interface_item(&mut self, input: &InterfaceItem) {
        match input {
            InterfaceItem::InterfaceItem0(x) => self.variable_declaration(&x.variable_declaration),
            InterfaceItem::InterfaceItem1(x) => {
                self.parameter_declaration(&x.parameter_declaration)
            }
            InterfaceItem::InterfaceItem2(x) => {
                self.localparam_declaration(&x.localparam_declaration)
            }
            InterfaceItem::InterfaceItem3(x) => self.modport_declaration(&x.modport_declaration),
        }
    }

    // ----------------------------------------------------------------------------
    // Declaration
    // ----------------------------------------------------------------------------

    fn variable_declaration(&mut self, input: &VariableDeclaration) {
        self.identifier(&input.identifier);
        self.r#type(&input.r#type);
    }

    fn parameter_declaration(&mut self, input: &ParameterDeclaration) {
        self.identifier(&input.identifier);
        self.r#type(&input.r#type);
        self.expression(&input.expression);
    }

    fn localparam_declaration(&mut self, input: &LocalparamDeclaration) {
        self.identifier(&input.identifier);
        self.r#type(&input.r#type);
        self.expression(&input.expression);
    }

    fn always_f_f_declaration(&mut self, input: &AlwaysFFDeclaration) {
        self.always_f_f_conditions(&input.always_f_f_conditions);
        for x in &input.always_f_f_declaration_list {
            self.statement(&x.statement);
        }
    }

    fn always_f_f_conditions(&mut self, input: &AlwaysFFConditions) {
        self.always_f_f_condition(&input.always_f_f_condition);
        for x in &input.always_f_f_conditions_list {
            self.always_f_f_condition(&x.always_f_f_condition);
        }
    }

    fn always_f_f_condition(&mut self, _input: &AlwaysFFCondition) {}

    fn always_comb_declaration(&mut self, input: &AlwaysCombDeclaration) {
        for x in &input.always_comb_declaration_list {
            self.statement(&x.statement);
        }
    }

    fn modport_declaration(&mut self, input: &ModportDeclaration) {
        self.identifier(&input.identifier);
        self.modport_list(&input.modport_list);
    }

    fn modport_list(&mut self, input: &ModportList) {
        self.modport_item(&input.modport_item);
        for x in &input.modport_list_list {
            self.modport_item(&x.modport_item);
        }
    }

    fn modport_item(&mut self, input: &ModportItem) {
        self.identifier(&input.identifier);
        self.direction(&input.direction);
    }
}
