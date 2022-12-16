use crate::veryl_grammar_trait::*;

pub trait VerylWalker {
    // ----------------------------------------------------------------------------
    // Terminals
    // ----------------------------------------------------------------------------

    fn identifier(&mut self, input: &Identifier);

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

    fn expression00(&mut self, input: &Expression00);

    fn expression01(&mut self, input: &Expression01);

    fn expression02(&mut self, input: &Expression02);

    fn expression03(&mut self, input: &Expression03);

    fn expression04(&mut self, input: &Expression04);

    fn expression05(&mut self, input: &Expression05);

    fn expression06(&mut self, input: &Expression06);

    fn expression07(&mut self, input: &Expression07);

    fn expression08(&mut self, input: &Expression08);

    fn expression09(&mut self, input: &Expression09);

    fn expression10(&mut self, input: &Expression10);

    fn expression11(&mut self, input: &Expression11);

    fn expression12(&mut self, input: &Expression12);

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

    fn always_ff_declaration(&mut self, input: &AlwaysFfDeclaration);

    fn always_ff_conditions(&mut self, input: &AlwaysFfConditions);

    fn always_ff_condition(&mut self, input: &AlwaysFfCondition);

    fn always_comb_declaration(&mut self, input: &AlwaysCombDeclaration);

    fn assign_declaration(&mut self, input: &AssignDeclaration);

    fn modport_declaration(&mut self, input: &ModportDeclaration);

    fn modport_list(&mut self, input: &ModportList);

    fn modport_item(&mut self, input: &ModportItem);
}
