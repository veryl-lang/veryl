use crate::veryl_error::VerylError;
use crate::veryl_grammar_trait::*;
use crate::veryl_token::VerylToken;
use crate::veryl_walker::VerylWalker;

pub struct Analyzer<'a> {
    text: &'a str,
    pub errors: Vec<VerylError>,
}

const BINARY_CHARS: [char; 6] = ['0', '1', 'x', 'z', 'X', 'Z'];
const OCTAL_CHARS: [char; 12] = ['0', '1', '2', '3', '4', '5', '6', '7', 'x', 'z', 'X', 'Z'];
const DECIMAL_CHARS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

impl<'a> Analyzer<'a> {
    pub fn new(text: &'a str) -> Self {
        Analyzer {
            text,
            errors: Vec::new(),
        }
    }

    pub fn analyze(&mut self, input: &Veryl) {
        self.veryl(input);
    }

    fn based(&mut self, token: &VerylToken) {
        let text = token.token.token.text();
        let (width, tail) = text.split_once('\'').unwrap();
        let base = &tail[0..1];
        let number = &tail[1..];

        let width: usize = width.replace('_', "").parse().unwrap();
        let number = number.replace('_', "");
        let number = number.trim_start_matches('0');

        match base {
            "b" => {
                if let Some(x) = number.chars().filter(|x| !BINARY_CHARS.contains(&x)).next() {
                    self.errors.push(VerylError::invalid_number_character(
                        x, "binary", &self.text, token,
                    ));
                }
                let actual_width = number.chars().count();
                if actual_width > width {
                    self.errors
                        .push(VerylError::number_overflow(width, &self.text, token));
                }
            }
            "o" => {
                if let Some(x) = number.chars().filter(|x| !OCTAL_CHARS.contains(&x)).next() {
                    self.errors.push(VerylError::invalid_number_character(
                        x, "octal", &self.text, token,
                    ));
                }
                let mut actual_width = number.chars().count() * 3;
                match number.chars().next() {
                    Some('1') => actual_width -= 2,
                    Some('2') => actual_width -= 1,
                    Some('3') => actual_width -= 1,
                    _ => (),
                }
                if actual_width > width {
                    self.errors
                        .push(VerylError::number_overflow(width, &self.text, token));
                }
            }
            "d" => {
                if let Some(x) = number
                    .chars()
                    .filter(|x| !DECIMAL_CHARS.contains(&x))
                    .next()
                {
                    self.errors.push(VerylError::invalid_number_character(
                        x, "decimal", &self.text, token,
                    ));
                }
            }
            "h" => {
                let mut actual_width = number.chars().count() * 4;
                match number.chars().next() {
                    Some('1') => actual_width -= 3,
                    Some('2') => actual_width -= 2,
                    Some('3') => actual_width -= 2,
                    Some('4') => actual_width -= 1,
                    Some('5') => actual_width -= 1,
                    Some('6') => actual_width -= 1,
                    Some('7') => actual_width -= 1,
                    _ => (),
                }
                if actual_width > width {
                    self.errors
                        .push(VerylError::number_overflow(width, &self.text, token));
                }
            }
            _ => unreachable!(),
        }
    }
}

impl<'a> VerylWalker for Analyzer<'a> {
    // ----------------------------------------------------------------------------
    // Terminals
    // ----------------------------------------------------------------------------

    fn identifier(&mut self, _input: &Identifier) {}

    // ----------------------------------------------------------------------------
    // Number
    // ----------------------------------------------------------------------------

    fn number(&mut self, input: &Number) {
        match input {
            Number::Number0(x) => match &*x.integral_number {
                IntegralNumber::IntegralNumber0(x) => self.based(&x.based.based_token),
                IntegralNumber::IntegralNumber1(_) => (),
                IntegralNumber::IntegralNumber2(_) => (),
            },
            Number::Number1(x) => match &*x.real_number {
                RealNumber::RealNumber0(_) => (),
                RealNumber::RealNumber1(_) => (),
            },
        };
    }

    // ----------------------------------------------------------------------------
    // Expression
    // ----------------------------------------------------------------------------

    fn expression(&mut self, input: &Expression) {
        self.expression1(&input.expression1);
        for x in &input.expression_list {
            self.expression1(&x.expression1);
        }
    }

    fn expression1(&mut self, input: &Expression1) {
        self.factor(&input.factor);
    }

    fn factor(&mut self, input: &Factor) {
        match input {
            Factor::Factor0(x) => self.number(&x.number),
            Factor::Factor1(x) => {
                self.identifier(&x.identifier);
                for x in &x.factor_list {
                    self.range(&x.range);
                }
            }
            Factor::Factor2(x) => {
                self.expression(&x.expression);
            }
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
        for x in &input.type_list {
            self.width(&x.width);
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

    fn always_ff_declaration(&mut self, input: &AlwaysFfDeclaration) {
        self.always_ff_conditions(&input.always_ff_conditions);
        for x in &input.always_ff_declaration_list {
            self.statement(&x.statement);
        }
    }

    fn always_ff_conditions(&mut self, input: &AlwaysFfConditions) {
        self.always_ff_condition(&input.always_ff_condition);
        for x in &input.always_ff_conditions_list {
            self.always_ff_condition(&x.always_ff_condition);
        }
    }

    fn always_ff_condition(&mut self, input: &AlwaysFfCondition) {
        self.identifier(&input.identifier);
    }

    fn always_comb_declaration(&mut self, input: &AlwaysCombDeclaration) {
        for x in &input.always_comb_declaration_list {
            self.statement(&x.statement);
        }
    }

    fn assign_declaration(&mut self, input: &AssignDeclaration) {
        self.identifier(&input.identifier);
        if let Some(ref x) = input.assign_declaration_opt {
            self.r#type(&x.r#type);
        }
        self.expression(&input.expression);
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
        self.identifier(&input.identifier);
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
            ModuleItem::ModuleItem3(x) => self.always_ff_declaration(&x.always_ff_declaration),
            ModuleItem::ModuleItem4(x) => self.always_comb_declaration(&x.always_comb_declaration),
            ModuleItem::ModuleItem5(x) => self.assign_declaration(&x.assign_declaration),
        }
    }

    fn direction(&mut self, _input: &Direction) {}

    // ----------------------------------------------------------------------------
    // Interface
    // ----------------------------------------------------------------------------

    fn interface_declaration(&mut self, input: &InterfaceDeclaration) {
        self.identifier(&input.identifier);
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
    // Description
    // ----------------------------------------------------------------------------

    fn description(&mut self, input: &Description) {
        match input {
            Description::Description0(x) => self.module_declaration(&x.module_declaration),
            Description::Description1(x) => self.interface_declaration(&x.interface_declaration),
        }
    }

    // ----------------------------------------------------------------------------
    // SourceCode
    // ----------------------------------------------------------------------------

    fn veryl(&mut self, input: &Veryl) {
        for x in &input.veryl_list {
            self.description(&x.description);
        }
    }
}
