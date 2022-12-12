use crate::veryl_grammar_trait::*;

pub trait VerylWalker {
    // ----------------------------------------------------------------------------
    // SourceCode
    // ----------------------------------------------------------------------------

    fn veryl(&mut self, input: &Veryl);

    // ----------------------------------------------------------------------------
    // Description
    // ----------------------------------------------------------------------------

    fn description(&mut self, input: &Description);

    // ----------------------------------------------------------------------------
    // Number
    // ----------------------------------------------------------------------------

    fn number(&mut self, input: &Number);

    // ----------------------------------------------------------------------------
    // Expression
    // ----------------------------------------------------------------------------

    fn expression(&mut self, input: &Expression);

    fn expression0(&mut self, input: &Expression0);

    fn expression1(&mut self, input: &Expression1);

    fn expression2(&mut self, input: &Expression2);

    fn factor(&mut self, input: &Factor);

    // ----------------------------------------------------------------------------
    // Statement
    // ----------------------------------------------------------------------------

    fn statement(&mut self, input: &Statement);

    fn assignment_statement(&mut self, input: &AssignmentStatement);

    fn if_statement(&mut self, input: &IfStatement);

    // ----------------------------------------------------------------------------
    // Range / Width
    // ----------------------------------------------------------------------------

    fn range(&mut self, input: &Range);

    fn width(&mut self, input: &Width);

    // ----------------------------------------------------------------------------
    // Type
    // ----------------------------------------------------------------------------

    fn r#type(&mut self, input: &Type);

    fn identifier(&mut self, x: &Identifier);

    // ----------------------------------------------------------------------------
    // WithParameter
    // ----------------------------------------------------------------------------

    fn with_parameter(&mut self, input: &WithParameter);

    fn with_parameter_list(&mut self, input: &WithParameterList);

    fn with_parameter_item(&mut self, input: &WithParameterItem);

    // ----------------------------------------------------------------------------
    // Module
    // ----------------------------------------------------------------------------

    fn module_declaration(&mut self, input: &ModuleDeclaration);

    fn module_port(&mut self, input: &ModulePort);

    fn module_port_list(&mut self, input: &ModulePortList);

    fn module_port_item(&mut self, input: &ModulePortItem);

    fn module_item(&mut self, input: &ModuleItem);

    fn direction(&mut self, input: &Direction);

    // ----------------------------------------------------------------------------
    // Interface
    // ----------------------------------------------------------------------------

    fn interface_declaration(&mut self, input: &InterfaceDeclaration);

    fn interface_item(&mut self, input: &InterfaceItem);

    // ----------------------------------------------------------------------------
    // Declaration
    // ----------------------------------------------------------------------------

    fn variable_declaration(&mut self, input: &VariableDeclaration);

    fn parameter_declaration(&mut self, input: &ParameterDeclaration);

    fn localparam_declaration(&mut self, input: &LocalparamDeclaration);

    fn always_f_f_declaration(&mut self, input: &AlwaysFFDeclaration);

    fn always_f_f_conditions(&mut self, input: &AlwaysFFConditions);

    fn always_f_f_condition(&mut self, input: &AlwaysFFCondition);

    fn always_comb_declaration(&mut self, input: &AlwaysCombDeclaration);

    fn modport_declaration(&mut self, input: &ModportDeclaration);

    fn modport_list(&mut self, input: &ModportList);

    fn modport_item(&mut self, input: &ModportItem);
}
