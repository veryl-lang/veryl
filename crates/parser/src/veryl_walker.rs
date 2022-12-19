use crate::veryl_grammar_trait::*;

pub trait VerylWalker {
    // ----------------------------------------------------------------------------
    // Terminals
    // ----------------------------------------------------------------------------

    fn identifier(&mut self, input: &Identifier);

    // ----------------------------------------------------------------------------
    // Number
    // ----------------------------------------------------------------------------

    fn number(&mut self, input: &Number);

    // ----------------------------------------------------------------------------
    // Expression
    // ----------------------------------------------------------------------------

    fn expression(&mut self, input: &Expression);

    fn expression1(&mut self, input: &Expression1);

    fn factor(&mut self, input: &Factor);

    // ----------------------------------------------------------------------------
    // Range / Width
    // ----------------------------------------------------------------------------

    fn range(&mut self, input: &Range);

    fn width(&mut self, input: &Width);

    // ----------------------------------------------------------------------------
    // Type
    // ----------------------------------------------------------------------------

    fn r#type(&mut self, input: &Type);

    // ----------------------------------------------------------------------------
    // Statement
    // ----------------------------------------------------------------------------

    fn statement(&mut self, input: &Statement);

    fn assignment_statement(&mut self, input: &AssignmentStatement);

    fn if_statement(&mut self, input: &IfStatement);

    // ----------------------------------------------------------------------------
    // Declaration
    // ----------------------------------------------------------------------------

    fn variable_declaration(&mut self, input: &VariableDeclaration);

    fn parameter_declaration(&mut self, input: &ParameterDeclaration);

    fn localparam_declaration(&mut self, input: &LocalparamDeclaration);

    fn always_ff_declaration(&mut self, input: &AlwaysFfDeclaration);

    fn always_ff_conditions(&mut self, input: &AlwaysFfConditions);

    fn always_ff_condition(&mut self, input: &AlwaysFfCondition);

    fn always_comb_declaration(&mut self, input: &AlwaysCombDeclaration);

    fn assign_declaration(&mut self, input: &AssignDeclaration);

    fn modport_declaration(&mut self, input: &ModportDeclaration);

    fn modport_list(&mut self, input: &ModportList);

    fn modport_item(&mut self, input: &ModportItem);

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
    // Description
    // ----------------------------------------------------------------------------

    fn description(&mut self, input: &Description);

    // ----------------------------------------------------------------------------
    // SourceCode
    // ----------------------------------------------------------------------------

    fn veryl(&mut self, input: &Veryl);
}
