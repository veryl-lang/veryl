use crate::analyzer_error::AnalyzerError;
use veryl_parser::ParolError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};

#[derive(Default)]
pub struct CheckPort {
    pub errors: Vec<AnalyzerError>,
    point: HandlerPoint,
    in_function: bool,
    in_module: bool,
    in_modport: bool,
}

impl CheckPort {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for CheckPort {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckPort {
    fn port_declaration_item(&mut self, arg: &PortDeclarationItem) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            if let PortDeclarationItemGroup::PortTypeConcrete(x) =
                arg.port_declaration_item_group.as_ref()
            {
                let x = x.port_type_concrete.as_ref();
                let direction = x.direction.as_ref();
                if let Direction::Inout(_) = direction {
                    let r#type = &x.array_type;
                    let is_tri = r#type
                        .scalar_type
                        .scalar_type_list
                        .iter()
                        .any(|x| matches!(x.type_modifier.as_ref(), TypeModifier::Tri(_)));

                    if !is_tri {
                        self.errors
                            .push(AnalyzerError::missing_tri(&r#type.as_ref().into()));
                    }
                }

                if let Some(x) = &x.port_type_concrete_opt0 {
                    let is_valid_port_default_value = match direction {
                        Direction::Input(_) => {
                            // For now, port default value is allowed for module only.
                            // https://github.com/veryl-lang/veryl/issues/1178#issuecomment-2568996379
                            !self.in_function
                        }
                        Direction::Output(_) => {
                            // For SystemVerilog, output ports of a function cannot be released.
                            !self.in_function
                                && x.port_default_value.expression.is_anonymous_expression()
                        }
                        _ => false,
                    };
                    if !is_valid_port_default_value {
                        self.errors.push(AnalyzerError::invalid_port_default_value(
                            &arg.identifier.identifier_token.to_string(),
                            &direction.to_string(),
                            &x.port_default_value.expression.as_ref().into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn direction(&mut self, arg: &Direction) -> Result<(), ParolError> {
        if let HandlerPoint::Before = self.point {
            match arg {
                Direction::Modport(x) => {
                    if !self.in_module || self.in_function {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "modport",
                            &x.modport.modport_token.token.into(),
                        ));
                    }
                }
                Direction::Import(x) => {
                    if !self.in_modport {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "import",
                            &x.import.import_token.token.into(),
                        ));
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }

    fn function_declaration(&mut self, _arg: &FunctionDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_function = true,
            HandlerPoint::After => self.in_function = false,
        }
        Ok(())
    }

    fn module_declaration(&mut self, _arg: &ModuleDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_module = true,
            HandlerPoint::After => self.in_module = false,
        }
        Ok(())
    }

    fn modport_declaration(&mut self, _arg: &ModportDeclaration) -> Result<(), ParolError> {
        match self.point {
            HandlerPoint::Before => self.in_modport = true,
            HandlerPoint::After => self.in_modport = false,
        }
        Ok(())
    }
}
