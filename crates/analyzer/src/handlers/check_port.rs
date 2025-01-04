use crate::analyzer_error::AnalyzerError;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_walker::{Handler, HandlerPoint};
use veryl_parser::ParolError;

#[derive(Default)]
pub struct CheckPort<'a> {
    pub errors: Vec<AnalyzerError>,
    text: &'a str,
    point: HandlerPoint,
    in_function: bool,
    in_module: bool,
    in_modport: bool,
}

impl<'a> CheckPort<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

impl Handler for CheckPort<'_> {
    fn set_point(&mut self, p: HandlerPoint) {
        self.point = p;
    }
}

impl VerylGrammarTrait for CheckPort<'_> {
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
                        self.errors.push(AnalyzerError::missing_tri(
                            self.text,
                            &r#type.as_ref().into(),
                        ));
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
                                && is_anonymous_expression(&x.port_default_value.expression)
                        }
                        _ => false,
                    };
                    if !is_valid_port_default_value {
                        self.errors.push(AnalyzerError::invalid_port_default_value(
                            &arg.identifier.identifier_token.to_string(),
                            &direction.to_string(),
                            self.text,
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
                Direction::Ref(x) => {
                    if !self.in_function {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "ref",
                            self.text,
                            &x.r#ref.ref_token.token.into(),
                        ));
                    }
                }
                Direction::Modport(x) => {
                    if !self.in_module || self.in_function {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "modport",
                            self.text,
                            &x.modport.modport_token.token.into(),
                        ));
                    }
                }
                Direction::Import(x) => {
                    if !self.in_modport {
                        self.errors.push(AnalyzerError::invalid_direction(
                            "import",
                            self.text,
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
